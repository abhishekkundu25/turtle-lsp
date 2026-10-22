//! Runs the vendored W3C SPARQL 1.1 query evaluation tests
//! (tests/w3c/sparql-tests/sparql11, the directories in [`DIRS`]) through
//! [`turtle_lsp::query::run`], the engine behind `turtle-lsp query` and "Run
//! query", and compares the answers with the expected results.
//!
//! - In scope: `mf:QueryEvaluationTest`s over a default graph only. The
//!   `qt:data` files are the documents the query runs over (their union is
//!   the default graph, as in the editor). A test with `qt:graphData` (named
//!   graphs) or a query with `FROM` / `FROM NAMED` is out of scope: `run` has
//!   no named graphs and ignores `FROM`.
//! - SELECT results (`.srx`, `.srj`) compare as RDF graphs, one blank node
//!   per row with its bindings, checked for isomorphism with oxrdf: the row
//!   order does not count and blank nodes match any consistent renaming.
//!   With a top-level `ORDER BY`, the leading plain-variable keys must also
//!   come in the expected order. ASK compares the boolean; CONSTRUCT compares
//!   the graph with the expected Turtle, isomorphic.
//!
//! Every test has a status in [`STATUS`]: `pass`, `known-fail` or
//! `out-of-scope`, with a reason. The run fails when a pass regresses, a
//! known failure starts passing, or the list and the suites disagree. To
//! rewrite the list from the actual results (reasons of known failures that
//! still fail are kept):
//!
//!     SPARQL_W3C_WRITE=1 cargo test --release --test w3c_sparql_eval
//!
//! The files load under the URL the manifests use
//! (`http://www.w3.org/2009/sparql/docs/tests/data-sparql11/<dir>/<file>`),
//! so relative IRIs resolve as the test authors meant.

use oxrdf::dataset::CanonicalizationAlgorithm;
use oxrdf::vocab::{rdf, xsd};
use oxrdf::{BlankNode, Graph, Literal, NamedNode, NamedOrBlankNode, Term, TermRef, Triple};
use spargebra::algebra::{Expression, GraphPattern, OrderExpression};
use spargebra::{Query, SparqlParser};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use turtle_lsp::analysis::Analysis;
use turtle_lsp::query::{self, Answer, Limits};
use turtle_lsp::syntax::PrefixMap;

/// The vendored evaluation suites, under tests/w3c/sparql-tests/sparql11.
const DIRS: [&str; 9] = [
    "aggregates",
    "bind",
    "construct",
    "exists",
    "functions",
    "grouping",
    "negation",
    "property-path",
    "subquery",
];

/// The status list, relative to the crate root.
const STATUS: &str = "tests/w3c/sparql-tests/sparql11/eval-status.tsv";

/// Where the manifests say the files are.
const BASE: &str = "http://www.w3.org/2009/sparql/docs/tests/data-sparql11/";

const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const QT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#";
const RS: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#";

fn suites_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/sparql-tests/sparql11")
}

