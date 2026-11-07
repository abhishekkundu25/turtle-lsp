//! A [`Graph`] as a `spareval` dataset ([`Index`]), so SPARQL queries run
//! over it in place, with no copy of the data.
//!
//! The graph is the default graph; a caller can also give it an IRI as a
//! named graph.

use super::graph::{GTriple, Graph, Id, Term as GTerm};
use crate::syntax::{Direction, Iri, Node};
use oxrdf::{LiteralRef, NamedNodeRef};
use spareval::{InternalQuad, QueryableDataset};
use spargebra::term::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};
use std::convert::Infallible;

/// Blank nodes have no constant syntax: a query can name one as this prefix
/// followed by its label, an IRI [`Index`] maps back to the node.
pub(crate) const BLANK: &str = "urn:x-shacl:blank:";

/// A term as the evaluator sees it: a graph term by id when the graph has it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum STerm {
    Id(Id),
    /// An IRI the graph uses as a predicate only.
    Iri(Iri),
    /// The extra named graph the dataset was given.
    Graph,
    /// A term the graph does not have.
    Other(Box<Term>),
}

/// A [`Graph`] as a SPARQL dataset: the graph is the default graph and, when
/// the dataset is given one, also a named graph.
pub(crate) struct Index<'a> {
    g: &'a Graph<'a>,
    /// The IRI of the named graph that is the same graph.
    graph: Option<&'a str>,
}

impl<'a> Index<'a> {
    /// `graph` names the graph as a named graph too. Literals are found by
    /// value with [`Graph::literal`]: over an overlay, that walks the
    /// overlay's own terms, and the shared graph's once for all overlays.
    pub(crate) fn new(g: &'a Graph<'a>, graph: Option<&'a str>) -> Index<'a> {
        Index { g, graph }
    }

    pub(crate) fn term(&self, id: Id) -> Term {
        match self.g.term(id) {
            GTerm::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
            GTerm::Blank(b) => BlankNode::new_unchecked(blank_label(b)).into(),
            GTerm::Value(n) => node_term(n, self.g.term_blank_base(id)),
        }
    }

    /// The graph id of `t`, if the graph has it.
    pub(crate) fn id(&self, t: &Term) -> Option<Id> {
        match self.internal(t) {
            STerm::Id(id) => Some(id),
            _ => None,
        }
    }

    /// `t` as the evaluator sees it.
    pub(crate) fn internal(&self, t: &Term) -> STerm {
        let found = match t {
            Term::NamedNode(n) if Some(n.as_str()) == self.graph => return STerm::Graph,
            Term::NamedNode(n) if n.as_str().starts_with(BLANK) => n.as_str()[BLANK.len()..]
                .strip_prefix('g')
                .and_then(|n| n.parse().ok())
                .and_then(|n| self.g.id(GTerm::Blank(n))),
            Term::NamedNode(n) => match Iri::get(n.as_str()) {
                Some(i) => return self.g.iri(i).map_or(STerm::Iri(i), STerm::Id),
                None => None,
            },
            Term::BlankNode(b) => b
                .as_str()
                .strip_prefix('g')
                .and_then(|n| n.parse().ok())
                .and_then(|n| self.g.id(GTerm::Blank(n))),
            Term::Literal(l) => {
                let lang = match l.language() {
                    Some(l) => Iri::get(l).map(Some),
                    None => Some(None),
                };
                let dir = l.direction().map(|d| {
                    if Some(d) == directions()[Direction::Rtl as usize].direction() {
                        Direction::Rtl
                    } else {
                        Direction::Ltr
                    }
                });
                match (Iri::get(l.datatype().as_str()), lang) {
                    (Some(dt), Some(lang)) => self.g.literal(&(l.value(), dt, lang, dir)),
                    _ => None,
                }
            }
            _ => None,
        };
        found.map_or_else(|| STerm::Other(Box::new(t.clone())), STerm::Id)
    }

    fn predicate(&self, p: Iri) -> STerm {
        self.g.iri(p).map_or(STerm::Iri(p), STerm::Id)
    }

    fn quads(
        &'a self,
        s: Option<&STerm>,
        p: Option<&STerm>,
        o: Option<&STerm>,
        graph: Option<Option<&STerm>>,
    ) -> Option<Box<dyn Iterator<Item = Result<InternalQuad<STerm>, Infallible>> + 'a>> {
        let graph_name = match graph {
            Some(None) => None,
            Some(Some(STerm::Graph)) | None if self.graph.is_some() => Some(STerm::Graph),
            _ => return None,
        };
        let id = |t: Option<&STerm>| match t {
            None => Some(None),
            Some(STerm::Id(i)) => Some(Some(*i)),
            Some(_) => None,
        };
        let (s, o) = (id(s)?, id(o)?);
        let p = match p {
            None => None,
            Some(STerm::Id(i)) => Some(self.g.as_iri(*i)?),
            Some(STerm::Iri(i)) => Some(*i),
            Some(_) => return None,
        };
        let g = self.g;
        let triples: Box<dyn Iterator<Item = &'a GTriple> + 'a> = match (s, o, p) {
            (Some(s), _, _) => Box::new(g.out(s).map(|t| t.1)),
            (None, Some(o), _) => Box::new(g.incoming(o).map(|t| t.1)),
            (None, None, Some(p)) => Box::new(g.with_p(p)),
            (None, None, None) => Box::new(g.all()),
        };
        Some(Box::new(
            triples
                .filter(move |t| {
                    s.is_none_or(|s| t.s == s)
                        && o.is_none_or(|o| t.o == o)
                        && p.is_none_or(|p| t.p == p)
                })
                .map(move |t| {
                    Ok(InternalQuad {
                        subject: STerm::Id(t.s),
                        predicate: self.predicate(t.p),
                        object: STerm::Id(t.o),
                        graph_name: graph_name.clone(),
                    })
                }),
        ))
    }
}

