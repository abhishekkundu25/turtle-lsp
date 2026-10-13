//! SHACL Core validation over a [`Graph`] that is both the data graph and
//! the shapes graph.
//!
//! Shapes are compiled once from the graph (lazily, as targets and shape
//! parameters reach them), then every shape with a target validates its
//! focus nodes. Covered: all targets (including implicit class targets and
//! the `rdfs:subClassOf` closure), all property paths, and every SHACL Core
//! constraint component; `sh:deactivated`, `sh:severity` and `sh:message`.
//!
//! Choices where the spec leaves room:
//! - a recursive shape reference (the same shape on the same focus node
//!   while it is being checked) conforms, as do nestings deeper than 64;
//! - a shape whose `sh:path` is ill-formed is skipped, as is a
//!   property-shape-only parameter on a node shape;
//! - an implicit class target is also taken from `owl:Class` (common in
//!   ontologies that type their shapes' classes with OWL);
//! - `sh:pattern` uses the `regex` crate (linear time); XPath's `x` flag
//!   drops whitespace outside character classes, `q` quotes the pattern, and
//!   a pattern that does not compile is ignored;
//! - literal comparisons follow [`super::literal`].

use super::Path;
use super::graph::{Graph, Id, Term};
use super::literal;
use crate::syntax::{Iri, RDF, XSD};
use regex::{Regex, RegexBuilder};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::OnceLock;

/// One `sh:ValidationResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidationResult {
    pub focus: Id,
    /// The property shape's path (`sh:resultPath`); for `sh:closed`, the
    /// predicate that is not allowed.
    pub path: Option<Path>,
    pub value: Option<Id>,
    /// `sh:sourceShape`.
    pub shape: Id,
    /// `sh:sourceConstraintComponent`.
    pub component: Iri,
    /// `sh:resultSeverity`: the shape's `sh:severity`, else `sh:Violation`.
    pub severity: Iri,
    /// The shape's `sh:message` values.
    pub messages: Vec<Id>,
    /// The graph triple that states the constraint's parameter.
    pub param: u32,
}

struct V {
    rdf_type: Iri,
    sub_class_of: Iri,
    rdfs_class: Iri,
    owl_class: Iri,
    node_shape: Iri,
    property_shape: Iri,
    target_class: Iri,
    target_node: Iri,
    target_subjects_of: Iri,
    target_objects_of: Iri,
    path: Iri,
    inverse_path: Iri,
    alternative_path: Iri,
    zero_or_more_path: Iri,
    one_or_more_path: Iri,
    zero_or_one_path: Iri,
    rdf_first: Iri,
    rdf_nil: Iri,
    deactivated: Iri,
    severity: Iri,
    violation: Iri,
    message: Iri,
    flags: Iri,
    property: Iri,
    qualified_value_shape: Iri,
    qualified_min_count: Iri,
    qualified_max_count: Iri,
    qualified_disjoint: Iri,
    ignored_properties: Iri,
    xsd_boolean: Iri,
    /// Parameter -> component, for the parameters that need no extra lookup.
    params: FxHashMap<Iri, Param>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Param {
    Class,
    Datatype,
    NodeKind,
    MinCount,
    MaxCount,
    MinExclusive,
    MinInclusive,
    MaxExclusive,
    MaxInclusive,
    MinLength,
    MaxLength,
    Pattern,
    LanguageIn,
    UniqueLang,
    Equals,
    Disjoint,
    LessThan,
    LessThanOrEquals,
    Not,
    And,
    Or,
    Xone,
    Node,
    Property,
    Closed,
    HasValue,
    In,
}

const PARAMS: [(&str, Param); 27] = [
    ("class", Param::Class),
    ("datatype", Param::Datatype),
    ("nodeKind", Param::NodeKind),
    ("minCount", Param::MinCount),
    ("maxCount", Param::MaxCount),
    ("minExclusive", Param::MinExclusive),
    ("minInclusive", Param::MinInclusive),
    ("maxExclusive", Param::MaxExclusive),
    ("maxInclusive", Param::MaxInclusive),
    ("minLength", Param::MinLength),
    ("maxLength", Param::MaxLength),
    ("pattern", Param::Pattern),
    ("languageIn", Param::LanguageIn),
    ("uniqueLang", Param::UniqueLang),
    ("equals", Param::Equals),
    ("disjoint", Param::Disjoint),
    ("lessThan", Param::LessThan),
    ("lessThanOrEquals", Param::LessThanOrEquals),
    ("not", Param::Not),
    ("and", Param::And),
    ("or", Param::Or),
    ("xone", Param::Xone),
    ("node", Param::Node),
    ("property", Param::Property),
    ("closed", Param::Closed),
    ("hasValue", Param::HasValue),
    ("in", Param::In),
];

fn sh(local: &str) -> Iri {
    Iri::intern(&format!("{}{local}", super::SH))
}

fn v() -> &'static V {
    static VOCAB: OnceLock<V> = OnceLock::new();
    VOCAB.get_or_init(|| {
        let rdfs = |l: &str| Iri::intern(&format!("http://www.w3.org/2000/01/rdf-schema#{l}"));
        let rdf = |l: &str| Iri::intern(&format!("{RDF}{l}"));
        V {
            rdf_type: rdf("type"),
            sub_class_of: rdfs("subClassOf"),
            rdfs_class: rdfs("Class"),
            owl_class: Iri::intern("http://www.w3.org/2002/07/owl#Class"),
            node_shape: sh("NodeShape"),
            property_shape: sh("PropertyShape"),
            target_class: sh("targetClass"),
            target_node: sh("targetNode"),
            target_subjects_of: sh("targetSubjectsOf"),
            target_objects_of: sh("targetObjectsOf"),
            path: sh("path"),
            inverse_path: sh("inversePath"),
            alternative_path: sh("alternativePath"),
            zero_or_more_path: sh("zeroOrMorePath"),
            one_or_more_path: sh("oneOrMorePath"),
            zero_or_one_path: sh("zeroOrOnePath"),
            rdf_first: rdf("first"),
            rdf_nil: rdf("nil"),
            deactivated: sh("deactivated"),
            severity: sh("severity"),
            violation: sh("Violation"),
            message: sh("message"),
            flags: sh("flags"),
            property: sh("property"),
            qualified_value_shape: sh("qualifiedValueShape"),
            qualified_min_count: sh("qualifiedMinCount"),
            qualified_max_count: sh("qualifiedMaxCount"),
            qualified_disjoint: sh("qualifiedValueShapesDisjoint"),
            ignored_properties: sh("ignoredProperties"),
            xsd_boolean: Iri::intern(&format!("{XSD}boolean")),
            params: PARAMS.iter().map(|&(l, p)| (sh(l), p)).collect(),
        }
    })
}

