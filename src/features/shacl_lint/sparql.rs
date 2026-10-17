//! SHACL-SPARQL query lint: each `sh:select` / `sh:ask` literal is parsed with
//! the prefixes its node declares through `sh:prefixes` (the `sh:declare`
//! entries of those nodes and the ontologies they `owl:imports`, found in
//! this document or where the workspace describes them). Reported on the literal: syntax errors at the parser's
//! position, undeclared prefixes, and the constructs the specification
//! forbids in queries with pre-bound variables (MINUS, VALUES, SERVICE,
//! `AS ?var` for a pre-bound variable, subqueries that drop one).
//!
//! SPARQL-based targets have no pre-bound `$this`, so only their syntax and
//! prefixes are checked.

use super::Finding;
use crate::analysis::Analysis;
use crate::knowledge::Knowledge;
use crate::syntax::{Iri, Node, RDF_TYPE, Span};
use rustc_hash::{FxHashMap, FxHashSet};
use spargebra::algebra::{AggregateExpression, Expression, GraphPattern, OrderExpression};
use spargebra::{Query, SparqlParser};
use std::sync::OnceLock;

pub const SYNTAX: &str = "shacl-sparql-syntax";
pub const PREFIX: &str = "shacl-sparql-prefix";
pub const UNSUPPORTED: &str = "shacl-sparql-unsupported";

struct V {
    rdf_type: Iri,
    select: Iri,
    ask: Iri,
    prefixes: Iri,
    declare: Iri,
    imports: Iri,
    prefix: Iri,
    namespace: Iri,
    validators: [Iri; 3],
    parameter: Iri,
    path: Iri,
    ask_validator: Iri,
    select_validator: Iri,
    target: Iri,
    target_types: [Iri; 2],
}

fn v() -> &'static V {
    static V: OnceLock<V> = OnceLock::new();
    V.get_or_init(|| {
        let sh = |l: &str| Iri::intern(&format!("{}{l}", crate::shacl::SH));
        V {
            rdf_type: Iri::intern(RDF_TYPE),
            select: sh("select"),
            ask: sh("ask"),
            prefixes: sh("prefixes"),
            declare: sh("declare"),
            imports: Iri::intern("http://www.w3.org/2002/07/owl#imports"),
            prefix: sh("prefix"),
            namespace: sh("namespace"),
            validators: [
                sh("validator"),
                sh("nodeValidator"),
                sh("propertyValidator"),
            ],
            parameter: sh("parameter"),
            path: sh("path"),
            ask_validator: sh("SPARQLAskValidator"),
            select_validator: sh("SPARQLSelectValidator"),
            target: sh("target"),
            target_types: [sh("SPARQLTarget"), sh("SPARQLTargetType")],
        }
    })
}

/// The document's triples by subject.
struct Index<'a> {
    triples: &'a [crate::syntax::Triple],
    by_subject: FxHashMap<&'a Node, Vec<u32>>,
}

impl<'a> Index<'a> {
    fn new(triples: &'a [crate::syntax::Triple]) -> Self {
        let mut by_subject: FxHashMap<&Node, Vec<u32>> = FxHashMap::default();
        for (i, t) in triples.iter().enumerate() {
            by_subject.entry(&t.s).or_default().push(i as u32);
        }
        Index {
            triples,
            by_subject,
        }
    }
    fn objects(&self, s: &Node, p: Iri) -> impl Iterator<Item = &'a Node> + '_ {
        self.by_subject
            .get(s)
            .into_iter()
            .flatten()
            .map(|&i| &self.triples[i as usize])
            .filter(move |t| t.p == p)
            .map(|t| &t.o)
    }
}

fn literal(n: &Node) -> Option<&str> {
    match n {
        Node::Literal { value, .. } => Some(value),
        _ => None,
    }
}