/// The name of blank node number `n` of a graph ([`GTerm::Blank`]).
pub(crate) fn blank_label(n: u32) -> String {
    format!("g{n}")
}

/// A literal or triple term of the graph as an RDF term. `blank_base` is added
/// to the numbers of the blank nodes inside a triple term, which then have the
/// names the same nodes have outside ([`Index::term`]).
pub(crate) fn node_term(n: &Node, blank_base: u32) -> Term {
    match n {
        Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
        Node::Blank(b) => {
            BlankNode::new_unchecked(blank_label(blank_base.saturating_add(b.0))).into()
        }
        Node::Literal { .. } => literal_ref(n).expect("a literal").into_owned().into(),
        Node::Triple(t) => {
            let s: NamedOrBlankNode = match node_term(&t.0, blank_base) {
                Term::NamedNode(n) => n.into(),
                Term::BlankNode(b) => b.into(),
                _ => BlankNode::default().into(),
            };
            Term::Triple(Box::new(Triple::new(
                s,
                NamedNode::new_unchecked(t.1.as_str()),
                node_term(&t.2, blank_base),
            )))
        }
    }
}

/// A literal node as an RDF literal, borrowed from the node; `None` for
/// another node. [`node_term`] owns the same literal.
pub(crate) fn literal_ref(n: &Node) -> Option<LiteralRef<'_>> {
    let Node::Literal {
        value,
        datatype,
        lang,
        dir,
    } = n
    else {
        return None;
    };
    Some(match (lang, dir) {
        (Some(l), Some(d)) => LiteralRef::new_directional_language_tagged_literal_unchecked(
            value,
            l.as_str(),
            directions()[*d as usize]
                .direction()
                .expect("a directional literal"),
        ),
        (Some(l), None) => LiteralRef::new_language_tagged_literal_unchecked(value, l.as_str()),
        _ => LiteralRef::new_typed_literal(value, NamedNodeRef::new_unchecked(datatype.as_str())),
    })
}

