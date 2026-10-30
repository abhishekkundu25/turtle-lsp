//! The workspace graph of the `turtle/graph` request: the resources of the
//! workspace documents as nodes, the triples between them as edges, the
//! filters, the cap, the facets the filter panel lists, and a Barnes-Hut
//! force layout.
//!
//! Everything here reads the knowledge index and the documents' parses and
//! changes neither: the request is computed when asked and keeps nothing.
//! [`graph`] validates the parameters once ([`GraphParams`] to [`Query`]);
//! the rest trusts them.
//!
//! How the graph is built, in order:
//!
//! 1. Every triple of every workspace document (imports and bundled
//!    vocabularies are not documents), documents by URI, triples in text
//!    order. A triple whose subject is an IRI or a blank node makes its
//!    subject a node. Its object becomes a node, joined by an edge, when it
//!    is an IRI or a blank node (an `rdf:type` object only with `types`), or
//!    a literal with `literals`: each literal occurrence is its own node.
//!    Triple terms and triples with an undeclared prefix are left out.
//! 2. The facets count that candidate graph, before the other filters, so
//!    the filter panel can list what a filter hides.
//! 3. The filters: the triples of the `files` documents whose predicate
//!    passes `predicates`; the nodes whose kind passes `kinds` and whose
//!    namespace passes `namespaces` (blank nodes and literals have no
//!    namespace and are not filtered by it; literals have no kind filter
//!    either, `literals` decides). A blank node or a literal left without
//!    an edge is dropped: it is only seen through its links. With `focus`,
//!    only the nodes within `depth` hops of it, either direction.
//! 4. The cap: `maxNodes` nodes, in breadth-first order from the focus,
//!    else highest degree first; ties by id. The edges between kept nodes.
//! 5. The layout, starting from the positions the client passes back.

use crate::analysis::{Analysis, Encoding};
use crate::features::hover::short;
use crate::knowledge::{Knowledge, Source, TERM_CLASS, TERM_CONCEPT, TERM_PROPERTY, namespace_of};
use crate::syntax::{BlankId, Iri, Node, PrefixMap, Span, XSD};
use crate::vocab::{TermKind, VOCABS};
use lsp_types::Range;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::collections::hash_map::Entry;
use std::fmt;

/// The most nodes a request can ask for.
pub(crate) const MAX_NODES: u32 = 20_000;
const DEFAULT_MAX_NODES: u32 = 2_000;
/// The most layout iterations a request can ask for: 1,000 on 20,000 nodes
/// is a few seconds.
pub(crate) const MAX_ITERATIONS: u32 = 1_000;
const DEFAULT_ITERATIONS: u32 = 300;
const DEFAULT_DEPTH: u32 = 2;
/// Past this many hops a focus reaches every node it is connected to.
const MAX_DEPTH: u32 = 32;
/// A previous position further out than this is not one the layout made.
const MAX_COORDINATE: f64 = 1e7;
/// A literal's name shows this many characters of its text.
const LITERAL_NAME_CHARS: usize = 60;

const SH: &str = "http://www.w3.org/ns/shacl#";
const SKOS_CONCEPT: &str = "http://www.w3.org/2004/02/skos/core#Concept";
const OWL_RESTRICTION: &str = "http://www.w3.org/2002/07/owl#Restriction";
/// What labels a blank node, as [`Analysis::label`] reads for an IRI.
const LABELS: [&str; 3] = [
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2004/02/skos/core#prefLabel",
    "http://www.w3.org/ns/shacl#name",
];

// ---- the request ----

/// The `turtle/graph` parameters as sent; every field is optional.
/// [`graph`] checks them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct GraphParams {
    /// Document URIs; omitted: every workspace document.
    pub(crate) files: Option<Vec<String>>,
    /// Omitted: every kind.
    pub(crate) kinds: Option<Vec<NodeKind>>,
    pub(crate) namespaces: Filter,
    pub(crate) predicates: Filter,
    pub(crate) literals: bool,
    /// `rdf:type` edges.
    pub(crate) types: bool,
    /// A node id.
    pub(crate) focus: Option<String>,
    /// The term at a document position (Follow cursor); the server resolves
    /// it to `focus` before the graph is drawn (`Session::graph_at`).
    pub(crate) at: Option<crate::features::schema::At>,
    pub(crate) depth: Option<u32>,
    pub(crate) max_nodes: Option<u32>,
    pub(crate) layout: LayoutParams,
}

/// IRIs to keep (omitted: all) and IRIs to leave out.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Filter {
    pub(crate) include: Option<Vec<String>>,
    pub(crate) exclude: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LayoutParams {
    /// Omitted: 300. 0: the start positions, unchanged.
    pub(crate) iterations: Option<u32>,
    /// Node id -> where the client drew it.
    pub(crate) positions: FxHashMap<String, [f64; 2]>,
}

/// What a node is, from its types and how the workspace uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NodeKind {
    Class,
    Property,
    Shape,
    Concept,
    /// Typed with something that is none of the above.
    Individual,
    Other,
}

impl NodeKind {
    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// Parameters [`graph`] cannot answer, for an `InvalidParams` error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InvalidParams(pub(crate) String);

impl fmt::Display for InvalidParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// [`GraphParams`] checked, with the defaults and limits applied.
struct Query {
    files: Option<FxHashSet<String>>,
    /// [`NodeKind::bit`]s.
    kinds: u8,
    /// Sets of IRIs; an IRI no document uses is in no set and matches nothing.
    namespaces: Sets,
    predicates: Sets,
    literals: bool,
    types: bool,
    focus: Option<(String, u32)>,
    max_nodes: usize,
    iterations: u32,
    positions: FxHashMap<String, [f64; 2]>,
}

#[derive(Default)]
struct Sets {
    include: Option<FxHashSet<String>>,
    exclude: FxHashSet<String>,
}

impl Sets {
    fn new(f: Filter) -> Sets {
        Sets {
            include: f.include.map(|v| v.into_iter().collect()),
            exclude: f.exclude.into_iter().collect(),
        }
    }

    fn pass(&self, iri: &str) -> bool {
        self.include.as_ref().is_none_or(|s| s.contains(iri)) && !self.exclude.contains(iri)
    }
}

impl Query {
    fn new(p: GraphParams) -> Result<Query, InvalidParams> {
        let focus = match p.focus {
            Some(f) if f.is_empty() => {
                return Err(InvalidParams("focus is an empty node id".into()));
            }
            Some(f) => Some((f, p.depth.unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH))),
            None => None,
        };
        for (id, [x, y]) in &p.layout.positions {
            if !(x.is_finite() && y.is_finite())
                || x.abs() > MAX_COORDINATE
                || y.abs() > MAX_COORDINATE
            {
                return Err(InvalidParams(format!(
                    "layout.positions[{id:?}] is not a coordinate within ±{MAX_COORDINATE}"
                )));
            }
        }
        let kinds = match p.kinds {
            Some(ks) => ks.iter().fold(0, |bits, k| bits | k.bit()),
            None => u8::MAX,
        };
        Ok(Query {
            files: p.files.map(|v| v.into_iter().collect()),
            kinds,
            namespaces: Sets::new(p.namespaces),
            predicates: Sets::new(p.predicates),
            literals: p.literals,
            types: p.types,
            focus,
            max_nodes: p.max_nodes.unwrap_or(DEFAULT_MAX_NODES).min(MAX_NODES) as usize,
            iterations: p
                .layout
                .iterations
                .unwrap_or(DEFAULT_ITERATIONS)
                .min(MAX_ITERATIONS),
            positions: p.layout.positions,
        })
    }
}

