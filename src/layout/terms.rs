//! The triple terms both formatters lay out, built by their own CST walks
//! over token indices ([`Ix`]): a term, an object with its annotations, a
//! predicate with its objects. What each one reports about itself (its first
//! and last token, whether it is a group, whether it is flat) and its width
//! on one line.

use super::doc::{Doc, Ix, LayoutToken};

pub(crate) enum Term {
    /// Tokens printed without spaces: an IRI, a variable, a literal with its
    /// language tag or datatype, a signed number, `[]`, `()`, a property path,
    /// `a`, `~`.
    Atom(Ix, Ix),
    /// `[ pol ]` or `{| pol |}`.
    List { open: Ix, pol: Vec<Po>, close: Ix },
    /// `( items )`.
    Coll {
        open: Ix,
        items: Vec<Obj>,
        close: Ix,
    },
    /// `<< s p o ~ r >>` or `<<( s p o )>>`: always on one line.
    Quoted {
        open: Ix,
        parts: Vec<Term>,
        close: Ix,
    },
}

pub(crate) struct Obj {
    pub(crate) term: Term,
    /// `~`, a reifier, `{| ... |}` blocks.
    pub(crate) annots: Vec<Term>,
    pub(crate) comma: Option<Ix>,
}

/// A predicate (or path) with its objects and the `;` after them.
pub(crate) struct Po {
    pub(crate) verb: Term,
    pub(crate) objs: Vec<Obj>,
    pub(crate) semis: Vec<Ix>,
}

impl Term {
    pub(crate) fn first(&self) -> Ix {
        match self {
            Term::Atom(a, _) => *a,
            Term::List { open, .. } | Term::Coll { open, .. } | Term::Quoted { open, .. } => *open,
        }
    }

    pub(crate) fn last(&self) -> Ix {
        match self {
            Term::Atom(_, b) => *b,
            Term::List { close, .. } | Term::Coll { close, .. } | Term::Quoted { close, .. } => {
                *close
            }
        }
    }

    /// `[ ]`, `{| |}` or `( )` with something in it.
    pub(crate) fn is_group(&self) -> bool {
        matches!(self, Term::List { .. } | Term::Coll { .. })
    }

    /// A group with nothing nested in it, or no group.
    pub(crate) fn flat(&self) -> bool {
        match self {
            Term::List { pol, .. } => pol.iter().all(|po| po.objs.iter().all(Obj::simple)),
            Term::Coll { items, .. } => items.iter().all(Obj::simple),
            _ => true,
        }
    }

    /// Width on one line, a space between the parts of a group.
    pub(crate) fn width<T: LayoutToken>(&self, d: &Doc<T>) -> usize {
        match self {
            Term::Atom(a, b) => (*a..=*b).fold(0, |acc, i| acc.saturating_add(d.width(i))),
            Term::List { open, pol, close } => d
                .width(*open)
                .saturating_add(pairs_width(pol, d))
                .saturating_add(d.width(*close) + 2),
            Term::Coll { open, items, close } => {
                let inner = items
                    .iter()
                    .fold(0usize, |acc, o| acc.saturating_add(o.width(d) + 1));
                d.width(*open)
                    .saturating_add(inner)
                    .saturating_add(d.width(*close) + 1)
            }
            Term::Quoted { open, parts, close } => {
                let inner = parts
                    .iter()
                    .fold(0usize, |acc, p| acc.saturating_add(p.width(d) + 1));
                d.width(*open)
                    .saturating_add(inner)
                    .saturating_add(d.width(*close) + 1)
            }
        }
    }
}

impl Obj {
    /// Prints the same on one line in any context: no group in it.
    pub(crate) fn simple(&self) -> bool {
        !self.term.is_group() && !self.annots.iter().any(Term::is_group)
    }

    pub(crate) fn flat(&self) -> bool {
        self.term.flat() && self.annots.iter().all(Term::flat)
    }

    pub(crate) fn last(&self) -> Ix {
        self.annots.last().unwrap_or(&self.term).last()
    }

    /// The term and its annotations, a space between each.
    pub(crate) fn width<T: LayoutToken>(&self, d: &Doc<T>) -> usize {
        self.annots.iter().fold(self.term.width(d), |acc, a| {
            acc.saturating_add(a.width(d) + 1)
        })
    }
}

/// Objects separated by ` , `.
pub(crate) fn objects_width<T: LayoutToken>(objs: &[Obj], d: &Doc<T>) -> usize {
    let sum = objs
        .iter()
        .fold(0usize, |acc, o| acc.saturating_add(o.width(d)));
    sum.saturating_add(3 * objs.len().saturating_sub(1))
}

