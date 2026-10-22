//! The SHACL shapes in a [`Graph`]: node shapes, property shapes (including
//! blank-node shapes reached through `sh:property`), their paths, targets and
//! parameters. Shapes refer to their triples by graph triple number;
//! [`Graph::origin`] turns one into the document and the index of the triple
//! in that document's parse, so callers keep every span (`p_span`, `o_span`)
//! the parser recorded.
//!
//! A node counts as a shape when it is typed `sh:NodeShape` /
//! `sh:PropertyShape`, has a target (a Core one or a SHACL-SPARQL
//! `sh:target`) or `sh:path`, or is the value of a shape-expecting parameter
//! (`sh:property`, `sh:node`, `sh:qualifiedValueShape`, `sh:not`, members of
//! `sh:and` / `sh:or` / `sh:xone`). IRIs count only when these triples
//! describe them; blank nodes always (`sh:property [ ]` is an empty property
//! shape).
//!
//! Building the model costs the SHACL triples and the shapes' own, found
//! through the graph's indexes, not a pass over every triple: the validator
//! builds it once per run, and a run over an overlay must cost in proportion
//! to the overlay and the shapes ([`Graph::overlay`]).

use super::vocab;
use crate::rdf::graph::{Graph, Id, Term};
use crate::syntax::Iri;
use rustc_hash::{FxHashMap, FxHashSet};

/// A SHACL property path.
#[derive(Clone, Debug, PartialEq)]
pub enum Path {
    Predicate(Iri),
    Inverse(Box<Path>),
    Sequence(Vec<Path>),
    Alternative(Vec<Path>),
    ZeroOrMore(Box<Path>),
    OneOrMore(Box<Path>),
    ZeroOrOne(Box<Path>),
    /// Not a well-formed path (a literal, an unknown blank node, too deep).
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Node,
    /// Has `sh:path`, is typed `sh:PropertyShape`, or is a value of `sh:property`.
    Property,
}

/// One shape. Every `u32` is a graph triple number.
#[derive(Clone, Debug)]
pub struct Shape {
    /// An IRI or a blank node.
    pub node: Id,
    pub kind: ShapeKind,
    pub typed_node_shape: bool,
    pub typed_property_shape: bool,
    /// The first `sh:property` triple with this shape as value.
    pub via_property: Option<u32>,
    /// The value of some `sh:node` (so a node shape).
    pub via_node: bool,
    /// The `sh:path` as written (the first, if there are several) and its
    /// triple.
    pub path: Option<(Path, u32)>,
    /// `sh:targetClass`, `sh:targetNode`, `sh:targetSubjectsOf` and
    /// `sh:targetObjectsOf` triples.
    pub targets: Vec<u32>,
    /// `sh:target` triples (SHACL-SPARQL targets).
    pub sparql_targets: Vec<u32>,
    /// Every triple with this shape as subject and a predicate in the SHACL
    /// namespace (constraint parameters, path, targets), in graph order.
    pub params: Vec<u32>,
}

#[derive(Default)]
pub struct Shapes {
    /// In the order the graph first makes each node a shape.
    pub shapes: Vec<Shape>,
    index: FxHashMap<Id, u32>,
}

const MAX_DEPTH: u32 = 64;

/// An IRI or a blank node: a term that can be a shape or a list cell.
fn is_node(g: &Graph, id: Id) -> bool {
    matches!(g.term(id), Term::Iri(_) | Term::Blank(_))
}