// ---- the answer ----

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct GraphResult {
    pub(crate) nodes: Vec<GraphNode>,
    pub(crate) edges: Vec<GraphEdge>,
    /// `x, y` of each node, in the order of `nodes`.
    pub(crate) positions: Vec<f64>,
    /// After the filters, before the cap.
    pub(crate) total: Total,
    pub(crate) facets: Facets,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct GraphNode {
    /// The IRI; `_:<document>:<n>` for a blank node of the document with
    /// that index among the workspace documents by URI; `"<text>"#<document>:<triple>`
    /// for a literal.
    pub(crate) id: String,
    /// `rdfs:label`, `skos:prefLabel` or `sh:name`, as hover picks it.
    pub(crate) label: Option<String>,
    /// A prefixed name, else `<iri>`; a blank node's `_:b<n>`; a literal as
    /// written.
    pub(crate) name: String,
    pub(crate) kind: NodeKind,
    /// Where the node is described, else where it is first mentioned.
    pub(crate) uri: String,
    pub(crate) range: Range,
    pub(crate) literal: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct GraphEdge {
    /// Indexes into `nodes`.
    pub(crate) source: u32,
    pub(crate) target: u32,
    pub(crate) predicate: String,
    pub(crate) name: String,
    /// The first document, by URI, that states the triple.
    pub(crate) uri: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Total {
    pub(crate) nodes: usize,
    pub(crate) edges: usize,
}

/// Counts of the graph before the filters (see the module doc), most first.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Facets {
    pub(crate) files: Vec<FileFacet>,
    pub(crate) namespaces: Vec<NamespaceFacet>,
    pub(crate) predicates: Vec<PredicateFacet>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct FileFacet {
    pub(crate) uri: String,
    /// The nodes the document's triples mention.
    pub(crate) nodes: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct NamespaceFacet {
    pub(crate) iri: String,
    pub(crate) prefix: Option<String>,
    pub(crate) nodes: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct PredicateFacet {
    pub(crate) iri: String,
    pub(crate) name: String,
    pub(crate) edges: usize,
}

/// `turtle/graph`: the workspace graph that `params` asks for, laid out.
/// Positions and ranges are in `enc`. The documents are those of `k` that
/// are not external sources.
pub(crate) fn graph(
    k: &Knowledge,
    enc: Encoding,
    params: GraphParams,
) -> Result<GraphResult, InvalidParams> {
    let q = Query::new(params)?;
    Ok(answer(k, enc, &q))
}

fn answer(k: &Knowledge, enc: Encoding, q: &Query) -> GraphResult {
    let mut docs: Vec<&Analysis> = k
        .sources()
        .filter(|(_, s, _)| *s != Source::External)
        .map(|(_, _, a)| &**a)
        .collect();
    docs.sort_by(|a, b| a.uri.cmp(&b.uri));
    let names = Names::new(&docs);
    let g = Candidates::walk(&docs, q);
    let kinds: Vec<NodeKind> = g.nodes.iter().map(|n| g.kind(k, n)).collect();
    let facets = g.facets(&names);
    let (nodes, edges) = g.filter(q, &kinds);
    let nodes = g.focused(q, nodes, &edges);
    let total = Total {
        nodes: nodes.len(),
        edges: edges
            .iter()
            .filter(|e| nodes.contains_key(&e.s) && nodes.contains_key(&e.o))
            .count(),
    };
    let kept = g.capped(q, nodes, &edges);
    let at: FxHashMap<u32, u32> = kept
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i as u32))
        .collect();
    let edges: Vec<GraphEdge> = edges
        .iter()
        .filter_map(|e| {
            Some(GraphEdge {
                source: *at.get(&e.s)?,
                target: *at.get(&e.o)?,
                predicate: e.p.to_string(),
                name: names.short(&e.p),
                uri: docs[e.doc as usize].uri.clone(),
            })
        })
        .collect();
    let nodes: Vec<GraphNode> = kept
        .iter()
        .map(|&n| g.output(k, enc, &names, n, kinds[n as usize]))
        .collect();
    let links: Vec<(u32, u32)> = edges.iter().map(|e| (e.source, e.target)).collect();
    let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let positions = layout(&ids, &links, &q.positions, q.iterations);
    GraphResult {
        nodes,
        edges,
        positions,
        total,
        facets,
    }
}

// ---- names ----

/// The prefixes of every workspace document, the first document by URI
/// winning a name two bind differently.
struct Names {
    prefixes: PrefixMap,
    /// Namespace -> its shortest prefix name.
    by_namespace: FxHashMap<String, String>,
}

impl Names {
    fn new(docs: &[&Analysis]) -> Names {
        let mut prefixes = PrefixMap::default();
        for a in docs {
            for (name, ns) in a.prefix_map() {
                prefixes.entry(name).or_insert(ns);
            }
        }
        let mut by_namespace: FxHashMap<String, String> = FxHashMap::default();
        for (name, ns) in &prefixes {
            let better = by_namespace
                .get(ns)
                .is_none_or(|n| (name.len(), name) < (n.len(), n));
            if better {
                by_namespace.insert(ns.clone(), name.clone());
            }
        }
        Names {
            prefixes,
            by_namespace,
        }
    }

    fn short(&self, iri: &str) -> String {
        short(iri, &self.prefixes)
    }

    fn prefix(&self, ns: &str) -> Option<String> {
        self.by_namespace
            .get(ns)
            .cloned()
            .or_else(|| VOCABS.by_namespace(ns).map(|v| v.prefix.to_string()))
    }
}

// ---- the candidate graph ----

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Iri(Iri),
    /// The document's index, the node.
    Blank(u32, BlankId),
    /// The document's index, the triple's.
    Literal(u32, u32),
}

impl Key {
    fn id(self, docs: &[&Analysis]) -> String {
        match self {
            Key::Iri(i) => i.to_string(),
            Key::Blank(d, b) => format!("_:{d}:{}", b.0),
            Key::Literal(d, t) => format!("\"{}\"#{d}:{t}", literal(docs, d, t).0),
        }
    }

    /// The key of a node id, the inverse of [`Key::id`].
    fn of(id: &str) -> Option<Key> {
        if let Some(rest) = id.strip_prefix("_:") {
            let (d, b) = rest.split_once(':')?;
            return Some(Key::Blank(d.parse().ok()?, BlankId(b.parse().ok()?)));
        }
        if id.starts_with('"') {
            let (d, t) = id.rsplit_once('#')?.1.split_once(':')?;
            return Some(Key::Literal(d.parse().ok()?, t.parse().ok()?));
        }
        Iri::get(id).map(Key::Iri)
    }
}

/// The literal object of triple `t` of document `d`: text, datatype, language.
fn literal<'a>(docs: &[&'a Analysis], d: u32, t: u32) -> (&'a str, Iri, Option<Iri>) {
    match &docs[d as usize].parsed.triples[t as usize].o {
        Node::Literal {
            value,
            datatype,
            lang,
            ..
        } => (value, *datatype, *lang),
        _ => unreachable!("a literal key names a literal object"),
    }
}

struct Vertex<'a> {
    key: Key,
    /// The first document, by URI, that mentions the node.
    doc: u32,
    /// Blank nodes and literals: where that document first writes it, an
    /// object's span preferred over its statement's.
    span: Span,
    seen_as_object: bool,
    /// Blank nodes: their types and their label (English or untagged
    /// preferred).
    types: Vec<Iri>,
    label: Option<(&'a str, bool)>,
}

/// A triple, as nodes. Literal and `rdf:type` triples that make no edge
/// still say their subject is there: their object is [`NONE`].
struct Fact {
    doc: u32,
    s: u32,
    p: Iri,
    o: u32,
}

const NONE: u32 = u32::MAX;

/// An edge after the filters.
#[derive(Clone, Copy)]
struct Link {
    doc: u32,
    s: u32,
    p: Iri,
    o: u32,
}

struct Candidates<'a> {
    docs: &'a [&'a Analysis],
    nodes: Vec<Vertex<'a>>,
    index: FxHashMap<Key, u32>,
    facts: Vec<Fact>,
}

