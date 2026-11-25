//! Loading one import: a file in the workspace, a cached copy or a web
//! document, and reading it in its own syntax.
//!
//! A `file:` import is read only through [`fs_policy`]; a web import only
//! through [`crate::fetch`], which holds the address, redirect and budget
//! rules. A file's format comes from its extension, else from what it starts
//! with. A web document's format comes from its content type, else from the
//! extension of the URL it was finally fetched from, else from what it starts
//! with. The format never comes from the import IRI.
//!
//! A local RDF/XML, OWL Functional or OWL/XML file is analysed by its
//! frontend over its own text, so locations point into the file. A fetched
//! one is converted to N-Triples ([`rdf_xml_to_ntriples`],
//! [`functional_to_ntriples`], [`owl_xml_to_ntriples`]), and that text is
//! what is cached and analysed. A text in OWL Functional syntax or OWL/XML
//! is read as such whatever its name or content type says (servers send
//! `.owl` files as `application/rdf+xml`, `application/xml` or
//! `text/plain`, whatever their syntax), and between the two XML formats
//! the text decides, so RDF/XML under a `.owx` name or the type
//! `application/owl+xml` is read as RDF/XML. One in Manchester syntax is
//! refused.

use super::catalog::{Catalog, Target};
use super::ntriples::stopped_at;
// Fetching converts; the browser build fetches nothing.
#[cfg(not(target_arch = "wasm32"))]
use super::ntriples::{functional_to_ntriples, owl_xml_to_ntriples, rdf_xml_to_ntriples};
use super::worker::{Memo, Ontology, RETRY_FETCH_AFTER};
use super::*;
use crate::syntax::Syntax;
use crate::syntax::sniff::SNIFF_BYTES;
pub(crate) use crate::syntax::sniff::Sniffed;
use std::path::Component;

// ---- files ----

/// Why a path is refused for being outside the workspace; also what a path
/// that does not exist gets, so the answer says nothing about files outside.
const OUTSIDE: &str = "it is outside the workspace or does not exist, so it is not read";
const NETWORK_PATH: &str =
    "it is on a network share, which is not read unless the workspace is on one";

/// The file a `file:` URL names, if a workspace may read it: a local file
/// (no host but `localhost`) inside one of `roots`. Nothing is asked of the
/// file system before the answer is known to be yes except the roots
/// themselves; a missing file and a file outside the roots get one error.
///
/// Local imports and contexts are read only from inside the workspace: a
/// cloned repository must not make the editor read arbitrary files of the
/// user's, or, on Windows, open a connection to a share it names.
pub(crate) fn fs_policy(url: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let path = file_url_path(url)?;
    fs_policy_path(&path, roots)
}

/// The path a `file:` URL names, with no file system involved: an error for
/// another scheme or a host (`file://server/share/x` is a network path).
fn file_url_path(url: &str) -> Result<PathBuf, String> {
    let u = url::Url::parse(url).map_err(|e| e.to_string())?;
    if u.scheme() != "file" {
        return Err(format!("{url} is not a file: URL"));
    }
    if let Some(host) = names_host(&u) {
        return Err(format!(
            "a file: URL naming a host (`{host}`) is not read; only files on this machine are"
        ));
    }
    crate::workspace::file_path_of(&u).ok_or_else(|| OUTSIDE.to_string())
}

/// The host a `file:` URL names, unless it is empty or `localhost`.
pub(super) fn names_host(u: &url::Url) -> Option<&str> {
    u.host_str()
        .filter(|h| !h.is_empty() && !h.eq_ignore_ascii_case("localhost"))
}

/// [`fs_policy`] for a path: refuse a network path unless a root is one,
/// compare with the roots lexically (`..` resolved, no file system), and only
/// then resolve links and compare again. The path is returned as given.
pub(crate) fn fs_policy_path(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, String> {
    if is_network_path(path) && !roots.iter().any(|r| is_network_path(r)) {
        return Err(NETWORK_PATH.into());
    }
    let canonical_roots: Vec<PathBuf> =
        roots.iter().filter_map(|r| r.canonicalize().ok()).collect();
    let lexical = normalized(path);
    let within = |p: &Path, roots: &[PathBuf]| roots.iter().any(|r| p.starts_with(normalized(r)));
    if !within(&lexical, roots) && !within(&lexical, &canonical_roots) {
        return Err(OUTSIDE.into());
    }
    let real = path.canonicalize().map_err(|_| OUTSIDE.to_string())?;
    if canonical_roots.iter().any(|r| real.starts_with(r)) {
        Ok(path.to_path_buf())
    } else {
        Err(OUTSIDE.into())
    }
}

/// `path` with `.` and `..` resolved by name, without asking the file system.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// Whether `path` is on a network share: a Windows UNC (`\\server\share`) or
/// device (`\\.\`) path. Other systems have no such spelling.
pub(crate) fn is_network_path(path: &Path) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let s = path.to_string_lossy();
    let s = s.replace('/', "\\");
    match s.strip_prefix("\\\\") {
        None => false,
        // `\\?\C:\x` is a local path in the long form; `\\?\UNC\server\share` is not.
        Some(rest) => match rest.strip_prefix("?\\") {
            Some(long) => long.len() < 2 || long.as_bytes()[1] != b':',
            None => true,
        },
    }
}

// ---- budget of the web, off in the browser ----

#[cfg(not(target_arch = "wasm32"))]
pub(super) use crate::fetch::Budget;

/// The browser build fetches nothing.
#[cfg(target_arch = "wasm32")]
pub(super) struct Budget;

#[cfg(target_arch = "wasm32")]
impl Budget {
    pub(super) fn pass() -> Budget {
        Budget
    }
}

/// Why a fetch failed, and whether to remember that.
struct Failure {
    message: String,
    /// The pass ran out of budget; a pass with more may do better, so the
    /// failure is not remembered.
    budget: bool,
}

