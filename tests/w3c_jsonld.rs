//! Runs the vendored W3C JSON-LD 1.1 toRdf, expand and remote-doc suites
//! (tests/w3c/jsonld) through `jsonld::parse`, offline: a loader serves the
//! suite's files under their `https://w3c.github.io/json-ld-api/tests/` URLs.
//!
//! What each kind of test checks:
//! - toRdf evaluation: no error, and the quads are isomorphic to the
//!   expected N-Quads;
//! - toRdf syntax (`nt01`..`nt16`, inputs already in expanded form): no
//!   error, and the quads match the input read by the expanded-form oracle
//!   below;
//! - expand and remote-doc evaluation: the engine has no expanded-JSON
//!   output, only RDF, so the input's quads must match the expected expanded
//!   document read by the expanded-form oracle. Expansion that changes the
//!   RDF fails; differences RDF cannot carry (`@index`, free-floating values)
//!   are not checked;
//! - negative tests: an error whose message starts with the test's
//!   `expectErrorCode` (the engine's `jsonld-context` messages begin with
//!   the W3C error code). A test that fails with another error, or with
//!   the engine's own `json-syntax` errors, is listed `known-fail` with
//!   what it reports instead.
//!
//! The expanded-form oracle turns an expanded JSON-LD document into quads
//! with the JSON-LD 1.1 deserialization rules (no rdfDirection, no
//! generalized RDF). It is independent of the engine, and
//! `the_expanded_form_oracle_agrees_with_the_tordf_suite` checks it against
//! the toRdf results of the expand tests that have a toRdf twin.
//!
//! Every test has an expected status in tests/w3c/jsonld/expected-status.tsv:
//! pass, known-fail or out-of-scope, with a reason. The run fails when a pass
//! regresses, a known-fail starts passing, or the list and the suites
//! disagree. `JSONLD_W3C_WRITE=1` rewrites the list from the actual results.

use oxrdf::dataset::CanonicalizationAlgorithm;
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{BlankNode, Dataset, GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, Term};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use turtle_lsp::jsonld::{self, ContextLoader, LoadError, RemoteDocument};
use turtle_lsp::syntax::{self, Node, Severity};

const BASE: &str = "https://w3c.github.io/json-ld-api/tests/";
const SUITES: [&str; 3] = ["toRdf", "expand", "remote-doc"];
const WRITE_VAR: &str = "JSONLD_W3C_WRITE";

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/jsonld")
}

fn status_file() -> PathBuf {
    suite_dir().join("expected-status.tsv")
}

/// The vendored file behind a suite URL, if the URL is in the suite.
fn local(url: &str) -> Option<PathBuf> {
    let rel = url.strip_prefix(BASE)?;
    let rel = rel.split(['#', '?']).next().unwrap_or(rel);
    Some(suite_dir().join(rel))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    PositiveEval,
    PositiveSyntax,
    Negative,
}

struct Test {
    /// `toRdf#t0001`.
    id: String,
    suite: &'static str,
    kind: Kind,
    /// Absolute URLs.
    input: String,
    expect: Option<String>,
    error_code: Option<String>,
    options: Map<String, Value>,
}

impl Test {
    fn option_str(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }
}

