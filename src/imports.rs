//! `owl:imports` resolution.
//!
//! Every `owl:imports <iri>` in the workspace is resolved, in this order:
//! 1. an ontology the workspace describes (`<iri> a owl:Ontology`, or some
//!    ontology's `owl:versionIRI <iri>`): nothing to load;
//! 2. a `catalog-v001.xml` (Protégé convention) anywhere under the workspace
//!    roots, mapping the IRI to a file (relative to the catalog) or a URL;
//! 3. the per-user import cache;
//! 4. the network, only when `turtle.imports.network` is on (then cached).
//!
//! Loaded ontologies are resolved transitively (up to [`MAX_DEPTH`] levels
//! below a workspace import, cycles stop) and join the knowledge index as
//! read-only external sources: hover, completion and lints know their terms,
//! but they are never diagnosed. Turtle and N-Triples are read with our parser;
//! RDF/XML is converted to N-Triples text first, so every source is an
//! [`Analysis`] like any other.
//!
//! All file and network work happens on one background thread, started on the
//! first import and blocked on its job channel when there is nothing to do.
//! Each job carries a generation; a newer job supersedes an unfinished one and
//! results of older jobs are dropped. The main loop selects on
//! [`Imports::results`].

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::SOURCE;
use crate::knowledge::{DocFacts, Knowledge, Source};
use crate::syntax::{self, Iri, Node, PrefixMap};
use crate::workspace::{MAX_FILE_BYTES, MAX_FILES, SKIP_DIRS, path_to_uri};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};

pub const OWL_IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";
const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";
const OWL_VERSION_IRI: &str = "http://www.w3.org/2002/07/owl#versionIRI";

pub const CATALOG_FILE: &str = "catalog-v001.xml";
/// Levels of imports followed below a workspace import.
pub const MAX_DEPTH: u32 = 5;
pub const MAX_FETCH_BYTES: u64 = 20 * 1024 * 1024;
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPT: &str = "text/turtle, application/n-triples;q=0.9, application/rdf+xml;q=0.8";
/// RDF/XML with a longer start tag is refused: quick-xml's cost per start tag
/// grows faster than its size (RUSTSEC-2026-0194, RUSTSEC-2026-0195).
const MAX_XML_TAG_BYTES: usize = 64 * 1024;
/// Prefix for external source keys in the knowledge index.
const KEY_PREFIX: &str = "import:";

/// How an import was resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Described in the workspace.
    Local,
    /// Loaded from this file or URL.
    Loaded(String),
    /// Not loaded; the message says why and how to fix it.
    Unresolved(String),
}

/// Everything a job needs; two equal requests give the same answer.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    /// Imports stated in workspace documents (sorted, unique).
    imports: Vec<String>,
    /// Ontology IRIs the workspace describes (sorted, unique).
    local: Vec<String>,
    roots: Vec<PathBuf>,
    cache: Option<PathBuf>,
    network: bool,
}

struct Job {
    generation: u64,
    req: Request,
}

/// One loaded ontology.
pub struct Loaded {
    key: String,
    analysis: Arc<Analysis>,
    /// Computed on the worker when the ontology was parsed for this job.
    facts: Option<DocFacts>,
}

/// The answer to one job.
pub struct Done {
    generation: u64,
    status: Vec<(String, Status)>,
    loaded: Vec<Loaded>,
}

/// What one workspace document says about ontologies.
#[derive(Default)]
struct DocOnto {
    imports: Vec<Iri>,
    ontologies: Vec<Iri>,
}

struct Preds {
    imports: Iri,
    rdf_type: Iri,
    ontology: Iri,
    version_iri: Iri,
}

fn preds() -> &'static Preds {
    static P: OnceLock<Preds> = OnceLock::new();
    P.get_or_init(|| Preds {
        imports: Iri::intern(OWL_IMPORTS),
        rdf_type: Iri::intern(syntax::RDF_TYPE),
        ontology: Iri::intern(OWL_ONTOLOGY),
        version_iri: Iri::intern(OWL_VERSION_IRI),
    })
}

