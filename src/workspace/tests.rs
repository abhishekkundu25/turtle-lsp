#[test]
fn file_uri_name_is_the_decoded_last_segment() {
    assert_eq!(
        file_uri_name("file:///work/shapes/my%20file.ttl").as_deref(),
        Some("my file.ttl")
    );
    assert_eq!(
        file_uri_name("file:///work/a.ttl").as_deref(),
        Some("a.ttl")
    );
    assert_eq!(file_uri_name("https://e.org/a.ttl"), None);
    assert_eq!(file_uri_name("file:///work/"), None);
}

use super::*;
use crate::analysis::ParsePath;
use crate::jsonld::{LoadError, RemoteDocument};
use crate::syntax::Span;
use std::path::Path;
use std::sync::Mutex;

#[cfg(not(debug_assertions))]
include!("../../tests/support/corpus.rs");

/// An `.owl` file read as its content says: N-Triples whose IRIs are
/// also XML names, XML whose names hold `·`, and an anonymous
/// `owl:Ontology` root in RDF/XML.
#[test]
fn an_owl_file_is_read_as_what_it_holds() {
    use crate::syntax::Node;
    let rdf_type = crate::syntax::RDF_TYPE;
    let doc = |uri: &str, text: &str| {
        let mut ws = Workspace::default();
        ws.open_doc(uri.into(), text.into(), 1);
        ws.analysis(uri).unwrap()
    };
    let a = doc("file:///w/a.owl", "<urn:s> <urn:p> <urn:o> .\n");
    assert_eq!(a.parsed.syntax, Syntax::Turtle);
    assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
    assert_eq!(a.parsed.triples.len(), 1);
    let a = doc("file:///w/a.owl", "<e:C·D xmlns:e=\"urn:ex:\"/>\n");
    assert_eq!(a.parsed.syntax, Syntax::RdfXml);
    assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
    assert_eq!(a.parsed.triples.len(), 1);
    assert_eq!(&*a.parsed.triples[0].p, rdf_type);
    assert!(matches!(&a.parsed.triples[0].o, Node::Iri(i) if &**i == "urn:ex:C·D"));
    let a = doc(
        "file:///w/a.owl",
        "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"><rdfs:label>My ontology</rdfs:label></owl:Ontology>\n",
    );
    assert_eq!(a.parsed.syntax, Syntax::RdfXml);
    assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
    let t = &a.parsed.triples;
    assert_eq!(t.len(), 2, "{t:?}");
    assert!(
        t.iter()
            .all(|x| matches!(x.s, Node::Blank(_)) && x.s == t[0].s)
    );
    assert!(t.iter().any(|t| &*t.p == rdf_type
        && matches!(&t.o, Node::Iri(i) if &**i == "http://www.w3.org/2002/07/owl#Ontology")));
    assert!(
        t.iter()
            .any(|t| &*t.p == "http://www.w3.org/2000/01/rdf-schema#label"
                && matches!(&t.o, Node::Literal { value, .. } if &**value == "My ontology"))
    );
    // A processing instruction and a comment before the root, with no
    // space inside them: XML, not two Turtle IRIs.
    let a = doc(
        "file:///w/a.owl",
        "<?build?>\n<!--ok-->\n<e:C xmlns:e=\"urn:ex:\"/>\n",
    );
    assert_eq!(a.parsed.syntax, Syntax::RdfXml);
    assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
    let t = &a.parsed.triples;
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(&*t[0].p, rdf_type);
    assert!(matches!(&t[0].o, Node::Iri(i) if &**i == "urn:ex:C"));
    // An `owl:Ontology` whose property is named as an OWL/XML element
    // but has `rdf:resource`: RDF/XML.
    let a = doc(
        "file:///w/a.owl",
        "<owl:Ontology xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:owl=\"http://www.w3.org/2002/07/owl#\" ><owl:Annotation rdf:resource=\"urn:x\"/></owl:Ontology>\n",
    );
    assert_eq!(a.parsed.syntax, Syntax::RdfXml);
    assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
    let t = &a.parsed.triples;
    assert_eq!(t.len(), 2, "{t:?}");
    assert!(
        t.iter()
            .any(|t| &*t.p == "http://www.w3.org/2002/07/owl#Annotation"
                && matches!(&t.o, Node::Iri(i) if &**i == "urn:x"))
    );
    // OWL/XML's `Ontology` with OWL/XML elements: read as OWL/XML, its
    // error shown and its terms described in the workspace.
    let mut ws = Workspace::default();
    let uri = "file:///w/o.owl";
    let text = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n  <Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n  <SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\"nope:B\"/></SubClassOf>\n</Ontology>\n";
    ws.open_doc(uri.into(), text.into(), 1);
    let a = ws.analysis(uri).unwrap();
    assert_eq!(a.parsed.syntax, Syntax::OwlXml);
    let codes: Vec<_> = a.parsed.errors.iter().map(|e| e.code).collect();
    assert_eq!(codes, ["owx-syntax"]);
    assert!(
        a.parsed.triples.iter().any(|t| &*t.p == rdf_type
            && matches!(&t.s, Node::Iri(i) if &**i == "http://example.org/o#A"))
    );
    assert!(ws.knowledge().is_described("http://example.org/o#A"));
    // N-Triples with no space between the terms.
    let a = doc("file:///w/a.owl", "<urn:s><urn:p><urn:o>.\n");
    assert_eq!(a.parsed.syntax, Syntax::Turtle);
    assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
    assert_eq!(a.parsed.triples.len(), 1);
}

#[test]
fn normalize_uri_is_stable() {
    let dir = std::env::temp_dir().join("turtle lsp");
    let uri = path_to_uri(&dir.join("a.ttl")).unwrap();
    assert_eq!(normalize_uri(&uri), uri);
    assert_eq!(normalize_uri(&normalize_uri(&uri)), uri);
    assert_eq!(normalize_uri("untitled:Untitled-1"), "untitled:Untitled-1");
}

/// The `url` crate leaves the `[`, `]`, `^` and `|` of a path as they
/// are, and the protocol's `Uri` refuses them: a file with one in its
/// name had a URI no location could carry.
#[test]
fn path_to_uri_is_a_uri_the_protocol_parses() {
    let dir = std::env::temp_dir();
    let ascii = (b' '..=b'~')
        .map(char::from)
        .filter(|c| !['/', '\\', ':'].contains(c))
        .map(|c| format!("a{c}b.ttl"));
    for name in ascii.chain(["données.ttl".to_string(), "数据.ttl".to_string()]) {
        let uri = path_to_uri(&dir.join(&name)).unwrap();
        assert!(uri.parse::<lsp_types::Uri>().is_ok(), "{name}: {uri}");
        assert_eq!(normalize_uri(&uri), uri, "{name}");
        let back = uri_to_path(&uri).unwrap();
        assert_eq!(back.file_name().unwrap().to_str(), Some(&*name), "{uri}");
    }
    // As VS Code writes them, so an open file and its copy on disk have one URI.
    let uri = path_to_uri(&dir.join("a [b]^c|d.ttl")).unwrap();
    assert!(uri.ends_with("/a%20%5Bb%5D%5Ec%7Cd.ttl"), "{uri}");
}

/// `a[1].ttl` and a file named `a%5B1%5D.ttl` are two files: the
/// second's `%` is escaped too, so their URIs differ and each comes back
/// to its own path. A URI that is one already is left as it is.
#[test]
fn a_bracket_and_its_escape_written_out_are_two_files() {
    let dir = std::env::temp_dir();
    let bracket = path_to_uri(&dir.join("a[1].ttl")).unwrap();
    let written = path_to_uri(&dir.join("a%5B1%5D.ttl")).unwrap();
    assert!(bracket.ends_with("/a%5B1%5D.ttl"), "{bracket}");
    assert!(written.ends_with("/a%255B1%255D.ttl"), "{written}");
    for (uri, name) in [(&bracket, "a[1].ttl"), (&written, "a%5B1%5D.ttl")] {
        assert_eq!(normalize_uri(uri), *uri);
        let back = uri_to_path(uri).unwrap();
        assert_eq!(back.file_name().unwrap().to_str(), Some(name), "{uri}");
    }
    for uri in [
        "vscode-remote://ssh-remote%2Bhost/home/u/a%5B1%5D.ttl",
        "file:///c%3A/w/a%5B1%5D.ttl",
        "untitled:Untitled-1",
    ] {
        assert!(uri.parse::<lsp_types::Uri>().is_ok(), "{uri}");
        assert_eq!(as_uri(uri), uri, "{uri}");
    }
}

#[test]
fn as_uri_escapes_what_a_uri_may_not_hold() {
    for (iri, uri) in [
        ("http://example.org/a#b", "http://example.org/a#b"),
        (
            "http://example.org/données/été",
            "http://example.org/donn%C3%A9es/%C3%A9t%C3%A9",
        ),
        (
            "http://example.org/a b|c^d`e{f}\"<g>\\h",
            "http://example.org/a%20b%7Cc%5Ed%60e%7Bf%7D%22%3Cg%3E%5Ch",
        ),
        // Brackets belong to the host only.
        (
            "http://[::1]:8080/a[1]?q=[x]#f[y]",
            "http://[::1]:8080/a%5B1%5D?q=%5Bx%5D#f%5By%5D",
        ),
        // An escape stays; a `%` that starts none is escaped.
        (
            "http://example.org/a%2Fb/100%/%zz",
            "http://example.org/a%2Fb/100%25/%25zz",
        ),
        ("urn:example:a[1]", "urn:example:a%5B1%5D"),
    ] {
        assert_eq!(as_uri(iri), uri);
        assert!(uri.parse::<lsp_types::Uri>().is_ok(), "{uri}");
        assert_eq!(as_uri(uri), uri, "an escaped one stays as it is");
    }
}

