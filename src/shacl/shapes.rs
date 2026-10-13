//! The SHACL shapes in one set of triples: node shapes, property shapes
//! (including blank-node shapes reached through `sh:property`), their paths,
//! targets and parameters. Shapes refer to their triples by index, so callers
//! keep every span (`p_span`, `o_span`) the parser recorded.
//!
//! A node counts as a shape when it is typed `sh:NodeShape` /
//! `sh:PropertyShape`, has a target or `sh:path`, or is the value of a
//! shape-expecting parameter (`sh:property`, `sh:node`,
//! `sh:qualifiedValueShape`, `sh:not`, members of `sh:and` / `sh:or` /
//! `sh:xone`). IRIs count only when these triples describe them; blank nodes
//! always (`sh:property [ ]` is an empty property shape).

use crate::syntax::{self, BlankId, Iri, Node, Triple};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::OnceLock;

/// The IRIs the shape model and lint compare against, interned once.
pub struct Vocab {
    pub rdf_type: Iri,
    pub rdf_first: Iri,
    pub rdf_rest: Iri,
    pub rdf_nil: Iri,
    pub rdfs_class: Iri,
    pub rdfs_datatype: Iri,
    pub owl_class: Iri,
    pub node_shape: Iri,
    pub property_shape: Iri,
    pub target_class: Iri,
    pub target_node: Iri,
    pub target_subjects_of: Iri,
    pub target_objects_of: Iri,
    pub property: Iri,
    pub node: Iri,
    pub qualified_value_shape: Iri,
    pub not: Iri,
    pub and: Iri,
    pub or: Iri,
    pub xone: Iri,
    pub path: Iri,
    pub inverse_path: Iri,
    pub alternative_path: Iri,
    pub zero_or_more_path: Iri,
    pub one_or_more_path: Iri,
    pub zero_or_one_path: Iri,
    pub min_count: Iri,
    pub max_count: Iri,
    pub qualified_min_count: Iri,
    pub qualified_max_count: Iri,
    pub class: Iri,
    pub datatype: Iri,
    pub unique_lang: Iri,
    pub less_than: Iri,
    pub less_than_or_equals: Iri,
}

pub fn vocab() -> &'static Vocab {
    static V: OnceLock<Vocab> = OnceLock::new();
    V.get_or_init(|| {
        let sh = |l: &str| Iri::intern(&format!("{}{l}", super::SH));
        let rdf = |l: &str| Iri::intern(&format!("{}{l}", syntax::RDF));
        Vocab {
            rdf_type: Iri::intern(syntax::RDF_TYPE),
            rdf_first: rdf("first"),
            rdf_rest: rdf("rest"),
            rdf_nil: rdf("nil"),
            rdfs_class: Iri::intern("http://www.w3.org/2000/01/rdf-schema#Class"),
            rdfs_datatype: Iri::intern("http://www.w3.org/2000/01/rdf-schema#Datatype"),
            owl_class: Iri::intern("http://www.w3.org/2002/07/owl#Class"),
            node_shape: sh("NodeShape"),
            property_shape: sh("PropertyShape"),
            target_class: sh("targetClass"),
            target_node: sh("targetNode"),
            target_subjects_of: sh("targetSubjectsOf"),
            target_objects_of: sh("targetObjectsOf"),
            property: sh("property"),
            node: sh("node"),
            qualified_value_shape: sh("qualifiedValueShape"),
            not: sh("not"),
            and: sh("and"),
            or: sh("or"),
            xone: sh("xone"),
            path: sh("path"),
            inverse_path: sh("inversePath"),
            alternative_path: sh("alternativePath"),
            zero_or_more_path: sh("zeroOrMorePath"),
            one_or_more_path: sh("oneOrMorePath"),
            zero_or_one_path: sh("zeroOrOnePath"),
            min_count: sh("minCount"),
            max_count: sh("maxCount"),
            qualified_min_count: sh("qualifiedMinCount"),
            qualified_max_count: sh("qualifiedMaxCount"),
            class: sh("class"),
            datatype: sh("datatype"),
            unique_lang: sh("uniqueLang"),
            less_than: sh("lessThan"),
            less_than_or_equals: sh("lessThanOrEquals"),
        }
    })
}

impl Vocab {
    pub fn is_target(&self, p: Iri) -> bool {
        p == self.target_class
            || p == self.target_node
            || p == self.target_subjects_of
            || p == self.target_objects_of
    }
}

/// A node that can be a subject: an IRI or a blank node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Term {
    Iri(Iri),
    Blank(BlankId),
}