/// Load one import: (where from, the ontology, freshly parsed). A file in
/// the workspace it is read from is added to `files`, loaded or not.
pub(super) fn load(
    iri: &str,
    req: &Request,
    catalog: &Catalog,
    memo: &mut Memo,
    budget: &mut Budget,
    files: &mut Vec<PathBuf>,
) -> Result<(String, Ontology, bool), String> {
    let base = without_fragment(iri);
    let mut url = base.to_string();
    if let Some((target, cat)) = catalog.lookup(iri) {
        match target {
            Target::File(path) => {
                let path = fs_policy_path(path, &req.roots).map_err(|e| {
                    format!(
                        "Import <{iri}> is mapped to {} by {}: {e}",
                        path.display(),
                        cat.display()
                    )
                })?;
                files.push(path.clone());
                return load_file(&path, None, base, memo)
                    .map(|(o, fresh)| (path.display().to_string(), o, fresh))
                    .map_err(|e| {
                        format!(
                            "Import <{iri}> is mapped to {} by {}, which could not be loaded: {e}",
                            path.display(),
                            cat.display()
                        )
                    });
            }
            Target::Url(u) => url = u.clone(),
        }
    }
    if url::Url::parse(&url).is_ok_and(|u| u.scheme() == "file") {
        let path =
            fs_policy(&url, &req.roots).map_err(|e| format!("Import <{iri}> names {url}: {e}"))?;
        files.push(path.clone());
        return load_file(&path, None, base, memo)
            .map(|(o, fresh)| (path.display().to_string(), o, fresh))
            .map_err(|e| {
                format!(
                    "Import <{iri}> could not be loaded from {}: {e}",
                    path.display()
                )
            });
    }
    if let Some(dir) = &req.cache
        && let Some((o, fresh)) = cached(dir, &url, base, memo)
    {
        return Ok((url, o, fresh));
    }
    let web = url.starts_with("http://") || url.starts_with("https://");
    let allowed = req.network || req.fetch.iter().any(|f| norm(f) == norm(iri));
    if !allowed || !web {
        let how = if !web {
            "Map it to a local file in a catalog-v001.xml"
        } else {
            "Use the quick fix to fetch it once, map it to a local file in a catalog-v001.xml, or enable turtle.imports.network to fetch web imports automatically"
        };
        return Err(format!(
            "Import <{iri}> is not resolved: no ontology with this IRI in the workspace, no catalog-v001.xml entry and nothing in the import cache. {how}"
        ));
    }
    // Failures are remembered under the import, not under where a catalog
    // sends it, so asking to fetch the import again finds the memo.
    let key = norm(iri);
    if let Some((at, failed_url, msg)) = memo.failed.get(key)
        && *failed_url == url
        && at.elapsed() < RETRY_FETCH_AFTER
    {
        return Err(msg.clone());
    }
    let remember = |memo: &mut Memo, msg: &str| {
        memo.failed.insert(
            key.to_string(),
            (Instant::now(), url.clone(), msg.to_string()),
        );
    };
    let fetched = match fetch_document(iri, &url, req.allow_private, budget) {
        Ok(v) => v,
        Err(f) => {
            if !f.budget {
                remember(memo, &f.message);
            }
            return Err(f.message);
        }
    };
    let Fetched {
        text,
        format,
        final_url,
        guessed,
    } = fetched;
    // What the cache holds is what is parsed, so its lines are the file's.
    let text = match format {
        Format::JsonLd => text,
        _ => stamped(&url, &text),
    };
    let mut parsed = match parse(text.as_bytes(), format, base, guessed) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("Import <{iri}> was fetched from {url} but could not be read: {e}");
            remember(memo, &msg);
            return Err(msg);
        }
    };
    let a = Arc::get_mut(&mut parsed.analysis).expect("fresh analysis");
    a.uri = url.clone();
    // A failed write only costs a fetch next time; locations then name the URL.
    if let Some(dir) = &req.cache
        && let Some(path) = store(dir, &url, &final_url, format, &text)
        && let Some(file) = path_to_uri(&path)
    {
        a.uri = file;
        a.origin = Some(url.clone());
    }
    Ok((url, parsed, true))
}

// ---- the web ----

/// What a web import is asked for: its content types, best first. OWL
/// Functional syntax and OWL/XML, which are read too, last: a server with
/// an RDF form of the same ontology sends that. `application/owl+xml` is
/// the type the OWL/XML Recommendation names (section 6); IANA does not
/// list it.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const ACCEPT: &str = "text/turtle, application/trig;q=0.9, application/n-triples;q=0.9, application/n-quads;q=0.9, application/ld+json;q=0.8, application/rdf+xml;q=0.8, text/owl-functional;q=0.7, application/owl+xml;q=0.7";

/// A fetched document, as [`parse`] reads it.
struct Fetched {
    /// Its text, whose relative IRIs, if any, resolve against where the
    /// document was finally found; RDF/XML, Functional syntax and OWL/XML
    /// become N-Triples ([`marked`]).
    text: String,
    /// The format the text is read in: a converted text is read as Turtle.
    format: Format,
    /// Where the document was finally found, after redirects.
    final_url: String,
    /// The format was sniffed, not given by the content type.
    guessed: bool,
}

/// Fetch `url` and bring it to text that [`parse`] reads.
#[cfg(not(target_arch = "wasm32"))]
fn fetch_document(
    iri: &str,
    url: &str,
    allow_private: bool,
    budget: &mut Budget,
) -> Result<Fetched, Failure> {
    use crate::fetch;
    let policy = fetch::Policy { allow_private };
    let r = fetch::get(url, ACCEPT, policy, budget).map_err(|e| Failure {
        message: format!(
            "Import <{iri}> could not be fetched from {url}: {}. Map it to a local file in a catalog-v001.xml",
            e.message
        ),
        budget: e.budget,
    })?;
    let unread = |message: String| Failure {
        message: format!("Import <{iri}> was fetched from {url} but could not be read: {message}"),
        budget: false,
    };
    let lossy = utf8(&r.body).map_err(unread)?;
    let lossy = lossy.trim_start_matches('\u{feff}');
    let sniffed = Sniffed::of(lossy).format().map_err(unread)?;
    let by_type = r
        .content_type
        .as_deref()
        .and_then(|ct| format_of_type(&ct.to_ascii_lowercase()));
    let (format, guessed) = match by_type {
        Some(format) => (format, false),
        None => {
            let path = url::Url::parse(&r.final_url)
                .map(|u| u.path().to_string())
                .unwrap_or_default();
            format_of(&path, &r.body).map_err(unread)?
        }
    };
    // Whatever the content type or the name says ([`by_content`]).
    let format = by_content(sniffed, format);
    let text = match format {
        Format::Turtle | Format::TriG => match oxiri::Iri::parse(r.final_url.as_str()) {
            Ok(_) => format!("@base <{}> .\n{lossy}", r.final_url),
            Err(_) => lossy.to_string(),
        },
        Format::NTriples | Format::NQuads | Format::JsonLd => lossy.to_string(),
        Format::RdfXml => {
            let (nt, truncated) = rdf_xml_to_ntriples(&r.body, &r.final_url).map_err(unread)?;
            marked(nt, truncated.as_deref(), TRUNCATED)
        }
        Format::Functional => {
            let (nt, truncated) = functional_to_ntriples(&r.body, &r.final_url).map_err(unread)?;
            marked(nt, truncated.as_deref(), TRUNCATED_FUNCTIONAL)
        }
        Format::OwlXml => {
            let (nt, truncated) = owl_xml_to_ntriples(&r.body, &r.final_url).map_err(unread)?;
            marked(nt, truncated.as_deref(), TRUNCATED_OWL_XML)
        }
    };
    let format = match format {
        Format::RdfXml | Format::Functional | Format::OwlXml => Format::Turtle,
        other => other,
    };
    Ok(Fetched {
        text,
        format,
        final_url: r.final_url,
        guessed,
    })
}