fn nn(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

struct Test {
    iri: String,
    dir: &'static str,
    query: String,
    data: Vec<String>,
    graph_data: bool,
    result: Option<String>,
}

impl Test {
    /// The local file behind a URL under [`BASE`].
    fn local(&self, url: &str) -> PathBuf {
        suites_dir().join(url.strip_prefix(BASE).expect("a URL under the suite base"))
    }
}

/// The `mf:QueryEvaluationTest`s among the manifest's `mf:entries`, in order.
fn read_manifest(dir: &'static str) -> Vec<Test> {
    let url = format!("{BASE}{dir}/manifest.ttl");
    let src = fs::read(suites_dir().join(dir).join("manifest.ttl")).unwrap();
    let mut g = Graph::new();
    for t in oxttl::TurtleParser::new()
        .with_base_iri(&url)
        .unwrap()
        .for_slice(&src)
    {
        g.insert(&t.unwrap());
    }
    let mf = |l: &str| nn(&format!("{MF}{l}"));
    let qt = |l: &str| nn(&format!("{QT}{l}"));
    let manifest = g
        .subjects_for_predicate_object(rdf::TYPE, &mf("Manifest"))
        .next()
        .expect("a mf:Manifest")
        .into_owned();
    let mut list = g
        .object_for_subject_predicate(&manifest, &mf("entries"))
        .expect("mf:entries")
        .into_owned();
    let iri_of = |t: TermRef| match t {
        TermRef::NamedNode(n) => n.as_str().to_string(),
        t => panic!("{t} is not an IRI"),
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
        if !g.contains(oxrdf::TripleRef::new(
            &entry,
            rdf::TYPE,
            &mf("QueryEvaluationTest"),
        )) {
            // The syntax tests among the entries are tests/w3c_sparql.rs's.
            continue;
        }
        let action: NamedOrBlankNode = match g
            .object_for_subject_predicate(&entry, &mf("action"))
            .unwrap_or_else(|| panic!("{entry} has no mf:action"))
        {
            TermRef::BlankNode(b) => b.into_owned().into(),
            TermRef::NamedNode(n) => n.into_owned().into(),
            t => panic!("{entry}: action {t}"),
        };
        let query = iri_of(
            g.object_for_subject_predicate(&action, &qt("query"))
                .unwrap_or_else(|| panic!("{entry} has no qt:query")),
        );
        let data = g
            .objects_for_subject_predicate(&action, &qt("data"))
            .map(iri_of)
            .collect();
        let graph_data = g
            .objects_for_subject_predicate(&action, &qt("graphData"))
            .next()
            .is_some();
        let result = g
            .object_for_subject_predicate(&entry, &mf("result"))
            .map(iri_of);
        tests.push(Test {
            iri: entry.as_str().to_string(),
            dir,
            query,
            data,
            graph_data,
            result,
        });
    }
    tests
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail(String),
    OutOfScope(String),
}

impl Outcome {
    fn status(&self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail(_) => "known-fail",
            Outcome::OutOfScope(_) => "out-of-scope",
        }
    }
}

/// The parsed query, for its form, dataset clause and order.
fn algebra(text: &str, base: &str) -> Result<Query, String> {
    SparqlParser::new()
        .with_base_iri(base)
        .map_err(|e| e.to_string())?
        .parse_query(text)
        .map_err(|e| e.to_string())
}

fn run(t: &Test) -> Outcome {
    if t.graph_data {
        return Outcome::OutOfScope("named graphs (qt:graphData)".into());
    }
    if t.data.iter().any(|d| d.ends_with(".rdf")) {
        return Outcome::OutOfScope("RDF/XML data (not vendored: query::run takes Turtle, TriG, N-Triples, N-Quads and JSON-LD documents)".into());
    }
    let text = match fs::read_to_string(t.local(&t.query)) {
        Ok(s) => s,
        Err(e) => return Outcome::Fail(format!("read {}: {e}", t.query)),
    };
    let parsed = match algebra(&text, &t.query) {
        Ok(q) => q,
        Err(e) => return Outcome::Fail(format!("spargebra rejects the query: {e}")),
    };
    if parsed.dataset().is_some() {
        return Outcome::OutOfScope("FROM / FROM NAMED (query::run ignores FROM)".into());
    }
    let Some(result) = &t.result else {
        return Outcome::Fail("no mf:result".into());
    };
    let docs: Vec<Analysis> = match t
        .data
        .iter()
        .map(|url| {
            fs::read_to_string(t.local(url))
                .map(|src| Analysis::new(url.clone(), &src, Arc::new(PrefixMap::default()), false))
                .map_err(|e| format!("read {url}: {e}"))
        })
        .collect()
    {
        Ok(d) => d,
        Err(e) => return Outcome::Fail(e),
    };
    if let Some(e) = docs.iter().find_map(|a| a.parsed.errors.first()) {
        return Outcome::Fail(format!("the data does not parse: {}", e.message));
    }
    let expected = match expected(&t.local(result), result) {
        Ok(e) => e,
        Err(e) => return Outcome::Fail(format!("expected results: {e}")),
    };
    let refs: Vec<&Analysis> = docs.iter().collect();
    let limits = Limits {
        timeout: Some(Duration::from_secs(10)),
        ..Limits::default()
    };
    let answer = std::panic::catch_unwind(|| {
        query::run(&refs, &text, Some(&t.query), &PrefixMap::default(), &limits)
    });
    let answer = match answer {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => return Outcome::Fail(format!("query::run: {e}")),
        Err(p) => {
            let m = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            return Outcome::Fail(format!("query::run panicked: {m}"));
        }
    };
    match compare(&parsed, &answer, &expected) {
        Ok(()) => Outcome::Pass,
        Err(e) => Outcome::Fail(e),
    }
}

