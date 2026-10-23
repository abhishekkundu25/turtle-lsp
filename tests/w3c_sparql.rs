//! Runs the vendored W3C SPARQL 1.1 (query, update, federation) and SPARQL
//! 1.2 syntax suites against the SPARQL document layer: what the server shows
//! in a `.rq` / `.ru` file.
//!
//! - Positive syntax tests, and the queries and updates of evaluation tests:
//!   no error diagnostic, no error token, and the formatter's output parses
//!   to the same algebra as the input and formats to itself.
//! - Negative syntax tests: at least one error diagnostic.
//! - Every query and update in the suites and in the SPARQL examples of
//!   `test/corpus` that `spargebra` parses: our lexer and `spargebra` agree
//!   where every IRI and `<` operator starts and ends.
//!
//! `spargebra` passes these suites itself; what is tested is our layer on
//! top of it (lexer, structure, prefixes, error positions, formatter), and
//! that its verdict and ours never disagree.
//!
//! Every manifest is read on its own (its `mf:entries`, not its
//! `mf:include`s), so no test runs twice.

use oxrdf::vocab::rdf;
use oxrdf::{Graph, NamedNode, NamedOrBlankNode, Term, TermRef};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use turtle_lsp::features::format::Options;
use turtle_lsp::sparql::{self, Document, Level, TokenKind};
use turtle_lsp::syntax::PrefixMap;

include!("support/corpus.rs");

const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const QT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#";
const UT: &str = "http://www.w3.org/2009/sparql/tests/test-update#";

/// Known deviations: (test IRI, reason). A listed test that passes fails the
/// run, so the list cannot go stale.
///
/// `spargebra` itself disagrees with four tests. Three are negative tests it
/// accepts (a triple term or literal as the subject of a triple term in an
/// expression, an aggregate inside an aggregate); the document layer
/// rejects those itself. The fourth needs a change to the parser.
const ALLOWED: &[(&str, &str)] = &[(
    "https://w3c.github.io/rdf-tests/sparql/sparql12/grouping/manifest#select-variable-reuse",
    "spargebra 0.4 rejects a projected variable used in a later SELECT expression (proposed for SPARQL 1.2)",
)];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Positive,
    Negative,
}

struct Test {
    iri: String,
    kind: Kind,
    file: PathBuf,
}

fn suites_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/sparql-tests")
}

/// The manifests under `dir`, without the SPARQL 1.1 evaluation suites
/// (`sparql11/` directories not named `syntax-*`), which
/// tests/w3c_sparql_eval.rs runs.
fn manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let eval_suite = p.parent().is_some_and(|d| d.ends_with("sparql11"))
            && !p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("syntax-"));
        if p.is_dir() && eval_suite {
            continue;
        }
        if p.is_dir() {
            manifests(&p, out);
        } else if p.file_name().is_some_and(|n| n == "manifest.ttl") {
            out.push(p);
        }
    }
}

