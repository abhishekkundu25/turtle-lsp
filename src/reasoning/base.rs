//! The base: the workspace's triples as TurbOWL reads them, and where each
//! one is written.
//!
//! The documents (imports included) are put in `(Source, uri)` order, as
//! SHACL validation sorts them, and made into one [`Graph`]: blank nodes
//! are kept apart per document and named `g<n>` as the SPARQL dataset names
//! them ([`Index::term`]), so the same documents give the same base, blank
//! node names included, whatever order they come in. TriG and N-Quads
//! graphs are merged into that one graph. A triple written twice keeps its
//! first place.

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Source;
use crate::rdf::{Graph, Index};
use crate::syntax::Span;
use lsp_types::Range;
use rustc_hash::FxHashMap;
use spargebra::term::{NamedNode, NamedOrBlankNode, Term, Triple};
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

/// The base of one run, with the place of each triple.
pub(crate) struct Base {
    /// In `(Source, uri)` order. Their analyses are the versions reasoned
    /// over, so places stay where they were until the next run.
    docs: Vec<(Source, Arc<Analysis>)>,
    enc: Encoding,
    /// Each triple, with (document, index into its parse's triples).
    places: FxHashMap<Triple, (u32, u32)>,
}

impl Base {
    /// The base of `docs`, in any order. Triples with an undeclared prefix
    /// are left out, as the SHACL and query graph leave them out.
    pub(crate) fn new(mut docs: Vec<(Source, Arc<Analysis>)>, enc: Encoding) -> Base {
        docs.sort_by(|a, b| (a.0, &a.1.uri).cmp(&(b.0, &b.1.uri)));
        let parses: Vec<&[crate::syntax::Triple]> =
            docs.iter().map(|d| d.1.parsed.triples.as_slice()).collect();
        let g = Graph::new(&parses);
        let terms = Index::new(&g, None);
        let mut places = FxHashMap::default();
        places.reserve(g.len());
        for (at, t) in g.added() {
            let subject: NamedOrBlankNode = match terms.term(t.s) {
                Term::NamedNode(n) => n.into(),
                Term::BlankNode(b) => b.into(),
                // RDF has no other subject.
                _ => continue,
            };
            let triple = Triple::new(
                subject,
                NamedNode::new_unchecked(t.p.as_str()),
                terms.term(t.o),
            );
            places.entry(triple).or_insert_with(|| g.origin(at));
        }
        Base { docs, enc, places }
    }

    /// Every triple, each once.
    pub(crate) fn triples(&self) -> impl Iterator<Item = &Triple> {
        self.places.keys()
    }

    /// The workspace documents' triples, not the imports', each with its
    /// (document, index).
    pub(super) fn in_workspace(&self) -> impl Iterator<Item = (&Triple, (u32, u32))> {
        self.places
            .iter()
            .filter(|(_, (d, _))| self.docs[*d as usize].0 != Source::External)
            .map(|(t, &place)| (t, place))
    }

    /// Where `t` is written, when it is a base triple.
    pub(crate) fn origin(&self, t: &Triple) -> Option<Origin> {
        self.places.get(t).map(|&place| self.place(place))
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
            .filter_map(|t| match &t.subject {
                NamedOrBlankNode::BlankNode(b) => Some(b.as_str().to_string()),
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
}