/// `sh:declare` entries of `node` in `triples`, and the nodes it
/// `owl:imports`.
fn declarations_in(
    triples: &[crate::syntax::Triple],
    node: &Node,
    out: &mut Vec<(String, String)>,
    imports: &mut Vec<Node>,
) {
    let v = v();
    let mut decls: FxHashSet<&Node> = FxHashSet::default();
    for t in triples.iter().filter(|t| t.s == *node) {
        if t.p == v.declare {
            decls.insert(&t.o);
        } else if t.p == v.imports {
            imports.push(t.o.clone());
        }
    }
    if decls.is_empty() {
        return;
    }
    let mut parts: FxHashMap<&Node, (Option<&str>, Option<&str>)> = FxHashMap::default();
    for t in triples {
        if decls.contains(&t.s) {
            if t.p == v.prefix {
                parts.entry(&t.s).or_default().0 = literal(&t.o);
            } else if t.p == v.namespace {
                parts.entry(&t.s).or_default().1 = literal(&t.o);
            }
        }
    }
    out.extend(
        parts
            .into_values()
            .filter_map(|(p, n)| Some((p?.to_owned(), n?.to_owned()))),
    );
}

/// The prefixes a `sh:prefixes` value declares: its own `sh:declare`
/// entries and those of the ontologies it imports, from this document and
/// wherever the workspace describes them.
fn declarations(a: &Analysis, k: &Knowledge, node: &Node) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen: FxHashSet<Node> = FxHashSet::default();
    let mut todo = vec![node.clone()];
    while let Some(n) = todo.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        let mut imports = Vec::new();
        declarations_in(&a.parsed.triples, &n, &mut out, &mut imports);
        if let Node::Iri(iri) = &n {
            for desc in k.descriptions(iri) {
                if !std::ptr::eq(desc.analysis, a) {
                    declarations_in(&desc.analysis.parsed.triples, &n, &mut out, &mut imports);
                }
            }
        }
        // Blank nodes are local to their document: follow imported IRIs only.
        todo.extend(imports.into_iter().filter(|i| matches!(i, Node::Iri(_))));
    }
    out
}

/// What a query is, which decides its pre-bound variables.
enum Role {
    /// A SPARQL-based target: nothing pre-bound.
    Target,
    /// A constraint or validator: these variables are pre-bound.
    PreBound(Vec<String>),
}

pub(super) fn lint(a: &Analysis, k: &Knowledge, out: &mut Vec<Finding>) {
    let v = v();
    let triples = &a.parsed.triples;
    let queries: Vec<usize> = (0..triples.len())
        .filter(|&i| {
            let t = &triples[i];
            (t.p == v.select || t.p == v.ask) && matches!(t.o, Node::Literal { .. })
        })
        .collect();
    if queries.is_empty() {
        return;
    }
    let ix = Index::new(triples);

    // Nodes used as validators (with their components) and as targets.
    let mut components: FxHashMap<&Node, Vec<&Node>> = FxHashMap::default();
    let mut targets: FxHashSet<&Node> = FxHashSet::default();
    let mut validators: FxHashSet<&Node> = FxHashSet::default();
    for t in triples {
        if v.validators.contains(&t.p) {
            components.entry(&t.o).or_default().push(&t.s);
        } else if t.p == v.target {
            targets.insert(&t.o);
        } else if t.p == v.rdf_type
            && let Node::Iri(c) = &t.o
        {
            if v.target_types.contains(c) {
                targets.insert(&t.s);
            } else if *c == v.ask_validator || *c == v.select_validator {
                validators.insert(&t.s);
            }
        }
    }

    let mut declared: FxHashMap<&Node, Vec<(String, String)>> = FxHashMap::default();
    for i in queries {
        let t = &triples[i];
        let Node::Literal { value, .. } = &t.o else {
            continue;
        };
        let mut parser = SparqlParser::new();
        for p in ix.objects(&t.s, v.prefixes) {
            let decls = declared.entry(p).or_insert_with(|| declarations(a, k, p));
            for (name, ns) in decls.iter() {
                if let Ok(next) = parser.clone().with_prefix(name, ns) {
                    parser = next;
                }
            }
        }
        let lit = Lit::new(&a.text, t.o_span, value);
        let query = match parser.clone().parse_query(value) {
            Ok(q) => q,
            Err(_) => {
                syntax_error(&lit, parser, out);
                continue;
            }
        };
        let role = if targets.contains(&t.s) {
            Role::Target
        } else {
            let mut vars: Vec<String> = ["this", "shapesGraph", "currentShape"]
                .map(String::from)
                .into();
            let is_validator = validators.contains(&t.s) || components.contains_key(&t.s);
            if is_validator && t.p == v.ask {
                vars.push("value".into());
            }
            for c in components.get(&t.s).into_iter().flatten() {
                for p in ix.objects(c, v.parameter) {
                    for path in ix.objects(p, v.path) {
                        if let Node::Iri(path) = path {
                            vars.push(
                                path[crate::knowledge::namespace_of(path).len()..].to_owned(),
                            );
                        }
                    }
                }
            }
            Role::PreBound(vars)
        };
        if let Role::PreBound(vars) = role {
            unsupported(&lit, &query, &vars, out);
        }
    }
}

