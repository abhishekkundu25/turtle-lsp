//! How reasoning writes what a proof names, for hover and diagnostics
//! alike: IRIs by the caller's short names, literals as Turtle writes them,
//! a blank node as `[]` (the base's names for blank nodes are its own, not
//! the user's), and the base triples of a proof as items:
//!
//! - an RDF list is one `( … )`, read from the base, so its `rdf:first` and
//!   `rdf:rest` triples are not items of their own;
//! - a blank node that a base triple of the proof has as its object, and
//!   that other base triples of the proof describe, is written there as
//!   `[ p o ; … ]`;
//! - the base triples about any other blank node are one item,
//!   `[] p o ; q r`.
//!
//! An item is placed where its triples are written: from the first of them
//! to the last in the first one's document, at the first one's line.

use super::Proof;
use super::base::{Base, Origin, RDF_FIRST, RDF_REST};
use crate::syntax::RDF_TYPE;
use lsp_types::Range;
use rustc_hash::{FxHashMap, FxHashSet};
use spargebra::term::{BlankNode, Term};
use turbowl::{Goal, Reason, Statement};

const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
/// How deep blank nodes are written inside each other; deeper ones are `[]`.
const MAX_NESTING: usize = 4;
/// The members a list shows; the rest are `…`.
const MAX_MEMBERS: usize = 20;

/// How an IRI is written: `prefix:local`, or `<iri>`.
pub(crate) type IriName<'a> = &'a dyn Fn(&str) -> String;

