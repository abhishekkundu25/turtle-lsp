//! One document's schema facts, read in one pass over its triples, and the
//! cache that keeps them per analysis. Everything here is a pure function of
//! the analysis: a reused entry equals a fresh one.

use super::DeclaredBy;
use crate::analysis::Analysis;
use crate::knowledge::Source;
use crate::rdf::graph::Graph;
use crate::shacl::model::{Path, Shapes};
use crate::shacl::vocab;
use crate::syntax::{BlankId, Iri, Node, Span};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// A subject: an IRI, or a blank node of the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Subj {
    Iri(Iri),
    Blank(BlankId),
}

/// What a property's value is, as far as the schema cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Value {
    /// A literal, by its datatype (`rdf:langString` for a tagged string).
    Literal(Iri),
    /// An IRI or a blank node.
    Resource,
    /// A triple term.
    Other,
}

/// A triple other than `rdf:type`: its subject (an index into
/// [`DocFacts::subjects`]), predicate and value, and the triple's index in
/// the parse (where it is written).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Use {
    pub(super) s: u32,
    pub(super) p: Iri,
    pub(super) v: Value,
    pub(super) t: u32,
}

/// `rdfs:domain` or `schema:domainIncludes`: `p` is a property of `class`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Decl {
    pub(super) p: Iri,
    pub(super) class: Iri,
    pub(super) how: DeclaredBy,
    pub(super) span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ShapePath {
    Predicate(Iri),
    /// Any other path, as written.
    Written(Box<str>),
}

/// A property shape of a shape that targets `class`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ShapeRow {
    pub(super) class: Iri,
    pub(super) path: ShapePath,
    pub(super) min: Option<u32>,
    pub(super) max: Option<u32>,
    pub(super) datatype: Option<Iri>,
    /// `sh:class`.
    pub(super) of_class: Option<Iri>,
    pub(super) span: Span,
}

/// What one document says about classes, properties and instances.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct DocFacts {
    /// Every subject, in order of first mention.
    pub(super) subjects: Vec<Subj>,
    /// Per subject: where its first triple names it (see [`subject_at`]).
    pub(super) spans: Vec<Span>,
    /// `rdf:type` with an IRI class: (subject, class), in text order.
    pub(super) types: Vec<(u32, Iri)>,
    pub(super) uses: Vec<Use>,
    /// Classes this document declares (typed `rdfs:Class` / `owl:Class`, or
    /// the subject of `rdfs:subClassOf`), each once, with where.
    pub(super) classes: Vec<(Iri, Span)>,
    /// (sub, super).
    pub(super) subclass: Vec<(Iri, Iri)>,
    pub(super) decls: Vec<Decl>,
    /// (property, range).
    pub(super) ranges: Vec<(Iri, Iri)>,
    pub(super) object_props: Vec<Iri>,
    pub(super) datatype_props: Vec<Iri>,
    pub(super) shape_rows: Vec<ShapeRow>,
    /// The objects of `sh:targetClass`.
    pub(super) targets: Vec<Iri>,
    /// The prefixes in scope at the end of the document, sorted.
    pub(super) prefixes: Vec<(String, String)>,
}

/// The IRIs the facts look for, beyond [`vocab::get`].
pub(super) struct Terms {
    pub(super) domain_includes: [Iri; 2],
    pub(super) object_property: Iri,
    pub(super) datatype_property: Iri,
    pub(super) rdfs_literal: Iri,
    /// The RDF datatypes outside XSD a literal can carry.
    pub(super) rdf_datatypes: [Iri; 5],
}

pub(super) fn terms() -> &'static Terms {
    static T: OnceLock<Terms> = OnceLock::new();
    T.get_or_init(|| {
        let rdf = |l: &str| Iri::intern(&format!("{}{l}", crate::syntax::RDF));
        Terms {
            domain_includes: [
                Iri::intern("http://schema.org/domainIncludes"),
                Iri::intern("https://schema.org/domainIncludes"),
            ],
            object_property: Iri::intern("http://www.w3.org/2002/07/owl#ObjectProperty"),
            datatype_property: Iri::intern("http://www.w3.org/2002/07/owl#DatatypeProperty"),
            rdfs_literal: Iri::intern("http://www.w3.org/2000/01/rdf-schema#Literal"),
            rdf_datatypes: [
                rdf("langString"),
                rdf("dirLangString"),
                rdf("HTML"),
                rdf("XMLLiteral"),
                rdf("JSON"),
            ],
        }
    })
}

/// From `a`'s start to `b`'s end, whichever way round they are.
pub(super) fn cover(a: Span, b: Span) -> Span {
    Span::new(a.start.min(b.start), a.end.max(b.end))
}