/// A query literal: its text in the document and its value.
struct Lit<'a> {
    text: &'a str,
    span: Span,
    /// Length of the opening quotes.
    quote: u32,
    value: &'a str,
}

impl<'a> Lit<'a> {
    fn new(text: &'a str, span: Span, value: &'a str) -> Self {
        let src = &text[span.start as usize..span.end as usize];
        let quote = if src.starts_with("\"\"\"") || src.starts_with("'''") {
            3
        } else {
            1
        };
        Lit {
            text,
            span,
            quote,
            value,
        }
    }

    /// The document offset of byte `off` of the value, walking the escapes.
    fn offset(&self, off: usize) -> Option<u32> {
        let start = (self.span.start + self.quote) as usize;
        let raw = self.text.get(start..self.span.end as usize)?.as_bytes();
        let val = self.value.as_bytes();
        let (mut r, mut d) = (0, 0);
        while d < off {
            let &c = raw.get(r)?;
            if c == b'\\' {
                let (rl, dl) = match raw.get(r + 1)? {
                    b'u' => (6, 4),
                    b'U' => (10, 8),
                    _ => (2, 1),
                };
                let dl = if rl > 2 {
                    let hex = std::str::from_utf8(raw.get(r + 2..r + 2 + dl)?).ok()?;
                    char::from_u32(u32::from_str_radix(hex, 16).ok()?)?.len_utf8()
                } else {
                    dl
                };
                r += rl;
                d += dl;
            } else {
                if val.get(d) != Some(&c) {
                    return None;
                }
                r += 1;
                d += 1;
            }
        }
        (d == off).then_some((start + r) as u32)
    }

    /// The span of value bytes `from..to`, or the whole literal.
    fn span(&self, from: usize, to: usize) -> Span {
        match (self.offset(from), self.offset(to)) {
            (Some(s), Some(e)) if s < e => Span::new(s, e),
            _ => self.span,
        }
    }
}

/// A query that cannot be parsed never runs, so that is an error; forbidden
/// constructs are warnings.
fn push(out: &mut Vec<Finding>, span: Span, code: &'static str, message: String) {
    out.push(Finding {
        span,
        error: code != UNSUPPORTED,
        code,
        message,
    });
}

/// Room after the query for the parser's keyword lookahead (see
/// [`syntax_error`]); longer than any SPARQL keyword.
const PAD: &str = "                ";