/// A proof's base triples as items (see the module doc).
pub(crate) struct Items {
    /// Per step: the item a base step leads; `None` for a rule step and for
    /// a base step written within another one's item.
    lead: Vec<Option<usize>>,
    items: Vec<Item>,
    /// The items in proof order (the tree's, from the goal down), each with
    /// the rule of the step that reads its first triple; `None` for the goal.
    order: Vec<(usize, Option<&'static str>)>,
    /// The base steps about each blank node written inline (`[ … ]`).
    inline: FxHashMap<BlankNode, Vec<usize>>,
    /// The members of the RDF lists the items write, by the cell they are
    /// written from.
    lists: FxHashMap<BlankNode, Vec<Term>>,
}

struct Item {
    what: What,
    origin: Option<Origin>,
}

enum What {
    /// One base step.
    Statement(usize),
    /// The base steps about a blank node.
    About(Vec<usize>),
    /// An RDF list that no item has as its object, from its first cell.
    List(BlankNode),
}

fn blank(t: &Term) -> Option<&BlankNode> {
    match t {
        Term::BlankNode(b) => Some(b),
        _ => None,
    }
}

fn is_iri(t: &Term, iri: &str) -> bool {
    matches!(t, Term::NamedNode(n) if n.as_str() == iri)
}

impl Items {
    pub(super) fn new(proof: &turbowl::Proof, origins: &[Option<Origin>], base: &Base) -> Items {
        let steps = proof.steps();
        let walked = walk(proof);
        let of = |i: usize| &steps[i].statement;
        let is_cell_triple = |s: &Statement| {
            blank(&s.subject).is_some_and(|b| base.is_cell(b))
                && (is_iri(&s.predicate, RDF_FIRST) || is_iri(&s.predicate, RDF_REST))
        };
        // Blank nodes a base triple has as its object, about another subject.
        let objects: FxHashSet<&BlankNode> = walked
            .iter()
            .filter_map(|&(i, _)| {
                let s = of(i);
                blank(&s.object).filter(|o| blank(&s.subject) != Some(o))
            })
            .collect();
        let mut inline: FxHashMap<BlankNode, Vec<usize>> = FxHashMap::default();
        let mut tops = Vec::new();
        for &(i, _) in &walked {
            let s = of(i);
            if is_cell_triple(s) {
                continue;
            }
            match blank(&s.subject).filter(|b| objects.contains(b) && !base.is_cell(b)) {
                Some(b) => inline.entry(b.clone()).or_default().push(i),
                None => tops.push(i),
            }
        }
        // What the other items write: the blank nodes and list cells they
        // reach. A blank node only a cycle reaches is an item of its own.
        let mut reached: FxHashSet<BlankNode> = FxHashSet::default();
        let mut cells: FxHashSet<BlankNode> = FxHashSet::default();
        let mut lists = FxHashMap::default();
        let mut stack: Vec<Term> = tops.iter().map(|&i| of(i).object.clone()).collect();
        while let Some(t) = stack.pop() {
            let Term::BlankNode(b) = t else { continue };
            if !reached.insert(b.clone()) {
                continue;
            }
            if base.is_cell(&b) {
                let list = base.list(&b);
                let mut members = Vec::with_capacity(list.len());
                for (cell, member) in list {
                    cells.insert(cell);
                    stack.push(member.clone());
                    members.push(member);
                }
                lists.insert(b, members);
            } else if let Some(v) = inline.get(&b) {
                stack.extend(v.iter().map(|&i| of(i).object.clone()));
            }
        }
        inline.retain(|b, _| reached.contains(b));

        let mut lead = vec![None; steps.len()];
        let mut items: Vec<(What, Vec<usize>)> = Vec::new();
        let mut order = Vec::new();
        let mut groups: FxHashMap<BlankNode, usize> = FxHashMap::default();
        for &(i, rule) in &walked {
            let s = of(i);
            let group = if is_cell_triple(s) {
                let cell = blank(&s.subject).expect("a cell is a blank node");
                if cells.contains(cell) {
                    continue;
                }
                let head = base.list_head(cell);
                if !lists.contains_key(&head) {
                    let members = base.list(&head).into_iter().map(|(_, m)| m).collect();
                    lists.insert(head.clone(), members);
                }
                Some((head, true))
            } else {
                match blank(&s.subject) {
                    Some(b) if inline.contains_key(b) => continue,
                    Some(b) => Some((b.clone(), false)),
                    None => None,
                }
            };
            let k = match group {
                Some((b, _)) if groups.contains_key(&b) => {
                    items[groups[&b]].1.push(i);
                    continue;
                }
                Some((b, is_list)) => {
                    groups.insert(b.clone(), items.len());
                    let what = if is_list {
                        What::List(b)
                    } else {
                        What::About(Vec::new())
                    };
                    items.push((what, vec![i]));
                    items.len() - 1
                }
                None => {
                    items.push((What::Statement(i), vec![i]));
                    items.len() - 1
                }
            };
            lead[i] = Some(k);
            order.push((k, rule));
        }
        let items = items
            .into_iter()
            .map(|(what, steps)| Item {
                origin: merged(steps.iter().filter_map(|&i| origins[i].as_ref())),
                what: match what {
                    What::About(_) => What::About(steps),
                    w => w,
                },
            })
            .collect();
        Items {
            lead,
            items,
            order,
            inline,
            lists,
        }
    }
}

/// The base steps of `proof` in the tree's order, from the goal down, each
/// once, with the rule of the step that reads it (`None` for the goal).
fn walk(proof: &turbowl::Proof) -> Vec<(usize, Option<&'static str>)> {
    let steps = proof.steps();
    let mut stack: Vec<(usize, Option<&'static str>)> = match (proof.goal(), proof.reason()) {
        (Goal::Diagnostic(_), Reason::Rule { premises, .. }) => {
            premises.iter().rev().map(|&p| (p, None)).collect()
        }
        (Goal::Diagnostic(_), Reason::Base) => Vec::new(),
        (Goal::Statement(_), _) => steps
            .len()
            .checked_sub(1)
            .map(|i| (i, None))
            .into_iter()
            .collect(),
    };
    let mut seen = vec![false; steps.len()];
    let mut out = Vec::new();
    while let Some((i, reading)) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        match &steps[i].reason {
            Reason::Base => out.push((i, reading)),
            Reason::Rule { rule, premises } => {
                stack.extend(premises.iter().rev().map(|&p| (p, Some(*rule))));
            }
        }
    }
    out
}

/// The place of an item's triples: the first one's document and line, from
/// the start of the first of them in that document to the end of the last.
fn merged<'o>(mut origins: impl Iterator<Item = &'o Origin>) -> Option<Origin> {
    let mut out = origins.next()?.clone();
    for o in origins.filter(|o| o.uri == out.uri) {
        let start = o.range.start.min(out.range.start);
        let end = o.range.end.max(out.range.end);
        out.range = Range { start, end };
        out.line = out.line.min(o.line);
    }
    Some(out)
}

/// An item of a proof: its place, and the rule of the step that reads it.
pub(crate) struct Leaf<'p> {
    proof: &'p Proof,
    item: usize,
    /// The rule of the step that reads it; `None` when the goal does.
    pub(crate) rule: Option<&'static str>,
}

impl<'p> Leaf<'p> {
    /// Where it is written; `None` for a triple that is not in the base.
    pub(crate) fn origin(&self) -> Option<&'p Origin> {
        self.proof.written.items[self.item].origin.as_ref()
    }

    /// The item as Turtle: `ex:rex a ex:Dog`, or
    /// `[] a owl:AllDisjointClasses ; owl:members ( ex:A ex:B )`.
    pub(crate) fn text(&self, iri: IriName<'_>) -> String {
        self.proof.item_text(self.item, iri)
    }
}