fn doc_onto(a: &Analysis) -> DocOnto {
    let p = preds();
    let mut d = DocOnto::default();
    for t in &a.parsed.triples {
        match (&t.s, &t.o) {
            (_, Node::Iri(o)) if t.p == p.imports => d.imports.push(*o),
            (Node::Iri(s), Node::Iri(o)) if t.p == p.rdf_type && *o == p.ontology => {
                d.ontologies.push(*s)
            }
            (_, Node::Iri(o)) if t.p == p.version_iri => d.ontologies.push(*o),
            _ => {}
        }
    }
    d
}

/// `owl:imports` state of the server: what was asked, what came back.
pub struct Imports {
    jobs: Option<Sender<Job>>,
    results_tx: Sender<Done>,
    results: Receiver<Done>,
    generation: u64,
    last: Option<Request>,
    /// Knowledge generation and network setting the last request was built from.
    seen: Option<(u64, bool)>,
    status: FxHashMap<String, Status>,
    /// External source keys registered with the knowledge index.
    registered: FxHashSet<String>,
    /// Workspace URI -> its analysis when [`DocOnto`] was computed.
    per_doc: FxHashMap<String, (Weak<Analysis>, DocOnto)>,
    cache: Option<PathBuf>,
}

impl Default for Imports {
    fn default() -> Self {
        Imports::new()
    }
}

impl Imports {
    pub fn new() -> Imports {
        let (results_tx, results) = unbounded();
        Imports {
            jobs: None,
            results_tx,
            results,
            generation: 0,
            last: None,
            seen: None,
            status: FxHashMap::default(),
            registered: FxHashSet::default(),
            per_doc: FxHashMap::default(),
            cache: cache_dir().map(|d| d.join("imports")),
        }
    }

    /// Answers from the background thread, for the main loop to select on.
    pub fn results(&self) -> &Receiver<Done> {
        &self.results
    }

    pub fn status(&self, iri: &str) -> Option<&Status> {
        self.status.get(iri)
    }