/// Reports a query that does not parse. The parser says `error at
/// LINE:COLUMN: expected WHAT` (column in characters) at the furthest
/// position any rule failed. Keywords are matched by reading as many
/// characters as the keyword has and then comparing, so a failed keyword
/// reports its start plus its length; that length is taken back off. The
/// query is parsed again with trailing room so that a keyword read near the
/// end does not stop at the end of the text instead.
fn syntax_error(lit: &Lit, parser: SparqlParser, out: &mut Vec<Finding>) {
    let v = lit.value;
    let padded = format!("{v}{PAD}");
    let msg = match parser.parse_query(&padded) {
        Err(e) => e.to_string(),
        Ok(_) => String::new(),
    };
    let parsed = msg.strip_prefix("error at ").and_then(|rest| {
        let (pos, expected) = rest.split_once(": expected ")?;
        let (line, col) = pos.split_once(':')?;
        Some((
            line.parse::<usize>().ok()?,
            col.parse::<usize>().ok()?,
            expected,
        ))
    });
    let Some((line, col, expected)) = parsed else {
        push(out, lit.span, SYNTAX, "SPARQL syntax error".into());
        return;
    };
    let line_start = padded
        .split_inclusive('\n')
        .take(line - 1)
        .map(str::len)
        .sum::<usize>();
    let mut off = padded[line_start..]
        .char_indices()
        .nth(col - 1)
        .map_or(padded.len(), |(i, _)| line_start + i);
    let items: Vec<&str> = expected
        .strip_prefix("one of ")
        .unwrap_or(expected)
        .split(", ")
        .collect();
    let keyword = items
        .iter()
        .filter(|i| !i.is_empty() && i.bytes().all(|c| c.is_ascii_alphabetic()))
        .map(|i| i.len())
        .max();
    if let Some(n) = keyword {
        off = off.saturating_sub(n);
    }
    let pname = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '%');
    if items.contains(&"Prefix not found") && off <= v.len() {
        // Reported at the end of the prefixed name.
        let start = v[..off].rfind(|c: char| !pname(c)).map_or(0, |i| i + 1);
        if let Some(colon) = v[start..off].find(':') {
            let name = &v[start..start + colon];
            push(
                out,
                lit.span(start, off),
                PREFIX,
                format!(
                    "Prefix {name}: is not declared for this query; declare it with sh:prefixes and sh:declare"
                ),
            );
            return;
        }
    }
    let rest = v.get(off..).unwrap_or("");
    let span = match rest.char_indices().find(|(_, c)| !c.is_whitespace()) {
        // Only room left: the query ends too early; mark the closing quotes.
        None => lit
            .offset(v.len())
            .map_or(lit.span, |s| Span::new(s, s + lit.quote)),
        Some((i, c)) => {
            let from = off + i;
            let len = if pname(c) {
                v[from..]
                    .find(|c: char| !pname(c))
                    .unwrap_or(v.len() - from)
            } else {
                c.len_utf8()
            };
            lit.span(from, from + len)
        }
    };
    let what = if keyword.is_some() || items.iter().all(|i| *i == "[_]") {
        String::new()
    } else {
        format!(": expected {expected}")
    };
    push(out, span, SYNTAX, format!("SPARQL syntax error{what}"));
}

/// What the walk over the algebra found.
#[derive(Default)]
struct Found {
    minus: bool,
    values: bool,
    service: bool,
    lateral: bool,
    /// Pre-bound variables assigned with `AS`.
    assigned: Vec<String>,
    /// Subqueries so far.
    subqueries: usize,
    /// Subqueries (by number) that leave out a pre-bound variable.
    dropped: Vec<(usize, String)>,
}

