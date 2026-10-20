//! Where remote `@context` documents come from. Context processing asks a
//! [`ContextLoader`] for each context URL; the loader decides whether a
//! document is available (a file next to the document, the imports cache,
//! the bundled common contexts) and whether the network may be used.
//!
//! The loaders:
//! - [`NoNetwork`] serves only the documents given to it: tests and the W3C
//!   remote-doc runner inject documents through it;
//! - [`Bundled`] serves the common contexts compiled in
//!   (`data/jsonld-contexts`): the browser engine's loader;
//! - [`DesktopLoader`] (not in the browser build) resolves a web URL, in
//!   order, from the imports cache, the bundled contexts, then the network
//!   when allowed, caching what it fetched. The network is used inline (the
//!   command line) or in the background (the language server, whose main
//!   loop must not wait for it: see [`DesktopLoader::in_background`]).
//!
//! [`for_platform`] picks the desktop or the browser loader. `file:` URLs
//! are the workspace's business (open documents first, then files inside
//! the workspace roots): see `crate::workspace`.

use rustc_hash::FxHashMap;
use std::sync::Arc;

/// A loaded context document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDocument {
    /// The URL the document was finally read from (after redirects): the
    /// base for relative IRIs inside it.
    pub url: String,
    /// The document's JSON text.
    pub text: Arc<str>,
}

/// Why a context document could not be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Fetching it would need the network, which is not allowed
    /// (`imports.network` / `--network` is off).
    NetworkNotAllowed,
    /// No such document (a missing file, an HTTP 404).
    NotFound,
    /// Any other failure (I/O, HTTP, not JSON), described for the user.
    Failed(String),
    /// Being fetched in the background: the document is parsed again when
    /// it arrives.
    Pending,
}

/// Loads context documents by absolute URL. Synchronous: parsing a document
/// waits for its contexts. Shared across the threads that parse documents.
pub trait ContextLoader: Send + Sync {
    /// The document at `url` (absolute, already resolved against the
    /// referencing document's base).
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError>;
}

/// A loader that never touches the network or the disk: it serves the
/// documents given with [`NoNetwork::with`], and refuses the rest
/// (`http(s):` URLs with [`LoadError::NetworkNotAllowed`], others with
/// [`LoadError::NotFound`]).
#[derive(Clone, Debug, Default)]
pub struct NoNetwork {
    docs: FxHashMap<String, Arc<str>>,
}

impl NoNetwork {
    /// Serve `text` as the document at `url`.
    pub fn with(mut self, url: impl Into<String>, text: impl Into<Arc<str>>) -> NoNetwork {
        self.docs.insert(url.into(), text.into());
        self
    }
}

impl ContextLoader for NoNetwork {
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        match self.docs.get(url) {
            Some(text) => Ok(RemoteDocument {
                url: url.to_string(),
                text: text.clone(),
            }),
            None if url.starts_with("http://") || url.starts_with("https://") => {
                Err(LoadError::NetworkNotAllowed)
            }
            None => Err(LoadError::NotFound),
        }
    }
}

/// The loader for this build: [`DesktopLoader`] over the user's imports
/// cache, fetching only when `network` is on; [`Bundled`] in the browser.
pub fn for_platform(network: bool) -> Arc<dyn ContextLoader> {
    #[cfg(not(target_arch = "wasm32"))]
    return Arc::new(DesktopLoader::new(
        crate::imports::import_cache_dir(),
        network,
    ));
    #[cfg(target_arch = "wasm32")]
    {
        let _ = network;
        Arc::new(Bundled)
    }
}

// ---- bundled contexts ----

/// A context compiled in: the URLs it is served for (without the scheme;
/// `http:` and `https:` both match), the URL it was fetched from, its text.
struct BundledContext {
    urls: &'static [&'static str],
    source: &'static str,
    text: &'static str,
}