    /// Look at the workspace's imports and start resolving them if they (or
    /// the setting) changed. Cheap when nothing did. With no imports at all
    /// the answer is immediate and returned.
    pub fn sync(&mut self, k: &Knowledge, roots: &[PathBuf], network: bool) -> Option<Done> {
        let seen = Some((k.generation(), network));
        if self.seen == seen {
            return None;
        }
        self.seen = seen;
        let mut ids: Vec<_> = [OWL_IMPORTS, OWL_ONTOLOGY, OWL_VERSION_IRI]
            .iter()
            .flat_map(|p| k.docs_referencing(*p))
            .filter(|(_, src, _)| *src != Source::External)
            .map(|(id, _, _)| id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let mut per_doc = FxHashMap::default();
        let (mut imports, mut local) = (FxHashSet::default(), FxHashSet::default());
        for id in ids {
            let uri = &k.doc(id).uri;
            let Some(a) = k.get(uri) else { continue };
            let entry = match self.per_doc.remove(uri) {
                Some((w, d)) if std::ptr::eq(w.as_ptr(), Arc::as_ptr(a)) => (w, d),
                _ => (Arc::downgrade(a), doc_onto(a)),
            };
            imports.extend(entry.1.imports.iter().map(|i| i.to_string()));
            local.extend(entry.1.ontologies.iter().map(|i| norm(i).to_string()));
            per_doc.insert(uri.clone(), entry);
        }
        self.per_doc = per_doc;
        let sorted = |s: FxHashSet<String>| {
            let mut v: Vec<String> = s.into_iter().collect();
            v.sort();
            v
        };
        let req = Request {
            imports: sorted(imports),
            local: sorted(local),
            roots: roots.to_vec(),
            cache: self.cache.clone(),
            network,
        };
        if self.last.as_ref() == Some(&req) {
            return None;
        }
        self.generation += 1;
        let empty = req.imports.is_empty();
        self.last = Some(req.clone());
        if empty {
            return Some(Done {
                generation: self.generation,
                status: vec![],
                loaded: vec![],
            });
        }
        let job = Job {
            generation: self.generation,
            req,
        };
        let jobs = self.jobs.get_or_insert_with(|| {
            let (tx, rx) = unbounded();
            let out = self.results_tx.clone();
            let _ = std::thread::Builder::new()
                .name("turtle-lsp-imports".into())
                .spawn(move || worker(rx, out));
            tx
        });
        let _ = jobs.send(job);
        None
    }

    /// Something on disk may have changed (a file was saved): resolve the
    /// unresolved imports again on the next [`Imports::sync`].
    pub fn retry(&mut self) {
        if self
            .status
            .values()
            .any(|s| matches!(s, Status::Unresolved(_)))
        {
            self.last = None;
            self.seen = None;
        }
    }

    /// Take a job's answer: register loaded ontologies with the knowledge
    /// index. Returns whether anything visible changed.
    pub fn apply(&mut self, done: Done, k: &mut Knowledge) -> bool {
        if done.generation != self.generation {
            return false;
        }
        let mut changed = false;
        let keep: FxHashSet<String> = done.loaded.iter().map(|l| l.key.clone()).collect();
        for key in self.registered.difference(&keep) {
            k.remove_external(key);
            changed = true;
        }
        for l in done.loaded {
            if k.has_external(&l.key, &l.analysis) {
                continue;
            }
            match l.facts {
                Some(f) => k.add_external_prepared(&l.key, l.analysis, f),
                None => k.add_external(&l.key, l.analysis),
            }
            changed = true;
        }
        self.registered = keep;
        let status: FxHashMap<String, Status> = done.status.into_iter().collect();
        if status != self.status {
            self.status = status;
            changed = true;
        }
        changed
    }

    /// Information diagnostics on the unresolved imports of `a`.
    pub fn diagnostics(&self, a: &Analysis, enc: Encoding) -> Vec<Diagnostic> {
        diagnostics_with(a, enc, |iri| self.status.get(iri))
    }
}

/// [`Imports::diagnostics`] with the status looked up by `status`.
pub fn diagnostics_with<'s>(
    a: &Analysis,
    enc: Encoding,
    status: impl Fn(&str) -> Option<&'s Status>,
) -> Vec<Diagnostic> {
    if !a.refs_by_iri.contains_key(OWL_IMPORTS) {
        return vec![];
    }
    let p = preds();
    a.parsed
        .triples
        .iter()
        .filter(|t| t.p == p.imports)
        .filter_map(|t| {
            let Node::Iri(o) = &t.o else { return None };
            let Some(Status::Unresolved(message)) = status(o) else {
                return None;
            };
            Some(Diagnostic {
                range: a.range(enc, t.o_span),
                severity: Some(DiagnosticSeverity::INFORMATION),
                code: Some(NumberOrString::String("unresolved-import".into())),
                source: Some(SOURCE.to_string()),
                message: message.clone(),
                ..Default::default()
            })
        })
        .collect()
}

// ---- background thread ----

#[derive(Default)]
struct Memo {
    /// Path -> (modified, length, base IRI, parsed ontology).
    files: FxHashMap<PathBuf, (SystemTime, u64, String, Ontology)>,
    /// URL -> when fetching it failed, and why.
    failed: FxHashMap<String, (Instant, String)>,
}

/// A failed fetch is not tried again for this long.
const RETRY_FETCH_AFTER: Duration = Duration::from_secs(300);
/// With the network on, a job waits this long for a newer one first, so
/// editing an import IRI does not fetch every intermediate spelling.
const NETWORK_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone)]
struct Ontology {
    analysis: Arc<Analysis>,
    imports: Vec<String>,
}

