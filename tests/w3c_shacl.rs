//! Runs the vendored W3C SHACL Core and SHACL-SPARQL test suites
//! (tests/w3c/shacl/core, tests/w3c/shacl/sparql) against the validator.
//! Each `sht:Validate` test passes when `sh:conforms` matches and the results
//! match the expected `sh:ValidationReport` as a multiset of (focus node,
//! path, value, source shape, source constraint, severity, component), plus
//! `sh:resultMessage` where the expected report gives one. Blank nodes match
//! any blank node; paths compare by structure. A test whose expected result
//! is `sht:Failure` passes when validation reports a failure.

use std::fs;
use std::path::{Path, PathBuf};
use turtle_lsp::rdf::graph::{Graph, Id, Term};
use turtle_lsp::shacl::validate::parse_path;
use turtle_lsp::shacl::{self, validate_report};
use turtle_lsp::syntax::{Iri, Parsed, PrefixMap, RDF, Syntax, XSD, parse_with_base};

const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const SHT: &str = "http://www.w3.org/ns/shacl-test#";

/// One vendored suite: its directory under tests/w3c/shacl, the base its
/// files are loaded under, and its known deviations as (test IRI, reason).
/// A listed test that passes fails the run, so the list cannot go stale.
struct Suite {
    name: &'static str,
    base: &'static str,
    allowed: &'static [(&'static str, &'static str)],
}

const CORE: Suite = Suite {
    name: "core",
    base: "http://shacl.test/core/",
    allowed: &[],
};

const SPARQL: Suite = Suite {
    name: "sparql",
    base: "http://shacl.test/sparql/",
    allowed: &[],
};

impl Suite {
    fn dir(&self) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/w3c/shacl")
            .join(self.name)
    }
}

fn files(d: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(d).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|x| x == "ttl") {
            out.push(p);
        }
    }
}

fn url_of(suite: &Suite, path: &Path) -> String {
    let rel = path.strip_prefix(suite.dir()).unwrap();
    format!("{}{}", suite.base, rel.to_string_lossy().replace('\\', "/"))
}

fn load(suite: &Suite, url: &str) -> Parsed {
    let path = suite.dir().join(
        url.strip_prefix(suite.base)
            .expect("URL under the suite base"),
    );
    let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let p = parse_with_base(&src, &PrefixMap::default(), Syntax::Turtle, Some(url));
    assert!(p.errors.is_empty(), "{url}: {:?}", p.errors);
    p
}

fn iri(s: &str) -> Iri {
    Iri::intern(s)
}

fn sh(l: &str) -> Iri {
    iri(&format!("{}{l}", shacl::SH))
}