#[test]
fn cached_imports_are_analyzed_but_not_registered() {
    let dir = std::env::temp_dir().join(format!("turtle-lsp-cache-{}", std::process::id()));
    let cache = dir.join("cache").join("imports");
    std::fs::create_dir_all(&cache).unwrap();
    let mut ws = Workspace {
        import_cache: Some(canonical(&cache)),
        ..Default::default()
    };
    let text = "@prefix ex: <http://example.org/> .\nex:A a ex:B .\n";
    let cached = path_to_uri(&cache.join("a.ttl")).unwrap();
    let own = path_to_uri(&dir.join("b.ttl")).unwrap();
    let import = Arc::new(Analysis::new(cached.clone(), text, Arc::default(), false));
    ws.knowledge_mut()
        .add_external("http://example.org/", import);

    ws.open_doc(cached.clone(), text.into(), 1);
    ws.open_doc(own.clone(), text.into(), 1);
    assert!(ws.is_cached_import(&cached));
    assert!(!ws.is_cached_import(&own));
    assert!(ws.analysis(&cached).is_some());
    assert!(ws.knowledge().get(&cached).is_none());
    assert!(ws.knowledge().get(&own).is_some());
    assert_eq!(ws.knowledge().docs().count(), 2);

    ws.close_doc(&cached);
    assert!(!ws.disk.contains_key(&cached));
    assert_eq!(ws.knowledge().docs().count(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn jsonld_documents_are_rdf_and_parse_without_turtle_errors() {
    assert_eq!(
        DocKind::of(Some("json"), "file:///w/a.jsonld", ""),
        DocKind::Rdf(Syntax::JsonLd)
    );
    assert!(is_rdf_file(Path::new("/w/a.jsonld")) && !is_rdf_file(Path::new("/w/a.json")));
    let mut ws = Workspace::default();
    let uri = "file:///w/a.jsonld".to_string();
    ws.open_doc(uri.clone(), r#"{"@id": "ex:a", "ex:p": 1}"#.into(), 1);
    let a = ws.analysis(&uri).unwrap();
    assert_eq!(a.parsed.syntax, Syntax::JsonLd);
    assert_eq!(a.parsed.document_base.as_deref(), Some(uri.as_str()));
    assert!(a.parsed.errors.is_empty() && a.external.is_empty());
}

#[test]
fn a_new_context_loader_analyzes_open_jsonld_documents_again() {
    let mut ws = Workspace::default();
    let (ld, ttl) = ("file:///w/a.jsonld", "file:///w/a.ttl");
    ws.open_doc(ld.into(), "{}".into(), 1);
    ws.open_doc(ttl.into(), "".into(), 1);
    let (a, b) = (ws.analysis(ld).unwrap(), ws.analysis(ttl).unwrap());
    ws.set_context_loader(Arc::new(crate::jsonld::NoNetwork::default()));
    assert!(!Arc::ptr_eq(&a, &ws.analysis(ld).unwrap()));
    assert!(Arc::ptr_eq(&b, &ws.analysis(ttl).unwrap()));
}

/// A fresh directory (canonical), removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let d = std::env::temp_dir().join(format!("turtle-lsp-ws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDir(canonical(&d))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn context_errors(ws: &mut Workspace, uri: &str) -> Vec<String> {
    ws.analysis(uri)
        .unwrap()
        .parsed
        .errors
        .iter()
        .filter(|e| e.code == "jsonld-context")
        .map(|e| e.message().to_string())
        .collect()
}

const USES_CTX: &str = r#"{"@context": "ctx.jsonld", "@id": "http://ex.org/a", "name": "A"}"#;
const CTX: &str = r#"{"@context": {"name": "http://ex.org/name"}}"#;

#[test]
fn a_file_context_comes_from_the_open_document_then_the_disk_inside_the_roots_only() {
    let root = TempDir::new("ctx-root");
    let outside = TempDir::new("ctx-outside");
    for dir in [&root.0, &outside.0] {
        std::fs::write(dir.join("ctx.jsonld"), CTX).unwrap();
    }
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let inside = path_to_uri(&root.0.join("a.jsonld")).unwrap();
    let away = path_to_uri(&outside.0.join("a.jsonld")).unwrap();
    ws.open_doc(inside.clone(), USES_CTX.into(), 1);
    ws.open_doc(away.clone(), USES_CTX.into(), 1);
    assert_eq!(context_errors(&mut ws, &inside), Vec::<String>::new());
    let refused = context_errors(&mut ws, &away);
    assert!(
        refused.len() == 1 && refused[0].contains("outside the workspace"),
        "{refused:?}"
    );
    assert_eq!(ws.analysis(&inside).unwrap().parsed.triples.len(), 1);

    // An open context file wins over the disk, and edits to it reach the
    // documents that use it.
    let ctx = path_to_uri(&root.0.join("ctx.jsonld")).unwrap();
    ws.open_doc(ctx.clone(), r#"{"@context": {"name": null}}"#.into(), 1);
    assert!(ws.context_changed(&ctx, false));
    assert_eq!(ws.analysis(&inside).unwrap().parsed.triples.len(), 0);
    // Outside the roots, an open document is served too (the browser
    // has nothing else).
    let away_ctx = path_to_uri(&outside.0.join("ctx.jsonld")).unwrap();
    ws.open_doc(away_ctx.clone(), CTX.into(), 1);
    assert!(ws.context_changed(&away_ctx, false));
    assert_eq!(context_errors(&mut ws, &away), Vec::<String>::new());
    assert!(!ws.context_changed("file:///nobody/uses.jsonld", true));
}

#[test]
fn indexed_json_ld_files_follow_their_context_file_and_the_network_setting() {
    let root = TempDir::new("ctx-disk");
    std::fs::write(root.0.join("ctx.jsonld"), CTX).unwrap();
    std::fs::write(root.0.join("a.jsonld"), USES_CTX).unwrap();
    let web =
        r#"{"@context": "https://example.org/unknown-context.jsonld", "@id": "http://ex.org/b"}"#;
    std::fs::write(root.0.join("b.jsonld"), web).unwrap();
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.set_context_loader(Arc::new(crate::jsonld::NoNetwork::default()));
    ws.index_paths(Workspace::scan_roots(&ws.roots.clone()));
    let a = path_to_uri(&root.0.join("a.jsonld")).unwrap();
    let b = path_to_uri(&root.0.join("b.jsonld")).unwrap();
    let ctx = path_to_uri(&root.0.join("ctx.jsonld")).unwrap();
    assert_eq!(ws.disk[&a].parsed.triples.len(), 1);
    std::fs::write(root.0.join("ctx.jsonld"), r#"{"@context": {"name": null}}"#).unwrap();
    assert!(ws.context_changed(&ctx, true));
    assert_eq!(ws.disk[&a].parsed.triples.len(), 0);
    // A new loader parses again the indexed files that use web contexts.
    let before = ws.disk[&b].clone();
    let a_before = ws.disk[&a].clone();
    ws.set_context_loader(Arc::new(crate::jsonld::NoNetwork::default().with(
        "https://example.org/unknown-context.jsonld",
        r#"{"@context": {}}"#,
    )));
    assert!(!Arc::ptr_eq(&before, &ws.disk[&b]));
    assert!(
        ws.disk[&b].parsed.errors.is_empty(),
        "{:?}",
        ws.disk[&b].parsed.errors
    );
    assert!(Arc::ptr_eq(&a_before, &ws.disk[&a]));
}

#[test]
fn sparql_documents_are_recognised_and_never_feed_the_index() {
    assert_eq!(
        DocKind::of(Some("sparql"), "file:///a.txt", ""),
        DocKind::Sparql
    );
    for id in [None, Some("turtle"), Some("plaintext")] {
        for name in ["a.rq", "a.sparql", "a.ru"] {
            let uri = format!("file:///x/{name}");
            assert_eq!(DocKind::of(id, &uri, ""), DocKind::Sparql, "{id:?} {name}");
        }
    }
    assert_eq!(
        DocKind::of(Some("trig"), "file:///a.rq", ""),
        DocKind::Rdf(Syntax::TriG)
    );
    assert_eq!(
        DocKind::of(None, "file:///a.nq", ""),
        DocKind::Rdf(Syntax::NQuads)
    );

    let mut ws = Workspace::default();
    let ttl = "file:///w/a.ttl".to_string();
    let rq = "file:///w/q.rq".to_string();
    ws.open_doc(
        ttl.clone(),
        "@prefix ex: <http://e.org/> .\nex:A a ex:B .\n".into(),
        1,
    );
    ws.open_doc(
        rq.clone(),
        "PREFIX ex: <http://e.org/>\nINSERT DATA { ex:G a ex:B }\n".into(),
        1,
    );
    assert!(ws.is_sparql(&rq) && !ws.is_sparql(&ttl));
    assert!(ws.analysis(&rq).is_none());
    let q = ws.sparql(&rq).unwrap();
    assert!(q.doc.is_update());
    assert!(
        Arc::ptr_eq(&q, &ws.sparql(&rq).unwrap()),
        "one parse per version"
    );
    assert!(ws.sparql(&ttl).is_none());
    assert_eq!(ws.knowledge().docs().count(), 1);
    assert!(!ws.knowledge().is_described("http://e.org/G"));
    assert_eq!(ws.sparql_documents().len(), 1);
    ws.close_doc(&rq);
    assert_eq!(ws.knowledge().docs().count(), 1);
    assert!(ws.sparql_documents().is_empty());
}

const TTL: &str = "@prefix ex: <http://e.org/> .\nex:A a ex:B .\n";

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn uri(path: &Path) -> String {
    path_to_uri(path).unwrap()
}

#[test]
fn fits_is_what_the_scan_would_index() {
    let root = TempDir::new("indexable");
    let outside = TempDir::new("indexable-outside");
    let ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let ok = write(&root.0, "sub/a.ttl", TTL);
    assert!(ws.fits(&ok));
    // Not RDF, outside the roots, in hidden and skipped directories.
    assert!(!ws.fits(&write(&root.0, "sub/a.txt", TTL)));
    assert!(!ws.fits(&write(&outside.0, "a.ttl", TTL)));
    for dir in [".git", ".hidden/deep", "node_modules", "a/target", "dist/x"] {
        let f = write(&root.0, &format!("{dir}/b.ttl"), TTL);
        assert!(!ws.fits(&f), "{dir}");
    }
    // A hidden file name is indexed, as the scan does.
    assert!(ws.fits(&write(&root.0, ".dotfile.ttl", TTL)));
    // Gone, or not a file.
    assert!(!ws.fits(&root.0.join("missing.ttl")));
    assert!(!ws.fits(&root.0.join("sub")));
    // Too large.
    let big = write(&root.0, "big.ttl", "");
    std::fs::File::options()
        .write(true)
        .open(&big)
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    assert!(!ws.fits(&big));
    // The scan and the check agree.
    let scanned = Workspace::scan_roots(&ws.roots);
    assert!(scanned.contains(&ok) && scanned.iter().all(|p| ws.fits(p)));
    // `..` does not climb out of a root.
    assert!(!ws.fits(&root.0.join("..").join("x.ttl")));
    // Folders: the folder's own name counts.
    assert!(ws.watched_dir(&root.0.join("sub")));
    assert!(!ws.watched_dir(&root.0.join("node_modules")));
    assert!(!ws.watched(&root.0.join("node_modules").join("x.json")));
}

/// A link in the workspace is not followed, as the scan does not follow
/// it: a file the watcher reports through a link to a file or a folder
/// outside the roots is not indexed, and neither is a link to a file
/// inside. Symbolic links need privileges on Windows: unix only, as the
/// other link tests.
#[cfg(unix)]
#[test]
fn a_watched_change_through_a_link_is_not_indexed_as_the_scan_does_not_follow_it() {
    use std::os::unix::fs::symlink;
    let root = TempDir::new("link-watch");
    let outside = TempDir::new("link-watch-outside");
    let target = write(&outside.0, "vocab.owl", "<urn:s> <urn:p> <urn:o> .\n");
    write(&outside.0, "dir/q.rq", "SELECT * WHERE { ?s ?p ?o }\n");
    let inside = write(&root.0, "real.ttl", TTL);
    let vocab = root.0.join("vocab.owl");
    symlink(&target, &vocab).unwrap();
    symlink(outside.0.join("dir"), root.0.join("dir")).unwrap();
    symlink(&inside, root.0.join("again.ttl")).unwrap();
    let via_dir = root.0.join("dir/vocab.owl");
    symlink(&target, outside.0.join("dir/vocab.owl")).unwrap();
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    assert_eq!(
        Workspace::scan_roots(&ws.roots),
        std::slice::from_ref(&inside)
    );
    let watched = [vocab.clone(), via_dir.clone(), root.0.join("again.ttl")];
    for p in &watched {
        assert!(ws.watched(p), "{}", p.display());
        assert!(!ws.fits(p), "{}", p.display());
    }
    for change in [Change::Created, Change::Changed] {
        let changes: Vec<_> = watched.iter().map(|p| (p.clone(), change)).collect();
        ws.files_changed(&changes);
        assert_eq!(ws.indexed_count(), 0, "{change:?}");
    }
    // The real file is indexed; queries through a link are not read.
    ws.files_changed(&[(inside.clone(), Change::Created)]);
    assert!(ws.is_indexed(&uri(&inside)));
    assert_eq!(ws.indexed_count(), 1);
    let _ = ws.sparql_documents();
    ws.sparql_files_changed(&[root.0.join("dir/q.rq")]);
    assert!(ws.sparql_documents().is_empty());
}

#[test]
fn a_capped_scan_keeps_the_first_files_in_path_order_and_says_it_stopped() {
    let root = TempDir::new("scan-cap");
    // Created in an order that is neither alphabetical nor reversed.
    for name in ["z/1.ttl", "b/2.ttl", "a.ttl", "b/1.ttl", "m.ttl", "a/9.ttl"] {
        write(&root.0, name, TTL);
    }
    write(&root.0, "node_modules/x.ttl", TTL);
    write(&root.0, "notes.txt", "");
    let names = |scan: &Scan| -> Vec<String> {
        scan.paths
            .iter()
            .map(|p| {
                p.strip_prefix(&root.0)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    };
    let all = scan_roots_capped(std::slice::from_ref(&root.0), is_rdf_file, 6);
    assert!(!all.truncated, "exactly at the cap is not truncated");
    assert_eq!(
        names(&all),
        ["a/9.ttl", "a.ttl", "b/1.ttl", "b/2.ttl", "m.ttl", "z/1.ttl"]
    );
    let roots = [root.0.clone()];
    let cut = scan_roots_capped(&roots, is_rdf_file, 4);
    assert!(cut.truncated);
    assert_eq!(names(&cut), ["a/9.ttl", "a.ttl", "b/1.ttl", "b/2.ttl"]);
    // The same roots twice, or nested, list a file once.
    let twice = scan_roots_capped(&[root.0.clone(), root.0.join("b")], is_rdf_file, 100);
    assert_eq!(twice.paths.len(), 6);
    // The server's scan is the capped scan at MAX_FILES.
    assert_eq!(scan_roots_matching(&roots, is_rdf_file).paths, all.paths);
}

#[test]
fn a_scan_lists_the_files_it_left_out_for_size() {
    let root = TempDir::new("scan-large");
    let ok = write(&root.0, "ok.ttl", TTL);
    let big = write(&root.0, "sub/big.ttl", "");
    std::fs::File::options()
        .write(true)
        .open(&big)
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    // Not wanted, so not named: only a wanted file can be too large.
    let other = write(&root.0, "big.txt", "");
    std::fs::File::options()
        .write(true)
        .open(&other)
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    let scan = scan_roots_matching(std::slice::from_ref(&root.0), is_rdf_file);
    assert_eq!(scan.paths, [ok]);
    assert_eq!(scan.too_large, [big]);
    assert!(!scan.truncated);
}

#[test]
fn a_file_changed_on_disk_is_indexed_only_where_the_scan_would() {
    let root = TempDir::new("reindex");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let a = write(&root.0, "a.ttl", TTL);
    let skipped = write(&root.0, "node_modules/b.ttl", TTL);
    assert!(ws.reindex_disk_file(&a));
    assert!(!ws.reindex_disk_file(&skipped));
    assert!(ws.disk.contains_key(&uri(&a)) && !ws.disk.contains_key(&uri(&skipped)));
    assert!(ws.knowledge().get(&uri(&a)).is_some());
    // Gone from disk: gone from the index.
    std::fs::remove_file(&a).unwrap();
    assert!(ws.reindex_disk_file(&a));
    assert!(ws.disk.is_empty() && ws.knowledge().get(&uri(&a)).is_none());
}

#[test]
fn removing_a_folder_drops_the_files_below_it_and_only_those() {
    let root = TempDir::new("remove-under");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let files: Vec<PathBuf> = [
        "gone/a.ttl",
        "gone/deep/b.ttl",
        "gone2/c.ttl",
        "kept/d.ttl",
        "gonex.ttl",
    ]
    .iter()
    .map(|f| write(&root.0, f, TTL))
    .collect();
    ws.index_paths(files.clone());
    assert_eq!(ws.disk.len(), 5);
    assert!(ws.remove_under_any(&[root.0.join("gone")]));
    let left: Vec<&str> = [
        "gone/a.ttl",
        "gone/deep/b.ttl",
        "gone2/c.ttl",
        "kept/d.ttl",
        "gonex.ttl",
    ]
    .into_iter()
    .filter(|f| ws.disk.contains_key(&uri(&root.0.join(f))))
    .collect();
    assert_eq!(left, ["gone2/c.ttl", "kept/d.ttl", "gonex.ttl"]);
    for gone in &files[..2] {
        assert!(ws.knowledge().get(&uri(gone)).is_none());
    }
    assert!(ws.knowledge().get(&uri(&files[3])).is_some());
    assert!(
        !ws.remove_under_any(&[root.0.join("gone")]),
        "nothing is left to remove"
    );
    assert!(!ws.remove_under_any(&[]));
    // Several at once, in one pass.
    assert!(ws.remove_under_any(&[root.0.join("gone2"), root.0.join("kept")]));
    assert_eq!(ws.disk.len(), 1);
}

#[test]
fn closing_a_file_outside_the_roots_leaves_no_entry() {
    let root = TempDir::new("close-root");
    let outside = TempDir::new("close-outside");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let inside = write(&root.0, "in.ttl", TTL);
    let away = write(&outside.0, "out.ttl", TTL);
    for f in [&inside, &away] {
        ws.open_doc(uri(f), TTL.into(), 1);
        assert!(ws.analysis(&uri(f)).is_some());
    }
    ws.close_doc(&uri(&inside));
    ws.close_doc(&uri(&away));
    assert!(ws.disk.contains_key(&uri(&inside)));
    assert!(!ws.disk.contains_key(&uri(&away)));
    assert!(ws.knowledge().get(&uri(&inside)).is_some());
    assert!(ws.knowledge().get(&uri(&away)).is_none());
    // Nor does a file in a skipped directory come back.
    let skipped = write(&root.0, "node_modules/x.ttl", TTL);
    ws.open_doc(uri(&skipped), TTL.into(), 1);
    ws.analysis(&uri(&skipped));
    ws.close_doc(&uri(&skipped));
    assert!(!ws.disk.contains_key(&uri(&skipped)));
    assert!(ws.knowledge().get(&uri(&skipped)).is_none());
}

#[test]
fn the_read_accessors_agree_with_the_documents_and_the_index() {
    let root = TempDir::new("accessors");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    assert_eq!(ws.roots(), std::slice::from_ref(&root.0));
    ws.set_rescan_sparql(true);
    assert!(ws.rescan_sparql);
    let prefixes = PrefixMap::from_iter([("ex".to_string(), "http://e.org/".to_string())]);
    ws.set_settings_prefixes(prefixes.clone());
    assert_eq!(ws.settings_prefixes(), &prefixes);

    let paths = [
        write(&root.0, "a.ttl", TTL),
        write(&root.0, "b.trig", TTL),
        write(&root.0, "q.rq", "SELECT * WHERE { ?s ?p ?o }\n"),
    ];
    let [a, b, q] = paths.each_ref().map(|p| uri(p));
    let agree = |ws: &Workspace| {
        fn sorted(mut v: Vec<&str>) -> Vec<&str> {
            v.sort_unstable();
            v
        }
        assert_eq!(
            sorted(ws.open_uris().collect()),
            sorted(ws.open.keys().map(String::as_str).collect())
        );
        assert_eq!(
            sorted(ws.indexed_uris().collect()),
            sorted(ws.disk.keys().map(String::as_str).collect())
        );
        assert_eq!(ws.indexed_count(), ws.disk.len());
        for u in [&a, &b, &q] {
            assert_eq!(ws.is_open(u), ws.open.contains_key(u));
            assert_eq!(ws.version(u), ws.open.get(u).map(|d| d.version));
            assert_eq!(ws.doc_kind(u), ws.open.get(u).map(|d| d.kind));
            assert_eq!(ws.is_indexed(u), ws.disk.contains_key(u));
            assert_eq!(
                ws.indexed_analysis(u).map(Arc::as_ptr),
                ws.disk.get(u).map(Arc::as_ptr)
            );
        }
    };

    let parsed = ws.parse_paths(paths[..2].to_vec());
    ws.add_disk(parsed, true);
    agree(&ws);
    assert!(ws.is_indexed(&a) && ws.is_indexed(&b) && !ws.is_indexed(&q));
    assert_eq!(ws.open_uris().count(), 0);
    assert_eq!(ws.version(&a), None);

    // An open RDF document that is also on disk is open, not indexed.
    ws.open_doc(a.clone(), TTL.into(), 3);
    ws.open_doc(q.clone(), "SELECT * WHERE { ?s ?p ?o }\n".into(), 1);
    agree(&ws);
    assert!(ws.is_open(&a) && !ws.is_indexed(&a));
    assert_eq!(ws.version(&a), Some(3));
    assert_eq!(ws.doc_kind(&a), Some(DocKind::Rdf(Syntax::Turtle)));
    assert_eq!(ws.doc_kind(&q), Some(DocKind::Sparql));
    assert_eq!(ws.doc_kind(&b), None);
    assert_eq!(ws.indexed_uris().collect::<Vec<_>>(), [b.as_str()]);

    // The index does not take back a file while it is open.
    let parsed = ws.parse_paths(vec![paths[0].clone()]);
    ws.add_disk(parsed, true);
    agree(&ws);
    assert!(!ws.is_indexed(&a));

    // Closing it indexes it from disk again.
    ws.close_doc(&a);
    ws.close_doc(&q);
    agree(&ws);
    assert!(!ws.is_open(&a) && ws.is_indexed(&a));
    assert_eq!((ws.version(&a), ws.doc_kind(&a)), (None, None));
    assert_eq!(ws.indexed_count(), 2);
}

#[test]
fn what_changed_before_the_initial_index_landed_is_not_undone_by_it() {
    let root = TempDir::new("pre-merge");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.expect_initial_index();
    let deleted = write(&root.0, "deleted.ttl", TTL);
    let changed = write(&root.0, "changed.ttl", TTL);
    let in_folder = write(&root.0, "folder/f.ttl", TTL);
    let kept = write(&root.0, "kept.ttl", TTL);
    // The index reads the disk as it is now ...
    let mut reader = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let files = reader.parse_paths(Workspace::scan_roots(std::slice::from_ref(&root.0)));
    assert_eq!(files.len(), 4);
    // ... and the disk changes before it lands.
    std::fs::remove_file(&deleted).unwrap();
    ws.reindex_disk_file(&deleted);
    std::fs::write(&changed, "@prefix ex: <http://e.org/> .\nex:New a ex:B .\n").unwrap();
    ws.reindex_disk_file(&changed);
    std::fs::remove_dir_all(root.0.join("folder")).unwrap();
    ws.remove_under_any(&[root.0.join("folder")]);
    ws.merge_initial_index(files);
    assert!(
        !ws.disk.contains_key(&uri(&deleted)),
        "a deleted file came back"
    );
    assert!(
        !ws.disk.contains_key(&uri(&in_folder)),
        "a deleted folder's file came back"
    );
    assert!(ws.disk.contains_key(&uri(&kept)));
    assert!(ws.knowledge().is_described("http://e.org/New"));
    assert!(!ws.knowledge().is_described("http://e.org/A") || ws.disk.contains_key(&uri(&kept)));
    assert!(ws.knowledge().get(&uri(&deleted)).is_none());
    // After the merge nothing is remembered any more.
    assert!(ws.touched.is_none());
}

#[test]
fn a_panic_in_analysis_is_that_document_s_error_not_the_end_of_the_server() {
    let quiet = QuietPanics::new();
    let mut ws = Workspace::default();
    let (bad, good, facts) = (
        "file:///w/bad.ttl",
        "file:///w/good.ttl",
        "file:///w/facts.ttl",
    );
    ws.open_doc(bad.into(), format!("# {PANIC_IN_ANALYSIS}\n{TTL}"), 1);
    ws.open_doc(good.into(), TTL.into(), 1);
    ws.open_doc(facts.into(), format!("# {PANIC_IN_FACTS}\n{TTL}"), 1);
    // The analyses of several documents at once, as the batch does.
    assert_eq!(ws.knowledge().docs().count(), 3);
    for u in [bad, facts] {
        let a = ws.analysis(u).unwrap();
        let e: Vec<_> = a
            .parsed
            .errors
            .iter()
            .filter(|e| e.code == "internal-error")
            .collect();
        assert_eq!(e.len(), 1, "{u}");
        assert!(
            e[0].message.contains("internal error") && e[0].message.contains("report"),
            "{}",
            e[0].message
        );
    }
    assert!(ws.analysis(good).unwrap().parsed.errors.is_empty());
    // One at a time, as a request does.
    ws.change_doc(
        good,
        2,
        vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: format!("# {PANIC_IN_ANALYSIS}\n"),
        }],
        Encoding::Utf16,
    );
    assert!(
        ws.analysis(good)
            .unwrap()
            .parsed
            .errors
            .iter()
            .any(|e| e.code == "internal-error")
    );
    // Indexed files too.
    let root = TempDir::new("panic-disk");
    let f = write(&root.0, "bad.ttl", &format!("# {PANIC_IN_ANALYSIS}\n{TTL}"));
    let g = write(&root.0, "ok.ttl", TTL);
    let mut disk = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    disk.index_paths(vec![f.clone(), g.clone()]);
    drop(quiet);
    assert_eq!(disk.disk.len(), 2);
    assert!(
        disk.disk[&uri(&f)]
            .parsed
            .errors
            .iter()
            .any(|e| e.code == "internal-error")
    );
    assert!(disk.disk[&uri(&g)].parsed.errors.is_empty());
}

#[test]
fn prefixes_are_cached_by_file_and_a_change_reaches_only_the_files_it_serves() {
    let root = TempDir::new("prefix-scope");
    let ns = |n: &str| format!("@prefix {n}: <http://{n}.org/> .\n");
    let pf = write(&root.0, "a/prefixes.ttl", &ns("one"));
    let nested = write(&root.0, "a/sub/prefixes.ttl", &ns("two"));
    let files: Vec<PathBuf> = [
        write(&root.0, "a/x.ttl", "ex:s ex:p ex:o .\n"),
        write(&root.0, "a/deeper/y.ttl", "ex:s ex:p ex:o .\n"),
        write(&root.0, "a/sub/z.ttl", "ex:s ex:p ex:o .\n"),
        write(&root.0, "b/w.ttl", "ex:s ex:p ex:o .\n"),
    ]
    .into();
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.index_paths(files.clone());
    let ext = |ws: &mut Workspace, f: &Path| ws.external_prefixes_for(&uri(f));
    // One map per prefixes.ttl, shared by every directory it serves.
    assert!(Arc::ptr_eq(
        &ext(&mut ws, &files[0]),
        &ext(&mut ws, &files[1])
    ));
    assert!(!Arc::ptr_eq(
        &ext(&mut ws, &files[0]),
        &ext(&mut ws, &files[2])
    ));
    assert!(ext(&mut ws, &files[0]).contains_key("one"));
    assert!(
        ext(&mut ws, &files[2]).contains_key("two") && !ext(&mut ws, &files[2]).contains_key("one")
    );
    assert!(ext(&mut ws, &files[3]).is_empty());
    // An `.owl` or `.rdf` name gets them too: its text may be Turtle.
    for name in ["a/onto.owl", "a/DATA.RDF"] {
        assert!(
            ext(&mut ws, &root.0.join(name)).contains_key("one"),
            "{name}"
        );
    }

    // Saving a prefixes.ttl without a change to what it declares re-parses nothing.
    assert!(ws.prefixes_changed(Some(&pf)).is_empty());
    // Changing it re-parses the files it serves, not the nested folder's, not b/'s.
    std::fs::write(&pf, format!("{}{}", ns("one"), ns("three"))).unwrap();
    let mut stale = ws.prefixes_changed(Some(&pf));
    stale.sort();
    let mut served = vec![files[0].clone(), files[1].clone()];
    served.sort();
    assert_eq!(stale, served);
    assert!(ext(&mut ws, &files[0]).contains_key("three"));
    // The reparse lands only for files still as they were and with today's prefixes.
    let jobs = ws.prefix_jobs(stale);
    let parsed = ws.reader().parse(jobs);
    assert_eq!(ws.add_reparsed(parsed), 2);
    assert!(ws.disk[&uri(&files[0])].external.contains_key("three"));

    // A prefixes.ttl created later takes over the files below it.
    let made = write(&root.0, "b/prefixes.ttl", &ns("four"));
    assert_eq!(ws.prefixes_changed(Some(&made)), [files[3].clone()]);
    // A deleted one gives its files back to the nearest above (here, none).
    std::fs::remove_file(&nested).unwrap();
    assert_eq!(ws.prefixes_changed(Some(&nested)), [files[2].clone()]);
    assert!(!ext(&mut ws, &files[2]).contains_key("two"));
    // A settings change reaches every file whose prefixes it changes.
    ws.settings_prefixes
        .insert("set".into(), "http://set.org/".into());
    assert_eq!(ws.prefixes_changed(None).len(), 4);
    // A reparse made before another change is dropped.
    let jobs = ws.prefix_jobs(vec![files[0].clone()]);
    let parsed = ws.reader().parse(jobs);
    ws.settings_prefixes
        .insert("later".into(), "http://later.org/".into());
    ws.prefixes_changed(None);
    assert_eq!(ws.add_reparsed(parsed), 0);
}

#[cfg(windows)]
#[test]
fn vscode_and_url_spellings_of_a_windows_path_match() {
    assert_eq!(
        normalize_uri("file:///c%3A/Users/x/a.ttl"),
        normalize_uri("file:///C:/Users/x/a.ttl")
    );
    assert_eq!(
        normalize_uri("file:///c:/Users/x/a.ttl"),
        "file:///C:/Users/x/a.ttl"
    );
}

const QUERY: &str = "PREFIX ex: <http://e.org/>\nSELECT * WHERE { ?s a ex:B }\n";

/// Every call used to walk the roots and read every query file again (9.6
/// ms for a tree of 20,000 entries with no hit). The files are found once
/// and kept; the watcher's events keep them current.
#[test]
fn sparql_files_are_found_once_and_kept_current() {
    let root = TempDir::new("sparql-kept");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let a = write(&root.0, "q/a.rq", QUERY);
    let b = write(&root.0, "b.sparql", QUERY);
    write(&root.0, "node_modules/x.rq", QUERY);
    let uris = |ws: &mut Workspace| {
        let mut v: Vec<String> = ws
            .sparql_documents()
            .iter()
            .map(|q| q.uri.clone())
            .collect();
        v.sort();
        v
    };
    let mut both = vec![uri(&a), uri(&b)];
    both.sort();
    assert_eq!(uris(&mut ws), both);

    // No directory is read again: a file nobody reported is not found...
    let c = write(&root.0, "c.ru", "CLEAR ALL\n");
    assert_eq!(uris(&mut ws), both);
    // ...and one the watcher reported is, with the ones that were deleted
    // and the one that changed, read together.
    let before = ws
        .sparql_documents()
        .into_iter()
        .find(|q| q.uri == uri(&b))
        .unwrap();
    let untouched = ws
        .sparql_documents()
        .into_iter()
        .find(|q| q.uri == uri(&a))
        .unwrap();
    std::fs::remove_file(&a).unwrap();
    write(
        &root.0,
        "b.sparql",
        "SELECT * WHERE { ?s ?p ?o . ?o ?p ?s }\n",
    );
    write(&root.0, "node_modules/y.rq", QUERY);
    ws.sparql_files_changed(&[
        a.clone(),
        b.clone(),
        c.clone(),
        c.clone(),
        root.0.join("node_modules/y.rq"),
        root.0.join("gone.rq"),
    ]);
    let mut now = vec![uri(&b), uri(&c)];
    now.sort();
    assert_eq!(uris(&mut ws), now);
    let after = ws
        .sparql_documents()
        .into_iter()
        .find(|q| q.uri == uri(&b))
        .unwrap();
    assert!(!Arc::ptr_eq(&before, &after));
    drop(untouched);

    // A file written without the watcher saying so is read again when asked.
    write(
        &root.0,
        "b.sparql",
        "SELECT * WHERE { ?s ?p ?o }  # longer than before\n",
    );
    let again = ws
        .sparql_documents()
        .into_iter()
        .find(|q| q.uri == uri(&b))
        .unwrap();
    assert!(!Arc::ptr_eq(&after, &again));
    let kept = ws
        .sparql_documents()
        .into_iter()
        .find(|q| q.uri == uri(&b))
        .unwrap();
    assert!(
        Arc::ptr_eq(&again, &kept),
        "an unchanged file is not parsed again"
    );

    // A deleted folder takes its queries with it.
    write(&root.0, "d/e/f.rq", QUERY);
    ws.sparql_files_changed(&[root.0.join("d/e/f.rq")]);
    assert_eq!(ws.sparql_documents().len(), 3);
    std::fs::remove_dir_all(root.0.join("d")).unwrap();
    ws.remove_under_any(&[root.0.join("d")]);
    assert_eq!(ws.sparql_documents().len(), 2);
    // A changed prefixes.ttl parses them again with the new prefixes.
    let was = ws
        .sparql_documents()
        .into_iter()
        .find(|q| q.uri == uri(&c))
        .unwrap();
    let prefixes = write(&root.0, "prefixes.ttl", "@prefix p: <http://p.org/> .\n");
    ws.prefixes_changed(Some(&prefixes));
    let now = ws
        .sparql_documents()
        .into_iter()
        .find(|q| q.uri == uri(&c))
        .unwrap();
    assert!(!Arc::ptr_eq(&was, &now));
    assert!(now.external.contains_key("p"));
}

/// An open query is the open text, not the file; a closed one is the file
/// again. The index of files is not asked before the first call.
#[test]
fn open_queries_win_over_their_files_in_the_sparql_documents() {
    let root = TempDir::new("sparql-open");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let a = write(&root.0, "a.rq", QUERY);
    // Before the first call, an event reads nothing.
    ws.sparql_files_changed(std::slice::from_ref(&a));
    assert!(ws.sparql_files.is_none());
    let open = "SELECT * WHERE { ?x ?y ?z }\n";
    ws.open_doc(uri(&a), open.into(), 1);
    let docs = ws.sparql_documents();
    assert_eq!(docs.len(), 1);
    assert!(docs[0].doc.text().contains("?x ?y ?z"));
    ws.close_doc(&uri(&a));
    let docs = ws.sparql_documents();
    assert_eq!(docs.len(), 1);
    assert!(docs[0].doc.text().contains("ex:B"));
}

/// One notification's changed files were parsed one at a time, each with a
/// pass of its own (2,000 files: 108 ms against 73 ms for the parallel
/// initial index). They are parsed in one pass.
#[test]
fn changed_files_are_indexed_in_one_pass() {
    let root = TempDir::new("batch");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let mut paths: Vec<PathBuf> = (0..50)
        .map(|i| write(&root.0, &format!("d{}/f{i}.ttl", i % 5), TTL))
        .collect();
    let before = INDEX_PASSES.with(|c| c.get());
    // Twice listed, not RDF, outside, in a skipped folder, and missing.
    let junk = [
        write(&root.0, "a.txt", TTL),
        write(&root.0, "node_modules/n.ttl", TTL),
        root.0.join("missing.ttl"),
        paths[0].clone(),
    ];
    let mut all = paths.clone();
    all.extend(junk);
    assert!(ws.reindex_disk_files(&all));
    assert_eq!(INDEX_PASSES.with(|c| c.get()) - before, 1);
    assert_eq!(ws.disk.len(), 50);
    // Deletions are dropped in the same call as the changes.
    std::fs::remove_file(&paths[1]).unwrap();
    let fresh = write(&root.0, "fresh.ttl", TTL);
    let before = INDEX_PASSES.with(|c| c.get());
    assert!(ws.reindex_disk_files(&[paths[1].clone(), fresh.clone()]));
    assert_eq!(INDEX_PASSES.with(|c| c.get()) - before, 1);
    assert!(ws.disk.contains_key(&uri(&fresh)) && !ws.disk.contains_key(&uri(&paths[1])));
    paths.push(fresh);
    assert_eq!(ws.disk.len(), 50);
    // Nothing to do, nothing changed.
    assert!(!ws.reindex_disk_files(&[root.0.join("nope.ttl")]));
    assert!(!ws.reindex_disk_files(&[]));
}

/// Silences panic messages while it lives, for tests whose panics are
/// the point. The panic hook is global to the test binary, where tests
/// run in parallel: a hook taken and set back by two tests at once can
/// end up silent for good. So the hook is replaced once, by one that
/// prints as the previous one did unless a `QuietPanics` is alive, and
/// the guards hold a lock, so one is alive at a time. Dropping the guard
/// (also when its test panics) makes the hook print again.
pub(crate) struct QuietPanics {
    _lock: std::sync::MutexGuard<'static, ()>,
}

static QUIET_LOCK: Mutex<()> = Mutex::new(());
static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static QUIET_HOOK: std::sync::Once = std::sync::Once::new();

impl QuietPanics {
    pub(crate) fn new() -> QuietPanics {
        // A test that failed while holding the lock poisoned it; the
        // guard it held was dropped all the same.
        let lock = QUIET_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        QUIET_HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if !QUIET.load(std::sync::atomic::Ordering::SeqCst) {
                    previous(info);
                }
            }));
        });
        QUIET.store(true, std::sync::atomic::Ordering::SeqCst);
        QuietPanics { _lock: lock }
    }
}