impl<'a> Candidates<'a> {
    fn walk(docs: &'a [&'a Analysis], q: &Query) -> Candidates<'a> {
        let rdf_type = crate::shacl::vocab::get().rdf_type;
        let labels: Vec<Iri> = LABELS.iter().filter_map(|l| Iri::get(l)).collect();
        let mut g = Candidates {
            docs,
            nodes: Vec::new(),
            index: FxHashMap::default(),
            facts: Vec::new(),
        };
        for (d, a) in docs.iter().enumerate() {
            let d = d as u32;
            let p = &a.parsed;
            for (ti, t) in p.triples.iter().enumerate() {
                if t.unresolved {
                    continue;
                }
                let key = match t.s {
                    Node::Iri(i) => Key::Iri(i),
                    Node::Blank(b) => Key::Blank(d, b),
                    _ => continue,
                };
                let at = p
                    .statements
                    .get(t.statement as usize)
                    .map_or(t.p_span, |s| s.span);
                let s = g.vertex(key, d, at, false);
                let o = match &t.o {
                    Node::Iri(i) if t.p != rdf_type || q.types => {
                        g.vertex(Key::Iri(*i), d, t.o_span, true)
                    }
                    Node::Blank(b) if t.p != rdf_type || q.types => {
                        g.vertex(Key::Blank(d, *b), d, t.o_span, true)
                    }
                    Node::Literal { .. } if q.literals => {
                        g.vertex(Key::Literal(d, ti as u32), d, t.o_span, true)
                    }
                    _ => NONE,
                };
                if let Key::Blank(..) = key {
                    let v = &mut g.nodes[s as usize];
                    match &t.o {
                        Node::Iri(c) if t.p == rdf_type => v.types.push(*c),
                        Node::Literal { value, lang, .. } if labels.contains(&t.p) => {
                            let en = crate::analysis::is_english(*lang);
                            if v.label.is_none_or(|(_, was)| en && !was) {
                                v.label = Some((value, en));
                            }
                        }
                        _ => {}
                    }
                }
                g.facts.push(Fact {
                    doc: d,
                    s,
                    p: t.p,
                    o,
                });
            }
        }
        g
    }

    fn vertex(&mut self, key: Key, doc: u32, span: Span, as_object: bool) -> u32 {
        if let Some(&i) = self.index.get(&key) {
            let v = &mut self.nodes[i as usize];
            if as_object && !v.seen_as_object && v.doc == doc {
                v.span = span;
                v.seen_as_object = true;
            }
            return i;
        }
        let i = self.nodes.len() as u32;
        self.index.insert(key, i);
        self.nodes.push(Vertex {
            key,
            doc,
            span,
            seen_as_object: as_object,
            types: Vec::new(),
            label: None,
        });
        i
    }

    fn kind(&self, k: &Knowledge, v: &Vertex) -> NodeKind {
        match v.key {
            Key::Iri(i) => iri_kind(k, i),
            Key::Blank(..) => kind_of_types(&v.types).unwrap_or(NodeKind::Other),
            Key::Literal(..) => NodeKind::Other,
        }
    }

    fn facets(&self, names: &Names) -> Facets {
        let mut files = Vec::new();
        let mut stamp = vec![NONE; self.nodes.len()];
        let mut count = 0;
        let mut doc = NONE;
        let mut flush = |doc: u32, count: usize| {
            if doc != NONE && count > 0 {
                files.push(FileFacet {
                    uri: self.docs[doc as usize].uri.clone(),
                    nodes: count,
                });
            }
        };
        for f in &self.facts {
            if f.doc != doc {
                flush(doc, count);
                (doc, count) = (f.doc, 0);
            }
            for n in [f.s, f.o] {
                if n != NONE && stamp[n as usize] != doc {
                    stamp[n as usize] = doc;
                    count += 1;
                }
            }
        }
        flush(doc, count);

        let mut namespaces: FxHashMap<&str, usize> = FxHashMap::default();
        for v in &self.nodes {
            if let Key::Iri(i) = v.key {
                *namespaces.entry(namespace_of(i.as_str())).or_default() += 1;
            }
        }
        let mut namespaces: Vec<NamespaceFacet> = namespaces
            .into_iter()
            .map(|(ns, nodes)| NamespaceFacet {
                iri: ns.to_string(),
                prefix: names.prefix(ns),
                nodes,
            })
            .collect();
        namespaces.sort_by(|a, b| b.nodes.cmp(&a.nodes).then_with(|| a.iri.cmp(&b.iri)));

        let mut seen: FxHashSet<(u32, Iri, u32)> = FxHashSet::default();
        let mut predicates: FxHashMap<Iri, usize> = FxHashMap::default();
        for f in self.facts.iter().filter(|f| f.o != NONE) {
            if seen.insert((f.s, f.p, f.o)) {
                *predicates.entry(f.p).or_default() += 1;
            }
        }
        let mut predicates: Vec<PredicateFacet> = predicates
            .into_iter()
            .map(|(p, edges)| PredicateFacet {
                iri: p.to_string(),
                name: names.short(&p),
                edges,
            })
            .collect();
        predicates.sort_by(|a, b| b.edges.cmp(&a.edges).then_with(|| a.iri.cmp(&b.iri)));
        Facets {
            files,
            namespaces,
            predicates,
        }
    }

    /// The nodes and edges that pass the filters (not the focus), each
    /// node with its degree.
    fn filter(&self, q: &Query, kinds: &[NodeKind]) -> (FxHashMap<u32, u32>, Vec<Link>) {
        let docs: Vec<bool> = self
            .docs
            .iter()
            .map(|a| q.files.as_ref().is_none_or(|f| f.contains(&a.uri)))
            .collect();
        let node_ok: Vec<bool> = self
            .nodes
            .iter()
            .zip(kinds)
            .map(|(v, kind)| match v.key {
                Key::Iri(i) => {
                    q.kinds & kind.bit() != 0 && q.namespaces.pass(namespace_of(i.as_str()))
                }
                Key::Blank(..) => q.kinds & kind.bit() != 0,
                Key::Literal(..) => true,
            })
            .collect();
        let mut degree: FxHashMap<u32, u32> = FxHashMap::default();
        let mut edges = Vec::new();
        let mut seen: FxHashSet<(u32, Iri, u32)> = FxHashSet::default();
        for f in &self.facts {
            if !docs[f.doc as usize] || !node_ok[f.s as usize] || !q.predicates.pass(&f.p) {
                continue;
            }
            degree.entry(f.s).or_default();
            if f.o == NONE || !node_ok[f.o as usize] || !seen.insert((f.s, f.p, f.o)) {
                continue;
            }
            edges.push(Link {
                doc: f.doc,
                s: f.s,
                p: f.p,
                o: f.o,
            });
            *degree.entry(f.o).or_default() += 1;
            *degree.get_mut(&f.s).expect("counted") += 1;
        }
        degree.retain(|&n, d| *d > 0 || matches!(self.nodes[n as usize].key, Key::Iri(_)));
        (degree, edges)
    }

    /// The nodes within the focus's depth, when there is a focus.
    fn focused(
        &self,
        q: &Query,
        nodes: FxHashMap<u32, u32>,
        edges: &[Link],
    ) -> FxHashMap<u32, u32> {
        let Some((focus, depth)) = &q.focus else {
            return nodes;
        };
        let Some(start) = self.find(focus).filter(|n| nodes.contains_key(n)) else {
            return FxHashMap::default();
        };
        let near = bfs(start, *depth, &adjacency(edges));
        nodes
            .into_iter()
            .filter(|(n, _)| near.contains_key(n))
            .collect()
    }

    fn find(&self, id: &str) -> Option<u32> {
        self.index.get(&Key::of(id)?).copied()
    }

    /// At most `q.max_nodes` of `nodes`: breadth-first from the focus,
    /// else by degree; each level by degree; ties by id.
    fn capped(&self, q: &Query, nodes: FxHashMap<u32, u32>, edges: &[Link]) -> Vec<u32> {
        let mut order: Vec<(u32, u32, u32, String)> = match &q.focus {
            Some((focus, depth)) => {
                let start = self.find(focus);
                let edges: Vec<Link> = edges
                    .iter()
                    .filter(|e| nodes.contains_key(&e.s) && nodes.contains_key(&e.o))
                    .copied()
                    .collect();
                let hops = start.map(|s| bfs(s, *depth, &adjacency(&edges)));
                nodes
                    .iter()
                    .map(|(&n, &d)| {
                        let hop = hops.as_ref().and_then(|h| h.get(&n)).copied();
                        (
                            hop.unwrap_or(u32::MAX),
                            d,
                            n,
                            self.nodes[n as usize].key.id(self.docs),
                        )
                    })
                    .collect()
            }
            None => nodes
                .iter()
                .map(|(&n, &d)| (0, d, n, self.nodes[n as usize].key.id(self.docs)))
                .collect(),
        };
        order.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(b.1.cmp(&a.1))
                .then_with(|| a.3.cmp(&b.3))
        });
        order.truncate(q.max_nodes);
        order.into_iter().map(|(_, _, n, _)| n).collect()
    }

    fn output(
        &self,
        k: &Knowledge,
        enc: Encoding,
        names: &Names,
        n: u32,
        kind: NodeKind,
    ) -> GraphNode {
        let v = &self.nodes[n as usize];
        let id = v.key.id(self.docs);
        let here = self.docs[v.doc as usize];
        let (label, name, (a, span), literal) = match v.key {
            Key::Iri(i) => {
                let described = k
                    .descriptions(&i)
                    .into_iter()
                    .find(|d| d.source != Source::External)
                    .and_then(|d| Some((d.analysis, *d.spans.first()?)));
                let at = described
                    .unwrap_or_else(|| (here, here.refs_of(&i).next().map_or(v.span, |r| r.span)));
                (label(k, i), names.short(&i), at, false)
            }
            Key::Blank(_, b) => (
                v.label.map(|(l, _)| l.to_string()),
                format!("_:b{}", b.0),
                (here, v.span),
                false,
            ),
            Key::Literal(d, t) => {
                let (text, datatype, lang) = literal(self.docs, d, t);
                let mut name: String = text.chars().take(LITERAL_NAME_CHARS).collect();
                if name.len() < text.len() {
                    name.push('…');
                }
                let mut name = format!("\"{name}\"");
                if let Some(l) = lang {
                    name.push_str(&format!("@{l}"));
                } else if datatype.as_str() != format!("{XSD}string") {
                    name.push_str(&format!("^^{}", names.short(&datatype)));
                }
                (None, name, (here, v.span), true)
            }
        };
        GraphNode {
            id,
            label,
            name,
            kind,
            uri: a.uri.clone(),
            range: a.range(enc, span),
            literal,
        }
    }
}

