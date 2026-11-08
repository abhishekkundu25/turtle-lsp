//! What TurbOWL infers from a query's documents, in the terms of the query
//! graph that reads them ([`crate::rdf::Graph::with_triples`]).
//!
//! The base reads the documents sorted by URI, the graph in the order the
//! query gives them, and each numbers blank nodes across its documents in
//! its own order. A blank node of an inferred triple is renumbered from the
//! base's order to the graph's, so it is the graph's node; one of an import
//! keeps its number, past the queried documents' (imports sort after them),
//! so it is a node of its own.
//!
//! Only the inferred triples about the queried documents are kept: those
//! whose subject or object is a term in a subject or object of a queried
//! document's triple (an IRI, a literal, a triple term, or one of its blank
//! nodes; never an import's blank node). What TurbOWL infers among an
//! import's terms alone, the imported vocabulary's own closure, is left
//! out. Past `max` kept triples, reading them stops: they are too many.

use super::{Base, rules};
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Source;
use crate::rdf::graph::{NodeTriple, blank_max};
use crate::settings::Reasoning;
use crate::syntax::{BlankId, Direction, Iri, Node};
use oxrdf::TermRef;
use rustc_hash::{FxHashMap, FxHashSet};
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
const CHECK_EVERY: usize = 1 << 16;

/// What TurbOWL infers with `rules` from `docs` (the queried documents, in
/// the order the query graph reads them) and `imports` (an import at a
/// queried document's URI left out), the triples about the queried
/// documents' terms only, at most `max` of them. Off infers nothing.
/// `cancel` stops the run (TurbOWL checks it before each round), and
/// `stopped`, asked every [`CHECK_EVERY`] inferred triples read, the reading
/// of its result.
pub(crate) fn infer(
    rules_of: Reasoning,
    docs: &[&Arc<Analysis>],
    imports: &[Arc<Analysis>],
    max: u64,
    cancel: &turbowl::Cancel,
    stopped: &dyn Fn() -> bool,
) -> Result<Closure, Halt> {
    let Some(switches) = rules(rules_of) else {
        return Ok(Closure::default());
    };
    let imports: Vec<&Arc<Analysis>> = imports
        .iter()
        .filter(|i| !docs.iter().any(|d| d.uri == i.uri))
        .collect();
    let sources = docs
        .iter()
        .map(|d| (Source::Open, Arc::clone(d)))
        .chain(imports.iter().map(|i| (Source::External, Arc::clone(i))))
        .collect();
    // Places are not read: any encoding.
    let base = Base::new(sources, Encoding::Utf16);
    let mut reasoner = turbowl::Reasoner::with_switches(switches);
    reasoner.set_base(base.triples());
    reasoner
        .reason_cancellable(cancel)
        .map_err(|_| Halt::Stopped)?;
    // The base's ids of the queried documents' subjects and objects.
    let queried: FxHashSet<u32> = base.in_workspace().flat_map(|(s, o, _)| [s, o]).collect();
    let about = |t: TermRef<'_>| base.id(t).is_some_and(|id| queried.contains(&id));
    let mut blanks = Blanks::new(docs, &imports);
    let mut triples = Vec::new();
    for (i, t) in reasoner.inferred().enumerate() {
        if i % CHECK_EVERY == CHECK_EVERY - 1 && stopped() {
            return Err(Halt::Stopped);
        }
        if !about(t.subject.as_ref().into()) && !about(t.object.as_ref()) {
            continue;
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

/// One past the highest blank node number of `a`, as the graph and the
/// base count it.
fn blank_count(a: &Analysis) -> u32 {
    let mut max = 0;
    for t in a.parsed.triples.iter().filter(|t| !t.unresolved) {
        blank_max(&t.s, &mut max);
        blank_max(&t.o, &mut max);
    }
    max
}

/// Base blank node numbers to the graph's.
struct Blanks {
    /// Per document in the base's order: where its blank nodes start in the
    /// base, and in the graph.
    starts: Vec<(u32, u32)>,
    /// Where the imports' blank nodes start: the queried documents' end.
    end: u32,
    /// The next number for a label the base did not give, and those given.
    next: u32,
    minted: FxHashMap<String, u32>,
}

impl Blanks {
    fn new(docs: &[&Arc<Analysis>], imports: &[&Arc<Analysis>]) -> Blanks {
        let counts: Vec<u32> = docs.iter().map(|d| blank_count(d)).collect();
        let mut graph = Vec::with_capacity(docs.len());
        let mut at = 0u32;
        for c in &counts {
            graph.push(at);
            at = at.saturating_add(*c);
        }
        let end = at;
        // The base's order: by URI, a stable sort, as `Base::new` sorts.
        let mut order: Vec<usize> = (0..docs.len()).collect();
        order.sort_by(|&a, &b| docs[a].uri.cmp(&docs[b].uri));
        let mut starts = Vec::with_capacity(docs.len());
        let mut at = 0u32;
        for i in order {
            starts.push((at, graph[i]));
            at = at.saturating_add(counts[i]);
        }
        let next = imports
            .iter()
            .fold(end, |n, i| n.saturating_add(blank_count(i)));
        Blanks {
            starts,
            end,
            next,
            minted: FxHashMap::default(),
        }
    }

    fn number(&mut self, label: &str) -> u32 {
        let given = label
            .strip_prefix('g')
            .filter(|n| !n.starts_with('0') || *n == "0")
            .and_then(|n| n.parse::<u32>().ok());
        match given {
            Some(n) if n < self.end => {
                // The last document starting at or before `n`: one without
                // blank nodes starts where the next one does.
                let k = self.starts.partition_point(|s| s.0 <= n) - 1;
                let (base, graph) = self.starts[k];
                graph + (n - base)
            }
            Some(n) => n,
            None => {
                let next = &mut self.next;
                *self.minted.entry(label.to_owned()).or_insert_with(|| {
                    let n = *next;
                    *next = next.saturating_add(1);
                    n
                })
            }
        }
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
