//! Our parse as an oxrdf dataset, canonicalized, to compare with another
//! parser's: every quad counts, blank nodes and triple terms included, and
//! blank nodes match under any consistent renaming (oxrdf's canonical
//! labels). Shared by tests/oxttl_oracle.rs, tests/jsonld_oracle.rs,
//! tests/w3c_conformance.rs and examples/corpus.rs, each with
//! `#[path = ".../support/oracle.rs"] mod oracle;`.
#![allow(dead_code)]

use oxrdf::dataset::CanonicalizationAlgorithm;
use oxrdf::{
    BaseDirection, BlankNode, Dataset, GraphName as OxGraph, Literal, NamedNode, NamedOrBlankNode,
    Quad, Term,
};
use std::fmt::Write as _;
use turtle_lsp::syntax::{Direction, GraphName, Node, Triple};

/// What a triple term with a subject no RDF subject can be (a literal or a
/// triple term) is written as, so it shows as a difference.
pub const NOT_A_SUBJECT: &str = "urn:turtle-lsp:oracle:not-a-subject";

fn blank(id: u32) -> BlankNode {
    BlankNode::new_unchecked(format!("b{id}"))
}

/// One of our nodes as an oxrdf term; our blank node `n` is `_:bn`.
pub fn term(n: &Node) -> Term {
    match n {
        Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
        Node::Blank(b) => blank(b.0).into(),
        Node::Literal {
            value,
            datatype,
            lang,
            dir,
        } => match (lang, dir) {
            (Some(l), Some(d)) => Literal::new_directional_language_tagged_literal_unchecked(
                &**value,
                l.as_str(),
                match d {
                    Direction::Rtl => BaseDirection::Rtl,
                    Direction::Ltr => BaseDirection::Ltr,
                },
            )
            .into(),
            (Some(l), None) => {
                Literal::new_language_tagged_literal_unchecked(&**value, l.as_str()).into()
            }
            _ => Literal::new_typed_literal(&**value, NamedNode::new_unchecked(datatype.as_str()))
                .into(),
        },
        Node::Triple(t) => oxrdf::Triple::new(
            subject(&t.0),
            NamedNode::new_unchecked(t.1.as_str()),
            term(&t.2),
        )
        .into(),
    }
}

/// A subject: an IRI or a blank node, else [`NOT_A_SUBJECT`].
pub fn subject(n: &Node) -> NamedOrBlankNode {
    match term(n) {
        Term::NamedNode(n) => n.into(),
        Term::BlankNode(b) => b.into(),
        _ => NamedNode::new_unchecked(NOT_A_SUBJECT).into(),
    }
}

pub fn quad(t: &Triple) -> Quad {
    let graph = match t.graph {
        None => OxGraph::DefaultGraph,
        Some(GraphName::Iri(i)) => NamedNode::new_unchecked(i.as_str()).into(),
        Some(GraphName::Blank(b)) => blank(b.0).into(),
    };
    Quad::new(
        subject(&t.s),
        NamedNode::new_unchecked(t.p.as_str()),
        term(&t.o),
        graph,
    )
}

/// Language tags compare case-insensitively.
fn lower_lang(t: Term) -> Term {
    match t {
        Term::Literal(l) => match (l.language(), l.direction()) {
            (Some(lang), Some(dir)) => Literal::new_directional_language_tagged_literal_unchecked(
                l.value(),
                lang.to_ascii_lowercase(),
                dir,
            )
            .into(),
            (Some(lang), None) => {
                Literal::new_language_tagged_literal_unchecked(l.value(), lang.to_ascii_lowercase())
                    .into()
            }
            _ => Term::Literal(l),
        },
        Term::Triple(t) => {
            let t = *t;
            Term::Triple(Box::new(oxrdf::Triple::new(
                t.subject,
                t.predicate,
                lower_lang(t.object),
            )))
        }
        t => t,
    }
}

/// `quads` as a canonical dataset: language tags lower-cased, blank nodes
/// relabelled canonically, each quad once.
pub fn canonical(quads: impl IntoIterator<Item = Quad>) -> Dataset {
    let mut d = Dataset::new();
    for q in quads {
        d.insert(&Quad::new(
            q.subject,
            q.predicate,
            lower_lang(q.object),
            q.graph_name,
        ));
    }
    d.canonicalize(CanonicalizationAlgorithm::Unstable);
    d
}

/// Our triples as a canonical dataset.
pub fn ours<'a>(triples: impl IntoIterator<Item = &'a Triple>) -> Dataset {
    canonical(triples.into_iter().map(quad))
}

/// `None` when the canonical datasets are equal (isomorphic), else up to
/// `max` quads only `a` has and only `b` has, named `a_name` and `b_name`.
pub fn difference(
    a: &Dataset,
    a_name: &str,
    b: &Dataset,
    b_name: &str,
    max: usize,
) -> Option<String> {
    if a == b {
        return None;
    }
    let mut out = format!(
        "not isomorphic: {a_name} {} quads, {b_name} {}\n",
        a.len(),
        b.len()
    );
    for (name, x, y) in [(a_name, a, b), (b_name, b, a)] {
        let only: Vec<_> = x.iter().filter(|q| !y.contains(*q)).collect();
        if !only.is_empty() {
            writeln!(out, "  only {name} ({}):", only.len()).unwrap();
        }
        for q in only.iter().take(max) {
            writeln!(out, "    {q}").unwrap();
        }
    }
    Some(out)
}

/// A fingerprint of how two canonical datasets differ: FNV-1a over the
/// sorted quads only one side has (marked by side), with blank node labels
/// left out, so it changes when the difference does and not with how the
/// canonical labels fall. 0 when they are equal.
pub fn fingerprint(a: &Dataset, b: &Dataset) -> u64 {
    if a == b {
        return 0;
    }
    let masked = |d: &Dataset| -> Vec<String> {
        let mut v: Vec<String> = d.iter().map(|q| mask(&q.to_string())).collect();
        v.sort();
        v
    };
    let (ma, mb) = (masked(a), masked(b));
    // The multiset difference of the masked quads, each side marked.
    let mut lines = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < ma.len() || j < mb.len() {
        match (ma.get(i), mb.get(j)) {
            (Some(x), Some(y)) if x == y => {
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x < y => {
                lines.push(format!("< {x}"));
                i += 1;
            }
            (Some(_), Some(y)) => {
                lines.push(format!("> {y}"));
                j += 1;
            }
            (Some(x), None) => {
                lines.push(format!("< {x}"));
                i += 1;
            }
            (None, Some(y)) => {
                lines.push(format!("> {y}"));
                j += 1;
            }
            (None, None) => unreachable!(),
        }
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in lines.join("\n").bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `s` (an N-Quads line) with every blank node label written `_:`.
fn mask(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let mut in_literal = false;
    while let Some(c) = rest.chars().next() {
        if in_literal {
            if c == '\\' {
                out.push_str(&rest[..rest[1..].chars().next().map_or(1, |n| 1 + n.len_utf8())]);
                rest = &rest[rest[1..].chars().next().map_or(1, |n| 1 + n.len_utf8())..];
                continue;
            }
            if c == '"' {
                in_literal = false;
            }
        } else if c == '"' {
            in_literal = true;
        } else if rest.starts_with("_:") {
            out.push_str("_:");
            let label = rest[2..]
                .find(|c: char| c.is_whitespace() || c == '>' || c == ')')
                .unwrap_or(rest.len() - 2);
            rest = &rest[2 + label..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}
