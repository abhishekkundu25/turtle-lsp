//! Loading one import: a file in the workspace, a cached copy or a web
//! document, and reading it as Turtle or RDF/XML.

use super::catalog::{Catalog, Target};
use super::rdfxml::rdf_xml_to_ntriples;
use super::worker::{Memo, Ontology, RETRY_FETCH_AFTER};
use super::*;

/// Local imports are read only from inside the workspace: a cloned repository
/// must not make the editor read arbitrary files of the user's.
pub(crate) fn inside_workspace(path: &Path, roots: &[PathBuf]) -> Result<(), String> {
    let real = path.canonicalize().map_err(|e| e.to_string())?;
    if roots
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .any(|r| real.starts_with(r))
    {
        Ok(())
    } else {
        Err("it is outside the workspace, so it is not read".into())
    }
}

/// Load one import: (where from, the ontology, freshly parsed).
pub(super) fn load(
    iri: &str,
    req: &Request,
    catalog: &Catalog,
    memo: &mut Memo,
) -> Result<(String, Ontology, bool), String> {
    let base = without_fragment(iri);
    let mut url = base.to_string();
    if let Some((target, cat)) = catalog.lookup(iri) {
        match target {
            Target::File(path) => {
                inside_workspace(path, &req.roots).map_err(|e| {
                    format!(
                        "Import <{iri}> is mapped to {} by {}: {e}",
                        path.display(),
                        cat.display()
                    )
                })?;
                return load_file(path, None, base, memo)
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
    if let Some(path) = url::Url::parse(&url)
        .ok()
        .filter(|u| u.scheme() == "file")
        .and_then(|u| crate::workspace::file_path_of(&u))
    {
        inside_workspace(&path, &req.roots)
            .map_err(|e| format!("Import <{iri}> names {}: {e}", path.display()))?;
        return load_file(&path, None, base, memo)
            .map(|(o, fresh)| (path.display().to_string(), o, fresh))
            .map_err(|e| {
                format!(
                    "Import <{iri}> could not be loaded from {}: {e}",
                    path.display()
                )
            });
    }
    let cached = req.cache.as_ref().map(|dir| cache_file(dir, &url));
    if let Some(path) = cached.as_ref().filter(|p| p.is_file())
        && let Ok((o, fresh)) = load_file(path, Some(&url), base, memo)
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
    if let Some((at, msg)) = memo.failed.get(&url)
        && at.elapsed() < RETRY_FETCH_AFTER
    {
        return Err(msg.clone());
    }
    let fetched = fetch_turtle(iri, &url).and_then(|text| {
        let parsed = parse(text.as_bytes(), Format::Turtle, base).map_err(|e| {
            format!("Import <{iri}> was fetched from {url} but could not be read: {e}")
        })?;
        Ok((text, parsed))
    });
    let (text, mut parsed) = match fetched {
        Ok(v) => v,
        Err(msg) => {
            memo.failed.insert(url, (Instant::now(), msg.clone()));
            return Err(msg);
        }
    };
    let a = Arc::get_mut(&mut parsed.analysis).expect("fresh analysis");
    a.uri = url.clone();
    // A failed write only costs a fetch next time; locations then name the URL.
    if let Some(path) = &cached
        && write_atomic(path, text.as_bytes()).is_ok()
        && let Some(file) = path_to_uri(path)
    {
        a.uri = file;
        a.origin = Some(url.clone());
    }
    Ok((url, parsed, true))
}

/// Fetch `url` as Turtle text (RDF/XML is converted to N-Triples) whose
/// relative IRIs, if any, resolve against where the document was finally
/// found, after redirects. This text is what the cache holds.
fn fetch_turtle(iri: &str, url: &str) -> Result<String, String> {
    let (bytes, content_type, final_url) = fetch(url).map_err(|e| {
        format!(
            "Import <{iri}> could not be fetched from {url}: {e}. Map it to a local file in a catalog-v001.xml"
        )
    })?;
    let format = match content_type.as_deref().map(str::to_ascii_lowercase) {
        Some(ct) if ct.contains("xml") => Format::RdfXml,
        Some(ct) if ct.contains("turtle") || ct.contains("n-triples") => Format::Turtle,
        _ => sniff(&bytes),
    };
    Ok(match format {
        Format::Turtle => {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.trim_start_matches('\u{feff}');
            match oxiri::Iri::parse(final_url.as_str()) {
                Ok(_) => format!("@base <{final_url}> .\n{text}"),
                Err(_) => text.to_string(),
            }
        }
        Format::RdfXml => rdf_xml_to_ntriples(&bytes, &final_url).map_err(|e| {
            format!("Import <{iri}> was fetched from {url} but could not be read: {e}")
        })?,
    })
}

/// Load an ontology file; its analysis is named by the file's URI. `origin` is
/// the URL a cached copy came from.
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
    let format = match path.extension().and_then(|e| e.to_str()) {
        Some("ttl" | "turtle" | "nt") => Format::Turtle,
        _ => sniff(&bytes),
    };
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Turtle,
    RdfXml,
}

/// RDF/XML or Turtle (N-Triples is Turtle), from the first bytes.
pub fn sniff(bytes: &[u8]) -> Format {
    let s = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let s = s.trim_start_matches('\u{feff}').trim_start();
    if s.starts_with("<?xml") || s.starts_with("<!") {
        return Format::RdfXml;
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
    let text = match format {
        Format::Turtle => {
            let text = String::from_utf8_lossy(bytes);
            let text = text.trim_start_matches('\u{feff}');
            // Relative IRIs resolve against the import IRI; on the first line,
            // so line numbers stay those of the file.
            match oxiri::Iri::parse(base) {
                Ok(_) => format!("@base <{base}> . {text}"),
                Err(_) => text.to_string(),
            }
        }
        Format::RdfXml => rdf_xml_to_ntriples(bytes, base)?,
    };
    let a = Analysis::new(
        base.to_string(),
        &text,
        Arc::new(PrefixMap::default()),
        false,
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

/// GET `url`: (body, content type, final URL after redirects).
fn fetch(url: &str) -> Result<(Vec<u8>, Option<String>, String), String> {
    http_get(url, ACCEPT)
        .map(|r| (r.body, r.content_type, r.final_url))
        .map_err(|e| e.message)
}

/// An HTTP response read by [`http_get`].
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) struct HttpResponse {
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    /// Where the body was finally read from, after redirects.
    pub final_url: String,
    /// The `Link` header, if any.
    pub link: Option<String>,
}

/// Why [`http_get`] failed: the HTTP status, if the server answered.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) struct HttpError {
    pub status: Option<u16>,
    pub message: String,
}

const ACCEPT: &str = "text/turtle, application/n-triples;q=0.9, application/rdf+xml;q=0.8";

/// GET `url` with `accept`, following redirects, within [`FETCH_TIMEOUT`]
/// and [`MAX_FETCH_BYTES`].
#[cfg(target_arch = "wasm32")]
pub(crate) fn http_get(_url: &str, _accept: &str) -> Result<HttpResponse, HttpError> {
    Err(HttpError {
        status: None,
        message: "the browser build does not fetch".into(),
    })
}

/// GET `url` with `accept`, following redirects, within [`FETCH_TIMEOUT`]
/// and [`MAX_FETCH_BYTES`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn http_get(url: &str, accept: &str) -> Result<HttpResponse, HttpError> {
    use ureq::ResponseExt;
    let failed = |e: ureq::Error| HttpError {
        status: match e {
            ureq::Error::StatusCode(s) => Some(s),
            _ => None,
        },
        message: e.to_string(),
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .max_redirects(10)
        .user_agent(concat!("turtle-lsp/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("Accept", accept)
        .call()
        .map_err(failed)?;
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let link = header("link");
    let final_url = resp.get_uri().to_string();
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_FETCH_BYTES)
        .read_to_vec()
        .map_err(failed)?;
    Ok(HttpResponse {
        body,
        content_type,
        final_url,
        link,
    })
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The cache file for `url`: Turtle (RDF/XML is cached as N-Triples).
fn cache_file(dir: &Path, url: &str) -> PathBuf {
    cache_file_as(dir, url, "ttl")
}

/// The cache file for `url` holding a document of kind `extension` (JSON-LD
/// contexts are cached as `.jsonld` beside the imports).
pub(crate) fn cache_file_as(dir: &Path, url: &str, extension: &str) -> PathBuf {
    dir.join(format!("{}.{extension}", hash_name(url)))
}

/// 64-bit FNV-1a of `s`, as 16 hex digits.
pub fn hash_name(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
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
    }

    #[test]
    fn hash_names_are_stable() {
        assert_eq!(hash_name(""), "cbf29ce484222325");
        assert_ne!(hash_name("http://a/"), hash_name("http://b/"));
        assert_eq!(hash_name("x").len(), 16);
    }
}
