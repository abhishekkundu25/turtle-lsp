//! JSON-LD 1.1 deserialization to RDF (JSON-LD 1.1 Processing Algorithms
//! §7.1 Deserialize JSON-LD to RDF, with node map generation §7.2, object to
//! RDF conversion §7.3 and list conversion §7.4): expanded [`Item`]s become
//! the triples, statements and references of a [`Parsed`]. rdfDirection is
//! not supported (a base direction is dropped, the language stays), and no
//! generalized RDF is produced.
//!
//! Node map generation is folded into one walk of the expanded tree: every
//! node object, wherever it is nested, contributes its own triples to the
//! graph it is in, which is the set of quads the spec's node map
//! serializes to (the same node described in two places gets both sets of
//! triples; blank node identifiers are per document, labels included).
//!
//! What lands where:
//! - each triple's `p_span` is the key that gave the predicate (`@type` for
//!   `rdf:type`), its `o_span` the value that gave the object (a literal's
//!   whole JSON token; an IRI's string without quotes; a nested node
//!   object's braces). List triples (`rdf:first`, `rdf:rest`) take the span
//!   of their item, as in Turtle;
//! - `statement` is the node object the subject comes from: a statement per
//!   node object that has triples, spanning the object, whose `subject` is
//!   its IRI `@id`. Statements are in text order and can nest;
//! - `graph` is the named graph (`@graph` beside an `@id`, or a graph
//!   container), as in TriG;
//! - `refs` hold every IRI written in the document that lands in a triple:
//!   keys ([`Role::Predicate`], `@type` as [`Form::A`]), the `@id` of a node
//!   object that describes the node ([`Role::Subject`], or [`Role::Graph`]
//!   when it only names a graph), `@id`s referred to and `@type` values
//!   ([`Role::Object`]), and datatypes written in value objects
//!   ([`Role::Datatype`]), sorted by position.
//!
//! What the spec drops silently is recorded as [`Dropped`]: IRIs that are
//! relative or not well-formed (with their triples), ill-formed language
//! tags (with their literal), and blank node predicates.
//!
//! Literals use the spec's canonical forms: `true`/`false`, integers
//! without exponent, doubles as `1.5E0`, and `@json` values in the JSON
//! Canonicalization Scheme (RFC 8785) as `rdf:JSON`.

use super::expand::{Item, ListObject, Literal, NodeObject, ValueObject};
use super::json::{Json, Value};
use super::{DropReason, Dropped};
use crate::syntax::{
    BlankId, Form, GraphBlock, GraphName, Iri, Node, Parsed, RDF, RDF_TYPE, Ref, Role, Span,
    Statement, Triple, XSD,
};
use rustc_hash::FxHashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

/// IRIs the conversion produces itself, interned once.
struct Vocab {
    rdf_type: Iri,
    first: Iri,
    rest: Iri,
    nil: Iri,
    json: Iri,
    lang_string: Iri,
    string: Iri,
    integer: Iri,
    double: Iri,
    boolean: Iri,
}

fn vocab() -> &'static Vocab {
    static V: OnceLock<Vocab> = OnceLock::new();
    V.get_or_init(|| {
        let rdf = |l: &str| Iri::intern(&format!("{RDF}{l}"));
        let xsd = |l: &str| Iri::intern(&format!("{XSD}{l}"));
        Vocab {
            rdf_type: Iri::intern(RDF_TYPE),
            first: rdf("first"),
            rest: rdf("rest"),
            nil: rdf("nil"),
            json: rdf("JSON"),
            lang_string: rdf("langString"),
            string: xsd("string"),
            integer: xsd("integer"),
            double: xsd("double"),
            boolean: xsd("boolean"),
        }
    })
}