fn term(g: &Graph, id: Id) -> String {
    match g.term(id) {
        Term::Iri(i) => format!("<{}>", i.as_str()),
        Term::Blank(_) => "_".into(),
        t => match t.literal() {
            Some((lex, dt, lang)) => format!(
                "{lex:?}^^<{}>@{}",
                dt.as_str(),
                lang.map_or("", |l| l.as_str())
            ),
            None => "<<triple>>".into(),
        },
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct Key {
    focus: String,
    path: String,
    value: String,
    shape: String,
    constraint: String,
    severity: String,
    component: String,
    messages: Vec<String>,
}

struct Case {
    iri: String,
    data: String,
    shapes: String,
    conforms: bool,
    /// The expected result is `sht:Failure`.
    failure: bool,
    expected: Vec<Key>,
    with_messages: bool,
}

fn cases(suite: &Suite, url: &str) -> Vec<Case> {
    let parsed = load(suite, url);
    let g = Graph::new(&[&parsed.triples]);
    let mf = |l: &str| iri(&format!("{MF}{l}"));
    let sht = |l: &str| iri(&format!("{SHT}{l}"));
    let rdf_type = iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    let mut out = Vec::new();
    for t in g.with_p(mf("entries")).collect::<Vec<_>>() {
        for entry in g.list(t.o) {
            if !g
                .objects(entry, rdf_type)
                .any(|c| g.as_iri(c) == Some(sht("Validate")))
            {
                continue;
            }
            let action = g.object(entry, mf("action")).unwrap();
            let graph_url = |p: &str| match g.term(g.object(action, sht(p)).unwrap()) {
                Term::Iri(i) => i.as_str().to_string(),
                _ => panic!("graph is not an IRI"),
            };
            let report = g.object(entry, mf("result")).unwrap();
            let failure = g.as_iri(report) == Some(sht("Failure"));
            let conforms = g
                .object(report, sh("conforms"))
                .and_then(|c| g.term(c).literal())
                .is_some_and(|l| l.0 == "true");
            let opt = |r: Id, p: &str| g.object(r, sh(p)).map_or(String::new(), |x| term(&g, x));
            let mut with_messages = false;
            let expected: Vec<Key> = g
                .objects(report, sh("result"))
                .map(|r| {
                    let mut messages: Vec<String> = g
                        .objects(r, sh("resultMessage"))
                        .map(|m| term(&g, m))
                        .collect();
                    messages.sort();
                    with_messages |= !messages.is_empty();
                    Key {
                        focus: opt(r, "focusNode"),
                        path: g
                            .object(r, sh("resultPath"))
                            .map_or(String::new(), |p| format!("{:?}", parse_path(&g, p))),
                        value: opt(r, "value"),
                        shape: opt(r, "sourceShape"),
                        constraint: opt(r, "sourceConstraint"),
                        severity: opt(r, "resultSeverity"),
                        component: opt(r, "sourceConstraintComponent"),
                        messages,
                    }
                })
                .collect();
            out.push(Case {
                iri: term(&g, entry),
                data: graph_url("dataGraph"),
                shapes: graph_url("shapesGraph"),
                conforms,
                failure,
                expected,
                with_messages,
            });
        }
    }
    out
}

/// None when the test passes, else what differed.
fn run(suite: &Suite, case: &mut Case) -> Option<String> {
    let data = load(suite, &case.data);
    let shapes_doc;
    let docs: Vec<&[turtle_lsp::syntax::Triple]> = if case.shapes == case.data {
        vec![&data.triples]
    } else {
        shapes_doc = load(suite, &case.shapes);
        vec![&data.triples, &shapes_doc.triples]
    };
    let g = Graph::new(&docs);
    let report = validate_report(&g, &|| false, &|_| true).expect("not cancelled");
    if case.failure {
        return report
            .failures
            .is_empty()
            .then(|| format!("expected a failure, got {} results", report.results.len()));
    }
    if !report.failures.is_empty() {
        return Some(format!("unexpected failures: {:?}", report.failures));
    }
    let results = report.results;
    let mut actual: Vec<Key> = results
        .iter()
        .map(|r| {
            let mut messages: Vec<String> = if !case.with_messages {
                Vec::new()
            } else if r.computed_messages.is_empty() {
                r.messages.iter().map(|&m| term(&g, m)).collect()
            } else {
                r.computed_messages
                    .iter()
                    .map(|(text, lang)| match lang {
                        Some(l) => format!("{text:?}^^<{RDF}langString>@{}", l.as_str()),
                        None => format!("{text:?}^^<{XSD}string>@"),
                    })
                    .collect()
            };
            messages.sort();
            Key {
                focus: term(&g, r.focus),
                path: r.path.as_ref().map_or(String::new(), |p| format!("{p:?}")),
                value: r.value.map_or(String::new(), |v| term(&g, v)),
                shape: term(&g, r.shape),
                constraint: r.constraint.map_or(String::new(), |c| term(&g, c)),
                severity: format!("<{}>", r.severity.as_str()),
                component: format!("<{}>", r.component.as_str()),
                messages,
            }
        })
        .collect();
    actual.sort();
    case.expected.sort();
    let conforms = results.is_empty();
    if conforms != case.conforms || actual != case.expected {
        let missing: Vec<_> = case
            .expected
            .iter()
            .filter(|k| !actual.contains(k))
            .collect();
        let extra: Vec<_> = actual
            .iter()
            .filter(|k| !case.expected.contains(k))
            .collect();
        return Some(format!(
            "conforms {conforms} (expected {}), {} results (expected {})\n  missing: {missing:#?}\n  extra: {extra:#?}",
            case.conforms,
            actual.len(),
            case.expected.len()
        ));
    }
    None
}

/// Runs every test of `suite`; returns (passed, total).
fn run_suite(suite: &Suite) -> (usize, usize) {
    let mut all = Vec::new();
    files(&suite.dir(), &mut all);
    all.sort();
    let mut total = 0;
    let mut passed = 0;
    let mut failures = Vec::new();
    let mut allowed_passing = Vec::new();
    for f in &all {
        for mut case in cases(suite, &url_of(suite, f)) {
            total += 1;
            let failure = run(suite, &mut case);
            let allowed = suite.allowed.iter().any(|(t, _)| case.iri.contains(t));
            match (failure, allowed) {
                (None, false) => passed += 1,
                (None, true) => allowed_passing.push(case.iri.clone()),
                (Some(_), true) => {}
                (Some(why), false) => failures.push(format!("{}: {why}", case.iri)),
            }
        }
    }
    println!(
        "W3C SHACL {}: {passed}/{total} pass, {} allowed failures",
        suite.name,
        suite.allowed.len()
    );
    assert!(
        allowed_passing.is_empty(),
        "allow-listed tests now pass: {allowed_passing:?}"
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    (passed, total)
}

#[test]
fn w3c_shacl_core() {
    let (_, total) = run_suite(&CORE);
    assert!(total >= 98, "only {total} tests found");
}

#[test]
fn w3c_shacl_sparql() {
    let (_, total) = run_suite(&SPARQL);
    assert!(total >= 23, "only {total} tests found");
}