/// See `data/jsonld-contexts/README.md` for where each came from.
static BUNDLED: &[BundledContext] = &[
    BundledContext {
        urls: &[
            "schema.org",
            "schema.org/",
            "schema.org/docs/jsonldcontext.json",
            "schema.org/docs/jsonldcontext.jsonld",
        ],
        source: "https://schema.org/docs/jsonldcontext.jsonld",
        text: include_str!("../../data/jsonld-contexts/schema-org.jsonld"),
    },
    BundledContext {
        urls: &[
            "www.w3.org/ns/activitystreams",
            "www.w3.org/ns/activitystreams.jsonld",
        ],
        source: "https://www.w3.org/ns/activitystreams",
        text: include_str!("../../data/jsonld-contexts/activitystreams.jsonld"),
    },
    BundledContext {
        urls: &["www.w3.org/2018/credentials/v1"],
        source: "https://www.w3.org/2018/credentials/v1",
        text: include_str!("../../data/jsonld-contexts/credentials-v1.jsonld"),
    },
    BundledContext {
        urls: &["www.w3.org/ns/credentials/v2"],
        source: "https://www.w3.org/ns/credentials/v2",
        text: include_str!("../../data/jsonld-contexts/credentials-v2.jsonld"),
    },
    BundledContext {
        urls: &["www.w3.org/ns/did/v1"],
        source: "https://www.w3.org/ns/did/v1",
        text: include_str!("../../data/jsonld-contexts/did-v1.jsonld"),
    },
];

/// The bundled context served for `url`, if any. Its `url` is where the
/// bundled copy was fetched from.
pub fn bundled(url: &str) -> Option<RemoteDocument> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    BUNDLED
        .iter()
        .find(|c| c.urls.contains(&rest))
        .map(|c| RemoteDocument {
            url: c.source.to_string(),
            text: Arc::from(c.text),
        })
}

/// A loader serving only the bundled contexts: the browser engine's, which
/// has neither files nor network. Other URLs fail as with [`NoNetwork`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Bundled;

impl ContextLoader for Bundled {
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        bundled(url).ok_or_else(|| refused(url))
    }
}

/// The error for a document nobody supplied: a web URL needs the network.
fn refused(url: &str) -> LoadError {
    if is_web(url) {
        LoadError::NetworkNotAllowed
    } else {
        LoadError::NotFound
    }
}

