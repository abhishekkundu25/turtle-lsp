//! What TurbOWL infers from a query's documents, in the terms of the query
//! graph that reads them ([`crate::rdf::Graph::with_triples`]).
//!
//! The documents are the query's, imports included, in the order the base
//! reads them ([`base_order`]: by URI), and the query graph reads them in
//! that order too. Both number blank nodes across the documents in that
//! order, and the base names node `n` `g<n>`, as the graph's dataset does
//! (the base's equivalence test checks it), so an inferred `g<n>` is the
//! graph's node `n`. Every inferred triple is kept; past `max` of them,
//! reading them stops: they are too many.

use super::{Base, rules};
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Source;
use crate::rdf::graph::{NodeTriple, blank_max};
use crate::settings::Reasoning;
use crate::syntax::{BlankId, Direction, Iri, Node};
use rustc_hash::FxHashMap;
use spargebra::term::{NamedOrBlankNode, Term, Triple};
use std::sync::Arc;

/// TurbOWL's inferred triples and how many findings it reported.
#[derive(Default)]
pub(crate) struct Closure {
    /// Subject, predicate, object; blank nodes numbered as the query graph
    /// of the documents numbers them.
    pub(crate) triples: Vec<NodeTriple>,
    pub(crate) findings: usize,
}

/// Why [`infer`] gave no closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Halt {
    /// `cancel` or `stopped` stopped it.
    Stopped,
    /// More than the `max` triples to keep were inferred.
    TooMany,
}

/// How many inferred triples [`infer`] reads between two calls of its
/// `stopped`.
pub(super) const CHECK_EVERY: usize = 1 << 16;

/// Puts `docs` in the order the base reads them: by URI, a stable sort, as
/// `Base::new` sorts documents of one [`Source`].
pub(crate) fn base_order(docs: &mut [&Arc<Analysis>]) {
    docs.sort_by(|a, b| a.uri.cmp(&b.uri));
}

/// What TurbOWL infers with `rules` from `docs` (in [`base_order`]), at
/// most `max` triples. Off infers nothing. `cancel` stops the run (TurbOWL
/// checks it before each round), and `stopped`, asked every
/// [`CHECK_EVERY`] inferred triples read, the reading of its result.
pub(crate) fn infer(
    rules_of: Reasoning,
    docs: &[&Arc<Analysis>],
    max: u64,
    cancel: &turbowl::Cancel,
    stopped: &dyn Fn() -> bool,
) -> Result<Closure, Halt> {
    debug_assert!(
        docs.is_sorted_by(|a, b| a.uri <= b.uri),
        "not in base order"
    );
    let Some(switches) = rules(rules_of) else {
        return Ok(Closure::default());
    };
    // One source for all: the base keeps their order.
    let sources = docs.iter().map(|d| (Source::Open, Arc::clone(d))).collect();
    // Places are not read: any encoding.
    let base = Base::new(sources, Encoding::Utf16);
    let mut reasoner = turbowl::Reasoner::with_switches(switches);
    reasoner.set_base(base.triples());
    reasoner
        .reason_cancellable(cancel)
        .map_err(|_| Halt::Stopped)?;
    let mut blanks = Blanks::new(docs);
    let mut triples = Vec::new();
    for (i, t) in reasoner.inferred().enumerate() {
        if i % CHECK_EVERY == CHECK_EVERY - 1 && stopped() {
            return Err(Halt::Stopped);
        }
        if triples.len() as u64 >= max {
            return Err(Halt::TooMany);
        }
        triples.push(blanks.triple(&t));
    }
    Ok(Closure {
        triples,
        findings: reasoner.diagnostics().len(),
    })
}

/// Blank node labels to the graph's numbers: `g<n>`, the base's name of
/// node `n`, is `n`; another label (TurbOWL's own, which OWL 2 RL never
/// makes) a new node past the documents'.
struct Blanks {
    /// One past the documents' highest blank node number.
    end: u32,
    minted: FxHashMap<String, u32>,
}

impl Blanks {
    fn new(docs: &[&Arc<Analysis>]) -> Blanks {
        // Each document's blank nodes follow the ones before it.
        let count = |a: &Analysis| {
            let mut max = 0;
            for t in a.parsed.triples.iter().filter(|t| !t.unresolved) {
                blank_max(&t.s, &mut max);
                blank_max(&t.o, &mut max);
            }
            max
        };
        let end = docs.iter().fold(0u32, |n, d| n.saturating_add(count(d)));
        Blanks {
            end,
            minted: FxHashMap::default(),
        }
    }

    fn number(&mut self, label: &str) -> u32 {
        let given = label
            .strip_prefix('g')
            .filter(|n| !n.starts_with('0') || *n == "0")
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|&n| n < self.end);
        given.unwrap_or_else(|| {
            let next = self.end.saturating_add(self.minted.len() as u32);
            *self.minted.entry(label.to_owned()).or_insert(next)
        })
    }

    fn triple(&mut self, t: &Triple) -> NodeTriple {
        let s = match &t.subject {
            NamedOrBlankNode::NamedNode(n) => Node::Iri(Iri::intern(n.as_str())),
            NamedOrBlankNode::BlankNode(b) => Node::Blank(BlankId(self.number(b.as_str()))),
        };
        (s, Iri::intern(t.predicate.as_str()), self.node(&t.object))
    }

    fn node(&mut self, t: &Term) -> Node {
        match t {
            Term::NamedNode(n) => Node::Iri(Iri::intern(n.as_str())),
            Term::BlankNode(b) => Node::Blank(BlankId(self.number(b.as_str()))),
            Term::Literal(l) => Node::Literal {
                value: l.value().into(),
                datatype: Iri::intern(l.datatype().as_str()),
                lang: l.language().map(Iri::intern),
                dir: l.direction().map(|d| match d {
                    oxrdf::BaseDirection::Ltr => Direction::Ltr,
                    oxrdf::BaseDirection::Rtl => Direction::Rtl,
                }),
            },
            Term::Triple(t) => Node::Triple(Box::new(self.triple(t))),
        }
    }
}