/// Pairs separated by ` ; `.
pub(crate) fn pairs_width<T: LayoutToken>(pol: &[Po], d: &Doc<T>) -> usize {
    let sum = pol.iter().fold(0usize, |acc, po| {
        acc.saturating_add(po.verb.width(d) + 1)
            .saturating_add(objects_width(&po.objs, d))
    });
    sum.saturating_add(3 * pol.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(i: Ix) -> Term {
        Term::Atom(i, i)
    }

    fn obj(term: Term) -> Obj {
        Obj {
            term,
            annots: Vec::new(),
            comma: None,
        }
    }

    fn po(verb: Ix, objs: Vec<Obj>) -> Po {
        Po {
            verb: atom(verb),
            objs,
            semis: Vec::new(),
        }
    }

    /// The Turtle-family token document and the SPARQL one of `src`.
    fn docs(
        src: &str,
    ) -> (
        Doc<'_, crate::syntax::lexer::Token>,
        Doc<'_, crate::sparql::Token>,
    ) {
        (
            Doc::new(src, &crate::syntax::lexer::tokenize(src)),
            Doc::new(src, &crate::sparql::lexer::tokenize(src)),
        )
    }

    #[test]
    fn a_nested_list() {
        let src = "[ :p [ :q :r ] ]";
        let inner = Term::List {
            open: 2,
            pol: vec![po(3, vec![obj(atom(4))])],
            close: 5,
        };
        let inner_w = "[ :q :r ]".len();
        assert!(inner.flat() && inner.is_group());
        let outer = Term::List {
            open: 0,
            pol: vec![po(1, vec![obj(inner)])],
            close: 6,
        };
        assert!(!outer.flat());
        assert_eq!((outer.first(), outer.last()), (0, 6));
        let (rdf, sparql) = docs(src);
        let Term::List { pol, .. } = &outer else {
            unreachable!()
        };
        assert_eq!(pol[0].objs[0].term.width(&rdf), inner_w);
        assert_eq!(outer.width(&rdf), src.len());
        assert_eq!(outer.width(&sparql), src.len());
    }

    #[test]
    fn a_collection() {
        let src = "( :a ( :b ) \"c\" )";
        let inner = Term::Coll {
            open: 2,
            items: vec![obj(atom(3))],
            close: 4,
        };
        assert!(inner.flat());
        assert_eq!(inner.width(&docs(src).0), "( :b )".len());
        let outer = Term::Coll {
            open: 0,
            items: vec![obj(atom(1)), obj(inner), obj(atom(5))],
            close: 6,
        };
        assert!(!outer.flat());
        let (rdf, sparql) = docs(src);
        assert_eq!(outer.width(&rdf), src.len());
        assert_eq!(outer.width(&sparql), src.len());
    }

    #[test]
    fn an_annotation() {
        let src = ":p :o {| :q :r |}";
        let annotated = Obj {
            term: atom(1),
            annots: vec![Term::List {
                open: 2,
                pol: vec![po(3, vec![obj(atom(4))])],
                close: 5,
            }],
            comma: None,
        };
        // Flat, but a group: what holds it is not flat.
        assert!(annotated.flat() && !annotated.simple());
        assert_eq!(annotated.last(), 5);
        let pair = po(0, vec![annotated]);
        let (rdf, sparql) = docs(src);
        assert_eq!(pairs_width(std::slice::from_ref(&pair), &rdf), src.len());
        assert_eq!(pairs_width(std::slice::from_ref(&pair), &sparql), src.len());
        let list = Term::List {
            open: 0,
            pol: vec![pair],
            close: 0,
        };
        assert!(!list.flat());
    }

    #[test]
    fn objects_and_pairs() {
        let src = ":p :a , :b ; :q :c";
        let (rdf, _) = docs(src);
        let pol = [
            po(0, vec![obj(atom(1)), obj(atom(3))]),
            po(5, vec![obj(atom(6))]),
        ];
        assert_eq!(objects_width(&pol[0].objs, &rdf), ":a , :b".len());
        assert_eq!(pairs_width(&pol, &rdf), src.len());
        assert!(pol[0].objs.iter().all(|o| o.simple() && o.flat()));
    }

    #[test]
    fn a_multi_line_string() {
        let src = ":p \"\"\"ab\ncdef\"\"\"";
        let (rdf, sparql) = docs(src);
        let s = atom(1);
        // SPARQL counts its first line, `"""ab`.
        assert_eq!(s.width(&sparql), 5);
        assert_eq!(pairs_width(&[po(0, vec![obj(atom(1))])], &sparql), 8);
        // The Turtle family keeps it off any shared line.
        assert_eq!(s.width(&rdf), crate::layout::BIG);
        assert!(s.flat());
    }
}
