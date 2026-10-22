//! Fetching documents over the network: one HTTP agent per thread, the
//! redirect and address rules, the size, time and request limits, and the
//! layout of the cache that fetched documents are kept in.
//!
//! Every web request of the server goes through [`get`], which:
//! - follows at most [`MAX_REDIRECTS`] redirects itself (the agent follows
//!   none), so each hop is checked: only `http` and `https`, never `https` to
//!   `http`, and, unless [`Policy::allow_private`] is set, never to a
//!   loopback, private, link-local (cloud metadata) or other internal
//!   address. A name is checked after DNS has answered (see `Guarded`), on
//!   the very addresses the connection then uses;
//! - counts every request, redirects included, and every byte against a
//!   [`Budget`], so one pass over the imports cannot fetch without end;
//! - reads at most [`MAX_FETCH_BYTES`] per document, and spends at most
//!   [`FETCH_TIMEOUT`] on it.
//!
//! The cache keeps a fetched document in `<sha256 of the URL>.<ext>`. The URL
//! is written into the file ([`with_header`], [`header_matches`]) and checked
//! when the file is read, so a file that another URL produced, or that someone
//! planted, is not taken for this one. Files are written to a name of their
//! own and renamed ([`write_atomic`]).
//!
//! The browser build has no network, so this module is not part of it.

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

/// The most bytes read from one document.
pub(crate) const MAX_FETCH_BYTES: u64 = 20 * 1024 * 1024;
/// The most time spent on one document, redirects included.
pub(crate) const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Redirects followed for one document.
pub(crate) const MAX_REDIRECTS: u32 = 10;
/// Requests one pass over the imports may make.
pub(crate) const PASS_REQUESTS: u32 = 100;
/// Bytes one pass over the imports may read.
pub(crate) const PASS_BYTES: u64 = 100 * 1024 * 1024;
/// Set to `1` to fetch from loopback, private, link-local and other internal
/// addresses (a server on the intranet, a vocabulary on this machine).
pub(crate) const ALLOW_PRIVATE_ENV: &str = "TURTLE_LSP_ALLOW_PRIVATE_HOSTS";

/// Which addresses may be fetched from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Policy {
    /// Loopback, private, link-local (cloud metadata) and other internal
    /// addresses are fetched from too.
    pub(crate) allow_private: bool,
}

impl Policy {
    /// Public addresses only, unless [`ALLOW_PRIVATE_ENV`] says otherwise.
    pub(crate) fn from_env() -> Policy {
        Policy::from_setting(std::env::var_os(ALLOW_PRIVATE_ENV).as_deref())
    }

    /// The policy that the value of [`ALLOW_PRIVATE_ENV`] asks for: `1` or
    /// `true` lets internal addresses through, anything else does not.
    fn from_setting(value: Option<&std::ffi::OsStr>) -> Policy {
        Policy {
            allow_private: value.is_some_and(|v| matches!(v.to_str(), Some("1" | "true"))),
        }
    }
}

/// What one pass (or one document) may still request and read.
#[derive(Clone, Debug)]
pub(crate) struct Budget {
    requests: u32,
    bytes: u64,
    max_requests: u32,
    max_bytes: u64,
}

impl Budget {
    pub(crate) fn new(max_requests: u32, max_bytes: u64) -> Budget {
        Budget {
            requests: 0,
            bytes: 0,
            max_requests,
            max_bytes,
        }
    }

    /// One pass over a workspace's imports.
    pub(crate) fn pass() -> Budget {
        Budget::new(PASS_REQUESTS, PASS_BYTES)
    }

    /// One document with its redirects and the one document it links to.
    pub(crate) fn document() -> Budget {
        Budget::new(2 * (MAX_REDIRECTS + 1), 2 * MAX_FETCH_BYTES)
    }

    fn spent(&self) -> Error {
        Error {
            status: None,
            budget: true,
            message: format!(
                "the fetch budget of this pass is spent ({} of {} requests, {} of {} MB read)",
                self.requests,
                self.max_requests,
                self.bytes / (1024 * 1024),
                self.max_bytes / (1024 * 1024)
            ),
        }
    }

