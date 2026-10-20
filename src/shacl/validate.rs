//! SHACL Core validation over a [`Graph`] that is both the data graph and
//! the shapes graph.
//!
//! Shapes are compiled once from the graph (lazily, as targets and shape
//! parameters reach them), then every shape with a target validates its
//! focus nodes. Covered: all targets (including implicit class targets and
//! the `rdfs:subClassOf` closure), all property paths, and every SHACL Core
//! constraint component; `sh:deactivated`, `sh:severity` and `sh:message`.
//! SHACL-SPARQL: SPARQL-based constraints (`sh:sparql`), SPARQL-based
//! constraint components (`sh:ConstraintComponent` with SELECT or ASK
//! validators) and SPARQL-based targets (`sh:target` with `sh:select`), run
//! through [`super::sparql`]. A query that cannot run is a [`Failure`].
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
use super::sparql::{self, Answer, Index, PreBound};
use crate::syntax::{Iri, RDF, XSD};
use regex::{Regex, RegexBuilder};
use rustc_hash::{FxHashMap, FxHashSet};
use spareval::QuerySolution;
use spargebra::Query;
use spargebra::term::{NamedNode, Term as RdfTerm, Variable};
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
    /// The shape's `sh:message` values (for SPARQL, the constraint's or
    /// validator's).
    pub messages: Vec<Id>,
    /// Messages computed for this result: a SPARQL solution's `?message`, or
    /// `messages` with their `{?var}` / `{$var}` templates filled, as (text,
    /// language tag). When not empty, these replace `messages`.
    pub computed_messages: Vec<(String, Option<Iri>)>,
    /// `sh:sourceConstraint`: the SPARQL constraint that produced the result.
    pub constraint: Option<Id>,
    /// The graph triple that states the constraint's parameter.
    pub param: u32,
}

/// A query in the shapes that could not run: it does not parse, breaks the
/// SHACL-SPARQL restrictions, fails to evaluate, or a solution bound
/// `?failure` to true. The SHACL specification calls the whole validation a
/// failure then; the other results are still reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    /// The node whose `sh:select` / `sh:ask` failed.
    pub node: Id,
    /// The graph triple that states the query.
    pub triple: Option<u32>,
    pub message: String,
}

/// What a validation found.
#[derive(Debug, Default)]
pub struct Report {
    pub results: Vec<ValidationResult>,
    pub failures: Vec<Failure>,
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
    target: Iri,
    sparql: Iri,
    select: Iri,
    ask: Iri,
    prefixes: Iri,
    declare: Iri,
    prefix: Iri,
    namespace: Iri,
    owl_imports: Iri,
    constraint_component: Iri,
    parameter: Iri,
    optional: Iri,
    validator: Iri,
    node_validator: Iri,
    property_validator: Iri,
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
            target: sh("target"),
            sparql: sh("sparql"),
            select: sh("select"),
            ask: sh("ask"),
            prefixes: sh("prefixes"),
            declare: sh("declare"),
            prefix: sh("prefix"),
            namespace: sh("namespace"),
            owl_imports: Iri::intern("http://www.w3.org/2002/07/owl#imports"),
            constraint_component: sh("ConstraintComponent"),
            parameter: sh("parameter"),
            optional: sh("optional"),
            validator: sh("validator"),
            node_validator: sh("nodeValidator"),
            property_validator: sh("propertyValidator"),
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
    Sparql(Rc<Check>),
}

/// A SPARQL-based constraint, or a shape's use of a SPARQL-based constraint
/// component, ready to run.
struct Check {
    query: Query,
    ask: bool,
    /// `sh:sourceConstraint` (SPARQL constraints only).
    constraint: Option<Id>,
    component: Iri,
    /// `sh:message` templates.
    messages: Vec<Id>,
    /// Component parameters, pre-bound by name.
    bindings: Vec<(Variable, Id)>,
    /// The node holding the query.
    source: Id,
    /// May run once for many focus nodes ([`sparql::batchable`]).
    batch: bool,
}