fn unsupported(lit: &Lit, q: &Query, vars: &[String], out: &mut Vec<Finding>) {
    let mut f = Found::default();
    let (Query::Select { pattern, .. }
    | Query::Ask { pattern, .. }
    | Query::Construct { pattern, .. }
    | Query::Describe { pattern, .. }) = q;
    walk(pattern, true, vars, &mut f);
    let select = matches!(q, Query::Select { .. });
    let v = lit.value;
    let at_word =
        |w: &str, nth: usize| find_word(v, w, nth).map_or(lit.span, |s| lit.span(s, s + w.len()));
    for (on, word) in [
        (f.minus, "MINUS"),
        (f.values, "VALUES"),
        (f.service, "SERVICE"),
    ] {
        if on {
            push(
                out,
                at_word(word, 0),
                UNSUPPORTED,
                format!("SHACL-SPARQL does not allow {word} in a query with pre-bound variables"),
            );
        }
    }
    if f.lateral {
        push(
            out,
            at_word("LATERAL", 0),
            UNSUPPORTED,
            "LATERAL is not standard SPARQL".to_owned(),
        );
    }
    for var in &f.assigned {
        let span = find_assignment(v, var).map_or(lit.span, |(s, e)| lit.span(s, e));
        push(
            out,
            span,
            UNSUPPORTED,
            format!("SHACL-SPARQL does not allow assigning the pre-bound variable ?{var} with AS"),
        );
    }
    for (n, var) in f.dropped {
        let at = find_word(v, "SELECT", n + usize::from(select));
        // `SELECT *` returns the pre-bound variables once they are bound.
        if at.is_some_and(|i| selects_all(&v[i + 6..])) {
            continue;
        }
        push(
            out,
            at.map_or(lit.span, |s| lit.span(s, s + 6)),
            UNSUPPORTED,
            format!("A subquery must return the pre-bound variable ?{var}"),
        );
    }
}

fn walk(p: &GraphPattern, top: bool, vars: &[String], f: &mut Found) {
    match p {
        GraphPattern::Bgp { .. } | GraphPattern::Path { .. } => {}
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } => {
            walk(left, false, vars, f);
            walk(right, false, vars, f);
        }
        GraphPattern::Lateral { left, right } => {
            f.lateral = true;
            walk(left, false, vars, f);
            walk(right, false, vars, f);
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            walk(left, false, vars, f);
            walk(right, false, vars, f);
            if let Some(e) = expression {
                expr(e, vars, f);
            }
        }
        GraphPattern::Filter { expr: e, inner } => {
            expr(e, vars, f);
            walk(inner, false, vars, f);
        }
        GraphPattern::Graph { inner, .. } => walk(inner, false, vars, f),
        GraphPattern::Extend {
            inner,
            variable,
            expression,
        } => {
            if vars.iter().any(|v| v == variable.as_str())
                && !f.assigned.iter().any(|v| v == variable.as_str())
            {
                f.assigned.push(variable.as_str().to_owned());
            }
            expr(expression, vars, f);
            walk(inner, top, vars, f);
        }
        GraphPattern::Minus { left, right } => {
            f.minus = true;
            walk(left, false, vars, f);
            walk(right, false, vars, f);
        }
        GraphPattern::Values { .. } => f.values = true,
        GraphPattern::OrderBy { inner, expression } => {
            for o in expression {
                let (OrderExpression::Asc(e) | OrderExpression::Desc(e)) = o;
                expr(e, vars, f);
            }
            walk(inner, top, vars, f);
        }
        GraphPattern::Project { inner, variables } => {
            if !top {
                if let Some(v) = vars
                    .iter()
                    .filter(|v| *v != "shapesGraph" && *v != "currentShape")
                    .find(|v| !variables.iter().any(|x| x.as_str() == v.as_str()))
                {
                    f.dropped.push((f.subqueries, v.clone()));
                }
                f.subqueries += 1;
            }
            walk(inner, false, vars, f);
        }
        GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => {
            walk(inner, top, vars, f);
        }
        GraphPattern::Group {
            inner, aggregates, ..
        } => {
            for (_, a) in aggregates {
                if let AggregateExpression::FunctionCall { expr: e, .. } = a {
                    expr(e, vars, f);
                }
            }
            walk(inner, top, vars, f);
        }
        GraphPattern::Service { inner, .. } => {
            f.service = true;
            walk(inner, false, vars, f);
        }
    }
}