/// The `sh:...ConstraintComponent` IRI of a component.
fn component(name: &str) -> Iri {
    sh(&format!("{name}ConstraintComponent"))
}

type ShapeIx = u32;

#[derive(Clone, Copy)]
enum Bound {
    MinExclusive,
    MinInclusive,
    MaxExclusive,
    MaxInclusive,
}

enum C {
    Class(Id),
    Datatype(Iri),
    /// Bits: 1 IRI, 2 blank node, 4 literal.
    NodeKind(u8),
    MinCount(u64),
    MaxCount(u64),
    Range(Bound, Id),
    MinLength(u64),
    MaxLength(u64),
    Pattern(Regex),
    LanguageIn(Vec<String>),
    UniqueLang,
    Equals(Iri),
    Disjoint(Iri),
    LessThan(Iri, bool),
    Not(ShapeIx),
    And(Vec<ShapeIx>),
    Or(Vec<ShapeIx>),
    Xone(Vec<ShapeIx>),
    Node(ShapeIx),
    Property(ShapeIx),
    Qualified {
        shape: ShapeIx,
        siblings: Rc<Vec<ShapeIx>>,
        count: u64,
        max: bool,
    },
    Closed(FxHashSet<Iri>),
    HasValue(Id),
    In(FxHashSet<Id>),
}

impl C {
    fn component(&self) -> Iri {
        component(match self {
            C::Class(_) => "Class",
            C::Datatype(_) => "Datatype",
            C::NodeKind(_) => "NodeKind",
            C::MinCount(_) => "MinCount",
            C::MaxCount(_) => "MaxCount",
            C::Range(Bound::MinExclusive, _) => "MinExclusive",
            C::Range(Bound::MinInclusive, _) => "MinInclusive",
            C::Range(Bound::MaxExclusive, _) => "MaxExclusive",
            C::Range(Bound::MaxInclusive, _) => "MaxInclusive",
            C::MinLength(_) => "MinLength",
            C::MaxLength(_) => "MaxLength",
            C::Pattern(_) => "Pattern",
            C::LanguageIn(_) => "LanguageIn",
            C::UniqueLang => "UniqueLang",
            C::Equals(_) => "Equals",
            C::Disjoint(_) => "Disjoint",
            C::LessThan(_, false) => "LessThan",
            C::LessThan(_, true) => "LessThanOrEquals",
            C::Not(_) => "Not",
            C::And(_) => "And",
            C::Or(_) => "Or",
            C::Xone(_) => "Xone",
            C::Node(_) => "Node",
            C::Property(_) => "Property",
            C::Qualified { max: false, .. } => "QualifiedMinCount",
            C::Qualified { max: true, .. } => "QualifiedMaxCount",
            C::Closed(_) => "Closed",
            C::HasValue(_) => "HasValue",
            C::In(_) => "In",
        })
    }
}