impl Proof {
    /// The items of the base triples the proof rests on, in proof order.
    pub(crate) fn leaves(&self) -> impl Iterator<Item = Leaf<'_>> {
        self.written.order.iter().map(|&(item, rule)| Leaf {
            proof: self,
            item,
            rule,
        })
    }

    /// The item base step `i` leads; `None` for one written within another
    /// item (and for a rule step).
    pub(super) fn lead(&self, i: usize) -> Option<Leaf<'_>> {
        let item = self.written.lead[i]?;
        Some(Leaf {
            proof: self,
            item,
            rule: None,
        })
    }

    /// Whether a triple of a document is among the triples the proof rests
    /// on (not so for a rule with no premises, as `dt-type1`).
    pub(crate) fn rests_on_documents(&self) -> bool {
        self.written.items.iter().any(|i| i.origin.is_some())
    }

    fn item_text(&self, k: usize, iri: IriName<'_>) -> String {
        let steps = self.proof.steps();
        match &self.written.items[k].what {
            What::Statement(i) => self.statement(&steps[*i].statement, iri),
            What::About(v) => format!("[] {}", self.pairs(v, iri, 0)),
            What::List(head) => self.object(&Term::BlankNode(head.clone()), iri, 0),
        }
    }

    /// A statement as Turtle: `ex:rex a ex:Dog`.
    pub(crate) fn statement(&self, s: &Statement, iri: IriName<'_>) -> String {
        format!(
            "{} {} {}",
            term(&s.subject, iri),
            predicate(&s.predicate, iri),
            self.object(&s.object, iri, 0)
        )
    }

    /// `p o ; q r` of steps `v`.
    fn pairs(&self, v: &[usize], iri: IriName<'_>, depth: usize) -> String {
        let steps = self.proof.steps();
        v.iter()
            .map(|&i| {
                let s = &steps[i].statement;
                format!(
                    "{} {}",
                    predicate(&s.predicate, iri),
                    self.object(&s.object, iri, depth)
                )
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    fn object(&self, t: &Term, iri: IriName<'_>, depth: usize) -> String {
        let w = &self.written;
        match t {
            Term::NamedNode(n) if n.as_str() == RDF_NIL => "()".into(),
            Term::BlankNode(b) if depth < MAX_NESTING => {
                if let Some(members) = w.lists.get(b) {
                    let mut parts: Vec<String> = members
                        .iter()
                        .take(MAX_MEMBERS)
                        .map(|m| self.object(m, iri, depth + 1))
                        .collect();
                    if members.len() > MAX_MEMBERS {
                        parts.push("…".into());
                    }
                    format!("( {} )", parts.join(" "))
                } else if let Some(v) = w.inline.get(b) {
                    format!("[ {} ]", self.pairs(v, iri, depth + 1))
                } else {
                    "[]".into()
                }
            }
            t => term(t, iri),
        }
    }
}

fn predicate(p: &Term, iri: IriName<'_>) -> String {
    if is_iri(p, RDF_TYPE) {
        "a".into()
    } else {
        term(p, iri)
    }
}

/// A term as Turtle, a blank node as `[]`.
pub(crate) fn term(t: &Term, iri: IriName<'_>) -> String {
    match t {
        Term::NamedNode(n) => iri(n.as_str()),
        Term::BlankNode(_) => "[]".into(),
        Term::Literal(l) => {
            let lexical = spargebra::term::Literal::new_simple_literal(l.value()).to_string();
            if let Some(lang) = l.language() {
                format!("{lexical}@{lang}")
            } else if l.datatype().as_str() == "http://www.w3.org/2001/XMLSchema#string" {
                lexical
            } else {
                format!("{lexical}^^{}", iri(l.datatype().as_str()))
            }
        }
        Term::Triple(t) => format!(
            "<<( {} {} {} )>>",
            term(&t.subject.clone().into(), iri),
            iri(t.predicate.as_str()),
            term(&t.object, iri)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_are_written_as_turtle() {
        let iri = |i: &str| format!("<{i}>");
        let lit = |l: spargebra::term::Literal| term(&l.into(), &iri);
        use spargebra::term::{Literal, NamedNode};
        assert_eq!(lit(Literal::new_simple_literal("a\"b")), "\"a\\\"b\"");
        assert_eq!(
            lit(Literal::new_language_tagged_literal_unchecked("chat", "fr")),
            "\"chat\"@fr"
        );
        assert_eq!(
            lit(Literal::new_typed_literal(
                "1",
                NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer")
            )),
            "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
        assert_eq!(term(&BlankNode::new_unchecked("g1").into(), &iri), "[]");
    }
}