/// Graph patterns inside expressions (`EXISTS`, `NOT EXISTS`).
fn expr(e: &Expression, vars: &[String], f: &mut Found) {
    match e {
        Expression::Exists(p) => walk(p, false, vars, f),
        Expression::Or(a, b)
        | Expression::And(a, b)
        | Expression::Equal(a, b)
        | Expression::SameTerm(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterOrEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b)
        | Expression::Subtract(a, b)
        | Expression::Multiply(a, b)
        | Expression::Divide(a, b) => {
            expr(a, vars, f);
            expr(b, vars, f);
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
            expr(a, vars, f);
        }
        Expression::In(a, list) => {
            expr(a, vars, f);
            list.iter().for_each(|x| expr(x, vars, f));
        }
        Expression::If(a, b, c) => {
            expr(a, vars, f);
            expr(b, vars, f);
            expr(c, vars, f);
        }
        Expression::Coalesce(list) | Expression::FunctionCall(_, list) => {
            list.iter().for_each(|x| expr(x, vars, f));
        }
        _ => {}
    }
}

/// Whether the text after a `SELECT` keyword projects `*`.
fn selects_all(rest: &str) -> bool {
    let mut rest = rest.trim_start();
    for m in ["DISTINCT", "REDUCED"] {
        if rest
            .get(..m.len())
            .is_some_and(|x| x.eq_ignore_ascii_case(m))
        {
            rest = rest[m.len()..].trim_start();
        }
    }
    rest.starts_with('*')
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'?' | b'$' | b'-')
}

/// Byte offset of the `nth` occurrence of keyword `w` in `q`, as a whole word.
fn find_word(q: &str, w: &str, nth: usize) -> Option<usize> {
    let b = q.as_bytes();
    let mut seen = 0;
    (0..b.len().saturating_sub(w.len() - 1)).find(|&i| {
        let hit = b[i..i + w.len()].eq_ignore_ascii_case(w.as_bytes())
            && (i == 0 || !is_word(b[i - 1]))
            && b.get(i + w.len()).is_none_or(|&c| !is_word(c));
        if hit {
            seen += 1;
        }
        hit && seen == nth + 1
    })
}