fn worker(jobs: Receiver<Job>, out: Sender<Done>) {
    let mut memo = Memo::default();
    while let Ok(mut job) = jobs.recv() {
        // Only the latest job matters.
        loop {
            while let Ok(j) = jobs.try_recv() {
                job = j;
            }
            if !job.req.network {
                break;
            }
            match jobs.recv_timeout(NETWORK_DEBOUNCE) {
                Ok(j) => job = j,
                Err(_) => break,
            }
        }
        let superseded = || !jobs.is_empty();
        let Some(done) = resolve(&job, &mut memo, &superseded) else {
            continue;
        };
        if out.send(done).is_err() {
            break;
        }
    }
}

/// Resolve a job's imports transitively; `None` when a newer job arrived.
fn resolve(job: &Job, memo: &mut Memo, superseded: &dyn Fn() -> bool) -> Option<Done> {
    let req = &job.req;
    let catalog = Catalog::scan(&req.roots);
    let local: FxHashSet<&str> = req.local.iter().map(String::as_str).collect();
    let mut queue: VecDeque<(String, u32)> = req.imports.iter().map(|i| (i.clone(), 0)).collect();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut status = Vec::new();
    let mut loaded = Vec::new();
    while let Some((iri, depth)) = queue.pop_front() {
        if !seen.insert(norm(&iri).to_string()) {
            continue;
        }
        if superseded() {
            return None;
        }
        let st = if local.contains(norm(&iri)) {
            Status::Local
        } else {
            match load(&iri, req, &catalog, memo) {
                Ok((location, onto, fresh)) => {
                    if depth < MAX_DEPTH {
                        queue.extend(onto.imports.iter().map(|i| (i.clone(), depth + 1)));
                    }
                    loaded.push(Loaded {
                        key: format!("{KEY_PREFIX}{iri}"),
                        facts: fresh.then(|| DocFacts::new(&onto.analysis)),
                        analysis: onto.analysis,
                    });
                    Status::Loaded(location)
                }
                Err(msg) => Status::Unresolved(msg),
            }
        };
        status.push((iri, st));
    }
    Some(Done {
        generation: job.generation,
        status,
        loaded,
    })
}