/// The label hover shows: the well-known vocabulary's, else the workspace's.
fn label(k: &Knowledge, iri: Iri) -> Option<String> {
    VOCABS
        .lookup(&iri)
        .and_then(|(v, local)| v.term(local))
        .map(|t| t.label.clone())
        .filter(|l| !l.is_empty())
        .or_else(|| k.label(&iri).map(str::to_string))
}

/// What a set of types makes a node, when they say.
fn kind_of_types(types: &[Iri]) -> Option<NodeKind> {
    let any = |f: &dyn Fn(&str) -> bool| types.iter().any(|t| f(t.as_str()));
    if any(&|t| t.starts_with(SH) && t.ends_with("Shape")) {
        Some(NodeKind::Shape)
    } else if any(&|t| t.ends_with("Property")) {
        Some(NodeKind::Property)
    } else if any(&|t| t.ends_with("Class") || t == OWL_RESTRICTION) {
        Some(NodeKind::Class)
    } else if any(&|t| t == SKOS_CONCEPT) {
        Some(NodeKind::Concept)
    } else if !types.is_empty() {
        Some(NodeKind::Individual)
    } else {
        None
    }
}

/// An IRI's kind: a shape by its types; else the term kind the workspace
/// gives it ([`Knowledge::term_kind`]), property before class before
/// concept; else the well-known vocabulary's; else its other types; else
/// how the workspace uses it (a predicate is a property; what has
/// instances, sub or super classes is a class).
fn iri_kind(k: &Knowledge, iri: Iri) -> NodeKind {
    let types = k.types(&iri);
    if kind_of_types(&types) == Some(NodeKind::Shape) {
        return NodeKind::Shape;
    }
    let bits = k.term_kind(&iri);
    if bits & TERM_PROPERTY != 0 {
        return NodeKind::Property;
    }
    if bits & TERM_CLASS != 0 {
        return NodeKind::Class;
    }
    if bits & TERM_CONCEPT != 0 {
        return NodeKind::Concept;
    }
    if let Some(t) = VOCABS.lookup(&iri).and_then(|(v, local)| v.term(local)) {
        return match t.kind {
            TermKind::Class => NodeKind::Class,
            TermKind::Property => NodeKind::Property,
            TermKind::Individual => NodeKind::Individual,
        };
    }
    if let Some(kind) = kind_of_types(&types) {
        return kind;
    }
    if k.used_as_predicate(&iri) {
        return NodeKind::Property;
    }
    if !k.instances(&iri).is_empty() || !k.subs(&iri).is_empty() || !k.supers(&iri).is_empty() {
        return NodeKind::Class;
    }
    NodeKind::Other
}

fn adjacency(edges: &[Link]) -> FxHashMap<u32, Vec<u32>> {
    let mut adj: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    for e in edges {
        adj.entry(e.s).or_default().push(e.o);
        adj.entry(e.o).or_default().push(e.s);
    }
    adj
}

/// Node -> hops from `start`, for the nodes within `depth`.
fn bfs(start: u32, depth: u32, adj: &FxHashMap<u32, Vec<u32>>) -> FxHashMap<u32, u32> {
    let mut hops = FxHashMap::default();
    hops.insert(start, 0);
    let mut queue = VecDeque::from([start]);
    while let Some(n) = queue.pop_front() {
        let h = hops[&n];
        if h == depth {
            continue;
        }
        for &m in adj.get(&n).into_iter().flatten() {
            if let Entry::Vacant(e) = hops.entry(m) {
                e.insert(h + 1);
                queue.push_back(m);
            }
        }
    }
    hops
}

// ---- layout ----

/// The edge length the layout settles on.
const EDGE: f64 = 30.0;
/// Barnes-Hut: a cell this small for its distance counts as one body.
const THETA: f64 = 0.9;
/// Pull toward the origin per unit of distance, which keeps components
/// that share no edge from drifting apart.
const GRAVITY: f64 = 0.3;
/// How far a node moves per unit of force, divided by one more than its
/// edges: below what makes it overshoot.
const STEP: f64 = 0.5;
/// From this many nodes the repulsion is summed on several threads.
const PARALLEL: usize = 2_000;
/// Below this squared distance two bodies push nothing (one body and itself,
/// or two on the same spot).
const NEAR: f64 = 1e-9;
/// Past this depth a quadtree cell keeps every body that reaches it.
const MAX_TREE_DEPTH: u32 = 48;

