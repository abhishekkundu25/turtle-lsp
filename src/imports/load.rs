//! Loading one import: a file in the workspace, a cached copy or a web
//! document, and reading it in its own syntax.
//!
//! A `file:` import is read only through [`fs_policy`]; a web import only
//! through [`crate::fetch`], which holds the address, redirect and budget
//! rules. Their format comes from the file's extension, else from what the
//! document starts with, else (a web document) from its content type: never
//! from the import IRI.

use super::catalog::{Catalog, Target};
use super::rdfxml::rdf_xml_to_ntriples;
use super::worker::{Memo, Ontology, RETRY_FETCH_AFTER};
use super::*;
use crate::syntax::Syntax;
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
pub fn fs_policy(url: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
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
pub fn fs_policy_path(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, String> {
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
pub fn is_network_path(path: &Path) -> bool {
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

/// Load one import: (where from, the ontology, freshly parsed).
pub(super) fn load(
    iri: &str,
    req: &Request,
    catalog: &Catalog,
    memo: &mut Memo,
    budget: &mut Budget,
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
    let (text, format, final_url) = match fetch_document(iri, &url, req.allow_private, budget) {
        Ok(v) => v,
        Err(f) => {
            if !f.budget {
                remember(memo, &f.message);
            }
            return Err(f.message);
        }
    };
    // What the cache holds is what is parsed, so its lines are the file's.
    let text = match format {
        Format::JsonLd => text,
        _ => stamped(&url, &text),
    };
    let mut parsed = match parse(text.as_bytes(), format, base) {
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

/// What a web import is asked for: its content types, best first.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const ACCEPT: &str = "text/turtle, application/trig;q=0.9, application/n-triples;q=0.9, application/n-quads;q=0.9, application/ld+json;q=0.8, application/rdf+xml;q=0.8";

/// Fetch `url` and bring it to text that [`parse`] reads in the returned
/// format (RDF/XML becomes N-Triples, read as Turtle), whose relative IRIs,
/// if any, resolve against where the document was finally found, after
/// redirects. Also that final URL.
#[cfg(not(target_arch = "wasm32"))]
fn fetch_document(
    iri: &str,
    url: &str,
    allow_private: bool,
    budget: &mut Budget,
) -> Result<(String, Format, String), Failure> {
    use crate::fetch;
    let policy = fetch::Policy { allow_private };
    let r = fetch::get(url, ACCEPT, policy, budget).map_err(|e| Failure {
        message: format!(
            "Import <{iri}> could not be fetched from {url}: {}. Map it to a local file in a catalog-v001.xml",
            e.message
        ),
        budget: e.budget,
    })?;
    let by_type = r
        .content_type
        .as_deref()
        .and_then(|ct| format_of_type(&ct.to_ascii_lowercase()));
    let format = by_type.unwrap_or_else(|| {
        let path = url::Url::parse(&r.final_url)
            .map(|u| u.path().to_string())
            .unwrap_or_default();
        format_of(&path, &r.body)
    });
    let lossy = String::from_utf8_lossy(&r.body);
    let lossy = lossy.trim_start_matches('\u{feff}');
    let text = match format {
        Format::Turtle | Format::TriG => match oxiri::Iri::parse(r.final_url.as_str()) {
            Ok(_) => format!("@base <{}> .\n{lossy}", r.final_url),
            Err(_) => lossy.to_string(),
        },
        Format::NTriples | Format::NQuads | Format::JsonLd => lossy.to_string(),
        Format::RdfXml => rdf_xml_to_ntriples(&r.body, &r.final_url).map_err(|e| Failure {
            message: format!("Import <{iri}> was fetched from {url} but could not be read: {e}"),
            budget: false,
        })?,
    };
    let format = if format == Format::RdfXml {
        Format::Turtle
    } else {
        format
    };
    Ok((text, format, r.final_url))
}

/// The browser build fetches nothing.
#[cfg(target_arch = "wasm32")]
fn fetch_document(
    iri: &str,
    url: &str,
    _allow_private: bool,
    _budget: &mut Budget,
) -> Result<(String, Format, String), Failure> {
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
    if ct.contains("xml") {
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
    if format == Format::JsonLd {
        write_atomic(&sidecar(&path), sidecar_text(url, final_url).as_bytes()).ok()?;
    }
    write_atomic(&path, text.as_bytes()).ok()?;
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
    let format = format_of(&path.to_string_lossy(), &bytes);
    let mut o = parse(&bytes, format, base)?;
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
}

impl Format {
    fn of_syntax(syntax: Syntax) -> Format {
        match syntax {
            Syntax::Turtle => Format::Turtle,
            Syntax::TriG => Format::TriG,
            Syntax::NTriples => Format::NTriples,
            Syntax::NQuads => Format::NQuads,
            Syntax::JsonLd => Format::JsonLd,
        }
    }

    /// The syntax the analysis reads it in. RDF/XML is converted to
    /// N-Triples text first, which is read as Turtle.
    pub fn syntax(self) -> Syntax {
        match self {
            Format::Turtle | Format::RdfXml => Syntax::Turtle,
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
            Format::Turtle | Format::RdfXml => "ttl",
            Format::TriG => "trig",
            Format::NTriples => "nt",
            Format::NQuads => "nq",
            Format::JsonLd => "jsonld",
        }
    }
}

/// The format of the document called `name` (a path or URL path) whose bytes
/// are `bytes`: by the extension if it is one of ours, else by [`sniff`].
pub fn format_of(name: &str, bytes: &[u8]) -> Format {
    match Syntax::from_extension(name) {
        Some(syntax) => Format::of_syntax(syntax),
        None => sniff(bytes),
    }
}

/// RDF/XML, JSON-LD or Turtle (N-Triples is Turtle), from the first bytes.
pub fn sniff(bytes: &[u8]) -> Format {
    let s = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let s = s.trim_start_matches('\u{feff}').trim_start();
    if s.starts_with("<?xml") || s.starts_with("<!") {
        return Format::RdfXml;
    }
    // JSON: an object that starts with a key (or is empty), or an array of objects.
    if let Some(rest) = s.strip_prefix('{') {
        let rest = rest.trim_start();
        if rest.starts_with('"') || rest.starts_with('}') {
            return Format::JsonLd;
        }
    }
    if let Some(rest) = s.strip_prefix('[')
        && rest.trim_start().starts_with('{')
    {
        return Format::JsonLd;
    }
    let Some(rest) = s.strip_prefix('<') else {
        return Format::Turtle;
    };
    let name_len = rest
        .find(|c: char| !(c.is_alphanumeric() || "_-.:".contains(c)))
        .unwrap_or(rest.len());
    let after = &rest[name_len..];
    let xml = name_len > 0
        && (after.starts_with(char::is_whitespace)
            || after.starts_with("/>")
            || after
                .strip_prefix('>')
                .is_some_and(|t| t.trim_start().starts_with('<')));
    if xml { Format::RdfXml } else { Format::Turtle }
}

pub(super) fn parse(bytes: &[u8], format: Format, base: &str) -> Result<Ontology, String> {
    let lossy = String::from_utf8_lossy(bytes);
    let text = lossy.trim_start_matches('\u{feff}');
    let text = match format {
        Format::Turtle | Format::TriG => {
            // Relative IRIs resolve against the import IRI; on the first line,
            // so line numbers stay those of the file.
            match oxiri::Iri::parse(base) {
                Ok(_) => format!("@base <{base}> . {text}"),
                Err(_) => text.to_string(),
            }
        }
        // N-Triples and N-Quads have absolute IRIs only, no base to declare;
        // a JSON-LD document's base is the URL the analysis is named by.
        Format::NTriples | Format::NQuads | Format::JsonLd => text.to_string(),
        Format::RdfXml => rdf_xml_to_ntriples(bytes, base)?,
    };
    let a = Analysis::with_syntax(
        base.to_string(),
        &text,
        Arc::new(PrefixMap::default()),
        false,
        format.syntax(),
    );
    if a.parsed.triples.is_empty()
        && let Some(e) = a.parsed.errors.first()
    {
        return Err(e.message.clone());
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_sniffed() {
        assert_eq!(sniff(b"<?xml version='1.0'?><rdf:RDF/>"), Format::RdfXml);
        assert_eq!(
            sniff(b"\xEF\xBB\xBF  <rdf:RDF xmlns:rdf='x'>"),
            Format::RdfXml
        );
        assert_eq!(sniff(b"<Ontology>\n  <Prefix/>"), Format::RdfXml);
        assert_eq!(
            sniff(b"<http://ex.com/a> <http://ex.com/p> \"x\" ."),
            Format::Turtle
        );
        assert_eq!(sniff(b"<urn:x:a> a <urn:x:C> ."), Format::Turtle);
        assert_eq!(sniff(b"@prefix ex: <http://ex.com/> ."), Format::Turtle);
        assert_eq!(sniff(b""), Format::Turtle);
        assert_eq!(sniff(b" {\n \"@id\": \"x\"}"), Format::JsonLd);
        assert_eq!(sniff(b"[ {\"@id\": \"x\"} ]"), Format::JsonLd);
        assert_eq!(sniff(b"{}"), Format::JsonLd);
        // A blank node subject is Turtle, not an array.
        assert_eq!(sniff(b"[ a <urn:x:C> ] <urn:x:p> 1 ."), Format::Turtle);
    }

    #[test]
    fn the_format_comes_from_the_extension_first_then_the_content_never_the_iri() {
        let ttl = b"<urn:x:a> a <urn:x:C> .";
        let xml = b"<?xml version='1.0'?><rdf:RDF/>";
        assert_eq!(format_of("/w/a.trig", ttl), Format::TriG);
        assert_eq!(format_of("/w/a.nq", ttl), Format::NQuads);
        assert_eq!(format_of("/w/a.nt", ttl), Format::NTriples);
        assert_eq!(format_of("/w/a.jsonld", ttl), Format::JsonLd);
        assert_eq!(format_of("C:\\w\\a.ttl", xml), Format::Turtle);
        // No extension of ours: what the document starts with decides.
        assert_eq!(format_of("/w/a.owl", xml), Format::RdfXml);
        assert_eq!(format_of("/w/a.owl", ttl), Format::Turtle);
        assert_eq!(format_of("/w/a", b"{\"@id\": \"x\"}"), Format::JsonLd);
        assert_eq!(format_of("", ttl), Format::Turtle);
        assert_eq!(format_of_type("application/trig"), Some(Format::TriG));
        assert_eq!(format_of_type("application/ld+json"), Some(Format::JsonLd));
        assert_eq!(
            format_of_type("application/rdf+xml; charset=utf-8"),
            Some(Format::RdfXml)
        );
        assert_eq!(format_of_type("text/plain"), None);
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