/// Load one import: (where from, the ontology, freshly parsed).
fn load(
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
        .and_then(|u| u.to_file_path().ok())
    {
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
    if !req.network || !web {
        let how = if !web {
            "Map it to a local file in a catalog-v001.xml"
        } else {
            "Map it to a local file in a catalog-v001.xml, or enable turtle.imports.network to fetch it once"
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
    Arc::get_mut(&mut parsed.analysis)
        .expect("fresh analysis")
        .uri = url.clone();
    if let Some(path) = &cached {
        // A failed write only costs a fetch next time.
        let _ = write_atomic(path, text.as_bytes());
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

/// Load an ontology file; its analysis is named `uri` (default: the file's URI).
fn load_file(
    path: &Path,
    uri: Option<&str>,
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
    // Hover names the file (or, for a cached copy, the URL) a term comes from.
    if let Some(uri) = uri.map(str::to_string).or_else(|| path_to_uri(path)) {
        let a = Arc::get_mut(&mut o.analysis).expect("fresh analysis");
        a.uri = uri;
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

fn parse(bytes: &[u8], format: Format, base: &str) -> Result<Ontology, String> {
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

/// RDF/XML as N-Triples text. Stops at the first error; fails only when that
/// leaves no triple at all.
pub fn rdf_xml_to_ntriples(bytes: &[u8], base: &str) -> Result<String, String> {
    check_xml_tags(bytes)?;
    let parser = oxrdfxml::RdfXmlParser::new();
    let parser = parser.clone().with_base_iri(base).unwrap_or(parser);
    let mut out = String::new();
    let mut count = 0usize;
    for t in parser.for_slice(bytes) {
        match t {
            Ok(t) => {
                use std::fmt::Write;
                let _ = writeln!(out, "{t} .");
                count += 1;
            }
            Err(e) if count == 0 => return Err(e.to_string()),
            Err(_) => break,
        }
    }
    Ok(out)
}

/// Refuse XML with a start tag longer than [`MAX_XML_TAG_BYTES`].
fn check_xml_tags(bytes: &[u8]) -> Result<(), String> {
    let mut i = 0;
    while let Some(off) = bytes[i..].iter().position(|&b| b == b'<') {
        let start = i + off;
        let rest = &bytes[start..];
        let skip_to = |end: &[u8]| {
            rest.windows(end.len())
                .position(|w| w == end)
                .map_or(bytes.len(), |p| start + p + end.len())
        };
        if rest.starts_with(b"<!--") {
            i = skip_to(b"-->");
            continue;
        }
        if rest.starts_with(b"<![CDATA[") {
            i = skip_to(b"]]>");
            continue;
        }
        let mut quote = None;
        let mut end = bytes.len();
        for (j, &b) in rest.iter().enumerate().skip(1) {
            match (quote, b) {
                (None, b'"' | b'\'') => quote = Some(b),
                (Some(q), _) if q == b => quote = None,
                (None, b'>') => {
                    end = start + j + 1;
                    break;
                }
                _ => {}
            }
            if j > MAX_XML_TAG_BYTES {
                return Err(format!(
                    "an XML tag is longer than {} KiB",
                    MAX_XML_TAG_BYTES / 1024
                ));
            }
        }
        i = end;
    }
    Ok(())
}

/// GET `url`: (body, content type, final URL after redirects).
fn fetch(url: &str) -> Result<(Vec<u8>, Option<String>, String), String> {
    use ureq::ResponseExt;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .max_redirects(10)
        .user_agent(concat!("turtle-lsp/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("Accept", ACCEPT)
        .call()
        .map_err(|e| e.to_string())?;
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let final_url = resp.get_uri().to_string();
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_FETCH_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    Ok((body, content_type, final_url))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

// ---- cache ----

/// The per-user cache directory of turtle-lsp: `$TURTLE_LSP_CACHE_DIR`, else
/// `~/Library/Caches/turtle-lsp` (macOS), `%LOCALAPPDATA%\turtle-lsp`
/// (Windows), `$XDG_CACHE_HOME/turtle-lsp` or `~/.cache/turtle-lsp`.
pub fn cache_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if let Some(d) = var("TURTLE_LSP_CACHE_DIR") {
        return Some(d);
    }
    let home = || var(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library").join("Caches")
    } else if cfg!(windows) {
        var("LOCALAPPDATA")?
    } else {
        var("XDG_CACHE_HOME").or_else(|| home().map(|h| h.join(".cache")))?
    };
    Some(base.join("turtle-lsp"))
}

/// The cache file for `url`: Turtle (RDF/XML is cached as N-Triples).
fn cache_file(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{}.ttl", hash_name(url)))
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

fn without_fragment(iri: &str) -> &str {
    iri.split_once('#').map_or(iri, |(b, _)| b)
}

/// The IRI used to match imports, ontology IRIs and catalog entries: an
/// ontology is often named `.../onto#` and imported as `.../onto`.
fn norm(iri: &str) -> &str {
    iri.trim_end_matches(['#', '/'])
}

// ---- catalogs ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    Url(String),
}

/// `<uri name=... uri=...>` entries of every catalog-v001.xml under the roots;
/// the first catalog (by path) that maps an IRI wins.
#[derive(Default)]
pub struct Catalog {
    map: FxHashMap<String, (Target, PathBuf)>,
}

impl Catalog {
    pub fn scan(roots: &[PathBuf]) -> Catalog {
        let mut files = Vec::new();
        let mut stack: Vec<PathBuf> = roots.to_vec();
        let mut seen = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                seen += 1;
                if seen > MAX_FILES * 4 {
                    stack.clear();
                    break;
                }
                let name = e.file_name();
                let name = name.to_string_lossy();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                        stack.push(e.path());
                    }
                } else if ft.is_file() && name == CATALOG_FILE {
                    files.push(e.path());
                }
            }
        }
        files.sort();
        let mut c = Catalog::default();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            let dir = f.parent().unwrap_or(Path::new(""));
            for (name, target) in parse_catalog(&text, dir) {
                c.map
                    .entry(norm(&name).to_string())
                    .or_insert((target, f.clone()));
            }
        }
        c
    }

    fn lookup(&self, iri: &str) -> Option<(&Target, &PathBuf)> {
        self.map.get(norm(iri)).map(|(t, f)| (t, f))
    }
}

/// The `<uri name="..." uri="..."/>` entries of an XML catalog, with targets
/// resolved against `dir`. Comments are skipped; anything else is ignored.
pub fn parse_catalog(text: &str, dir: &Path) -> Vec<(String, Target)> {
    let mut out = Vec::new();
    let base = url::Url::from_directory_path(dir).ok();
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix("!--") {
            rest = r.find("-->").map_or("", |j| &r[j + 3..]);
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        rest = &rest[end..];
        let Some(attrs) = tag.strip_prefix("uri") else {
            continue;
        };
        if !attrs.starts_with(char::is_whitespace) {
            continue;
        }
        let attrs = attributes(attrs);
        let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let (Some(name), Some(uri)) = (get("name"), get("uri")) else {
            continue;
        };
        let target = match url::Url::parse(&uri) {
            Ok(u) if u.scheme() == "file" => u.to_file_path().ok().map(Target::File),
            // A one-letter scheme is a Windows drive.
            Ok(u) if u.scheme().len() > 1 => Some(Target::Url(uri)),
            _ => base
                .as_ref()
                .and_then(|b| b.join(&uri).ok())
                .and_then(|u| u.to_file_path().ok())
                .or_else(|| Some(dir.join(&uri)))
                .map(Target::File),
        };
        if let Some(t) = target {
            out.push((name, t));
        }
    }
    out
}

/// Length of a tag's content up to its closing `>` (quotes respected).
fn tag_end(s: &str) -> usize {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if q == c => quote = None,
            (None, '>') => return i,
            _ => {}
        }
    }
    s.len()
}

/// `name="value"` pairs of a tag, values unescaped.
fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let Some(close) = after[1..].find(q) else {
            break;
        };
        out.push((name, unescape(&after[1..1 + close])));
        rest = &after[close + 2..];
    }
    out
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("turtle-imports-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn catalog_entries_resolve_relative_to_the_catalog() {
        let dir = Path::new(if cfg!(windows) { "C:\\p\\q" } else { "/p/q" });
        let xml = r#"<?xml version="1.0"?>
<catalog prefer="public" xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <!-- <uri name="http://ex.com/hidden" uri="no.ttl"/> -->
  <uri id="Imports Wizard Entry" name="http://ex.com/a" uri="vendor/a%20b.ttl"/>
  <uri name='http://ex.com/b?x=1&amp;y=2' uri="../b.owl" />
  <uri name="http://ex.com/c" uri="https://mirror.example.com/c.ttl"/>
  <uriSuffix uriSuffix="x" uri="y"/>
</catalog>"#;
        let got = parse_catalog(xml, dir);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].0, "http://ex.com/a");
        assert_eq!(got[0].1, Target::File(dir.join("vendor").join("a b.ttl")));
        assert_eq!(got[1].0, "http://ex.com/b?x=1&y=2");
        assert_eq!(got[1].1, Target::File(dir.parent().unwrap().join("b.owl")));
        assert_eq!(
            got[2].1,
            Target::Url("https://mirror.example.com/c.ttl".into())
        );
        // Garbage never panics.
        for s in [
            "<uri",
            "<uri name=\"a",
            "<uri name='a' uri=",
            "&#xZZ;<uri name=\"&#x41;\" uri=\"x\">",
        ] {
            let _ = parse_catalog(s, dir);
        }
        assert_eq!(unescape("a&lt;b&#65;&#x42;&bogus;"), "a<bAB&bogus;");
    }

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
    fn rdf_xml_becomes_ntriples() {
        let xml = br##"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:owl="http://www.w3.org/2002/07/owl#">
  <owl:Class rdf:about="#Gear">
    <rdfs:label xml:lang="en">Gear "wheel"</rdfs:label>
  </owl:Class>
</rdf:RDF>"##;
        let nt = rdf_xml_to_ntriples(xml, "http://ex.com/rx").unwrap();
        let o = parse(nt.as_bytes(), Format::Turtle, "http://ex.com/rx").unwrap();
        assert!(o.analysis.parsed.errors.is_empty(), "{nt}");
        assert_eq!(
            o.analysis.label("http://ex.com/rx#Gear"),
            Some("Gear \"wheel\"")
        );
        assert!(rdf_xml_to_ntriples(b"<rdf:RDF", "http://ex.com/").is_err());
        let mut long = b"<?xml version='1.0'?><rdf:RDF ".to_vec();
        long.extend(std::iter::repeat_n(b' ', MAX_XML_TAG_BYTES + 10));
        long.extend(b"/>");
        assert!(
            rdf_xml_to_ntriples(&long, "http://ex.com/")
                .unwrap_err()
                .contains("KiB")
        );
    }

    #[test]
    fn hash_names_are_stable() {
        assert_eq!(hash_name(""), "cbf29ce484222325");
        assert_ne!(hash_name("http://a/"), hash_name("http://b/"));
        assert_eq!(hash_name("x").len(), 16);
    }

    fn job(imports: &[&str], local: &[&str], roots: Vec<PathBuf>, cache: Option<PathBuf>) -> Job {
        Job {
            generation: 1,
            req: Request {
                imports: imports.iter().map(|s| s.to_string()).collect(),
                local: local.iter().map(|s| s.to_string()).collect(),
                roots,
                cache,
                network: false,
            },
        }
    }

    #[test]
    fn imports_resolve_transitively_through_catalog_and_cache_with_cycles() {
        let root = tmp("transitive");
        let ws = root.join("ws");
        fs::create_dir_all(ws.join("sub")).unwrap();
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            ws.join("sub").join(CATALOG_FILE),
            r#"<catalog><uri name="http://ex.com/a" uri="../../a.ttl"/></catalog>"#,
        )
        .unwrap();
        // a imports b (cache) and itself; b imports a (cycle) and c (missing) and the local one.
        fs::write(
            root.join("a.ttl"),
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<> owl:imports <http://ex.com/b>, <http://ex.com/a> .\n<#T> a owl:Class .\n",
        )
        .unwrap();
        fs::write(
            cache.join(format!("{}.ttl", hash_name("http://ex.com/b"))),
            "<http://ex.com/b> <http://www.w3.org/2002/07/owl#imports> <http://ex.com/a#>, <http://ex.com/c>, <http://ex.com/local> .\n",
        )
        .unwrap();
        let j = job(
            &["http://ex.com/a"],
            &["http://ex.com/local"],
            vec![ws.clone()],
            Some(cache.clone()),
        );
        let mut memo = Memo::default();
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        let st: FxHashMap<_, _> = done.status.iter().cloned().collect();
        assert!(matches!(st["http://ex.com/a"], Status::Loaded(_)), "{st:?}");
        assert!(matches!(st["http://ex.com/b"], Status::Loaded(_)), "{st:?}");
        assert_eq!(st["http://ex.com/local"], Status::Local);
        assert!(
            matches!(&st["http://ex.com/c"], Status::Unresolved(m) if m.contains("turtle.imports.network")),
            "{st:?}"
        );
        // The cycle back to a#: visited once.
        assert_eq!(done.status.len(), 4, "{st:?}");
        assert_eq!(done.loaded.len(), 2);
        // Relative IRIs resolve against the import IRI.
        let a = &done
            .loaded
            .iter()
            .find(|l| l.key.ends_with("/a"))
            .unwrap()
            .analysis;
        assert!(a.defs.contains_key("http://ex.com/a#T"));
        assert!(a.uri.starts_with("file:"), "{}", a.uri);
        assert!(done.loaded.iter().all(|l| l.facts.is_some()));
        // Unchanged files are not parsed again.
        let again = resolve(&j, &mut memo, &|| false).unwrap();
        assert!(again.loaded.iter().all(|l| l.facts.is_none()));
        assert!(Arc::ptr_eq(
            &again.loaded[0].analysis,
            &done.loaded[0].analysis
        ));
        // A newer job supersedes this one.
        assert!(resolve(&j, &mut memo, &|| true).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn depth_is_limited() {
        let root = tmp("depth");
        let mut catalog = String::from("<catalog>");
        for i in 0..10 {
            fs::write(
                root.join(format!("o{i}.ttl")),
                format!(
                    "<http://ex.com/o{i}> <{OWL_IMPORTS}> <http://ex.com/o{}> .\n",
                    i + 1
                ),
            )
            .unwrap();
            catalog.push_str(&format!(
                "<uri name=\"http://ex.com/o{i}\" uri=\"o{i}.ttl\"/>"
            ));
        }
        catalog.push_str("</catalog>");
        fs::write(root.join(CATALOG_FILE), catalog).unwrap();
        let j = job(&["http://ex.com/o0"], &[], vec![root.clone()], None);
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        // o0 (direct) plus 5 levels below it.
        assert_eq!(done.loaded.len(), 1 + MAX_DEPTH as usize);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unresolved_imports_get_an_information_diagnostic() {
        let text = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://ex.com/me> owl:imports <http://ex.com/x>, <http://ex.com/y> .\n";
        let a = Analysis::new(
            "file:///d.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            false,
        );
        let missing = Status::Unresolved("nope".into());
        let ok = Status::Local;
        let d = diagnostics_with(&a, Encoding::Utf16, |i| {
            Some(if i.ends_with('x') { &missing } else { &ok })
        });
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::INFORMATION));
        assert_eq!(d[0].range.start.character, 31);
        assert_eq!(d[0].message, "nope");
    }

    #[test]
    fn workspace_ontologies_and_imports_are_found() {
        let mut k = Knowledge::default();
        let none = Arc::new(PrefixMap::default());
        let text = format!(
            "<http://ex.com/me> a <{OWL_ONTOLOGY}> ; <{OWL_VERSION_IRI}> <http://ex.com/me/1.0> ; <{OWL_IMPORTS}> <http://ex.com/me#>, <http://ex.com/other> .\n"
        );
        k.set(
            "file:///a.ttl",
            Source::Disk,
            Arc::new(Analysis::new("file:///a.ttl".into(), &text, none, false)),
        );
        let mut imports = Imports::new();
        imports.cache = None;
        assert!(imports.sync(&k, &[], false).is_none());
        let req = imports.last.clone().unwrap();
        assert_eq!(req.imports, ["http://ex.com/me#", "http://ex.com/other"]);
        assert_eq!(req.local, ["http://ex.com/me", "http://ex.com/me/1.0"]);
        // Nothing changed: no new job.
        let g = imports.generation;
        imports.seen = None;
        assert!(imports.sync(&k, &[], false).is_none());
        assert_eq!(imports.generation, g);
        let done = imports
            .results()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(imports.apply(done, &mut k));
        assert_eq!(imports.status("http://ex.com/me#"), Some(&Status::Local));
        assert!(matches!(
            imports.status("http://ex.com/other"),
            Some(Status::Unresolved(_))
        ));
        // Without imports the answer is immediate.
        k.remove("file:///a.ttl");
        let done = imports.sync(&k, &[], false).unwrap();
        assert!(imports.apply(done, &mut k));
        assert!(imports.status("http://ex.com/other").is_none());
    }
}