// ---- expected results ------------------------------------------------------

/// A row: (variable, value) for each bound variable.
type Row = Vec<(String, Term)>;

enum Expected {
    Solutions {
        variables: Vec<String>,
        rows: Vec<Row>,
    },
    Boolean(bool),
    Graph(Box<Graph>),
}

fn expected(path: &Path, url: &str) -> Result<Expected, String> {
    let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("srx") => srx(&src),
        Some("srj") => srj(&src),
        Some("ttl") => {
            let mut g = Graph::new();
            for t in oxttl::TurtleParser::new()
                .with_base_iri(url)
                .map_err(|e| e.to_string())?
                .for_slice(&src)
            {
                g.insert(&t.map_err(|e| e.to_string())?);
            }
            if g.subjects_for_predicate_object(rdf::TYPE, &nn(&format!("{RS}ResultSet")))
                .next()
                .is_some()
            {
                return Err("result sets in RDF are not read".into());
            }
            Ok(Expected::Graph(Box::new(g)))
        }
        e => Err(format!("unknown result format {e:?}")),
    }
}

/// A literal from the result formats: language tags lower-cased, as
/// [`normalize`] does to the answers.
fn literal(value: String, lang: Option<&str>, datatype: Option<&str>) -> Term {
    match (lang, datatype) {
        (Some(l), _) => {
            Literal::new_language_tagged_literal_unchecked(value, l.to_ascii_lowercase())
        }
        (None, Some(dt)) => Literal::new_typed_literal(value, nn(dt)),
        (None, None) => Literal::new_simple_literal(value),
    }
    .into()
}

/// SPARQL JSON results.
fn srj(src: &str) -> Result<Expected, String> {
    let v: serde_json::Value = serde_json::from_str(src).map_err(|e| e.to_string())?;
    if let Some(b) = v["boolean"].as_bool() {
        return Ok(Expected::Boolean(b));
    }
    let variables = v["head"]["vars"]
        .as_array()
        .ok_or("no head.vars")?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_string)
                .ok_or("a variable is not a string")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut rows = Vec::new();
    for b in v["results"]["bindings"]
        .as_array()
        .ok_or("no results.bindings")?
    {
        let mut row = Row::new();
        for (var, t) in b.as_object().ok_or("a binding is not an object")? {
            let value = t["value"]
                .as_str()
                .ok_or("a value is not a string")?
                .to_string();
            let term = match t["type"].as_str() {
                Some("uri") => nn(&value).into(),
                Some("bnode") => BlankNode::new_unchecked(value).into(),
                Some("literal" | "typed-literal") => {
                    literal(value, t["xml:lang"].as_str(), t["datatype"].as_str())
                }
                other => return Err(format!("unknown term type {other:?}")),
            };
            row.push((var.clone(), term));
        }
        rows.push(row);
    }
    Ok(Expected::Solutions { variables, rows })
}

/// What [`xml`] reads.
#[derive(Debug, PartialEq)]
enum Xml {
    Open {
        name: String,
        attrs: Vec<(String, String)>,
        empty: bool,
    },
    Close(String),
    Text(String),
}

