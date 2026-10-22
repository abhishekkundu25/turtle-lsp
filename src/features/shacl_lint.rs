//! SHACL shape lint: mistakes in the shapes themselves, found as you type
//! from the document's triples and the workspace knowledge index.
//!
//! Parameter scopes follow the SHACL Core syntax rules: node shapes cannot
//! have `sh:path`, `sh:minCount`, `sh:maxCount`, `sh:uniqueLang`,
//! `sh:lessThan`, `sh:lessThanOrEquals` or `sh:qualifiedValueShape`. The spec
//! restricts no parameter to node shapes (targets and `sh:closed` are allowed
//! on property shapes), so property shapes are only checked for `sh:path`.
//!
//! "Known nowhere" (classes, datatypes, shapes) is judged only for IRIs in an
//! own namespace of the workspace: an IRI from an ontology that is referenced
//! but not loaded stays quiet, and so does a well-known vocabulary, where
//! unknown-term already reports typos.

use crate::analysis::Analysis;
use crate::features::hover::short;
use crate::knowledge::{Knowledge, namespace_of};
use crate::rdf::Graph;
use crate::shacl::{Shape, ShapeKind, Shapes, model, vocab};
use crate::syntax::{Iri, Node, PrefixMap, RDF, Span, Triple, XSD};
use crate::vocab::{TermKind, VOCABS};
use rustc_hash::FxHashMap;
use std::borrow::Cow;

mod sparql;

/// Codes that depend on the whole workspace (see [`super::diagnostics::needs_index`]).
pub(crate) const NEEDS_INDEX: &[&str] = &[
    // An IRI shape's sh:path may live in a file the index has not read yet.
    "shacl-missing-path",
    "shacl-unknown-class",
    "shacl-bad-datatype",
    "shacl-dangling-shape",
    // sh:prefixes may be declared in a file the index has not read yet.
    sparql::PREFIX,
];

pub(crate) struct Finding {
    pub(crate) span: Span,
    pub(crate) error: bool,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

const RDF_DATATYPES: [&str; 5] = ["langString", "HTML", "XMLLiteral", "JSON", "dirLangString"];
/// Well-known classes that are often mistaken for datatypes.
const NOT_DATATYPES: [&str; 2] = [
    "http://www.w3.org/2000/01/rdf-schema#Literal",
    "http://www.w3.org/2000/01/rdf-schema#Resource",
];

/// Whether "described nowhere" means something for `iri`: its namespace is
/// one the workspace describes (see [`Knowledge::is_project_namespace`]), not
/// one it only references, such as an ontology that is not loaded. Never
/// a well-known vocabulary: unknown-term covers typos there.
fn judged(k: &Knowledge, iri: &str) -> bool {
    k.is_project_namespace(namespace_of(iri))
}

struct Ctx<'a> {
    a: &'a Analysis,
    k: &'a Knowledge,
    prefixes: Option<PrefixMap>,
    out: Vec<Finding>,
}

impl Ctx<'_> {
    fn name(&mut self, iri: &str) -> String {
        let a = self.a;
        short(iri, self.prefixes.get_or_insert_with(|| a.prefix_map()))
    }
    fn push(&mut self, span: Span, error: bool, code: &'static str, message: String) {
        self.out.push(Finding {
            span,
            error,
            code,
            message,
        });
    }
}

/// The triples the shape model reads: all of the document's. A triple with
/// an undeclared prefix ([`Triple::unresolved`]) still counts here, as the
/// author wrote it (`sh:path foaf:name` before `foaf:` is declared), while
/// [`Graph::new`] leaves it out of the graph that validation reads; such a
/// document's triples are copied with the flag cleared.
fn model_triples(triples: &[Triple]) -> Cow<'_, [Triple]> {
    if !triples.iter().any(|t| t.unresolved) {
        return Cow::Borrowed(triples);
    }
    Cow::Owned(
        triples
            .iter()
            .map(|t| Triple {
                unresolved: false,
                ..t.clone()
            })
            .collect(),
    )
}

/// Whether `triples` can hold a shape or a SHACL parameter: a predicate in
/// the SHACL namespace, or a type there.
fn mentions_shacl(triples: &[Triple]) -> bool {
    let rdf_type = vocab::get().rdf_type;
    let shacl = |i: &Iri| i.starts_with(crate::shacl::SH);
    triples
        .iter()
        .any(|t| shacl(&t.p) || matches!(&t.o, Node::Iri(o) if t.p == rdf_type && shacl(o)))
}