    fn request(&mut self) -> Result<(), Error> {
        if self.requests >= self.max_requests {
            return Err(self.spent());
        }
        self.requests += 1;
        Ok(())
    }

    fn left(&self) -> u64 {
        self.max_bytes.saturating_sub(self.bytes)
    }
}

/// A document read by [`get`].
#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) body: Vec<u8>,
    pub(crate) content_type: Option<String>,
    /// Where the body was finally read from, after redirects.
    pub(crate) final_url: String,
    /// The `Link` header, if any.
    pub(crate) link: Option<String>,
}

/// Why [`get`] failed.
#[derive(Debug)]
pub(crate) struct Error {
    /// The HTTP status, if the server answered with an error.
    pub(crate) status: Option<u16>,
    pub(crate) message: String,
    /// The [`Budget`] was spent: another pass may do better, so this failure
    /// is not remembered.
    pub(crate) budget: bool,
}

impl Error {
    fn new(message: impl Into<String>) -> Error {
        Error {
            status: None,
            budget: false,
            message: message.into(),
        }
    }
}

/// GET `url` with `accept`: follow up to [`MAX_REDIRECTS`] redirects, each
/// checked against `policy`, and count them in `budget`.
pub(crate) fn get(
    url: &str,
    accept: &str,
    policy: Policy,
    budget: &mut Budget,
) -> Result<Response, Error> {
    get_from(url, accept, policy, budget, false)
}

/// [`get`]; `vetted` when `url` itself is not checked against the policy (a
/// caller that has chosen where to start; its redirects still are).
fn get_from(
    url: &str,
    accept: &str,
    policy: Policy,
    budget: &mut Budget,
    vetted: bool,
) -> Result<Response, Error> {
    let deadline = Instant::now() + FETCH_TIMEOUT;
    let mut current = url::Url::parse(url).map_err(|e| Error::new(format!("{url}: {e}")))?;
    let mut hops = 0;
    loop {
        let policy = if hops == 0 && vetted {
            Policy {
                allow_private: true,
            }
        } else {
            policy
        };
        check_target(&current, policy)?;
        budget.request()?;
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| Error::new(format!("{url} took longer than the time allowed")))?;
        let mut resp = with_agent(policy, |agent| {
            agent
                .get(current.as_str())
                .header("Accept", accept)
                .config()
                .timeout_global(Some(left))
                .build()
                .call()
        })
        .map_err(failed)?;
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = header("location").ok_or_else(|| Error {
                status: Some(status),
                budget: false,
                message: format!("{current} redirects without saying where"),
            })?;
            if hops >= MAX_REDIRECTS {
                return Err(Error::new(format!(
                    "{url} redirects more than {MAX_REDIRECTS} times"
                )));
            }
            hops += 1;
            let next = current
                .join(&location)
                .map_err(|e| Error::new(format!("{current} redirects to {location}: {e}")))?;
            redirect_allowed(&current, &next)?;
            current = next;
            continue;
        }
        let content_type = header("content-type");
        let link = header("link");
        let limit = budget.left().min(MAX_FETCH_BYTES);
        let body = resp
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) if limit < MAX_FETCH_BYTES => budget.spent(),
                ureq::Error::BodyExceedsLimit(_) => Error::new(format!(
                    "{current} is larger than {} MB",
                    MAX_FETCH_BYTES / (1024 * 1024)
                )),
                e => failed(e),
            })?;
        budget.bytes += body.len() as u64;
        return Ok(Response {
            body,
            content_type,
            final_url: current.to_string(),
            link,
        });
    }
}

fn check_scheme(url: &url::Url) -> Result<(), Error> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(Error::new(format!(
            "{url}: only http and https are fetched, not {other}"
        ))),
    }
}

/// A redirect from `from` to `to` that is never followed.
fn redirect_allowed(from: &url::Url, to: &url::Url) -> Result<(), Error> {
    check_scheme(to)?;
    if from.scheme() == "https" && to.scheme() == "http" {
        return Err(Error::new(format!(
            "{from} redirects from https to http ({to}), which is refused"
        )));
    }
    Ok(())
}