impl Shapes {
    /// The shapes in `g`, in the order the graph's triples first make each
    /// node a shape (by triple number; members of one `sh:and` / `sh:or` /
    /// `sh:xone` list in list order).
    pub fn new(g: &Graph) -> Shapes {
        let v = vocab::get();
        // A value counts as a shape when it is a blank node or an IRI with
        // triples of its own.
        let described = |o: Id| match g.term(o) {
            Term::Blank(_) => true,
            Term::Iri(_) => g.out(o).next().is_some(),
            Term::Value(_) => false,
        };
        let makes_subject = |p: Iri| v.is_target(p) || p == v.target || p == v.path;
        let makes_object =
            |p: Iri| p == v.property || p == v.node || p == v.qualified_value_shape || p == v.not;
        let makes_members = |p: Iri| p == v.and || p == v.or || p == v.xone;

        // Each time a triple makes a node a shape: (triple number, position
        // in its list, node).
        let mut made: Vec<(u32, u32, Id)> = Vec::new();
        let mut via_property: FxHashMap<Id, u32> = FxHashMap::default();
        let mut via_node: FxHashSet<Id> = FxHashSet::default();
        for class in [v.node_shape, v.property_shape] {
            for (i, t) in g.iri(class).into_iter().flat_map(|c| g.incoming(c)) {
                if t.p == v.rdf_type && is_node(g, t.s) {
                    made.push((i, 0, t.s));
                }
            }
        }
        // The subjects of those predicates, whose own triples carry the
        // triple numbers.
        let mut subjects: Vec<Id> = [
            v.target_class,
            v.target_node,
            v.target_subjects_of,
            v.target_objects_of,
            v.target,
            v.path,
            v.property,
            v.node,
            v.qualified_value_shape,
            v.not,
            v.and,
            v.or,
            v.xone,
        ]
        .into_iter()
        .flat_map(|p| g.with_p(p).map(|t| t.s))
        .collect();
        subjects.sort_unstable();
        subjects.dedup();
        for s in subjects {
            if !is_node(g, s) {
                continue;
            }
            for (i, t) in g.out(s) {
                let p = t.p;
                if makes_subject(p) {
                    made.push((i, 0, s));
                } else if makes_object(p) {
                    if described(t.o) {
                        made.push((i, 0, t.o));
                        if p == v.property {
                            let first = via_property.entry(t.o).or_insert(i);
                            *first = (*first).min(i);
                        } else if p == v.node {
                            via_node.insert(t.o);
                        }
                    }
                } else if makes_members(p) {
                    for (k, m) in list(g, t.o).into_iter().enumerate() {
                        let o = g.triple_at(m).o;
                        if described(o) {
                            made.push((i, k as u32, o));
                        }
                    }
                }
            }
        }
        made.sort_unstable();
        let mut seen: FxHashSet<Id> = FxHashSet::default();
        let order = made.into_iter().map(|m| m.2).filter(|&n| seen.insert(n));

        let shacl = |i: &Iri| i.starts_with(super::SH);
        let mut s = Shapes::default();
        for node in order {
            let mut shape = Shape {
                node,
                kind: ShapeKind::Node,
                typed_node_shape: false,
                typed_property_shape: false,
                via_property: via_property.get(&node).copied(),
                via_node: via_node.contains(&node),
                path: None,
                targets: Vec::new(),
                sparql_targets: Vec::new(),
                params: Vec::new(),
            };
            for (i, t) in g.out(node) {
                if t.p == v.rdf_type {
                    if let Some(o) = g.as_iri(t.o) {
                        shape.typed_node_shape |= o == v.node_shape;
                        shape.typed_property_shape |= o == v.property_shape;
                    }
                    continue;
                }
                if !shacl(&t.p) {
                    continue;
                }
                shape.params.push(i);
                if t.p == v.path && shape.path.is_none() {
                    shape.path = Some((path(g, t.o), i));
                } else if v.is_target(t.p) {
                    shape.targets.push(i);
                } else if t.p == v.target {
                    shape.sparql_targets.push(i);
                }
            }
            if shape.path.is_some() || shape.typed_property_shape || shape.via_property.is_some() {
                shape.kind = ShapeKind::Property;
            }
            s.index.insert(node, s.shapes.len() as u32);
            s.shapes.push(shape);
        }
        s
    }

    pub fn get(&self, node: Id) -> Option<&Shape> {
        self.index.get(&node).map(|&i| &self.shapes[i as usize])
    }
}

/// The `rdf:first` triples of the RDF collection starting at `head`, in
/// order (cycle-safe; stops at the first malformed cell). A cell with several
/// `rdf:first` or `rdf:rest` triples follows the first of each.
pub fn list(g: &Graph, head: Id) -> Vec<u32> {
    let v = vocab::get();
    let mut out = Vec::new();
    let mut visited = FxHashSet::default();
    let mut cur = head;
    while is_node(g, cur) && g.as_iri(cur) != Some(v.rdf_nil) && visited.insert(cur) {
        let mut next = None;
        let mut first = None;
        for (i, t) in g.out(cur) {
            if t.p == v.rdf_first && first.is_none() {
                first = Some(i);
            } else if t.p == v.rdf_rest && next.is_none() {
                next = Some(t.o);
            }
        }
        let Some(first) = first else { break };
        out.push(first);
        match next {
            Some(n) => cur = n,
            None => break,
        }
    }
    out
}

/// The property path whose root is `node`, as written: nested repetitions
/// stay nested (the validator merges them, see
/// [`super::validate::parse_path`]). Recursion stops past 64 levels with
/// [`Path::Invalid`].
pub fn path(g: &Graph, node: Id) -> Path {
    path_at(g, node, 0)
}