/// The browser build fetches nothing.
#[cfg(target_arch = "wasm32")]
fn fetch_document(
    iri: &str,
    url: &str,
    _allow_private: bool,
    _budget: &mut Budget,
) -> Result<Fetched, Failure> {
    Err(Failure {
        message: format!(
            "Import <{iri}> could not be fetched from {url}: the browser build does not fetch"
        ),
        budget: false,
    })
}

/// The format a content type names, if it names one (`text/plain` and the
/// like say nothing about RDF).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn format_of_type(content_type: &str) -> Option<Format> {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    if ct == "text/owl-functional" {
        Some(Format::Functional)
    } else if ct == "application/owl+xml" {
        Some(Format::OwlXml)
    } else if ct.contains("xml") {
        Some(Format::RdfXml)
    } else if ct.ends_with("json") {
        Some(Format::JsonLd)
    } else if ct.contains("trig") {
        Some(Format::TriG)
    } else if ct.contains("n-quads") {
        Some(Format::NQuads)
    } else if ct.contains("turtle") || ct.contains("n-triples") {
        Some(Format::Turtle)
    } else {
        None
    }
}

// ---- the cache ----

/// The kinds of cache file a document is kept in, in the order they are
/// looked for.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const CACHE_EXTENSIONS: [&str; 5] = ["ttl", "trig", "nt", "nq", "jsonld"];

/// The cached copy of `url` under `dir`, if there is a valid one.
#[cfg(not(target_arch = "wasm32"))]
fn cached(dir: &Path, url: &str, base: &str, memo: &mut Memo) -> Option<(Ontology, bool)> {
    CACHE_EXTENSIONS.iter().find_map(|ext| {
        let path = crate::fetch::cache_file_as(dir, url, ext);
        if !path.is_file() {
            return None;
        }
        load_file(&path, Some(url), base, memo).ok()
    })
}

#[cfg(target_arch = "wasm32")]
fn cached(_dir: &Path, _url: &str, _base: &str, _memo: &mut Memo) -> Option<(Ontology, bool)> {
    None
}

/// `text` as a cache file for `url`: with the header line that names it.
#[cfg(not(target_arch = "wasm32"))]
fn stamped(url: &str, text: &str) -> String {
    crate::fetch::with_header(url, text)
}

#[cfg(target_arch = "wasm32")]
fn stamped(_url: &str, text: &str) -> String {
    text.to_string()
}

/// Write `text` as the cache file of `url` (a JSON-LD document with the
/// sidecar that names its URL), and drop the copies of other kinds. The file
/// written, if it was.
#[cfg(not(target_arch = "wasm32"))]
fn store(dir: &Path, url: &str, final_url: &str, format: Format, text: &str) -> Option<PathBuf> {
    use crate::fetch::{cache_file_as, cacheable, sidecar, sidecar_text, write_atomic};
    if !cacheable(url) {
        return None;
    }
    let path = cache_file_as(dir, url, format.cache_extension());
    // The document first: a new `.url` file beside an old document would vouch for it.
    write_atomic(&path, text.as_bytes()).ok()?;
    if format == Format::JsonLd {
        write_atomic(&sidecar(&path), sidecar_text(url, final_url).as_bytes()).ok()?;
    }
    for ext in CACHE_EXTENSIONS {
        if ext != format.cache_extension() {
            let other = cache_file_as(dir, url, ext);
            let _ = std::fs::remove_file(&other);
        }
    }
    Some(path)
}

#[cfg(target_arch = "wasm32")]
fn store(
    _dir: &Path,
    _url: &str,
    _final_url: &str,
    _format: Format,
    _text: &str,
) -> Option<PathBuf> {
    None
}

/// Whether the cache file `path`, whose content is `bytes`, was written for
/// `url`: its header line (or, for JSON, its sidecar) names that URL.
#[cfg(not(target_arch = "wasm32"))]
fn belongs_to(path: &Path, bytes: &[u8], url: &str) -> bool {
    use crate::fetch::{header_matches, read_sidecar, sidecar};
    if path.extension().is_some_and(|e| e == "jsonld") {
        std::fs::read_to_string(sidecar(path))
            .ok()
            .and_then(|t| read_sidecar(&t, url))
            .is_some()
    } else {
        header_matches(bytes, url)
    }
}

#[cfg(target_arch = "wasm32")]
fn belongs_to(_path: &Path, _bytes: &[u8], _url: &str) -> bool {
    false
}

/// Load an ontology file; its analysis is named by the file's URI. `origin` is
/// the URL a cached copy came from, which the copy must name.
fn load_file(
    path: &Path,
    origin: Option<&str>,
    base: &str,
    memo: &mut Memo,
) -> Result<(Ontology, bool), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("larger than {} MB", MAX_FILE_BYTES / (1024 * 1024)));
    }
    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if let Some((m, len, b, o)) = memo.files.get(path)
        && *m == modified
        && *len == meta.len()
        && b == base
    {
        return Ok((o.clone(), false));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if let Some(origin) = origin
        && !belongs_to(path, &bytes, origin)
    {
        return Err(format!("the cache file does not name {origin}"));
    }
    // Bad bytes win over the syntax, as for a fetched document: the sniff
    // reads only the head, with bad bytes replaced.
    utf8(&bytes)?;
    let (format, guessed) = format_of(&path.to_string_lossy(), &bytes)?;
    let mut o = parse(&bytes, format, base, guessed)?;
    // Locations open the file; hover names the URL a cached copy came from.
    let a = Arc::get_mut(&mut o.analysis).expect("fresh analysis");
    match path_to_uri(path) {
        Some(uri) => {
            a.uri = uri;
            a.origin = origin.map(str::to_string);
        }
        None => {
            if let Some(origin) = origin {
                a.uri = origin.to_string();
            }
        }
    }
    memo.files.insert(
        path.to_path_buf(),
        (modified, meta.len(), base.to_string(), o.clone()),
    );
    Ok((o, true))
}

// ---- formats ----

/// How an imported document is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Turtle,
    TriG,
    NTriples,
    NQuads,
    JsonLd,
    RdfXml,
    /// OWL 2 Functional-Style Syntax, its axioms mapped to RDF.
    Functional,
    /// The OWL 2 XML Serialization, its axioms mapped to RDF as
    /// Functional syntax's are.
    OwlXml,
}

