//! SPARQL embedded in SHACL shapes: the query literals of a document
//! (`sh:select`, `sh:ask`, `sh:construct`, `sh:update`), the prefixes the
//! literal's node declares through `sh:prefixes`, and the map between a
//! literal's value and the document text (escapes included).
//!
//! The SHACL-SPARQL lint (`shacl_lint::sparql`) checks these queries. In
//! Turtle and TriG, the SPARQL display and completion features also run on
//! each query's value ([`SparqlAnalysis::embedded`]) and their positions are
//! mapped back: semantic tokens, completion and hover inside the literal.

use crate::analysis::{Analysis, EmbeddedQuery, Encoding, SparqlAnalysis};
use crate::features::semantic::Raw;
use crate::knowledge::Knowledge;
use crate::sparql::features as sparql;
use crate::syntax::{Iri, Node, PrefixMap, Span, Syntax, Triple};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::{Arc, OnceLock};

#[cfg(test)]
thread_local! {
    /// Queries this thread has parsed for the features, to tell a cached one.
    static PARSED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct V {
    select: Iri,
    ask: Iri,
    construct: Iri,
    update: Iri,
    prefixes: Iri,
    declare: Iri,
    imports: Iri,
    prefix: Iri,
    namespace: Iri,
}

fn v() -> &'static V {
    static V: OnceLock<V> = OnceLock::new();
    V.get_or_init(|| {
        let sh = |l: &str| Iri::intern(&format!("{}{l}", crate::shacl::SH));
        V {
            select: sh("select"),
            ask: sh("ask"),
            construct: sh("construct"),
            update: sh("update"),
            prefixes: sh("prefixes"),
            declare: sh("declare"),
            imports: Iri::intern("http://www.w3.org/2002/07/owl#imports"),
            prefix: sh("prefix"),
            namespace: sh("namespace"),
        }
    })
}

/// Whether `p` holds a SELECT or ASK query (the forms SHACL-SPARQL
/// constraints, validators and targets use; the lint checks only these).
pub(crate) fn is_select_or_ask(p: Iri) -> bool {
    p == v().select || p == v().ask
}

/// The document's triples by subject.
pub(crate) struct Index<'a> {
    triples: &'a [Triple],
    by_subject: FxHashMap<&'a Node, Vec<u32>>,
}

impl<'a> Index<'a> {
    pub fn new(triples: &'a [Triple]) -> Self {
        let mut by_subject: FxHashMap<&Node, Vec<u32>> = FxHashMap::default();
        for (i, t) in triples.iter().enumerate() {
            by_subject.entry(&t.s).or_default().push(i as u32);
        }
        Index {
            triples,
            by_subject,
        }
    }

    pub fn objects(&self, s: &Node, p: Iri) -> impl Iterator<Item = &'a Node> + '_ {
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
    triples: &[Triple],
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

/// The query literals of a document.
pub(crate) struct Queries<'a> {
    a: &'a Analysis,
    k: &'a Knowledge,
    /// The triples whose object is a query literal, in document order.
    pub triples: Vec<usize>,
    pub ix: Index<'a>,
    declared: FxHashMap<&'a Node, Vec<(String, String)>>,
}

impl<'a> Queries<'a> {
    /// `None` when the document has no query literal (the common case,
    /// which costs one pass over the triples).
    pub fn new(a: &'a Analysis, k: &'a Knowledge) -> Option<Self> {
        let triples = query_triples(a);
        if triples.is_empty() {
            return None;
        }
        Some(Queries {
            a,
            k,
            triples,
            ix: Index::new(&a.parsed.triples),
            declared: FxHashMap::default(),
        })
    }

    /// The literal of triple `i`.
    pub fn lit(&self, i: usize) -> Option<Lit<'a>> {
        lit_of(self.a, i)
    }

    /// The prefixes the queries of `s` are parsed with, in order (a later
    /// one wins): what its `sh:prefixes` values declare. When one of them is
    /// declared nowhere (projects that keep the declarations outside their
    /// Turtle files), the prefixes in scope in the document follow, so that
    /// prefixed names are not all undeclared.
    pub fn prefixes(&mut self, s: &Node) -> Vec<(String, String)> {
        let Queries {
            a, k, ix, declared, ..
        } = self;
        let mut out = Vec::new();
        let mut unresolved = false;
        for p in ix.objects(s, v().prefixes) {
            let decls = declared.entry(p).or_insert_with(|| declarations(a, k, p));
            unresolved |= decls.is_empty();
            out.extend(decls.iter().cloned());
        }
        if unresolved {
            out.extend(a.external.iter().map(|(n, ns)| (n.clone(), ns.clone())));
            out.extend(
                a.parsed
                    .prefixes
                    .iter()
                    .map(|d| (d.name.to_string(), d.iri.to_string())),
            );
        }
        out
    }