fn nn(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

fn local(url: &str) -> PathBuf {
    url::Url::parse(url)
        .unwrap()
        .to_file_path()
        .unwrap_or_else(|_| panic!("not a file URL: {url}"))
}

fn read_manifest(path: &Path) -> Vec<Test> {
    let base = file_url(path);
    let src = fs::read(path).unwrap();
    let mut g = Graph::new();
    for t in oxttl::TurtleParser::new()
        .with_base_iri(&base)
        .unwrap()
        .for_slice(&src)
    {
        g.insert(&t.unwrap());
    }
    let mf = |l: &str| nn(&format!("{MF}{l}"));
    let Some(manifest) = g
        .subjects_for_predicate_object(rdf::TYPE, &mf("Manifest"))
        .next()
        .map(|m| m.into_owned())
    else {
        return Vec::new();
    };
    let Some(mut list) = g
        .object_for_subject_predicate(&manifest, &mf("entries"))
        .map(|t| t.into_owned())
    else {
        return Vec::new();
    };
    let mut tests = Vec::new();
    while list != Term::NamedNode(rdf::NIL.into_owned()) {
        let node: NamedOrBlankNode = match &list {
            Term::NamedNode(n) => n.clone().into(),
            Term::BlankNode(b) => b.clone().into(),
            _ => panic!("bad list"),
        };
        let entry = g
            .object_for_subject_predicate(&node, rdf::FIRST)
            .unwrap()
            .into_owned();
        list = g
            .object_for_subject_predicate(&node, rdf::REST)
            .unwrap()
            .into_owned();
        let Term::NamedNode(entry) = entry else {
            panic!("entry is not an IRI")
        };
        let ty = match g.object_for_subject_predicate(&entry, rdf::TYPE) {
            Some(TermRef::NamedNode(n)) => n.as_str().to_string(),
            _ => panic!("{entry} has no type"),
        };
        let ty = ty.strip_prefix(MF).unwrap_or(&ty);
        let kind = match ty {
            "PositiveSyntaxTest"
            | "PositiveSyntaxTest11"
            | "PositiveUpdateSyntaxTest"
            | "PositiveUpdateSyntaxTest11"
            | "QueryEvaluationTest"
            | "UpdateEvaluationTest" => Kind::Positive,
            "NegativeSyntaxTest"
            | "NegativeSyntaxTest11"
            | "NegativeUpdateSyntaxTest"
            | "NegativeUpdateSyntaxTest11" => Kind::Negative,
            // Result-format, protocol and entailment tests have no query to check.
            _ => continue,
        };
        // Syntax tests name the file as the action; evaluation tests name it
        // with qt:query / ut:request on the action node.
        let action = g
            .object_for_subject_predicate(&entry, &mf("action"))
            .unwrap_or_else(|| panic!("{entry} has no mf:action"))
            .into_owned();
        let file = match &action {
            Term::NamedNode(n) => n.as_str().to_string(),
            Term::BlankNode(b) => {
                let q = [nn(&format!("{QT}query")), nn(&format!("{UT}request"))]
                    .into_iter()
                    .find_map(|p| g.object_for_subject_predicate(b, &p));
                match q {
                    Some(TermRef::NamedNode(n)) => n.as_str().to_string(),
                    _ => panic!("{entry}: no qt:query or ut:request"),
                }
            }
            t => panic!("{entry}: action {t}"),
        };
        tests.push(Test {
            iri: entry.as_str().to_string(),
            kind,
            file: local(&file),
        });
    }
    tests
}

/// The algebra as text with generated names renumbered in order of first
/// appearance: blank node labels (`[]` and `_:x` get fresh labels on every
/// parse) and the hex-named variables spargebra makes up for aggregates.
fn canonical(algebra: &str) -> String {
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = String::with_capacity(algebra.len());
    let mut rest = algebra;
    loop {
        let blank = rest.find("_:");
        // spargebra writes a random u128 in hex, without leading zeros.
        let hex_len = |name: &str| {
            let n = name.bytes().take_while(u8::is_ascii_hexdigit).count();
            let ends = !name[n..].starts_with(|c: char| c.is_alphanumeric() || c == '_');
            ((20..=32).contains(&n) && ends).then_some(n)
        };
        let var = rest
            .match_indices('?')
            .find_map(|(i, _)| hex_len(&rest[i + 1..]).map(|n| (i, n)));
        let (i, sigil, len) = match (blank, var) {
            (Some(b), Some((v, n))) if v < b => (v, "?", n),
            (Some(b), _) => {
                let tail = &rest[b + 2..];
                let len = tail
                    .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
                    .unwrap_or(tail.len());
                (b, "_:", len)
            }
            (None, Some((v, n))) => (v, "?", n),
            (None, None) => break,
        };
        out.push_str(&rest[..i]);
        let name = &rest[i + sigil.len()..i + sigil.len() + len];
        let n = ids.len();
        let id = *ids.entry(format!("{sigil}{name}")).or_insert(n);
        out.push_str(&format!("{sigil}g{id}"));
        rest = &rest[i + sigil.len() + len..];
    }
    out.push_str(rest);
    out
}

fn algebra(text: &str, base: &str, update: bool) -> Result<String, String> {
    let parser = spargebra::SparqlParser::new()
        .with_base_iri(base)
        .map_err(|e| e.to_string())?;
    let s = if update {
        parser.parse_update(text).map(|u| u.to_string())
    } else {
        parser.parse_query(text).map(|q| q.to_string())
    };
    s.map(|s| canonical(&s)).map_err(|e| e.to_string())
}

/// `Ok` when `spargebra` reads the IRIs and `<` operators where our lexer
/// does in `text`, whose algebra is `before`: a space at both ends of each
/// keeps the algebra (none of ours is part of a longer token to `spargebra`),
/// and each IRI replaced by another still parses (none of ours is operators).
fn same_boundaries(text: &str, base: &str, update: bool, before: &str) -> Result<(), String> {
    let (mut spaced, mut renamed, mut copied) = (String::new(), String::new(), 0);
    for (i, t) in sparql::lexer::tokenize(text).iter().enumerate() {
        let s = t.text(text);
        if t.kind != TokenKind::Iri && !(t.kind == TokenKind::Punct && s.starts_with('<')) {
            continue;
        }
        let gap = &text[copied..t.span.start as usize];
        spaced += &format!("{gap} {s} ");
        let swap = if t.kind == TokenKind::Iri {
            format!("<urn:tok:{i}>")
        } else {
            s.to_owned()
        };
        renamed += &format!("{gap} {swap} ");
        copied = t.span.end as usize;
    }
    spaced += &text[copied..];
    renamed += &text[copied..];
    match algebra(&spaced, base, update) {
        Ok(a) if a == before => {}
        other => return Err(format!("spaced around IRIs and `<`: {other:?}\n{spaced}")),
    }
    algebra(&renamed, base, update)
        .map(|_| ())
        .map_err(|e| format!("an IRI of ours is not one to spargebra: {e}\n{renamed}"))
}

#[test]
fn iri_and_less_than_boundaries_agree_with_spargebra() {
    let mut files = Vec::new();
    let mut walk = vec![suites_dir()];
    walk.extend(corpus_path("sib-sparql-examples/examples"));
    while let Some(dir) = walk.pop() {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk.push(p);
            } else {
                files.push(p);
            }
        }
    }
    // (text, base, update) of each query and update.
    let mut queries = vec![(
        "PREFIX ex: <http://e/> SELECT * { ?s ?p ?a FILTER(?a<=ex:b&&?c>?a) }".to_owned(),
        "http://e/".to_owned(),
        false,
    )];
    for p in &files {
        let base = file_url(p);
        match p.extension().and_then(|e| e.to_str()) {
            Some("rq") => queries.push((fs::read_to_string(p).unwrap(), base, false)),
            Some("ru") => queries.push((fs::read_to_string(p).unwrap(), base, true)),
            Some("ttl") if p.starts_with(suites_dir()) => {}
            Some("ttl") => {
                let src = fs::read(p).unwrap();
                for t in oxttl::TurtleParser::new().for_slice(&src).flatten() {
                    let key = t
                        .predicate
                        .as_str()
                        .strip_prefix("http://www.w3.org/ns/shacl#");
                    if let Term::Literal(l) = &t.object
                        && let Some(k @ ("select" | "ask" | "construct" | "describe" | "update")) =
                            key
                    {
                        queries.push((l.value().to_owned(), base.clone(), k == "update"));
                    }
                }
            }
            _ => {}
        }
    }
    let mut checked = 0;
    let mut failures = Vec::new();
    for (text, base, update) in &queries {
        let Ok(before) = algebra(text, base, *update) else {
            continue;
        };
        checked += 1;
        if let Err(e) = same_boundaries(text, base, *update, &before) {
            failures.push(e);
        }
    }
    eprintln!("IRI and `<` boundaries: {checked} queries checked");
    assert!(checked > 1000, "only {checked} queries parse");
    assert!(
        failures.is_empty(),
        "{} disagree:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// `Ok` if the test passes, else why not.
fn run(t: &Test) -> Result<(), String> {
    let text =
        fs::read_to_string(&t.file).map_err(|e| format!("read {}: {e}", t.file.display()))?;
    let base = file_url(&t.file);
    let doc = Document::parse(&text, &PrefixMap::default(), Some(&base));
    let errors: Vec<_> = doc
        .diagnostics()
        .iter()
        .filter(|d| d.level == Level::Error)
        .collect();
    match t.kind {
        Kind::Negative => {
            if errors.is_empty() {
                return Err("no error reported".into());
            }
            Ok(())
        }
        Kind::Positive => {
            if let Some(d) = errors.first() {
                return Err(format!(
                    "{} at {}..{}: {}",
                    d.code, d.span.start, d.span.end, d.message
                ));
            }
            if let Some(tok) = doc.tokens().iter().find(|t| t.kind == TokenKind::Error) {
                return Err(format!(
                    "error token at {}..{}",
                    tok.span.start, tok.span.end
                ));
            }
            let update = doc.is_update();
            let before = algebra(&text, &base, update)
                .map_err(|e| format!("spargebra rejects it ({e}), we do not"))?;
            let formatted = sparql::format(
                &text,
                &PrefixMap::default(),
                Some(&base),
                &Options::default(),
            )
            .map_err(|e| format!("not formatted: {e}"))?;
            let after = algebra(&formatted, &base, update)
                .map_err(|e| format!("formatted text does not parse: {e}\n{formatted}"))?;
            if after != before {
                return Err(format!(
                    "formatting changed the algebra\n  before: {before}\n  after:  {after}\n{formatted}"
                ));
            }
            let again = sparql::format(
                &formatted,
                &PrefixMap::default(),
                Some(&base),
                &Options::default(),
            )
            .map_err(|e| format!("formatted text not formatted again: {e}"))?;
            if again != formatted {
                return Err(format!(
                    "formatting is not idempotent\n{formatted}\n---\n{again}"
                ));
            }
            Ok(())
        }
    }
}

#[test]
fn w3c_sparql_suites() {
    let mut files = Vec::new();
    manifests(&suites_dir(), &mut files);
    files.sort();
    // Suite ("sparql11/syntax-query") -> (passed, total).
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut stale = Vec::new();
    // Panics are reported as failures with their message; keep them off stderr.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for file in &files {
        let rel = file.strip_prefix(suites_dir()).unwrap();
        let suite: Vec<_> = rel.iter().take(2).map(|c| c.to_string_lossy()).collect();
        let c = counts.entry(suite.join("/")).or_default();
        for t in read_manifest(file) {
            c.1 += 1;
            let allowed = ALLOWED.iter().find(|(iri, _)| *iri == t.iri);
            let result = std::panic::catch_unwind(|| run(&t)).unwrap_or_else(|p| {
                Err(p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .map_or("panicked".into(), |m| format!("panicked: {m}")))
            });
            match (result, allowed) {
                (Ok(()), None) => c.0 += 1,
                (Ok(()), Some(_)) => stale.push(t.iri.clone()),
                (Err(_), Some(_)) => {}
                (Err(e), None) => failures.push(format!("{} ({:?}): {e}", t.iri, t.kind)),
            }
        }
    }
    std::panic::set_hook(hook);
    let (mut passed, mut total) = (0, 0);
    let mut report = String::new();
    for (suite, (p, n)) in &counts {
        report += &format!("W3C {suite}: {p}/{n} pass\n");
        passed += p;
        total += n;
    }
    report += &format!(
        "W3C SPARQL total: {passed}/{total} pass ({} allowed deviations)\n",
        ALLOWED.len()
    );
    std::io::stderr().write_all(report.as_bytes()).unwrap();
    assert!(total > 400, "only {total} tests found");
    assert!(
        stale.is_empty(),
        "allow-listed tests now pass; remove them from ALLOWED:\n{}",
        stale.join("\n")
    );
    // The first failures in full, the rest by name.
    let shown: Vec<String> = failures
        .iter()
        .enumerate()
        .map(|(i, f)| {
            if i < 20 {
                f.clone()
            } else {
                f.lines().next().unwrap_or("").to_string()
            }
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        shown.join("\n")
    );
}

/// The harness's own check: the golden formatter fixtures differ only in
/// layout, so their algebras compare equal once generated names are renumbered.
#[test]
fn canonical_algebra_ignores_generated_names() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/format");
    for (messy, expected, update) in [
        ("messy.rq", "expected.rq", false),
        ("messy.ru", "expected.ru", true),
    ] {
        let read = |f: &str| fs::read_to_string(dir.join(f)).unwrap();
        let a = algebra(&read(messy), "http://example.com/", update).unwrap();
        let b = algebra(&read(expected), "http://example.com/", update).unwrap();
        assert_eq!(a, b, "{messy} and {expected}");
        assert!(!a.contains("_:b0"), "blank labels not renumbered");
    }
    assert_ne!(
        algebra("SELECT * { ?s ?p ?o }", "http://example.com/", false).unwrap(),
        algebra("SELECT * { ?s ?p ?x }", "http://example.com/", false).unwrap()
    );
}