/// Whether `url` may be requested: a web URL whose host, when it is an
/// address or `localhost`, the policy lets through. (A name that resolves to
/// an internal address is stopped by `Guarded` once DNS has answered.)
fn check_target(url: &url::Url, policy: Policy) -> Result<(), Error> {
    check_scheme(url)?;
    if policy.allow_private {
        return Ok(());
    }
    let why = match url.host() {
        Some(url::Host::Ipv4(ip)) => internal(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => internal(IpAddr::V6(ip)),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            (name == "localhost" || name.ends_with(".localhost")).then_some("loopback")
        }
        None => Some("missing"),
    };
    match why {
        Some(why) => Err(refused(url.as_str(), why)),
        None => Ok(()),
    }
}

fn refused(what: &str, why: &str) -> Error {
    Error::new(format!(
        "{what} is a {why} address, which is not fetched from (set {ALLOW_PRIVATE_ENV}=1 to allow it)"
    ))
}

/// What makes `ip` an address that is not fetched from, if anything.
pub(crate) fn internal(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(ip) => internal_v4(ip),
        IpAddr::V6(ip) => internal_v6(ip),
    }
}

fn internal_v4(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, ..] = ip.octets();
    if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_unspecified() || a == 0 {
        Some("unspecified")
    } else if ip == Ipv4Addr::new(169, 254, 169, 254) {
        Some("cloud metadata")
    } else if ip.is_link_local() {
        Some("link-local")
    } else if ip.is_private() {
        Some("private")
    } else if a == 100 && (64..128).contains(&b) {
        Some("shared (carrier-grade NAT)")
    } else if ip.is_multicast() || ip.is_broadcast() {
        Some("multicast")
    } else {
        None
    }
}

fn internal_v6(ip: Ipv6Addr) -> Option<&'static str> {
    let s = ip.segments();
    let v4 = |hi: u16, lo: u16| {
        internal_v4(Ipv4Addr::new(
            (hi >> 8) as u8,
            hi as u8,
            (lo >> 8) as u8,
            lo as u8,
        ))
    };
    if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_unspecified() {
        Some("unspecified")
    } else if s[..5] == [0; 5] && s[5] == 0xffff {
        // An IPv4 address in an IPv6 one (mapped, compatible, NAT64) is that address.
        v4(s[6], s[7])
    } else if s[..6] == [0; 6] || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        v4(s[6], s[7])
    } else if s[0] & 0xfe00 == 0xfc00 {
        Some("private")
    } else if s[0] & 0xffc0 == 0xfe80 {
        Some("link-local")
    } else if ip.is_multicast() {
        Some("multicast")
    } else {
        None
    }
}

/// Resolves names as the system does and drops the addresses the policy
/// refuses, so the connection can only go to an address that was checked: the
/// check and the connect share one answer from DNS.
#[derive(Debug)]
struct Guarded {
    policy: Policy,
}

impl Resolver for Guarded {
    fn resolve(
        &self,
        uri: &Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let all = DefaultResolver::default().resolve(uri, config, timeout)?;
        // The proxy a user configured is theirs to choose; the proxy then
        // resolves the target itself (and is the one to apply a policy).
        let is_proxy = config
            .proxy()
            .is_some_and(|p| p.uri().authority() == uri.authority());
        if self.policy.allow_private || is_proxy {
            return Ok(all);
        }
        let mut kept = self.empty();
        let mut why = None;
        for addr in all.iter() {
            match internal(addr.ip()) {
                None => kept.push(*addr),
                Some(w) => why = Some((addr.ip(), w)),
            }
        }
        match (kept.is_empty(), why) {
            (true, Some((ip, why))) => Err(ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                refused(
                    &format!("{} (resolves to {ip})", uri.host().unwrap_or("the host")),
                    why,
                )
                .message,
            ))),
            _ => Ok(kept),
        }
    }
}

thread_local! {
    /// The thread's agents, one per policy: connections are reused between
    /// requests of one thread, and no agent is shared between threads.
    static AGENTS: RefCell<Vec<(Policy, ureq::Agent)>> = const { RefCell::new(Vec::new()) };
}