    /// The query of triple `i` as a SPARQL document, with the prefixes of
    /// its node.
    fn embed(&mut self, i: usize) -> Option<Embedded<'a>> {
        let lit = self.lit(i)?;
        let s = &self.a.parsed.triples[i].s;
        let prefixes = self.prefixes(s);
        let key = i as u32;
        let cached = self.a.embedded_queries.lock().ok().and_then(|m| {
            m.get(&key)
                .filter(|e| e.prefixes == prefixes)
                .map(|e| e.query.clone())
        });
        let q = match cached {
            Some(q) => q,
            None => {
                #[cfg(test)]
                PARSED.with(|c| c.set(c.get() + 1));
                let map: PrefixMap = prefixes.iter().cloned().collect();
                let q = Arc::new(SparqlAnalysis::embedded(
                    self.a.uri.clone(),
                    lit.value,
                    Arc::new(map),
                ));
                if let Ok(mut m) = self.a.embedded_queries.lock() {
                    m.insert(
                        key,
                        EmbeddedQuery {
                            prefixes,
                            query: q.clone(),
                        },
                    );
                }
                q
            }
        };
        Some(Embedded { lit, q })
    }
}

/// The triples whose object is a query literal.
fn query_triples(a: &Analysis) -> Vec<usize> {
    let v = v();
    let forms = [v.select, v.ask, v.construct, v.update];
    a.parsed
        .triples
        .iter()
        .enumerate()
        .filter(|(_, t)| forms.contains(&t.p) && matches!(t.o, Node::Literal { .. }))
        .map(|(i, _)| i)
        .collect()
}

fn lit_of(a: &Analysis, i: usize) -> Option<Lit<'_>> {
    let t = &a.parsed.triples[i];
    Some(Lit::new(&a.text, t.o_span, literal(&t.o)?))
}

/// A query literal: its text in the document and its value.
pub(crate) struct Lit<'a> {
    text: &'a str,
    pub span: Span,
    /// Length of the opening quotes.
    pub quote: u32,
    pub value: &'a str,
}

impl<'a> Lit<'a> {
    pub fn new(text: &'a str, span: Span, value: &'a str) -> Self {
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

    /// The raw text after the opening quotes.
    fn raw(&self) -> Option<&'a [u8]> {
        let start = (self.span.start + self.quote) as usize;
        Some(self.text.get(start..self.span.end as usize)?.as_bytes())
    }

    /// The (raw, value) lengths of the character or escape at `raw[r..]`.
    fn step(raw: &[u8], r: usize) -> Option<(usize, usize)> {
        if *raw.get(r)? != b'\\' {
            return Some((1, 1));
        }
        Some(match raw.get(r + 1)? {
            b'u' | b'U' => {
                let n = if raw[r + 1] == b'u' { 4 } else { 8 };
                let hex = std::str::from_utf8(raw.get(r + 2..r + 2 + n)?).ok()?;
                let c = char::from_u32(u32::from_str_radix(hex, 16).ok()?)?;
                (n + 2, c.len_utf8())
            }
            _ => (2, 1),
        })
    }

    /// The document offset of byte `off` of the value, walking the escapes.
    pub fn offset(&self, off: usize) -> Option<u32> {
        let raw = self.raw()?;
        let val = self.value.as_bytes();
        let (mut r, mut d) = (0, 0);
        while d < off {
            let (rl, dl) = Self::step(raw, r)?;
            if rl == 1 && val.get(d) != raw.get(r) {
                return None;
            }
            r += rl;
            d += dl;
        }
        (d == off).then_some(self.span.start + self.quote + r as u32)
    }

    /// The value offset of document offset `doc`, when it is between the
    /// quotes and not inside an escape.
    pub fn value_offset(&self, doc: u32) -> Option<usize> {
        let start = self.span.start + self.quote;
        let want = doc.checked_sub(start)? as usize;
        let raw = self.raw()?;
        let (mut r, mut d) = (0, 0);
        while r < want {
            let (rl, dl) = Self::step(raw, r)?;
            r += rl;
            d += dl;
        }
        (r == want && d <= self.value.len()).then_some(d)
    }