/// The elements and text of an XML document: enough XML for the result
/// format (no DTD). Element names lose their namespace prefix; attribute
/// names keep theirs (`xml:lang`).
fn xml(src: &str) -> Result<Vec<Xml>, String> {
    let mut out = Vec::new();
    let mut rest = src;
    let local = |n: &str| n.rsplit(':').next().unwrap_or(n).to_string();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("<?") {
            rest = &r[r.find("?>").ok_or("unclosed <?")? + 2..];
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = &r[r.find("-->").ok_or("unclosed comment")? + 3..];
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").ok_or("unclosed CDATA")?;
            out.push(Xml::Text(r[..end].to_string()));
            rest = &r[end + 3..];
        } else if rest.starts_with("<!") {
            return Err("a DTD is not read".into());
        } else if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>').ok_or("unclosed end tag")?;
            out.push(Xml::Close(local(r[..end].trim())));
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix('<') {
            let end = tag_end(r).ok_or("unclosed tag")?;
            let (body, empty) = match r[..end].strip_suffix('/') {
                Some(b) => (b, true),
                None => (&r[..end], false),
            };
            let name_end = body
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(body.len());
            let mut attrs = Vec::new();
            let mut a = body[name_end..].trim_start();
            while !a.is_empty() {
                let eq = a.find('=').ok_or("attribute without a value")?;
                let key = a[..eq].trim().to_string();
                let v = a[eq + 1..].trim_start();
                let quote = v.chars().next().ok_or("attribute without a value")?;
                if quote != '"' && quote != '\'' {
                    return Err("unquoted attribute".into());
                }
                let close = v[1..].find(quote).ok_or("unclosed attribute")? + 1;
                attrs.push((key, unescape(&v[1..close])?));
                a = v[close + 1..].trim_start();
            }
            out.push(Xml::Open {
                name: local(&body[..name_end]),
                attrs,
                empty,
            });
            rest = &r[end + 1..];
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            out.push(Xml::Text(unescape(&rest[..end])?));
            rest = &rest[end..];
        }
    }
    Ok(out)
}

/// The `>` that ends a start tag (not one inside a quoted attribute).
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let end = rest[i..].find(';').ok_or("unclosed entity")? + i;
        let name = &rest[i + 1..end];
        let c = match name {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(h) = name.strip_prefix("#x") {
                    u32::from_str_radix(h, 16)
                } else if let Some(d) = name.strip_prefix('#') {
                    d.parse()
                } else {
                    return Err(format!("unknown entity &{name};"));
                };
                code.ok()
                    .and_then(char::from_u32)
                    .ok_or(format!("bad character reference &{name};"))?
            }
        };
        out.push(c);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// SPARQL XML results.
fn srx(src: &str) -> Result<Expected, String> {
    let events = xml(src)?;
    let mut variables = Vec::new();
    let mut rows = Vec::new();
    let mut row: Option<Row> = None;
    let mut var: Option<String> = None;
    let mut i = 0;
    // The text up to the end of the element opened at `events[i]`.
    let text_of = |i: &mut usize| -> Result<String, String> {
        let Xml::Open { name, empty, .. } = &events[*i] else {
            unreachable!()
        };
        *i += 1;
        if *empty {
            return Ok(String::new());
        }
        let mut s = String::new();
        loop {
            match events.get(*i) {
                Some(Xml::Text(t)) => s.push_str(t),
                Some(Xml::Close(n)) if n == name => return Ok(s),
                other => return Err(format!("unexpected {other:?} in <{name}>")),
            }
            *i += 1;
        }
    };
    while i < events.len() {
        let Xml::Open { name, attrs, .. } = &events[i] else {
            if let Xml::Close(n) = &events[i] {
                match n.as_str() {
                    "result" => rows.push(row.take().ok_or("</result> without <result>")?),
                    "binding" => var = None,
                    _ => {}
                }
            }
            i += 1;
            continue;
        };
        let attr = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        match name.as_str() {
            "variable" => variables.push(attr("name").ok_or("<variable> without a name")?),
            "boolean" => {
                return match text_of(&mut i)?.trim() {
                    "true" => Ok(Expected::Boolean(true)),
                    "false" => Ok(Expected::Boolean(false)),
                    b => Err(format!("boolean {b:?}")),
                };
            }
            "result" => row = Some(Row::new()),
            "binding" => var = Some(attr("name").ok_or("<binding> without a name")?),
            "uri" | "bnode" | "literal" => {
                let (lang, datatype) = (attr("xml:lang"), attr("datatype"));
                let kind = name.clone();
                let value = text_of(&mut i)?;
                let term = match kind.as_str() {
                    "uri" => nn(value.trim()).into(),
                    "bnode" => BlankNode::new_unchecked(value.trim()).into(),
                    _ => literal(value, lang.as_deref(), datatype.as_deref()),
                };
                let v = var.clone().ok_or("a value outside <binding>")?;
                row.as_mut()
                    .ok_or("a binding outside <result>")?
                    .push((v, term));
                continue;
            }
            "triple" => return Err("triple terms in results are not read".into()),
            _ => {}
        }
        i += 1;
    }
    Ok(Expected::Solutions { variables, rows })
}