fn read_manifest(suite: &'static str) -> Vec<Test> {
    let path = suite_dir().join(format!("{suite}-manifest.jsonld"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let m: Value = serde_json::from_str(&text).unwrap();
    let mut tests = Vec::new();
    for t in m["sequence"].as_array().expect("sequence") {
        let types: Vec<&str> = t["@type"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let kind = if types.contains(&"jld:PositiveEvaluationTest") {
            Kind::PositiveEval
        } else if types.contains(&"jld:PositiveSyntaxTest") {
            Kind::PositiveSyntax
        } else if types.contains(&"jld:NegativeEvaluationTest") {
            Kind::Negative
        } else {
            panic!("unknown test type {types:?}")
        };
        let url = |k: &str| t[k].as_str().map(|p| format!("{BASE}{p}"));
        tests.push(Test {
            id: format!("{suite}{}", t["@id"].as_str().unwrap()),
            suite,
            kind,
            input: url("input").expect("input"),
            expect: url("expect"),
            error_code: t["expectErrorCode"].as_str().map(str::to_string),
            options: t["option"].as_object().cloned().unwrap_or_default(),
        });
    }
    tests
}

/// Why a test is outside what the engine does, if it is.
fn out_of_scope(t: &Test) -> Option<&'static str> {
    let o = |k: &str| t.options.contains_key(k);
    if t.option_str("specVersion") == Some("json-ld-1.0") {
        return Some("JSON-LD 1.0 only (specVersion json-ld-1.0)");
    }
    if t.option_str("processingMode") == Some("json-ld-1.0") {
        return Some("processingMode json-ld-1.0: the engine is JSON-LD 1.1 only");
    }
    if o("expandContext") {
        return Some("expandContext: the engine takes no initial context");
    }
    if o("produceGeneralizedRdf") {
        return Some("produceGeneralizedRdf: the parse model holds plain RDF only");
    }
    if o("rdfDirection") {
        return Some("rdfDirection: the engine has no rdfDirection option");
    }
    if t.suite == "remote-doc" && t.error_code.as_deref() == Some("loading document failed") {
        return Some("loading the input document is the caller's job, not the engine's");
    }
    if o("processorFeature") || t.input.ends_with(".html") {
        return Some("HTML input and script extraction");
    }
    if o("httpLink") {
        return Some(
            "HTTP Link headers belong to document loading; the engine takes text and a base",
        );
    }
    if t.option_str("contentType")
        .is_some_and(|c| !c.ends_with("json"))
    {
        return Some("content-type checks belong to document loading");
    }
    None
}

/// Serves the suite's files under their URLs, following the test's
/// `redirectTo` the way an HTTP loader would: the document's URL becomes the
/// redirect target.
struct SuiteLoader {
    redirect: Option<(String, String)>,
}

impl ContextLoader for SuiteLoader {
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        let url = match &self.redirect {
            Some((from, to)) if from == url => to.as_str(),
            _ => url,
        };
        let Some(path) = local(url) else {
            return Err(if url.starts_with("http") {
                LoadError::NetworkNotAllowed
            } else {
                LoadError::NotFound
            });
        };
        match fs::read_to_string(&path) {
            Ok(text) => Ok(RemoteDocument {
                url: url.to_string(),
                text: Arc::from(text),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(LoadError::NotFound),
            Err(e) => Err(LoadError::Failed(e.to_string())),
        }
    }
}

fn nn(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

/// Language tags compare case-insensitively.
fn norm_term(t: Term) -> Term {
    match t {
        Term::Literal(l) => match (l.language(), l.direction()) {
            (Some(lang), Some(dir)) => Literal::new_directional_language_tagged_literal_unchecked(
                l.value(),
                lang.to_ascii_lowercase(),
                dir,
            )
            .into(),
            (Some(lang), None) => {
                Literal::new_language_tagged_literal_unchecked(l.value(), lang.to_ascii_lowercase())
                    .into()
            }
            _ => Term::Literal(l),
        },
        t => t,
    }
}

fn canonical(quads: impl IntoIterator<Item = Quad>) -> Dataset {
    let mut d = Dataset::new();
    for q in quads {
        d.insert(&Quad::new(
            q.subject,
            q.predicate,
            norm_term(q.object),
            q.graph_name,
        ));
    }
    d.canonicalize(CanonicalizationAlgorithm::Unstable);
    d
}

fn blank(b: syntax::BlankId) -> BlankNode {
    BlankNode::new_unchecked(format!("b{}", b.0))
}

fn term(n: &Node) -> Term {
    match n {
        Node::Iri(i) => nn(i).into(),
        Node::Blank(b) => blank(*b).into(),
        Node::Literal {
            value,
            datatype,
            lang,
        } => match lang {
            Some(l) => match l.split_once("--") {
                Some((l, dir)) => Literal::new_directional_language_tagged_literal_unchecked(
                    &**value,
                    l,
                    match dir.to_ascii_lowercase().as_str() {
                        "rtl" => oxrdf::BaseDirection::Rtl,
                        _ => oxrdf::BaseDirection::Ltr,
                    },
                ),
                None => Literal::new_language_tagged_literal_unchecked(&**value, &**l),
            }
            .into(),
            None => Literal::new_typed_literal(&**value, nn(datatype)).into(),
        },
        // JSON-LD has no triple terms.
        Node::Triple(_) => nn("urn:unexpected-triple-term").into(),
    }
}

fn ours(parsed: &syntax::Parsed) -> Result<Dataset, String> {
    let mut quads = Vec::new();
    for t in &parsed.triples {
        let s: NamedOrBlankNode = match term(&t.s) {
            Term::NamedNode(n) => n.into(),
            Term::BlankNode(b) => b.into(),
            other => return Err(format!("{other} as a subject")),
        };
        let g = match t.graph {
            None => GraphName::DefaultGraph,
            Some(syntax::GraphName::Iri(i)) => nn(&i).into(),
            Some(syntax::GraphName::Blank(b)) => blank(b).into(),
        };
        quads.push(Quad::new(s, nn(&t.p), term(&t.o), g));
    }
    Ok(canonical(quads))
}

fn nquads(path: &Path) -> Dataset {
    let src = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    canonical(
        oxttl::NQuadsParser::new()
            .for_slice(&src)
            .map(|q| q.unwrap_or_else(|e| panic!("{}: {e}", path.display()))),
    )
}

fn json_file(url: &str) -> Value {
    let path = local(url).expect("URL in the suite");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The quads an expanded JSON-LD document denotes (JSON-LD 1.1 node map
/// generation and deserialization to RDF, with rdfDirection null and no
/// generalized RDF): relative IRIs, blank-node predicates and datatypes, and
/// ill-formed language tags drop the statements they are in.
struct Oracle {
    quads: Vec<Quad>,
    labels: HashMap<String, usize>,
    fresh: usize,
}

/// A subject, object or graph name; `None` when it is not well-formed.
type Id = Option<Term>;

impl Oracle {
    fn read(doc: &Value) -> Dataset {
        let mut o = Oracle {
            quads: Vec::new(),
            labels: HashMap::new(),
            fresh: 0,
        };
        let items = match doc {
            Value::Array(a) => a.as_slice(),
            other => std::slice::from_ref(other),
        };
        for item in items {
            if let Some(obj) = item.as_object()
                && !obj.contains_key("@value")
                && !obj.contains_key("@list")
            {
                o.node(obj, &Some(GraphName::DefaultGraph));
            }
        }
        canonical(o.quads)
    }

    fn fresh(&mut self) -> Term {
        self.fresh += 1;
        BlankNode::new_unchecked(format!("f{}", self.fresh)).into()
    }

    fn id(&mut self, s: Option<&str>) -> Id {
        match s {
            None => Some(self.fresh()),
            Some(s) => {
                if let Some(label) = s.strip_prefix("_:") {
                    let n = self.labels.len();
                    let n = *self.labels.entry(label.to_string()).or_insert(n);
                    Some(BlankNode::new_unchecked(format!("l{n}")).into())
                } else {
                    iri(s).map(Term::from)
                }
            }
        }
    }

    fn emit(&mut self, s: &Id, p: &Option<NamedNode>, o: &Id, g: &Option<GraphName>) {
        let (Some(s), Some(p), Some(o), Some(g)) = (s, p, o, g) else {
            return;
        };
        let s = match s {
            Term::NamedNode(n) => NamedOrBlankNode::from(n.clone()),
            Term::BlankNode(b) => b.clone().into(),
            _ => return,
        };
        self.quads
            .push(Quad::new(s, p.clone(), o.clone(), g.clone()));
    }

    /// A node object (or graph object): its statements, and its id.
    fn node(&mut self, obj: &Map<String, Value>, g: &Option<GraphName>) -> Id {
        let id = self.id(obj.get("@id").and_then(Value::as_str));
        let type_p = Some(rdf::TYPE.into_owned());
        for t in array(obj.get("@type")) {
            let t = self.id(t.as_str());
            self.emit(&id, &type_p, &t, g);
        }
        for (k, v) in obj {
            if k.starts_with('@') {
                continue;
            }
            let p = iri(k);
            for item in array(Some(v)) {
                let o = self.object(item, g);
                self.emit(&id, &p, &o, g);
            }
        }
        if let Some(Value::Object(rev)) = obj.get("@reverse") {
            for (k, v) in rev {
                let p = iri(k);
                for item in array(Some(v)) {
                    if let Some(n) = item.as_object() {
                        let s = self.node(n, g);
                        self.emit(&s, &p, &id, g);
                    }
                }
            }
        }
        for item in array(obj.get("@included")) {
            if let Some(n) = item.as_object() {
                self.node(n, g);
            }
        }
        if obj.contains_key("@graph") {
            let inner = match &id {
                Some(Term::NamedNode(n)) => Some(GraphName::from(n.clone())),
                Some(Term::BlankNode(b)) => Some(b.clone().into()),
                _ => None,
            };
            for item in array(obj.get("@graph")) {
                if let Some(n) = item.as_object()
                    && !n.contains_key("@value")
                    && !n.contains_key("@list")
                {
                    self.node(n, &inner);
                }
            }
        }
        id
    }

    fn object(&mut self, v: &Value, g: &Option<GraphName>) -> Id {
        let obj = v.as_object()?;
        if obj.contains_key("@value") {
            return literal(obj).map(Term::from);
        }
        if let Some(items) = obj.get("@list") {
            return self.list(array(Some(items)), g);
        }
        self.node(obj, g)
    }

    fn list(&mut self, items: &[Value], g: &Option<GraphName>) -> Id {
        let nil: Term = rdf::NIL.into_owned().into();
        let Some(_) = items.first() else {
            return Some(nil);
        };
        let nodes: Vec<Id> = items.iter().map(|_| Some(self.fresh())).collect();
        let first = Some(rdf::FIRST.into_owned());
        let rest = Some(rdf::REST.into_owned());
        for (i, item) in items.iter().enumerate() {
            let o = self.object(item, g);
            self.emit(&nodes[i], &first, &o, g);
            let next = nodes.get(i + 1).cloned().unwrap_or(Some(nil.clone()));
            self.emit(&nodes[i], &rest, &next, g);
        }
        nodes[0].clone()
    }
}

fn array(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a,
        Some(v) => std::slice::from_ref(v),
        None => &[],
    }
}

/// An absolute, well-formed IRI.
fn iri(s: &str) -> Option<NamedNode> {
    oxiri::Iri::parse(s).ok().map(|_| nn(s))
}

fn literal(obj: &Map<String, Value>) -> Option<Literal> {
    let value = &obj["@value"];
    let ty = obj.get("@type").and_then(Value::as_str);
    if ty == Some("@json") {
        let mut lex = String::new();
        jcs(value, &mut lex);
        return Some(Literal::new_typed_literal(lex, rdf::JSON));
    }
    let datatype = match ty {
        Some(t) => Some(iri(t)?),
        None => None,
    };
    if let Some(lang) = obj.get("@language").and_then(Value::as_str) {
        oxilangtag::LanguageTag::parse(lang).ok()?;
        return Some(Literal::new_language_tagged_literal_unchecked(
            value.as_str()?,
            lang,
        ));
    }
    let (lex, default) = match value {
        Value::Bool(b) => (b.to_string(), xsd::BOOLEAN),
        Value::String(s) => (s.clone(), xsd::STRING),
        Value::Number(n) => {
            let f = n.as_f64()?;
            let is_double = datatype.as_ref().is_some_and(|d| d.as_ref() == xsd::DOUBLE);
            if is_double || f.fract() != 0.0 || f.abs() >= 1e21 {
                (xsd_double(f), xsd::DOUBLE)
            } else {
                (format!("{}", f as i128), xsd::INTEGER)
            }
        }
        _ => return None,
    };
    Some(Literal::new_typed_literal(
        lex,
        datatype.unwrap_or_else(|| default.into_owned()),
    ))
}

/// The canonical xsd:double form JSON-LD uses: `5.3E0`, `1.0E21`.
fn xsd_double(f: f64) -> String {
    let s = format!("{f:E}");
    let (mantissa, exp) = s.split_once('E').unwrap();
    if mantissa.contains('.') {
        s
    } else {
        format!("{mantissa}.0E{exp}")
    }
}

/// RFC 8785 (JCS) serialization, for `@json` literals.
fn jcs(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&es_number(n.as_f64().unwrap())),
        Value::String(s) => jcs_string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs_string(k, out);
                out.push(':');
                jcs(&o[k], out);
            }
            out.push('}');
        }
    }
}

fn jcs_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// ECMAScript `Number.prototype.toString`, which JCS uses for numbers.
fn es_number(f: f64) -> String {
    if f == 0.0 {
        return "0".into();
    }
    let sign = if f < 0.0 { "-" } else { "" };
    let s = format!("{:e}", f.abs());
    let (mantissa, exp) = s.split_once('e').unwrap();
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap() + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e = if e >= 0 {
            format!("+{e}")
        } else {
            e.to_string()
        };
        if k == 1 {
            format!("{digits}e{e}")
        } else {
            format!("{}.{}e{e}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

fn first_lines(d: &Dataset) -> String {
    d.iter()
        .take(6)
        .map(|q| format!("    {q}\n"))
        .collect::<String>()
}

/// A negative test passes when one of the `errors` (messages) starts with
/// the expected W3C error code.
fn negative(errors: &[&str], code: Option<&str>) -> Result<(), String> {
    match (errors.first(), code) {
        (None, _) => Err(format!(
            "no error reported (expected {})",
            code.unwrap_or("an error")
        )),
        (Some(_), None) => Ok(()),
        (Some(_), Some(code)) if errors.iter().any(|m| m.starts_with(code)) => Ok(()),
        (Some(_), Some(code)) => Err(format!(
            "expected error {code}, reported: {}",
            errors
                .iter()
                .take(3)
                .copied()
                .collect::<Vec<_>>()
                .join(" | ")
        )),
    }
}

/// `Ok` if the test passes, else why not.
fn run(t: &Test) -> Result<(), String> {
    let loader = SuiteLoader {
        redirect: t
            .option_str("redirectTo")
            .map(|to| (t.input.clone(), format!("{BASE}{to}"))),
    };
    let doc = loader
        .load(&t.input)
        .map_err(|e| format!("loading the input: {e:?}"))?;
    let base = t.option_str("base").unwrap_or(&doc.url);
    let parsed = jsonld::parse(&doc.text, Some(base), &loader);
    let error = parsed.errors.iter().find(|e| e.severity == Severity::Error);
    if t.kind == Kind::Negative {
        let errors: Vec<&str> = parsed
            .errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .map(|e| e.message.as_str())
            .collect();
        return negative(&errors, t.error_code.as_deref());
    }
    if let Some(e) = error {
        return Err(format!(
            "error at {}..{}: {}",
            e.span.start, e.span.end, e.message
        ));
    }
    let want = match (t.suite, t.kind, &t.expect) {
        ("toRdf", Kind::PositiveEval, Some(nq)) => nquads(&local(nq).unwrap()),
        ("toRdf", Kind::PositiveSyntax, _) => Oracle::read(&json_file(&t.input)),
        (_, _, Some(expanded)) => Oracle::read(&json_file(expanded)),
        _ => panic!("{}: nothing to compare with", t.id),
    };
    let got = ours(&parsed)?;
    if got != want {
        return Err(format!(
            "{} quads, expected {}\n  got:\n{}  want:\n{}",
            got.len(),
            want.len(),
            first_lines(&got),
            first_lines(&want)
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Pass,
    KnownFail,
    OutOfScope,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::KnownFail => "known-fail",
            Status::OutOfScope => "out-of-scope",
        }
    }
}

struct Entry {
    status: Status,
    reason: String,
}

fn read_status() -> BTreeMap<String, Entry> {
    let text = fs::read_to_string(status_file()).unwrap_or_default();
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut cols = line.splitn(3, '\t');
        let id = cols.next().unwrap().to_string();
        let status = match cols.next() {
            Some("pass") => Status::Pass,
            Some("known-fail") => Status::KnownFail,
            Some("out-of-scope") => Status::OutOfScope,
            other => panic!("expected-status.tsv:{}: bad status {other:?}", n + 1),
        };
        let reason = cols.next().unwrap_or("").to_string();
        out.insert(id, Entry { status, reason });
    }
    out
}

/// One line, at most 120 characters.
fn short(why: &str) -> String {
    let line = why.lines().next().unwrap_or("").trim();
    line.chars().take(120).collect()
}

#[test]
fn w3c_jsonld() {
    let write = std::env::var(WRITE_VAR).is_ok_and(|v| v == "1");
    let listed = read_status();
    let update = format!(
        "update tests/w3c/jsonld/expected-status.tsv (rerun with {WRITE_VAR}=1 to rewrite it from the actual results)"
    );
    let mut rows: Vec<(String, Status, String)> = Vec::new();
    let mut problems = Vec::new();
    let mut report = String::new();
    let mut total = 0;
    for suite in SUITES {
        let mut counts = [0usize; 3];
        let mut scope_reasons: BTreeMap<String, usize> = BTreeMap::new();
        for t in read_manifest(suite) {
            total += 1;
            let entry = listed.get(&t.id);
            let (status, reason) = if let Some(r) = out_of_scope(&t) {
                (Status::OutOfScope, r.to_string())
            } else if let Some(e) = entry.filter(|e| e.status == Status::OutOfScope) {
                // Listed out of scope by hand.
                (Status::OutOfScope, e.reason.clone())
            } else {
                match run(&t) {
                    Ok(()) => (Status::Pass, String::new()),
                    Err(why) => (Status::KnownFail, why),
                }
            };
            match entry {
                None => problems.push(format!("{}: not listed (it is {})", t.id, status.name())),
                Some(e) if e.status == status => {}
                Some(e) => problems.push(match (e.status, status) {
                    (Status::Pass, Status::KnownFail) => format!("{}: regressed: {reason}", t.id),
                    (Status::KnownFail, Status::Pass) => {
                        format!("{}: listed known-fail but now passes", t.id)
                    }
                    (listed, actual) => format!(
                        "{}: listed {} but is {} ({})",
                        t.id,
                        listed.name(),
                        actual.name(),
                        short(&reason)
                    ),
                }),
            }
            counts[status as usize] += 1;
            if status == Status::OutOfScope {
                *scope_reasons.entry(short(&reason)).or_default() += 1;
            }
            // A reason survives rewrites while the status holds, so hand-written
            // reasons are kept.
            let reason = match entry {
                Some(e) if e.status == status && !e.reason.is_empty() => e.reason.clone(),
                _ => short(&reason),
            };
            rows.push((t.id, status, reason));
        }
        let _ = writeln!(
            report,
            "W3C JSON-LD {suite}: {} pass, {} known-fail, {} out of scope",
            counts[0], counts[1], counts[2]
        );
        for (r, n) in scope_reasons {
            let _ = writeln!(report, "    out of scope {n}: {r}");
        }
    }
    for id in listed.keys() {
        if !rows.iter().any(|r| &r.0 == id) {
            problems.push(format!("{id}: listed but not in the suites"));
        }
    }
    std::io::stderr().write_all(report.as_bytes()).unwrap();
    assert!(total > 850, "only {total} tests found");
    if write {
        let mut out = String::from(
            "# W3C JSON-LD 1.1 expected statuses (tests/w3c_jsonld.rs).\n\
             # test<TAB>pass|known-fail|out-of-scope<TAB>reason\n\
             # Rewrite from the actual results: JSONLD_W3C_WRITE=1 cargo test --release --test w3c_jsonld\n",
        );
        for (id, status, reason) in &rows {
            let _ = write!(out, "{id}\t{}", status.name());
            if !reason.is_empty() {
                let _ = write!(out, "\t{reason}");
            }
            out.push('\n');
        }
        fs::write(status_file(), out).unwrap();
        let note = format!(
            "wrote {} ({} changes)\n",
            status_file().display(),
            problems.len()
        );
        std::io::stderr().write_all(note.as_bytes()).unwrap();
        return;
    }
    assert!(
        problems.is_empty(),
        "{} tests differ from their expected status; {update}:\n{}",
        problems.len(),
        problems
            .iter()
            .take(40)
            .map(|p| short(p))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The expanded-form oracle reads each expand test's expected output to the
/// same quads the toRdf suite expects from the same input.
#[test]
fn the_expanded_form_oracle_agrees_with_the_tordf_suite() {
    let to_rdf: Vec<Test> = read_manifest("toRdf")
        .into_iter()
        .filter(|t| t.kind == Kind::PositiveEval && out_of_scope(t).is_none())
        .collect();
    let mut by_input: HashMap<(Vec<u8>, String), &Test> = HashMap::new();
    for t in &to_rdf {
        let input = fs::read(local(&t.input).unwrap()).unwrap();
        let options = serde_json::to_string(&t.options).unwrap();
        by_input.insert((input, options), t);
    }
    let mut checked = 0;
    let mut differ = Vec::new();
    for e in read_manifest("expand") {
        if e.kind != Kind::PositiveEval || out_of_scope(&e).is_some() {
            continue;
        }
        let input = fs::read(local(&e.input).unwrap()).unwrap();
        let options = serde_json::to_string(&e.options).unwrap();
        let Some(t) = by_input.get(&(input, options)) else {
            continue;
        };
        checked += 1;
        // The twins sit in different folders, so IRIs resolved against the
        // input's URL differ only by it.
        let expanded = json_file(e.expect.as_ref().unwrap());
        let got = Oracle::read(&rebase(&expanded, &|s: &str| {
            s.replace(&e.input, &t.input)
                .replace(&format!("{BASE}expand/"), &format!("{BASE}toRdf/"))
        }));
        let want = nquads(&local(t.expect.as_ref().unwrap()).unwrap());
        if got != want {
            differ.push(format!(
                "{} vs {}: {} quads, expected {}\n  got:\n{}  want:\n{}",
                e.id,
                t.id,
                got.len(),
                want.len(),
                first_lines(&got),
                first_lines(&want)
            ));
        }
    }
    assert!(checked > 200, "only {checked} twins found");
    assert!(
        differ.is_empty(),
        "{} of {checked} differ:\n{}",
        differ.len(),
        differ.join("\n")
    );
}

/// `v` with `f` applied to every string and key.
fn rebase(v: &Value, f: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(f(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| rebase(x, f)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (f(k), rebase(x, f))).collect()),
        v => v.clone(),
    }
}

#[test]
fn numbers_serialize_as_json_ld_and_jcs_expect() {
    assert_eq!(xsd_double(5.3), "5.3E0");
    assert_eq!(xsd_double(1.0), "1.0E0");
    assert_eq!(xsd_double(1e21), "1.0E21");
    assert_eq!(xsd_double(123.45), "1.2345E2");
    let es: Vec<String> = [333333333.3333333, 1e30, 4.5, 0.002, 1e-27, -12.0, 1e21]
        .into_iter()
        .map(es_number)
        .collect();
    assert_eq!(
        es,
        [
            "333333333.3333333",
            "1e+30",
            "4.5",
            "0.002",
            "1e-27",
            "-12",
            "1e+21"
        ]
    );
}

/// A negative test compares the error code, not only that there is an error.
#[test]
fn a_negative_test_needs_its_error_code() {
    let reported = ["invalid scoped context: invalid IRI mapping", "json-ish"];
    assert_eq!(negative(&reported, Some("invalid scoped context")), Ok(()));
    assert_eq!(
        negative(&reported, Some("invalid IRI mapping")).unwrap_err(),
        "expected error invalid IRI mapping, reported: invalid scoped context: invalid IRI mapping | json-ish"
    );
    // Any of the errors may carry the code; the engine reports several.
    assert_eq!(negative(&reported, Some("json-ish")), Ok(()));
    assert!(
        negative(&[], Some("loading document failed"))
            .unwrap_err()
            .starts_with("no error reported")
    );
    // A test with no code only needs an error.
    assert_eq!(negative(&reported, None), Ok(()));
}