    /// The span of value bytes `from..to`, or the whole literal.
    pub fn span(&self, from: usize, to: usize) -> Span {
        match (self.offset(from), self.offset(to)) {
            (Some(s), Some(e)) if s < e => Span::new(s, e),
            _ => self.span,
        }
    }

    /// Whether document offset `doc` is between the quotes.
    fn holds(&self, doc: u32) -> bool {
        doc >= self.span.start + self.quote
            && doc < self.span.end
            && self.offset(self.value.len()).is_some_and(|end| doc <= end)
    }
}

/// A query literal parsed as SPARQL.
struct Embedded<'a> {
    lit: Lit<'a>,
    q: Arc<SparqlAnalysis>,
}

impl Embedded<'_> {
    /// The range in the document of a range of the query.
    fn range(&self, a: &Analysis, enc: Encoding, r: Range) -> Option<Range> {
        let at = |p: Position| {
            let off = self.q.offset(enc, p)? as usize;
            Some(a.position(enc, self.lit.offset(off)?))
        };
        Some(Range::new(at(r.start)?, at(r.end)?))
    }
}

/// Whether the document can embed queries the editor features map: Turtle
/// and TriG (JSON-LD strings escape differently).
fn embeds(a: &Analysis) -> bool {
    matches!(a.parsed.syntax, Syntax::Turtle | Syntax::TriG)
}

/// The query whose literal holds document offset `offset` between its quotes.
fn at<'a>(a: &'a Analysis, k: &'a Knowledge, offset: u32) -> Option<Embedded<'a>> {
    let i = holding(a, offset)?;
    Queries::new(a, k)?.embed(i)
}

/// The triple whose query literal holds document offset `offset`.
fn holding(a: &Analysis, offset: u32) -> Option<usize> {
    if !embeds(a) {
        return None;
    }
    query_triples(a)
        .into_iter()
        .find(|&i| lit_of(a, i).is_some_and(|l| l.holds(offset)))
}

/// Whether document offset `offset` is inside a query literal.
pub(crate) fn in_query(a: &Analysis, offset: u32) -> bool {
    holding(a, offset).is_some()
}

/// Adds the SPARQL tokens of the queries overlapping `from..to`, as spans of
/// the document text (unsorted).
pub(crate) fn semantic_tokens(a: &Analysis, k: &Knowledge, from: u32, to: u32, out: &mut Vec<Raw>) {
    if !embeds(a) {
        return;
    }
    let Some(mut qs) = Queries::new(a, k) else {
        return;
    };
    for i in qs.triples.clone() {
        let t = &a.parsed.triples[i];
        if t.o_span.end <= from || t.o_span.start > to {
            continue;
        }
        let Some(e) = qs.embed(i) else {
            continue;
        };
        for r in sparql::semantic_tokens_raw(&e.q, k) {
            let (Some(start), Some(end)) =
                (e.lit.offset(r.start as usize), e.lit.offset(r.end as usize))
            else {
                continue;
            };
            if end > from && start <= to && start < end {
                out.push(Raw { start, end, ..r });
            }
        }
    }
}

/// Completion at document offset `offset`, when it is inside a query
/// literal: the SPARQL completion of the query, its edits mapped into the
/// document.
pub(crate) fn completion(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    offset: u32,
) -> Option<CompletionResponse> {
    let e = at(a, k, offset)?;
    let none = CompletionResponse::Array(vec![]);
    let Some(pos) = e
        .lit
        .value_offset(offset)
        .map(|v| e.q.position(enc, v as u32))
    else {
        return Some(none);
    };
    let items = match sparql::completion_at(&e.q, k, enc, pos) {
        Some(CompletionResponse::List(l)) => l,
        Some(CompletionResponse::Array(items)) => CompletionList {
            is_incomplete: false,
            items,
        },
        None => return Some(none),
    };
    let map = |t: &mut TextEdit| -> Option<()> {
        t.range = e.range(a, enc, t.range)?;
        // A short string cannot hold a line break (an added PREFIX line).
        if e.lit.quote == 1 {
            t.new_text = t.new_text.replace('\n', "\\n");
        }
        Some(())
    };
    let mapped = items
        .items
        .into_iter()
        .filter_map(|mut item| {
            match &mut item.text_edit {
                Some(CompletionTextEdit::Edit(t)) => map(t)?,
                Some(CompletionTextEdit::InsertAndReplace(t)) => {
                    t.insert = e.range(a, enc, t.insert)?;
                    t.replace = e.range(a, enc, t.replace)?;
                }
                None => {}
            }
            for t in item.additional_text_edits.iter_mut().flatten() {
                map(t)?;
            }
            Some(item)
        })
        .collect();
    Some(CompletionResponse::List(CompletionList {
        is_incomplete: items.is_incomplete,
        items: mapped,
    }))
}