/// Add the triples, statements and references of `items` (an expanded
/// document) to `out`, and what is dropped to `dropped`.
pub fn to_rdf<'e>(items: &'e [Item<'e>], out: &mut Parsed, dropped: &mut Vec<Dropped>) {
    let mut r = Rdf {
        out,
        dropped,
        labels: FxHashMap::default(),
        bnodes: 0,
        iris: FxHashMap::default(),
        langs: FxHashMap::default(),
        v: vocab(),
    };
    for item in items {
        if let Item::Node(n) = item {
            r.node(n, G::Default, Role::Subject);
        }
    }
    sort_statements(r.out);
    r.out.refs.sort_by_key(|x| (x.span.start, x.span.end));
}

/// Put statements in text order, renumbering the triples' references.
fn sort_statements(out: &mut Parsed) {
    if out.statements.is_sorted_by_key(|s| s.span.start) {
        return;
    }
    let mut order: Vec<u32> = (0..out.statements.len() as u32).collect();
    order.sort_by_key(|&i| out.statements[i as usize].span.start);
    let mut new_index = vec![0u32; order.len()];
    for (new, &old) in order.iter().enumerate() {
        new_index[old as usize] = new as u32;
    }
    let mut old: Vec<Option<Statement>> = std::mem::take(&mut out.statements)
        .into_iter()
        .map(Some)
        .collect();
    out.statements = order
        .iter()
        .map(|&i| old[i as usize].take().expect("each statement once"))
        .collect();
    for t in &mut out.triples {
        t.statement = new_index[t.statement as usize];
    }
}

/// The graph triples go to.
#[derive(Clone, Copy)]
enum G {
    Default,
    Named(GraphName),
    /// Named by an IRI that is not well-formed: its triples are dropped.
    Dropped,
}

struct Rdf<'e, 'o> {
    out: &'o mut Parsed,
    dropped: &'o mut Vec<Dropped>,
    /// Blank node labels (`_:b0`) -> node.
    labels: FxHashMap<&'e str, BlankId>,
    bnodes: u32,
    /// IRI text -> interned IRI, `None` when not well-formed.
    iris: FxHashMap<&'e str, Option<Iri>>,
    /// Language tag as written -> interned lowercase tag, `None` when not
    /// well-formed.
    langs: FxHashMap<&'e str, Option<Iri>>,
    v: &'static Vocab,
}