// Golden record of what spargebra makes of real queries, so a change to the
// vendored grammar (vendor/spargebra) that rejects a query that parsed, or
// changes its algebra, shows as a difference. The record is
// tests/sparql_golden.txt: one `path<TAB>algebra` line per `.rq` file in the
// W3C suites and test/corpus that parses, sorted by path, with generated
// names renumbered by `canonical` and `\` and line breaks escaped. Each
// query is parsed with the base `http://example.org/base/<path>`. To accept
// a deliberate change, rerun with UPDATE_GOLDEN=1 and review the data diff.

const GOLDEN: &str = "tests/sparql_golden.txt";

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn query_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries {
        let p = e.unwrap().path();
        if p.is_dir() {
            query_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rq") {
            out.push(p);
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

fn current() -> String {
    let mut files = Vec::new();
    query_files(&repo().join("tests/w3c/sparql-tests"), &mut files);
    query_files(&repo().join("test/corpus"), &mut files);
    let mut lines = BTreeMap::new();
    for f in files {
        let rel = f
            .strip_prefix(repo())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(&f) else {
            continue;
        };
        let parsed = spargebra::SparqlParser::new()
            .with_base_iri(format!("http://example.org/base/{rel}"))
            .unwrap()
            .parse_query(&text);
        if let Ok(q) = parsed {
            lines.insert(rel, escape(&canonical(&q.to_string())));
        }
    }
    let mut out = String::new();
    for (path, algebra) in lines {
        out.push_str(&format!("{path}\t{algebra}\n"));
    }
    out
}

#[test]
fn parsed_queries_match_the_golden_record() {
    let now = current();
    let path = repo().join(GOLDEN);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::write(&path, &now).unwrap();
        return;
    }
    let had = fs::read_to_string(&path).unwrap_or_default();
    // test/corpus is not in a fresh checkout: its entries are compared only
    // when it is present.
    let corpus = repo().join("test/corpus").is_dir();
    let keep = |l: &&str| corpus || !l.starts_with("test/corpus/");
    let map = |s: &str| -> BTreeMap<String, String> {
        s.lines()
            .filter(keep)
            .filter_map(|l| l.split_once('\t'))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    let (want, got) = (map(&had), map(&now));
    let mut diffs = Vec::new();
    for (k, v) in &want {
        match got.get(k) {
            None => diffs.push(format!("{k}: parsed before, no longer parses")),
            Some(g) if g != v => {
                diffs.push(format!("{k}: algebra changed\n  was: {v}\n  now: {g}"))
            }
            _ => {}
        }
    }
    for k in got.keys().filter(|k| !want.contains_key(*k)) {
        diffs.push(format!("{k}: parses now, not in the record"));
    }
    assert!(
        diffs.is_empty(),
        "{} differences from {GOLDEN} (UPDATE_GOLDEN=1 accepts them):\n{}",
        diffs.len(),
        diffs
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(want.len() > 300, "only {} queries recorded", want.len());
}
