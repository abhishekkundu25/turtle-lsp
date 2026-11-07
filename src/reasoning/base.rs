//! The base: the workspace's triples as TurbOWL reads them, and where each
//! one is written.
//!
//! The documents (imports included) are put in `(Source, uri)` order, as
//! SHACL validation sorts them, and read into one base: blank nodes are
//! kept apart per document, numbered as the graph of SHACL and queries
//! numbers them ([`blank_max`]) and named `g<n>` as its SPARQL dataset
//! names them ([`blank_label`], [`node_term`]), so the same documents give
//! the same base, blank node names included, whatever order they come in. TriG and N-Quads
//! graphs are merged into that one graph. A triple written twice is one
//! base triple: proofs link to its first place, and findings are shown at
//! every place.
//!
//! Each distinct term is held once ([`Terms`]), by an id, and a triple is
//! three ids: an IRI is its interned [`Iri`], a literal is read from where
//! the document writes it, and only blank node labels and triple terms are
//! copied. TurbOWL and the readers get borrowed terms made from the ids.
//! Terms are told apart as RDF tells them apart (an IRI from a literal of
//! the same text, a language or a datatype from another), and a term is
//! found by value only after its hash is checked against the term.
//! Ids are given in the order terms first appear in the sorted documents,
//! and TurbOWL reads the triples in id order, so the same documents are
//! read in the same order (which of two equal proofs TurbOWL finds first
//! follows it).
//!
//! The base also knows its RDF lists (the blank nodes with `rdf:first` and
//! `rdf:rest`), so that a proof can write a list as one `( … )`.

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Source;
use crate::rdf::dataset::{blank_label, literal_ref, node_term};
use crate::rdf::graph::blank_max;
use crate::syntax::{Iri, Node, Span};
use lsp_types::Range;
use oxrdf::{BlankNodeRef, NamedNodeRef, NamedOrBlankNodeRef, TermRef, TripleRef};
use rustc_hash::{FxHashMap, FxHasher};
use spargebra::term::{BlankNode, Term, Triple};
use std::collections::hash_map::Entry;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Where a base triple is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    /// The document's URI; for a cached import, its cache file's.
    pub(crate) uri: String,
    /// The URL a cached import came from, which hover names it by.
    pub(crate) imported_from: Option<String>,
    /// From the predicate to the end of the object, in the encoding of the
    /// run (in the text that was reasoned over).
    pub(crate) range: Range,
    /// The line of the predicate, counted from 1, as `#L<line>` links count.
    pub(crate) line: u32,
}

type Docs = [(Source, Arc<Analysis>)];

/// The base of one run, with the place of each triple.
pub(crate) struct Base {
    /// In `(Source, uri)` order. Their analyses are the versions reasoned
    /// over, so places stay where they were until the next run.
    docs: Vec<(Source, Arc<Analysis>)>,
    enc: Encoding,
    terms: Terms,
    /// Each triple once, by its terms' ids, in id order, with (document,
    /// index into its parse's triples) of its first place.
    triples: Vec<Row>,
    /// The places after the first of the triples written more than once:
    /// (index into `triples`, place).
    repeats: Vec<(u32, (u32, u32))>,
    /// The RDF list cells: each blank node with `rdf:first` or `rdf:rest`,
    /// by id.
    cells: FxHashMap<u32, Cell>,
}

/// A base triple: its subject, predicate and object ids, and a place.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    s: u32,
    p: u32,
    o: u32,
    place: (u32, u32),
}

impl Row {
    fn key(&self) -> (u32, u32, u32) {
        (self.s, self.p, self.o)
    }
}

/// A blank node of an RDF list: its member, the rest of the list, and the
/// cell whose rest it is (ids).
#[derive(Default)]
struct Cell {
    first: Option<u32>,
    rest: Option<u32>,
    previous: Option<u32>,
}

/// The terms of a base, each once, numbered from 0.
#[derive(Default)]
struct Terms {
    stored: Vec<Stored>,
    iris: FxHashMap<Iri, u32>,
    /// The other terms by the hash of the term ([`hash`]); the terms whose
    /// hash an earlier one has are in `collided`.
    by_hash: FxHashMap<u64, u32>,
    collided: FxHashMap<u64, Vec<u32>>,
    /// The blank nodes' labels, one after the other.
    labels: String,
    triple_terms: Vec<Triple>,
}

