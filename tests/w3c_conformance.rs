//! Runs the vendored W3C Turtle, TriG, N-Triples and N-Quads suites (RDF 1.1
//! and 1.2)
//! against the parser. Each test file is analyzed as the server analyzes an
//! open document and diagnosed by the server's own composer
//! ([`diagnostics::diagnostics`]): positive syntax tests must get no error
//! diagnostic, negative ones at least one, and evaluation tests must give a
//! dataset isomorphic to the expected N-Triples / N-Quads.
//!
//! Every manifest is read on its own (its `mf:entries`, not its
//! `mf:include`s, which are manifests of their own), so no test runs twice.

use lsp_types::DiagnosticSeverity;
use oxrdf::vocab::rdf;
use oxrdf::{Dataset, Graph, NamedNode, NamedOrBlankNode, Term, TermRef};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use turtle_lsp::analysis::{Analysis, Encoding};
use turtle_lsp::features::diagnostics;
use turtle_lsp::knowledge::Knowledge;
use turtle_lsp::syntax::Syntax;

#[path = "support/oracle.rs"]
mod oracle;

const BASE: &str = "https://w3c.github.io/rdf-tests/rdf/";
const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const RDFT: &str = "http://www.w3.org/ns/rdftest#";

/// Known deviations: (test IRI, reason). A listed test that passes fails the
/// run, so the list cannot go stale.
const ALLOWED: &[(&str, &str)] = &[];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Positive,
    Negative,
    Eval,
}

struct Test {
    iri: String,
    kind: Kind,
    syntax: Syntax,
    action: String,
    result: Option<String>,
}

fn suites_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/rdf-tests")
}

/// The local file behind a test-suite URL.
fn local(url: &str) -> PathBuf {
    suites_dir().join(
        url.strip_prefix(BASE)
            .expect("test URL under the suite base"),
    )
}

fn manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
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

fn read_manifest(path: &Path) -> Vec<Test> {
    let rel = path.strip_prefix(suites_dir()).unwrap();
    let url = format!("{BASE}{}", rel.to_string_lossy().replace('\\', "/"));
    let src = fs::read(path).unwrap();
    let mut g = Graph::new();
    for t in oxttl::TurtleParser::new()
        .with_base_iri(&url)
        .unwrap()
        .for_slice(&src)
    {
        g.insert(&t.unwrap());
    }
    let mf = |l: &str| nn(&format!("{MF}{l}"));
    let manifest = g
        .subjects_for_predicate_object(rdf::TYPE, &mf("Manifest"))
        .next()
        .expect("a mf:Manifest")
        .into_owned();
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
        let iri = |p: &str| match g.object_for_subject_predicate(&entry, &mf(p)) {
            Some(TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
            _ => None,
        };
        let ty = match g.object_for_subject_predicate(&entry, rdf::TYPE) {
            Some(TermRef::NamedNode(n)) => n.as_str().to_string(),
            _ => panic!("{entry} has no type"),
        };
        let ty = ty.strip_prefix(RDFT).expect("an rdft: test type");
        let (syntax, rest) = if let Some(r) = ty.strip_prefix("TestTurtle") {
            (Syntax::Turtle, r)
        } else if let Some(r) = ty.strip_prefix("TestTrig") {
            (Syntax::TriG, r)
        } else if let Some(r) = ty.strip_prefix("TestNTriples") {
            (Syntax::NTriples, r)
        } else if let Some(r) = ty.strip_prefix("TestNQuads") {
            (Syntax::NQuads, r)
        } else {
            panic!("unknown test type {ty}")
        };
        let kind = match rest {
            "PositiveSyntax" => Kind::Positive,
            "NegativeSyntax" | "NegativeEval" => Kind::Negative,
            // `PositiveC14N`: the canonical form is the expected result.
            "Eval" | "PositiveC14N" => Kind::Eval,
            _ => panic!("unknown test type {ty}"),
        };
        tests.push(Test {
            iri: entry.as_str().to_string(),
            kind,
            syntax,
            action: iri("action").expect("mf:action"),
            result: iri("result"),
        });
    }
    tests
}

fn expected(path: &Path) -> Dataset {
    let src = fs::read(path).unwrap();
    oracle::canonical(
        oxttl::NQuadsParser::new()
            .for_slice(&src)
            .map(|q| q.unwrap_or_else(|e| panic!("{}: {e}", path.display()))),
    )
}

fn first_lines(d: &Dataset) -> String {
    d.iter()
        .take(8)
        .map(|q| format!("    {q}\n"))
        .collect::<String>()
}

/// `Ok` if the test passes, else why not.
fn run(t: &Test) -> Result<(), String> {
    let path = local(&t.action);
    let src = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    // Parsed against the test's URL, as an open document is against its URI.
    let analysis = Analysis::with_syntax(t.action.clone(), &src, Arc::default(), false, t.syntax);
    let parsed = &analysis.parsed;
    let errors: Vec<_> = diagnostics::diagnostics(&analysis, &Knowledge::default(), Encoding::Utf8)
        .into_iter()
        .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
        .map(|d| (d.range, d.message))
        .collect();
    match t.kind {
        Kind::Negative => {
            if errors.is_empty() {
                return Err("no error reported".into());
            }
        }
        Kind::Positive | Kind::Eval => {
            if let Some((range, message)) = errors.first() {
                return Err(format!(
                    "error at {}:{}: {message}",
                    range.start.line + 1,
                    range.start.character + 1
                ));
            }
        }
    }
    if t.kind == Kind::Eval {
        let result = t.result.as_ref().ok_or("eval test without mf:result")?;
        let want = expected(&local(result));
        let got = oracle::ours(&parsed.triples);
        if got != want {
            return Err(format!(
                "{} quads, expected {}\n  got:\n{}  want:\n{}",
                got.len(),
                want.len(),
                first_lines(&got),
                first_lines(&want)
            ));
        }
    }
    Ok(())
}

#[test]
fn w3c_suites() {
    let mut files = Vec::new();
    manifests(&suites_dir(), &mut files);
    files.sort();
    // Suite ("rdf11/rdf-turtle") -> (passed, total).
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut stale = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(suites_dir()).unwrap();
        let suite: Vec<_> = rel.iter().take(2).map(|c| c.to_string_lossy()).collect();
        let c = counts.entry(suite.join("/")).or_default();
        for t in read_manifest(file) {
            c.1 += 1;
            let allowed = ALLOWED.iter().find(|(iri, _)| *iri == t.iri);
            match (run(&t), allowed) {
                (Ok(()), None) => c.0 += 1,
                (Ok(()), Some(_)) => stale.push(t.iri.clone()),
                (Err(_), Some(_)) => {}
                (Err(e), None) => failures.push(format!("{} ({:?}): {e}", t.iri, t.kind)),
            }
        }
    }
    let (mut passed, mut total) = (0, 0);
    let mut report = String::new();
    for (suite, (p, n)) in &counts {
        report += &format!("W3C {suite}: {p}/{n} pass\n");
        passed += p;
        total += n;
    }
    report += &format!(
        "W3C total: {passed}/{total} pass ({} allowed deviations)\n",
        ALLOWED.len()
    );
    // Straight to stderr: the test harness captures `eprintln!`.
    std::io::stderr().write_all(report.as_bytes()).unwrap();
    assert!(total > 1100, "only {total} tests found");
    assert!(
        stale.is_empty(),
        "allow-listed tests now pass; remove them from ALLOWED:\n{}",
        stale.join("\n")
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