/// The `AS ?var` that assigns `var`.
fn find_assignment(q: &str, var: &str) -> Option<(usize, usize)> {
    let b = q.as_bytes();
    let mut nth = 0;
    while let Some(i) = find_word(q, "AS", nth) {
        let rest = &q[i + 2..];
        let j = i + 2 + (rest.len() - rest.trim_start().len());
        if matches!(b.get(j), Some(b'?' | b'$'))
            && q[j + 1..].starts_with(var)
            && b.get(j + 1 + var.len()).is_none_or(|&c| !is_word(c))
        {
            return Some((i, j + 1 + var.len()));
        }
        nth += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::lint as shacl_lint;
    use super::*;
    use crate::knowledge::Source;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    const HEAD: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
                        @prefix ex: <http://example.com/> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                        ex: sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\"^^xsd:anyURI ] .\n";

    fn analysis(uri: &str, text: &str) -> Arc<Analysis> {
        Arc::new(Analysis::new(
            uri.into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ))
    }

    /// The SPARQL findings for `body` after HEAD, as (code, covered text).
    fn findings(body: &str) -> Vec<(&'static str, String)> {
        findings_with(body, &[])
    }

    fn findings_with(body: &str, others: &[&str]) -> Vec<(&'static str, String)> {
        let text = format!("{HEAD}{body}");
        let a = analysis("file:///w/shapes.ttl", &text);
        let mut k = Knowledge::default();
        k.set("file:///w/shapes.ttl", Source::Open, a.clone());
        for (n, o) in others.iter().enumerate() {
            let uri = format!("file:///w/other{n}.ttl");
            k.set(&uri, Source::Disk, analysis(&uri, o));
        }
        shacl_lint(&a, &k)
            .into_iter()
            .filter(|f| f.code.starts_with("shacl-sparql"))
            .map(|f| {
                assert_eq!(f.error, f.code != UNSUPPORTED, "{}", f.code);
                (
                    f.code,
                    text[f.span.start as usize..f.span.end as usize].to_owned(),
                )
            })
            .collect()
    }

    fn constraint(query: &str) -> String {
        format!(
            "ex:S a sh:NodeShape ; sh:targetNode ex:x ;\n  sh:sparql [ sh:prefixes ex: ;\n    sh:select {query} ] .\n"
        )
    }

    #[test]
    fn a_valid_query_is_quiet() {
        let q = "\"\"\"\n    SELECT $this ?value WHERE { $this ex:p ?value . FILTER NOT EXISTS { ?value a ex:C } }\n  \"\"\"";
        assert_eq!(findings(&constraint(q)), vec![]);
    }

    #[test]
    fn syntax_error_at_the_parser_position() {
        let q = "\"\"\"\n    SELECT $this WHERE {\n      $this ex:p ?v ) }\"\"\"";
        assert_eq!(findings(&constraint(q)), vec![(SYNTAX, ")".to_owned())]);
        // A one-line literal with escapes before the error.
        let q = "\"SELECT $this WHERE { $this ex:p \\\"a\\\" ; ex:q ?v ) }\"";
        assert_eq!(findings(&constraint(q)), vec![(SYNTAX, ")".to_owned())]);
    }

    #[test]
    fn misspelled_keyword_is_marked() {
        let q = "\"\"\"SELECT $this WHERE {\n  $this ex:p ?v .\n  FILTRE (?v > 1) }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(SYNTAX, "FILTRE".to_owned())]
        );
    }

    /// The W3C SHACL-SPARQL tests: the unsupported-sparql ones are each
    /// warned, every other query is clean.
    #[test]
    fn w3c_sparql_tests() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/w3c/shacl/sparql");
        let mut files = vec![];
        let mut dirs = vec![std::path::PathBuf::from(root)];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "ttl") {
                    files.push(p);
                }
            }
        }
        assert!(files.len() > 20);
        for p in files {
            let text = std::fs::read_to_string(&p).unwrap();
            let uri = format!("file://{}", p.display());
            let a = analysis(&uri, &text);
            let mut k = Knowledge::default();
            k.set(&uri, Source::Disk, a.clone());
            let found: Vec<_> = shacl_lint(&a, &k)
                .into_iter()
                .filter(|f| f.code.starts_with("shacl-sparql"))
                .map(|f| (f.code, f.message))
                .collect();
            let name = p.file_name().unwrap().to_str().unwrap();
            if name.starts_with("unsupported-sparql-") {
                assert!(
                    found.len() == 1 && found[0].0 == UNSUPPORTED,
                    "{name}: {found:?}"
                );
            } else {
                assert!(found.is_empty(), "{name}: {found:?}");
            }
        }
    }

    #[test]
    fn syntax_error_at_the_end_points_at_the_closing_quotes() {
        let q = "\"\"\"SELECT $this WHERE { $this ex:p ?v \"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(SYNTAX, "\"\"\"".to_owned())]
        );
    }

    #[test]
    fn undeclared_prefix_is_named() {
        let q = "\"\"\"SELECT $this WHERE { $this foaf:name ?v }\"\"\"";
        let f = findings(&constraint(q));
        assert_eq!(f, vec![(PREFIX, "foaf:name".to_owned())]);
        let text = format!("{HEAD}{}", constraint(q));
        let a = analysis("file:///w/s.ttl", &text);
        let msg = &shacl_lint(&a, &Knowledge::default())[0].message;
        assert!(msg.contains("foaf:"), "{msg}");
    }

    #[test]
    fn prefixes_declared_in_another_document() {
        let other = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
                     <http://example.com/onto> sh:declare [ sh:prefix \"foaf\" ; sh:namespace \"http://xmlns.com/foaf/0.1/\" ] .\n";
        let body = "ex:S sh:sparql [ sh:prefixes <http://example.com/onto> ;\n  sh:select \"SELECT $this WHERE { $this foaf:name ?v }\" ] .\n";
        assert_eq!(findings_with(body, &[other]), vec![]);
        assert_eq!(findings_with(body, &[]).len(), 1);
    }

    #[test]
    fn minus_values_service_are_warned() {
        let q = "\"\"\"SELECT $this WHERE { $this ?x ?any . MINUS { $this ?x \"Value\" } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "MINUS".to_owned())]
        );
        let q = "\"\"\"SELECT $this WHERE { VALUES ?any { true } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "VALUES".to_owned())]
        );
        let q = "\"\"\"SELECT $this WHERE { $this ?x ?any . SERVICE <http://a.example/> { ?a ?b ?c } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "SERVICE".to_owned())]
        );
    }

    #[test]
    fn lateral_is_warned() {
        let q = "\"\"\"SELECT $this WHERE { $this ?x ?any . LATERAL { SELECT $this ?any WHERE { $this ?b ?any } LIMIT 1 } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "LATERAL".to_owned())]
        );
    }

    #[test]
    fn prebound_assignment_and_subquery_are_warned() {
        let q = "\"\"\"SELECT $this WHERE { BIND (true AS $this) . }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "AS $this".to_owned())]
        );
        let q = "\"\"\"SELECT $this WHERE { $this ?x ?any . { SELECT ?other ?b WHERE { ?other ?b ?c } } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "SELECT".to_owned())]
        );
        let q = "\"\"\"SELECT $this WHERE { { SELECT $this WHERE { $this ?b ?c } } }\"\"\"";
        assert_eq!(findings(&constraint(q)), vec![]);
    }

    #[test]
    fn ask_validator_cannot_assign_value() {
        let body = "ex:C a sh:ConstraintComponent ;\n  sh:parameter [ sh:path ex:lang ] ;\n  sh:validator ex:V .\n\
                    ex:V a sh:SPARQLAskValidator ; sh:ask \"ASK { BIND (true AS ?value) . FILTER (langMatches(lang(?value), $lang)) }\" .\n";
        assert_eq!(findings(body), vec![(UNSUPPORTED, "AS ?value".to_owned())]);
        let body = "ex:C a sh:ConstraintComponent ;\n  sh:parameter [ sh:path ex:lang ] ;\n  sh:validator ex:V .\n\
                    ex:V a sh:SPARQLAskValidator ; sh:ask \"ASK { BIND (\\\"en\\\" AS ?lang) }\" .\n";
        assert_eq!(findings(body), vec![(UNSUPPORTED, "AS ?lang".to_owned())]);
    }

    #[test]
    fn targets_only_get_syntax_checks() {
        let body = "ex:S a sh:NodeShape ; sh:target [ a sh:SPARQLTarget ; sh:prefixes ex: ;\n  sh:select \"SELECT ?this WHERE { ?this a ex:C MINUS { ?this ex:p 1 } }\" ] .\n";
        assert_eq!(findings(body), vec![]);
    }

    #[test]
    fn many_constraints_lint_fast() {
        let mut body = String::new();
        for i in 0..200 {
            body.push_str(&format!(
                "ex:S{i} a sh:NodeShape ; sh:targetClass ex:C ;\n  sh:sparql [ sh:prefixes ex: ; sh:message \"m\" ;\n    sh:select \"\"\"\n      SELECT $this ?value WHERE {{\n        $this ex:p{i} ?value .\n        FILTER NOT EXISTS {{ ?value a ex:D }}\n      }}\"\"\" ] .\n"
            ));
        }
        let text = format!("{HEAD}{body}");
        let a = analysis("file:///w/shapes.ttl", &text);
        let mut k = Knowledge::default();
        k.set("file:///w/shapes.ttl", Source::Open, a.clone());
        shacl_lint(&a, &k);
        let t = std::time::Instant::now();
        let n = shacl_lint(&a, &k)
            .iter()
            .filter(|f| f.code.starts_with("shacl-sparql"))
            .count();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("200 sh:select constraints linted in {ms:.2} ms");
        assert_eq!(n, 0);
        assert!(ms < 20.0, "{ms} ms");
    }
}