fn path_at(g: &Graph, node: Id, depth: u32) -> Path {
    let v = vocab::get();
    if depth > MAX_DEPTH {
        return Path::Invalid;
    }
    match g.term(node) {
        Term::Iri(i) if i == v.rdf_nil => Path::Invalid,
        Term::Iri(i) => Path::Predicate(i),
        Term::Blank(_) => {
            let members = |head: Id| -> Vec<Path> {
                list(g, head)
                    .into_iter()
                    .map(|m| path_at(g, g.triple_at(m).o, depth + 1))
                    .collect()
            };
            if g.out(node).any(|(_, t)| t.p == v.rdf_first) {
                return Path::Sequence(members(node));
            }
            for (_, t) in g.out(node) {
                let inner = || Box::new(path_at(g, t.o, depth + 1));
                if t.p == v.inverse_path {
                    return Path::Inverse(inner());
                } else if t.p == v.alternative_path {
                    return Path::Alternative(members(t.o));
                } else if t.p == v.zero_or_more_path {
                    return Path::ZeroOrMore(inner());
                } else if t.p == v.one_or_more_path {
                    return Path::OneOrMore(inner());
                } else if t.p == v.zero_or_one_path {
                    return Path::ZeroOrOne(inner());
                }
            }
            Path::Invalid
        }
        Term::Value(_) => Path::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{Parsed, PrefixMap, Syntax, parse};

    const HEAD: &str =
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n";

    fn parsed(body: &str) -> Parsed {
        parse(
            &format!("{HEAD}{body}"),
            &PrefixMap::default(),
            Syntax::Turtle,
        )
    }

    fn ex(l: &str) -> Iri {
        Iri::intern(&format!("http://example.com/{l}"))
    }

    #[test]
    fn no_shacl_no_work() {
        let p = parsed("ex:a ex:b ex:c .");
        let g = Graph::new(&[&p.triples]);
        assert!(Shapes::new(&g).shapes.is_empty());
    }

    #[test]
    fn node_and_blank_property_shapes() {
        let p = parsed(
            "ex:S a sh:NodeShape ; sh:targetClass ex:C ;\n  sh:property [ sh:path ex:p ; sh:minCount 1 ] , [ sh:minCount 2 ] .",
        );
        let g = Graph::new(&[&p.triples]);
        let s = Shapes::new(&g);
        assert_eq!(s.shapes.len(), 3);
        let n = s.get(g.iri(ex("S")).unwrap()).unwrap();
        assert_eq!(n.kind, ShapeKind::Node);
        assert!(n.typed_node_shape);
        assert_eq!(n.targets.len(), 1);
        assert_eq!(s.shapes[1].kind, ShapeKind::Property);
        assert_eq!(
            s.shapes[1].path.as_ref().unwrap().0,
            Path::Predicate(ex("p"))
        );
        assert!(s.shapes[2].path.is_none());
        assert!(s.shapes[2].via_property.is_some());
    }

    #[test]
    fn paths() {
        let p = parsed(
            "ex:S sh:property [ sh:path ( ex:a [ sh:inversePath ex:b ] [ sh:alternativePath ( ex:c [ sh:zeroOrMorePath ex:d ] ) ] [ sh:oneOrMorePath ex:e ] [ sh:zeroOrOnePath ex:f ] ) ] .",
        );
        let g = Graph::new(&[&p.triples]);
        let s = Shapes::new(&g);
        let ps = s.shapes.iter().find(|x| x.path.is_some()).unwrap();
        use Path::*;
        let b = |p: Path| Box::new(p);
        assert_eq!(
            ps.path.as_ref().unwrap().0,
            Sequence(vec![
                Predicate(ex("a")),
                Inverse(b(Predicate(ex("b")))),
                Alternative(vec![Predicate(ex("c")), ZeroOrMore(b(Predicate(ex("d"))))]),
                OneOrMore(b(Predicate(ex("e")))),
                ZeroOrOne(b(Predicate(ex("f")))),
            ])
        );
    }

    /// The shapes, `via_property` and `via_node` that one pass over every
    /// triple in number order finds: how the model was built before it read
    /// the graph's indexes (with `sh:target` added).
    fn by_scan(g: &Graph) -> (Vec<Id>, FxHashMap<Id, u32>, FxHashSet<Id>) {
        let v = vocab::get();
        let described = |o: Id| match g.term(o) {
            Term::Blank(_) => true,
            Term::Iri(_) => g.out(o).next().is_some(),
            Term::Value(_) => false,
        };
        let mut order = Vec::new();
        let mut seen = FxHashSet::default();
        let mut via_property = FxHashMap::default();
        let mut via_node = FxHashSet::default();
        let mut add = |n: Id, order: &mut Vec<Id>| {
            if seen.insert(n) {
                order.push(n);
            }
        };
        for i in 0..g.len() as u32 {
            let t = g.triple_at(i);
            let p = t.p;
            if p == v.rdf_type {
                if g.as_iri(t.o)
                    .is_some_and(|o| o == v.node_shape || o == v.property_shape)
                {
                    add(t.s, &mut order);
                }
            } else if v.is_target(p) || p == v.target || p == v.path {
                add(t.s, &mut order);
            } else if p == v.property || p == v.node || p == v.qualified_value_shape || p == v.not {
                if described(t.o) {
                    add(t.o, &mut order);
                    if p == v.property {
                        via_property.entry(t.o).or_insert(i);
                    } else if p == v.node {
                        via_node.insert(t.o);
                    }
                }
            } else if p == v.and || p == v.or || p == v.xone {
                for m in list(g, t.o) {
                    let o = g.triple_at(m).o;
                    if described(o) {
                        add(o, &mut order);
                    }
                }
            }
        }
        (order, via_property, via_node)
    }

    fn same_as_by_scan(g: &Graph) {
        let s = Shapes::new(g);
        let (order, via_property, via_node) = by_scan(g);
        assert_eq!(s.shapes.iter().map(|x| x.node).collect::<Vec<_>>(), order);
        for x in &s.shapes {
            assert_eq!(x.via_property, via_property.get(&x.node).copied());
            assert_eq!(x.via_node, via_node.contains(&x.node));
        }
    }

    /// Read through the indexes, the shapes come in the order a pass over
    /// every triple finds them, also over an overlay (whose lookups list its
    /// own triples before the base's).
    #[test]
    fn the_indexes_find_the_shapes_a_scan_finds_in_the_same_order() {
        let base = parsed(
            "ex:Q sh:path ex:q ; sh:not [ sh:class ex:C ] .
             ex:T sh:target [ sh:select \"SELECT ?this {}\" ] .
             ex:S a sh:NodeShape ; sh:targetClass ex:C ;
               sh:property [ sh:path ex:p ] , ex:Q ;
               sh:and ( ex:A [ sh:minCount 1 ] ex:Undescribed ) ;
               sh:xone ( ex:Q ex:S ) .
             ex:A sh:datatype ex:d .
             ex:P a sh:PropertyShape ; sh:node ex:A ; sh:property ex:Q .
             ex:S sh:targetNode ex:x .
",
        );
        let g = Graph::new(&[&base.triples]);
        same_as_by_scan(&g);
        let over = parsed(
            "ex:O sh:targetSubjectsOf ex:p ; sh:property ex:Q , [ sh:path ex:r ] .
             ex:A sh:or ( ex:O [ sh:maxCount 2 ] ) .
             ex:N a sh:NodeShape .
",
        );
        let shared = std::sync::Arc::new(Graph::new(&[&base.triples]));
        let g = Graph::overlay(&shared, &over.triples);
        same_as_by_scan(&g);
        let t = Shapes::new(&g);
        let t = t.get(g.iri(ex("T")).unwrap()).unwrap();
        assert_eq!(
            (t.kind, t.targets.len(), t.sparql_targets.len()),
            (ShapeKind::Node, 0, 1)
        );
    }

    #[test]
    fn logical_members_and_cyclic_lists() {
        let p = parsed(
            "ex:S sh:or ( [ sh:datatype ex:d ] ex:Undescribed ) .\n_:l <http://www.w3.org/1999/02/22-rdf-syntax-ns#first> ex:x ; <http://www.w3.org/1999/02/22-rdf-syntax-ns#rest> _:l .\nex:T sh:and _:l .",
        );
        let g = Graph::new(&[&p.triples]);
        let s = Shapes::new(&g);
        // ex:S (no target, but a SHACL subject is not a shape by itself) is
        // not a candidate; the blank member is.
        assert_eq!(s.shapes.len(), 1);
        assert!(matches!(g.term(s.shapes[0].node), Term::Blank(_)));
        let head = g.with_p(vocab::get().and).next().unwrap().o;
        assert_eq!(list(&g, head).len(), 1);
    }

    /// A shape as (node, kind, typed sh:NodeShape, typed sh:PropertyShape,
    /// via_property, via_node, path, targets, params), with triples as
    /// indexes into the document's parse.
    type Summary = (
        String,
        ShapeKind,
        bool,
        bool,
        Option<u32>,
        bool,
        Option<(Path, u32)>,
        Vec<u32>,
        Vec<u32>,
    );

    /// The answers the shape model this one replaced (`shapes::Shapes`,
    /// built straight from the parse) gave for the same document.
    #[test]
    fn same_shapes_as_the_model_it_replaced() {
        let p = parsed(
            "ex:S a sh:NodeShape ; sh:targetClass ex:C ;\n\
               sh:property [ sh:path ex:p ; sh:node ex:N ;\n\
                 sh:qualifiedValueShape [ sh:datatype ex:d ] ; sh:qualifiedMinCount 1 ] ;\n\
               sh:and ( ex:A [ sh:minCount 1 ] ) ;\n\
               sh:or ( [ sh:path [ sh:inversePath ex:q ] ] ex:Undescribed ) ;\n\
               sh:xone ( [ sh:class ex:C ] ) .\n\
             ex:N sh:class ex:K .\n\
             ex:A sh:datatype ex:d .\n\
             ex:P a sh:PropertyShape ; sh:targetNode ex:x ; sh:node ex:N .\n\
             ex:Q sh:path ( ex:a [ sh:zeroOrMorePath ex:b ] ) ; sh:not [ sh:class ex:C ] ; sh:property ex:P .\n\
             ex:R sh:targetSubjectsOf ex:p ; sh:property [ ] .\n",
        );
        let g = Graph::new(&[&p.triples]);
        let doc = |t: u32| {
            let (d, i) = g.origin(t);
            assert_eq!(d, 0);
            i
        };
        let found: Vec<Summary> = Shapes::new(&g)
            .shapes
            .iter()
            .map(|s| {
                let node = match g.term(s.node) {
                    Term::Iri(i) => format!("ex:{}", &i["http://example.com/".len()..]),
                    Term::Blank(b) => format!("_:{b}"),
                    Term::Value(_) => unreachable!("a shape is a node"),
                };
                (
                    node,
                    s.kind,
                    s.typed_node_shape,
                    s.typed_property_shape,
                    s.via_property.map(doc),
                    s.via_node,
                    s.path.as_ref().map(|(p, i)| (p.clone(), doc(*i))),
                    s.targets.iter().copied().map(doc).collect(),
                    s.params.iter().copied().map(doc).collect(),
                )
            })
            .collect();
        use Path::*;
        use ShapeKind::*;
        let pinned: Vec<Summary> = vec![
            (
                "ex:S".into(),
                Node,
                true,
                false,
                None,
                false,
                None,
                vec![1],
                vec![1, 7, 13, 20, 24],
            ),
            (
                "_:1".into(),
                Property,
                false,
                false,
                Some(7),
                false,
                Some((Predicate(ex("p")), 2)),
                vec![],
                vec![2, 3, 5, 6],
            ),
            (
                "ex:N".into(),
                Node,
                false,
                false,
                None,
                true,
                None,
                vec![],
                vec![25],
            ),
            (
                "_:2".into(),
                Node,
                false,
                false,
                None,
                false,
                None,
                vec![],
                vec![4],
            ),
            (
                "ex:A".into(),
                Node,
                false,
                false,
                None,
                false,
                None,
                vec![],
                vec![26],
            ),
            (
                "_:3".into(),
                Node,
                false,
                false,
                None,
                false,
                None,
                vec![],
                vec![8],
            ),
            (
                "_:6".into(),
                Property,
                false,
                false,
                None,
                false,
                Some((Inverse(Box::new(Predicate(ex("q")))), 15)),
                vec![],
                vec![15],
            ),
            (
                "_:10".into(),
                Node,
                false,
                false,
                None,
                false,
                None,
                vec![],
                vec![21],
            ),
            (
                "ex:P".into(),
                Property,
                false,
                true,
                Some(38),
                false,
                None,
                vec![28],
                vec![28, 29],
            ),
            (
                "ex:Q".into(),
                Property,
                false,
                false,
                None,
                false,
                Some((
                    Sequence(vec![
                        Predicate(ex("a")),
                        ZeroOrMore(Box::new(Predicate(ex("b")))),
                    ]),
                    35,
                )),
                vec![],
                vec![35, 37, 38],
            ),
            (
                "_:15".into(),
                Node,
                false,
                false,
                None,
                false,
                None,
                vec![],
                vec![36],
            ),
            (
                "ex:R".into(),
                Node,
                false,
                false,
                None,
                false,
                None,
                vec![39],
                vec![39, 40],
            ),
            (
                "_:16".into(),
                Property,
                false,
                false,
                Some(40),
                false,
                None,
                vec![],
                vec![],
            ),
        ];
        assert_eq!(found, pinned);
    }
}