struct Shape {
    node: Id,
    /// `Some` for a property shape.
    path: Option<Path>,
    /// Deactivated, or not well-formed enough to validate.
    skip: bool,
    severity: Iri,
    messages: Vec<Id>,
    constraints: Vec<(C, u32)>,
}

const MAX_DEPTH: usize = 64;

/// The property path rooted at `node`.
pub fn parse_path(g: &Graph, node: Id) -> Path {
    path_at(g, node, 0)
}

fn path_at(g: &Graph, node: Id, depth: usize) -> Path {
    let v = v();
    if depth > MAX_DEPTH {
        return Path::Invalid;
    }
    match g.term(node) {
        Term::Iri(i) if i == v.rdf_nil => Path::Invalid,
        Term::Iri(i) => Path::Predicate(i),
        Term::Blank(_) => {
            let members = |head: Id| -> Vec<Path> {
                g.list(head)
                    .into_iter()
                    .map(|m| path_at(g, m, depth + 1))
                    .collect()
            };
            if g.object(node, v.rdf_first).is_some() {
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

fn well_formed(p: &Path) -> bool {
    match p {
        Path::Predicate(_) => true,
        Path::Invalid => false,
        Path::Sequence(ps) | Path::Alternative(ps) => !ps.is_empty() && ps.iter().all(well_formed),
        Path::Inverse(q) | Path::ZeroOrMore(q) | Path::OneOrMore(q) | Path::ZeroOrOne(q) => {
            well_formed(q)
        }
    }
}

/// XPath regular expression flags (`sh:flags`) applied to `pattern`.
fn regex(pattern: &str, flags: &str) -> Option<Regex> {
    let pat = if flags.contains('q') {
        regex::escape(pattern)
    } else if flags.contains('x') {
        let mut out = String::with_capacity(pattern.len());
        let mut class = 0u32;
        let mut escaped = false;
        for c in pattern.chars() {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '[' => class += 1,
                ']' => class = class.saturating_sub(1),
                ' ' | '\t' | '\n' | '\r' if class == 0 => continue,
                _ => {}
            }
            out.push(c);
        }
        out
    } else {
        pattern.to_string()
    };
    RegexBuilder::new(&pat)
        .case_insensitive(flags.contains('i'))
        .dot_matches_new_line(flags.contains('s'))
        .multi_line(flags.contains('m'))
        .size_limit(1 << 22)
        .build()
        .ok()
}

fn number(g: &Graph, id: Id) -> Option<u64> {
    let (lex, _, _) = g.term(id).literal()?;
    lex.parse().ok()
}

struct Compiler<'g, 'a> {
    g: &'g Graph<'a>,
    shapes: Vec<Shape>,
    index: FxHashMap<Id, ShapeIx>,
}

impl Compiler<'_, '_> {
    fn shape(&mut self, node: Id) -> ShapeIx {
        if let Some(&i) = self.index.get(&node) {
            return i;
        }
        let ix = self.shapes.len() as ShapeIx;
        self.index.insert(node, ix);
        self.shapes.push(Shape {
            node,
            path: None,
            skip: true,
            severity: v().violation,
            messages: Vec::new(),
            constraints: Vec::new(),
        });
        let built = self.build(node);
        self.shapes[ix as usize] = built;
        ix
    }

    fn shapes(&mut self, nodes: Vec<Id>) -> Vec<ShapeIx> {
        nodes.into_iter().map(|n| self.shape(n)).collect()
    }

    /// The literal `true` (the spec names only that one, not `"1"^^xsd:boolean`).
    fn is_true(&self, id: Id) -> bool {
        matches!(self.g.term(id).literal(), Some(("true", dt, _)) if dt == v().xsd_boolean)
    }

    fn build(&mut self, node: Id) -> Shape {
        let g = self.g;
        let v = v();
        let own: Vec<(u32, Iri, Id)> = g.out(node).map(|(i, t)| (i, t.p, t.o)).collect();
        let first = |p: Iri| own.iter().find(|t| t.1 == p).map(|t| t.2);
        let path = first(v.path).map(|p| parse_path(g, p));
        let mut shape = Shape {
            node,
            skip: path.as_ref().is_some_and(|p| !well_formed(p))
                || own
                    .iter()
                    .any(|t| t.1 == v.deactivated && self.is_true(t.2)),
            path,
            severity: first(v.severity)
                .and_then(|s| g.as_iri(s))
                .unwrap_or(v.violation),
            messages: own
                .iter()
                .filter(|t| t.1 == v.message)
                .map(|t| t.2)
                .collect(),
            constraints: Vec::new(),
        };
        if shape.skip {
            return shape;
        }
        let property_shape = shape.path.is_some();
        let flags = first(v.flags)
            .and_then(|f| g.term(f).literal())
            .map_or("", |l| l.0);
        let mut qualified_done = false;
        for &(ti, p, o) in &own {
            let Some(&param) = v.params.get(&p) else {
                if (p == v.qualified_value_shape) && property_shape && !qualified_done {
                    qualified_done = true;
                    self.qualified(node, o, &own, &mut shape);
                }
                continue;
            };
            let iri = || g.as_iri(o);
            let list = || g.list(o);
            let c = match param {
                Param::Class => Some(C::Class(o)),
                Param::Datatype => iri().map(C::Datatype),
                Param::NodeKind => iri().and_then(|k| {
                    let bits = match k.as_str().strip_prefix(super::SH)? {
                        "IRI" => 1,
                        "BlankNode" => 2,
                        "Literal" => 4,
                        "BlankNodeOrIRI" => 3,
                        "BlankNodeOrLiteral" => 6,
                        "IRIOrLiteral" => 5,
                        _ => return None,
                    };
                    Some(C::NodeKind(bits))
                }),
                Param::MinCount if property_shape => number(g, o).map(C::MinCount),
                Param::MaxCount if property_shape => number(g, o).map(C::MaxCount),
                Param::MinExclusive => Some(C::Range(Bound::MinExclusive, o)),
                Param::MinInclusive => Some(C::Range(Bound::MinInclusive, o)),
                Param::MaxExclusive => Some(C::Range(Bound::MaxExclusive, o)),
                Param::MaxInclusive => Some(C::Range(Bound::MaxInclusive, o)),
                Param::MinLength => number(g, o).map(C::MinLength),
                Param::MaxLength => number(g, o).map(C::MaxLength),
                Param::Pattern => g
                    .term(o)
                    .literal()
                    .and_then(|l| regex(l.0, flags))
                    .map(C::Pattern),
                Param::LanguageIn => Some(C::LanguageIn(
                    list()
                        .into_iter()
                        .filter_map(|m| g.term(m).literal().map(|l| l.0.to_ascii_lowercase()))
                        .collect(),
                )),
                Param::UniqueLang if property_shape => self.is_true(o).then_some(C::UniqueLang),
                Param::Equals => iri().map(C::Equals),
                Param::Disjoint => iri().map(C::Disjoint),
                Param::LessThan if property_shape => iri().map(|p| C::LessThan(p, false)),
                Param::LessThanOrEquals if property_shape => iri().map(|p| C::LessThan(p, true)),
                Param::Not => Some(C::Not(self.shape(o))),
                Param::And => Some(C::And(self.shapes(list()))),
                Param::Or => Some(C::Or(self.shapes(list()))),
                Param::Xone => Some(C::Xone(self.shapes(list()))),
                Param::Node => Some(C::Node(self.shape(o))),
                Param::Property => Some(C::Property(self.shape(o))),
                Param::Closed => self.is_true(o).then(|| {
                    let mut allowed: FxHashSet<Iri> = own
                        .iter()
                        .filter(|t| t.1 == v.property)
                        .filter_map(|t| match g.object(t.2, v.path).map(|p| g.term(p)) {
                            Some(Term::Iri(i)) => Some(i),
                            _ => None,
                        })
                        .collect();
                    for t in own.iter().filter(|t| t.1 == v.ignored_properties) {
                        allowed.extend(g.list(t.2).into_iter().filter_map(|m| g.as_iri(m)));
                    }
                    C::Closed(allowed)
                }),
                Param::HasValue => Some(C::HasValue(o)),
                Param::In => Some(C::In(list().into_iter().collect())),
                _ => None,
            };
            if let Some(c) = c {
                shape.constraints.push((c, ti));
            }
        }
        shape
    }

    /// `sh:qualifiedValueShape` with its counts and, when asked, the sibling
    /// shapes its conforming values must not conform to.
    fn qualified(&mut self, node: Id, q: Id, own: &[(u32, Iri, Id)], shape: &mut Shape) {
        let g = self.g;
        let v = v();
        let disjoint = own
            .iter()
            .any(|t| t.1 == v.qualified_disjoint && self.is_true(t.2));
        let mut siblings = Vec::new();
        if disjoint {
            let parents: Vec<Id> = g.subjects(v.property, node).collect();
            for parent in parents {
                let props: Vec<Id> = g.objects(parent, v.property).collect();
                for ps in props {
                    for sib in g.objects(ps, v.qualified_value_shape).collect::<Vec<_>>() {
                        if sib != q && !siblings.contains(&sib) {
                            siblings.push(sib);
                        }
                    }
                }
            }
        }
        let siblings = Rc::new(self.shapes(siblings));
        let qs = self.shape(q);
        for &(ti, p, o) in own {
            let max = if p == v.qualified_min_count {
                false
            } else if p == v.qualified_max_count {
                true
            } else {
                continue;
            };
            if let Some(count) = number(g, o) {
                shape.constraints.push((
                    C::Qualified {
                        shape: qs,
                        siblings: siblings.clone(),
                        count,
                        max,
                    },
                    ti,
                ));
            }
        }
    }
}

struct Run<'r, 'g, 'a> {
    g: &'g Graph<'a>,
    shapes: &'r [Shape],
    stack: Vec<(ShapeIx, Id)>,
    supers: FxHashMap<Id, Rc<FxHashSet<Id>>>,
    conforms: FxHashMap<(ShapeIx, Id), bool>,
}

fn kind_bits(t: Term) -> u8 {
    match t {
        Term::Iri(_) => 1,
        Term::Blank(_) => 2,
        Term::Value(crate::syntax::Node::Literal { .. }) => 4,
        Term::Value(_) => 0,
    }
}

/// `langMatches` basic filtering: `*` matches any tag, else the range or a
/// tag that starts with it followed by `-` (ranges are lower-cased).
fn lang_matches(tag: &str, range: &str) -> bool {
    let tag = tag.to_ascii_lowercase();
    range == "*"
        || tag == range
        || (tag.len() > range.len()
            && tag.starts_with(range)
            && tag.as_bytes()[range.len()] == b'-')
}

fn dedup(v: &mut Vec<Id>) {
    let mut seen = FxHashSet::default();
    v.retain(|x| seen.insert(*x));
}

impl Run<'_, '_, '_> {
    /// The values reached from `x` along `path` (backwards when `inv`).
    fn values(&self, path: &Path, x: Id, inv: bool) -> Vec<Id> {
        let g = self.g;
        let mut out = match path {
            Path::Predicate(p) => {
                if inv {
                    g.subjects(*p, x).collect()
                } else {
                    g.objects(x, *p).collect()
                }
            }
            Path::Inverse(q) => self.values(q, x, !inv),
            Path::Sequence(ps) => {
                let mut cur = vec![x];
                let steps: Box<dyn Iterator<Item = &Path>> = if inv {
                    Box::new(ps.iter().rev())
                } else {
                    Box::new(ps.iter())
                };
                for q in steps {
                    let mut next: Vec<Id> =
                        cur.iter().flat_map(|&y| self.values(q, y, inv)).collect();
                    dedup(&mut next);
                    cur = next;
                }
                cur
            }
            Path::Alternative(ps) => ps.iter().flat_map(|q| self.values(q, x, inv)).collect(),
            Path::ZeroOrMore(q) => self.closure(q, x, inv, true),
            Path::OneOrMore(q) => self.closure(q, x, inv, false),
            Path::ZeroOrOne(q) => std::iter::once(x).chain(self.values(q, x, inv)).collect(),
            Path::Invalid => Vec::new(),
        };
        dedup(&mut out);
        out
    }

    fn closure(&self, q: &Path, x: Id, inv: bool, with_self: bool) -> Vec<Id> {
        let mut seen = FxHashSet::default();
        let mut out = Vec::new();
        if with_self {
            seen.insert(x);
            out.push(x);
        }
        let mut frontier = self.values(q, x, inv);
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for y in frontier {
                if seen.insert(y) {
                    out.push(y);
                    next.extend(self.values(q, y, inv));
                }
            }
            frontier = next;
        }
        out
    }

    /// `c` and its superclasses (rdfs:subClassOf*).
    fn supers(&mut self, c: Id) -> Rc<FxHashSet<Id>> {
        if let Some(s) = self.supers.get(&c) {
            return s.clone();
        }
        let sub = v().sub_class_of;
        let mut seen = FxHashSet::default();
        let mut todo = vec![c];
        while let Some(x) = todo.pop() {
            if seen.insert(x) {
                todo.extend(self.g.objects(x, sub));
            }
        }
        let s = Rc::new(seen);
        self.supers.insert(c, s.clone());
        s
    }

    /// Whether `x` is a SHACL instance of `class`.
    fn is_instance(&mut self, x: Id, class: Id) -> bool {
        let types: Vec<Id> = self.g.objects(x, v().rdf_type).collect();
        types.into_iter().any(|t| self.supers(t).contains(&class))
    }

    fn compare(&self, a: Id, b: Id) -> Option<Ordering> {
        let (la, da, _) = self.g.term(a).literal()?;
        let (lb, db, _) = self.g.term(b).literal()?;
        literal::compare((&da, la), (&db, lb))
    }

    fn text(&self, x: Id) -> Option<&str> {
        match self.g.term(x) {
            Term::Iri(i) => Some(i.as_str()),
            t => t.literal().map(|l| l.0),
        }
    }

    fn conforms(&mut self, si: ShapeIx, focus: Id) -> bool {
        if let Some(&c) = self.conforms.get(&(si, focus)) {
            return c;
        }
        let mut tmp = Vec::new();
        let recursive = self.stack.contains(&(si, focus));
        self.check(si, focus, &mut tmp, false);
        let ok = tmp.is_empty();
        if !recursive {
            self.conforms.insert((si, focus), ok);
        }
        ok
    }

    /// Validate `focus` against shape `si`, adding its results to `out`. When
    /// `collect` is false, stop at the first result.
    fn check(&mut self, si: ShapeIx, focus: Id, out: &mut Vec<ValidationResult>, collect: bool) {
        let shapes = self.shapes;
        let shape = &shapes[si as usize];
        if shape.skip || self.stack.len() > MAX_DEPTH || self.stack.contains(&(si, focus)) {
            return;
        }
        self.stack.push((si, focus));
        let values = match &shape.path {
            None => vec![focus],
            Some(p) => self.values(p, focus, false),
        };
        for (c, param) in &shape.constraints {
            let before = out.len();
            self.constraint(shape, c, *param, focus, &values, out, collect);
            if !collect && out.len() > before {
                break;
            }
        }
        self.stack.pop();
    }

    #[allow(clippy::too_many_arguments)]
    fn constraint(
        &mut self,
        shape: &Shape,
        c: &C,
        param: u32,
        focus: Id,
        values: &[Id],
        out: &mut Vec<ValidationResult>,
        collect: bool,
    ) {
        let g = self.g;
        let emit = |out: &mut Vec<ValidationResult>, value: Option<Id>, path: Option<Path>| {
            out.push(ValidationResult {
                focus,
                path: path.or_else(|| shape.path.clone()),
                value,
                shape: shape.node,
                component: c.component(),
                severity: shape.severity,
                messages: shape.messages.clone(),
                param,
            });
        };
        let each = |out: &mut Vec<ValidationResult>, bad: &mut dyn FnMut(Id) -> bool| {
            for &x in values {
                if bad(x) {
                    emit(out, Some(x), None);
                }
            }
        };
        match c {
            C::Class(class) => {
                for &x in values {
                    if !self.is_instance(x, *class) {
                        emit(out, Some(x), None);
                    }
                }
            }
            C::Datatype(dt) => each(out, &mut |x| {
                !g.term(x)
                    .literal()
                    .is_some_and(|(lex, d, _)| d == *dt && literal::well_formed(&d, lex))
            }),
            C::NodeKind(bits) => each(out, &mut |x| kind_bits(g.term(x)) & bits == 0),
            C::MinCount(n) => {
                if (values.len() as u64) < *n {
                    emit(out, None, None);
                }
            }
            C::MaxCount(n) => {
                if values.len() as u64 > *n {
                    emit(out, None, None);
                }
            }
            C::Range(bound, b) => each(out, &mut |x| {
                let ord = self.compare(x, *b);
                !match bound {
                    Bound::MinExclusive => ord == Some(Ordering::Greater),
                    Bound::MinInclusive => matches!(ord, Some(Ordering::Greater | Ordering::Equal)),
                    Bound::MaxExclusive => ord == Some(Ordering::Less),
                    Bound::MaxInclusive => matches!(ord, Some(Ordering::Less | Ordering::Equal)),
                }
            }),
            C::MinLength(n) => each(out, &mut |x| {
                self.text(x).is_none_or(|s| (s.chars().count() as u64) < *n)
            }),
            C::MaxLength(n) => each(out, &mut |x| {
                self.text(x).is_none_or(|s| s.chars().count() as u64 > *n)
            }),
            C::Pattern(re) => each(out, &mut |x| {
                matches!(g.term(x), Term::Blank(_)) || self.text(x).is_none_or(|s| !re.is_match(s))
            }),
            C::LanguageIn(ranges) => each(out, &mut |x| {
                !g.term(x)
                    .literal()
                    .and_then(|l| l.2)
                    .is_some_and(|tag| ranges.iter().any(|r| lang_matches(&tag, r)))
            }),
            C::UniqueLang => {
                let mut counts: Vec<(String, u32)> = Vec::new();
                for &x in values {
                    if let Some(tag) = g.term(x).literal().and_then(|l| l.2) {
                        let tag = tag.to_ascii_lowercase();
                        match counts.iter_mut().find(|c| c.0 == tag) {
                            Some(c) => c.1 += 1,
                            None => counts.push((tag, 1)),
                        }
                    }
                }
                for _ in counts.iter().filter(|c| c.1 > 1) {
                    emit(out, None, None);
                }
            }
            C::Equals(p) => {
                let others: Vec<Id> = g.objects(focus, *p).collect();
                for &x in values.iter().filter(|x| !others.contains(x)) {
                    emit(out, Some(x), None);
                }
                for &o in others.iter().filter(|o| !values.contains(o)) {
                    emit(out, Some(o), None);
                }
            }
            C::Disjoint(p) => {
                let others: FxHashSet<Id> = g.objects(focus, *p).collect();
                each(out, &mut |x| others.contains(&x));
            }
            C::LessThan(p, or_equal) => {
                let others: Vec<Id> = g.objects(focus, *p).collect();
                for &x in values {
                    for &o in &others {
                        let ord = self.compare(x, o);
                        let ok = ord == Some(Ordering::Less)
                            || (*or_equal && ord == Some(Ordering::Equal));
                        if !ok {
                            emit(out, Some(x), None);
                        }
                    }
                }
            }
            C::Not(s) => {
                for &x in values {
                    if self.conforms(*s, x) {
                        emit(out, Some(x), None);
                    }
                }
            }
            C::And(ss) | C::Or(ss) | C::Xone(ss) => {
                for &x in values {
                    let mut n = 0;
                    for &s in ss {
                        if self.conforms(s, x) {
                            n += 1;
                        } else if matches!(c, C::And(_)) {
                            break;
                        }
                        if matches!(c, C::Or(_)) && n > 0 {
                            break;
                        }
                    }
                    let ok = match c {
                        C::And(_) => n == ss.len(),
                        C::Or(_) => n > 0,
                        _ => n == 1,
                    };
                    if !ok {
                        emit(out, Some(x), None);
                    }
                }
            }
            C::Node(s) => {
                for &x in values {
                    if !self.conforms(*s, x) {
                        emit(out, Some(x), None);
                    }
                }
            }
            C::Property(s) => {
                for &x in values {
                    self.check(*s, x, out, collect);
                    if !collect && !out.is_empty() {
                        return;
                    }
                }
            }
            C::Qualified {
                shape: q,
                siblings,
                count,
                max,
            } => {
                let mut n = 0u64;
                for &x in values {
                    if self.conforms(*q, x) && !siblings.iter().any(|&s| self.conforms(s, x)) {
                        n += 1;
                    }
                }
                if (*max && n > *count) || (!*max && n < *count) {
                    emit(out, None, None);
                }
            }
            C::Closed(allowed) => {
                for &x in values {
                    for (_, t) in g.out(x) {
                        if !allowed.contains(&t.p) {
                            emit(out, Some(t.o), Some(Path::Predicate(t.p)));
                        }
                    }
                }
            }
            C::HasValue(h) => {
                if !values.contains(h) {
                    emit(out, None, None);
                }
            }
            C::In(set) => each(out, &mut |x| !set.contains(&x)),
        }
    }

    /// The focus nodes of shape `node`: its explicit targets, and itself as
    /// a class when it is one.
    fn focus_nodes(&mut self, node: Id) -> Vec<Id> {
        let g = self.g;
        let v = v();
        let mut out = Vec::new();
        let own: Vec<(Iri, Id)> = g.out(node).map(|(_, t)| (t.p, t.o)).collect();
        let is_class = own.iter().any(|&(p, o)| {
            p == v.rdf_type
                && g.as_iri(o)
                    .is_some_and(|c| c == v.rdfs_class || c == v.owl_class)
        }) || {
            let types: Vec<Id> = own
                .iter()
                .filter(|t| t.0 == v.rdf_type)
                .map(|t| t.1)
                .collect();
            let classes = [v.rdfs_class, v.owl_class].map(|c| g.iri(c));
            types.into_iter().any(|t| {
                let s = self.supers(t);
                classes.iter().flatten().any(|c| s.contains(c))
            })
        };
        if is_class {
            out.extend(self.instances(node));
        }
        for (p, o) in own {
            if p == v.target_node {
                out.push(o);
            } else if p == v.target_class {
                out.extend(self.instances(o));
            } else if p == v.target_subjects_of {
                if let Some(pi) = g.as_iri(o) {
                    out.extend(g.with_p(pi).map(|t| t.s));
                }
            } else if p == v.target_objects_of
                && let Some(pi) = g.as_iri(o)
            {
                out.extend(g.with_p(pi).map(|t| t.o));
            }
        }
        dedup(&mut out);
        out
    }

    /// SHACL instances of `class`: typed with it or one of its subclasses.
    fn instances(&self, class: Id) -> Vec<Id> {
        let v = v();
        let mut classes = FxHashSet::default();
        let mut todo = vec![class];
        let mut out = Vec::new();
        while let Some(c) = todo.pop() {
            if classes.insert(c) {
                todo.extend(self.g.subjects(v.sub_class_of, c));
                out.extend(self.g.subjects(v.rdf_type, c));
            }
        }
        out
    }
}