/// A literal of each base direction, indexed by [`Direction`]. `oxrdf` does not
/// come through `spargebra`'s re-exports, so its `BaseDirection` is read from
/// these instead of being named.
fn directions() -> &'static [Literal; 2] {
    static D: std::sync::OnceLock<[Literal; 2]> = std::sync::OnceLock::new();
    D.get_or_init(|| {
        let of = |d: &str| {
            format!("\"\"@und--{d}")
                .parse()
                .expect("a directional literal")
        };
        [of("ltr"), of("rtl")]
    })
}

impl<'a> QueryableDataset<'a> for &'a Index<'a> {
    type InternalTerm = STerm;
    type Error = Infallible;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&STerm>,
        predicate: Option<&STerm>,
        object: Option<&STerm>,
        graph_name: Option<Option<&STerm>>,
    ) -> impl Iterator<Item = Result<InternalQuad<STerm>, Infallible>> + use<'a> {
        let ix: &'a Index<'a> = self;
        ix.quads(subject, predicate, object, graph_name)
            .into_iter()
            .flatten()
    }

    fn internal_named_graphs(&self) -> impl Iterator<Item = Result<STerm, Infallible>> + use<'a> {
        self.graph.map(|_| Ok(STerm::Graph)).into_iter()
    }

    fn contains_internal_graph_name(&self, graph_name: &STerm) -> Result<bool, Infallible> {
        Ok(*graph_name == STerm::Graph)
    }

    fn internalize_term(&self, term: Term) -> Result<STerm, Infallible> {
        Ok(self.internal(&term))
    }

    fn externalize_term(&self, term: STerm) -> Result<Term, Infallible> {
        Ok(match term {
            STerm::Id(id) => self.term(id),
            STerm::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
            STerm::Graph => NamedNode::new_unchecked(self.graph.unwrap_or_default()).into(),
            STerm::Other(t) => *t,
        })
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use crate::syntax::{PrefixMap, Syntax, parse};
    use std::sync::Arc;

    /// Each SHACL-SPARQL dataset run builds an [`Index`] over an overlay of
    /// the shared graph. Finding literals by value must cost in proportion
    /// to the dataset, not walk the shared graph's terms each time.
    #[test]
    fn an_index_over_an_overlay_walks_only_the_overlays_terms() {
        let mut shared = String::from("@prefix ex: <http://e.org/> .\n");
        for i in 0..2000 {
            shared.push_str(&format!("ex:s{i} ex:name \"n{i}\" .\n"));
        }
        let shared = parse(&shared, &PrefixMap::default(), Syntax::Turtle);
        let base = Arc::new(Graph::new(&[&shared.triples]));
        let base_terms = base.terms().count() as u64;
        let datasets: Vec<_> = (0..5)
            .map(|d| {
                parse(
                    &format!(
                        "@prefix ex: <http://e.org/> .\nex:d{d} ex:name \"own{d}\" , \"n7\" .\n"
                    ),
                    &PrefixMap::default(),
                    Syntax::Turtle,
                )
            })
            .collect();
        let walked = || crate::rdf::graph::TERMS_WALKED.with(|c| c.get());
        let before = walked();
        for (d, data) in datasets.iter().enumerate() {
            let over = Graph::overlay(&base, &data.triples);
            let ix = Index::new(&over, None);
            let own = Literal::new_simple_literal(format!("own{d}"));
            let shared = Literal::new_simple_literal("n7");
            assert!(ix.id(&own.into()).is_some(), "the dataset's literal");
            assert!(
                ix.id(&shared.into()).is_some(),
                "the shared graph's literal"
            );
            let missing = Literal::new_simple_literal("nowhere");
            assert!(ix.id(&missing.into()).is_none());
        }
        let walked = walked() - before;
        // The shared graph's terms once, then a few per dataset.
        assert!(
            walked <= base_terms + 5 * 10,
            "{walked} terms walked, {base_terms} shared"
        );
    }
}