/// A SPARQL-based constraint component.
struct Component {
    iri: Iri,
    /// (`sh:path`, variable name, `sh:optional`).
    params: Vec<(Iri, String, bool)>,
    node_validator: Option<Id>,
    property_validator: Option<Id>,
    validator: Option<Id>,
}

const SPARQL_CONSTRAINT: PreBound = PreBound {
    names: &["this", "shapesGraph", "currentShape"],
    required: &["this"],
};

/// The local name of `iri`: after its last `#`, `/` or `:`.
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/', ':']).next().unwrap_or(iri)
}

/// The prefixes `sh:prefixes` of `node` declares (`sh:declare`), following
/// `owl:imports`.
fn prefixes(g: &Graph, node: Id) -> Vec<(String, String)> {
    let v = v();
    let mut out = Vec::new();
    let mut seen = FxHashSet::default();
    let mut todo: Vec<Id> = g.objects(node, v.prefixes).collect();
    while let Some(x) = todo.pop() {
        if !seen.insert(x) {
            continue;
        }
        for d in g.objects(x, v.declare) {
            let text = |p: Iri| {
                g.object(d, p).and_then(|o| match g.term(o) {
                    Term::Iri(i) => Some(i.as_str()),
                    t => t.literal().map(|l| l.0),
                })
            };
            if let (Some(p), Some(ns)) = (text(v.prefix), text(v.namespace)) {
                out.push((p.to_string(), ns.to_string()));
            }
        }
        todo.extend(g.objects(x, v.owl_imports));
    }
    out
}

/// The `sh:select` (or, with `ask`, `sh:ask`) of `node`: (whether ASK, text,
/// triple).
fn query_text<'a>(g: &Graph<'a>, node: Id, ask: bool) -> Option<(bool, &'a str, u32)> {
    let v = v();
    g.out(node).find_map(|(i, t)| {
        let is_ask = t.p == v.ask;
        if t.p != v.select && !(ask && is_ask) {
            return None;
        }
        g.term(t.o).literal().map(|l| (is_ask, l.0, i))
    })
}

/// Prepares the query of `node`, or records why it cannot run.
fn prepare(
    g: &Graph,
    node: Id,
    ask: bool,
    path: Option<&Path>,
    pre: &PreBound,
    failures: &mut Vec<Failure>,
) -> Option<(Query, bool)> {
    let (is_ask, text, triple) = query_text(g, node, ask)?;
    match sparql::prepare(text, &prefixes(g, node), path, pre) {
        Ok(q) => Some((q, is_ask)),
        Err(message) => {
            failures.push(Failure {
                node,
                triple: Some(triple),
                message,
            });
            None
        }
    }
}