/// Hover at document offset `offset`, when it is inside a query literal:
/// the SPARQL hover of the query, its range mapped into the document.
pub(crate) fn hover(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    offset: u32,
) -> Option<Option<Hover>> {
    let e = at(a, k, offset)?;
    let Some(v) = e.lit.value_offset(offset) else {
        return Some(None);
    };
    let mut h = sparql::hover(&e.q, k, enc, e.q.position(enc, v as u32));
    if let Some(h) = &mut h {
        h.range = h.range.and_then(|r| e.range(a, enc, r));
    }
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::Source;

    const HEAD: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n";

    fn shapes(queries: usize) -> String {
        let mut text = HEAD.to_string();
        for i in 0..queries {
            text.push_str(&format!(
                "ex:S{i} sh:sparql [ sh:prefixes ex:P ; sh:select \"SELECT $this WHERE {{ $this ex:p{i} ?o }}\" ] .\n"
            ));
        }
        text
    }

    fn analysis(text: &str) -> Arc<Analysis> {
        Arc::new(Analysis::new(
            "file:///w/shapes.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ))
    }

    fn parsed() -> usize {
        PARSED.with(|c| c.get())
    }

    /// Each feature call parsed every query it touched again (500 queries:
    /// a keystroke went from 1.1 to 2.9 ms). A query is parsed once per
    /// document, and again only when its prefixes change.
    #[test]
    fn a_query_literal_is_parsed_once_per_document() {
        let text = shapes(40);
        let a = analysis(&text);
        let mut k = Knowledge::default();
        k.set("file:///w/shapes.ttl", Source::Open, a.clone());
        let before = parsed();
        let mut first = Vec::new();
        semantic_tokens(&a, &k, 0, text.len() as u32, &mut first);
        assert_eq!(parsed() - before, 40);
        assert!(!first.is_empty());
        let mut again = Vec::new();
        semantic_tokens(&a, &k, 0, text.len() as u32, &mut again);
        assert_eq!(parsed() - before, 40, "the second call parsed nothing");
        assert_eq!(first.len(), again.len());
        // Completion and hover at a query take the cached parse too, with
        // the same answer as the first time.
        let at = (text.find("$this ex:p7").unwrap() + "$this ex:p".len()) as u32;
        let a_items = completion(&a, &k, Encoding::Utf16, at).is_some();
        hover(&a, &k, Encoding::Utf16, at);
        assert!(a_items);
        assert_eq!(parsed() - before, 40);
        // Other documents have their own.
        let b = analysis(&text);
        let mut other = Vec::new();
        semantic_tokens(&b, &k, 0, text.len() as u32, &mut other);
        assert_eq!(parsed() - before, 80);
    }

    /// What the workspace declares for a query's `sh:prefixes` node can change
    /// without the document changing; then the query is parsed again.
    #[test]
    fn a_query_is_parsed_again_when_its_prefixes_change() {
        let shape = |ns: &str| {
            format!(
                "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\nex:S sh:sparql [ sh:prefixes ex:P ; sh:select \"SELECT $this WHERE {{ $this my:p ?o }}\" ] .\n{ns}"
            )
        };
        let a = analysis(&shape(""));
        let declarations = Arc::new(Analysis::new(
            "file:///w/decls.ttl".into(),
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\nex:P sh:declare [ sh:prefix \"my\" ; sh:namespace \"http://my.org/\" ] .\n",
            Arc::new(PrefixMap::default()),
            true,
        ));
        let mut k = Knowledge::default();
        k.set("file:///w/shapes.ttl", Source::Open, a.clone());
        let before = parsed();
        let mut out = Vec::new();
        semantic_tokens(&a, &k, 0, u32::MAX, &mut out);
        semantic_tokens(&a, &k, 0, u32::MAX, &mut out);
        assert_eq!(parsed() - before, 1);
        k.set("file:///w/decls.ttl", Source::Disk, declarations);
        semantic_tokens(&a, &k, 0, u32::MAX, &mut out);
        assert_eq!(parsed() - before, 2, "parsed again with the new prefix");
        semantic_tokens(&a, &k, 0, u32::MAX, &mut out);
        assert_eq!(parsed() - before, 2);
    }
}