impl DocFacts {
    pub(super) fn new(a: &Analysis) -> DocFacts {
        let v = vocab::get();
        let t = terms();
        let p = &a.parsed;
        let mut f = DocFacts::default();
        let mut index: FxHashMap<Subj, u32> = FxHashMap::default();
        let mut declared: FxHashSet<Iri> = FxHashSet::default();
        let mut shacl = false;
        for (ti, tr) in p.triples.iter().enumerate() {
            if tr.unresolved {
                continue;
            }
            let subj = match tr.s {
                Node::Iri(i) => Subj::Iri(i),
                Node::Blank(b) => Subj::Blank(b),
                _ => continue,
            };
            let s = *index.entry(subj).or_insert_with(|| {
                f.subjects.push(subj);
                f.spans.push(subject_at(a, tr));
                (f.subjects.len() - 1) as u32
            });
            let o_iri = match tr.o {
                Node::Iri(o) => Some(o),
                _ => None,
            };
            let mut declares = false;
            if tr.p == v.rdf_type {
                if let Some(o) = o_iri {
                    f.types.push((s, o));
                    declares = o == v.rdfs_class || o == v.owl_class;
                    if o == t.object_property {
                        if let Subj::Iri(i) = subj {
                            f.object_props.push(i);
                        }
                    } else if o == t.datatype_property {
                        if let Subj::Iri(i) = subj {
                            f.datatype_props.push(i);
                        }
                    } else if o == v.node_shape {
                        shacl = true;
                    }
                }
                if let (true, Subj::Iri(i)) = (declares, subj)
                    && declared.insert(i)
                {
                    f.classes.push((i, subject_span(a, tr)));
                }
                continue;
            }
            f.uses.push(Use {
                s,
                p: tr.p,
                v: match &tr.o {
                    Node::Literal { datatype, .. } => Value::Literal(*datatype),
                    Node::Iri(_) | Node::Blank(_) => Value::Resource,
                    Node::Triple(_) => Value::Other,
                },
                t: ti as u32,
            });
            if tr.p == v.target_class {
                shacl = true;
                f.targets.extend(o_iri);
            }
            let (Subj::Iri(si), Some(o)) = (subj, o_iri) else {
                continue;
            };
            let span = cover(tr.p_span, tr.o_span);
            if tr.p == v.sub_class_of {
                f.subclass.push((si, o));
                declares = true;
            } else if tr.p == v.rdfs_domain {
                f.decls.push(Decl {
                    p: si,
                    class: o,
                    how: DeclaredBy::Domain,
                    span,
                });
            } else if t.domain_includes.contains(&tr.p) {
                f.decls.push(Decl {
                    p: si,
                    class: o,
                    how: DeclaredBy::DomainIncludes,
                    span,
                });
            } else if tr.p == v.rdfs_range {
                f.ranges.push((si, o));
            }
            if declares && declared.insert(si) {
                f.classes.push((si, subject_span(a, tr)));
            }
        }
        if shacl {
            f.shape_rows = shape_rows(a);
        }
        let mut prefixes: Vec<(String, String)> = a.prefix_map().into_iter().collect();
        prefixes.sort_unstable();
        f.prefixes = prefixes;
        f
    }
}

/// Where the triple's statement names its subject, else the statement.
fn subject_span(a: &Analysis, tr: &crate::syntax::Triple) -> Span {
    match a.parsed.statements.get(tr.statement as usize) {
        Some(st) => match st.subject {
            Some((i, span)) if Node::Iri(i) == tr.s => span,
            _ => st.span,
        },
        None => tr.p_span,
    }
}

/// Where a subject is named: the statement's subject when the triple's is
/// that IRI, else the triple's predicate and object (a blank node's
/// `[ ... ]`, a nested description).
pub(super) fn subject_at(a: &Analysis, tr: &crate::syntax::Triple) -> Span {
    match a
        .parsed
        .statements
        .get(tr.statement as usize)
        .and_then(|st| st.subject)
    {
        Some((i, span)) if Node::Iri(i) == tr.s => span,
        _ => cover(tr.p_span, tr.o_span),
    }
}

