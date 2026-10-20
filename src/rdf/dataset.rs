//! A [`Graph`] as a `spareval` dataset ([`Index`]), so SPARQL queries run
//! over it in place, with no copy of the data.
//!
//! The graph is the default graph; a caller can also give it an IRI as a
//! named graph.

use super::graph::{GTriple, Graph, Id, Term as GTerm};
use crate::syntax::{Iri, Node};
use rustc_hash::FxHashMap;
use spareval::{InternalQuad, QueryableDataset};
use spargebra::term::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};
use std::convert::Infallible;

/// Blank nodes have no constant syntax: a query can name one as this prefix
/// followed by its label, an IRI [`Index`] maps back to the node.
pub const BLANK: &str = "urn:x-shacl:blank:";

/// A term as the evaluator sees it: a graph term by id when the graph has it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum STerm {
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
pub struct Index<'a> {
    g: &'a Graph<'a>,
    literals: FxHashMap<(&'a str, Iri, Option<Iri>), Id>,
    /// The IRI of the named graph that is the same graph.
    graph: Option<&'a str>,
}

impl<'a> Index<'a> {
    /// One pass over the graph's terms, to find literals by value. `graph`
    /// names the graph as a named graph too.
    pub fn new(g: &'a Graph<'a>, graph: Option<&'a str>) -> Index<'a> {
        let literals = g
            .terms()
            .filter_map(|(id, t)| t.literal().map(|(v, d, l)| ((v, d, l), id)))
            .collect();
        Index { g, literals, graph }
    }

    pub fn term(&self, id: Id) -> Term {
        match self.g.term(id) {
            GTerm::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
            GTerm::Blank(b) => BlankNode::new_unchecked(format!("g{b}")).into(),
            GTerm::Value(n) => node_term(n),
        }
    }

    /// The graph id of `t`, if the graph has it.
    pub fn id(&self, t: &Term) -> Option<Id> {
        match self.internal(t) {
            STerm::Id(id) => Some(id),
            _ => None,
        }
    }

    /// `t` as the evaluator sees it.
    pub fn internal(&self, t: &Term) -> STerm {
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
                match (Iri::get(l.datatype().as_str()), lang) {
                    (Some(dt), Some(lang)) => self.literals.get(&(l.value(), dt, lang)).copied(),
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
            (None, None, None) => Box::new(g.triples.iter()),
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

fn node_term(n: &Node) -> Term {
    match n {
        Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
        Node::Blank(b) => BlankNode::new_unchecked(format!("t{}", b.0)).into(),
        Node::Literal {
            value,
            datatype,
            lang,
        } => match lang {
            Some(l) => {
                Literal::new_language_tagged_literal_unchecked(value.as_ref(), l.as_str()).into()
            }
            None => Literal::new_typed_literal(
                value.as_ref(),
                NamedNode::new_unchecked(datatype.as_str()),
            )
            .into(),
        },
        Node::Triple(t) => {
            let s: NamedOrBlankNode = match node_term(&t.0) {
                Term::NamedNode(n) => n.into(),
                Term::BlankNode(b) => b.into(),
                _ => BlankNode::default().into(),
            };
            Term::Triple(Box::new(Triple::new(
                s,
                NamedNode::new_unchecked(t.1.as_str()),
                node_term(&t.2),
            )))
        }
    }
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
