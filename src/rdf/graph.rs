//! The graph SHACL validation and SPARQL queries read: the union of several
//! documents' triples, with blank nodes kept apart per document. Every
//! distinct term gets a dense [`Id`]; literals are borrowed from the
//! documents' parses, never copied. Triples are indexed by subject and object
//! (compressed rows) and by predicate.

use crate::syntax::{BlankId, Direction, Iri, Node, Triple};
use rustc_hash::{FxHashMap, FxHasher};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

/// A term of the graph, numbered densely from 0.
pub type Id = u32;

/// A literal's lexical form, datatype, language tag and base direction.
pub type LiteralKey<'a> = (&'a str, Iri, Option<Iri>, Option<Direction>);

/// A graph's own literals, found by value: by the hash of the value, then
/// compared with the term. It borrows nothing (a map keyed by the borrowed
/// text would make [`Graph`] invariant in its lifetime).
#[derive(Default)]
struct LiteralIds {
    by_hash: FxHashMap<u64, Id>,
    /// Literals whose hash another one has already.
    collided: Vec<Id>,
}

fn literal_hash(key: &LiteralKey) -> u64 {
    let mut h = FxHasher::default();
    key.hash(&mut h);
    h.finish()
}

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
                ..
            }) => Some((value, *datatype, *lang)),
            _ => None,
        }
    }

    /// The base direction of a directional language-tagged string.
    pub fn direction(&self) -> Option<Direction> {
        match self {
            Term::Value(Node::Literal { dir, .. }) => *dir,
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

/// Triple indexes grouped by one position: dense rows of `items`,
/// `offsets[id]..offsets[id + 1]`, for a graph built whole; a sparse map for an
/// overlay, whose few triples mention terms spread over the whole base.
#[derive(Default)]
struct Rows {
    offsets: Vec<u32>,
    items: Vec<u32>,
    sparse: FxHashMap<Id, Vec<u32>>,
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
        Rows {
            offsets,
            items,
            sparse: FxHashMap::default(),
        }
    }

    fn get(&self, id: Id) -> &[u32] {
        if !self.sparse.is_empty()
            && let Some(v) = self.sparse.get(&id)
        {
            return v;
        }
        let i = id as usize;
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(&a), Some(&b)) => &self.items[a as usize..b as usize],
            _ => &[],
        }
    }
}

/// The triples one row of an index names: the overlay's, then the base's.
struct Adjacent<'g> {
    own: std::slice::Iter<'g, u32>,
    base: std::slice::Iter<'g, u32>,
    own_triples: &'g [GTriple],
    /// The number of the overlay's first triple.
    own_from: u32,
    base_triples: &'g [GTriple],
}

impl<'g> Iterator for Adjacent<'g> {
    type Item = (u32, &'g GTriple);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(&i) = self.own.next() {
            return Some((i, &self.own_triples[(i - self.own_from) as usize]));
        }
        let &i = self.base.next()?;
        Some((i, &self.base_triples[i as usize]))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.own.len() + self.base.len();
        (n, Some(n))
    }
}

/// A graph. [`Graph::new`] builds one whole; [`Graph::overlay`] adds one
/// document's triples to a finished graph (the base, shared) without copying
/// or re-indexing it, so many small datasets can each be read together with
/// the same large graph at a cost in proportion to their own size.
///
/// Term and triple numbers continue the base's: an overlay's own start where
/// the base's end. What a lookup yields from the overlay comes before what it
/// yields from the base.
pub struct Graph<'a> {
    /// The finished graph this one adds to; never itself an overlay.
    base: Option<Arc<Graph<'a>>>,
    /// The number of terms and of triples in the base.
    term_off: u32,
    tri_off: u32,
    /// Terms this graph added, numbered from `term_off`.
    terms: Vec<Term<'a>>,
    /// Term -> id, for the terms in `terms`. The `u32` scopes triple terms
    /// that contain blank nodes to their document (u32::MAX otherwise), as
    /// blank nodes themselves are.
    ids: FxHashMap<(Term<'a>, u32), Id>,
    /// Triples this graph added, numbered from `tri_off`.
    triples: Vec<GTriple>,
    /// Per triple added: (document, index into that document's triples).
    origin: Vec<(u32, u32)>,
    by_s: Rows,
    by_o: Rows,
    by_p: FxHashMap<Iri, Vec<u32>>,
    /// Per document (the base's first): the number its blank nodes are offset by.
    blank_base: Vec<u32>,
    /// The number the next document's blank nodes would be offset by.
    blank_end: u32,
    /// Per triple term with blank nodes: the offset of its document, so the
    /// blank nodes inside carry the numbers the same nodes have outside.
    term_base: FxHashMap<Id, u32>,
    /// The literals among `terms`, by value, once one is looked for
    /// ([`Graph::literal`]).
    literals: OnceLock<LiteralIds>,
}