impl Drop for QuietPanics {
    fn drop(&mut self) {
        QUIET.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn quiet_panics_silence_only_while_alive() {
    // Under the lock no other test's guard is alive.
    let quiet_now = || {
        let _lock = QUIET_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        QUIET.load(std::sync::atomic::Ordering::SeqCst)
    };
    let quiet = QuietPanics::new();
    assert!(QUIET.load(std::sync::atomic::Ordering::SeqCst));
    assert!(std::panic::catch_unwind(|| panic!("silenced")).is_err());
    drop(quiet);
    assert!(!quiet_now());
    // A guard dropped while its test panics makes the hook print again.
    let _ = std::panic::catch_unwind(|| {
        let _quiet = QuietPanics::new();
        panic!("silenced too");
    });
    assert!(!quiet_now());
}

// ---- incremental sync (textDocument/didChange with ranges) ----

fn pos(line: u32, character: u32) -> lsp_types::Position {
    lsp_types::Position { line, character }
}

/// A ranged change from `(l0, c0)` to `(l1, c1)`.
fn ranged(
    (l0, c0): (u32, u32),
    (l1, c1): (u32, u32),
    text: &str,
) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
        range: Some(lsp_types::Range::new(pos(l0, c0), pos(l1, c1))),
        range_length: None,
        text: text.into(),
    }
}