fn is_web(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::{DesktopLoader, read_context_file};

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use super::{ContextLoader, LoadError, RemoteDocument, bundled, is_web, refused};
    use crate::imports::{self, MAX_FETCH_BYTES};
    use crossbeam_channel::{Receiver, Sender, unbounded};
    use rustc_hash::{FxHashMap, FxHashSet};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const ACCEPT: &str = "application/ld+json, application/json;q=0.9";
    /// A failed fetch is not tried again for this long.
    const RETRY_FETCH_AFTER: Duration = Duration::from_secs(300);

    /// Loads web contexts on the desktop, in order: the imports cache
    /// (`<hash>.jsonld` beside the cached imports); the bundled contexts;
    /// then, only when the network is on, the web, saving what it fetched to
    /// the cache. Documents are kept in memory once loaded, and a failed
    /// fetch is not retried for five minutes. Other URLs are not found.
    ///
    /// Clones share their state (memory, fetches in flight).
    #[derive(Clone)]
    pub struct DesktopLoader {
        inner: Arc<Inner>,
    }

    struct Inner {
        cache: Option<PathBuf>,
        network: Network,
        memo: Mutex<FxHashMap<String, Memo>>,
        /// Held while fetching, so documents parsed in parallel fetch a
        /// context once.
        fetching: Mutex<()>,
    }

    enum Network {
        Off,
        /// Fetch while the document waits.
        Inline,
        /// Fetch on another thread; the URL goes to `done` once the result
        /// is known (loaded or failed).
        Background {
            queued: Mutex<FxHashSet<String>>,
            done: Sender<String>,
        },
    }

    #[derive(Clone)]
    enum Memo {
        Loaded(RemoteDocument),
        Failed(Instant, LoadError),
    }

    impl DesktopLoader {
        /// `cache` is the imports cache directory (None: no cache). With
        /// `network`, a context that is neither cached nor bundled is
        /// fetched while the document waits.
        pub fn new(cache: Option<PathBuf>, network: bool) -> DesktopLoader {
            let network = if network {
                Network::Inline
            } else {
                Network::Off
            };
            DesktopLoader::with(cache, network)
        }

        /// A loader that fetches in the background: a context that is
        /// neither cached nor bundled is [`LoadError::Pending`] at first,
        /// and its URL arrives on the receiver once it has been fetched (or
        /// failed); documents that use it are then parsed again.
        pub fn in_background(cache: Option<PathBuf>) -> (DesktopLoader, Receiver<String>) {
            let (done, rx) = unbounded();
            let network = Network::Background {
                queued: Mutex::default(),
                done,
            };
            (DesktopLoader::with(cache, network), rx)
        }

        fn with(cache: Option<PathBuf>, network: Network) -> DesktopLoader {
            DesktopLoader {
                inner: Arc::new(Inner {
                    cache,
                    network,
                    memo: Mutex::default(),
                    fetching: Mutex::default(),
                }),
            }
        }
    }

    impl Inner {
        fn remembered(&self, url: &str) -> Option<Result<RemoteDocument, LoadError>> {
            match self.memo.lock().ok()?.get(url)? {
                Memo::Loaded(doc) => Some(Ok(doc.clone())),
                Memo::Failed(at, e) if at.elapsed() < RETRY_FETCH_AFTER => Some(Err(e.clone())),
                Memo::Failed(..) => None,
            }
        }

        fn remember(&self, url: &str, result: &Result<RemoteDocument, LoadError>) {
            let memo = match result {
                Ok(doc) => Memo::Loaded(doc.clone()),
                Err(e) => Memo::Failed(Instant::now(), e.clone()),
            };
            if let Ok(mut m) = self.memo.lock() {
                m.insert(url.to_string(), memo);
            }
        }

        /// The cached copy of `url`; its final URL is in a `.url` file
        /// beside it when a redirect changed it.
        fn cached(&self, url: &str) -> Option<RemoteDocument> {
            let path = imports::cache_file_as(self.cache.as_ref()?, url, "jsonld");
            let text = read_limited(&path).ok()?;
            let final_url = std::fs::read_to_string(path.with_extension("url"))
                .ok()
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| url.to_string());
            Some(RemoteDocument {
                url: final_url,
                text: text.into(),
            })
        }

        /// Save a fetched document; a failed write only costs a fetch later.
        fn save(&self, url: &str, doc: &RemoteDocument) {
            let Some(dir) = &self.cache else { return };
            let path = imports::cache_file_as(dir, url, "jsonld");
            let redirect = path.with_extension("url");
            if doc.url != url {
                if imports::write_atomic(&redirect, doc.url.as_bytes()).is_err() {
                    return;
                }
            } else {
                let _ = std::fs::remove_file(&redirect);
            }
            let _ = imports::write_atomic(&path, doc.text.as_bytes());
        }

        fn fetch(&self, url: &str) -> Result<RemoteDocument, LoadError> {
            let _one_at_a_time = self.fetching.lock();
            if let Some(r) = self.remembered(url) {
                return r;
            }
            let result = fetch(url);
            if let Ok(doc) = &result {
                self.save(url, doc);
            }
            self.remember(url, &result);
            result
        }
    }

    impl ContextLoader for DesktopLoader {
        fn load(&self, url: &str) -> Result<RemoteDocument, LoadError> {
            let inner = &self.inner;
            if !is_web(url) {
                return Err(LoadError::NotFound);
            }
            if let Some(r) = inner.remembered(url) {
                return r;
            }
            if let Some(doc) = inner.cached(url).or_else(|| bundled(url)) {
                inner.remember(url, &Ok(doc.clone()));
                return Ok(doc);
            }
            match &inner.network {
                Network::Off => Err(refused(url)),
                Network::Inline => inner.fetch(url),
                Network::Background { queued, .. } => {
                    let first = queued
                        .lock()
                        .map(|mut q| q.insert(url.to_string()))
                        .unwrap_or(false);
                    if first {
                        let inner = inner.clone();
                        let url = url.to_string();
                        std::thread::spawn(move || {
                            let _ = inner.fetch(&url);
                            if let Network::Background { queued, done } = &inner.network {
                                if let Ok(mut q) = queued.lock() {
                                    q.remove(&url);
                                }
                                let _ = done.send(url);
                            }
                        });
                    }
                    Err(LoadError::Pending)
                }
            }
        }
    }

    /// The text of a context file on disk: at most [`MAX_FETCH_BYTES`],
    /// UTF-8, without a BOM.
    pub fn read_context_file(path: &Path) -> Result<String, LoadError> {
        read_limited(path)
    }

    /// A context file's text: at most [`MAX_FETCH_BYTES`], UTF-8, no BOM.
    fn read_limited(path: &Path) -> Result<String, LoadError> {
        let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => LoadError::NotFound,
            _ => LoadError::Failed(e.to_string()),
        })?;
        if meta.len() > MAX_FETCH_BYTES {
            return Err(too_large());
        }
        let bytes = std::fs::read(path).map_err(|e| LoadError::Failed(e.to_string()))?;
        text_of(bytes)
    }

    fn too_large() -> LoadError {
        LoadError::Failed(format!(
            "larger than {} MB",
            MAX_FETCH_BYTES / (1024 * 1024)
        ))
    }

    fn text_of(bytes: Vec<u8>) -> Result<String, LoadError> {
        let text = String::from_utf8(bytes)
            .map_err(|_| LoadError::Failed("the document is not UTF-8".into()))?;
        Ok(match text.strip_prefix('\u{feff}') {
            Some(t) => t.to_string(),
            None => text,
        })
    }

    /// GET a context. A response that is not JSON but links to a JSON-LD
    /// alternate (`Link: <...>; rel="alternate"; type="application/ld+json"`,
    /// as schema.org answers) is followed once.
    pub(super) fn fetch(url: &str) -> Result<RemoteDocument, LoadError> {
        let mut r = get(url)?;
        if !is_json(r.content_type.as_deref())
            && let Some(alt) = r.link.as_deref().and_then(alternate_jsonld)
            && let Some(alt) = url::Url::parse(&r.final_url)
                .ok()
                .and_then(|base| base.join(&alt).ok())
        {
            r = get(alt.as_str())?;
        }
        let text = text_of(r.body)?;
        serde_json::from_str::<serde::de::IgnoredAny>(&text)
            .map_err(|e| LoadError::Failed(format!("{url} is not JSON: {e}")))?;
        Ok(RemoteDocument {
            url: r.final_url,
            text: text.into(),
        })
    }

    fn get(url: &str) -> Result<imports::HttpResponse, LoadError> {
        imports::http_get(url, ACCEPT).map_err(|e| match e.status {
            Some(404 | 410) => LoadError::NotFound,
            _ => LoadError::Failed(format!("{url} could not be fetched: {}", e.message)),
        })
    }

    fn is_json(content_type: Option<&str>) -> bool {
        content_type.is_some_and(|ct| {
            let ct = ct
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            ct == "application/json" || ct.ends_with("+json")
        })
    }

    /// The target of the first `rel="alternate"` link of type
    /// `application/ld+json` in a `Link` header.
    pub(super) fn alternate_jsonld(header: &str) -> Option<String> {
        header.split(',').find_map(|link| {
            let (target, params) = link.trim().strip_prefix('<')?.split_once('>')?;
            let mut alternate = false;
            let mut jsonld = false;
            for p in params.split(';') {
                let Some((k, v)) = p.split_once('=') else {
                    continue;
                };
                let v = v.trim().trim_matches('"');
                match k.trim().to_ascii_lowercase().as_str() {
                    "rel" => alternate = v.split_whitespace().any(|r| r == "alternate"),
                    "type" => jsonld = v.eq_ignore_ascii_case("application/ld+json"),
                    _ => {}
                }
            }
            (alternate && jsonld).then(|| target.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_network_serves_only_what_it_was_given() {
        let l = NoNetwork::default().with("https://example.org/ctx", "{}");
        let doc = l.load("https://example.org/ctx").unwrap();
        assert_eq!(
            (doc.url.as_str(), &*doc.text),
            ("https://example.org/ctx", "{}")
        );
        assert_eq!(
            l.load("https://schema.org/"),
            Err(LoadError::NetworkNotAllowed)
        );
        assert_eq!(l.load("file:///x/ctx.jsonld"), Err(LoadError::NotFound));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod desktop_tests {
    use super::*;
    use crate::imports::cache_file_as;
    use std::path::PathBuf;

    /// A fresh directory, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let d =
                std::env::temp_dir().join(format!("turtle-lsp-ctx-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            TempDir(d)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn file_url(path: &std::path::Path) -> String {
        url::Url::from_file_path(path).unwrap().to_string()
    }

    #[test]
    fn offline_the_common_contexts_come_from_the_bundle() {
        let cache = TempDir::new("bundled");
        let l = DesktopLoader::new(Some(cache.0.clone()), false);
        for url in [
            "https://schema.org/",
            "http://schema.org",
            "https://schema.org/docs/jsonldcontext.json",
        ] {
            let doc = l.load(url).unwrap();
            assert_eq!(doc.url, "https://schema.org/docs/jsonldcontext.jsonld");
            assert!(doc.text.contains("\"@vocab\": \"http://schema.org/\""));
        }
        for url in [
            "https://www.w3.org/ns/activitystreams",
            "https://www.w3.org/2018/credentials/v1",
            "https://www.w3.org/ns/credentials/v2",
            "https://www.w3.org/ns/did/v1",
        ] {
            assert_eq!(l.load(url).unwrap().url, url);
            assert_eq!(Bundled.load(url).unwrap().url, url);
        }
        // Nothing was written to the cache.
        assert_eq!(std::fs::read_dir(&cache.0).unwrap().count(), 0);
    }

    #[test]
    fn every_bundled_context_is_json() {
        for c in BUNDLED {
            let v: serde_json::Value = serde_json::from_str(c.text).unwrap();
            assert!(v.get("@context").is_some(), "{}", c.source);
        }
    }

    #[test]
    fn with_the_network_off_an_unknown_remote_context_is_refused() {
        let cache = TempDir::new("refused");
        let l = DesktopLoader::new(Some(cache.0.clone()), false);
        assert_eq!(
            l.load("https://example.org/context.jsonld"),
            Err(LoadError::NetworkNotAllowed)
        );
        assert_eq!(
            Bundled.load("https://example.org/context.jsonld"),
            Err(LoadError::NetworkNotAllowed)
        );
        assert_eq!(l.load("urn:x:context"), Err(LoadError::NotFound));
    }

    #[test]
    fn a_cached_context_is_served_without_the_network() {
        let cache = TempDir::new("cached");
        let url = "https://example.org/context.jsonld";
        let moved = "https://example.org/v2/context.jsonld";
        std::fs::write(
            cache_file_as(&cache.0, url, "jsonld"),
            r#"{"@context": {}}"#,
        )
        .unwrap();
        let other = "https://example.org/other";
        let path = cache_file_as(&cache.0, other, "jsonld");
        std::fs::write(&path, "{}").unwrap();
        std::fs::write(path.with_extension("url"), moved).unwrap();

        let l = DesktopLoader::new(Some(cache.0.clone()), false);
        let doc = l.load(url).unwrap();
        assert_eq!((doc.url.as_str(), &*doc.text), (url, r#"{"@context": {}}"#));
        // The cache comes before the bundle, and keeps a redirect's target.
        assert_eq!(l.load(other).unwrap().url, moved);
    }

    #[test]
    fn a_context_file_is_read_without_its_bom_and_the_loader_leaves_file_urls_alone() {
        let dir = TempDir::new("file");
        let path = dir.0.join("ctx.jsonld");
        std::fs::write(
            &path,
            "\u{feff}{\"@context\": {\"a\": \"http://ex.org/a\"}}",
        )
        .unwrap();
        assert!(read_context_file(&path).unwrap().starts_with('{'));
        assert_eq!(
            read_context_file(&dir.0.join("missing.jsonld")),
            Err(LoadError::NotFound)
        );
        // The workspace serves `file:` URLs (open documents, then files
        // inside its roots); the loader never reads one.
        let l = DesktopLoader::new(None, false);
        assert_eq!(l.load(&file_url(&path)), Err(LoadError::NotFound));
    }

    /// A local HTTP server answering every request with a context; the
    /// requested paths are logged.
    fn serve() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                seen.lock()
                    .unwrap()
                    .push(head.split(' ').nth(1).unwrap_or("").to_string());
                let body = r#"{"@context": {"a": "http://ex.org/a"}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/ld+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (base, log)
    }

    #[test]
    fn in_the_background_a_context_is_pending_then_loaded_and_fetched_once() {
        let cache = TempDir::new("background");
        let (base, log) = serve();
        let url = format!("{base}/ctx.jsonld");
        let (l, done) = DesktopLoader::in_background(Some(cache.0.clone()));
        assert_eq!(l.load(&url), Err(LoadError::Pending));
        // Asked again while the fetch runs: still pending, not fetched twice.
        let again = l.load(&url);
        assert!(
            again == Err(LoadError::Pending) || again.is_ok(),
            "{again:?}"
        );
        let arrived = done
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(arrived, url);
        let doc = l.load(&url).unwrap();
        assert!(doc.text.contains("ex.org"));
        assert!(cache_file_as(&cache.0, &url, "jsonld").is_file());
        assert_eq!(*log.lock().unwrap(), ["/ctx.jsonld"]);
        // Nothing else is announced.
        assert!(done.try_recv().is_err());
    }

    #[test]
    fn link_headers_name_the_jsonld_alternate() {
        use super::desktop::alternate_jsonld;
        assert_eq!(
            alternate_jsonld(
                r#"</docs/jsonldcontext.jsonld>; rel="alternate"; type="application/ld+json""#
            )
            .as_deref(),
            Some("/docs/jsonldcontext.jsonld")
        );
        assert_eq!(
            alternate_jsonld(r#"<a.html>; rel="alternate"; type="text/html", <b.jsonld>; rel=alternate; type="application/ld+json""#)
                .as_deref(),
            Some("b.jsonld")
        );
        assert_eq!(
            alternate_jsonld(r#"<c.jsonld>; rel="http://www.w3.org/ns/json-ld#context""#),
            None
        );
    }

    /// Fetches a context from the web: only with TURTLE_LSP_NETWORK_TESTS=1.
    #[test]
    fn with_the_network_on_a_context_is_fetched_once_then_cached() {
        if std::env::var_os("TURTLE_LSP_NETWORK_TESTS").is_none() {
            return;
        }
        let cache = TempDir::new("network");
        let url = "https://www.w3.org/ns/odrl.jsonld";
        let l = DesktopLoader::new(Some(cache.0.clone()), true);
        let doc = l.load(url).unwrap();
        assert!(doc.text.contains("@context"));
        assert!(cache_file_as(&cache.0, url, "jsonld").is_file());
        let offline = DesktopLoader::new(Some(cache.0.clone()), false);
        assert_eq!(offline.load(url).unwrap(), doc);
        // schema.org's home page is HTML linking to its context.
        let doc = super::desktop::fetch("https://schema.org/").unwrap();
        assert!(
            doc.url.ends_with("/docs/jsonldcontext.jsonld"),
            "{}",
            doc.url
        );
    }
}