/// `x, y` per node, rounded to a tenth. A node starts where `given` puts it,
/// else near a neighbour that has a given position, else at a point the
/// hash of its id picks; then `iterations` steps of a force layout
/// (Fruchterman-Reingold forces, Barnes-Hut repulsion, linear cooling), each
/// node moving in proportion to its force.
/// Each step moves a node at most the temperature, which starts lower the
/// more nodes kept their position (a fiftieth when all did), so a graph
/// drawn before stays put.
fn layout(
    ids: &[&str],
    edges: &[(u32, u32)],
    given: &FxHashMap<String, [f64; 2]>,
    iterations: u32,
) -> Vec<f64> {
    let n = ids.len();
    let mut pos: Vec<[f64; 2]> = vec![[0.0; 2]; n];
    let mut placed: Vec<bool> = vec![false; n];
    for (i, id) in ids.iter().enumerate() {
        if let Some(&p) = given.get(*id) {
            pos[i] = p;
            placed[i] = true;
        }
    }
    let kept = placed.iter().filter(|p| **p).count();
    let mut near: Vec<Option<u32>> = vec![None; n];
    for &(s, t) in edges {
        let (s, t) = (s as usize, t as usize);
        if placed[t] && !placed[s] && near[s].is_none() {
            near[s] = Some(t as u32);
        }
        if placed[s] && !placed[t] && near[t].is_none() {
            near[t] = Some(s as u32);
        }
    }
    let radius = EDGE * (n as f64).sqrt();
    for i in 0..n {
        if placed[i] {
            continue;
        }
        let h = fnv(ids[i]);
        let (a, b) = (unit(h), unit(mix(h)));
        let angle = a * std::f64::consts::TAU;
        pos[i] = match near[i] {
            Some(j) => {
                let [x, y] = pos[j as usize];
                [x + EDGE * angle.cos(), y + EDGE * angle.sin()]
            }
            None => {
                let r = radius * b.sqrt();
                [r * angle.cos(), r * angle.sin()]
            }
        };
    }

    // Bodies on one spot push nothing: part them by a hair.
    if iterations > 0 {
        for (p, id) in pos.iter_mut().zip(ids) {
            let h = mix(fnv(id) ^ 0x9e37_79b9_7f4a_7c15);
            p[0] += (unit(h) - 0.5) * 0.02;
            p[1] += (unit(mix(h)) - 0.5) * 0.02;
        }
    }
    let fresh = if n == 0 {
        0.0
    } else {
        (n - kept) as f64 / n as f64
    };
    let start = EDGE * (0.2 + 9.8 * fresh);
    let mut tree = Tree::default();
    let mut force: Vec<[f64; 2]> = vec![[0.0; 2]; n];
    let mut stack = Vec::new();
    // Each edge stiffens a node: a smaller step keeps a hub from
    // overshooting where its edges balance.
    let mut rates = vec![1.0; n];
    for &(s, t) in edges {
        rates[s as usize] += 1.0;
        rates[t as usize] += 1.0;
    }
    for r in &mut rates {
        *r = STEP / *r;
    }
    for step in 0..iterations {
        tree.build(&pos);
        // Each body's push is its own sum: the same in any thread.
        if n >= PARALLEL {
            force
                .par_iter_mut()
                .zip(&pos)
                .for_each_init(Vec::new, |stack, (f, &p)| *f = tree.repulsion(p, stack));
        } else {
            for (f, &p) in force.iter_mut().zip(&pos) {
                *f = tree.repulsion(p, &mut stack);
            }
        }
        for &(s, t) in edges {
            let (s, t) = (s as usize, t as usize);
            let (dx, dy) = (pos[t][0] - pos[s][0], pos[t][1] - pos[s][1]);
            let d = (dx * dx + dy * dy).sqrt();
            // Attraction d² / EDGE along the edge.
            let (fx, fy) = (dx * d / EDGE, dy * d / EDGE);
            force[s][0] += fx;
            force[s][1] += fy;
            force[t][0] -= fx;
            force[t][1] -= fy;
        }
        let temperature = start * (1.0 - step as f64 / iterations as f64) + 0.01 * EDGE;
        for ((p, f), &rate) in pos.iter_mut().zip(&force).zip(&rates) {
            let (fx, fy) = (f[0] - GRAVITY * p[0], f[1] - GRAVITY * p[1]);
            let len = (fx * fx + fy * fy).sqrt();
            if len > 0.0 {
                let step = (len * rate).min(temperature) / len;
                p[0] += fx * step;
                p[1] += fy * step;
            }
        }
    }
    pos.iter()
        .flat_map(|[x, y]| [(x * 10.0).round() / 10.0, (y * 10.0).round() / 10.0])
        .collect()
}

/// FNV-1a: the same on every platform, the browser build's included.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// SplitMix64's finaliser: a second, independent number from a hash.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A hash as a number in `[0, 1)`.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// A quadtree of the bodies, each cell with its bodies' count and the sum of
/// their positions.
#[derive(Default)]
struct Tree {
    cells: Vec<Cell>,
}

#[derive(Clone, Copy)]
struct Cell {
    /// Centre and half the side.
    x: f64,
    y: f64,
    half: f64,
    sx: f64,
    sy: f64,
    count: u32,
    /// Index into `cells` of each quadrant's cell; 0 for none (the root is
    /// no one's child).
    children: [u32; 4],
    leaf: bool,
    /// A leaf's body; past [`MAX_TREE_DEPTH`] the others are only counted.
    body: [f64; 2],
}

impl Cell {
    fn new(x: f64, y: f64, half: f64) -> Cell {
        Cell {
            x,
            y,
            half,
            sx: 0.0,
            sy: 0.0,
            count: 0,
            children: [0; 4],
            leaf: true,
            body: [0.0; 2],
        }
    }

    fn quadrant(&self, [x, y]: [f64; 2]) -> usize {
        usize::from(x >= self.x) | usize::from(y >= self.y) << 1
    }
}

impl Tree {
    fn build(&mut self, pos: &[[f64; 2]]) {
        self.cells.clear();
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in pos {
            for a in 0..2 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        if pos.is_empty() {
            return;
        }
        let half = (hi[0] - lo[0]).max(hi[1] - lo[1]) / 2.0 + 1.0;
        self.cells.push(Cell::new(
            (lo[0] + hi[0]) / 2.0,
            (lo[1] + hi[1]) / 2.0,
            half,
        ));
        for &p in pos {
            self.insert(p);
        }
    }

    fn insert(&mut self, p: [f64; 2]) {
        let mut c = 0;
        let mut depth = 0;
        loop {
            let cell = &mut self.cells[c];
            cell.count += 1;
            cell.sx += p[0];
            cell.sy += p[1];
            if cell.leaf {
                if cell.count == 1 {
                    cell.body = p;
                    return;
                }
                if depth >= MAX_TREE_DEPTH {
                    return;
                }
                // Split: the body already here moves down a level.
                let old = cell.body;
                cell.leaf = false;
                let q = cell.quadrant(old);
                let child = self.child(c, q);
                let moved = &mut self.cells[child];
                moved.count = 1;
                (moved.sx, moved.sy) = (old[0], old[1]);
                moved.body = old;
            }
            let q = self.cells[c].quadrant(p);
            c = self.child(c, q);
            depth += 1;
        }
    }

    /// The cell of quadrant `q` of cell `c`, made empty if there is none.
    fn child(&mut self, c: usize, q: usize) -> usize {
        let at = self.cells[c].children[q];
        if at != 0 {
            return at as usize;
        }
        let parent = self.cells[c];
        let h = parent.half / 2.0;
        let x = if q & 1 == 1 {
            parent.x + h
        } else {
            parent.x - h
        };
        let y = if q & 2 == 2 {
            parent.y + h
        } else {
            parent.y - h
        };
        self.cells.push(Cell::new(x, y, h));
        let i = self.cells.len() - 1;
        self.cells[c].children[q] = i as u32;
        i
    }

    /// The push on a body at `p` from all the others: EDGE² / d each, a far
    /// cell's bodies counted as one at their centre.
    fn repulsion(&self, p: [f64; 2], stack: &mut Vec<u32>) -> [f64; 2] {
        let mut f = [0.0; 2];
        if self.cells.is_empty() {
            return f;
        }
        stack.clear();
        stack.push(0);
        while let Some(c) = stack.pop() {
            let cell = &self.cells[c as usize];
            if cell.count == 0 {
                continue;
            }
            let m = f64::from(cell.count);
            let (dx, dy) = (p[0] - cell.sx / m, p[1] - cell.sy / m);
            let d2 = dx * dx + dy * dy;
            if cell.leaf || 4.0 * cell.half * cell.half < THETA * THETA * d2 {
                if d2 > NEAR {
                    let push = EDGE * EDGE * m / d2;
                    f[0] += dx * push;
                    f[1] += dy * push;
                }
            } else {
                stack.extend(cell.children.iter().filter(|&&ch| ch != 0));
            }
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;
    use std::sync::Arc;

    const HEAD: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\
        @prefix sh: <http://www.w3.org/ns/shacl#> .\n";

    fn knowledge(docs: &[(&str, &str)]) -> Knowledge {
        let mut k = Knowledge::default();
        for (name, body) in docs {
            let uri = format!("file:///w/{name}.ttl");
            let a = Analysis::new(uri.clone(), &format!("{HEAD}{body}"), Arc::default(), false);
            k.set(&uri, Source::Disk, Arc::new(a));
        }
        k
    }

    fn run(k: &Knowledge, params: serde_json::Value) -> GraphResult {
        let p: GraphParams = serde_json::from_value(params).expect("params");
        graph(k, Encoding::Utf16, p).expect("valid params")
    }

    fn ids(r: &GraphResult) -> Vec<&str> {
        let mut v: Vec<&str> = r.nodes.iter().map(|n| n.id.as_str()).collect();
        v.sort();
        v
    }

    fn node<'r>(r: &'r GraphResult, name: &str) -> &'r GraphNode {
        r.nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("no node {name} in {:?}", ids(r)))
    }