#[cfg(test)]
thread_local! {
    /// Triples this thread has indexed into a graph, to tell what a run costs.
    pub static INDEXED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Terms this thread has looked at to find literals by value.
    pub static TERMS_WALKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

impl<'a> Graph<'a> {
    /// The union of `docs`; a blank node of one document never equals one of
    /// another. Triple terms are compared by content. Triples with an
    /// undeclared prefix ([`Triple::unresolved`]) are left out.
    pub fn new(docs: &[&'a [Triple]]) -> Graph<'a> {
        let total: usize = docs.iter().map(|d| d.len()).sum();
        let mut g = Graph::empty(None, total, docs.len());
        for triples in docs {
            g.add_document::<false>(triples);
        }
        let n = g.terms.len();
        g.by_s = Rows::build(n, g.triples.iter().map(|t| t.s));
        g.by_o = Rows::build(n, g.triples.iter().map(|t| t.o));
        g
    }

    /// `base` with one more document, `triples`, which is document number
    /// [`Graph::documents`] of `base` in [`Graph::origin`] and the blank nodes.
    /// Its blank nodes are its own. Costs in proportion to `triples`.
    pub fn overlay(base: &Arc<Graph<'a>>, triples: &'a [Triple]) -> Graph<'a> {
        debug_assert!(base.base.is_none(), "an overlay of an overlay");
        let mut g = Graph::empty(Some(base.clone()), triples.len(), 1);
        g.add_document::<true>(triples);
        g
    }

    fn empty(base: Option<Arc<Graph<'a>>>, triples: usize, docs: usize) -> Graph<'a> {
        let (term_off, tri_off, blank_base, blank_end) = match &base {
            Some(b) => (
                b.term_off + b.terms.len() as u32,
                b.tri_off + b.triples.len() as u32,
                b.blank_base.clone(),
                b.blank_end,
            ),
            None => (0, 0, Vec::with_capacity(docs), 0),
        };
        Graph {
            base,
            term_off,
            tri_off,
            terms: Vec::new(),
            ids: FxHashMap::default(),
            triples: Vec::with_capacity(triples),
            origin: Vec::with_capacity(triples),
            by_s: Rows::default(),
            by_o: Rows::default(),
            by_p: FxHashMap::default(),
            blank_base,
            blank_end,
            term_base: FxHashMap::default(),
            literals: OnceLock::new(),
        }
    }

    /// `OVERLAY`: the graph has a base (a separate path, so that building a
    /// graph whole does not pay for it).
    fn add_document<const OVERLAY: bool>(&mut self, triples: &'a [Triple]) {
        let base = self.blank_end;
        let d = self.blank_base.len() as u32;
        self.blank_base.push(base);
        let mut max = 0u32;
        for (i, t) in triples.iter().enumerate() {
            if t.unresolved {
                continue;
            }
            let s = self.add::<OVERLAY>(base, &t.s, &mut max);
            let o = self.add::<OVERLAY>(base, &t.o, &mut max);
            let at = self.tri_off + self.triples.len() as u32;
            self.by_p.entry(t.p).or_default().push(at);
            if OVERLAY {
                self.by_s.sparse.entry(s).or_default().push(at);
                self.by_o.sparse.entry(o).or_default().push(at);
            }
            self.triples.push(GTriple { s, p: t.p, o });
            self.origin.push((d, i as u32));
            #[cfg(test)]
            INDEXED.with(|c| c.set(c.get() + 1));
        }
        self.blank_end = base.saturating_add(max);
    }

    fn add<const OVERLAY: bool>(&mut self, base: u32, n: &'a Node, max: &mut u32) -> Id {
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
        let id = self.intern::<OVERLAY>(term, scope);
        if scope != u32::MAX {
            self.term_base.entry(id).or_insert(base);
        }
        id
    }

    fn intern<const OVERLAY: bool>(&mut self, t: Term<'a>, scope: u32) -> Id {
        if OVERLAY && let Some(&id) = self.base.as_ref().and_then(|b| b.ids.get(&(t, scope))) {
            return id;
        }
        if let Some(&id) = self.ids.get(&(t, scope)) {
            return id;
        }
        let id = self.term_off + self.terms.len() as Id;
        self.terms.push(t);
        self.ids.insert((t, scope), id);
        id
    }

    pub fn term(&self, id: Id) -> Term<'a> {
        match id.checked_sub(self.term_off) {
            Some(i) => self.terms[i as usize],
            None => self.base.as_ref().expect("a base term").terms[id as usize],
        }
    }

    /// The number of triples.
    pub fn len(&self) -> usize {
        self.tri_off as usize + self.triples.len()
    }

    /// The number of documents (of the base and the overlay).
    pub fn documents(&self) -> u32 {
        self.blank_base.len() as u32
    }

    /// The id of the literal `key`, if the graph has it. The first look
    /// walks the terms this graph added, once: an overlay walks its own, and
    /// the shared base walks its terms once for all its overlays.
    pub fn literal(&self, key: &LiteralKey) -> Option<Id> {
        self.own_literal(key)
            .or_else(|| self.base.as_ref()?.own_literal(key))
    }

    fn own_literal(&self, key: &LiteralKey) -> Option<Id> {
        let ids = self.literals.get_or_init(|| {
            let mut ids = LiteralIds::default();
            for (i, t) in self.terms.iter().enumerate() {
                #[cfg(test)]
                TERMS_WALKED.with(|c| c.set(c.get() + 1));
                let Some((v, d, l)) = t.literal() else {
                    continue;
                };
                let id = self.term_off + i as Id;
                // Terms are distinct: a hash already taken is a collision.
                match ids.by_hash.entry(literal_hash(&(v, d, l, t.direction()))) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(id);
                    }
                    std::collections::hash_map::Entry::Occupied(_) => ids.collided.push(id),
                }
            }
            ids
        });
        let is = |id: &Id| {
            let t = self.term(*id);
            t.literal()
                .is_some_and(|(v, d, l)| (v, d, l, t.direction()) == *key)
        };
        ids.by_hash
            .get(&literal_hash(key))
            .filter(|id| is(id))
            .or_else(|| ids.collided.iter().find(|id| is(id)))
            .copied()
    }

    /// Every term with its id, the overlay's first.
    pub fn terms(&self) -> impl Iterator<Item = (Id, Term<'a>)> + '_ {
        let base: &[Term<'a>] = self.base.as_deref().map_or(&[], |b| &b.terms);
        self.terms
            .iter()
            .enumerate()
            .map(|(i, t)| (self.term_off + i as Id, *t))
            .chain(base.iter().enumerate().map(|(i, t)| (i as Id, *t)))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Triple number `t`.
    pub fn triple_at(&self, t: u32) -> &GTriple {
        match t.checked_sub(self.tri_off) {
            Some(i) => &self.triples[i as usize],
            None => &self.base.as_ref().expect("a base triple").triples[t as usize],
        }
    }

    /// The (document, index into that document's triples) of triple `t`.
    pub fn origin(&self, t: u32) -> (u32, u32) {
        match t.checked_sub(self.tri_off) {
            Some(i) => self.origin[i as usize],
            None => self.base.as_ref().expect("a base triple").origin[t as usize],
        }
    }

    /// Every triple, the overlay's first.
    pub fn all(&self) -> impl Iterator<Item = &GTriple> {
        self.triples
            .iter()
            .chain(self.base.iter().flat_map(|b| b.triples.iter()))
    }

    /// The triples the overlay added (all of a graph built whole), with their
    /// numbers.
    pub fn added(&self) -> impl Iterator<Item = (u32, &GTriple)> {
        self.triples
            .iter()
            .enumerate()
            .map(|(i, t)| (self.tri_off + i as u32, t))
    }

    pub fn id(&self, t: Term<'a>) -> Option<Id> {
        // Lookups by content: triple terms with blank nodes are per document.
        let key = (t, u32::MAX);
        self.ids
            .get(&key)
            .or_else(|| self.base.as_ref()?.ids.get(&key))
            .copied()
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

    /// The number added to the blank nodes inside triple term `id` to get
    /// their numbers in the graph (0 for a term without blank nodes).
    pub fn term_blank_base(&self, id: Id) -> u32 {
        self.term_base
            .get(&id)
            .or_else(|| self.base.as_ref()?.term_base.get(&id))
            .copied()
            .unwrap_or(0)
    }

    /// The (document, number in that document) of blank node `Term::Blank(b)`.
    pub fn blank_origin(&self, b: u32) -> (u32, u32) {
        // A document without blank nodes shares its base with the next one.
        let d = self.blank_base.partition_point(|&base| base <= b).max(1) - 1;
        (d as u32, b - self.blank_base.get(d).copied().unwrap_or(0))
    }

    pub fn as_iri(&self, id: Id) -> Option<Iri> {
        match self.term(id) {
            Term::Iri(i) => Some(i),
            _ => None,
        }
    }

    /// The triples in `own` (the overlay's rows) and `base` (the base's),
    /// the overlay's first, each with its number.
    fn adjacent<'g>(&'g self, own: &'g [u32], base: &'g [u32]) -> Adjacent<'g> {
        Adjacent {
            own: own.iter(),
            base: base.iter(),
            own_triples: &self.triples,
            own_from: self.tri_off,
            base_triples: self.base.as_deref().map_or(&[], |b| &b.triples),
        }
    }

    /// Triples with subject `s`.
    pub fn out(&self, s: Id) -> impl Iterator<Item = (u32, &GTriple)> {
        let base = self.base.as_deref().map_or(&[][..], |b| b.by_s.get(s));
        self.adjacent(self.by_s.get(s), base)
    }

    /// Triples with object `o`.
    pub fn incoming(&self, o: Id) -> impl Iterator<Item = (u32, &GTriple)> {
        let base = self.base.as_deref().map_or(&[][..], |b| b.by_o.get(o));
        self.adjacent(self.by_o.get(o), base)
    }

    /// Triples with predicate `p`.
    pub fn with_p(&self, p: Iri) -> impl Iterator<Item = &GTriple> {
        let own = self.by_p.get(&p).map_or(&[][..], Vec::as_slice);
        let base = self
            .base
            .as_ref()
            .and_then(|b| b.by_p.get(&p))
            .map_or(&[][..], Vec::as_slice);
        self.adjacent(own, base).map(|t| t.1)
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
        let v = list_vocab();
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

/// `rdf:first`, `rdf:rest` and `rdf:nil`, interned once.
struct ListVocab {
    rdf_first: Iri,
    rdf_rest: Iri,
    rdf_nil: Iri,
}

fn list_vocab() -> &'static ListVocab {
    static V: std::sync::OnceLock<ListVocab> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let rdf = |l: &str| Iri::intern(&format!("{}{l}", crate::syntax::RDF));
        ListVocab {
            rdf_first: rdf("first"),
            rdf_rest: rdf("rest"),
            rdf_nil: rdf("nil"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{Direction, PrefixMap, Syntax, parse};

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

    #[test]
    fn triples_with_an_undeclared_prefix_stay_out_of_the_graph() {
        let p = parse(
            "<http://e/a> <http://e/p> nope:o , <http://e/o> .\nnope:s <http://e/p> <http://e/o> .\n<http://e/b> <http://e/p> <http://e/o> .",
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        assert_eq!(p.triples.len(), 4);
        let g = Graph::new(&[&p.triples]);
        assert_eq!(g.len(), 2);
        // `origin` still names the triple's place in the document.
        assert_eq!(g.origin, [(0, 1), (0, 3)]);
    }

    #[test]
    fn a_literal_has_a_direction_apart_from_its_tag() {
        let p = parse(
            "<http://e/a> <http://e/p> \"x\"@ar--rtl , \"x\"@ar--ltr , \"x\"@ar .",
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        let g = Graph::new(&[&p.triples]);
        // Three literals: the tag alone does not tell them apart.
        let objects: Vec<_> = g.triples.iter().map(|t| g.term(t.o)).collect();
        assert_eq!(g.terms().count(), 4);
        let dirs: Vec<_> = objects
            .iter()
            .map(|t| (t.literal().unwrap().2.map(|l| l.to_string()), t.direction()))
            .collect();
        let ar = Some("ar".to_string());
        assert_eq!(
            dirs,
            [
                (ar.clone(), Some(Direction::Rtl)),
                (ar.clone(), Some(Direction::Ltr)),
                (ar, None)
            ]
        );
    }

    /// A term as text, with blank nodes named by their document.
    fn show(g: &Graph, id: Id) -> String {
        match g.term(id) {
            Term::Iri(i) => i.to_string(),
            Term::Blank(b) => {
                let (d, n) = g.blank_origin(b);
                format!("_:{d}.{n}")
            }
            Term::Value(n) => format!("{n:?}"),
        }
    }

    fn shown(g: &Graph, t: &GTriple) -> String {
        format!("{} {} {}", show(g, t.s), t.p, show(g, t.o))
    }

    /// Reading an overlay is reading the graph of all its documents built
    /// whole, with blank nodes still apart per document; the overlay's own
    /// triples come first.
    #[test]
    fn an_overlay_reads_as_the_union_of_its_documents() {
        let parse = |t: &str| parse(t, &PrefixMap::default(), Syntax::Turtle);
        let a = parse("<http://e/a> <http://e/p> _:b , \"x\" .\n_:b <http://e/q> ( 1 2 ) .");
        let b = parse(
            "<http://e/a> <http://e/p> _:b , \"y\" .\n_:b <http://e/q> \"x\" .\n<http://e/c> <http://e/r> <http://e/a> .",
        );
        let c = parse(
            "_:b <http://e/q> <http://e/c> , \"x\" .\n<http://e/a> <http://e/p> \"y\" , _:b .\nnope:s <http://e/p> \"x\" .",
        );
        let base = Arc::new(Graph::new(&[&a.triples, &b.triples]));
        let base_len = base.len();
        let over = Graph::overlay(&base, &c.triples);
        let whole = Graph::new(&[&a.triples, &b.triples, &c.triples]);
        assert_eq!(over.len(), whole.len());
        assert_eq!(over.documents(), 3);
        assert_eq!(base.len(), base_len, "the base is not changed");
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        let all = |g: &Graph| sorted(g.all().map(|t| shown(g, t)).collect());
        assert_eq!(all(&over), all(&whole));
        assert_eq!(over.all().count(), over.len());
        // By subject, object and predicate.
        for (id, _) in whole.terms() {
            let name = show(&whole, id);
            let find =
                |g: &Graph| -> Option<Id> { g.terms().map(|t| t.0).find(|&i| show(g, i) == name) };
            let o = find(&over).unwrap_or_else(|| panic!("{name} is in the overlay"));
            let out = |g: &Graph, id| sorted(g.out(id).map(|t| shown(g, t.1)).collect());
            let inc = |g: &Graph, id| sorted(g.incoming(id).map(|t| shown(g, t.1)).collect());
            assert_eq!(out(&over, o), out(&whole, id), "out of {name}");
            assert_eq!(inc(&over, o), inc(&whole, id), "incoming of {name}");
        }
        let p = Iri::intern("http://e/p");
        let with_p = |g: &Graph| sorted(g.with_p(p).map(|t| shown(g, t)).collect());
        assert_eq!(with_p(&over), with_p(&whole));
        // Triple numbers lead to the same triple and place in a document.
        for (t, gt) in over.added() {
            assert_eq!(over.origin(t).0, 2);
            assert_eq!(shown(&over, over.triple_at(t)), shown(&over, gt));
        }
        for t in 0..over.len() as u32 {
            let (d, i) = over.origin(t);
            let doc = [&a, &b, &c][d as usize];
            assert_eq!(doc.triples[i as usize].p, over.triple_at(t).p);
        }
        // The overlay's triples come first.
        let first = over
            .out(over.iri(Iri::intern("http://e/a")).unwrap())
            .next()
            .unwrap()
            .0;
        assert_eq!(over.origin(first).0, 2);
        // Blank nodes of the three documents are three nodes.
        let blanks = over
            .terms()
            .filter(|t| matches!(t.1, Term::Blank(_)))
            .count();
        assert_eq!(
            blanks,
            whole
                .terms()
                .filter(|t| matches!(t.1, Term::Blank(_)))
                .count()
        );
        // Nodes resolve by document, the overlay's included.
        let nb = &c.triples[0].s;
        let id = over.node(2, nb).unwrap();
        assert_eq!(
            over.blank_origin(match over.term(id) {
                Term::Blank(b) => b,
                _ => unreachable!(),
            })
            .0,
            2
        );
        // An undeclared prefix stays out here as well.
        assert!(over.all().all(|t| shown(&over, t) != "nope:s"));
    }
}