impl Term {
    pub fn of(n: &Node) -> Option<Term> {
        match n {
            Node::Iri(i) => Some(Term::Iri(*i)),
            Node::Blank(b) => Some(Term::Blank(*b)),
            _ => None,
        }
    }
}

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

#[derive(Clone, Debug)]
pub struct Shape {
    pub node: Term,
    pub kind: ShapeKind,
    pub typed_node_shape: bool,
    pub typed_property_shape: bool,
    /// The first `sh:property` triple with this shape as value.
    pub via_property: Option<u32>,
    /// The value of some `sh:node` (so a node shape).
    pub via_node: bool,
    /// The parsed `sh:path` and its triple.
    pub path: Option<(Path, u32)>,
    /// `sh:target*` triples.
    pub targets: Vec<u32>,
    /// Every triple with this shape as subject and a predicate in the SHACL
    /// namespace (constraint parameters, path, targets), in text order.
    pub params: Vec<u32>,
}

#[derive(Default)]
pub struct Shapes {
    pub shapes: Vec<Shape>,
    index: FxHashMap<Term, u32>,
    /// Subject -> its triples, for every subject (empty when there are no shapes).
    by_subject: FxHashMap<Term, Vec<u32>>,
}

const MAX_DEPTH: u32 = 64;

impl Shapes {
    /// The shapes in `triples`. Costs one scan when no predicate or type is
    /// in the SHACL namespace.
    pub fn new(triples: &[Triple]) -> Shapes {
        let v = vocab();
        let shacl = |i: &Iri| i.starts_with(super::SH);
        if !triples
            .iter()
            .any(|t| shacl(&t.p) || matches!(&t.o, Node::Iri(o) if t.p == v.rdf_type && shacl(o)))
        {
            return Shapes::default();
        }
        let mut s = Shapes::default();
        for (i, t) in triples.iter().enumerate() {
            if let Some(subj) = Term::of(&t.s) {
                s.by_subject.entry(subj).or_default().push(i as u32);
            }
        }

        // Candidates in text order, with what made them shapes.
        let mut order: Vec<Term> = Vec::new();
        let mut seen: FxHashSet<Term> = FxHashSet::default();
        let mut via_property: FxHashMap<Term, u32> = FxHashMap::default();
        let mut via_node: FxHashSet<Term> = FxHashSet::default();
        let described =
            |t: Term, s: &Shapes| matches!(t, Term::Blank(_)) || s.by_subject.contains_key(&t);
        let mut add = |t: Term, order: &mut Vec<Term>| {
            if seen.insert(t) {
                order.push(t);
            }
        };
        for (i, t) in triples.iter().enumerate() {
            let (Some(subj), p) = (Term::of(&t.s), t.p) else {
                continue;
            };
            if p == v.rdf_type {
                if matches!(&t.o, Node::Iri(o) if *o == v.node_shape || *o == v.property_shape) {
                    add(subj, &mut order);
                }
                continue;
            }
            if !shacl(&p) {
                continue;
            }
            if v.is_target(p) || p == v.path {
                add(subj, &mut order);
            }
            let value = Term::of(&t.o).filter(|&o| described(o, &s));
            if p == v.property || p == v.node || p == v.qualified_value_shape || p == v.not {
                if let Some(o) = value {
                    add(o, &mut order);
                    if p == v.property {
                        via_property.entry(o).or_insert(i as u32);
                    } else if p == v.node {
                        via_node.insert(o);
                    }
                }
            } else if p == v.and || p == v.or || p == v.xone {
                for m in s.list(triples, &t.o) {
                    if let Some(o) = Term::of(&triples[m as usize].o).filter(|&o| described(o, &s))
                    {
                        add(o, &mut order);
                    }
                }
            }
        }

        for node in order {
            let own = s.triples_of(node);
            let mut shape = Shape {
                node,
                kind: ShapeKind::Node,
                typed_node_shape: false,
                typed_property_shape: false,
                via_property: via_property.get(&node).copied(),
                via_node: via_node.contains(&node),
                path: None,
                targets: Vec::new(),
                params: Vec::new(),
            };
            for &i in own {
                let t = &triples[i as usize];
                if t.p == v.rdf_type {
                    if let Node::Iri(o) = &t.o {
                        shape.typed_node_shape |= *o == v.node_shape;
                        shape.typed_property_shape |= *o == v.property_shape;
                    }
                    continue;
                }
                if !shacl(&t.p) {
                    continue;
                }
                shape.params.push(i);
                if t.p == v.path && shape.path.is_none() {
                    shape.path = Some((s.path(triples, &t.o), i));
                } else if v.is_target(t.p) {
                    shape.targets.push(i);
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

    pub fn get(&self, node: Term) -> Option<&Shape> {
        self.index.get(&node).map(|&i| &self.shapes[i as usize])
    }

    /// Indexes of the triples with `node` as subject.
    pub fn triples_of(&self, node: Term) -> &[u32] {
        self.by_subject.get(&node).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The `rdf:first` triples of the RDF collection starting at `head`, in
    /// order (cycle-safe; stops at the first malformed cell).
    pub fn list(&self, triples: &[Triple], head: &Node) -> Vec<u32> {
        let v = vocab();
        let mut out = Vec::new();
        let mut visited = FxHashSet::default();
        let mut cur = head.clone();
        while let Some(t) = Term::of(&cur) {
            if t == Term::Iri(v.rdf_nil) || !visited.insert(t) {
                break;
            }
            let mut next = None;
            let mut first = None;
            for &i in self.triples_of(t) {
                let tr = &triples[i as usize];
                if tr.p == v.rdf_first && first.is_none() {
                    first = Some(i);
                } else if tr.p == v.rdf_rest && next.is_none() {
                    next = Some(tr.o.clone());
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

    /// The property path whose root is `node`.
    pub fn path(&self, triples: &[Triple], node: &Node) -> Path {
        self.path_at(triples, node, 0)
    }

    fn path_at(&self, triples: &[Triple], node: &Node, depth: u32) -> Path {
        let v = vocab();
        if depth > MAX_DEPTH {
            return Path::Invalid;
        }
        match node {
            Node::Iri(i) if *i == v.rdf_nil => Path::Invalid,
            Node::Iri(i) => Path::Predicate(*i),
            Node::Blank(b) => {
                let own = self.triples_of(Term::Blank(*b));
                let members = |s: &Self, head: &Node| -> Vec<Path> {
                    s.list(triples, head)
                        .into_iter()
                        .map(|m| s.path_at(triples, &triples[m as usize].o, depth + 1))
                        .collect()
                };
                if own.iter().any(|&i| triples[i as usize].p == v.rdf_first) {
                    return Path::Sequence(members(self, node));
                }
                for &i in own {
                    let t = &triples[i as usize];
                    let inner = || Box::new(self.path_at(triples, &t.o, depth + 1));
                    if t.p == v.inverse_path {
                        return Path::Inverse(inner());
                    } else if t.p == v.alternative_path {
                        return Path::Alternative(members(self, &t.o));
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
            _ => Path::Invalid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{PrefixMap, parse};

    const HEAD: &str =
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n";

    fn shapes(body: &str) -> (Vec<Triple>, Shapes) {
        let p = parse(
            &format!("{HEAD}{body}"),
            &PrefixMap::default(),
            crate::syntax::Syntax::Turtle,
        );
        let s = Shapes::new(&p.triples);
        (p.triples, s)
    }

    fn ex(l: &str) -> Iri {
        Iri::intern(&format!("http://example.com/{l}"))
    }

    #[test]
    fn no_shacl_no_work() {
        let (_, s) = shapes("ex:a ex:b ex:c .");
        assert!(s.shapes.is_empty());
    }

    #[test]
    fn node_and_blank_property_shapes() {
        let (_, s) = shapes(
            "ex:S a sh:NodeShape ; sh:targetClass ex:C ;\n  sh:property [ sh:path ex:p ; sh:minCount 1 ] , [ sh:minCount 2 ] .",
        );
        assert_eq!(s.shapes.len(), 3);
        let n = s.get(Term::Iri(ex("S"))).unwrap();
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
        let (_, s) = shapes(
            "ex:S sh:property [ sh:path ( ex:a [ sh:inversePath ex:b ] [ sh:alternativePath ( ex:c [ sh:zeroOrMorePath ex:d ] ) ] [ sh:oneOrMorePath ex:e ] [ sh:zeroOrOnePath ex:f ] ) ] .",
        );
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

    #[test]
    fn logical_members_and_cyclic_lists() {
        let (t, s) = shapes(
            "ex:S sh:or ( [ sh:datatype ex:d ] ex:Undescribed ) .\n_:l <http://www.w3.org/1999/02/22-rdf-syntax-ns#first> ex:x ; <http://www.w3.org/1999/02/22-rdf-syntax-ns#rest> _:l .\nex:T sh:and _:l .",
        );
        // ex:S (no target, but a SHACL subject is not a shape by itself) is
        // not a candidate; the blank member is.
        assert_eq!(s.shapes.len(), 1);
        assert!(matches!(s.shapes[0].node, Term::Blank(_)));
        let head = t.iter().find(|x| x.p == vocab().and).unwrap().o.clone();
        assert_eq!(s.list(&t, &head).len(), 1);
    }
}