    /// (source name, predicate name, target name) of each edge, sorted.
    fn edges(r: &GraphResult) -> Vec<(String, String, String)> {
        let mut v: Vec<_> = r
            .edges
            .iter()
            .map(|e| {
                (
                    r.nodes[e.source as usize].name.clone(),
                    e.name.clone(),
                    r.nodes[e.target as usize].name.clone(),
                )
            })
            .collect();
        v.sort();
        v
    }

    const ONTO: &str = "ex:Machine a owl:Class ; rdfs:label \"Machine\"@en .\n\
        ex:Pump a owl:Class ; rdfs:subClassOf ex:Machine .\n\
        ex:feeds a owl:ObjectProperty ; rdfs:domain ex:Pump .\n\
        ex:Wet a skos:Concept .\n\
        ex:PumpShape a sh:NodeShape ; sh:targetClass ex:Pump .\n\
        ex:p1 a ex:Pump ; ex:feeds ex:p2 ; rdfs:label \"P1\" .\n\
        ex:p2 ex:state ex:Wet .\n";

    /// Catches a classifier that reads only the types (`ex:p2`, untyped, is
    /// other), or that lets the class bit win over the shape type.
    #[test]
    fn kinds_come_from_types_and_use() {
        let k = knowledge(&[("onto", ONTO)]);
        let r = run(&k, serde_json::json!({}));
        let kind = |n: &str| node(&r, n).kind;
        assert_eq!(kind("ex:Machine"), NodeKind::Class);
        assert_eq!(kind("ex:Pump"), NodeKind::Class);
        assert_eq!(kind("ex:feeds"), NodeKind::Property);
        assert_eq!(kind("ex:Wet"), NodeKind::Concept);
        assert_eq!(kind("ex:PumpShape"), NodeKind::Shape);
        assert_eq!(kind("ex:p1"), NodeKind::Individual);
        assert_eq!(kind("ex:p2"), NodeKind::Other);
    }

    /// Catches a label taken from the name, or a name that ignores the
    /// documents' prefixes.
    #[test]
    fn labels_are_hovers_and_names_are_prefixed() {
        let k = knowledge(&[("onto", ONTO)]);
        let r = run(&k, serde_json::json!({}));
        assert_eq!(node(&r, "ex:Machine").label.as_deref(), Some("Machine"));
        assert_eq!(node(&r, "ex:Pump").label, None);
        assert_eq!(node(&r, "ex:Pump").id, "http://example.org/Pump");
        // A well-known term gets its vocabulary's label and prefix.
        let r = run(&k, serde_json::json!({ "types": true }));
        let class = node(&r, "owl:Class");
        assert_eq!(class.id, "http://www.w3.org/2002/07/owl#Class");
        assert!(class.label.is_some());
        let k = knowledge(&[("x", "<http://other.org/a> <http://other.org/p> ex:b .")]);
        let r = run(&k, serde_json::json!({}));
        assert_eq!(ids(&r), ["http://example.org/b", "http://other.org/a"]);
        node(&r, "<http://other.org/a>");
    }

    /// Catches literals or `rdf:type` edges drawn by default, and the flags
    /// not turning them on.
    #[test]
    fn literals_and_type_edges_are_off_by_default() {
        let k = knowledge(&[("onto", ONTO)]);
        let off = run(&k, serde_json::json!({}));
        assert!(off.nodes.iter().all(|n| !n.literal));
        assert!(off.edges.iter().all(|e| e.name != "rdf:type"));
        // ex:p1's type edge is gone, the individual stays.
        node(&off, "ex:p1");
        let on = run(&k, serde_json::json!({ "literals": true, "types": true }));
        let lits: Vec<&GraphNode> = on.nodes.iter().filter(|n| n.literal).collect();
        assert_eq!(lits.len(), 2, "{:?}", ids(&on));
        assert!(lits.iter().any(|n| n.name == "\"Machine\"@en"));
        assert!(lits.iter().any(|n| n.name == "\"P1\""));
        assert!(edges(&on).contains(&("ex:p1".into(), "rdf:type".into(), "ex:Pump".into())));
        // Each occurrence is its own node.
        let k = knowledge(&[("x", "ex:a rdfs:label \"x\" . ex:b rdfs:label \"x\" .")]);
        let r = run(&k, serde_json::json!({ "literals": true }));
        assert_eq!(r.nodes.iter().filter(|n| n.literal).count(), 2);
    }

    /// Catches blank nodes merged across documents by their number.
    #[test]
    fn blank_nodes_are_scoped_to_their_document() {
        let body = "ex:a ex:p [ rdfs:label \"Inner\" ] .";
        let k = knowledge(&[("a", body), ("b", body)]);
        let r = run(&k, serde_json::json!({}));
        let blanks: Vec<&GraphNode> = r.nodes.iter().filter(|n| n.id.starts_with("_:")).collect();
        assert_eq!(blanks.len(), 2, "{:?}", ids(&r));
        assert_ne!(blanks[0].id, blanks[1].id);
        assert_ne!(blanks[0].uri, blanks[1].uri);
        assert!(blanks.iter().all(|b| b.label.as_deref() == Some("Inner")));
        // One subject, two edges: the same triple in two documents is one
        // edge only when it is the same blank node.
        assert_eq!(r.edges.len(), 2);
    }

    /// Catches a range at the first mention when the node is described
    /// elsewhere, and a mention range off the term.
    #[test]
    fn ranges_point_at_the_description_else_the_first_mention() {
        let k = knowledge(&[
            ("a", "ex:x ex:p ex:Pump ."),
            ("b", "\n\nex:Pump a owl:Class ."),
        ]);
        let r = run(&k, serde_json::json!({}));
        let pump = node(&r, "ex:Pump");
        assert_eq!(pump.uri, "file:///w/b.ttl");
        let line = HEAD.lines().count() as u32 + 2;
        assert_eq!(
            pump.range,
            Range::new(Position::new(line, 0), Position::new(line, 7))
        );
        let x = node(&r, "ex:x");
        assert_eq!(x.uri, "file:///w/a.ttl");
        let line = HEAD.lines().count() as u32;
        assert_eq!(
            x.range,
            Range::new(Position::new(line, 0), Position::new(line, 4))
        );
        // An undescribed object: its first mention.
        let k = knowledge(&[("a", "ex:x ex:p ex:y .")]);
        let r = run(&k, serde_json::json!({}));
        assert_eq!(node(&r, "ex:y").range.start, Position::new(line, 10));
        assert_eq!(r.edges[0].uri, "file:///w/a.ttl");
    }