/// Whether the graph declares anything validation would act on: a target,
/// or a shape that is also a class. Costs a few hash lookups.
pub fn has_targets(g: &Graph) -> bool {
    let v = v();
    [
        v.target_class,
        v.target_node,
        v.target_subjects_of,
        v.target_objects_of,
    ]
    .into_iter()
    .any(|p| g.with_p(p).next().is_some())
        || [v.node_shape, v.property_shape]
            .into_iter()
            .filter_map(|s| g.iri(s))
            .any(|s| g.subjects(v.rdf_type, s).next().is_some())
}

/// Validate `g` against the shapes in it. `cancelled` is polled between
/// focus nodes; a run it stops returns `None`.
pub fn validate(g: &Graph, cancelled: &dyn Fn() -> bool) -> Option<Vec<ValidationResult>> {
    let v = v();
    // Shapes with targets, in graph order; shape-typed nodes may be class targets.
    let mut roots: Vec<Id> = Vec::new();
    for p in [
        v.target_class,
        v.target_node,
        v.target_subjects_of,
        v.target_objects_of,
    ] {
        roots.extend(g.with_p(p).map(|t| t.s));
    }
    for s in [v.node_shape, v.property_shape]
        .into_iter()
        .filter_map(|s| g.iri(s))
    {
        roots.extend(g.subjects(v.rdf_type, s));
    }
    dedup(&mut roots);

    let mut comp = Compiler {
        g,
        shapes: Vec::new(),
        index: FxHashMap::default(),
    };
    let roots: Vec<ShapeIx> = roots.into_iter().map(|r| comp.shape(r)).collect();
    let shapes = comp.shapes;
    let mut run = Run {
        g,
        shapes: &shapes,
        stack: Vec::new(),
        supers: FxHashMap::default(),
        conforms: FxHashMap::default(),
    };
    let mut out = Vec::new();
    for si in roots {
        let shape = &shapes[si as usize];
        if shape.skip {
            continue;
        }
        for focus in run.focus_nodes(shape.node) {
            if cancelled() {
                return None;
            }
            run.check(si, focus, &mut out, true);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{PrefixMap, Syntax, parse};

    fn results(body: &str) -> usize {
        let p = parse(
            &format!(
                "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n{body}"
            ),
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        let g = Graph::new(&[&p.triples]);
        validate(&g, &|| false).unwrap().len()
    }

    #[test]
    fn owl_class_is_an_implicit_target() {
        assert_eq!(
            results(
                "ex:C a owl:Class , sh:NodeShape ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\nex:a a ex:C ."
            ),
            1
        );
    }

    #[test]
    fn pattern_flags() {
        let t = |pattern: &str, flags: &str, value: &str| {
            results(&format!(
                "ex:S sh:targetNode {value} ; sh:pattern {pattern:?} ; sh:flags {flags:?} ."
            ))
        };
        assert_eq!(t("^a b$", "x", "\"ab\""), 0);
        assert_eq!(t("[ ]", "x", "\"a b\""), 0);
        assert_eq!(t("a.c", "q", "\"abc\""), 1);
        assert_eq!(t("a.c", "q", "\"a.c\""), 0);
        assert_eq!(t("^ABC$", "i", "\"abc\""), 0);
        // A pattern that does not compile is ignored.
        assert_eq!(t("(", "", "\"x\""), 0);
    }

    #[test]
    fn recursion_and_cancellation() {
        let body = "ex:S sh:targetNode ex:a ; sh:node ex:S ; sh:property [ sh:path ex:p ; sh:node ex:S ] .\nex:a ex:p ex:a .";
        assert_eq!(results(body), 0);
        let p = parse(
            &format!(
                "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n{body}"
            ),
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        let g = Graph::new(&[&p.triples]);
        assert!(validate(&g, &|| true).is_none());
    }
}