fn full(text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
        range: None,
        range_length: None,
        text: text.into(),
    }
}

#[test]
fn an_owl_document_takes_its_kind_from_its_text_at_each_change() {
    let xml = "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
            <rdf:Description rdf:about=\"http://example.org/a\">\
            <rdf:type rdf:resource=\"http://example.org/C\"/>\
            </rdf:Description></rdf:RDF>\n";
    let ttl = "<http://example.org/a> a <http://example.org/C> , <http://example.org/D> .\n";
    let mut ws = Workspace::default();
    let uri = "file:///w/onto.owl";
    // The web editor's language id for an `.owl` file.
    ws.open_doc_in(uri.into(), xml.into(), 1, Some("xml"));
    assert_eq!(ws.doc_kind(uri), Some(DocKind::Rdf(Syntax::RdfXml)));
    let a = ws.analysis(uri).unwrap();
    assert_eq!(
        (a.parsed.syntax, a.parsed.triples.len()),
        (Syntax::RdfXml, 1)
    );
    ws.change_doc(uri, 2, vec![full(ttl)], Encoding::Utf16);
    assert_eq!(ws.doc_kind(uri), Some(DocKind::Rdf(Syntax::Turtle)));
    let a = ws.analysis(uri).unwrap();
    assert_eq!(
        (a.parsed.syntax, a.parsed.triples.len()),
        (Syntax::Turtle, 2)
    );
    ws.change_doc(uri, 3, vec![full(xml)], Encoding::Utf16);
    assert_eq!(ws.analysis(uri).unwrap().parsed.syntax, Syntax::RdfXml);
    // Emptied, or only white space: still RDF/XML, until Turtle is typed.
    for (version, text) in [(4, ""), (5, " \n\t"), (6, ttl)] {
        ws.change_doc(uri, version, vec![full(text)], Encoding::Utf16);
        let want = if text == ttl {
            Syntax::Turtle
        } else {
            Syntax::RdfXml
        };
        assert_eq!(ws.doc_kind(uri), Some(DocKind::Rdf(want)), "{text:?}");
    }
    // Opened empty: RDF/XML.
    let empty = "file:///w/new.rdf";
    ws.open_doc_in(empty.into(), String::new(), 1, Some("xml"));
    assert_eq!(ws.doc_kind(empty), Some(DocKind::Rdf(Syntax::RdfXml)));
    // A kind the caller names stays, whatever the text becomes.
    let given = "file:///w/given.owl";
    ws.open_doc_as(given.into(), ttl.into(), 1, Syntax::Turtle);
    ws.change_doc(given, 2, vec![full(xml)], Encoding::Utf16);
    assert_eq!(ws.doc_kind(given), Some(DocKind::Rdf(Syntax::Turtle)));
}