impl<'e> Rdf<'e, '_> {
    fn fresh(&mut self) -> Node {
        self.bnodes += 1;
        Node::Blank(BlankId(self.bnodes))
    }

    fn drop(&mut self, span: Span, reason: DropReason) {
        self.dropped.push(Dropped { span, reason });
    }

    /// A well-formed absolute IRI, interned; otherwise recorded at `span`.
    fn iri(&mut self, s: &'e str, span: Option<Span>) -> Option<Iri> {
        let iri = match self.iris.get(s) {
            Some(&hit) => hit,
            None => {
                let iri = oxiri::Iri::parse(s).ok().map(|_| Iri::intern(s));
                self.iris.insert(s, iri);
                iri
            }
        };
        if iri.is_none()
            && let Some(span) = span
        {
            let reason = if super::context::is_absolute_iri(s) {
                DropReason::InvalidIri
            } else {
                DropReason::RelativeIri
            };
            self.drop(span, reason);
        }
        iri
    }

    /// A node identifier: a blank node identifier or an IRI. A keyword-like
    /// `@id` (`null` once expanded, already recorded) is neither.
    fn resource(&mut self, s: &'e str, span: Span) -> Option<Node> {
        if s.starts_with('@') {
            return None;
        }
        if let Some(label) = s.strip_prefix("_:") {
            let next = self.bnodes + 1;
            let id = *self.labels.entry(label).or_insert(BlankId(next));
            if id.0 == next {
                self.bnodes = next;
            }
            return Some(Node::Blank(id));
        }
        self.iri(s, Some(span)).map(Node::Iri)
    }

    fn reference(&mut self, span: Span, iri: Iri, role: Role, form: Form) {
        self.out.refs.push(Ref {
            span,
            iri,
            role,
            form,
        });
    }

    fn statement(&mut self, n: &NodeObject, s: &Node) -> u32 {
        let subject = match (s, &n.id) {
            (Node::Iri(i), Some(id)) => Some((*i, id.span)),
            _ => None,
        };
        self.out.statements.push(Statement {
            span: n.span,
            subject,
            graph: None,
        });
        self.out.statements.len() as u32 - 1
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(&mut self, s: &Node, p: Iri, o: Node, p_span: Span, o_span: Span, stmt: u32, g: G) {
        let graph = match g {
            G::Default => None,
            G::Named(n) => Some(n),
            G::Dropped => return,
        };
        self.out.triples.push(Triple {
            s: s.clone(),
            p,
            o,
            p_span,
            o_span,
            statement: stmt,
            graph,
        });
    }

    /// A node object in graph `g`: its triples, and those of every node
    /// nested in it. Returns its subject (`None` when its `@id` is not
    /// well-formed) and its statement, if it has one.
    fn node(&mut self, n: &'e NodeObject<'e>, g: G, role: Role) -> (Option<Node>, Option<u32>) {
        let s = match &n.id {
            None => Some(self.fresh()),
            Some(id) => self.resource(&id.iri, id.span),
        };
        if let (Some(Node::Iri(i)), Some(id)) = (&s, &n.id) {
            let role = match () {
                _ if !n.described => role,
                _ if n.is_graph_object() => Role::Graph,
                _ => Role::Subject,
            };
            self.reference(id.span, *i, role, id.form);
        }
        let mut stmt = match &s {
            Some(s) if n.described => Some(self.statement(n, s)),
            _ => None,
        };
        let mut a_key = None;
        for t in &n.types {
            let o = self.resource(&t.iri.iri, t.iri.span);
            if let Some(Node::Iri(i)) = o {
                self.reference(t.iri.span, i, Role::Object, t.iri.form);
            }
            if a_key != Some(t.key) {
                a_key = Some(t.key);
                self.reference(t.key, self.v.rdf_type, Role::Predicate, Form::A);
            }
            if let (Some(s), Some(o)) = (&s, o) {
                let st = *stmt.get_or_insert_with(|| self.statement(n, s));
                self.emit(s, self.v.rdf_type, o, t.key, t.iri.span, st, g);
            }
        }
        for p in &n.props {
            let pi = self.predicate(&p.iri, p.key);
            if let Some(pi) = pi {
                self.reference(p.key, pi, Role::Predicate, p.form);
            }
            let st = match (&s, pi) {
                (Some(s), Some(_)) => Some(*stmt.get_or_insert_with(|| self.statement(n, s))),
                _ => None,
            };
            for item in &p.items {
                let o = self.object(item, g, st);
                if let (Some(s), Some(pi), Some(st), Some((o, o_span))) = (&s, pi, st, o) {
                    self.emit(s, pi, o, p.key, o_span, st, g);
                }
            }
        }
        for p in &n.reverse {
            let pi = self.predicate(&p.iri, p.key);
            if let Some(pi) = pi {
                self.reference(p.key, pi, Role::Predicate, p.form);
            }
            for item in &p.items {
                let Item::Node(c) = item else { continue };
                let (cs, cst) = self.node(c, g, Role::Object);
                if let (Some(cs), Some(s), Some(pi)) = (cs, &s, pi) {
                    let cst = match cst {
                        Some(i) => i,
                        None => self.statement(c, &cs),
                    };
                    self.emit(&cs, pi, s.clone(), p.key, n.reference_span(), cst, g);
                }
            }
        }
        for item in &n.included {
            if let Item::Node(c) = item {
                self.node(c, g, Role::Subject);
            }
        }
        if let Some(items) = &n.graph {
            let inner = match &s {
                Some(Node::Iri(i)) => G::Named(GraphName::Iri(*i)),
                Some(Node::Blank(b)) => G::Named(GraphName::Blank(*b)),
                _ => G::Dropped,
            };
            let block = match (&inner, &n.id) {
                (G::Named(name), Some(id)) => {
                    self.out.graphs.push(GraphBlock {
                        span: n.span,
                        name: Some(*name),
                        name_span: Some(id.span),
                        statements: self.out.statements.len() as u32
                            ..self.out.statements.len() as u32,
                    });
                    Some(self.out.graphs.len() - 1)
                }
                _ => None,
            };
            for item in items {
                if let Item::Node(c) = item {
                    self.node(c, inner, Role::Subject);
                }
            }
            if let Some(i) = block {
                self.out.graphs[i].statements.end = self.out.statements.len() as u32;
            }
        }
        (s, stmt)
    }

    /// A predicate: an IRI. A blank node is not one without generalized RDF.
    fn predicate(&mut self, iri: &'e str, key: Span) -> Option<Iri> {
        if iri.starts_with("_:") {
            self.drop(key, DropReason::BlankNodePredicate);
            return None;
        }
        self.iri(iri, Some(key))
    }

    /// The object an item converts to, and its span. Nested nodes are
    /// always converted; with `stmt` `None` (the triple that would hold the
    /// object is dropped), lists produce no triples.
    fn object(&mut self, item: &'e Item<'e>, g: G, stmt: Option<u32>) -> Option<(Node, Span)> {
        match item {
            Item::Node(c) => {
                let (o, _) = self.node(c, g, Role::Object);
                Some((o?, c.reference_span()))
            }
            Item::Value(v) => Some((self.literal(v)?, v.span)),
            Item::List(l) => Some((self.list(l, g, stmt)?, l.span)),
        }
    }

    /// List Conversion (§7.4): the head of an `rdf:first`/`rdf:rest` chain.
    fn list(&mut self, l: &'e ListObject<'e>, g: G, stmt: Option<u32>) -> Option<Node> {
        let Some(stmt) = stmt else {
            for item in &l.items {
                self.object(item, g, None);
            }
            return None;
        };
        if l.items.is_empty() {
            return Some(Node::Iri(self.v.nil));
        }
        let nodes: Vec<Node> = l.items.iter().map(|_| self.fresh()).collect();
        for (i, item) in l.items.iter().enumerate() {
            let span = item.span();
            if let Some((o, _)) = self.object(item, g, Some(stmt)) {
                self.emit(&nodes[i], self.v.first, o, span, span, stmt, g);
            }
            let (rest, rest_span) = match nodes.get(i + 1) {
                Some(n) => (n.clone(), l.items[i + 1].span()),
                None => (Node::Iri(self.v.nil), span),
            };
            self.emit(&nodes[i], self.v.rest, rest, span, rest_span, stmt, g);
        }
        Some(nodes[0].clone())
    }

    /// Object to RDF Conversion (§7.3) of a value object; `None` when its
    /// datatype or language tag is not well-formed.
    fn literal(&mut self, v: &'e ValueObject<'e>) -> Option<Node> {
        let datatype = match &v.datatype {
            Some((dt, span, form)) => {
                let iri = self.iri(dt, *span)?;
                if let Some(span) = span {
                    self.reference(*span, iri, Role::Datatype, *form);
                }
                Some(iri)
            }
            None => None,
        };
        let (value, datatype, lang): (Box<str>, Iri, Option<Iri>) = match v.value {
            Literal::Json(j) => {
                let mut s = String::new();
                jcs(j, &mut s);
                (s.into(), self.v.json, None)
            }
            Literal::Bool(b) => (
                if b { "true" } else { "false" }.into(),
                datatype.unwrap_or(self.v.boolean),
                None,
            ),
            Literal::Number(n) => {
                let f: f64 = n.parse().unwrap_or(f64::NAN);
                if datatype == Some(self.v.double) || f.fract() != 0.0 || f.abs() >= 1e21 {
                    (
                        canonical_double(f).into(),
                        datatype.unwrap_or(self.v.double),
                        None,
                    )
                } else {
                    (
                        canonical_integer(n, f).into(),
                        datatype.unwrap_or(self.v.integer),
                        None,
                    )
                }
            }
            Literal::String(s) => match &v.language {
                Some((tag, span)) if datatype.is_none() => {
                    let lang = self.language(tag, *span)?;
                    (s.into(), self.v.lang_string, Some(lang))
                }
                _ => (s.into(), datatype.unwrap_or(self.v.string), None),
            },
        };
        Some(Node::Literal {
            value,
            datatype,
            lang,
        })
    }

    /// A well-formed language tag, interned in lowercase as Turtle does;
    /// otherwise recorded where it is written.
    fn language(&mut self, tag: &'e str, span: Option<Span>) -> Option<Iri> {
        let lang = match self.langs.get(tag) {
            Some(&hit) => hit,
            None => {
                let lang = oxilangtag::LanguageTag::parse(tag)
                    .ok()
                    .map(|_| Iri::intern(&tag.to_ascii_lowercase()));
                self.langs.insert(tag, lang);
                lang
            }
        };
        if lang.is_none()
            && let Some(span) = span
        {
            self.drop(span, DropReason::InvalidLanguageTag);
        }
        lang
    }
}