    /// Catches each filter ignored, and filters that do not combine.
    #[test]
    fn filters_each_and_combined() {
        let k = knowledge(&[
            ("onto", ONTO),
            ("other", "<http://o.org/a> ex:feeds ex:p1 ."),
        ]);
        let all = run(&k, serde_json::json!({}));
        let names = |r: &GraphResult| {
            let mut v: Vec<String> = r.nodes.iter().map(|n| n.name.clone()).collect();
            v.sort();
            v
        };
        // files
        let r = run(&k, serde_json::json!({ "files": ["file:///w/other.ttl"] }));
        assert_eq!(names(&r), ["<http://o.org/a>", "ex:p1"]);
        // kinds
        let r = run(&k, serde_json::json!({ "kinds": ["class"] }));
        assert_eq!(names(&r), ["ex:Machine", "ex:Pump"]);
        assert_eq!(
            edges(&r),
            [(
                "ex:Pump".into(),
                "rdfs:subClassOf".into(),
                "ex:Machine".into()
            )]
        );
        // namespaces
        let r = run(
            &k,
            serde_json::json!({ "namespaces": { "exclude": ["http://example.org/"] } }),
        );
        assert_eq!(names(&r), ["<http://o.org/a>"]);
        let r = run(
            &k,
            serde_json::json!({ "namespaces": { "include": ["http://o.org/"] } }),
        );
        assert_eq!(names(&r), ["<http://o.org/a>"]);
        // predicates
        let feeds = "http://example.org/feeds";
        let r = run(
            &k,
            serde_json::json!({ "predicates": { "include": [feeds] } }),
        );
        assert_eq!(names(&r), ["<http://o.org/a>", "ex:p1", "ex:p2"]);
        let r = run(
            &k,
            serde_json::json!({ "predicates": { "exclude": [feeds] } }),
        );
        assert!(r.edges.iter().all(|e| e.predicate != feeds));
        assert_eq!(r.nodes.len(), all.nodes.len() - 1, "only <o.org/a> goes");
        // combined: one file, one predicate, one kind
        let r = run(
            &k,
            serde_json::json!({
                "files": ["file:///w/onto.ttl"],
                "predicates": { "include": [feeds] },
                "kinds": ["individual", "other"],
            }),
        );
        assert_eq!(
            edges(&r),
            [("ex:p1".into(), "ex:feeds".into(), "ex:p2".into())]
        );
        assert_eq!(r.total, Total { nodes: 2, edges: 1 });
        // The facets are those of the whole graph whatever the filters.
        assert_eq!(r.facets, all.facets);
    }

    /// Catches a focus that follows edges one way only, or ignores depth.
    #[test]
    fn focus_and_depth() {
        let k = knowledge(&[(
            "c",
            "ex:a ex:p ex:b . ex:b ex:p ex:c . ex:c ex:p ex:d . ex:z ex:p ex:a .",
        )]);
        let focus = "http://example.org/b";
        let r = run(&k, serde_json::json!({ "focus": focus, "depth": 1 }));
        assert_eq!(r.nodes[0].id, focus, "the focus comes first");
        assert_eq!(
            ids(&r),
            ["http://example.org/a", focus, "http://example.org/c"]
        );
        let r = run(&k, serde_json::json!({ "focus": focus }));
        assert_eq!(r.nodes.len(), 5, "depth 2 by default reaches d and z");
        let r = run(
            &k,
            serde_json::json!({ "focus": "http://example.org/nope" }),
        );
        assert!(r.nodes.is_empty() && r.total == Total::default());
        // A blank node can be the focus, by its id.
        let k = knowledge(&[("c", "ex:a ex:p [ ex:q ex:b ] . ex:z ex:p ex:y .")]);
        let all = run(&k, serde_json::json!({}));
        let blank = all.nodes.iter().find(|n| n.id.starts_with("_:")).unwrap();
        let r = run(&k, serde_json::json!({ "focus": blank.id, "depth": 1 }));
        assert_eq!(r.nodes.len(), 3, "{:?}", ids(&r));
    }