/// The SPARQL-based constraint components declared in `g`: instances of
/// `sh:ConstraintComponent` (or a subclass) with an IRI and parameters.
fn components(g: &Graph) -> Vec<Component> {
    let v = v();
    let Some(cc) = g.iri(v.constraint_component) else {
        return Vec::new();
    };
    let mut classes = FxHashSet::default();
    let mut todo = vec![cc];
    let mut nodes = Vec::new();
    while let Some(c) = todo.pop() {
        if classes.insert(c) {
            todo.extend(g.subjects(v.sub_class_of, c));
            nodes.extend(g.subjects(v.rdf_type, c));
        }
    }
    dedup(&mut nodes);
    nodes
        .into_iter()
        .filter_map(|n| {
            let iri = g.as_iri(n)?;
            let params: Vec<(Iri, String, bool)> = g
                .objects(n, v.parameter)
                .filter_map(|p| {
                    let path = g.as_iri(g.object(p, v.path)?)?;
                    let optional = g.objects(p, v.optional).any(|o| {
                        matches!(g.term(o).literal(), Some(("true", dt, _)) if dt == v.xsd_boolean)
                    });
                    Some((path, local_name(path.as_str()).to_string(), optional))
                })
                .collect();
            (!params.is_empty()).then(|| Component {
                iri,
                params,
                node_validator: g.object(n, v.node_validator),
                property_validator: g.object(n, v.property_validator),
                validator: g.object(n, v.validator),
            })
        })
        .collect()
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
            C::Sparql(c) => return c.component,
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

/// Shapes with fewer focus nodes run SPARQL checks one by one: a batched run
/// may evaluate the query over the whole graph before matching the focus
/// nodes, which pays off only for many.
const MIN_BATCH: usize = 16;

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
    /// Shapes reserved but not built yet. Building one only reserves the
    /// shapes it refers to, so a chain of thousands of `sh:node` references
    /// compiles without recursion (and without overflowing the stack).
    pending: Vec<(Id, ShapeIx)>,
    components: Vec<Component>,
    failures: Vec<Failure>,
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
        self.pending.push((node, ix));
        ix
    }

    /// Build every reserved shape, including those found while building.
    fn build_pending(&mut self) {
        while let Some((node, ix)) = self.pending.pop() {
            let built = self.build(node);
            self.shapes[ix as usize] = built;
        }
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
        for &(ti, p, o) in &own {
            if p == v.sparql {
                self.sparql_constraint(&mut shape, ti, o);
            }
        }
        self.component_uses(&mut shape, &own);
        shape
    }

    /// The SPARQL-based constraint `c` of `shape` (stated by triple `ti`).
    fn sparql_constraint(&mut self, shape: &mut Shape, ti: u32, c: Id) {
        let g = self.g;
        let v = v();
        if g.objects(c, v.deactivated).any(|d| self.is_true(d)) {
            return;
        }
        let path = shape.path.as_ref();
        if let Some((query, _)) = prepare(g, c, false, path, &SPARQL_CONSTRAINT, &mut self.failures)
        {
            let check = Check {
                batch: sparql::batchable(&query),
                query,
                ask: false,
                constraint: Some(c),
                component: sh("SPARQLConstraintComponent"),
                messages: g.objects(c, v.message).collect(),
                bindings: Vec::new(),
                source: c,
            };
            shape.constraints.push((C::Sparql(Rc::new(check)), ti));
        }
    }

    /// The SPARQL-based components whose mandatory parameters `shape` has.
    fn component_uses(&mut self, shape: &mut Shape, own: &[(u32, Iri, Id)]) {
        let g = self.g;
        let v = v();
        for comp in &self.components {
            let mut bindings = Vec::new();
            let mut first = None;
            let complete = comp.params.iter().all(|(path, name, optional)| {
                match own.iter().find(|t| t.1 == *path) {
                    Some(&(ti, _, o)) => {
                        first.get_or_insert(ti);
                        bindings.push((Variable::new_unchecked(name.as_str()), o));
                        true
                    }
                    None => *optional,
                }
            });
            let Some(param) = first.filter(|_| complete) else {
                continue;
            };
            let property = shape.path.is_some();
            let specific = if property {
                comp.property_validator
            } else {
                comp.node_validator
            };
            // sh:nodeValidator / sh:propertyValidator take SELECT queries,
            // sh:validator ASK queries.
            let Some((validator, ask)) = specific
                .filter(|&s| query_text(g, s, false).is_some())
                .map(|s| (s, false))
                .or_else(|| comp.validator.map(|s| (s, true)))
            else {
                continue;
            };
            let mut names = vec!["this", "shapesGraph", "currentShape"];
            let mut required = vec!["this"];
            if ask {
                names.push("value");
                required.push("value");
            }
            for (_, name, _) in &comp.params {
                names.push(name);
                required.push(name);
            }
            let pre = PreBound {
                names: &names,
                required: &required,
            };
            let Some((query, is_ask)) = prepare(
                g,
                validator,
                ask,
                shape.path.as_ref(),
                &pre,
                &mut self.failures,
            ) else {
                continue;
            };
            let messages = if shape.messages.is_empty() {
                g.objects(validator, v.message).collect()
            } else {
                shape.messages.clone()
            };
            let check = Check {
                batch: !is_ask && sparql::batchable(&query),
                query,
                ask: is_ask,
                constraint: None,
                component: comp.iri,
                messages,
                bindings,
                source: validator,
            };
            shape.constraints.push((C::Sparql(Rc::new(check)), param));
        }
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
    /// The graph as a SPARQL dataset, when the shapes use SPARQL.
    ix: Option<&'r Index<'g>>,
    ev: sparql::Engine,
    cancelled: &'r dyn Fn() -> bool,
    /// `cancelled` said so between two queries.
    stopped: bool,
    failures: Vec<Failure>,
    /// The focus nodes of the shape being validated: a SPARQL check that
    /// reaches one of them runs once for all of them, when there are at
    /// least `min_batch`.
    min_batch: usize,
    batch: Vec<Id>,
    in_batch: FxHashSet<Id>,
    /// The solutions of such runs per focus node, by shape and check (`None`
    /// when the check runs per focus node).
    batched: FxHashMap<(Id, *const Check), Option<sparql::ByFocus>>,
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
                computed_messages: Vec::new(),
                constraint: None,
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
            C::Sparql(check) => self.sparql(shape, check, param, focus, values, out),
        }
    }

    fn fail(&mut self, node: Id, message: String) {
        let f = Failure {
            node,
            triple: None,
            message,
        };
        if !self.failures.contains(&f) {
            self.failures.push(f);
        }
    }

    /// Runs a SPARQL check on `focus`: a SELECT query once, an ASK query once
    /// per value node.
    fn sparql(
        &mut self,
        shape: &Shape,
        check: &Check,
        param: u32,
        focus: Id,
        values: &[Id],
        out: &mut Vec<ValidationResult>,
    ) {
        let Some(ix) = self.ix else { return };
        let g = self.g;
        let var = Variable::new_unchecked;
        let mut base: Vec<(Variable, RdfTerm)> = vec![
            (var("this"), ix.term(focus)),
            (var("currentShape"), ix.term(shape.node)),
            (
                var("shapesGraph"),
                NamedNode::new_unchecked(sparql::SHAPES_GRAPH).into(),
            ),
        ];
        base.extend(
            check
                .bindings
                .iter()
                .map(|(v, id)| (v.clone(), ix.term(*id))),
        );
        let fill = |lookup: &dyn Fn(&str) -> Option<RdfTerm>| -> Vec<(String, Option<Iri>)> {
            check
                .messages
                .iter()
                .filter_map(|&m| g.term(m).literal())
                .map(|(text, _, lang)| (sparql::fill(text, lookup), lang))
                .collect()
        };
        let result = |value: Option<Id>, path: Option<Path>, computed| ValidationResult {
            focus,
            path,
            value,
            shape: shape.node,
            component: check.component,
            severity: shape.severity,
            messages: check.messages.clone(),
            computed_messages: computed,
            constraint: check.constraint,
            param,
        };
        let asks: Vec<Option<Id>> = if check.ask {
            values.iter().map(|&x| Some(x)).collect()
        } else {
            vec![None]
        };
        for value in asks {
            if (self.cancelled)() {
                self.stopped = true;
                return;
            }
            let mut bindings = base.clone();
            if let Some(x) = value {
                bindings.push((var("value"), ix.term(x)));
            }
            let lookup_base = |n: &str| {
                bindings
                    .iter()
                    .find(|b| b.0.as_str() == n)
                    .map(|b| b.1.clone())
            };
            let answer = match value {
                None => self.batched(ix, shape, check, focus, &base[1..]),
                Some(_) => None,
            };
            let answer = match answer {
                Some(solutions) => Ok(Answer::Solutions(solutions)),
                None => self.ev.run(ix, &check.query, bindings.clone()),
            };
            match answer {
                Err(e) => self.fail(check.source, e),
                Ok(Answer::Boolean(true)) => {}
                Ok(Answer::Boolean(false)) => {
                    out.push(result(value, shape.path.clone(), fill(&lookup_base)));
                }
                Ok(Answer::Solutions(solutions)) => {
                    for sol in solutions {
                        if matches!(sol.get("failure"), Some(RdfTerm::Literal(l)) if l.value() == "true")
                        {
                            self.fail(check.source, "a solution bound ?failure to true".into());
                            continue;
                        }
                        let value = match sol.get("value") {
                            Some(t) => ix.id(t),
                            None => shape.path.is_none().then_some(focus),
                        };
                        let path = match sol.get("path") {
                            Some(RdfTerm::NamedNode(n)) => {
                                Some(Path::Predicate(Iri::intern(n.as_str())))
                            }
                            _ => shape.path.clone(),
                        };
                        let computed = match sol.get("message") {
                            Some(RdfTerm::Literal(l)) => {
                                vec![(l.value().to_string(), l.language().map(Iri::intern))]
                            }
                            _ => fill(&|n: &str| sol.get(n).cloned().or_else(|| lookup_base(n))),
                        };
                        out.push(result(value, path, computed));
                    }
                }
            }
        }
    }

    fn set_batch(&mut self, mut focus: Vec<Id>) {
        dedup(&mut focus);
        self.in_batch = focus.iter().copied().collect();
        self.batch = focus;
        self.batched.clear();
    }

    /// The solutions of `check` for `focus` from one run for the whole
    /// batch (with `constants` pre-bound), when `focus` is in it and the
    /// check can run that way.
    fn batched(
        &mut self,
        ix: &Index,
        shape: &Shape,
        check: &Check,
        focus: Id,
        constants: &[(Variable, RdfTerm)],
    ) -> Option<Vec<QuerySolution>> {
        if !check.batch || self.batch.len() < self.min_batch || !self.in_batch.contains(&focus) {
            return None;
        }
        let key = (shape.node, check as *const Check);
        let found = self.batched.entry(key).or_insert_with(|| {
            match self.ev.run_batch(ix, &check.query, constants, &self.batch) {
                Some(Ok(map)) => Some(map),
                _ => None,
            }
        });
        // Taken: a second check of the same focus node runs on its own.
        found.as_mut()?.remove(&focus)
    }

    /// The focus nodes a SPARQL-based target (`sh:target` with `sh:select`)
    /// selects: the values of `?this`.
    fn sparql_target(&mut self, target: Id, out: &mut Vec<Id>) {
        let Some(ix) = self.ix else { return };
        let g = self.g;
        if g.objects(target, v().deactivated)
            .any(|d| matches!(g.term(d).literal(), Some(("true", _, _))))
        {
            return;
        }
        let pre = PreBound {
            names: &["shapesGraph"],
            required: &[],
        };
        let Some((query, _)) = prepare(g, target, false, None, &pre, &mut self.failures) else {
            return;
        };
        let graph = NamedNode::new_unchecked(sparql::SHAPES_GRAPH);
        match self.ev.run(
            ix,
            &query,
            vec![(Variable::new_unchecked("shapesGraph"), graph.into())],
        ) {
            Ok(Answer::Solutions(sols)) => {
                out.extend(
                    sols.iter()
                        .filter_map(|s| s.get("this").and_then(|t| ix.id(t))),
                );
            }
            Ok(_) => {}
            Err(e) => self.fail(target, e),
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
            } else if p == v.target {
                self.sparql_target(o, &mut out);
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
        v.target,
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
    validate_focus(g, cancelled, &|_| true)
}

/// Like [`validate`], for the focus nodes `keep` accepts only.
pub fn validate_focus(
    g: &Graph,
    cancelled: &dyn Fn() -> bool,
    keep: &dyn Fn(Id) -> bool,
) -> Option<Vec<ValidationResult>> {
    validate_report(g, cancelled, keep).map(|r| r.results)
}

/// Like [`validate_focus`], with the [`Failure`]s of SPARQL queries.
/// `cancelled` is also polled between SPARQL queries.
pub fn validate_report(
    g: &Graph,
    cancelled: &dyn Fn() -> bool,
    keep: &dyn Fn(Id) -> bool,
) -> Option<Report> {
    report_with(g, cancelled, keep, MIN_BATCH)
}

/// [`validate_report`], batching SPARQL checks from `min_batch` focus nodes.
fn report_with(
    g: &Graph,
    cancelled: &dyn Fn() -> bool,
    keep: &dyn Fn(Id) -> bool,
    min_batch: usize,
) -> Option<Report> {
    let v = v();
    // Shapes with targets, in graph order; shape-typed nodes may be class targets.
    let mut roots: Vec<Id> = Vec::new();
    for p in [
        v.target_class,
        v.target_node,
        v.target_subjects_of,
        v.target_objects_of,
        v.target,
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
        pending: Vec::new(),
        components: components(g),
        failures: Vec::new(),
    };
    let roots: Vec<ShapeIx> = roots.into_iter().map(|r| comp.shape(r)).collect();
    comp.build_pending();
    let shapes = comp.shapes;
    let uses_sparql = g.with_p(v.target).next().is_some()
        || shapes
            .iter()
            .any(|s| s.constraints.iter().any(|c| matches!(c.0, C::Sparql(_))));
    let ix = uses_sparql.then(|| Index::new(g));
    let mut run = Run {
        g,
        shapes: &shapes,
        stack: Vec::new(),
        supers: FxHashMap::default(),
        conforms: FxHashMap::default(),
        ix: ix.as_ref(),
        ev: sparql::Engine::default(),
        cancelled,
        stopped: false,
        failures: comp.failures,
        min_batch,
        batch: Vec::new(),
        in_batch: FxHashSet::default(),
        batched: FxHashMap::default(),
    };
    let mut out = Vec::new();
    for si in roots {
        let shape = &shapes[si as usize];
        if shape.skip {
            continue;
        }
        let focus_nodes = run.focus_nodes(shape.node);
        run.set_batch(focus_nodes.iter().copied().filter(|&f| keep(f)).collect());
        for focus in focus_nodes {
            if cancelled() {
                return None;
            }
            if !keep(focus) {
                continue;
            }
            run.check(si, focus, &mut out, true);
            if run.stopped {
                return None;
            }
        }
    }
    let mut failures = run.failures;
    failures.dedup();
    Some(Report {
        results: out,
        failures,
    })
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

    const HEAD: &str =
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n";

    fn report(body: &str, cancelled: &dyn Fn() -> bool) -> Option<(usize, Vec<String>, usize)> {
        let p = parse(
            &format!("{HEAD}{body}"),
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let g = Graph::new(&[&p.triples]);
        let r = validate_report(&g, cancelled, &|_| true)?;
        let messages = r
            .results
            .iter()
            .flat_map(|x| x.computed_messages.iter().map(|m| m.0.clone()))
            .collect();
        Some((r.results.len(), messages, r.failures.len()))
    }

    const RULE: &str = "ex:S sh:targetClass ex:C ; sh:sparql [ sh:message \"{$this} has {?value}\" ; sh:prefixes ex:P ; sh:select \"SELECT $this ?value WHERE { $this ex:p ?value . FILTER (?value > 1) }\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n";

    #[test]
    fn sparql_constraint_with_message_template() {
        let (n, messages, failures) = report(
            &format!("{RULE}ex:a a ex:C ; ex:p 1 , 2 .\nex:b a ex:C ; ex:p 3 ."),
            &|| false,
        )
        .unwrap();
        assert_eq!((n, failures), (2, 0));
        assert!(
            messages.contains(&"http://example.com/a has 2".to_string()),
            "{messages:?}"
        );
    }

    #[test]
    fn deactivated_sparql_constraint() {
        let body = "ex:S sh:targetNode ex:a ; sh:sparql [ sh:deactivated true ; sh:select \"SELECT $this WHERE { }\" ] .";
        assert_eq!(report(body, &|| false).unwrap().0, 0);
    }

    #[test]
    fn sparql_target() {
        let body = "ex:S sh:target [ a sh:SPARQLTarget ; sh:select \"SELECT ?this WHERE { ?this <http://example.com/p> 1 }\" ] ; sh:sparql [ sh:select \"SELECT $this WHERE { }\" ] .\nex:a ex:p 1 .\nex:b ex:p 2 .";
        assert_eq!(report(body, &|| false).unwrap().0, 1);
    }

    #[test]
    fn malformed_and_forbidden_queries_are_failures() {
        for q in [
            "SELECT $this WHERE { $this undeclared:p ?o }",
            "SELECT $this WHERE { VALUES ?x { 1 } }",
        ] {
            let body = format!("ex:S sh:targetNode ex:a ; sh:sparql [ sh:select {q:?} ] .");
            assert_eq!(report(&body, &|| false).unwrap().2, 1, "{q}");
        }
    }

    #[test]
    fn cancelled_between_queries() {
        // Allow the poll before the focus node, stop at the one before the query.
        let calls = std::cell::Cell::new(0);
        let cancelled = || {
            calls.set(calls.get() + 1);
            calls.get() > 1
        };
        let body = format!("{RULE}ex:a a ex:C ; ex:p 2 .");
        assert!(report(&body, &cancelled).is_none());
        assert_eq!(calls.get(), 2);
    }

    /// Blank focus nodes are pre-bound like IRIs: in triple patterns, in
    /// expressions, in EXISTS and as GROUP BY keys.
    #[test]
    fn sparql_on_blank_focus_nodes() {
        let q = |sel: &str| {
            format!(
                "ex:S sh:targetClass ex:C ; sh:sparql [ sh:prefixes ex:P ; sh:select \"{sel}\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n[] a ex:C ; ex:p 2 .\n[] a ex:C ; ex:p 0 .\nex:i a ex:C ; ex:p 5 .\n"
            )
        };
        let n = |sel: &str| report(&q(sel), &|| false).unwrap();
        assert_eq!(
            n("SELECT $this ?value WHERE { $this ex:p ?value . FILTER (?value > 1) }").0,
            2
        );
        assert_eq!(n("SELECT $this WHERE { FILTER (isBlank($this)) }").0, 2);
        assert_eq!(
            n("SELECT $this WHERE { ?x ex:p 2 . FILTER (sameTerm(?x, $this)) }").0,
            1
        );
        assert_eq!(
            n("SELECT $this WHERE { FILTER NOT EXISTS { $this ex:p 0 } }").0,
            2
        );
        assert_eq!(
            n("SELECT $this (COUNT(*) AS ?n) WHERE { $this ex:p ?v } GROUP BY $this").0,
            3
        );
        assert_eq!(
            n("SELECT $this ?n WHERE { { SELECT (COUNT(?v) AS ?n) WHERE { $this ex:q ?v } } }").2,
            1
        );
        assert_eq!(n("SELECT $this ?n WHERE { { SELECT $this (COUNT(?v) AS ?n) WHERE { $this ex:q ?v } GROUP BY $this } }").0, 0);
    }

    /// A NOT EXISTS over a subclass closure, the shape of a type check.
    #[test]
    fn sparql_not_exists_over_subclasses() {
        let body = "ex:S sh:targetClass ex:Thing ; sh:sparql [ sh:prefixes ex:P ; sh:select \"SELECT $this ?value WHERE { $this ex:kind ?value . FILTER NOT EXISTS { $this a/<http://www.w3.org/2000/01/rdf-schema#subClassOf>* ?value } }\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\nex:B <http://www.w3.org/2000/01/rdf-schema#subClassOf> ex:A .\nex:x a ex:Thing , ex:B ; ex:kind ex:A .\nex:y a ex:Thing ; ex:kind ex:A .\n[] a ex:Thing , ex:B ; ex:kind ex:A , ex:B , ex:Thing , ex:Z .";
        assert_eq!(report(body, &|| false).unwrap().0, 2);
    }

    /// Queries cover the forms batching joins into (filters, OPTIONAL,
    /// UNION, EXISTS, GROUP BY, subqueries, blank focus nodes) and ones
    /// that run one by one (LIMIT).
    #[test]
    fn batched_sparql_matches_one_by_one() {
        let mut data = String::new();
        for i in 0..30 {
            data.push_str(&format!(
                "ex:n{i} a ex:C ; ex:p {} ; ex:q ex:n{} .\n[] a ex:C ; ex:p {} ; ex:q ex:n{i} .\n",
                i % 4,
                (i + 1) % 30,
                i % 3
            ));
        }
        for sel in [
            "SELECT $this ?value WHERE { $this ex:p ?value . FILTER (?value > 1) }",
            "SELECT $this WHERE { FILTER (isBlank($this)) }",
            "SELECT $this WHERE { ?x ex:p 2 . FILTER (sameTerm(?x, $this)) }",
            "SELECT $this WHERE { FILTER NOT EXISTS { $this ex:p 0 } }",
            "SELECT $this (COUNT(*) AS ?n) WHERE { $this ex:p ?v } GROUP BY $this",
            "SELECT $this ?n WHERE { { SELECT $this (COUNT(?v) AS ?n) WHERE { ?v ex:q $this } GROUP BY $this } }",
            "SELECT $this ?value WHERE { ?value ex:p 1 OPTIONAL { $this ex:q ?value } }",
            "SELECT $this ?value WHERE { $this ex:q ?value } LIMIT 1",
            "SELECT $this ?value WHERE { { SELECT $this ?value WHERE { ?value ex:q $this } LIMIT 3 } }",
            "SELECT $this ?value WHERE { { $this ex:p ?value } UNION { ?value ex:q $this } FILTER (?value != 3) }",
            "SELECT DISTINCT $this ?value WHERE { $this ex:q/ex:p ?value . FILTER EXISTS { ?y ex:q $this } }",
            "SELECT $this ?value WHERE { { ?value ex:p ?w FILTER (sameTerm(?value, $this)) } UNION { $this ex:p ?value } }",
            "SELECT $this ?value WHERE { $this ex:p ?value OPTIONAL { ?z ex:q $this FILTER (?value > 1) } FILTER (!BOUND(?z)) }",
            "SELECT $this WHERE { $currentShape sh:targetClass ?c . $this a ?c . FILTER NOT EXISTS { $this ex:q ?z . ?z ex:p 3 } }",
        ] {
            let p = parse(
                &format!(
                    "{HEAD}ex:S sh:targetClass ex:C ; sh:sparql [ sh:prefixes ex:P ; sh:message \"{{?value}}\" ; sh:select \"{sel}\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] , [ sh:prefix \"sh\" ; sh:namespace \"http://www.w3.org/ns/shacl#\" ] .\n{data}"
                ),
                &PrefixMap::default(),
                Syntax::Turtle,
            );
            assert!(p.errors.is_empty(), "{:?}", p.errors);
            let g = Graph::new(&[&p.triples]);
            let run = |min_batch| {
                let r = report_with(&g, &|| false, &|_| true, min_batch).unwrap();
                let mut out: Vec<String> = r.results.iter().map(|x| format!("{x:?}")).collect();
                out.extend(r.failures.iter().map(|x| format!("{x:?}")));
                out.sort();
                out
            };
            let one_by_one = run(usize::MAX);
            assert!(!one_by_one.is_empty(), "{sel}");
            assert_eq!(run(1), one_by_one, "{sel}");
        }
    }

    /// One SPARQL constraint on 10,000 focus nodes (run with --nocapture
    /// for the time).
    #[test]
    fn sparql_on_ten_thousand_focus_nodes() {
        let mut body = RULE.to_string();
        for i in 0..10_000 {
            body.push_str(&format!("ex:n{i} a ex:C ; ex:p {} .\n", i % 3));
        }
        let start = std::time::Instant::now();
        let (n, _, failures) = report(&body, &|| false).unwrap();
        let took = start.elapsed();
        println!("10,000 focus nodes, one SPARQL constraint: {took:?}");
        assert_eq!((n, failures), (3333, 0));
        if !cfg!(debug_assertions) {
            // About 30 ms when the query runs once for all focus nodes.
            assert!(took.as_secs_f64() < 0.5, "{took:?}");
        }
    }
}