pub(crate) fn lint(a: &Analysis, k: &Knowledge) -> Vec<Finding> {
    let triples = &a.parsed.triples;
    if !mentions_shacl(triples) {
        return Vec::new();
    }
    let all = model_triples(triples);
    let g = Graph::new(&[&*all]);
    let shapes = Shapes::new(&g);
    if shapes.shapes.is_empty() && !triples.iter().any(|t| t.p.starts_with(crate::shacl::SH)) {
        return Vec::new();
    }
    // The document triple a graph triple number stands for.
    let doc = |n: u32| &triples[g.origin(n).1 as usize];
    let v = vocab::get();
    let mut cx = Ctx {
        a,
        k,
        prefixes: None,
        out: Vec::new(),
    };

    // Shape structure: missing paths and parameters a node shape cannot have.
    let node_only_not: [Iri; 6] = [
        v.min_count,
        v.max_count,
        v.unique_lang,
        v.less_than,
        v.less_than_or_equals,
        v.qualified_value_shape,
    ];
    for s in &shapes.shapes {
        if s.typed_node_shape
            && let Some((_, i)) = &s.path
        {
            cx.push(
                doc(*i).p_span,
                false,
                "shacl-misplaced",
                "A sh:NodeShape cannot have sh:path; make it a sh:PropertyShape".into(),
            );
        }
        match s.kind {
            ShapeKind::Property if s.path.is_none() && !path_elsewhere(k, g.as_iri(s.node)) => {
                let span = shape_span(a, &g, s);
                cx.push(
                    span,
                    true,
                    "shacl-missing-path",
                    "Property shape has no sh:path".into(),
                );
            }
            ShapeKind::Node if s.typed_node_shape || s.via_node || !s.targets.is_empty() => {
                for &i in &s.params {
                    let t = doc(i);
                    if node_only_not.contains(&t.p) {
                        let name = cx.name(&t.p);
                        cx.push(
                            t.p_span,
                            false,
                            "shacl-misplaced",
                            format!(
                                "{name} applies to property shapes only; this is a node shape (no sh:path)"
                            ),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // Parameter values.
    let mut class_known: FxHashMap<Iri, bool> = FxHashMap::default();
    // Per subject (an IRI or a blank node).
    let mut counts: FxHashMap<&Node, [Option<(i64, u32)>; 4]> = FxHashMap::default();
    let count_preds = [
        v.min_count,
        v.max_count,
        v.qualified_min_count,
        v.qualified_max_count,
    ];
    for (i, t) in triples.iter().enumerate() {
        if !t.p.starts_with(crate::shacl::SH) {
            continue;
        }
        if let Some(slot) = count_preds.iter().position(|&p| p == t.p) {
            if let (Node::Literal { value, .. }, Node::Iri(_) | Node::Blank(_)) = (&t.o, &t.s)
                && let Ok(n) = value.trim().trim_start_matches('+').parse::<i64>()
            {
                if n < 0 {
                    let name = cx.name(&t.p);
                    cx.push(
                        t.o_span,
                        true,
                        "shacl-count-conflict",
                        format!("{name} must not be negative"),
                    );
                }
                counts.entry(&t.s).or_default()[slot].get_or_insert((n, i as u32));
            }
            continue;
        }
        let Node::Iri(o) = &t.o else {
            // sh:and / sh:or / sh:xone members.
            if t.p == v.and || t.p == v.or || t.p == v.xone {
                let head = g.node(0, &t.o);
                for m in head.map(|h| model::list(&g, h)).unwrap_or_default() {
                    let mt = doc(m);
                    if let Node::Iri(o) = &mt.o {
                        dangling(&mut cx, *o, mt.o_span);
                    }
                }
            }
            continue;
        };
        let o = *o;
        if t.p == v.target_class || t.p == v.class {
            if !judged(k, &o) {
                continue;
            }
            let known = *class_known.entry(o).or_insert_with(|| is_known_class(k, o));
            if !known {
                let name = cx.name(&o);
                let pred = cx.name(&t.p);
                cx.push(
                    t.o_span,
                    false,
                    "shacl-unknown-class",
                    format!("{pred} {name}: no class by that name is known in the workspace"),
                );
            }
        } else if t.p == v.datatype {
            if !is_datatype(k, o) {
                let name = cx.name(&o);
                cx.push(
                    t.o_span,
                    false,
                    "shacl-bad-datatype",
                    format!(
                        "{name} is not an XSD or RDF datatype, nor declared as an rdfs:Datatype"
                    ),
                );
            }
        } else if t.p == v.node
            || t.p == v.property
            || t.p == v.qualified_value_shape
            || t.p == v.not
        {
            dangling(&mut cx, o, t.o_span);
        }
    }

    let mut conflicts: Vec<_> = counts.into_values().collect();
    conflicts.sort_by_key(|c| c.iter().flatten().map(|x| x.1).min());
    for c in conflicts {
        for (lo, hi) in [(0, 1), (2, 3)] {
            if let (Some((min, _)), Some((max, j))) = (c[lo], c[hi])
                && min > max
                && max >= 0
            {
                let t = &triples[j as usize];
                let (lo_name, hi_name) = (cx.name(&count_preds[lo]), cx.name(&t.p));
                cx.push(
                    t.o_span,
                    true,
                    "shacl-count-conflict",
                    format!("{hi_name} {max} is less than {lo_name} {min}"),
                );
            }
        }
    }

    sparql::lint(a, k, &mut cx.out);

    cx.out.sort_by_key(|f| (f.span.start, f.span.end));
    cx.out
}

/// Where to report a property shape: its subject when it has an IRI, the
/// opening `[` of an inline shape, else its type.
fn shape_span(a: &Analysis, g: &Graph, s: &Shape) -> Span {
    let triples = &a.parsed.triples;
    let doc = |n: u32| &triples[g.origin(n).1 as usize];
    if let Some(i) = g.as_iri(s.node)
        && let Some(span) = a.defs.get(&i).and_then(|d| d.first())
    {
        return *span;
    }
    if let Some(i) = s.via_property {
        let o = doc(i).o_span;
        return if a.text.as_bytes().get(o.start as usize) == Some(&b'[') {
            Span::new(o.start, o.start + 1)
        } else {
            o
        };
    }
    let own = || g.out(s.node).map(|(n, _)| doc(n));
    own()
        .find(|t| t.p == vocab::get().rdf_type)
        .or_else(|| own().next())
        .map_or_else(Span::default, |t| t.o_span)
}

/// An IRI property shape whose `sh:path` another source states.
fn path_elsewhere(k: &Knowledge, node: Option<Iri>) -> bool {
    let Some(iri) = node else {
        return false;
    };
    let path = vocab::get().path;
    k.descriptions(&iri).iter().any(|d| {
        d.analysis
            .parsed
            .triples
            .iter()
            .any(|t| t.p == path && t.s == Node::Iri(iri))
    })
}

/// Known as a class: typed as one (or a node shape, an implicit class
/// target), in a class hierarchy, or with instances.
fn is_known_class(k: &Knowledge, iri: Iri) -> bool {
    let v = vocab::get();
    k.types(&iri)
        .iter()
        .any(|t| *t == v.rdfs_class || *t == v.owl_class || *t == v.node_shape)
        || !k.supers(&iri).is_empty()
        || !k.subs(&iri).is_empty()
        || !k.instances(&iri).is_empty()
}

fn is_datatype(k: &Knowledge, iri: Iri) -> bool {
    if iri.starts_with(XSD) {
        return true;
    }
    if let Some(local) = iri.strip_prefix(RDF) {
        // Unknown rdf: terms are already reported as unknown-term.
        return RDF_DATATYPES.contains(&local)
            || VOCABS
                .lookup(&iri)
                .is_some_and(|(v, l)| v.term(l).is_none());
    }
    if let Some((voc, local)) = VOCABS.lookup(&iri) {
        return match voc.term(local) {
            None => true,
            Some(t) => t.kind == TermKind::Class && !NOT_DATATYPES.contains(&&*iri),
        };
    }
    !judged(k, &iri) || k.types(&iri).contains(&vocab::get().rdfs_datatype)
}

fn dangling(cx: &mut Ctx, iri: Iri, span: Span) {
    if !judged(cx.k, &iri) || cx.k.is_described(&iri) {
        return;
    }
    let name = cx.name(&iri);
    cx.push(
        span,
        false,
        "shacl-dangling-shape",
        format!("Shape {name} is not described anywhere in the workspace"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::Source;
    use std::sync::Arc;

    /// Triples with an undeclared prefix still count in the shape model, as
    /// they did when the lint read the parse directly: `nope:S` is a node
    /// shape, `sh:path nope:p` makes `ex:T` a property shape, and the list
    /// cell holding `nope:A` still leads to `ex:Gone`.
    #[test]
    fn undeclared_prefixes_still_count() {
        let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n\
                    nope:S a sh:NodeShape ; sh:minCount 1 ; sh:property [ sh:minCount 1 ] .\n\
                    ex:T a sh:NodeShape ; sh:path nope:p ; sh:minCount 1 .\n\
                    ex:U sh:or ( nope:A ex:Gone ) .\n\
                    ex:V sh:targetNode ex:U .\n";
        let a = Arc::new(Analysis::new(
            "file:///w/shapes.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ));
        let mut k = Knowledge::default();
        k.set("file:///w/shapes.ttl", Source::Open, a.clone());
        let found: Vec<_> = lint(&a, &k)
            .into_iter()
            .map(|f| {
                (
                    f.code,
                    f.error,
                    &text[f.span.start as usize..f.span.end as usize],
                    f.message,
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (
                    "shacl-misplaced",
                    false,
                    "sh:minCount",
                    "sh:minCount applies to property shapes only; this is a node shape (no sh:path)"
                        .to_owned()
                ),
                (
                    "shacl-missing-path",
                    true,
                    "[",
                    "Property shape has no sh:path".to_owned()
                ),
                (
                    "shacl-misplaced",
                    false,
                    "sh:path",
                    "A sh:NodeShape cannot have sh:path; make it a sh:PropertyShape".to_owned()
                ),
                (
                    "shacl-dangling-shape",
                    false,
                    "ex:Gone",
                    "Shape ex:Gone is not described anywhere in the workspace".to_owned()
                ),
            ]
        );
    }
}
