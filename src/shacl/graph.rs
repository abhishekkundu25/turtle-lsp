//! The graph SHACL validation reads: the union of several documents' triples,
//! with blank nodes kept apart per document. Every distinct term gets a dense
//! [`Id`]; literals are borrowed from the documents' parses, never copied.
//! Triples are indexed by subject and object (compressed rows) and by
//! predicate.

use crate::syntax::{BlankId, Iri, Node, Triple};
use rustc_hash::FxHashMap;
use std::hash::{Hash, Hasher};

/// A term of the graph, numbered densely from 0.
pub type Id = u32;

/// What an [`Id`] stands for.
#[derive(Clone, Copy, Debug)]
pub enum Term<'a> {
    Iri(Iri),
    /// A blank node, numbered across the graph.
    Blank(u32),
    /// A literal or a triple term, borrowed from its document's parse.
    Value(&'a Node),
}

impl PartialEq for Term<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Term::Iri(a), Term::Iri(b)) => a == b,
            (Term::Blank(a), Term::Blank(b)) => a == b,
            (Term::Value(a), Term::Value(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Term<'_> {}

impl Hash for Term<'_> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        match self {
            Term::Iri(i) => (0u8, i).hash(h),
            Term::Blank(b) => (1u8, b).hash(h),
            Term::Value(n) => (2u8, n).hash(h),
        }
    }
}