/// How a term is held.
#[derive(Clone, Copy)]
enum Stored {
    Iri(Iri),
    /// A blank node: its label, `labels[from..to]`.
    Blank(u32, u32),
    /// A literal: the object of triple `.1` of document `.0`.
    Literal(u32, u32),
    /// A triple term: `triple_terms[.0]`.
    Triple(u32),
}

fn hash(t: TermRef<'_>) -> u64 {
    let mut h = FxHasher::default();
    t.hash(&mut h);
    h.finish()
}

fn subject(t: TermRef<'_>) -> NamedOrBlankNodeRef<'_> {
    match t {
        TermRef::NamedNode(n) => n.into(),
        TermRef::BlankNode(b) => b.into(),
        // Only IRIs and blank nodes are put in subject places.
        _ => unreachable!("a subject is an IRI or a blank node"),
    }
}

impl Terms {
    /// Term `id`, borrowed from the terms and from `docs`, the documents
    /// the terms were read from.
    fn get<'a>(&'a self, docs: &'a Docs, id: u32) -> TermRef<'a> {
        match self.stored[id as usize] {
            Stored::Iri(i) => NamedNodeRef::new_unchecked(i.as_str()).into(),
            Stored::Blank(from, to) => {
                BlankNodeRef::new_unchecked(&self.labels[from as usize..to as usize]).into()
            }
            Stored::Literal(d, i) => literal_ref(&docs[d as usize].1.parsed.triples[i as usize].o)
                .expect("a literal is held where it is written")
                .into(),
            Stored::Triple(k) => TermRef::Triple(&self.triple_terms[k as usize]),
        }
    }

    /// The IRI that predicate `id` is.
    fn iri(&self, id: u32) -> Iri {
        match self.stored[id as usize] {
            Stored::Iri(i) => i,
            _ => unreachable!("a predicate is an IRI"),
        }
    }

    fn is_blank(&self, id: u32) -> bool {
        matches!(self.stored[id as usize], Stored::Blank(..))
    }

    /// The id of `t`, if it is a term of the base.
    fn find(&self, docs: &Docs, t: TermRef<'_>) -> Option<u32> {
        if let TermRef::NamedNode(n) = t {
            return self.iris.get(&Iri::get(n.as_str())?).copied();
        }
        self.find_hashed(docs, t, hash(t))
    }

    /// The id of `t`, not an IRI, whose hash is `h`: the terms of that hash
    /// compared with `t`.
    fn find_hashed(&self, docs: &Docs, t: TermRef<'_>, h: u64) -> Option<u32> {
        let is = |id: &u32| self.get(docs, *id) == t;
        self.by_hash
            .get(&h)
            .filter(|id| is(id))
            .or_else(|| self.collided.get(&h)?.iter().find(|id| is(id)))
            .copied()
    }

    /// The id of IRI `i`, added when it is new.
    fn add_iri(&mut self, i: Iri) -> u32 {
        match self.iris.entry(i) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                let id = self.stored.len() as u32;
                self.stored.push(Stored::Iri(i));
                *e.insert(id)
            }
        }
    }

    /// Add a term that [`Terms::find`] does not find, of hash `h`, held as
    /// `stored`.
    fn add(&mut self, h: u64, stored: Stored) -> u32 {
        let id = self.stored.len() as u32;
        self.stored.push(stored);
        match self.by_hash.entry(h) {
            Entry::Vacant(e) => {
                e.insert(id);
            }
            Entry::Occupied(_) => self.collided.entry(h).or_default().push(id),
        }
        id
    }

    /// The id of `t`, added when it is new, held as `stored` makes it.
    fn value(
        &mut self,
        docs: &Docs,
        t: TermRef<'_>,
        stored: impl FnOnce(&mut Terms) -> Stored,
    ) -> u32 {
        match self.find(docs, t) {
            Some(id) => id,
            None => {
                let s = stored(self);
                self.add(hash(t), s)
            }
        }
    }
}

/// The base ids of one document's nodes.
struct Reader<'d> {
    docs: &'d Docs,
    /// The number added to the document's blank node numbers
    /// ([`blank_max`], as the graph numbers them).
    blank_base: u32,
    /// Each blank node's id, by its number across the documents.
    blanks: FxHashMap<u32, u32>,
}