impl Format {
    /// The format of a document of `syntax`.
    fn of_syntax(syntax: Syntax) -> Format {
        match syntax {
            Syntax::Turtle => Format::Turtle,
            Syntax::TriG => Format::TriG,
            Syntax::NTriples => Format::NTriples,
            Syntax::NQuads => Format::NQuads,
            Syntax::JsonLd => Format::JsonLd,
            Syntax::RdfXml => Format::RdfXml,
            Syntax::OwlFunctional => Format::Functional,
            Syntax::OwlXml => Format::OwlXml,
        }
    }

    /// The syntax the analysis reads it in. (A fetched RDF/XML, Functional
    /// or OWL/XML document is converted to N-Triples first, and read as
    /// Turtle.)
    pub(crate) fn syntax(self) -> Syntax {
        match self {
            Format::Turtle => Syntax::Turtle,
            Format::RdfXml => Syntax::RdfXml,
            Format::Functional => Syntax::OwlFunctional,
            Format::OwlXml => Syntax::OwlXml,
            Format::TriG => Syntax::TriG,
            Format::NTriples => Syntax::NTriples,
            Format::NQuads => Syntax::NQuads,
            Format::JsonLd => Syntax::JsonLd,
        }
    }

    /// The extension of a cache file holding it.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn cache_extension(self) -> &'static str {
        match self {
            Format::Turtle | Format::RdfXml | Format::Functional | Format::OwlXml => "ttl",
            Format::TriG => "trig",
            Format::NTriples => "nt",
            Format::NQuads => "nq",
            Format::JsonLd => "jsonld",
        }
    }
}

/// The format of the document called `name` (a path or URL path) whose bytes
/// are `bytes`: by the extension if it is one of ours, else by [`sniff`]; and
/// whether it was sniffed. An error says the content is in an OWL syntax that
/// is not read (Manchester).
pub(crate) fn format_of(name: &str, bytes: &[u8]) -> Result<(Format, bool), String> {
    match Syntax::from_extension(name) {
        Some(syntax) => Ok((Format::of_syntax(syntax), false)),
        None => sniff(bytes).map(|f| (f, true)),
    }
}

/// The format to read a document in, from its first bytes ([`Sniffed`]), or
/// why it is not read (Manchester).
pub fn sniff(bytes: &[u8]) -> Result<Format, String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(SNIFF_BYTES)]);
    Sniffed::of(&head).format()
}

impl Sniffed {
    /// The format turtle-lsp reads it in, or why it reads none: Manchester
    /// is an OWL syntax that is not read. (The axioms of Functional syntax
    /// and OWL/XML are mapped to RDF.)
    pub(crate) fn format(self) -> Result<Format, String> {
        match self {
            Sniffed::Turtle => Ok(Format::Turtle),
            Sniffed::JsonLd => Ok(Format::JsonLd),
            Sniffed::RdfXml => Ok(Format::RdfXml),
            Sniffed::Functional => Ok(Format::Functional),
            Sniffed::OwlXml => Ok(Format::OwlXml),
            Sniffed::Manchester => Err(format!("it {}", self.unread().unwrap_or_default())),
        }
    }
}

/// The format to read a document in, `format` by its name or content type
/// and `sniffed` by its text: a Functional or an OWL/XML text is read as
/// such whatever the name or the type says (servers send `.owl` files as
/// `application/rdf+xml`, `application/xml` or `text/plain`, whatever their
/// syntax), and between the two XML formats the text decides, so an RDF/XML
/// text is RDF/XML under an OWL/XML name or type.
fn by_content(sniffed: Format, format: Format) -> Format {
    match (sniffed, format) {
        (Format::Functional | Format::OwlXml, _) => sniffed,
        (Format::RdfXml, Format::OwlXml) => Format::RdfXml,
        _ => format,
    }
}

/// The comment line that starts the converted text of an RDF/XML document
/// with an error, followed by where the first is and why. A cached copy
/// keeps it, so its next load says so too.
const TRUNCATED: &str = "# turtle-lsp: the RDF/XML has an error at ";
/// The same line for a converted OWL Functional document.
const TRUNCATED_FUNCTIONAL: &str = "# turtle-lsp: the OWL Functional syntax has an error at ";
/// The same line for a converted OWL/XML document.
const TRUNCATED_OWL_XML: &str = "# turtle-lsp: the OWL/XML has an error at ";

/// `nt`, converted from RDF/XML, Functional syntax or OWL/XML, with the line
/// (starting with `marker`) that says where its first error is.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn marked(nt: String, truncated: Option<&str>, marker: &str) -> String {
    match truncated {
        Some(why) => format!("{marker}{}\n{nt}", why.replace(['\r', '\n'], " ")),
        None => nt,
    }
}

/// `bytes` as text, or why they are not: an import is read as UTF-8, as
/// a workspace file is, and never with its bad bytes replaced.
pub(super) fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|e| {
        format!(
            "not valid UTF-8 (first bad byte at offset {})",
            e.valid_up_to()
        )
    })
}