impl<'a> Term<'a> {
    /// The literal's lexical form, datatype and language tag.
    pub fn literal(&self) -> Option<(&'a str, Iri, Option<Iri>)> {
        match self {
            Term::Value(Node::Literal {
                value,
                datatype,
                lang,
            }) => Some((value, *datatype, *lang)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GTriple {
    pub s: Id,
    pub p: Iri,
    pub o: Id,
}

/// Triple indexes grouped by one position: rows of `items`, `offsets[id]..offsets[id + 1]`.
#[derive(Default)]
struct Rows {
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl Rows {
    fn build(n_terms: usize, keys: impl Iterator<Item = Id> + Clone) -> Rows {
        let mut offsets = vec![0u32; n_terms + 1];
        for k in keys.clone() {
            offsets[k as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut fill = offsets.clone();
        let mut items = vec![0u32; offsets[n_terms] as usize];
        for (t, k) in keys.enumerate() {
            let slot = &mut fill[k as usize];
            items[*slot as usize] = t as u32;
            *slot += 1;
        }
        Rows { offsets, items }
    }

    fn get(&self, id: Id) -> &[u32] {
        let i = id as usize;
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(&a), Some(&b)) => &self.items[a as usize..b as usize],
            _ => &[],
        }
    }
}

pub struct Graph<'a> {
    terms: Vec<Term<'a>>,
    /// Term -> id. The `u32` scopes triple terms that contain blank nodes to
    /// their document (u32::MAX otherwise), as blank nodes themselves are.
    ids: FxHashMap<(Term<'a>, u32), Id>,
    pub triples: Vec<GTriple>,
    /// Per triple: (document, index into that document's triples).
    pub origin: Vec<(u32, u32)>,
    by_s: Rows,
    by_o: Rows,
    by_p: FxHashMap<Iri, Vec<u32>>,
    /// Per document: the number its blank nodes are offset by.
    blank_base: Vec<u32>,
}

impl<'a> Graph<'a> {
    /// The union of `docs`; a blank node of one document never equals one of
    /// another. Triple terms are compared by content.
    pub fn new(docs: &[&'a [Triple]]) -> Graph<'a> {
        let total: usize = docs.iter().map(|d| d.len()).sum();
        let mut g = Graph {
            terms: Vec::new(),
            ids: FxHashMap::default(),
            triples: Vec::with_capacity(total),
            origin: Vec::with_capacity(total),
            by_s: Rows::default(),
            by_o: Rows::default(),
            by_p: FxHashMap::default(),
            blank_base: Vec::with_capacity(docs.len()),
        };
        let mut base = 0u32;
        for (d, triples) in docs.iter().enumerate() {
            g.blank_base.push(base);
            let mut max = 0u32;
            for (i, t) in triples.iter().enumerate() {
                let s = g.add(base, &t.s, &mut max);
                let o = g.add(base, &t.o, &mut max);
                g.by_p.entry(t.p).or_default().push(g.triples.len() as u32);
                g.triples.push(GTriple { s, p: t.p, o });
                g.origin.push((d as u32, i as u32));
            }
            base = base.saturating_add(max);
        }
        let n = g.terms.len();
        g.by_s = Rows::build(n, g.triples.iter().map(|t| t.s));
        g.by_o = Rows::build(n, g.triples.iter().map(|t| t.o));
        g
    }

    fn add(&mut self, base: u32, n: &'a Node, max: &mut u32) -> Id {
        let term = match n {
            Node::Iri(i) => Term::Iri(*i),
            Node::Blank(BlankId(b)) => {
                *max = (*max).max(b.saturating_add(1));
                Term::Blank(base.saturating_add(*b))
            }
            Node::Literal { .. } | Node::Triple(_) => Term::Value(n),
        };
        let scope = if matches!(n, Node::Triple(_)) && has_blank(n) {
            // Count the blank nodes inside, so the next document's start after them.
            let mut b = 0;
            blank_max(n, &mut b);
            *max = (*max).max(b);
            base
        } else {
            u32::MAX
        };
        self.intern(term, scope)
    }

    fn intern(&mut self, t: Term<'a>, scope: u32) -> Id {
        if let Some(&id) = self.ids.get(&(t, scope)) {
            return id;
        }
        let id = self.terms.len() as Id;
        self.terms.push(t);
        self.ids.insert((t, scope), id);
        id
    }

    pub fn term(&self, id: Id) -> Term<'a> {
        self.terms[id as usize]
    }

    pub fn len(&self) -> usize {
        self.triples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.triples.is_empty()
    }

    pub fn id(&self, t: Term<'a>) -> Option<Id> {
        // Lookups by content: triple terms with blank nodes are per document.
        self.ids.get(&(t, u32::MAX)).copied()
    }

    pub fn iri(&self, i: Iri) -> Option<Id> {
        self.id(Term::Iri(i))
    }

    /// The term for node `n` of document `doc`, if the graph has it.
    pub fn node(&self, doc: u32, n: &'a Node) -> Option<Id> {
        match n {
            Node::Iri(i) => self.iri(*i),
            Node::Blank(BlankId(b)) => {
                let base = *self.blank_base.get(doc as usize)?;
                self.id(Term::Blank(base.saturating_add(*b)))
            }
            _ => self.id(Term::Value(n)),
        }
    }

    pub fn as_iri(&self, id: Id) -> Option<Iri> {
        match self.term(id) {
            Term::Iri(i) => Some(i),
            _ => None,
        }
    }

    /// Triples with subject `s`.
    pub fn out(&self, s: Id) -> impl Iterator<Item = (u32, &GTriple)> {
        self.by_s
            .get(s)
            .iter()
            .map(|&i| (i, &self.triples[i as usize]))
    }

    /// Triples with object `o`.
    pub fn incoming(&self, o: Id) -> impl Iterator<Item = (u32, &GTriple)> {
        self.by_o
            .get(o)
            .iter()
            .map(|&i| (i, &self.triples[i as usize]))
    }

    /// Triples with predicate `p`.
    pub fn with_p(&self, p: Iri) -> impl Iterator<Item = &GTriple> {
        self.by_p
            .get(&p)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|&i| &self.triples[i as usize])
    }

    pub fn objects(&self, s: Id, p: Iri) -> impl Iterator<Item = Id> {
        self.out(s).filter(move |t| t.1.p == p).map(|t| t.1.o)
    }

    pub fn subjects(&self, p: Iri, o: Id) -> impl Iterator<Item = Id> {
        self.incoming(o).filter(move |t| t.1.p == p).map(|t| t.1.s)
    }

    pub fn object(&self, s: Id, p: Iri) -> Option<Id> {
        self.objects(s, p).next()
    }

    /// The first triple `(s, p, ?)`.
    pub fn triple(&self, s: Id, p: Iri) -> Option<u32> {
        self.out(s).find(|t| t.1.p == p).map(|t| t.0)
    }

    /// The members of the RDF collection starting at `head`, in order
    /// (cycle-safe; stops at the first malformed cell).
    pub fn list(&self, head: Id) -> Vec<Id> {
        let v = super::shapes::vocab();
        let mut out = Vec::new();
        let mut cur = head;
        let mut seen = rustc_hash::FxHashSet::default();
        while self.as_iri(cur) != Some(v.rdf_nil) && seen.insert(cur) {
            let (Some(first), Some(rest)) =
                (self.object(cur, v.rdf_first), self.object(cur, v.rdf_rest))
            else {
                if let Some(first) = self.object(cur, v.rdf_first) {
                    out.push(first);
                }
                break;
            };
            out.push(first);
            cur = rest;
        }
        out
    }
}

/// Whether a triple term contains a blank node (anywhere inside).
fn has_blank(n: &Node) -> bool {
    match n {
        Node::Triple(t) => has_blank(&t.0) || has_blank(&t.2),
        Node::Blank(_) => true,
        _ => false,
    }
}

fn blank_max(n: &Node, max: &mut u32) {
    match n {
        Node::Triple(t) => {
            blank_max(&t.0, max);
            blank_max(&t.2, max);
        }
        Node::Blank(BlankId(b)) => *max = (*max).max(b.saturating_add(1)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{PrefixMap, Syntax, parse};

    #[test]
    fn blank_nodes_stay_apart_per_document() {
        let a = parse(
            "_:b <http://e/p> \"x\" .",
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        let b = parse(
            "_:b <http://e/p> \"x\" .",
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        let g = Graph::new(&[&a.triples, &b.triples]);
        assert_eq!(g.len(), 2);
        assert_ne!(g.triples[0].s, g.triples[1].s);
        // Equal literals are one term.
        assert_eq!(g.triples[0].o, g.triples[1].o);
        assert_eq!(g.out(g.triples[1].s).count(), 1);
        assert_eq!(g.incoming(g.triples[0].o).count(), 2);
    }

    #[test]
    fn lists() {
        let p = parse(
            "<http://e/s> <http://e/p> ( 1 2 3 ) .",
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        let g = Graph::new(&[&p.triples]);
        let head = g.object(
            g.iri(Iri::intern("http://e/s")).unwrap(),
            Iri::intern("http://e/p"),
        );
        assert_eq!(g.list(head.unwrap()).len(), 3);
    }
}