/// The property shapes of the shapes that target a class, read with the
/// SHACL model over this document alone.
fn shape_rows(a: &Analysis) -> Vec<ShapeRow> {
    let v = vocab::get();
    let triples = &a.parsed.triples;
    let g = Graph::new(&[triples]);
    let shapes = Shapes::new(&g);
    let span_of = |i: u32| triples[g.origin(i).1 as usize].o_span;
    let mut rows = Vec::new();
    for shape in &shapes.shapes {
        let mut classes: Vec<Iri> = shape
            .targets
            .iter()
            .map(|&i| g.triple_at(i))
            .filter(|t| t.p == v.target_class)
            .filter_map(|t| g.as_iri(t.o))
            .collect();
        if let Some(node) = g.as_iri(shape.node)
            && shape.typed_node_shape
            && g.objects(shape.node, v.rdf_type)
                .filter_map(|o| g.as_iri(o))
                .any(|o| o == v.rdfs_class || o == v.owl_class)
        {
            classes.push(node);
        }
        if classes.is_empty() {
            continue;
        }
        let mut add = |ps: &crate::shacl::model::Shape, span: Span| {
            let Some((path, pi)) = &ps.path else {
                return;
            };
            let path = match path {
                Path::Predicate(p) => ShapePath::Predicate(*p),
                _ => {
                    let s = span_of(*pi);
                    let text = a.text.get(s.start as usize..s.end as usize).unwrap_or("");
                    ShapePath::Written(text.trim().into())
                }
            };
            let (mut min, mut max, mut datatype, mut of_class) = (None, None, None, None);
            for &i in &ps.params {
                let t = g.triple_at(i);
                let count = || {
                    g.term(t.o)
                        .literal()
                        .and_then(|(text, ..)| text.trim().parse::<u32>().ok())
                };
                if t.p == v.min_count {
                    min = min.or_else(count);
                } else if t.p == v.max_count {
                    max = max.or_else(count);
                } else if t.p == v.datatype {
                    datatype = datatype.or_else(|| g.as_iri(t.o));
                } else if t.p == v.class {
                    of_class = of_class.or_else(|| g.as_iri(t.o));
                }
            }
            for &class in &classes {
                rows.push(ShapeRow {
                    class,
                    path: path.clone(),
                    min,
                    max,
                    datatype,
                    of_class,
                    span,
                });
            }
        };
        if let Some((_, pi)) = &shape.path {
            let tr = &triples[g.origin(*pi).1 as usize];
            add(shape, cover(tr.p_span, tr.o_span));
        }
        for &i in &shape.params {
            let t = g.triple_at(i);
            if t.p != v.property {
                continue;
            }
            if let Some(ps) = shapes.get(t.o) {
                add(ps, span_of(i));
            }
        }
    }
    rows
}

/// Per-document facts kept across requests, keyed by the source's key and
/// the analysis they were computed from (a changed document has a new
/// analysis). Shared between jobs: the lock is held only to look up and to
/// insert; facts are computed outside it, and two jobs computing the same
/// entry write equal values.
#[derive(Default)]
pub(crate) struct FactsCache {
    map: Mutex<FxHashMap<String, Entry>>,
}

struct Entry {
    /// Keeps the analysis' allocation, so its address cannot be reused
    /// while the entry lives.
    analysis: Weak<Analysis>,
    facts: Arc<DocFacts>,
}

impl FactsCache {
    /// The facts of `sources[i]` for each `i` in `wanted`, in that order.
    /// Entries of keys no longer among `sources` are dropped.
    pub(super) fn facts(
        &self,
        sources: &[(String, Source, Arc<Analysis>)],
        wanted: &[usize],
    ) -> Vec<Arc<DocFacts>> {
        let mut out: Vec<Option<Arc<DocFacts>>> = {
            let map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            wanted
                .iter()
                .map(|&i| {
                    let (key, _, a) = &sources[i];
                    map.get(key)
                        .filter(|e| std::ptr::eq(e.analysis.as_ptr(), Arc::as_ptr(a)))
                        .map(|e| e.facts.clone())
                })
                .collect()
        };
        let missing: Vec<usize> = (0..wanted.len()).filter(|&j| out[j].is_none()).collect();
        let fresh: Vec<Arc<DocFacts>> = missing
            .par_iter()
            .map(|&j| Arc::new(DocFacts::new(&sources[wanted[j]].2)))
            .collect();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        for (&j, facts) in missing.iter().zip(fresh) {
            let (key, _, a) = &sources[wanted[j]];
            map.insert(
                key.clone(),
                Entry {
                    analysis: Arc::downgrade(a),
                    facts: facts.clone(),
                },
            );
            out[j] = Some(facts);
        }
        if map.len() > sources.len() || !missing.is_empty() {
            let keys: FxHashSet<&str> = sources.iter().map(|(k, ..)| k.as_str()).collect();
            map.retain(|k, _| keys.contains(k.as_str()));
        }
        out.into_iter().map(|f| f.expect("filled")).collect()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}