// ---- comparison ------------------------------------------------------------

/// Language tags lower-cased and numbers in one lexical form per value
/// ([`numeric`]), inside triple terms too: the result formats write a
/// computed value in any lexical form (`3.21E4` for the double 32100).
fn normalize(t: &Term) -> Term {
    match t {
        Term::Literal(l) => match (l.language(), numeric(l.value(), l.datatype().as_str())) {
            (Some(lang), _) => {
                Literal::new_language_tagged_literal_unchecked(l.value(), lang.to_ascii_lowercase())
                    .into()
            }
            (None, Some(v)) => Literal::new_typed_literal(v, l.datatype().into_owned()).into(),
            (None, None) => t.clone(),
        },
        Term::Triple(tr) => Term::Triple(Box::new(Triple::new(
            tr.subject.clone(),
            tr.predicate.clone(),
            normalize(&tr.object),
        ))),
        _ => t.clone(),
    }
}

/// One lexical form per value of an `xsd:integer`, `xsd:decimal`,
/// `xsd:double` or `xsd:float` literal (the datatype stays: `1` and `1.0`
/// of different types still differ); `None` for other literals and for a
/// lexical form that is not a number.
fn numeric(value: &str, datatype: &str) -> Option<String> {
    let local = datatype.strip_prefix("http://www.w3.org/2001/XMLSchema#")?;
    let v = value.trim();
    match local {
        "integer" => v.parse::<i128>().ok().map(|i| i.to_string()),
        "decimal" => {
            let (neg, digits) = match v.strip_prefix('-') {
                Some(d) => (true, d),
                None => (false, v.strip_prefix('+').unwrap_or(v)),
            };
            let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
            if int.is_empty() && frac.is_empty()
                || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let int = int.trim_start_matches('0');
            let frac = frac.trim_end_matches('0');
            let zero = int.is_empty() && frac.is_empty();
            Some(format!(
                "{}{}.{}",
                if neg && !zero { "-" } else { "" },
                if int.is_empty() { "0" } else { int },
                if frac.is_empty() { "0" } else { frac }
            ))
        }
        "double" | "float" => match v {
            "INF" | "+INF" => Some("INF".into()),
            "-INF" => Some("-INF".into()),
            "NaN" => Some("NaN".into()),
            _ => v.parse::<f64>().ok().map(|f| format!("{f:e}")),
        },
        _ => None,
    }
}

/// The rows as an RDF graph: a blank node per row with an `rs:binding` per
/// bound variable (with `rs:index` when `indexed`), canonicalized.
fn rows_graph<'a>(
    rows: impl Iterator<Item = &'a Row>,
    keep: &dyn Fn(&str) -> bool,
    indexed: bool,
) -> Graph {
    let rs = |l: &str| nn(&format!("{RS}{l}"));
    let mut g = Graph::new();
    for (i, row) in rows.enumerate() {
        let r = BlankNode::default();
        if indexed {
            g.insert(&Triple::new(
                r.clone(),
                rs("index"),
                Literal::new_typed_literal(i.to_string(), xsd::INTEGER),
            ));
        }
        g.insert(&Triple::new(r.clone(), rdf::TYPE, rs("Solution")));
        for (var, value) in row.iter().filter(|(v, _)| keep(v)) {
            let b = BlankNode::default();
            g.insert(&Triple::new(r.clone(), rs("binding"), b.clone()));
            g.insert(&Triple::new(
                b.clone(),
                rs("variable"),
                Literal::new_simple_literal(var),
            ));
            g.insert(&Triple::new(b, rs("value"), normalize(value)));
        }
    }
    g.canonicalize(CanonicalizationAlgorithm::Unstable);
    g
}

