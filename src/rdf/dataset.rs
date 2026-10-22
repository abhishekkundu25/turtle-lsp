//! A [`Graph`] as a `spareval` dataset ([`Index`]), so SPARQL queries run
//! over it in place, with no copy of the data.
//!
//! The graph is the default graph; a caller can also give it an IRI as a
//! named graph.

use super::graph::{GTriple, Graph, Id, Term as GTerm};
use crate::syntax::{Direction, Iri, Node};
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

/// A literal's lexical form, datatype, language tag and base direction.
type LiteralKey<'a> = (&'a str, Iri, Option<Iri>, Option<Direction>);

/// A [`Graph`] as a SPARQL dataset: the graph is the default graph and, when
/// the dataset is given one, also a named graph.
pub struct Index<'a> {
    g: &'a Graph<'a>,
    literals: FxHashMap<LiteralKey<'a>, Id>,
    /// The IRI of the named graph that is the same graph.
    graph: Option<&'a str>,
}

impl<'a> Index<'a> {
    /// One pass over the graph's terms, to find literals by value. `graph`
    /// names the graph as a named graph too.
    pub fn new(g: &'a Graph<'a>, graph: Option<&'a str>) -> Index<'a> {
        let literals = g
            .terms()
            .filter_map(|(id, t)| t.literal().map(|(v, d, l)| ((v, d, l, t.direction()), id)))
            .collect();
        Index { g, literals, graph }
    }

    pub fn term(&self, id: Id) -> Term {
        match self.g.term(id) {
            GTerm::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
            GTerm::Blank(b) => BlankNode::new_unchecked(format!("g{b}")).into(),
            GTerm::Value(n) => node_term(n, self.g.term_blank_base(id)),
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
                let dir = l.direction().map(|d| {
                    if Some(d) == directions()[Direction::Rtl as usize].direction() {
                        Direction::Rtl
                    } else {
                        Direction::Ltr
                    }
                });
                match (Iri::get(l.datatype().as_str()), lang) {
                    (Some(dt), Some(lang)) => {
                        self.literals.get(&(l.value(), dt, lang, dir)).copied()
                    }
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

/// A literal or triple term of the graph as an RDF term. `blank_base` is added
/// to the numbers of the blank nodes inside a triple term, which then have the
/// names the same nodes have outside ([`Index::term`]).
fn node_term(n: &Node, blank_base: u32) -> Term {
    match n {
        Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
        Node::Blank(b) => {
            BlankNode::new_unchecked(format!("g{}", blank_base.saturating_add(b.0))).into()
        }
        Node::Literal {
            value,
            datatype,
            lang,
            dir,
        } => match (lang, dir) {
            (Some(l), Some(d)) => Literal::new_directional_language_tagged_literal_unchecked(
                value.as_ref(),
                l.as_str(),
                directions()[*d as usize]
                    .direction()
                    .expect("a directional literal"),
            )
            .into(),
            (Some(l), None) => {
                Literal::new_language_tagged_literal_unchecked(value.as_ref(), l.as_str()).into()
            }
            _ => Literal::new_typed_literal(
                value.as_ref(),
                NamedNode::new_unchecked(datatype.as_str()),
            )
            .into(),
        },
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
mod tests {
    use crate::analysis::Analysis;
    use crate::query::{Answer, Limits, run};
    use crate::syntax::PrefixMap;
    use spargebra::term::Term;
    use std::sync::Arc;

    const EX: &str = "http://example.com/";

    fn doc(uri: &str, body: &str) -> Analysis {
        let text = format!("@prefix ex: <{EX}> .\n{body}");
        Analysis::new(uri.into(), &text, Arc::new(PrefixMap::default()), false)
    }

    fn rows(docs: &[&Analysis], query: &str) -> Vec<Vec<Option<Term>>> {
        let prefixes = PrefixMap::from_iter([("ex".to_string(), EX.to_string())]);
        match run(docs, query, None, &prefixes, &Limits::default()) {
            Ok(Answer::Solutions(s)) => s.rows,
            other => panic!("not solutions: {other:?}"),
        }
    }

    fn ask(docs: &[&Analysis], query: &str) -> bool {
        let prefixes = PrefixMap::from_iter([("ex".to_string(), EX.to_string())]);
        match run(docs, query, None, &prefixes, &Limits::default()) {
            Ok(Answer::Boolean(b)) => b,
            other => panic!("not a boolean: {other:?}"),
        }
    }

    fn text(t: &Option<Term>) -> String {
        match t {
            Some(Term::Literal(l)) => l.value().to_string(),
            other => panic!("not a literal: {other:?}"),
        }
    }

    #[test]
    fn a_directional_string_has_a_language_and_a_direction() {
        let d = doc(
            "file:///d.ttl",
            "ex:s ex:l \"hi\"@en--rtl , \"hi\"@en , \"salut\"@FR--ltr .\n",
        );
        let r = rows(
            &[&d],
            "SELECT ?v (LANG(?l) AS ?lang) (LANGDIR(?l) AS ?dir) (DATATYPE(?l) AS ?dt) \
             WHERE { ex:s ex:l ?l . BIND(STR(?l) AS ?v) } ORDER BY ?lang ?dir",
        );
        let shown: Vec<_> = r
            .iter()
            .map(|row| {
                let dt = match &row[3] {
                    Some(Term::NamedNode(n)) => n.as_str().rsplit('#').next().unwrap().to_string(),
                    other => panic!("{other:?}"),
                };
                (text(&row[1]), text(&row[2]), dt)
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("en".to_string(), String::new(), "langString".to_string()),
                (
                    "en".to_string(),
                    "rtl".to_string(),
                    "dirLangString".to_string()
                ),
                (
                    "fr".to_string(),
                    "ltr".to_string(),
                    "dirLangString".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_constant_directional_string_matches_only_its_own_direction() {
        let d = doc("file:///d.ttl", "ex:s ex:l \"hi\"@en--rtl .\n");
        assert!(ask(&[&d], "ASK { ex:s ex:l \"hi\"@en--rtl }"));
        assert!(!ask(&[&d], "ASK { ex:s ex:l \"hi\"@en--ltr }"));
        assert!(!ask(&[&d], "ASK { ex:s ex:l \"hi\"@en }"));
    }

    const JOIN: &str = "SELECT ?t ?v WHERE { ?t ex:r ?tt . BIND(SUBJECT(?tt) AS ?x) ?x ex:p ?v } \
                        ORDER BY ?t";

    #[test]
    fn a_blank_node_in_a_triple_term_is_the_same_node_outside_it() {
        let d = doc(
            "file:///d.ttl",
            "ex:t ex:r <<( _:x ex:q 1 )>> .\n_:x ex:p 2 .\n",
        );
        let r = rows(&[&d], JOIN);
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(text(&r[0][1]), "2");
    }

    #[test]
    fn the_same_blank_label_in_two_documents_stays_two_nodes_in_triple_terms() {
        let a = doc(
            "file:///a.ttl",
            "ex:t ex:r <<( _:x ex:q 1 )>> .\n_:x ex:p 2 .\n",
        );
        let b = doc(
            "file:///b.ttl",
            "ex:u ex:r <<( _:x ex:q 1 )>> .\n_:x ex:p 3 .\n",
        );
        let r = rows(&[&a, &b], JOIN);
        let shown: Vec<_> = r
            .iter()
            .map(|row| match &row[0] {
                Some(Term::NamedNode(n)) => (
                    n.as_str().rsplit('/').next().unwrap().to_string(),
                    text(&row[1]),
                ),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("t".to_string(), "2".to_string()),
                ("u".to_string(), "3".to_string())
            ]
        );
    }

    #[test]
    fn a_relative_iri_is_the_absolute_one_in_query_results() {
        let a = doc("file:///w/a.ttl", "<#me> ex:p 1 .\n");
        let b = doc("file:///w/b.ttl", "<#me> ex:p 2 .\n");
        let r = rows(&[&a, &b], "SELECT ?s ?v WHERE { ?s ex:p ?v } ORDER BY ?v");
        let subjects: Vec<_> = r
            .iter()
            .map(|row| match &row[0] {
                Some(Term::NamedNode(n)) => n.as_str().to_string(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(subjects, ["file:///w/a.ttl#me", "file:///w/b.ttl#me"]);
    }

    #[test]
    fn triples_with_an_undeclared_prefix_are_not_in_the_graph() {
        let d = Analysis::new(
            "file:///d.ttl".into(),
            "@prefix ex: <http://example.com/> .\nex:a ex:p nope:o , ex:o .\nnope:s ex:p ex:o .\n",
            Arc::new(PrefixMap::default()),
            false,
        );
        assert_eq!(d.parsed.triples.len(), 3);
        let r = rows(&[&d], "SELECT ?s ?o WHERE { ?s ex:p ?o }");
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(!ask(
            &[&d],
            "ASK { ?s ?p ?o FILTER (STRSTARTS(STR(?o), \"nope:\")) }"
        ));
    }
}