    /// Catches a cap that keeps an arbitrary subset, one that varies run to
    /// run, and totals counted after the cap.
    #[test]
    fn the_cap_keeps_the_best_connected_deterministically() {
        let mut body = String::new();
        for i in 0..30 {
            body += &format!("ex:hub ex:p ex:n{i} . ");
        }
        body += "ex:n1 ex:p ex:n2 . ex:n1 ex:p ex:n3 . ex:n2 ex:p ex:n3 . ex:lone ex:p ex:other .";
        let k = knowledge(&[("cap", &body)]);
        let p = serde_json::json!({ "maxNodes": 4, "layout": { "iterations": 20 } });
        let r = run(&k, p.clone());
        let names: Vec<&str> = r.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["ex:hub", "ex:n1", "ex:n2", "ex:n3"]);
        assert_eq!(
            r.total,
            Total {
                nodes: 33,
                edges: 34
            }
        );
        assert_eq!(r.edges.len(), 6);
        assert_eq!(r, run(&k, p), "the same input, the same answer");
        // Focused: breadth-first, each level by degree.
        let r = run(
            &k,
            serde_json::json!({ "focus": "http://example.org/n3", "maxNodes": 3 }),
        );
        let names: Vec<&str> = r.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["ex:n3", "ex:hub", "ex:n1"]);
        // maxNodes over the limit is the limit.
        let p = GraphParams {
            max_nodes: Some(1_000_000),
            ..GraphParams::default()
        };
        assert_eq!(Query::new(p).ok().unwrap().max_nodes, MAX_NODES as usize);
    }

    /// Catches facets counted after the filters, or miscounted.
    #[test]
    fn facets_and_totals() {
        let k = knowledge(&[
            ("a", "ex:a ex:p ex:b . ex:a ex:p ex:c ."),
            ("b", "ex:a ex:p ex:b . ex:a ex:q <http://o.org/x> ."),
        ]);
        let r = run(&k, serde_json::json!({ "kinds": ["class"] }));
        assert!(r.nodes.is_empty());
        assert_eq!(r.total, Total::default());
        let f = &r.facets;
        assert_eq!(
            f.files,
            [
                FileFacet {
                    uri: "file:///w/a.ttl".into(),
                    nodes: 3
                },
                FileFacet {
                    uri: "file:///w/b.ttl".into(),
                    nodes: 3
                },
            ]
        );
        assert_eq!(
            f.namespaces,
            [
                NamespaceFacet {
                    iri: "http://example.org/".into(),
                    prefix: Some("ex".into()),
                    nodes: 3
                },
                NamespaceFacet {
                    iri: "http://o.org/".into(),
                    prefix: None,
                    nodes: 1
                },
            ]
        );
        // The triple both documents state is one edge.
        assert_eq!(
            f.predicates,
            [
                PredicateFacet {
                    iri: "http://example.org/p".into(),
                    name: "ex:p".into(),
                    edges: 2
                },
                PredicateFacet {
                    iri: "http://example.org/q".into(),
                    name: "ex:q".into(),
                    edges: 1
                },
            ]
        );
        let r = run(&k, serde_json::json!({}));
        assert_eq!(r.total, Total { nodes: 4, edges: 3 });
    }

    /// Catches imports drawn as workspace documents.
    #[test]
    fn imports_are_not_drawn() {
        let mut k = knowledge(&[("a", "ex:a ex:p ex:b .")]);
        let a = Analysis::new(
            "file:///cache/x.ttl".into(),
            &format!("{HEAD}ex:imported ex:p ex:b ."),
            Arc::default(),
            false,
        );
        k.add_external("import:http://example.org/x", Arc::new(a));
        let r = run(&k, serde_json::json!({}));
        assert_eq!(ids(&r), ["http://example.org/a", "http://example.org/b"]);
    }

    /// Catches nonsense accepted, and a panic on it.
    #[test]
    fn invalid_params_are_rejected() {
        let k = knowledge(&[("a", "ex:a ex:p ex:b .")]);
        let bad = |v: serde_json::Value| {
            let p: GraphParams = serde_json::from_value(v).expect("decodes");
            graph(&k, Encoding::Utf16, p).expect_err("rejected")
        };
        bad(serde_json::json!({ "focus": "" }));
        bad(serde_json::json!({ "layout": { "positions": { "x": [1e300, 0] } } }));
        // What serde rejects, S2 reports as InvalidParams too.
        for v in [
            serde_json::json!({ "kinds": ["planet"] }),
            serde_json::json!({ "maxNodes": -1 }),
            serde_json::json!({ "files": "file:///w/a.ttl" }),
        ] {
            assert!(serde_json::from_value::<GraphParams>(v).is_err());
        }
        let p = GraphParams {
            layout: LayoutParams {
                iterations: Some(u32::MAX),
                ..LayoutParams::default()
            },
            ..GraphParams::default()
        };
        assert_eq!(Query::new(p).ok().unwrap().iterations, MAX_ITERATIONS);
    }

    /// Catches a layout that is not reproducible, that drops the given
    /// positions, or that moves anything with no iterations.
    #[test]
    fn layout_is_deterministic_and_starts_from_given_positions() {
        let k = knowledge(&[("onto", ONTO)]);
        let a = run(&k, serde_json::json!({}));
        assert_eq!(a.positions, run(&k, serde_json::json!({})).positions);
        assert_eq!(a.positions.len(), 2 * a.nodes.len());
        assert!(a.positions.iter().all(|x| x.is_finite()));
        let at = |r: &GraphResult, id: &str| {
            let i = r.nodes.iter().position(|n| n.id == id).unwrap();
            [r.positions[2 * i], r.positions[2 * i + 1]]
        };
        let shift = |r: &GraphResult, from: &FxHashMap<String, [f64; 2]>| {
            let moved: Vec<f64> = from
                .iter()
                .map(|(id, [x, y])| {
                    let [u, v] = at(r, id);
                    (u - x).hypot(v - y)
                })
                .collect();
            moved.iter().sum::<f64>() / moved.len() as f64
        };
        // iterations 0: the given positions as they are.
        let grid: FxHashMap<String, [f64; 2]> = a
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), [i as f64 * 10.0, -(i as f64)]))
            .collect();
        let b = run(
            &k,
            serde_json::json!({ "layout": { "iterations": 0, "positions": grid } }),
        );
        for n in &b.nodes {
            assert_eq!(at(&b, &n.id), grid[&n.id], "{}", n.id);
        }
        // A laid-out graph passed back stays about where it was; a fresh
        // start would not.
        let laid: FxHashMap<String, [f64; 2]> = a
            .nodes
            .iter()
            .map(|n| (n.id.clone(), at(&a, &n.id)))
            .collect();
        let again = run(&k, serde_json::json!({ "layout": { "positions": laid } }));
        let hashed = run(&k, serde_json::json!({ "layout": { "iterations": 0 } }));
        assert!(
            shift(&again, &laid) < EDGE / 4.0,
            "moved {}",
            shift(&again, &laid)
        );
        assert!(
            shift(&hashed, &laid) > EDGE,
            "moved {}",
            shift(&hashed, &laid)
        );
        // A new node starts next to a placed neighbour.
        let first = a.nodes[0].id.clone();
        let mut kept = laid.clone();
        kept.remove(&first);
        let fresh = run(
            &k,
            serde_json::json!({ "layout": { "iterations": 0, "positions": kept } }),
        );
        let [x, y] = at(&fresh, &first);
        let nearest = kept
            .values()
            .map(|[u, v]| (u - x).hypot(v - y))
            .fold(f64::MAX, f64::min);
        assert!(nearest <= EDGE + 0.2, "{nearest}");
    }

    /// Catches forces with the wrong sign or a broken quadtree: linked
    /// nodes end up much closer than unlinked ones, and nothing coincides.
    #[test]
    fn the_layout_pulls_linked_nodes_together() {
        // Ten chains of ten, no links between chains.
        let mut ids = Vec::new();
        let mut edges = Vec::new();
        for c in 0..10u32 {
            for i in 0..10u32 {
                ids.push(format!("n{c}_{i}"));
                if i > 0 {
                    edges.push((c * 10 + i - 1, c * 10 + i));
                }
            }
        }
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let pos = layout(&refs, &edges, &FxHashMap::default(), 300);
        let d =
            |a: usize, b: usize| (pos[2 * a] - pos[2 * b]).hypot(pos[2 * a + 1] - pos[2 * b + 1]);
        let linked = edges
            .iter()
            .map(|&(a, b)| d(a as usize, b as usize))
            .sum::<f64>()
            / edges.len() as f64;
        let mut all = 0.0;
        let mut pairs = 0;
        for a in 0..ids.len() {
            for b in a + 1..ids.len() {
                all += d(a, b);
                pairs += 1;
            }
        }
        let all = all / pairs as f64;
        assert!(linked * 3.0 < all, "linked {linked}, all pairs {all}");
        assert!(
            linked > EDGE / 4.0 && linked < EDGE * 3.0,
            "linked {linked}"
        );
        // Spread on both axes, no two bodies close.
        let spread = |axis: usize| {
            let v: Vec<f64> = pos.iter().skip(axis).step_by(2).copied().collect();
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let ratio = spread(0) / spread(1);
        assert!((0.5..2.0).contains(&ratio), "x/y spread {ratio}");
        let nearest = (0..ids.len())
            .map(|a| {
                (0..ids.len())
                    .filter(|&b| b != a)
                    .map(|b| d(a, b))
                    .fold(f64::MAX, f64::min)
            })
            .sum::<f64>()
            / ids.len() as f64;
        assert!(nearest > EDGE / 2.0, "nearest neighbour {nearest}");
        // Two bodies on one spot: no NaN, and they part.
        let same: FxHashMap<String, [f64; 2]> =
            [("a".into(), [1.0, 1.0]), ("b".into(), [1.0, 1.0])]
                .into_iter()
                .collect();
        let pos = layout(&["a", "b"], &[], &same, 50);
        assert!(pos.iter().all(|x| x.is_finite()), "{pos:?}");
        assert!(
            (pos[0] - pos[2]).hypot(pos[1] - pos[3]) > EDGE / 2.0,
            "{pos:?}"
        );
    }

    /// Catches a parallel sum that depends on the threads' order.
    #[test]
    fn a_large_layout_is_the_same_every_time() {
        let ids: Vec<String> = (0..PARALLEL + 500).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let edges: Vec<(u32, u32)> = (1..refs.len() as u32).map(|i| (i / 2, i)).collect();
        let none = FxHashMap::default();
        let first = layout(&refs, &edges, &none, 5);
        for _ in 0..3 {
            assert_eq!(first, layout(&refs, &edges, &none, 5));
        }
    }

    #[test]
    fn ids_read_back_as_their_keys() {
        let k = knowledge(&[("a", "ex:a ex:p [ ex:q \"text # with hash\" ] .")]);
        let r = run(&k, serde_json::json!({ "literals": true }));
        assert_eq!(r.nodes.len(), 3);
        let a = k.get("file:///w/a.ttl").unwrap().clone();
        let docs = [&*a];
        for n in &r.nodes {
            let key = Key::of(&n.id).unwrap_or_else(|| panic!("{}", n.id));
            assert_eq!(key.id(&docs), n.id);
        }
    }

    /// 17,000 classes and 18,000 subclass edges (the size of a large
    /// ontology, shaped like the `big-vocab` benchmark). On an M-series
    /// laptop: the query 13 ms; 300 layout iterations 0.53 s, 2.5 s on one
    /// thread. The bounds leave a CI runner five times slower on one thread
    /// some room: 500 ms and 20 s.
    #[cfg(not(debug_assertions))]
    #[test]
    fn timing_a_large_ontology_is_queried_and_laid_out_in_budget() {
        let n = 17_000;
        let mut body = String::new();
        for i in 0..n {
            body += &format!(
                "ex:C{i} a owl:Class ; rdfs:label \"Class {i}\"@en ; rdfs:subClassOf ex:C{} .\n",
                i / 3
            );
        }
        for i in 0..1_000 {
            body += &format!("ex:C{} rdfs:subClassOf ex:C{} .\n", i * 7 + 5, i * 11 + 3);
        }
        let k = knowledge(&[("big", &body)]);
        let query = |iterations: u32| {
            let p =
                serde_json::json!({ "maxNodes": 20_000, "layout": { "iterations": iterations } });
            let t = std::time::Instant::now();
            let r = run(&k, p);
            (t.elapsed(), r)
        };
        let (q, r) = query(0);
        assert_eq!(r.nodes.len(), n);
        assert!(r.edges.len() >= 17_900, "{}", r.edges.len());
        let (with_layout, laid) = query(300);
        let layout = with_layout.saturating_sub(q);
        eprintln!("query {q:?}, layout of 300 iterations {layout:?}");
        assert!(laid.positions.iter().all(|x| x.is_finite()));
        assert!(q.as_millis() < 500, "query {q:?}");
        assert!(layout.as_secs() < 20, "layout {layout:?}");
    }
}