fn with_agent<R>(policy: Policy, f: impl FnOnce(&ureq::Agent) -> R) -> R {
    AGENTS.with(|agents| {
        let mut agents = agents.borrow_mut();
        let at = match agents.iter().position(|(p, _)| *p == policy) {
            Some(at) => at,
            None => {
                agents.push((policy, agent(policy)));
                agents.len() - 1
            }
        };
        f(&agents[at].1)
    })
}

fn agent(policy: Policy) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        // The redirects are followed by `get`, hop by hop.
        .max_redirects(0)
        .user_agent(concat!("turtle-lsp/", env!("CARGO_PKG_VERSION")))
        .build();
    ureq::Agent::with_parts(config, DefaultConnector::default(), Guarded { policy })
}

fn failed(e: ureq::Error) -> Error {
    match e {
        ureq::Error::StatusCode(s) => Error {
            status: Some(s),
            budget: false,
            message: ureq::Error::StatusCode(s).to_string(),
        },
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied => {
            Error::new(io.to_string())
        }
        e => Error::new(e.to_string()),
    }
}

// ---- cache ----

/// SHA-256 of `data`.
fn sha256(data: &[u8]) -> [u8; 32] {
    #[rustfmt::skip]
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    #[rustfmt::skip]
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    let (blocks, _) = msg.as_chunks::<64>();
    for block in blocks {
        let mut w = [0u32; 64];
        let (words, _) = block.as_chunks::<4>();
        for (i, word) in words.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    let (chunks, _) = out.as_chunks_mut::<4>();
    for (chunk, word) in chunks.iter_mut().zip(h) {
        *chunk = word.to_be_bytes();
    }
    out
}

/// SHA-256 of `s` as 64 hex digits: the name of `s` in the cache.
pub(crate) fn hash_name(s: &str) -> String {
    use std::fmt::Write;
    sha256(s.as_bytes())
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The cache file for `url` holding a document of kind `extension`.
pub(crate) fn cache_file_as(dir: &Path, url: &str, extension: &str) -> PathBuf {
    dir.join(format!("{}.{extension}", hash_name(url)))
}

/// Whether `url` can be written into a cache file's header line.
pub(crate) fn cacheable(url: &str) -> bool {
    !url.chars().any(char::is_control)
}

/// The line that starts a cached document, naming the URL it came from.
pub(crate) fn header_line(url: &str) -> String {
    format!("# turtle-lsp cache for {url}")
}

/// `text` as a cache file for `url`: the header line, then the text.
pub(crate) fn with_header(url: &str, text: &str) -> String {
    format!("{}\n{text}", header_line(url))
}

/// Whether `bytes` start with the header line of `url`.
pub(crate) fn header_matches(bytes: &[u8], url: &str) -> bool {
    let line = header_line(url);
    bytes
        .strip_prefix(line.as_bytes())
        .is_some_and(|rest| rest.is_empty() || rest[0] == b'\n' || rest.starts_with(b"\r\n"))
}

/// The file beside a cached JSON document, which cannot carry a header line:
/// its first line is the header of the URL, its second (if the URL moved) the
/// URL the document was finally read from.
pub(crate) fn sidecar(path: &Path) -> PathBuf {
    path.with_extension("url")
}

/// The text of the [`sidecar`] of a document read from `url` and finally from `final_url`.
pub(crate) fn sidecar_text(url: &str, final_url: &str) -> String {
    if final_url == url {
        format!("{}\n", header_line(url))
    } else {
        format!("{}\n{final_url}\n", header_line(url))
    }
}

/// The URL a [`sidecar`] says the document was finally read from, if the
/// sidecar belongs to `url`: `Some(None)` when it was not redirected.
pub(crate) fn read_sidecar(text: &str, url: &str) -> Option<Option<String>> {
    let mut lines = text.lines();
    if lines.next()? != header_line(url) {
        return None;
    }
    Some(
        lines
            .next()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string),
    )
}

/// Write `bytes` to `path` so that readers see the old file or the whole new
/// one. The temporary file has a name of its own (process, counter), so two
/// writers, in this process or another, never share it.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn loopback() -> Policy {
        Policy {
            allow_private: true,
        }
    }

    /// A local server; `route` answers a request path with the whole HTTP
    /// response. Returns the base URL and the requested paths.
    fn serve(route: impl Fn(&str) -> String + Send + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let path = head.split(' ').nth(1).unwrap_or("").to_string();
                seen.lock().unwrap().push(path.clone());
                let _ = stream.write_all(route(&path).as_bytes());
            }
        });
        (base, log)
    }

    fn ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/turtle\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn redirect(to: &str) -> String {
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }

    fn message(r: Result<Response, Error>) -> String {
        match r {
            Ok(r) => panic!("fetched {}", r.final_url),
            Err(e) => e.message,
        }
    }

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            hash_name(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_name("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash_name("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hash_name(&"a".repeat(1_000_000)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        assert_ne!(hash_name("http://a/"), hash_name("http://b/"));
        assert_eq!(hash_name("x").len(), 64);
    }

    #[test]
    fn only_one_and_true_allow_internal_addresses() {
        let policy = |v: Option<&str>| Policy::from_setting(v.map(std::ffi::OsStr::new));
        assert!(!policy(None).allow_private);
        assert!(policy(Some("1")).allow_private);
        assert!(policy(Some("true")).allow_private);
        for no in ["0", "", "yes", "false", "2"] {
            assert!(!policy(Some(no)).allow_private, "{no}");
        }
        assert_eq!(Policy::default(), policy(None));
    }

    #[test]
    fn internal_addresses_are_named() {
        for (ip, why) in [
            ("127.0.0.1", "loopback"),
            ("127.9.9.9", "loopback"),
            ("0.0.0.0", "unspecified"),
            ("10.1.2.3", "private"),
            ("172.16.0.1", "private"),
            ("172.31.255.255", "private"),
            ("192.168.1.1", "private"),
            ("169.254.169.254", "cloud metadata"),
            ("169.254.1.1", "link-local"),
            ("100.64.0.1", "shared (carrier-grade NAT)"),
            ("224.0.0.1", "multicast"),
            ("::1", "loopback"),
            ("::", "unspecified"),
            ("fd00:ec2::254", "private"),
            ("fe80::1", "link-local"),
            ("::ffff:127.0.0.1", "loopback"),
            ("::ffff:169.254.169.254", "cloud metadata"),
            ("64:ff9b::a00:1", "private"),
            ("ff02::1", "multicast"),
        ] {
            assert_eq!(internal(ip.parse().unwrap()), Some(why), "{ip}");
        }
        for ip in [
            "93.184.216.34",
            "8.8.8.8",
            "172.15.0.1",
            "172.32.0.1",
            "100.63.0.1",
            "100.128.0.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert_eq!(internal(ip.parse().unwrap()), None, "{ip}");
        }
    }

    #[test]
    fn urls_naming_internal_hosts_are_refused_before_any_request() {
        let strict = Policy::default();
        for url in [
            "http://127.0.0.1:1/x",
            "http://[::1]/x",
            "https://10.0.0.5/x",
            "http://169.254.169.254/latest/meta-data/",
            "http://localhost/x",
            "http://LOCALHOST./x",
            "http://api.localhost/x",
            "ftp://example.org/x",
            "file:///etc/passwd",
        ] {
            let mut budget = Budget::pass();
            let e = get(url, "*/*", strict, &mut budget).unwrap_err();
            assert!(!e.budget, "{url}");
            assert_eq!(budget.requests, 0, "{url}: {e:?}");
        }
        let m = message(get(
            "http://127.0.0.1:1/x",
            "*/*",
            strict,
            &mut Budget::pass(),
        ));
        assert!(
            m.contains("loopback") && m.contains(ALLOW_PRIVATE_ENV),
            "{m}"
        );
    }

    #[test]
    fn a_name_that_resolves_to_an_internal_address_is_refused_after_dns() {
        let (base, log) = serve(|_| ok("<a> <b> <c> ."));
        let port = base.rsplit(':').next().unwrap();
        let strict = Policy::default();
        // The resolver is the one place that sees what a name resolves to.
        let uri: Uri = format!("http://127.0.0.1:{port}/").parse().unwrap();
        let config = ureq::config::Config::default();
        let timeout = || NextTimeout {
            after: ureq::unversioned::transport::time::Duration::NotHappening,
            reason: ureq::Timeout::Resolve,
        };
        let err = Guarded { policy: strict }
            .resolve(&uri, &config, timeout())
            .unwrap_err();
        assert!(err.to_string().contains("resolves to 127.0.0.1"), "{err}");
        let all = Guarded { policy: loopback() }
            .resolve(&uri, &config, timeout())
            .unwrap();
        assert_eq!(all.len(), 1);
        // And a request through an agent with that resolver is refused too.
        let e = with_agent(strict, |a| a.get(uri.to_string()).call()).unwrap_err();
        assert!(failed(e).message.contains("loopback"));
        assert!(log.lock().unwrap().is_empty(), "nothing was requested");
    }

    #[test]
    fn a_document_is_fetched_with_its_type_and_final_url() {
        let (base, log) = serve(|path| match path {
            "/a" => ok("<a> <b> <c> ."),
            _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        });
        let mut budget = Budget::pass();
        let r = get(&format!("{base}/a"), "text/turtle", loopback(), &mut budget).unwrap();
        assert_eq!(r.body, b"<a> <b> <c> .");
        assert_eq!(r.content_type.as_deref(), Some("text/turtle"));
        assert_eq!(r.final_url, format!("{base}/a"));
        assert_eq!((budget.requests, budget.bytes), (1, 13));
        let e = get(&format!("{base}/b"), "*/*", loopback(), &mut budget).unwrap_err();
        assert_eq!(e.status, Some(404));
        assert_eq!(*log.lock().unwrap(), ["/a", "/b"]);
    }

    #[test]
    fn redirects_are_followed_hop_by_hop_and_counted() {
        let (base, log) = serve(|path| match path {
            "/one" => redirect("/two"),
            "/two" => redirect("/three"),
            "/three" => ok("<a> <b> <c> ."),
            _ => redirect(path),
        });
        let mut budget = Budget::pass();
        let r = get(&format!("{base}/one"), "*/*", loopback(), &mut budget).unwrap();
        assert_eq!(r.final_url, format!("{base}/three"));
        assert_eq!(budget.requests, 3);
        assert_eq!(*log.lock().unwrap(), ["/one", "/two", "/three"]);
        // A loop stops at MAX_REDIRECTS.
        log.lock().unwrap().clear();
        let m = message(get(
            &format!("{base}/loop"),
            "*/*",
            loopback(),
            &mut Budget::pass(),
        ));
        assert!(m.contains("redirects more than 10 times"), "{m}");
        assert_eq!(log.lock().unwrap().len(), 11);
    }

    #[test]
    fn a_redirect_to_an_internal_address_is_refused_without_a_request() {
        // The first server is trusted (as a caller may vet its start URL);
        // where it sends the client is not.
        let (base, log) = serve(|path| match path {
            "/metadata" => redirect("http://169.254.169.254/latest/meta-data/"),
            "/private" => redirect("http://10.0.0.7:9/secret"),
            "/loopback" => redirect("http://127.0.0.1:9/secret"),
            "/v6" => redirect("http://[::1]:9/secret"),
            "/ftp" => redirect("ftp://example.org/x"),
            _ => ok(""),
        });
        let strict = Policy::default();
        for path in ["metadata", "private", "loopback", "v6"] {
            let mut budget = Budget::pass();
            let m = message(get_from(
                &format!("{base}/{path}"),
                "*/*",
                strict,
                &mut budget,
                true,
            ));
            assert!(m.contains("not fetched from"), "{path}: {m}");
            // Only the first hop was requested.
            assert_eq!(budget.requests, 1, "{path}");
        }
        let m = message(get_from(
            &format!("{base}/ftp"),
            "*/*",
            strict,
            &mut Budget::pass(),
            true,
        ));
        assert!(m.contains("only http and https"), "{m}");
        assert_eq!(log.lock().unwrap().len(), 5);
    }

    #[test]
    fn a_redirect_from_https_to_http_is_refused() {
        let from = url::Url::parse("https://example.org/a").unwrap();
        let down = url::Url::parse("http://example.org/a").unwrap();
        let up = url::Url::parse("https://other.example.org/a").unwrap();
        let m = redirect_allowed(&from, &down).unwrap_err().message;
        assert!(m.contains("https to http"), "{m}");
        assert!(redirect_allowed(&down, &from).is_ok());
        assert!(redirect_allowed(&from, &up).is_ok());
        assert!(redirect_allowed(&from, &url::Url::parse("file:///x").unwrap()).is_err());
    }

    #[test]
    fn the_budget_stops_requests_and_bytes_and_says_so() {
        let (base, log) = serve(|_| ok(&"x".repeat(100)));
        let mut budget = Budget::new(2, 1000);
        for _ in 0..2 {
            get(&format!("{base}/a"), "*/*", loopback(), &mut budget).unwrap();
        }
        let e = get(&format!("{base}/a"), "*/*", loopback(), &mut budget).unwrap_err();
        assert!(e.budget && e.message.contains("2 of 2 requests"), "{e:?}");
        assert_eq!(log.lock().unwrap().len(), 2);
        // Bytes: the body is cut off at what is left.
        let mut budget = Budget::new(10, 150);
        get(&format!("{base}/a"), "*/*", loopback(), &mut budget).unwrap();
        let e = get(&format!("{base}/a"), "*/*", loopback(), &mut budget).unwrap_err();
        assert!(e.budget, "{e:?}");
    }

    #[test]
    fn each_thread_has_one_agent_per_policy() {
        std::thread::spawn(|| {
            with_agent(Policy::default(), |_| {});
            with_agent(Policy::default(), |_| {});
            assert_eq!(AGENTS.with(|a| a.borrow().len()), 1);
            with_agent(loopback(), |_| {});
            assert_eq!(AGENTS.with(|a| a.borrow().len()), 2);
            std::thread::spawn(|| {
                with_agent(loopback(), |_| {});
                assert_eq!(AGENTS.with(|a| a.borrow().len()), 1);
            })
            .join()
            .unwrap();
        })
        .join()
        .unwrap();
    }

    #[test]
    fn a_cache_header_names_its_url() {
        let url = "http://ex.com/o";
        let file = with_header(url, "<a> <b> <c> .\n");
        assert!(header_matches(file.as_bytes(), url));
        assert!(header_matches(
            b"# turtle-lsp cache for http://ex.com/o\r\n<a>",
            url
        ));
        assert!(!header_matches(file.as_bytes(), "http://ex.com/p"));
        // A longer URL with the same start is another URL.
        assert!(!header_matches(file.as_bytes(), "http://ex.com/"));
        assert!(!header_matches(b"<a> <b> <c> .\n", url));
        assert!(cacheable(url) && !cacheable("http://ex.com/a\nb"));
        let text = sidecar_text(url, url);
        assert_eq!(read_sidecar(&text, url), Some(None));
        let text = sidecar_text(url, "http://ex.com/v2/o");
        assert_eq!(
            read_sidecar(&text, url),
            Some(Some("http://ex.com/v2/o".into()))
        );
        assert_eq!(read_sidecar(&text, "http://ex.com/p"), None);
        assert_eq!(read_sidecar("http://ex.com/v2/o\n", url), None);
    }

    #[test]
    fn writers_of_one_name_use_temporary_names_of_their_own() {
        let dir = std::env::temp_dir().join(format!("turtle-lsp-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ttl = cache_file_as(&dir, "http://ex.com/o", "ttl");
        let json = cache_file_as(&dir, "http://ex.com/o", "jsonld");
        // The same stem, two extensions, many threads: the old temporary
        // name (`<stem>.tmp<pid>`) was one for both.
        std::thread::scope(|s| {
            for i in 0..8 {
                let (ttl, json) = (&ttl, &json);
                s.spawn(move || {
                    for n in 0..20 {
                        write_atomic(ttl, format!("ttl {i} {n}").as_bytes()).unwrap();
                        write_atomic(json, format!("json {i} {n}").as_bytes()).unwrap();
                    }
                });
            }
        });
        assert!(std::fs::read_to_string(&ttl).unwrap().starts_with("ttl "));
        assert!(std::fs::read_to_string(&json).unwrap().starts_with("json "));
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(names.len(), 2, "no temporary file is left: {names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