#[test]
fn change_doc_applies_ranged_changes_in_order() {
    let ab = "ab\ncd\n";
    // (what, encoding, text before, the changes in one batch, text after)
    let cases: Vec<(
        &str,
        Encoding,
        &str,
        Vec<TextDocumentContentChangeEvent>,
        &str,
    )> = vec![
        (
            "insert",
            Encoding::Utf16,
            ab,
            vec![ranged((0, 1), (0, 1), "X")],
            "aXb\ncd\n",
        ),
        (
            "delete",
            Encoding::Utf16,
            ab,
            vec![ranged((0, 0), (0, 2), "")],
            "\ncd\n",
        ),
        (
            "delete a line break",
            Encoding::Utf16,
            ab,
            vec![ranged((0, 2), (1, 0), "")],
            "abcd\n",
        ),
        (
            "replace",
            Encoding::Utf16,
            ab,
            vec![ranged((1, 0), (1, 2), "ZZZ")],
            "ab\nZZZ\n",
        ),
        (
            "a batch in order: the second change's positions are in the first's result",
            Encoding::Utf16,
            ab,
            vec![ranged((0, 0), (0, 0), "1\n"), ranged((1, 0), (1, 1), "2")],
            "1\n2b\ncd\n",
        ),
        (
            "a full-text change in a batch, then a ranged one on its text",
            Encoding::Utf16,
            ab,
            vec![full("xyz\n"), ranged((0, 1), (0, 2), "Y")],
            "xYz\n",
        ),
        // U+1F600 is two UTF-16 code units and four UTF-8 bytes.
        (
            "UTF-16 after an astral character",
            Encoding::Utf16,
            "\u{1F600}x\n",
            vec![ranged((0, 2), (0, 3), "y")],
            "\u{1F600}y\n",
        ),
        (
            "UTF-8 after an astral character",
            Encoding::Utf8,
            "\u{1F600}x\n",
            vec![ranged((0, 4), (0, 5), "y")],
            "\u{1F600}y\n",
        ),
        (
            "a UTF-16 column inside the astral character goes to its start",
            Encoding::Utf16,
            "\u{1F600}x\n",
            vec![ranged((0, 1), (0, 1), "Z")],
            "Z\u{1F600}x\n",
        ),
        (
            "a column past the end of its line is the line's end",
            Encoding::Utf16,
            ab,
            vec![ranged((0, 99), (0, 99), "X")],
            "abX\ncd\n",
        ),
        (
            "a line past the end of the file is the file's end",
            Encoding::Utf16,
            ab,
            vec![ranged((7, 0), (9, 9), "!")],
            "ab\ncd\n!",
        ),
        (
            "a range ending past the end of the file",
            Encoding::Utf16,
            ab,
            vec![ranged((1, 1), (9, 0), "")],
            "ab\nc",
        ),
        (
            "a reversed range is the same range",
            Encoding::Utf16,
            ab,
            vec![ranged((1, 1), (0, 1), "-")],
            "a-d\n",
        ),
        (
            "an empty document",
            Encoding::Utf16,
            "",
            vec![ranged((0, 0), (0, 0), "a")],
            "a",
        ),
    ];
    let mut failed = Vec::new();
    for (what, enc, before, changes, after) in cases {
        let mut ws = Workspace::default();
        let uri = "file:///w/a.ttl";
        ws.open_doc(uri.into(), before.into(), 1);
        ws.change_doc(uri, 2, changes, enc);
        if ws.text(uri) != Some(after) {
            failed.push(format!("{what}: {:?}, expected {after:?}", ws.text(uri)));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn fifty_changes_in_one_notification_are_fifty_notifications_of_one() {
    let start =
        "@prefix ex: <http://e/> .\r\nex:a ex:p \"\u{e9}\u{1F600}\" .\n\nex:b ex:q ex:c .\n";
    let pieces = [
        "",
        "x",
        "\n",
        "\u{e9}",
        "\u{1F600}",
        "ab\r\n",
        "\n\n",
        " ex:d",
    ];
    let mut seed = 7u32;
    let mut next = |n: u32| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (seed >> 16) % n
    };
    for enc in [Encoding::Utf16, Encoding::Utf8] {
        let changes: Vec<_> = (0..50)
            .map(|_| {
                let a = (next(8), next(30));
                let b = (a.0 + next(2), next(30));
                ranged(a, b, pieces[next(pieces.len() as u32) as usize])
            })
            .collect();
        let uri = "file:///w/a.ttl";
        let mut one = Workspace::default();
        one.open_doc(uri.into(), start.into(), 1);
        one.change_doc(uri, 2, changes.clone(), enc);
        let mut many = Workspace::default();
        many.open_doc(uri.into(), start.into(), 1);
        // And as a line index of the whole text per change converts them.
        let mut indexed = start.to_string();
        for (i, ch) in changes.into_iter().enumerate() {
            let r = ch.range.unwrap();
            let lines = crate::analysis::LineIndex::new(&indexed);
            let at = |p| crate::analysis::offset_in(&lines, &indexed, enc, p).unwrap() as usize;
            let (s, e) = (at(r.start), at(r.end));
            indexed.replace_range(s.min(e)..e.max(s), &ch.text);
            many.change_doc(uri, 2 + i as i32, vec![ch], enc);
        }
        assert_eq!(one.text(uri), many.text(uri), "{enc:?}");
        assert_eq!(one.text(uri), Some(indexed.as_str()), "{enc:?}");
        assert_ne!(one.text(uri), Some(start), "{enc:?}");
    }
}

/// The analysis of `uri` (made now), checked against a fresh workspace
/// opened on the same text: the same line index, and the same positions
/// for every reference and error.
fn assert_as_fresh(ws: &mut Workspace, uri: &str, step: &str) {
    let a = ws.analysis(uri).unwrap();
    let mut fresh = Workspace::default();
    fresh.open_doc(uri.into(), a.text.to_string(), 1);
    let f = fresh.analysis(uri).unwrap();
    assert_eq!(*a.text, *ws.text(uri).unwrap(), "{step}");
    assert_eq!(a.lines, f.lines, "{step}");
    let spans = |a: &Analysis| -> Vec<Span> {
        let refs = a.parsed.refs.iter().map(|r| r.span);
        refs.chain(a.parsed.errors.iter().map(|e| e.span)).collect()
    };
    assert_eq!(spans(&a), spans(&f), "{step}");
    for span in spans(&a) {
        for enc in [Encoding::Utf16, Encoding::Utf8] {
            let range = a.range(enc, span);
            assert_eq!(range, f.range(enc, span), "{step}");
            assert_eq!(a.offset(enc, range.start), Some(span.start), "{step}");
        }
    }
}

#[test]
fn analyses_after_edits_match_a_fresh_open() {
    let long = format!(
        "ex:long rdfs:label \"{}\" .",
        "\u{e9}t\u{1F600}".repeat(800)
    );
    let text = format!(
        "@prefix ex: <http://e/> .\r\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\r\n\
             ex:a ex:p \"\u{e9}\" .\rex:b ex:q ex:c .\n{long}\nex:d ex:p ex:e .\n"
    );
    let mut ws = Workspace::default();
    let uri = "file:///w/a.ttl";
    ws.open_doc(uri.into(), text, 1);
    assert_as_fresh(&mut ws, uri, "open");
    let edited = |ws: &Workspace| matches!(ws.open[uri].analyzed, Analyzed::Edited { .. });
    // A batch: inside the long line, a line break added and one removed,
    // a `\r\n` split, before and after one another.
    ws.change_doc(
        uri,
        2,
        vec![
            ranged((3, 300), (3, 302), "\n\u{4e2d}"),
            ranged((0, 25), (0, 25), "x"),
            ranged((2, 0), (2, 4), "ex:z"),
            ranged((1, 50), (2, 2), ""),
        ],
        Encoding::Utf16,
    );
    assert!(edited(&ws));
    assert_as_fresh(&mut ws, uri, "a batch of ranged changes");
    // Two batches before one analysis collapse into one edit.
    ws.change_doc(
        uri,
        3,
        vec![ranged((4, 10), (4, 10), "\r")],
        Encoding::Utf16,
    );
    ws.change_doc(uri, 4, vec![ranged((0, 0), (0, 1), "@")], Encoding::Utf8);
    assert!(edited(&ws));
    assert_as_fresh(&mut ws, uri, "two batches");
    ws.change_doc(
        uri,
        5,
        vec![full("ex:a ex:p \"\u{e9}\" .\n")],
        Encoding::Utf16,
    );
    assert!(matches!(ws.open[uri].analyzed, Analyzed::No));
    assert_as_fresh(&mut ws, uri, "a full change");
    ws.change_doc(
        uri,
        6,
        vec![ranged((0, 12), (1, 0), "\u{1F600}\" .\r\n")],
        Encoding::Utf16,
    );
    assert!(edited(&ws));
    assert_as_fresh(&mut ws, uri, "ranged after a full change");
    // A full change then a ranged one in one batch: nothing to build on.
    ws.change_doc(
        uri,
        7,
        vec![full("ex:a ex:p ex:b .\n"), ranged((0, 0), (0, 0), "\u{e9}")],
        Encoding::Utf16,
    );
    assert!(matches!(ws.open[uri].analyzed, Analyzed::No));
    assert_as_fresh(&mut ws, uri, "full then ranged in one batch");
    // New prefixes: analyzed again from the same text, then edited.
    ws.open_doc(
        "file:///w/prefixes.ttl".into(),
        "@prefix ex: <http://e/> .\n".into(),
        1,
    );
    assert!(matches!(ws.open[uri].analyzed, Analyzed::Outdated(_)));
    ws.change_doc(uri, 8, vec![ranged((0, 3), (0, 3), "\n")], Encoding::Utf16);
    assert!(edited(&ws));
    assert_as_fresh(&mut ws, uri, "an edit after new prefixes");
}

#[test]
fn documents_analyzed_together_after_edits_match_a_fresh_open() {
    let mut ws = Workspace::default();
    let uris = ["file:///w/a.ttl", "file:///w/b.ttl", "file:///w/c.trig"];
    for u in uris {
        ws.open_doc(
            u.into(),
            "ex:a ex:p \"\u{e9}\" .\r\nex:b ex:p ex:c .\n".into(),
            1,
        );
    }
    ws.knowledge();
    for (i, u) in uris.into_iter().enumerate() {
        let at = (i as u32 % 2, 2 + i as u32);
        ws.change_doc(u, 2, vec![ranged(at, at, "\u{1F600}\r")], Encoding::Utf16);
        assert!(matches!(ws.open[u].analyzed, Analyzed::Edited { .. }));
    }
    // Several stale documents are analyzed in parallel.
    ws.knowledge();
    for u in uris {
        assert!(ws.open[u].analyzed.current().is_some());
        assert_as_fresh(&mut ws, u, u);
    }
}

/// How the analyses of an open document were parsed since the last
/// call, and whether any splice differed from a full parse.
fn paths(ws: &mut Workspace) -> Vec<(ParsePath, Option<String>)> {
    let log = ws.take_parse_log().into_iter();
    log.map(|r| (r.path, r.difference)).collect()
}

#[test]
fn an_edit_is_spliced_unless_the_setting_is_off() {
    let text = "@prefix ex: <http://e/> .\nex:a ex:p 1 .\nex:b ex:p 2 .\n\
                    ex:c ex:p \"x\" .\nex:d ex:p 3 .\n";
    let uris = ["file:///w/a.ttl", "file:///w/b.trig"];
    for (on, edited) in [(true, ParsePath::Spliced), (false, ParsePath::Off)] {
        let mut ws = Workspace::default();
        ws.set_check_incremental(true);
        ws.set_incremental(on);
        for u in uris {
            ws.open_doc(u.into(), text.into(), 1);
        }
        ws.knowledge();
        let whole = (ParsePath::Whole, None);
        assert_eq!(paths(&mut ws), [whole.clone(), whole]);
        // Inside the literal: one analysis, then both in parallel.
        ws.change_doc(
            uris[0],
            2,
            vec![ranged((4, 11), (4, 11), "y")],
            Encoding::Utf16,
        );
        assert_as_fresh(&mut ws, uris[0], "one");
        assert_eq!(paths(&mut ws), [(edited, None)]);
        for u in uris {
            ws.change_doc(u, 3, vec![ranged((4, 11), (4, 12), "")], Encoding::Utf16);
        }
        ws.knowledge();
        assert_eq!(paths(&mut ws), [(edited, None), (edited, None)]);
        for u in uris {
            assert_as_fresh(&mut ws, u, u);
        }
        // A directive: in full whatever the setting.
        ws.change_doc(
            uris[0],
            4,
            vec![ranged((0, 8), (0, 10), "ey")],
            Encoding::Utf16,
        );
        ws.analysis(uris[0]);
        let directive = match on {
            true => ParsePath::Fallback(crate::analysis::Fallback::Directive),
            false => ParsePath::Off,
        };
        assert_eq!(paths(&mut ws), [(directive, None)]);
    }
    // Without the check, nothing is recorded.
    let mut ws = Workspace::default();
    ws.open_doc(uris[0].into(), text.into(), 1);
    ws.change_doc(
        uris[0],
        2,
        vec![ranged((4, 11), (4, 11), "y")],
        Encoding::Utf16,
    );
    assert_as_fresh(&mut ws, uris[0], "unchecked");
    assert_eq!(paths(&mut ws), []);
}

#[test]
fn change_doc_reanalyzes_and_ignores_documents_that_are_not_open() {
    let mut ws = Workspace::default();
    let uri = "file:///w/a.ttl";
    let text = "@prefix ex: <http://e.org/> .\nex:a ex:p ex:b .\n";
    ws.open_doc(uri.into(), text.into(), 1);
    assert_eq!(ws.analysis(uri).unwrap().parsed.triples.len(), 1);
    // Turn `ex:b .` into `ex:b , ex:c .`: two triples.
    ws.change_doc(
        uri,
        2,
        vec![ranged((1, 14), (1, 14), " , ex:c")],
        Encoding::Utf16,
    );
    assert_eq!(
        ws.text(uri),
        Some("@prefix ex: <http://e.org/> .\nex:a ex:p ex:b , ex:c .\n")
    );
    assert_eq!(ws.analysis(uri).unwrap().parsed.triples.len(), 2);
    assert_eq!(ws.open[uri].version, 2);
    ws.change_doc("file:///w/closed.ttl", 3, vec![full("x")], Encoding::Utf16);
    assert!(ws.text("file:///w/closed.ttl").is_none());
}

// ---- effects ----

/// `Effects` with knowledge and data changed.
fn knowledge_and_data() -> Effects {
    Effects {
        data: true,
        ..Effects::knowledge()
    }
}

fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    paths
}

#[test]
fn an_edit_to_a_data_file_is_that_document_alone() {
    let root = TempDir::new("fx-data");
    let a = write(&root.0, "a.ttl", TTL);
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.index_paths(vec![a.clone()]);
    let one = Effects {
        dirty: Dirty::One(uri(&a)),
        ..Effects::default()
    };
    assert_eq!(ws.open_doc(uri(&a), TTL.into(), 1), one);
    let edit = vec![full("@prefix ex: <http://e.org/> .\nex:C a ex:D .\n")];
    assert_eq!(ws.change_doc(&uri(&a), 2, edit, Encoding::Utf16), one);
    assert_eq!(
        ws.save_doc(&uri(&a)),
        Effects {
            data: true,
            ..Effects::default()
        }
    );
}

#[test]
fn an_edit_to_an_open_prefix_file_reaches_open_documents_now_and_indexed_files_on_save() {
    let root = TempDir::new("fx-prefixes");
    let pf = write(
        &root.0,
        "prefixes.ttl",
        "@prefix one: <http://one.org/> .\n",
    );
    let served = vec![
        write(&root.0, "x.ttl", "one:s one:p one:o .\n"),
        write(&root.0, "deeper/y.ttl", "one:s one:p one:o .\n"),
    ];
    // A folder with a prefixes.ttl of its own is not served.
    let own = write(
        &root.0,
        "own/prefixes.ttl",
        "@prefix two: <http://two.org/> .\n",
    );
    let not_served = write(&root.0, "own/z.ttl", "two:s two:p two:o .\n");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.index_paths(vec![
        pf.clone(),
        served[0].clone(),
        served[1].clone(),
        own,
        not_served,
    ]);
    let open = uri(&root.0.join("open.ttl"));
    ws.open_doc(open.clone(), "one:s one:p one:o .\n".into(), 1);
    assert!(ws.analysis(&open).unwrap().external.contains_key("one"));

    let text = "@prefix one: <http://one.org/> .\n";
    assert_eq!(ws.open_doc(uri(&pf), text.into(), 1), Effects::knowledge());
    let edit = vec![full("@prefix uno: <http://uno.org/> .\n")];
    let fx = ws.change_doc(&uri(&pf), 2, edit, Encoding::Utf16);
    // Every open document is due, and none is left to parse again yet.
    assert_eq!(fx, Effects::knowledge());
    assert!(ws.analysis(&open).unwrap().external.contains_key("uno"));
    let indexed = |ws: &Workspace| ws.indexed_analysis(&uri(&served[0])).unwrap().clone();
    assert!(indexed(&ws).external.contains_key("one"));

    let mut fx = ws.save_doc(&uri(&pf));
    fx.reparse.sort();
    assert_eq!(
        fx,
        Effects {
            reparse: sorted(served),
            ..knowledge_and_data()
        }
    );
}

#[test]
fn closing_a_context_file_with_unsaved_edits_reaches_its_users() {
    let root = TempDir::new("fx-context");
    let ctx = write(&root.0, "ctx.jsonld", CTX);
    let closed = write(&root.0, "b.jsonld", USES_CTX);
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.index_paths(vec![closed.clone()]);
    let user = uri(&root.0.join("a.jsonld"));
    ws.open_doc(user.clone(), USES_CTX.into(), 1);
    let triples = |ws: &mut Workspace| ws.analysis(&user).unwrap().parsed.triples.len();
    assert_eq!(triples(&mut ws), 1);
    let indexed = |ws: &Workspace| ws.indexed_analysis(&uri(&closed)).unwrap().clone();
    let before = indexed(&ws);

    let unsaved = r#"{"@context": {"name": null}}"#;
    assert_eq!(
        ws.open_doc(uri(&ctx), unsaved.into(), 1),
        Effects::knowledge()
    );
    assert_eq!(triples(&mut ws), 0);
    // An edit does not parse the indexed users again.
    assert!(Arc::ptr_eq(&before, &indexed(&ws)));

    assert_eq!(ws.close_doc(&uri(&ctx)), Effects::knowledge());
    assert_eq!(triples(&mut ws), 1);
    assert!(!Arc::ptr_eq(&before, &indexed(&ws)));
}

/// JSON-LD documents that name one context file under different spellings
/// (an escaped letter, on Windows a lower-case drive) all follow edits, a
/// save and a close of the file under its canonical URI. A web context URL
/// is taken as written.
#[test]
fn aliased_spellings_of_a_context_file_reach_its_users() {
    let root = TempDir::new("ctx-aliases");
    let ctx_path = write(&root.0, "ctx.jsonld", CTX);
    let ctx = uri(&ctx_path);
    let escaped = ctx.replace("/ctx.jsonld", "/c%74x.jsonld");
    #[cfg(not(windows))]
    let spellings = [ctx.clone(), escaped];
    #[cfg(windows)]
    let spellings = {
        // `file:///c:/...` and `file:///c%3A/...` (VS Code's).
        let mut lower = ctx.clone();
        lower[8..9].make_ascii_lowercase();
        let vscode = format!("{}%3A{}", &lower[..9], &lower[10..]);
        [ctx.clone(), escaped, vscode, lower]
    };
    let user =
        |at: &str| format!(r#"{{"@context": "{at}", "@id": "http://example.org/a", "name": "A"}}"#);
    let mut on_disk = Vec::new();
    let mut open = Vec::new();
    for (i, at) in spellings.iter().enumerate() {
        on_disk.push(write(&root.0, &format!("disk{i}.jsonld"), &user(at)));
        open.push((uri(&root.0.join(format!("open{i}.jsonld"))), user(at)));
    }
    let start = |ctx_text: Option<&str>| {
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.index_paths(on_disk.clone());
        for (u, text) in &open {
            ws.open_doc(u.clone(), text.clone(), 1);
        }
        if let Some(text) = ctx_text {
            ws.open_doc(ctx.clone(), text.into(), 1);
        }
        ws
    };
    let triples = |ws: &mut Workspace| {
        let mut all = Vec::new();
        for (u, _) in &open {
            all.push(ws.analysis(u).unwrap().parsed.triples.clone());
        }
        for p in &on_disk {
            all.push(ws.indexed_analysis(&uri(p)).unwrap().parsed.triples.clone());
        }
        all
    };

    let mut ws = start(None);
    for (u, _) in &open {
        assert_eq!(context_errors(&mut ws, u), Vec::<String>::new(), "{u}");
    }
    let before = triples(&mut ws);
    assert!(before.iter().all(|t| t.len() == 1), "{before:?}");

    let gone = r#"{"@context": {"name": null}}"#;
    let other = r#"{"@context": {"name": "http://example.org/other"}}"#;
    ws.open_doc(ctx.clone(), gone.into(), 1);
    assert_eq!(triples(&mut ws), triples(&mut start(Some(gone))));
    ws.change_doc(&ctx, 2, vec![full(other)], Encoding::Utf16);
    assert_eq!(triples(&mut ws), triples(&mut start(Some(other))));
    std::fs::write(&ctx_path, other).unwrap();
    ws.save_doc(&ctx);
    let saved = triples(&mut ws);
    assert_eq!(saved, triples(&mut start(Some(other))));
    assert!(saved.iter().all(|t| t.len() == 1) && saved != before);
    ws.change_doc(&ctx, 3, vec![full(gone)], Encoding::Utf16);
    assert_eq!(triples(&mut ws), triples(&mut start(Some(gone))));
    ws.close_doc(&ctx);
    assert_eq!(triples(&mut ws), saved);
    assert_eq!(saved, triples(&mut start(None)));

    // Two spellings of one web URL are two contexts.
    let web = "https://example.org/ctx.jsonld";
    let mut ws = Workspace::default();
    ws.set_context_loader(Arc::new(crate::jsonld::NoNetwork::default().with(web, CTX)));
    ws.open_doc("file:///w/web.jsonld".into(), user(web), 1);
    assert_eq!(
        ws.analysis("file:///w/web.jsonld")
            .unwrap()
            .parsed
            .triples
            .len(),
        1
    );
    assert!(!ws.context_changed("https://example.org/c%74x.jsonld", true));
    assert!(ws.context_changed(web, true));
}

#[test]
fn a_settings_prefix_change_lists_every_indexed_turtle_file() {
    let root = TempDir::new("fx-settings");
    let turtle = vec![
        write(&root.0, "a.ttl", TTL),
        write(&root.0, "sub/b.ttl", TTL),
        write(&root.0, "c.trig", TTL),
    ];
    let others = [
        write(
            &root.0,
            "d.nt",
            "<http://e.org/s> <http://e.org/p> <http://e.org/o> .\n",
        ),
        write(&root.0, "e.jsonld", r#"{"@id": "http://e.org/s"}"#),
    ];
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.index_paths(turtle.iter().chain(&others).cloned().collect());
    let mut prefixes = PrefixMap::default();
    prefixes.insert("set".into(), "http://set.org/".into());
    let mut fx = ws.set_settings_prefixes(prefixes);
    fx.reparse.sort();
    assert_eq!(
        fx,
        Effects {
            reparse: sorted(turtle),
            ..knowledge_and_data()
        }
    );
}

#[test]
fn a_prefix_file_and_a_data_file_changed_together_parse_the_data_once() {
    let root = TempDir::new("fx-together");
    let pf = write(
        &root.0,
        "prefixes.ttl",
        "@prefix ex: <http://old.example/> .\n",
    );
    let data = write(&root.0, "data.ttl", "ex:a ex:p ex:o .\n");
    let other = write(&root.0, "other.ttl", "ex:a ex:p ex:o .\n");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let created = |p: &PathBuf| (p.clone(), Change::Created);
    ws.files_changed(&[created(&data), created(&pf), created(&other)]);
    assert_eq!(ws.indexed_count(), 3);

    std::fs::write(&pf, "@prefix ex: <http://newer.example/> .\n").unwrap();
    std::fs::write(&data, "ex:b ex:p ex:o .\n").unwrap();
    let before = INDEX_PASSES.with(|c| c.get());
    let fx = ws.files_changed(&[
        (data.clone(), Change::Changed),
        (pf.clone(), Change::Changed),
    ]);
    assert_eq!(INDEX_PASSES.with(|c| c.get()) - before, 1);
    let a = ws.indexed_analysis(&uri(&data)).unwrap();
    assert!(
        a.external
            .get("ex")
            .is_some_and(|i| i == "http://newer.example/")
    );
    // Only the file that did not change is left to parse again.
    assert_eq!(
        fx,
        Effects {
            reparse: vec![other],
            on_disk: vec![data, pf],
            ..knowledge_and_data()
        }
    );
}

#[test]
fn a_deleted_folder_forgets_the_files_below_it() {
    let root = TempDir::new("fx-folder");
    let below = [
        write(&root.0, "gone/a.ttl", TTL),
        write(&root.0, "gone/deep/b.ttl", TTL),
    ];
    let kept = write(&root.0, "kept.ttl", TTL);
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.index_paths(below.iter().chain([&kept]).cloned().collect());
    std::fs::remove_dir_all(root.0.join("gone")).unwrap();
    let fx = ws.files_changed(&[(root.0.join("gone"), Change::Deleted)]);
    assert_eq!(fx, knowledge_and_data());
    assert!(below.iter().all(|p| !ws.is_indexed(&uri(p))));
    assert!(ws.is_indexed(&uri(&kept)));
    // Nothing indexed below it: nothing changed.
    let fx = ws.files_changed(&[(root.0.join("gone"), Change::Deleted)]);
    assert_eq!(fx, Effects::default());
}

#[test]
fn a_query_changed_on_disk_makes_closed_files_due_but_changes_no_knowledge() {
    let root = TempDir::new("fx-query");
    let q = write(&root.0, "q.rq", "SELECT * WHERE { ?s ?p ?o }\n");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let fx = ws.files_changed(&[(q, Change::Changed)]);
    assert_eq!(
        fx,
        Effects {
            closed_files: true,
            ..Effects::default()
        }
    );
}

#[test]
fn effects_add_up() {
    let (a, b) = ("file:///w/a.ttl", "file:///w/b.ttl");
    let mut fx = Effects::one(a);
    fx.add(Effects::one(a));
    assert_eq!(fx.dirty, Dirty::One(a.into()));
    fx.add(Effects {
        reparse: vec![PathBuf::from("/w/c.ttl")],
        ..Effects::default()
    });
    assert_eq!(fx.dirty, Dirty::One(a.into()));
    fx.add(Effects::one(b));
    assert_eq!(fx.dirty, Dirty::AllOpen);
    fx.add(Effects {
        data: true,
        ..Effects::default()
    });
    assert_eq!(
        fx,
        Effects {
            dirty: Dirty::AllOpen,
            data: true,
            reparse: vec![PathBuf::from("/w/c.ttl")],
            ..Effects::default()
        }
    );
}
// ---- kept diagnostics ----

const KEPT_HEAD: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

/// The diagnostics of the open `uri` as the server composes them, with
/// what the document keeps between edits.
fn kept_diagnostics(ws: &mut Workspace, uri: &str) -> Vec<Diagnostic> {
    let a = ws.analysis(uri).unwrap();
    let settings = crate::settings::Settings::default();
    ws.diagnostics(uri, &a, Encoding::Utf16, &DiagnosticInputs::new(&settings))
}

/// The diagnostics of `uri` in full, in a fresh workspace with the
/// texts of `ws`'s open documents.
fn fresh_diagnostics(ws: &Workspace, uri: &str) -> Vec<Diagnostic> {
    let mut fresh = Workspace::default();
    for (u, d) in &ws.open {
        fresh.open_doc(u.clone(), d.text.clone(), 1);
    }
    let a = fresh.analysis(uri).unwrap();
    let settings = crate::settings::Settings::default();
    let inputs = DiagnosticInputs::new(&settings);
    diagnose::document(
        diagnose::Doc::Rdf(&a),
        fresh.knowledge(),
        Encoding::Utf16,
        &inputs,
    )
}

fn count(ds: &[Diagnostic], code: &str) -> usize {
    let code = Some(lsp_types::NumberOrString::String(code.into()));
    ds.iter().filter(|d| d.code == code).count()
}

/// Where `needle` ends in `text`, as (line, column), ASCII.
fn after(text: &str, needle: &str) -> (u32, u32) {
    let i = text.find(needle).unwrap() + needle.len();
    let line = text[..i].matches('\n').count();
    let col = i - text[..i].rfind('\n').map_or(0, |n| n + 1);
    (line as u32, col as u32)
}

/// Forty classes, each with a label and an ill-typed literal, so each
/// statement has findings to keep.
fn forty_classes() -> String {
    let mut text = KEPT_HEAD.to_string();
    for i in 0..40 {
        text += &format!(
            "ex:C{i} a owl:Class ; rdfs:label \"Class {i}\" ; ex:size \"x{i}\"^^xsd:integer .\n"
        );
    }
    text
}

#[test]
fn an_edit_in_one_statement_checks_only_its_window_again() {
    let uri = "file:///w/a.ttl";
    let text = forty_classes();
    let mut ws = Workspace::default();
    ws.open_doc(uri.into(), text.clone(), 1);
    let before = kept_diagnostics(&mut ws, uri);
    assert_eq!(count(&before, "invalid-literal"), 40);
    assert_eq!(ws.open[uri].kept.checked.statements, 40);
    // A character typed in the label of class 20, then a line break
    // after the label: what follows moves down a line.
    let at = after(&text, "\"Class 20");
    ws.change_doc(uri, 2, vec![ranged(at, at, "x")], Encoding::Utf16);
    let got = kept_diagnostics(&mut ws, uri);
    let checked = ws.open[uri].kept.checked;
    // The splice parses one statement before the edited one again (error
    // recovery and lookahead cross a statement's end): two statements,
    // their 12 references (the subject, `a`, the class, two predicates
    // and the datatype), their two labels.
    assert_eq!(checked.statements, 2, "{checked:?}");
    assert_eq!(checked.refs, 12, "{checked:?}");
    assert_eq!(checked.labels, 2, "{checked:?}");
    assert_eq!(got, fresh_diagnostics(&ws, uri));
    let at = (at.0, at.1 + 2);
    ws.change_doc(uri, 3, vec![ranged(at, at, "\n")], Encoding::Utf16);
    let got = kept_diagnostics(&mut ws, uri);
    assert_eq!(ws.open[uri].kept.checked.statements, 2);
    assert_eq!(got, fresh_diagnostics(&ws, uri));
    assert_eq!(count(&got, "invalid-literal"), 40);
}

#[test]
fn a_label_edit_that_makes_a_duplicate_in_another_document_refreshes_it() {
    let (a, b) = ("file:///w/a.ttl", "file:///w/b.ttl");
    let mut ws = Workspace::default();
    let text = forty_classes();
    ws.open_doc(a.into(), text.clone(), 1);
    let other = format!("{KEPT_HEAD}ex:B a owl:Class ; rdfs:label \"Valve\" .\n");
    ws.open_doc(b.into(), other, 1);
    kept_diagnostics(&mut ws, a);
    assert_eq!(count(&kept_diagnostics(&mut ws, b), "duplicate-label"), 0);
    // Class 20 is labelled "Valve" too.
    let (line, end) = after(&text, "\"Class 20");
    let range = ranged((line, end - "Class 20".len() as u32), (line, end), "Valve");
    ws.change_doc(a, 2, vec![range], Encoding::Utf16);
    let got = kept_diagnostics(&mut ws, a);
    assert_eq!(count(&got, "duplicate-label"), 1);
    let got = kept_diagnostics(&mut ws, b);
    assert_eq!(got, fresh_diagnostics(&ws, b));
    let dup: Vec<&str> = got
        .iter()
        .filter(|d| count(std::slice::from_ref(d), "duplicate-label") == 1)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(dup, ["Label \"Valve\" is also the label of ex:C20"]);
    // B took it from the index's change set, its one label again; none
    // of its statements.
    let checked = ws.open[b].kept.checked;
    assert_eq!((checked.statements, checked.labels), (0, 1), "{checked:?}");
}

#[test]
fn a_prefix_edit_checks_everything_again() {
    let uri = "file:///w/a.ttl";
    let text = forty_classes();
    let mut ws = Workspace::default();
    ws.set_check_incremental(true);
    ws.open_doc(uri.into(), text.clone(), 1);
    kept_diagnostics(&mut ws, uri);
    let at = after(&text, "example.");
    ws.change_doc(
        uri,
        2,
        vec![ranged(at, (at.0, at.1 + 3), "com")],
        Encoding::Utf16,
    );
    let got = kept_diagnostics(&mut ws, uri);
    let directive = ParsePath::Fallback(crate::analysis::Fallback::Directive);
    assert_eq!(paths(&mut ws).last(), Some(&(directive, None)));
    assert_eq!(ws.open[uri].kept.checked.statements, 40);
    assert_eq!(got, fresh_diagnostics(&ws, uri));
}

#[test]
fn a_deprecation_reaches_the_uses_outside_the_window() {
    let uri = "file:///w/a.ttl";
    let mut text = format!("{KEPT_HEAD}ex:old a owl:Class ; owl:deprecated false .\n");
    for i in 0..30 {
        text += &format!("ex:a{i} rdfs:subClassOf ex:old .\n");
    }
    let mut ws = Workspace::default();
    ws.open_doc(uri.into(), text.clone(), 1);
    assert_eq!(count(&kept_diagnostics(&mut ws, uri), "deprecated-term"), 0);
    let at = after(&text, "owl:deprecated ");
    ws.change_doc(
        uri,
        2,
        vec![ranged(at, (at.0, at.1 + 5), "true")],
        Encoding::Utf16,
    );
    let got = kept_diagnostics(&mut ws, uri);
    assert_eq!(count(&got, "deprecated-term"), 30);
    assert_eq!(got, fresh_diagnostics(&ws, uri));
    // The window, then the 31 references to ex:old.
    let checked = ws.open[uri].kept.checked;
    assert!(checked.statements <= 2, "{checked:?}");
    assert!(checked.refs <= 31 + 8, "{checked:?}");
}

/// Edits in a block of statements without a label, in the middle of a
/// document of labelled classes whose labels repeat inside it and in a
/// second document, keep every label finding of both documents as a
/// fresh open gives them; so does an edit of a label after them.
#[test]
fn an_edit_outside_every_label_keeps_the_label_findings_of_both_documents() {
    let (a, b) = ("file:///w/a.ttl", "file:///w/b.ttl");
    let head = format!("{KEPT_HEAD}@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n");
    // Class 8 of ten has the label of the class before it; class 5 has
    // two untagged labels.
    let class = |i: usize| {
        let label = if i % 10 == 8 { i - 1 } else { i };
        let second = if i % 10 == 5 {
            format!(" ; rdfs:label \"Other {i}\"")
        } else {
            String::new()
        };
        format!(
            "ex:C{i} a owl:Class ; rdfs:label \"Class {label}\" ; \
             skos:prefLabel \"Name {i}\"@en{second} .\n"
        )
    };
    let mut text = head.clone();
    (0..30).for_each(|i| text += &class(i));
    for j in 0..10 {
        text += &format!("ex:n{j} ex:note \"Note {j}\" ; ex:size \"x{j}\"^^xsd:integer .\n");
    }
    (30..60).for_each(|i| text += &class(i));
    let other = format!(
        "{head}ex:B0 a owl:Class ; rdfs:label \"Class 3\" .\n\
         ex:B1 a owl:Class ; rdfs:label \"Class 42\" .\n\
         ex:B2 a owl:Class ; skos:prefLabel \"Name 50\"@en .\n"
    );
    let mut ws = Workspace::default();
    ws.open_doc(a.into(), text, 1);
    ws.open_doc(b.into(), other, 1);
    let check = |ws: &mut Workspace, step: &str| {
        for uri in [a, b] {
            let got = kept_diagnostics(ws, uri);
            assert_eq!(got, fresh_diagnostics(ws, uri), "{uri}, {step}");
            assert_as_fresh(ws, uri, step);
        }
    };
    check(&mut ws, "open");
    let ds = kept_diagnostics(&mut ws, a);
    assert_eq!(count(&ds, "duplicate-label"), 12 + 3, "{ds:#?}");
    assert_eq!(count(&ds, "multiple-labels"), 12);
    assert_eq!(count(&kept_diagnostics(&mut ws, b), "duplicate-label"), 3);
    // Each edit is two statements or more from a label: a character typed
    // in a literal and taken back, a triple added (the triples after it
    // move), a line break; then a label edited.
    let text = |ws: &Workspace| ws.text(a).unwrap().to_string();
    let edits: [(&str, usize, &str); 5] = [
        ("\"Note 4", 0, "y"),
        ("\"Note 4y", 1, ""),
        ("\"x5\"^^xsd:integer", 0, " ; ex:note \"More\""),
        ("ex:n6 ", 0, "\n"),
        ("\"Class 42", 1, "x"),
    ];
    for (version, (needle, cut, new)) in (2..).zip(edits) {
        let at = after(&text(&ws), needle);
        let range = ranged((at.0, at.1 - cut as u32), at, new);
        ws.change_doc(a, version, vec![range], Encoding::Utf16);
        check(&mut ws, &format!("{needle} -> {new}"));
    }
    assert_eq!(count(&kept_diagnostics(&mut ws, b), "duplicate-label"), 2);
}

#[test]
fn with_the_splice_off_nothing_is_kept() {
    let uri = "file:///w/a.ttl";
    let text = forty_classes();
    let mut ws = Workspace::default();
    ws.open_doc(uri.into(), text.clone(), 1);
    kept_diagnostics(&mut ws, uri);
    ws.set_incremental(false);
    let at = after(&text, "\"Class 20");
    ws.change_doc(uri, 2, vec![ranged(at, at, "x")], Encoding::Utf16);
    let got = kept_diagnostics(&mut ws, uri);
    assert_eq!(ws.open[uri].kept.checked, Default::default());
    assert_eq!(got, fresh_diagnostics(&ws, uri));
}

/// A vocabulary of `n` classes, each labelled and defined: 2.4 MB at
/// 17,200.
fn big_vocabulary(n: usize) -> String {
    let mut text = String::from(
        "@prefix ex: <http://example.org/> .\n\
             @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             @prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n",
    );
    for i in 0..n {
        text += &format!(
            "\nex:C{i} a owl:Class ;\n    rdfs:label \"Class {i}\"@en ;\n    \
                 skos:definition \"A class numbered {i}.\"@en ;\n    rdfs:subClassOf ex:C{} .\n",
            (i + 1) % n
        );
    }
    text
}

/// The diagnostics of a keystroke in a label in the middle of a 2.4 MB
/// vocabulary, with what the document keeps, against composing them in
/// full on the same analysis and index (the analysis is not timed). On
/// an M-series laptop (load 1.4 to 2.2, medians of 25 keystrokes, five
/// runs): 0.77 ms kept against 6.4 ms in full, 0.12 of it, the spread
/// under 2%. In full, the label checks and the term uses were 80% of
/// the composition (sampled); kept, the checks a keystroke still runs
/// in full limit it: `undescribed-term` about 70% (it reads the
/// workspace and shares one budget of suggestions), the SHACL lint's
/// scan for SHACL terms about 25%, and bringing the kept findings up to
/// date about 3%. The bound, a quarter, leaves twice the room.
#[cfg(not(debug_assertions))]
#[test]
fn timing_the_diagnostics_of_a_keystroke_are_a_fraction_of_a_full_composition() {
    let n = 17_200;
    let text = big_vocabulary(n);
    let uri = "file:///w/vocab.ttl";
    let mut ws = Workspace::default();
    ws.open_doc(uri.into(), text.clone(), 1);
    kept_diagnostics(&mut ws, uri);
    let label = format!("\"Class {}", n / 2);
    let (line, mut col) = after(&text, &label);
    let settings = crate::settings::Settings::default();
    let inputs = DiagnosticInputs::new(&settings);
    let (mut kept, mut full) = (Vec::new(), Vec::new());
    for version in 2..27 {
        ws.change_doc(
            uri,
            version,
            vec![ranged((line, col), (line, col), "x")],
            Encoding::Utf16,
        );
        col += 1;
        let a = ws.analysis(uri).unwrap();
        let t = std::time::Instant::now();
        let got = ws.diagnostics(uri, &a, Encoding::Utf16, &inputs);
        kept.push(t.elapsed());
        let t = std::time::Instant::now();
        let want = diagnose::document(
            diagnose::Doc::Rdf(&a),
            ws.knowledge(),
            Encoding::Utf16,
            &inputs,
        );
        full.push(t.elapsed());
        assert_eq!(got, want);
        // The window and the label's two keys, not the 17,200 labels.
        let checked = ws.open[uri].kept.checked;
        assert!(
            checked.statements <= 2 && checked.labels <= 4,
            "{checked:?}"
        );
    }
    let median = |v: &mut [std::time::Duration]| {
        v.sort();
        v[v.len() / 2]
    };
    let (kept, full) = (median(&mut kept), median(&mut full));
    eprintln!("diagnostics of a keystroke: kept {kept:?}, in full {full:?}");
    assert!(kept * 4 < full, "kept {kept:?}, in full {full:?}");
}

/// Typing in a label in the middle of a 2.4 MB vocabulary (17,200
/// classes), one character and one analysis at a time, as the server
/// does on each keystroke: with the splice, then in full. On an
/// M-series laptop (load about 3, medians, five runs): 4.3 to 4.4 ms a
/// keystroke spliced against 11.0 to 11.5 ms in full (0.39 of it), 24
/// keystrokes 105 to 130 ms. Where a spliced keystroke goes, timed
/// apart: the analysis maps rebuilt in full 1.4 ms, copying the shared
/// old parse for the splice 0.6 ms, the splice 0.13 ms, the line index
/// 0.01 ms; then the facts for the knowledge index 1.3 ms
/// (`DocFacts::new`, a full rebuild), dropping the old analysis 0.6 ms
/// and the index swap. The maps and the facts limit it; the full
/// keystroke pays the same facts and drop, which is why the share is
/// not lower. A Windows CI runner measured 24.8 ms spliced against
/// 50.7 ms in full, so the checks are ratios; the wall-clock bound only
/// catches a keystroke gone badly wrong.
#[cfg(not(debug_assertions))]
#[test]
fn timing_typing_in_the_middle_of_a_big_file_is_spliced() {
    let n = 17_200;
    let text = big_vocabulary(n);
    let uri = "file:///w/vocab.ttl";
    let mut ws = Workspace::default();
    ws.open_doc(uri.into(), text.clone(), 1);
    ws.analysis(uri);
    // The cursor: after `"Class 8600` on its line.
    let label = format!("\"Class {}", n / 2);
    let line = text[..text.find(&label).unwrap()].matches('\n').count() as u32;
    let mut col = 4 + "rdfs:label ".len() as u32 + label.len() as u32;
    let mut version = 1;
    let mut typing = |ws: &mut Workspace, keys: usize| -> Vec<std::time::Duration> {
        (0..keys)
            .map(|_| {
                version += 1;
                let at = (line, col);
                col += 1;
                let t = std::time::Instant::now();
                ws.change_doc(uri, version, vec![ranged(at, at, "x")], Encoding::Utf16);
                ws.analysis(uri);
                t.elapsed()
            })
            .collect()
    };
    let keys = 24;
    let spliced = typing(&mut ws, keys);
    ws.set_incremental(false);
    let mut full = typing(&mut ws, 9);
    // The typing gave what a fresh open gives, and it was spliced.
    ws.set_incremental(true);
    ws.set_check_incremental(true);
    typing(&mut ws, 1);
    assert_eq!(paths(&mut ws), [(ParsePath::Spliced, None)]);
    let got = ws.analysis(uri).unwrap();
    let mut fresh = Workspace::default();
    fresh.open_doc(uri.into(), got.text.to_string(), 1);
    let want = fresh.analysis(uri).unwrap();
    assert_eq!(crate::analysis::same_analysis(&got, &want), Ok(()));

    let median = |v: &mut [std::time::Duration]| {
        v.sort();
        v[v.len() / 2]
    };
    let (mut first, mut last) = (spliced[..8].to_vec(), spliced[keys - 8..].to_vec());
    let all: std::time::Duration = spliced.iter().sum();
    let (first, last, full) = (median(&mut first), median(&mut last), median(&mut full));
    let msg = format!("a keystroke spliced: first {first:?}, last {last:?}; in full {full:?}");
    // Each keystroke costs what the one before did: nothing piles up.
    assert!(last < first * 2, "{msg}");
    // N keystrokes cost N times one, a fraction of N full analyses.
    let n = keys as u32;
    assert!(all * 5 < full * n * 3, "{msg}; {keys} spliced {all:?}");
    assert!(first.as_millis() < 200, "{msg}");
}

/// Typing in a label in the middle of PATO (20 MB of RDF/XML, from
/// `test/corpus`), one character at a time, as the server does on each
/// keystroke: the analysis in full (RDF/XML is not spliced) with the
/// knowledge index's facts, then the diagnostics with the lints of
/// decision 8 (checked again in full: RDF/XML is not followed). On an
/// M-series laptop (load 1.7, medians of 7 keystrokes, three runs): 87
/// to 91 ms a keystroke, the analysis 77 to 81 ms of it (the parse about
/// 70, `timing_rdfxml_parse`) and the diagnostics 9 to 10 ms; the open
/// 103 to 131 ms. That is the cost per megabyte of a Turtle file
/// analysed in full (`timing_typing_in_the_middle_of_a_big_file_is_spliced`).
/// The bound leaves the room CI runners need (a Windows runner was 2.3
/// times slower there) and catches a keystroke gone badly wrong; past
/// it, a large open RDF/XML file would be analysed on save only.
#[cfg(not(debug_assertions))]
#[test]
fn timing_a_keystroke_in_a_large_rdfxml_file() {
    // Absent: skipped, but a failure in CI (`CI=true`).
    let Some(path) = corpus_path("obo-pato.rdf") else {
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap();
    let uri = "file:///w/obo-pato.owl";
    let mut ws = Workspace::default();
    let t = std::time::Instant::now();
    ws.open_doc(uri.into(), text.clone(), 1);
    let a = ws.analysis(uri).unwrap();
    assert_eq!(a.parsed.syntax, Syntax::RdfXml);
    assert!(a.parsed.errors.is_empty());
    let first = kept_diagnostics(&mut ws, uri);
    let open = t.elapsed();
    // A label in the middle of the file: type after its first letter.
    let mid = text[text.len() / 2..].find("<rdfs:label").unwrap() + text.len() / 2;
    let label = mid + text[mid..].find('>').unwrap() + 2;
    let line = text[..label].matches('\n').count() as u32;
    let col0 = (label - text[..label].rfind('\n').map_or(0, |n| n + 1)) as u32;
    let settings = crate::settings::Settings::default();
    let inputs = DiagnosticInputs::new(&settings);
    let (mut analysis, mut diagnostics, mut keystroke) = (Vec::new(), Vec::new(), Vec::new());
    for (version, col) in (2..9).zip(col0..) {
        let t = std::time::Instant::now();
        ws.change_doc(
            uri,
            version,
            vec![ranged((line, col), (line, col), "x")],
            Encoding::Utf16,
        );
        let a = ws.analysis(uri).unwrap();
        let analysed = t.elapsed();
        let got = ws.diagnostics(uri, &a, Encoding::Utf16, &inputs);
        let all = t.elapsed();
        assert_eq!(got.len(), first.len());
        analysis.push(analysed);
        diagnostics.push(all - analysed);
        keystroke.push(all);
    }
    let median = |v: &mut [std::time::Duration]| {
        v.sort();
        v[v.len() / 2]
    };
    let (analysis, diagnostics, keystroke) = (
        median(&mut analysis),
        median(&mut diagnostics),
        median(&mut keystroke),
    );
    eprintln!(
        "obo-pato.owl: open {open:?} ({} diagnostics); a keystroke {keystroke:?}: analysis {analysis:?}, diagnostics {diagnostics:?}",
        first.len()
    );
    assert!(keystroke.as_millis() < 750, "{keystroke:?}");
}

// ---- context uses: recorded for the parses the workspace keeps ----

/// A context file that maps `name` into the namespace `ns`.
fn named_in(ns: &str) -> String {
    format!(r#"{{"@context": {{"name": "http://example.org/{ns}#name"}}}}"#)
}

/// A JSON-LD document whose context is `context` (relative or absolute).
fn using(context: &str) -> String {
    format!(r#"{{"@context": "{context}", "@id": "http://example.org/d", "name": "D"}}"#)
}

/// The triples of `uri`: the open document's, else the indexed copy's.
fn triples_of(ws: &mut Workspace, uri: &str) -> Vec<crate::syntax::Triple> {
    ws.analysis(uri).unwrap().parsed.triples.clone()
}

/// The triples' predicates, to say what a test expects.
fn predicates(t: &[crate::syntax::Triple]) -> Vec<String> {
    t.iter().map(|t| t.p.to_string()).collect()
}

const WEB_CONTEXT: &str = "http://example.org/context.jsonld";

/// A web context that is being fetched until `arrived` is set.
struct Fetching {
    arrived: Arc<std::sync::atomic::AtomicBool>,
}

impl ContextLoader for Fetching {
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        if url != WEB_CONTEXT {
            return Err(LoadError::NotFound);
        }
        if !self.arrived.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(LoadError::Pending);
        }
        Ok(RemoteDocument {
            url: url.to_string(),
            text: named_in("web").into(),
        })
    }
}

fn fetching(arrived: bool) -> (Arc<dyn ContextLoader>, Arc<std::sync::atomic::AtomicBool>) {
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(arrived));
    (
        Arc::new(Fetching {
            arrived: flag.clone(),
        }),
        flag,
    )
}

/// Interleaving 1: the initial index reads D's saved text (context A)
/// on its own thread while D is open with context B; the merge drops
/// that parse, and a change to B still reaches D.
#[test]
fn an_open_document_keeps_its_context_when_the_initial_index_reads_its_saved_text() {
    let root = TempDir::new("c1-index-open");
    write(&root.0, "a.jsonld", &named_in("a"));
    let b = write(&root.0, "b.jsonld", &named_in("b"));
    let d = uri(&write(&root.0, "d.jsonld", &using("a.jsonld")));
    let open = using("b.jsonld");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.expect_initial_index();
    ws.open_doc(d.clone(), open.clone(), 1);
    ws.analysis(&d);
    let mut index = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    index.use_shared_contexts(ws.shared_contexts());
    let files = index.parse_paths(Workspace::scan_roots(std::slice::from_ref(&root.0)));
    ws.merge_initial_index(files);
    std::fs::write(&b, named_in("b2")).unwrap();
    ws.files_changed(&[(b, Change::Changed)]);

    let mut fresh = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    fresh.open_doc(d.clone(), open, 1);
    let want = triples_of(&mut fresh, &d);
    assert_eq!(predicates(&want), ["http://example.org/b2#name"]);
    assert_eq!(triples_of(&mut ws, &d), want);
}

/// Interleaving 3: a prefix reparse reads D's saved text (context A),
/// D is opened with context B meanwhile, and the merge drops the
/// reparse; a change to B still reaches D.
#[test]
fn a_rejected_prefix_reparse_does_not_hide_an_open_document_from_its_context() {
    let root = TempDir::new("c1-reparse-open");
    write(&root.0, "a.jsonld", &named_in("a"));
    let b = write(&root.0, "b.jsonld", &named_in("b"));
    let path = write(&root.0, "d.jsonld", &using("a.jsonld"));
    let d = uri(&path);
    let open = using("b.jsonld");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.index_paths(vec![path.clone()]);
    let jobs = ws.prefix_jobs(vec![path]);
    ws.open_doc(d.clone(), open.clone(), 1);
    ws.analysis(&d);
    let parsed = ws.reader().parse(jobs);
    assert_eq!(ws.add_reparsed(parsed), 0);
    std::fs::write(&b, named_in("b2")).unwrap();
    ws.files_changed(&[(b, Change::Changed)]);

    let mut fresh = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    fresh.open_doc(d.clone(), open, 1);
    let want = triples_of(&mut fresh, &d);
    assert_eq!(predicates(&want), ["http://example.org/b2#name"]);
    assert_eq!(triples_of(&mut ws, &d), want);
}

/// Interleaving 2: the initial index reads closed D (context A), the
/// watcher reindexes D rewritten to context C before the index lands,
/// and a change to C reaches D.
#[test]
fn a_file_rewritten_during_the_initial_index_follows_its_new_context() {
    let root = TempDir::new("c1-index-rewrite");
    write(&root.0, "a.jsonld", &named_in("a"));
    let c = write(&root.0, "c.jsonld", &named_in("c"));
    let path = write(&root.0, "d.jsonld", &using("a.jsonld"));
    let d = uri(&path);
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.expect_initial_index();
    let mut index = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    index.use_shared_contexts(ws.shared_contexts());
    let files = index.parse_paths(Workspace::scan_roots(std::slice::from_ref(&root.0)));
    std::fs::write(&path, using("c.jsonld")).unwrap();
    ws.files_changed(&[(path.clone(), Change::Changed)]);
    ws.merge_initial_index(files);
    std::fs::write(&c, named_in("c2")).unwrap();
    ws.files_changed(&[(c, Change::Changed)]);

    let mut fresh = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    fresh.index_paths(Workspace::scan_roots(std::slice::from_ref(&root.0)));
    let want = triples_of(&mut fresh, &d);
    assert_eq!(predicates(&want), ["http://example.org/c2#name"]);
    assert_eq!(triples_of(&mut ws, &d), want);
}

/// A closed `untitled:` document (no file, nothing indexed) no longer
/// counts as a user of the context it named.
#[test]
fn closing_an_untitled_json_ld_document_forgets_its_contexts() {
    let root = TempDir::new("c1-untitled");
    let ctx = uri(&write(&root.0, "ctx.jsonld", &named_in("a")));
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    let untitled = "untitled:Untitled-1";
    ws.open_doc_as(untitled.into(), using(&ctx), 1, Syntax::JsonLd);
    let t = triples_of(&mut ws, untitled);
    assert_eq!(predicates(&t), ["http://example.org/a#name"]);
    ws.close_doc(untitled);
    assert!(
        !ws.context_changed(&ctx, true),
        "a closed untitled document still uses the context"
    );
}

/// A web context that is still being fetched, then arrives, reaches
/// the open document and the indexed one that asked for it.
#[test]
fn a_context_that_fails_then_arrives_reaches_its_documents() {
    let root = TempDir::new("c1-fetch");
    let indexed = write(&root.0, "i.jsonld", &using(WEB_CONTEXT));
    let i = uri(&indexed);
    let o = uri(&root.0.join("o.jsonld"));
    let (loader, arrived) = fetching(false);
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.set_context_loader(loader);
    ws.index_paths(vec![indexed.clone()]);
    ws.open_doc(o.clone(), using(WEB_CONTEXT), 1);
    let waiting = triples_of(&mut ws, &o);
    arrived.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_ne!(ws.context_fetched(WEB_CONTEXT), Effects::default());

    let mut fresh = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    fresh.set_context_loader(fetching(true).0);
    fresh.index_paths(vec![indexed]);
    fresh.open_doc(o.clone(), using(WEB_CONTEXT), 1);
    for doc in [&o, &i] {
        let want = triples_of(&mut fresh, doc);
        assert_eq!(predicates(&want), ["http://example.org/web#name"], "{doc}");
        assert_ne!(waiting, want);
        assert_eq!(triples_of(&mut ws, doc), want, "{doc}");
    }
}

/// A prefix reparse reads D, rewritten to use a web context the
/// watcher has not reported yet; the context is fetched before the
/// reparse is merged, and D ends parsed with it.
#[test]
fn a_context_fetched_during_a_prefix_reparse_reaches_the_reparsed_document() {
    let root = TempDir::new("c1-fetch-reparse");
    write(&root.0, "a.jsonld", &named_in("a"));
    let path = write(&root.0, "d.jsonld", &using("a.jsonld"));
    let d = uri(&path);
    let (loader, arrived) = fetching(false);
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.set_context_loader(loader);
    ws.index_paths(vec![path.clone()]);
    let jobs = ws.prefix_jobs(vec![path.clone()]);
    std::fs::write(&path, using(WEB_CONTEXT)).unwrap();
    let parsed = ws.reader().parse(jobs);
    arrived.store(true, std::sync::atomic::Ordering::SeqCst);
    ws.context_fetched(WEB_CONTEXT);
    ws.add_reparsed(parsed);

    let mut fresh = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    fresh.set_context_loader(fetching(true).0);
    fresh.index_paths(vec![path]);
    let want = triples_of(&mut fresh, &d);
    assert_eq!(predicates(&want), ["http://example.org/web#name"]);
    assert_eq!(triples_of(&mut ws, &d), want);
}