impl Reader<'_> {
    /// The id of node `n`, read at `place` (where a literal is held).
    /// `None` for a literal or a triple term in a subject place.
    fn id(&mut self, terms: &mut Terms, n: &Node, place: (u32, u32), subject: bool) -> Option<u32> {
        Some(match n {
            Node::Iri(i) => terms.add_iri(*i),
            Node::Blank(b) => {
                let number = self.blank_base.saturating_add(b.0);
                match self.blanks.entry(number) {
                    Entry::Occupied(e) => *e.get(),
                    Entry::Vacant(e) => {
                        let label = blank_label(number);
                        let t = BlankNodeRef::new_unchecked(&label).into();
                        *e.insert(terms.value(self.docs, t, |terms| {
                            let from = terms.labels.len() as u32;
                            terms.labels.push_str(&label);
                            Stored::Blank(from, terms.labels.len() as u32)
                        }))
                    }
                }
            }
            _ if subject => return None,
            Node::Literal { .. } => {
                let literal = literal_ref(n).expect("a literal");
                terms.value(self.docs, literal.into(), |_| {
                    Stored::Literal(place.0, place.1)
                })
            }
            Node::Triple(_) => {
                let Term::Triple(t) = node_term(n, self.blank_base) else {
                    unreachable!("a triple node is a triple term")
                };
                let h = hash(TermRef::Triple(&t));
                match terms.find(self.docs, TermRef::Triple(&t)) {
                    Some(id) => id,
                    None => {
                        let k = terms.triple_terms.len() as u32;
                        terms.triple_terms.push(*t);
                        terms.add(h, Stored::Triple(k))
                    }
                }
            }
        })
    }
}

pub(super) const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
pub(super) const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";

impl Base {
    /// The base of `docs`, in any order. Triples with an undeclared prefix
    /// are left out, as the SHACL and query graph leave them out.
    pub(crate) fn new(mut docs: Vec<(Source, Arc<Analysis>)>, enc: Encoding) -> Base {
        docs.sort_by(|a, b| (a.0, &a.1.uri).cmp(&(b.0, &b.1.uri)));
        let mut terms = Terms::default();
        let mut cells: FxHashMap<u32, Cell> = FxHashMap::default();
        let (first, rest) = (Iri::intern(RDF_FIRST), Iri::intern(RDF_REST));
        let total = docs.iter().map(|d| d.1.parsed.triples.len()).sum();
        let mut rows = Vec::with_capacity(total);
        let mut reader = Reader {
            docs: &docs,
            blank_base: 0,
            blanks: FxHashMap::default(),
        };
        for (d, (_, a)) in docs.iter().enumerate() {
            let mut max = 0;
            for (i, t) in a.parsed.triples.iter().enumerate() {
                if t.unresolved {
                    continue;
                }
                blank_max(&t.s, &mut max);
                blank_max(&t.o, &mut max);
                let place = (d as u32, i as u32);
                // RDF has no other subject.
                let Some(s) = reader.id(&mut terms, &t.s, place, true) else {
                    continue;
                };
                let p = terms.add_iri(t.p);
                let o = reader
                    .id(&mut terms, &t.o, place, false)
                    .expect("any term is an object");
                if (t.p == first || t.p == rest) && terms.is_blank(s) {
                    let cell = cells.entry(s).or_default();
                    let slot = if t.p == first {
                        &mut cell.first
                    } else {
                        &mut cell.rest
                    };
                    slot.get_or_insert(o);
                }
                rows.push(Row { s, p, o, place });
            }
            reader.blank_base = reader.blank_base.saturating_add(max);
        }
        drop(reader);
        // In id order, and a triple's places in document order: its first
        // place leads.
        rows.sort_unstable();
        let mut repeats = Vec::new();
        let mut kept = 0;
        for k in 0..rows.len() {
            let r = rows[k];
            if kept > 0 && rows[kept - 1].key() == r.key() {
                repeats.push((kept as u32 - 1, r.place));
            } else {
                rows[kept] = r;
                kept += 1;
            }
        }
        rows.truncate(kept);
        rows.shrink_to_fit();
        let mut links: Vec<(u32, u32)> = cells
            .iter()
            .filter_map(|(&b, c)| {
                c.rest
                    .filter(|&next| terms.is_blank(next))
                    .map(|next| (next, b))
            })
            .collect();
        links.sort_unstable();
        for (next, b) in links {
            if let Some(c) = cells.get_mut(&next) {
                c.previous.get_or_insert(b);
            }
        }
        Base {
            docs,
            enc,
            terms,
            triples: rows,
            repeats,
            cells,
        }
    }