/// Read an imported document. `guessed`: its format comes from [`sniff`],
/// not from an extension or a content type; Turtle then fails when every
/// triple is unresolved (a text in another syntax that reads as junk).
pub(super) fn parse(
    bytes: &[u8],
    format: Format,
    base: &str,
    guessed: bool,
) -> Result<Ontology, String> {
    let text = utf8(bytes)?.trim_start_matches('\u{feff}');
    // Whatever the name or the content type says: Manchester is refused,
    // and the text decides for the OWL syntaxes ([`by_content`]).
    let format = by_content(Sniffed::of(text).format()?, format);
    let mut truncated = None;
    let text = match format {
        Format::Turtle | Format::TriG => {
            // A cached copy of a converted document with an error: its
            // header line, then this line.
            truncated = text
                .lines()
                .take(2)
                .find_map(|l| {
                    [TRUNCATED, TRUNCATED_FUNCTIONAL, TRUNCATED_OWL_XML]
                        .iter()
                        .find_map(|m| l.strip_prefix(m))
                })
                .map(str::to_string);
            // Relative IRIs resolve against the import IRI; on the first line,
            // so line numbers stay those of the file.
            match oxiri::Iri::parse(base) {
                Ok(_) => format!("@base <{base}> . {text}"),
                Err(_) => text.to_string(),
            }
        }
        // N-Triples, N-Quads and Functional syntax have absolute IRIs only,
        // no base to declare; a JSON-LD, RDF/XML or OWL/XML document's base
        // is the URL the analysis is named by.
        Format::NTriples
        | Format::NQuads
        | Format::JsonLd
        | Format::RdfXml
        | Format::Functional
        | Format::OwlXml => text.to_string(),
    };
    let a = Analysis::with_syntax(
        base.to_string(),
        &text,
        Arc::new(PrefixMap::default()),
        false,
        format.syntax(),
    );
    // A warning is no error: the import is read whole past it.
    let first_error = a
        .parsed
        .errors
        .iter()
        .find(|e| e.severity == crate::syntax::Severity::Error);
    if matches!(format, Format::RdfXml | Format::Functional | Format::OwlXml) {
        // Read on past its errors; the first says where it went wrong.
        truncated = first_error.map(|e| stopped_at(&text, e));
    }
    if a.parsed.triples.is_empty()
        && let Some(e) = first_error
    {
        return Err(truncated.unwrap_or_else(|| e.message().to_string()));
    }
    if guessed
        && format == Format::Turtle
        && !a.parsed.triples.is_empty()
        && a.parsed.triples.iter().all(|t| t.unresolved)
    {
        let first = a.parsed.errors.first().map_or(String::new(), |e| {
            format!(" (first error: {})", e.message())
        });
        return Err(format!(
            "read as Turtle, none of its triples is usable: each has an undeclared prefix{first}. In Protégé: File > Save as > RDF/XML or Turtle"
        ));
    }
    let p = preds();
    let imports = a
        .parsed
        .triples
        .iter()
        .filter(|t| t.p == p.imports)
        .filter_map(|t| match &t.o {
            Node::Iri(o) => Some(o.to_string()),
            _ => None,
        })
        .collect();
    Ok(Ontology {
        analysis: Arc::new(a),
        imports,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imports::worker::Memo;

    #[test]
    fn formats_are_sniffed() {
        let sniffed = |b: &[u8]| sniff(b).unwrap();
        assert_eq!(sniffed(b"<?xml version='1.0'?><rdf:RDF/>"), Format::RdfXml);
        assert_eq!(
            sniffed(b"\xEF\xBB\xBF  <rdf:RDF xmlns:rdf='x'>"),
            Format::RdfXml
        );
        assert_eq!(
            sniffed(b"<http://ex.com/a> <http://ex.com/p> \"x\" ."),
            Format::Turtle
        );
        assert_eq!(sniffed(b"<urn:x:a> a <urn:x:C> ."), Format::Turtle);
        assert_eq!(sniffed(b"@prefix ex: <http://ex.com/> ."), Format::Turtle);
        assert_eq!(sniffed(b""), Format::Turtle);
        assert_eq!(sniffed(b" {\n \"@id\": \"x\"}"), Format::JsonLd);
        assert_eq!(sniffed(b"[ {\"@id\": \"x\"} ]"), Format::JsonLd);
        assert_eq!(sniffed(b"{}"), Format::JsonLd);
        // A blank node subject is Turtle, not an array.
        assert_eq!(sniffed(b"[ a <urn:x:C> ] <urn:x:p> 1 ."), Format::Turtle);
        // Cut inside a character, at the edge of what a sniff looks at.
        let mut big = "#".repeat(SNIFF_BYTES - 1);
        big.push('é');
        assert_eq!(sniff(big.as_bytes()), Ok(Format::Turtle));
        // An `Ontology` root in no namespace is not RDF/XML (whose elements
        // need one): it is taken for OWL/XML.
        assert_eq!(sniffed(b"<Ontology>\n  <Prefix/>"), Format::OwlXml);
        // OWL/XML past a declaration, a DOCTYPE and a byte order mark.
        let owx = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\"/>";
        let declared = format!("<?xml version=\"1.0\"?>\n{owx}");
        let doctype = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE Ontology [\n  <!ENTITY ex \"http://example.org/\">\n]>\n{owx}"
        );
        let bom = format!("\u{feff}{doctype}");
        for text in [owx, &declared, &doctype, &bom] {
            assert_eq!(sniffed(text.as_bytes()), Format::OwlXml, "{text}");
        }
        // Manchester syntax is refused, and says how to make RDF of it.
        assert_eq!(
            sniff(b"Prefix: ex: <http://example.org/o#>").unwrap_err(),
            "it holds OWL Manchester syntax, which turtle-lsp does not read. In Protégé: File > Save as > RDF/XML or Turtle"
        );
    }

    #[test]
    fn manchester_syntax_is_not_read_whatever_the_name_says() {
        let base = "http://example.org/o";
        let text = "Prefix: ex: <http://example.org/o#>";
        for format in [
            Format::Turtle,
            Format::RdfXml,
            Format::NTriples,
            Format::OwlXml,
        ] {
            let e = parse(text.as_bytes(), format, base, false).err().unwrap();
            assert!(
                e.starts_with("it holds OWL Manchester syntax, "),
                "{format:?}: {e}"
            );
        }
    }

    /// An OWL/XML text is read as OWL/XML whatever its name or content type
    /// says (a `.owl` name and an `application/rdf+xml` type give
    /// `Format::RdfXml`): its axioms as triples, its own imports, and its
    /// first error, as for a Functional text. Between the two XML formats
    /// the text decides, so RDF/XML under an OWL/XML name is RDF/XML.
    #[test]
    fn an_owl_xml_text_is_read_as_such_whatever_its_name_or_type_says() {
        let base = "file:///w/o.owx";
        let text = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n<Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n<Import>http://example.org/b</Import>\n<Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n<SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>\n</Ontology>\n";
        for format in [
            Format::Turtle,
            Format::RdfXml,
            Format::NTriples,
            Format::Functional,
            Format::OwlXml,
        ] {
            let o = parse(text.as_bytes(), format, base, false).unwrap();
            assert_eq!(o.analysis.parsed.syntax, Syntax::OwlXml, "{format:?}");
            assert!(
                o.analysis.parsed.errors.is_empty(),
                "{:?}",
                o.analysis.parsed.errors
            );
            assert_eq!(o.analysis.parsed.triples.len(), 4);
            assert_eq!(o.imports, ["http://example.org/b"]);
            assert_eq!(o.truncated, None);
        }
        // The same graph as its Functional form.
        let functional = "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\nImport(<http://example.org/b>)\nDeclaration(Class(:A))\nSubClassOf(:A :B)\n)\n";
        assert_eq!(
            owl_xml_to_ntriples(text.as_bytes(), base),
            functional_to_ntriples(functional.as_bytes(), base)
        );
        // RDF/XML under an OWL/XML name or type is RDF/XML.
        let rdf = "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:owl=\"http://www.w3.org/2002/07/owl#\">\n<owl:Ontology rdf:about=\"http://example.org/r\"><owl:imports rdf:resource=\"http://example.org/c\"/></owl:Ontology>\n</rdf:RDF>\n";
        let o = parse(rdf.as_bytes(), Format::OwlXml, base, false).unwrap();
        assert_eq!(o.analysis.parsed.syntax, Syntax::RdfXml);
        assert!(o.analysis.parsed.errors.is_empty());
        assert_eq!(o.imports, ["http://example.org/c"]);
        // An axiom with an error gives nothing; the rest is read, and the
        // first error says where it is.
        let broken = text.replace("\":B\"", "\"nope:B\"");
        let o = parse(broken.as_bytes(), Format::RdfXml, base, false).unwrap();
        assert_eq!(o.analysis.parsed.triples.len(), 3);
        assert_eq!(o.imports, ["http://example.org/b"]);
        let why = o.truncated.clone().expect("truncated");
        assert!(
            why.starts_with("line 6, column 70: Undeclared prefix 'nope:'"),
            "{why}"
        );
        // An error and no triple: the import is not read, and says why.
        let e = parse(
            b"<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" versionIRI=\"http://example.org/v\"/>",
            Format::OwlXml,
            base,
            false,
        )
        .err()
        .unwrap();
        assert!(e.starts_with("line 1, column "), "{e}");
        // Fetched, it is converted to N-Triples for the cache, which reads
        // back to the same graph, its error noted on the first line.
        let (nt, why) = owl_xml_to_ntriples(broken.as_bytes(), base).unwrap();
        assert_eq!(why.as_deref(), o.truncated.as_deref());
        let marked = marked(nt, why.as_deref(), TRUNCATED_OWL_XML);
        assert!(marked.starts_with("# turtle-lsp: the OWL/XML has an error at line 6, "));
        let cached = format!("# turtle-lsp cache for {base}\n{marked}");
        let cached = parse(cached.as_bytes(), Format::Turtle, base, false).unwrap();
        assert_eq!(cached.truncated, o.truncated);
        assert_eq!(cached.imports, o.imports);
        assert_eq!(cached.analysis.parsed.triples.len(), 3);
    }

    /// A warning is no error: an OWL/XML import whose only diagnostic is a
    /// warning (a reference OWL API joins to the `xml:base` otherwise) is
    /// read whole, fresh and from the cache.
    #[test]
    fn an_import_with_a_warning_only_is_not_truncated() {
        let base = "file:///w/o.owx";
        let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:base=\"http://example.org/lib\" ontologyIRI=\"http://example.org/lib\">\n<Declaration><Class IRI=\"/B\"/></Declaration>\n</Ontology>\n";
        let o = parse(text.as_bytes(), Format::OwlXml, base, false).unwrap();
        assert_eq!(o.analysis.parsed.errors.len(), 1);
        assert_eq!(o.analysis.parsed.triples.len(), 2);
        assert_eq!(o.truncated, None);
        let (nt, why) = owl_xml_to_ntriples(text.as_bytes(), base).unwrap();
        assert_eq!(why, None);
        assert_eq!(nt.lines().count(), 2);
    }

    /// The content type and the text, as a fetch weighs them: a Functional
    /// or an OWL/XML text is itself whatever it is served as, and an
    /// RDF/XML text is RDF/XML under the OWL/XML type.
    #[test]
    fn the_text_decides_between_the_xml_formats_and_for_functional_syntax() {
        use Format::*;
        for format in [
            Turtle, TriG, NTriples, NQuads, JsonLd, RdfXml, Functional, OwlXml,
        ] {
            assert_eq!(by_content(OwlXml, format), OwlXml, "{format:?}");
            assert_eq!(by_content(Functional, format), Functional, "{format:?}");
        }
        assert_eq!(by_content(RdfXml, OwlXml), RdfXml);
        assert_eq!(by_content(RdfXml, RdfXml), RdfXml);
        assert_eq!(by_content(Turtle, OwlXml), OwlXml);
        assert_eq!(by_content(Turtle, RdfXml), RdfXml);
        assert_eq!(by_content(Turtle, TriG), TriG);
        assert_eq!(by_content(JsonLd, Turtle), Turtle);
    }

    /// A Functional text is read as Functional syntax whatever its name or
    /// content type says: its axioms as triples, its own imports, and its
    /// first error, as for a truncated RDF/XML document.
    #[test]
    fn functional_syntax_is_read_whatever_the_name_says() {
        let base = "file:///w/o.ofn";
        let text = "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\nImport(<http://example.org/b>)\nDeclaration(Class(:A))\nSubClassOf(:A :B)\n)\n";
        for format in [
            Format::Turtle,
            Format::RdfXml,
            Format::NTriples,
            Format::Functional,
        ] {
            let o = parse(text.as_bytes(), format, base, false).unwrap();
            assert_eq!(
                o.analysis.parsed.syntax,
                Syntax::OwlFunctional,
                "{format:?}"
            );
            assert!(o.analysis.parsed.errors.is_empty());
            assert_eq!(o.analysis.parsed.triples.len(), 4);
            assert_eq!(o.imports, ["http://example.org/b"]);
            assert_eq!(o.truncated, None);
        }
        let broken = text.replace("SubClassOf(:A :B)", "SubClassOf(:A nope:B)");
        let o = parse(broken.as_bytes(), Format::Functional, base, false).unwrap();
        assert_eq!(o.analysis.parsed.triples.len(), 3);
        assert_eq!(
            o.truncated.as_deref(),
            Some("line 5, column 21: Undeclared prefix 'nope:'")
        );
        // An error and no triple: the import is not read, and says why.
        let e = parse(
            b"Prefix(:=<http://e/>) SubClassOf(",
            Format::Functional,
            base,
            false,
        )
        .err()
        .unwrap();
        assert_eq!(
            e,
            "line 1, column 33: Expected 'Prefix(' or 'Ontology(', found 'SubClassOf'"
        );
        // Fetched, it is converted to N-Triples for the cache, which reads
        // back to the same graph, its error noted on the first line.
        let (nt, why) = functional_to_ntriples(broken.as_bytes(), base).unwrap();
        assert_eq!(why.as_deref(), o.truncated.as_deref());
        let marked = marked(nt, why.as_deref(), TRUNCATED_FUNCTIONAL);
        let cached = parse(marked.as_bytes(), Format::Turtle, base, false).unwrap();
        assert_eq!(cached.truncated, o.truncated);
        assert_eq!(cached.imports, o.imports);
        assert_eq!(cached.analysis.parsed.triples.len(), 3);
    }

    #[test]
    fn sniffed_turtle_with_no_usable_triple_is_refused() {
        let base = "http://example.org/o";
        // Manchester the sniff does not know (a frame without a header).
        let junk = "Class: ex:Dog\n    SubClassOf: ex:Animal\n";
        let e = parse(junk.as_bytes(), Format::Turtle, base, true)
            .err()
            .unwrap();
        assert!(
            e.starts_with("read as Turtle, none of its triples is usable"),
            "{e}"
        );
        assert!(
            e.ends_with(". In Protégé: File > Save as > RDF/XML or Turtle"),
            "{e}"
        );
        // Named Turtle, empty Turtle, and Turtle with one usable triple load.
        assert!(parse(junk.as_bytes(), Format::Turtle, base, false).is_ok());
        assert!(parse(b"", Format::Turtle, base, true).is_ok());
        let some = "ex:a ex:p ex:b .\n<http://example.org/a> a <http://example.org/C> .\n";
        assert!(parse(some.as_bytes(), Format::Turtle, base, true).is_ok());
    }

    /// Bytes that are not UTF-8 fail the import, in every format, and are
    /// never read with a replacement character.
    #[test]
    fn an_import_that_is_not_utf8_is_not_read() {
        let base = "http://example.org/o";
        let xml = b"<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\"><rdf:Description rdf:about=\"http://example.org/a\" ex:p=\"a\xFFb\"/></rdf:RDF>\n";
        let at = xml.iter().position(|&b| b == 0xFF).unwrap();
        let want = format!("not valid UTF-8 (first bad byte at offset {at})");
        for format in [
            Format::RdfXml,
            Format::Turtle,
            Format::NTriples,
            Format::JsonLd,
        ] {
            assert_eq!(
                parse(xml, format, base, true).err(),
                Some(want.clone()),
                "{format:?}"
            );
        }
        let ttl = b"<http://example.org/a> <http://example.org/p> \"a\xFFb\" .\n";
        let e = parse(ttl, Format::Turtle, base, false).err();
        assert_eq!(
            e.as_deref(),
            Some("not valid UTF-8 (first bad byte at offset 48)")
        );
        // Valid UTF-8, with a byte order mark, reads.
        let fixed = [
            b"\xEF\xBB\xBF".as_slice(),
            &String::from_utf8_lossy(xml).into_owned().into_bytes(),
        ]
        .concat();
        assert_eq!(
            parse(&fixed, Format::RdfXml, base, true)
                .unwrap()
                .analysis
                .parsed
                .triples
                .len(),
            1
        );
    }

    #[test]
    fn a_truncated_rdf_xml_says_so_fresh_from_the_memo_and_from_the_cache() {
        let xml = "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\">\n  <ex:A rdf:about=\"http://example.org/a\"/>\n  <ex:B rdf:about=\"http://example.org/b\"></ex:C>\n</rdf:RDF>\n";
        let base = "http://example.org/o";
        let o = parse(xml.as_bytes(), Format::RdfXml, base, true).unwrap();
        let why = o.truncated.clone().expect("truncated");
        assert!(why.starts_with("line 4, column "), "{why}");
        // `ex:A`, and the type of `ex:B`, from its start tag.
        assert_eq!(o.analysis.parsed.triples.len(), 2);
        // A whole document is not.
        let whole = xml.replace("</ex:C>", "</ex:B>");
        let o = parse(whole.as_bytes(), Format::RdfXml, base, true).unwrap();
        assert_eq!(o.truncated, None);
        // A cached copy carries the line that says so.
        let (nt, stop) = rdf_xml_to_ntriples(xml.as_bytes(), base).unwrap();
        let cached = format!(
            "# turtle-lsp cache for {base}\n{}",
            marked(nt, stop.as_deref(), TRUNCATED)
        );
        let o = parse(cached.as_bytes(), Format::Turtle, base, false).unwrap();
        assert_eq!(o.truncated.as_deref(), Some(why.as_str()));
        assert_eq!(o.analysis.parsed.triples.len(), 2);
        assert!(o.analysis.parsed.errors.is_empty());
        // From the memo, as fresh.
        let dir = std::env::temp_dir().join(format!("turtle-truncated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("onto.owl");
        std::fs::write(&file, xml).unwrap();
        let mut memo = Memo::default();
        let (first, fresh) = load_file(&file, None, base, &mut memo).unwrap();
        assert!(fresh);
        let (again, fresh) = load_file(&file, None, base, &mut memo).unwrap();
        assert!(!fresh);
        assert_eq!(first.truncated.as_deref(), Some(why.as_str()));
        assert_eq!(again.truncated, first.truncated);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A local `.owl` in an OWL syntax that is also not UTF-8 is refused
    /// for its bytes, with the offset, before its syntax is looked at.
    #[test]
    fn a_local_import_that_is_not_utf8_says_so_before_naming_its_syntax() {
        let dir = std::env::temp_dir().join(format!("turtle-local-utf8-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let head = "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\n";
        for pad in [0, SNIFF_BYTES] {
            let mut bytes = head.as_bytes().to_vec();
            bytes.extend(std::iter::repeat_n(b' ', pad));
            let at = bytes.len();
            bytes.extend(b"# a\xFFb\n)\n");
            let file = dir.join(format!("onto-{pad}.owl"));
            std::fs::write(&file, &bytes).unwrap();
            let e = load_file(&file, None, "http://example.org/o", &mut Memo::default())
                .err()
                .unwrap();
            assert_eq!(
                e,
                format!("not valid UTF-8 (first bad byte at offset {})", at + 3),
                "bad byte at {at}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_format_comes_from_the_extension_first_then_the_content_never_the_iri() {
        let ttl = b"<urn:x:a> a <urn:x:C> .";
        let xml = b"<?xml version='1.0'?><rdf:RDF/>";
        let of = |name: &str, b: &[u8]| format_of(name, b).unwrap();
        assert_eq!(of("/w/a.trig", ttl), (Format::TriG, false));
        assert_eq!(of("/w/a.nq", ttl), (Format::NQuads, false));
        assert_eq!(of("/w/a.nt", ttl), (Format::NTriples, false));
        assert_eq!(of("/w/a.jsonld", ttl), (Format::JsonLd, false));
        assert_eq!(of("C:\\w\\a.ttl", xml), (Format::Turtle, false));
        // No extension of ours: what the document starts with decides.
        assert_eq!(of("/w/a.owl", xml), (Format::RdfXml, true));
        assert_eq!(of("/w/a.owl", ttl), (Format::Turtle, true));
        assert_eq!(of("/w/a", b"{\"@id\": \"x\"}"), (Format::JsonLd, true));
        assert_eq!(of("", ttl), (Format::Turtle, true));
        assert_eq!(
            of("/w/a.owl", b"Ontology(<http://e/>)"),
            (Format::Functional, true)
        );
        assert!(format_of("/w/a.ttl", b"Ontology(<http://e/>)").is_ok());
        // A `.ofn` name is Functional whatever its text.
        for text in [&b"Ontology(<http://e/>)"[..], ttl] {
            assert_eq!(of("/w/a.ofn", text), (Format::Functional, false));
        }
        assert_eq!(of("/w/a.OFN", b""), (Format::Functional, false));
        // The registered type of Functional syntax.
        assert_eq!(
            format_of_type("text/owl-functional"),
            Some(Format::Functional)
        );
        assert_eq!(format_of_type("application/trig"), Some(Format::TriG));
        assert_eq!(format_of_type("application/ld+json"), Some(Format::JsonLd));
        assert_eq!(
            format_of_type("application/rdf+xml; charset=utf-8"),
            Some(Format::RdfXml)
        );
        assert_eq!(format_of_type("text/plain"), None);
        // The type the OWL/XML Recommendation names (section 6); the other
        // XML types are RDF/XML.
        assert_eq!(format_of_type("application/owl+xml"), Some(Format::OwlXml));
        assert_eq!(
            format_of_type("application/owl+xml; charset=utf-8"),
            Some(Format::OwlXml)
        );
        assert_eq!(format_of_type("application/rdf+xml"), Some(Format::RdfXml));
        assert_eq!(format_of_type("application/xml"), Some(Format::RdfXml));
        // A `.owx` name is OWL/XML whatever its text; a `.owl` that holds
        // OWL/XML is sniffed so.
        let owx = b"<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"/>";
        assert_eq!(of("/w/a.owx", ttl), (Format::OwlXml, false));
        assert_eq!(of("/w/a.OWX", owx), (Format::OwlXml, false));
        assert_eq!(of("/w/a.owl", owx), (Format::OwlXml, true));
    }

    #[test]
    fn a_host_in_a_file_url_is_refused_without_asking_the_file_system() {
        let root = std::env::temp_dir();
        for url in [
            "file://example.invalid/s/x.ttl",
            "file://server/share/x.ttl",
            "file://192.0.2.1/x.ttl",
            "FILE://Example.Invalid/s/x.ttl",
        ] {
            let e = fs_policy(url, std::slice::from_ref(&root)).unwrap_err();
            assert!(e.contains("naming a host"), "{url}: {e}");
            // It is refused by its name alone: the same answer when the
            // roots do not exist, so no path was looked at.
            let none = [PathBuf::from("/nonexistent-turtle-lsp-root")];
            assert_eq!(fs_policy(url, &none).unwrap_err(), e);
        }
        assert!(fs_policy("http://example.org/x.ttl", &[root]).is_err());
    }

    #[test]
    fn localhost_and_an_empty_host_name_local_files() {
        let dir = std::env::temp_dir().join(format!("turtle-fs-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ttl");
        std::fs::write(&file, "").unwrap();
        let roots = [dir.clone()];
        let plain = url::Url::from_file_path(&file).unwrap();
        assert_eq!(fs_policy(plain.as_str(), &roots), Ok(file.clone()));
        let local = plain.as_str().replacen("file://", "file://localhost", 1);
        assert_eq!(fs_policy(&local, &roots), Ok(file));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_is_compared_with_the_roots_by_name_before_the_file_system_is_asked() {
        let dir = std::env::temp_dir().join(format!("turtle-fs-lex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ws = dir.join("ws");
        std::fs::create_dir_all(ws.join("sub")).unwrap();
        std::fs::write(dir.join("secret.ttl"), "").unwrap();
        std::fs::write(ws.join("sub").join("a.ttl"), "").unwrap();
        let roots = [ws.clone()];
        // `..` that leaves the root, through an existing and a missing name.
        let escapes = [
            ws.join("sub").join("..").join("..").join("secret.ttl"),
            ws.join("..").join("secret.ttl"),
            ws.join("..").join("missing.ttl"),
            dir.join("secret.ttl"),
            dir.join("missing.ttl"),
            PathBuf::from("/nonexistent/x.ttl"),
        ];
        // One answer for a file that exists and one that does not: nothing
        // can be learned about paths outside the roots.
        for p in &escapes {
            assert_eq!(fs_policy_path(p, &roots).unwrap_err(), OUTSIDE, "{p:?}");
        }
        // A `..` that stays inside is fine.
        let inside = ws.join("sub").join("..").join("sub").join("a.ttl");
        assert_eq!(fs_policy_path(&inside, &roots), Ok(inside.clone()));
        // A name that is inside but missing says the same as outside.
        let e = fs_policy_path(&ws.join("sub").join("nope.ttl"), &roots).unwrap_err();
        assert_eq!(e, OUTSIDE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_workspace_to_a_file_outside_is_refused() {
        let dir = std::env::temp_dir().join(format!("turtle-fs-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(dir.join("secret.ttl"), "").unwrap();
        std::os::unix::fs::symlink(dir.join("secret.ttl"), ws.join("link.ttl")).unwrap();
        std::os::unix::fs::symlink(dir.join("missing.ttl"), ws.join("dangling.ttl")).unwrap();
        let roots = [ws.clone()];
        assert_eq!(
            fs_policy_path(&ws.join("link.ttl"), &roots).unwrap_err(),
            OUTSIDE
        );
        assert_eq!(
            fs_policy_path(&ws.join("dangling.ttl"), &roots).unwrap_err(),
            OUTSIDE
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_paths_are_recognised_only_where_they_exist() {
        let unc = Path::new(r"\\example.invalid\s\x.ttl");
        let verbatim = Path::new(r"\\?\UNC\example.invalid\s\x.ttl");
        let long_local = Path::new(r"\\?\C:\w\x.ttl");
        assert_eq!(is_network_path(unc), cfg!(windows));
        assert_eq!(is_network_path(verbatim), cfg!(windows));
        assert!(!is_network_path(long_local));
        assert!(!is_network_path(Path::new("/w/x.ttl")));
        assert!(!is_network_path(Path::new("C:\\w\\x.ttl")));
    }

    /// On Windows a `file://host/share/x` URL would open an SMB connection if
    /// anything asked the file system about it. The answer is the refusal of
    /// the host (an operating-system error would come from the file system).
    #[cfg(windows)]
    #[test]
    fn a_share_is_not_touched_on_windows() {
        let roots = [std::env::temp_dir()];
        let e = fs_policy("file://example.invalid/s/x.ttl", &roots).unwrap_err();
        assert!(e.contains("naming a host"), "{e}");
        let e = fs_policy_path(Path::new(r"\\example.invalid\s\x.ttl"), &roots).unwrap_err();
        assert_eq!(e, NETWORK_PATH);
        // A UNC root makes its own shares readable by name.
        let unc = [PathBuf::from(r"\\example.invalid\s")];
        assert_eq!(
            fs_policy_path(Path::new(r"\\example.invalid\other\x.ttl"), &unc).unwrap_err(),
            OUTSIDE
        );
    }
}