/// The canonical `xsd:double` form JSON-LD uses: `5.3E0`, `1.0E21`.
pub fn canonical_double(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "INF" } else { "-INF" }.into();
    }
    let s = format!("{f:E}");
    let (mantissa, exp) = s.split_once('E').unwrap_or((&s, "0"));
    if mantissa.contains('.') {
        s
    } else {
        format!("{mantissa}.0E{exp}")
    }
}

/// The canonical `xsd:integer` form of a JSON number with no fractional
/// part: its digits when written as a plain integer, else from its value.
fn canonical_integer(written: &str, f: f64) -> String {
    let digits = written.strip_prefix('-').unwrap_or(written);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && digits.len() <= 15 {
        let trimmed = digits.trim_start_matches('0');
        return match (trimmed.is_empty(), written.starts_with('-')) {
            (true, _) => "0".into(),
            (false, true) => format!("-{trimmed}"),
            (false, false) => trimmed.into(),
        };
    }
    format!("{}", f as i128)
}

/// RFC 8785 (JCS) serialization, for `@json` literals.
pub fn jcs(v: &Json, out: &mut String) {
    match &v.value {
        Value::Null | Value::Invalid => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&es_number(n.parse().unwrap_or(0.0))),
        Value::String(s) => jcs_string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            let mut members: Vec<_> = o
                .members
                .iter()
                .enumerate()
                .filter(|(i, m)| !o.members[i + 1..].iter().any(|n| n.key == m.key))
                .map(|(_, m)| m)
                .collect();
            members.sort_by(|a, b| a.key.encode_utf16().cmp(b.key.encode_utf16()));
            out.push('{');
            for (i, m) in members.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs_string(&m.key, out);
                out.push(':');
                jcs(&m.value, out);
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
    if f == 0.0 || !f.is_finite() {
        return if f.is_nan() || f == 0.0 { "0" } else { "null" }.into();
    }
    let sign = if f < 0.0 { "-" } else { "" };
    let s = format!("{:e}", f.abs());
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap_or(0) + 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_numbers() {
        assert_eq!(canonical_double(5.3), "5.3E0");
        assert_eq!(canonical_double(1.0), "1.0E0");
        assert_eq!(canonical_double(1e21), "1.0E21");
        assert_eq!(canonical_double(-0.5), "-5.0E-1");
        assert_eq!(canonical_integer("007", 7.0), "7");
        assert_eq!(canonical_integer("-0", -0.0), "0");
        assert_eq!(canonical_integer("1e3", 1e3), "1000");
        assert_eq!(canonical_integer("10.0", 10.0), "10");
        assert_eq!(es_number(333333333.3333333), "333333333.3333333");
        assert_eq!(es_number(1e30), "1e+30");
        assert_eq!(es_number(0.002), "0.002");
        assert_eq!(es_number(-12.0), "-12");
    }

    #[test]
    fn json_canonicalization_sorts_keys_and_escapes() {
        let doc =
            crate::jsonld::json::parse(r#"{"b": [1.0, true, null], "a": "x\u0001\"", "a": "y"}"#);
        let mut s = String::new();
        jcs(&doc.root.unwrap(), &mut s);
        assert_eq!(s, r#"{"a":"y","b":[1,true,null]}"#);
    }
}