/// The leading `ORDER BY` keys of a SELECT that are plain variables: in any
/// correct answer their values come in the same order.
fn order_keys(q: &Query) -> Vec<String> {
    let Query::Select { pattern, .. } = q else {
        return Vec::new();
    };
    let mut p = pattern;
    loop {
        match p {
            GraphPattern::Slice { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Project { inner, .. } => p = inner,
            GraphPattern::OrderBy { expression, .. } => {
                return expression
                    .iter()
                    .map_while(|e| match e {
                        OrderExpression::Asc(Expression::Variable(v))
                        | OrderExpression::Desc(Expression::Variable(v)) => {
                            Some(v.as_str().to_string())
                        }
                        _ => None,
                    })
                    .collect();
            }
            _ => return Vec::new(),
        }
    }
}

fn show_rows(rows: &[Row]) -> String {
    let mut s = String::new();
    for r in rows.iter().take(6) {
        let cells: Vec<String> = r.iter().map(|(v, t)| format!("?{v}={t}")).collect();
        writeln!(s, "    {}", cells.join(" ")).unwrap();
    }
    if rows.len() > 6 {
        writeln!(s, "    ... {} rows", rows.len()).unwrap();
    }
    s
}

fn compare(q: &Query, answer: &Answer, expected: &Expected) -> Result<(), String> {
    match (answer, expected) {
        (Answer::Boolean(a), Expected::Boolean(e)) => {
            if a != e {
                return Err(format!("ASK gave {a}, expected {e}"));
            }
            Ok(())
        }
        (Answer::Solutions(s), Expected::Solutions { variables, rows }) => {
            let mut got_vars = s.variables.clone();
            let mut want_vars = variables.clone();
            got_vars.sort();
            want_vars.sort();
            if got_vars != want_vars {
                return Err(format!("variables {got_vars:?}, expected {want_vars:?}"));
            }
            let got: Vec<Row> = s
                .rows
                .iter()
                .map(|r| {
                    s.variables
                        .iter()
                        .zip(r)
                        .filter_map(|(v, t)| Some((v.clone(), t.clone()?)))
                        .collect()
                })
                .collect();
            let all = |_: &str| true;
            if rows_graph(got.iter(), &all, false) != rows_graph(rows.iter(), &all, false) {
                return Err(format!(
                    "{} rows, expected {}\n  got:\n{}  expected:\n{}",
                    got.len(),
                    rows.len(),
                    show_rows(&got),
                    show_rows(rows)
                ));
            }
            let keys = order_keys(q);
            if !keys.is_empty() {
                let key = |v: &str| keys.iter().any(|k| k == v);
                if rows_graph(got.iter(), &key, true) != rows_graph(rows.iter(), &key, true) {
                    return Err(format!(
                        "the rows are not in ORDER BY order ({keys:?})\n  got:\n{}  expected:\n{}",
                        show_rows(&got),
                        show_rows(rows)
                    ));
                }
            }
            Ok(())
        }
        (Answer::Triples(ts), Expected::Graph(want)) => {
            let mut got = Graph::new();
            for t in ts.iter() {
                got.insert(&Triple::new(
                    t.subject.clone(),
                    t.predicate.clone(),
                    normalize(&t.object),
                ));
            }
            let mut want_n = Graph::new();
            for t in want.iter() {
                want_n.insert(&Triple::new(
                    t.subject.into_owned(),
                    t.predicate.into_owned(),
                    normalize(&t.object.into_owned()),
                ));
            }
            got.canonicalize(CanonicalizationAlgorithm::Unstable);
            want_n.canonicalize(CanonicalizationAlgorithm::Unstable);
            if got != want_n {
                let mut d = String::new();
                for t in want_n.iter().filter(|t| !got.contains(*t)).take(4) {
                    writeln!(d, "    missing: {t}").unwrap();
                }
                for t in got.iter().filter(|t| !want_n.contains(*t)).take(4) {
                    writeln!(d, "    extra:   {t}").unwrap();
                }
                return Err(format!(
                    "{} triples, expected {} (not isomorphic)\n{d}",
                    got.len(),
                    want_n.len()
                ));
            }
            Ok(())
        }
        (a, _) => Err(format!(
            "the answer is of another form: {}",
            match a {
                Answer::Boolean(_) => "a boolean",
                Answer::Solutions(_) => "solutions",
                Answer::Triples(_) => "a graph",
            }
        )),
    }
}

// ---- the status list -------------------------------------------------------

/// Test IRI -> (status, reason), in file order.
fn read_status(path: &Path) -> Vec<(String, String, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut cols = l.splitn(3, '\t');
            let iri = cols.next().unwrap_or_default().to_string();
            let status = cols.next().unwrap_or_default().to_string();
            let reason = cols.next().unwrap_or_default().to_string();
            (iri, status, reason)
        })
        .collect()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