    fn term(&self, id: u32) -> TermRef<'_> {
        self.terms.get(&self.docs, id)
    }

    /// The id of `t`, when it is a term of the base.
    pub(super) fn id<'t>(&self, t: impl Into<TermRef<'t>>) -> Option<u32> {
        self.terms.find(&self.docs, t.into())
    }

    fn blank(&self, id: u32) -> BlankNode {
        match self.term(id) {
            TermRef::BlankNode(b) => b.into_owned(),
            _ => unreachable!("a list cell is a blank node"),
        }
    }

    /// `b` is a cell of an RDF list.
    pub(super) fn is_cell(&self, b: &BlankNode) -> bool {
        self.id(b).is_some_and(|id| self.cells.contains_key(&id))
    }

    /// The list from cell `b` on: each cell with its member, up to
    /// `rdf:nil` or a cell that is not well formed. Empty when `b` is not
    /// a cell.
    pub(super) fn list(&self, b: &BlankNode) -> Vec<(BlankNode, Term)> {
        let mut out = Vec::new();
        let Some(mut at) = self.id(b) else {
            return out;
        };
        let mut seen = Vec::new();
        while let Some(cell) = self.cells.get(&at) {
            let Some(first) = cell.first else { break };
            // A list that runs into itself ends where it does.
            if seen.contains(&at) {
                break;
            }
            seen.push(at);
            out.push((self.blank(at), self.term(first).into_owned()));
            match cell.rest {
                Some(next) if self.terms.is_blank(next) => at = next,
                _ => break,
            }
        }
        out
    }

    /// The first cell of the list that cell `b` is in.
    pub(super) fn list_head(&self, b: &BlankNode) -> BlankNode {
        let Some(start) = self.id(b) else {
            return b.clone();
        };
        let mut at = start;
        let mut steps = 0;
        while let Some(previous) = self.cells.get(&at).and_then(|c| c.previous) {
            steps += 1;
            if previous == start || steps > self.cells.len() {
                break;
            }
            at = previous;
        }
        self.blank(at)
    }

    /// Every triple, each once.
    pub(crate) fn triples(&self) -> impl Iterator<Item = TripleRef<'_>> {
        self.triples.iter().map(|r| self.triple(r))
    }

    fn triple(&self, r: &Row) -> TripleRef<'_> {
        TripleRef::new(
            subject(self.term(r.s)),
            NamedNodeRef::new_unchecked(self.terms.iri(r.p).as_str()),
            self.term(r.o),
        )
    }

    /// The workspace documents' triples, not the imports', as the ids of
    /// their subject and object, each with its (document, index), at every
    /// place it is written.
    pub(super) fn in_workspace(&self) -> impl Iterator<Item = (u32, u32, (u32, u32))> {
        let repeats = self.repeats.iter().map(|&(k, place)| {
            let r = &self.triples[k as usize];
            (r.s, r.o, place)
        });
        self.triples
            .iter()
            .map(|r| (r.s, r.o, r.place))
            .chain(repeats)
            .filter(|(_, _, (d, _))| self.docs[*d as usize].0 != Source::External)
    }

    /// Where `t` is written, when it is a base triple.
    pub(crate) fn origin<'t>(&self, t: impl Into<TripleRef<'t>>) -> Option<Origin> {
        let t = t.into();
        let key = (
            self.id(t.subject)?,
            self.id(t.predicate)?,
            self.id(t.object)?,
        );
        let k = self.triples.partition_point(|r| r.key() < key);
        let r = self.triples.get(k).filter(|r| r.key() == key)?;
        Some(self.place(r.place))
    }

    /// Triple `i` of document `d`'s parse, and that document.
    pub(super) fn written(&self, (d, i): (u32, u32)) -> (&Arc<Analysis>, &crate::syntax::Triple) {
        let a = &self.docs[d as usize].1;
        (a, &a.parsed.triples[i as usize])
    }

    /// Where triple `i` of document `d` is written.
    pub(super) fn place(&self, (d, i): (u32, u32)) -> Origin {
        let (a, t) = self.written((d, i));
        let span = Span::new(t.p_span.start, t.o_span.end.max(t.p_span.end));
        Origin {
            uri: a.uri.clone(),
            imported_from: a.origin.clone(),
            range: a.range(self.enc, span),
            line: a.position(Encoding::Utf8, t.p_span.start).line + 1,
        }
    }

    /// The encoding of the run's ranges.
    pub(super) fn encoding(&self) -> Encoding {
        self.enc
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::syntax::PrefixMap;
    use spargebra::term::NamedNode;

    pub(crate) fn doc(source: Source, uri: &str, text: &str) -> (Source, Arc<Analysis>) {
        let a = Analysis::new(uri.into(), text, Arc::new(PrefixMap::default()), false);
        (source, Arc::new(a))
    }

    fn listed(b: &Base) -> Vec<String> {
        let mut v: Vec<String> = b
            .triples()
            .map(|t| {
                let o = b.origin(t).unwrap();
                format!("{t} @ {}:{}", o.uri, o.line)
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn the_same_documents_in_any_order_give_the_same_base() {
        let a = doc(
            Source::Disk,
            "file:///w/a.ttl",
            "@prefix ex: <http://example.org/> .\nex:a ex:p [ ex:q 1 ] .\n",
        );
        let b = doc(
            Source::Open,
            "file:///w/b.ttl",
            "@prefix ex: <http://example.org/> .\n_:x ex:p ex:b .\nex:b ex:r _:x .\n",
        );
        let c = doc(
            Source::External,
            "file:///cache/c.ttl",
            "@prefix ex: <http://example.org/> .\n[] ex:s ex:c .\n",
        );
        let one = Base::new(vec![a.clone(), b.clone(), c.clone()], Encoding::Utf16);
        let two = Base::new(vec![c, a, b], Encoding::Utf16);
        assert_eq!(one.triples().count(), 5);
        assert_eq!(listed(&one), listed(&two));
        // Blank nodes of different documents stay apart.
        let blanks: std::collections::BTreeSet<String> = one
            .triples()
            .filter_map(|t| match t.subject {
                NamedOrBlankNodeRef::BlankNode(b) => Some(b.as_str().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(blanks.len(), 3, "{blanks:?}");
    }

    #[test]
    fn a_base_triple_maps_back_to_its_document_and_line() {
        let text = "@prefix ex: <http://example.org/> .\n\nex:a a ex:C ;\n  ex:label \"A\" .\n";
        let d = doc(Source::Open, "file:///w/a.ttl", text);
        let base = Base::new(vec![d], Encoding::Utf16);
        let ex = |l: &str| NamedNode::new_unchecked(format!("http://example.org/{l}"));
        let t = Triple::new(
            ex("a"),
            ex("label"),
            spargebra::term::Literal::new_simple_literal("A"),
        );
        let o = base.origin(&t).unwrap();
        assert_eq!((o.uri.as_str(), o.line), ("file:///w/a.ttl", 4));
        assert_eq!((o.range.start.line, o.range.start.character), (3, 2));
        assert_eq!((o.range.end.line, o.range.end.character), (3, 14));
        assert_eq!(o.imported_from, None);
        let typed = Triple::new(
            ex("a"),
            NamedNode::new_unchecked(crate::syntax::RDF_TYPE),
            ex("C"),
        );
        assert_eq!(base.origin(&typed).unwrap().line, 3);
        assert!(
            base.origin(&Triple::new(ex("a"), ex("label"), ex("C")))
                .is_none()
        );
    }

    #[test]
    fn triples_with_an_undeclared_prefix_are_left_out() {
        let d = doc(
            Source::Open,
            "file:///w/a.ttl",
            "@prefix ex: <http://example.org/> .\nex:a ex:p nope:b .\nex:a ex:p ex:b .\n",
        );
        assert_eq!(Base::new(vec![d], Encoding::Utf16).triples().count(), 1);
    }

    /// The document and line where `t` is written.
    fn origin_line<'t>(b: &Base, t: impl Into<TripleRef<'t>>) -> Option<(String, u32)> {
        b.origin(t).map(|o| (o.uri, o.line))
    }

    const EX: &str = "http://example.org/";

    #[test]
    fn an_iri_and_literals_of_the_same_text_are_different_terms() {
        let text = "@prefix ex: <http://example.org/> .\n\
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
            ex:a ex:p <http://example.org/x> .\n\
            ex:a ex:p \"http://example.org/x\" .\n\
            ex:a ex:p \"x\" .\n\
            ex:a ex:p \"x\"^^xsd:string .\n\
            ex:a ex:p \"x\"@en .\n\
            ex:a ex:p \"x\"@fr .\n\
            ex:a ex:p \"x\"@en--rtl .\n\
            ex:a ex:p \"x\"^^xsd:token .\n";
        let base = Base::new(
            vec![doc(Source::Open, "file:///w/a.ttl", text)],
            Encoding::Utf16,
        );
        // `"x"^^xsd:string` is `"x"`: one triple, first written on line 5.
        assert_eq!(base.triples().count(), 7);
        let a = NamedNodeRef::new_unchecked("http://example.org/a");
        let p = NamedNodeRef::new_unchecked("http://example.org/p");
        let line = |o: TermRef<'_>| origin_line(&base, TripleRef::new(a, p, o)).map(|(_, l)| l);
        let x = NamedNodeRef::new_unchecked("http://example.org/x");
        let lit = oxrdf::LiteralRef::new_simple_literal;
        let tagged = oxrdf::LiteralRef::new_language_tagged_literal_unchecked;
        let typed = |t: &'static str| {
            oxrdf::LiteralRef::new_typed_literal("x", NamedNodeRef::new_unchecked(t))
        };
        assert_eq!(line(x.into()), Some(3));
        assert_eq!(line(lit("http://example.org/x").into()), Some(4));
        assert_eq!(line(lit("x").into()), Some(5));
        assert_eq!(
            line(typed("http://www.w3.org/2001/XMLSchema#string").into()),
            Some(5)
        );
        assert_eq!(line(tagged("x", "en").into()), Some(7));
        assert_eq!(line(tagged("x", "fr").into()), Some(8));
        let rtl = oxrdf::LiteralRef::new_directional_language_tagged_literal_unchecked(
            "x",
            "en",
            oxrdf::BaseDirection::Rtl,
        );
        assert_eq!(line(rtl.into()), Some(9));
        let ltr = oxrdf::LiteralRef::new_directional_language_tagged_literal_unchecked(
            "x",
            "en",
            oxrdf::BaseDirection::Ltr,
        );
        assert_eq!(line(ltr.into()), None);
        assert_eq!(
            line(typed("http://www.w3.org/2001/XMLSchema#token").into()),
            Some(10)
        );
        assert_eq!(
            line(typed("http://www.w3.org/2001/XMLSchema#integer").into()),
            None
        );
        assert_eq!(line(tagged("x", "de").into()), None);
        // The IRI is not a literal, nor the literal an IRI.
        assert_ne!(base.id(x), base.id(lit("http://example.org/x")));
        assert_eq!(base.id(NamedNodeRef::new_unchecked("x")), None);
    }

    #[test]
    fn blank_nodes_of_the_same_label_in_two_documents_are_two_terms() {
        let text = "@prefix ex: <http://example.org/> .\n_:b ex:p ex:o .\n";
        let a = doc(Source::Open, "file:///w/a.ttl", text);
        let b = doc(Source::Open, "file:///w/b.ttl", text);
        let base = Base::new(vec![b, a], Encoding::Utf16);
        let mut found: Vec<(String, (String, u32))> = base
            .triples()
            .map(|t| {
                let NamedOrBlankNodeRef::BlankNode(b) = t.subject else {
                    panic!("{t}")
                };
                (b.as_str().to_string(), origin_line(&base, t).unwrap())
            })
            .collect();
        found.sort();
        let places: Vec<&(String, u32)> = found.iter().map(|(_, at)| at).collect();
        assert_eq!(
            places,
            [
                &("file:///w/a.ttl".into(), 2),
                &("file:///w/b.ttl".into(), 2)
            ]
        );
        assert_ne!(found[0].0, found[1].0);
        // A label written another way is another blank node.
        let p = NamedNodeRef::new_unchecked("http://example.org/p");
        let o = NamedNodeRef::new_unchecked("http://example.org/o");
        let first = &found[0].0;
        let padded = format!("g0{}", &first[1..]);
        for label in [padded.as_str(), "g999", "b", "1"] {
            let t = TripleRef::new(BlankNodeRef::new_unchecked(label), p, o);
            assert_eq!(base.origin(t), None, "{label}");
        }
    }

    #[test]
    fn triple_terms_are_terms_by_their_content_and_their_documents_blank_nodes() {
        let text = "@prefix ex: <http://example.org/> .\n\
            ex:a ex:says <<( ex:s ex:p \"o\" )>> .\n\
            ex:b ex:says <<( _:x ex:p ex:o )>> .\n\
            ex:c ex:says <<( ex:s ex:p \"o\" )>> .\n";
        let a = doc(Source::Open, "file:///w/a.ttl", text);
        let b = doc(Source::Open, "file:///w/b.ttl", text);
        let base = Base::new(vec![a, b], Encoding::Utf16);
        // ex:a and ex:c say the same in both documents; ex:b, about a
        // blank node, says something else in each.
        assert_eq!(base.triples().count(), 4);
        let objects: std::collections::BTreeSet<String> =
            base.triples().map(|t| t.object.to_string()).collect();
        assert_eq!(objects.len(), 3, "{objects:?}");
        let ex = |l: &str| NamedNode::new_unchecked(format!("{EX}{l}"));
        let said = Triple::new(
            ex("s"),
            ex("p"),
            spargebra::term::Literal::new_simple_literal("o"),
        );
        let says = |s: &str| Triple::new(ex(s), ex("says"), Term::Triple(Box::new(said.clone())));
        assert_eq!(
            origin_line(&base, &says("a")),
            Some(("file:///w/a.ttl".into(), 2))
        );
        assert_eq!(
            origin_line(&base, &says("c")),
            Some(("file:///w/a.ttl".into(), 4))
        );
        assert_eq!(origin_line(&base, &says("d")), None);
        let other = Triple::new(
            ex("s"),
            ex("p"),
            spargebra::term::Literal::new_simple_literal("q"),
        );
        let t = Triple::new(ex("a"), ex("says"), Term::Triple(Box::new(other)));
        assert_eq!(origin_line(&base, &t), None);
    }

    #[test]
    fn a_term_not_in_the_base_has_no_origin_and_no_id() {
        let text = "@prefix ex: <http://example.org/> .\nex:a ex:p ex:b .\n";
        let base = Base::new(
            vec![doc(Source::Open, "file:///w/a.ttl", text)],
            Encoding::Utf16,
        );
        let ex = |l: &str| NamedNodeRef::new_unchecked(format!("{EX}{l}").leak());
        // Never interned anywhere, interned but not in the base, and in
        // another place of the triple.
        let never = NamedNodeRef::new_unchecked("http://example.org/never-written-anywhere-7f3a");
        Iri::intern("http://example.org/interned-elsewhere");
        let elsewhere = ex("interned-elsewhere");
        for t in [
            TripleRef::new(never, ex("p"), ex("b")),
            TripleRef::new(elsewhere, ex("p"), ex("b")),
            TripleRef::new(ex("a"), never, ex("b")),
            TripleRef::new(ex("a"), ex("p"), elsewhere),
            TripleRef::new(ex("b"), ex("p"), ex("a")),
            TripleRef::new(ex("a"), ex("b"), ex("p")),
            TripleRef::new(BlankNodeRef::new_unchecked("g0"), ex("p"), ex("b")),
        ] {
            assert_eq!(base.origin(t), None, "{t}");
        }
        assert_eq!(base.id(never), None);
        assert_eq!(base.id(oxrdf::LiteralRef::new_simple_literal("a")), None);
        assert!(!base.is_cell(&BlankNode::new_unchecked("g0")));
        assert!(base.list(&BlankNode::new_unchecked("g0")).is_empty());
        assert_eq!(
            base.origin(TripleRef::new(ex("a"), ex("p"), ex("b")))
                .unwrap()
                .line,
            2
        );
    }

    #[test]
    fn a_term_is_found_by_its_hash_only_when_it_is_that_term() {
        let docs: Vec<(Source, Arc<Analysis>)> = Vec::new();
        let mut terms = Terms::default();
        let blank = |terms: &mut Terms, label: &str, h: u64| {
            let from = terms.labels.len() as u32;
            terms.labels.push_str(label);
            terms.add(h, Stored::Blank(from, terms.labels.len() as u32))
        };
        // Three terms that share a hash, as a collision would have them.
        let a = blank(&mut terms, "ga", 7);
        let b = blank(&mut terms, "gb", 7);
        let c = blank(&mut terms, "gc", 7);
        let find = |label: &str, h: u64| {
            terms.find_hashed(&docs, BlankNodeRef::new_unchecked(label).into(), h)
        };
        assert_eq!(find("ga", 7), Some(a));
        assert_eq!(find("gb", 7), Some(b));
        assert_eq!(find("gc", 7), Some(c));
        assert_eq!(find("gd", 7), None);
        // Another hash finds none of them.
        assert_eq!(find("ga", 8), None);
        // By the term's own hash, which is not 7.
        assert_eq!(
            terms.find(&docs, BlankNodeRef::new_unchecked("ga").into()),
            None
        );
    }

    #[test]
    fn a_triple_written_twice_leads_with_its_first_place_and_is_in_the_workspace_at_both() {
        let text = "@prefix ex: <http://example.org/> .\nex:a ex:p ex:b .\nex:a ex:p ex:b .\n";
        let docs = vec![
            doc(Source::External, "file:///cache/c.ttl", text),
            doc(Source::Disk, "file:///w/b.ttl", text),
            doc(Source::Open, "file:///w/a.ttl", text),
        ];
        let base = Base::new(docs, Encoding::Utf16);
        assert_eq!(base.triples().count(), 1);
        let t = base.triples().next().unwrap();
        // `(Source, uri)` order: the open document first.
        assert_eq!(origin_line(&base, t), Some(("file:///w/a.ttl".into(), 2)));
        let mut places: Vec<(String, u32)> = base
            .in_workspace()
            .map(|(s, o, place)| {
                assert_eq!((Some(s), Some(o)), (base.id(t.subject), base.id(t.object)));
                let o = base.place(place);
                (o.uri, o.line)
            })
            .collect();
        places.sort();
        // The import's places are not the workspace's.
        assert_eq!(
            places,
            [
                ("file:///w/a.ttl".into(), 2),
                ("file:///w/a.ttl".into(), 3),
                ("file:///w/b.ttl".into(), 2),
                ("file:///w/b.ttl".into(), 3),
            ]
        );
    }

    #[test]
    fn a_list_is_read_from_its_cells() {
        let text = "@prefix ex: <http://example.org/> .\nex:a ex:p ( ex:x \"y\" ( ex:z ) ) .\n";
        let base = Base::new(
            vec![doc(Source::Open, "file:///w/a.ttl", text)],
            Encoding::Utf16,
        );
        let head = base
            .triples()
            .find(|t| t.predicate.as_str() == format!("{EX}p"))
            .map(|t| match t.object {
                TermRef::BlankNode(b) => b.into_owned(),
                o => panic!("{o}"),
            })
            .unwrap();
        let list = base.list(&head);
        let members: Vec<String> = list.iter().map(|(_, m)| m.to_string()).collect();
        assert_eq!(members[..2], ["<http://example.org/x>", "\"y\""]);
        assert!(matches!(&list[2].1, Term::BlankNode(b) if base.is_cell(b)));
        // From the last cell back to the first.
        let (last, _) = &list[2];
        assert!(base.is_cell(last));
        assert_eq!(base.list_head(last), head);
        assert_eq!(base.list_head(&head), head);
    }

    #[test]
    fn the_base_has_the_triples_and_blank_node_names_of_the_graph_and_its_dataset() {
        let h = "@prefix ex: <http://example.org/> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";
        let texts = [
            // An unresolved triple's blank node is not counted.
            "_:a ex:p [ ex:q ( 1 \"two\"@en ) ] .\nnope:x ex:p _:b9 .\n",
            "ex:a ex:p ex:b .\n",
            "ex:s ex:says <<( _:x ex:p \"o\"@ar--rtl )>> , <<( ex:a ex:p _:y )>> .\n_:x ex:p \"1\"^^xsd:integer .\n",
            "[] ex:p _:z .\n_:z ex:q <<( _:z ex:r [] )>> .\n",
            "ex:c ex:p \"x\" , \"x\"^^xsd:string .\n",
        ];
        let docs: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(k, t)| {
                doc(
                    Source::Open,
                    &format!("file:///w/{k}.ttl"),
                    &format!("{h}{t}"),
                )
            })
            .collect();
        let base = Base::new(docs.clone(), Encoding::Utf16);
        let parses: Vec<&[crate::syntax::Triple]> =
            docs.iter().map(|d| d.1.parsed.triples.as_slice()).collect();
        let g = crate::rdf::Graph::new(&parses);
        let index = crate::rdf::Index::new(&g, None);
        let graph: std::collections::BTreeSet<String> = g
            .added()
            .map(|(_, t)| {
                let s = index.term(t.s);
                format!("{s} <{}> {}", t.p.as_str(), index.term(t.o))
            })
            .collect();
        let ours: std::collections::BTreeSet<String> =
            base.triples().map(|t| t.to_string()).collect();
        assert_eq!(ours, graph);
        assert!(ours.iter().any(|t| t.contains("<<( _:g")), "{ours:#?}");
    }
}