#[test]
fn w3c_sparql_eval() {
    let status_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(STATUS);
    let listed = read_status(&status_path);
    let by_iri: BTreeMap<&str, (&str, &str)> = listed
        .iter()
        .map(|(i, s, r)| (i.as_str(), (s.as_str(), r.as_str())))
        .collect();
    // Dir -> (pass, known-fail, out-of-scope).
    let mut counts: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    let mut problems = Vec::new();
    let mut seen = Vec::new();
    let mut written = String::from(
        "# W3C SPARQL 1.1 query evaluation tests through query::run (tests/w3c_sparql_eval.rs).\n\
         # test IRI\tstatus (pass, known-fail, out-of-scope)\treason\n\
         # Rewrite from the actual results: SPARQL_W3C_WRITE=1 cargo test --release --test w3c_sparql_eval\n",
    );
    for dir in DIRS {
        let c = counts.entry(dir).or_default();
        for t in read_manifest(dir) {
            let outcome = run(&t);
            match &outcome {
                Outcome::Pass => c.0 += 1,
                Outcome::Fail(_) => c.1 += 1,
                Outcome::OutOfScope(_) => c.2 += 1,
            }
            let listed = by_iri.get(t.iri.as_str()).copied();
            let reason = match (&outcome, listed) {
                (Outcome::Pass, _) => String::new(),
                (Outcome::OutOfScope(why), _) => why.clone(),
                // A known failure keeps its written reason.
                (Outcome::Fail(_), Some(("known-fail", r))) if !r.is_empty() => r.to_string(),
                (Outcome::Fail(why), _) => first_line(why).replace('\t', " "),
            };
            writeln!(written, "{}\t{}\t{reason}", t.iri, outcome.status()).unwrap();
            match (listed.map(|l| l.0), &outcome) {
                (Some("pass"), Outcome::Pass)
                | (Some("known-fail"), Outcome::Fail(_))
                | (Some("out-of-scope"), Outcome::OutOfScope(_)) => {}
                (Some("pass"), Outcome::Fail(why)) => {
                    problems.push(format!("{}: regressed: {why}", t.iri))
                }
                (Some("known-fail"), Outcome::Pass) => problems.push(format!(
                    "{}: a known failure now passes; mark it pass",
                    t.iri
                )),
                (Some(s), o) => {
                    problems.push(format!("{}: listed {s}, is {} {o:?}", t.iri, o.status()))
                }
                (None, o) => problems.push(format!("{}: not in the status list ({o:?})", t.iri)),
            }
            seen.push((t.iri.clone(), t.dir));
        }
    }
    for (iri, _, _) in &listed {
        if !seen.iter().any(|(s, _)| s == iri) {
            problems.push(format!("{iri}: in the status list, not in the suites"));
        }
    }
    let mut report = String::new();
    let (mut p, mut f, mut o) = (0, 0, 0);
    for (dir, (pass, fail, out)) in &counts {
        writeln!(
            report,
            "W3C SPARQL eval {dir}: {pass}/{} pass, {fail} known failures, {out} out of scope",
            pass + fail
        )
        .unwrap();
        (p, f, o) = (p + pass, f + fail, o + out);
    }
    writeln!(
        report,
        "W3C SPARQL eval total: {p}/{} pass, {f} known failures, {o} out of scope ({} tests)",
        p + f,
        p + f + o
    )
    .unwrap();
    std::io::stderr().write_all(report.as_bytes()).unwrap();
    assert!(seen.len() > 150, "only {} tests found", seen.len());
    if std::env::var("SPARQL_W3C_WRITE").is_ok_and(|v| v == "1") {
        fs::write(&status_path, written).unwrap();
        eprintln!("wrote {}", status_path.display());
        return;
    }
    assert!(
        problems.is_empty(),
        "{} tests disagree with {STATUS}:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// The harness's own checks: the XML results reader, and the comparison
/// (row order, blank node renaming, a changed value, ORDER BY).
#[test]
fn results_reader_and_comparison() {
    let x = r#"<?xml version="1.0"?>
<!-- a comment -->
<sparql xmlns="http://www.w3.org/2005/sparql-results#">
  <head><variable name="x"/><variable name='y'/></head>
  <results>
    <result><binding name="x"><bnode>r1</bnode></binding>
      <binding name="y"><literal xml:lang="EN">a &lt;b&gt; &amp; &#x263A;</literal></binding></result>
    <result><binding name="x"><uri>http://e/a</uri></binding>
      <binding name="y"><literal datatype="http://www.w3.org/2001/XMLSchema#integer">1</literal></binding></result>
    <result><binding name="x"><bnode>r1</bnode></binding><binding name="y"><literal/></binding></result>
  </results>
</sparql>"#;
    let Ok(Expected::Solutions { variables, rows }) = srx(x) else {
        panic!("not read")
    };
    assert_eq!(variables, ["x", "y"]);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0][1].1,
        Term::from(Literal::new_language_tagged_literal_unchecked(
            "a <b> & \u{263A}",
            "en"
        ))
    );
    assert_eq!(rows[2][1].1, Term::from(Literal::new_simple_literal("")));
    assert!(matches!(
        srx(r#"<sparql><head/><boolean> true </boolean></sparql>"#),
        Ok(Expected::Boolean(true))
    ));

    let iri = |s: &str| -> Term { nn(&format!("http://e/{s}")).into() };
    let b = |s: &str| -> Term { BlankNode::new_unchecked(s).into() };
    let row = |x: Term, y: Term| -> Row { vec![("x".into(), x), ("y".into(), y)] };
    let all = |_: &str| true;
    let g = |rows: &[Row]| rows_graph(rows.iter(), &all, false);
    let want = [
        row(b("r1"), iri("a")),
        row(b("r1"), iri("b")),
        row(iri("c"), iri("d")),
    ];
    // Reordered, with the blank node renamed: the same.
    let same = [
        row(iri("c"), iri("d")),
        row(b("z"), iri("b")),
        row(b("z"), iri("a")),
    ];
    assert_eq!(g(&want), g(&same));
    // Two blank nodes where the expected has one: not the same.
    let split = [
        row(b("p"), iri("a")),
        row(b("q"), iri("b")),
        row(iri("c"), iri("d")),
    ];
    assert_ne!(g(&want), g(&split));
    // A row twice is not the row once.
    let twice = [row(iri("c"), iri("d")), row(iri("c"), iri("d"))];
    assert_ne!(g(&twice), g(&twice[..1]));
    // ORDER BY: the key order counts, other columns do not.
    let q = algebra("SELECT * { ?x ?p ?y } ORDER BY ?y", "http://e/").unwrap();
    assert_eq!(order_keys(&q), ["y"]);
    let key = |v: &str| v == "y";
    let ordered = |rows: &[Row]| rows_graph(rows.iter(), &key, true);
    let a = [row(iri("1"), iri("a")), row(iri("2"), iri("b"))];
    let swapped_x = [row(iri("2"), iri("a")), row(iri("1"), iri("b"))];
    let swapped_y = [row(iri("2"), iri("b")), row(iri("1"), iri("a"))];
    assert_eq!(ordered(&a), ordered(&swapped_x));
    assert_ne!(ordered(&a), ordered(&swapped_y));
    let q = algebra("SELECT * { ?x ?p ?y } ORDER BY STR(?y) ?x", "http://e/").unwrap();
    assert!(order_keys(&q).is_empty());
    // Numbers compare by value within their datatype.
    let xsd = |l: &str| format!("http://www.w3.org/2001/XMLSchema#{l}");
    for (a, b, dt) in [
        ("3.21E4", "32100", "double"),
        ("2.0", "2", "decimal"),
        ("-0.0", "0", "decimal"),
        ("+007", "7", "integer"),
    ] {
        assert_eq!(numeric(a, &xsd(dt)), numeric(b, &xsd(dt)), "{a} {b}");
    }
    assert_ne!(
        numeric("2.5", &xsd("decimal")),
        numeric("2", &xsd("decimal"))
    );
    assert_eq!(numeric("x", &xsd("integer")), None);
    assert_eq!(numeric("1", &xsd("string")), None);
}
