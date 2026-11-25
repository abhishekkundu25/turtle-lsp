//! SHACL Core validation over a [`Graph`] that is both the data graph and
//! the shapes graph.
//!
//! The shape model ([`super::model`]) says which nodes are shapes, with
//! their paths, targets and parameters; its shapes are compiled into
//! constraints once (lazily, as targets and shape parameters reach them),
//! then every shape with a target validates its focus nodes. Covered: all targets (including implicit class targets and
//! the `rdfs:subClassOf` closure), all property paths, and every SHACL Core
//! constraint component; `sh:deactivated`, `sh:severity` and `sh:message`.
//! SHACL-SPARQL: SPARQL-based constraints (`sh:sparql`), SPARQL-based
//! constraint components (`sh:ConstraintComponent` with SELECT or ASK
//! validators) and SPARQL-based targets (`sh:target` with `sh:select`), run
//! through [`super::sparql`]. A query that cannot run is a [`Failure`].
//!
//! Choices where the spec leaves room:
//! - a recursive shape reference (the same shape on the same focus node
//!   while it is being checked) conforms; a nesting deeper than 64 shapes is
//!   cut short and reported as a [`Failure`], and what it cut is never cached;
//! - a shape whose `sh:path` is ill-formed (including one nested deeper than
//!   64 levels) is skipped with a [`Failure`] that is an error, as is a
//!   property-shape-only parameter on a node shape;
//! - a validation has a [`Budget`]: past its result cap it stops with
//!   [`Report::capped`] set, past its steps (of path evaluation, and one
//!   per shape check) with a [`Failure`];
//! - an implicit class target is also taken from `owl:Class` (common in
//!   ontologies that type their shapes' classes with OWL);
//! - `sh:pattern` uses the `regex` crate (linear time); XPath's `x` flag
//!   drops whitespace outside character classes, `q` quotes the pattern, and
//!   a pattern that does not compile is ignored;
//! - literal comparisons follow [`super::literal`].

use super::literal;
use super::model::{self, Path, Shapes};
use super::sparql::{self, Answer, Cancel, Fail, Role};
use super::vocab::{self, Param};
use crate::rdf::Index;
use crate::rdf::graph::{Graph, Id, Term};
use crate::syntax::Iri;
use regex::{Regex, RegexBuilder};
use rustc_hash::{FxHashMap, FxHashSet};
use spareval::QuerySolution;
use spargebra::Query;
use spargebra::term::{NamedNode, Term as RdfTerm, Variable};
use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

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
    pub(crate) param: u32,
}

/// A query in the shapes that could not run: it does not parse, breaks the
/// SHACL-SPARQL restrictions, fails to evaluate, or a solution bound
/// `?failure` to true. The SHACL specification calls the whole validation a
/// failure then; the other results are still reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    /// The node whose `sh:select` / `sh:ask` failed, or the shape that was
    /// cut short or skipped.
    pub(crate) node: Id,
    /// The graph triple that states the query (or the `sh:path`).
    pub(crate) triple: Option<u32>,
    pub message: String,
    pub(crate) kind: FailureKind,
}

/// What a [`Failure`] is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// A query that could not run to the end.
    Query,
    /// A shape cut short: nested too deep, or past the step budget.
    Cut,
    /// A shape skipped for its `sh:path`, which is not a well-formed path.
    Path,
}

/// What a validation found.
#[derive(Debug, Default)]
pub struct Report {
    pub results: Vec<ValidationResult>,
    pub failures: Vec<Failure>,
    /// Validation stopped at [`Budget::results`]: there are more results.
    pub(crate) capped: bool,
}

fn sh(local: &str) -> Iri {
    Iri::intern(&format!("{}{local}", super::SH))
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

/// The local name of `iri`: after its last `#`, `/` or `:`.
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/', ':']).next().unwrap_or(iri)
}

/// The prefixes `sh:prefixes` of `node` declares (`sh:declare`), following
/// `owl:imports`.
fn prefixes(g: &Graph, node: Id) -> Vec<(String, String)> {
    let v = vocab::get();
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
    let v = vocab::get();
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
    role: &Role,
    failures: &mut Vec<Failure>,
) -> Option<(Query, bool)> {
    let (is_ask, text, triple) = query_text(g, node, ask)?;
    match sparql::prepare(text, &prefixes(g, node), path, role) {
        Ok(q) => Some((q, is_ask)),
        Err(message) => {
            failures.push(Failure {
                node,
                triple: Some(triple),
                message,
                kind: FailureKind::Query,
            });
            None
        }
    }
}

/// The SPARQL-based constraint components declared in `g`: instances of
/// `sh:ConstraintComponent` (or a subclass) with an IRI and parameters.
fn components(g: &Graph) -> Vec<Component> {
    let v = vocab::get();
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

impl Shape {
    /// A node shape with nothing to check.
    fn empty(node: Id, skip: bool) -> Shape {
        Shape {
            node,
            path: None,
            skip,
            severity: vocab::get().violation,
            messages: Vec::new(),
            constraints: Vec::new(),
        }
    }
}

const MAX_DEPTH: usize = 64;

/// Shapes with fewer focus nodes run SPARQL checks one by one: a batched run
/// may evaluate the query over the whole graph before matching the focus
/// nodes, which pays off only for many.
const MIN_BATCH: usize = 16;

/// Nor do shapes with fewer than one focus node per this many triples: a
/// query the optimizer would start from `$this` can, batched, start from a
/// pattern over the whole graph (20 focus nodes in 300,000
/// triples ran 50x slower batched).
const TRIPLES_PER_BATCHED_FOCUS: usize = 1000;

/// Too few focus nodes for the graph's size to run SPARQL checks batched.
fn sparse(focus: usize, triples: usize) -> bool {
    focus.saturating_mul(TRIPLES_PER_BATCHED_FOCUS) < triples
}

/// The property path rooted at `node`: the shape model's ([`model::path`]),
/// with nested repetitions merged ([`merged`]).
#[cfg(test)]
pub(crate) fn parse_path(g: &Graph, node: Id) -> Path {
    merged(model::path(g, node))
}

/// `p` with every repetition directly inside another merged into one (see
/// [`repeated`]), from the innermost out.
fn merged(p: Path) -> Path {
    let each = |ps: Vec<Path>| -> Vec<Path> { ps.into_iter().map(merged).collect() };
    match p {
        Path::Inverse(q) => Path::Inverse(Box::new(merged(*q))),
        Path::Sequence(ps) => Path::Sequence(each(ps)),
        Path::Alternative(ps) => Path::Alternative(each(ps)),
        Path::ZeroOrMore(q) => repeated(Repeat::Star, merged(*q)),
        Path::OneOrMore(q) => repeated(Repeat::Plus, merged(*q)),
        Path::ZeroOrOne(q) => repeated(Repeat::Optional, merged(*q)),
        p @ (Path::Predicate(_) | Path::Invalid) => p,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Repeat {
    Star,
    Plus,
    Optional,
}

/// `inner` repeated, with a repetition directly inside another merged into
/// one: `(p*)*`, `(p+)*`, `(p?)*`, `(p*)+`, `(p?)+`, `(p+)?` and `(p*)?` are
/// all `p*`, and doubled `+` or `?` is the same `+` or `?`. Nested closures
/// would otherwise cost the size of the graph to the power of the nesting.
fn repeated(outer: Repeat, inner: Path) -> Path {
    let merge = |inner: Repeat| if inner == outer { inner } else { Repeat::Star };
    match inner {
        Path::ZeroOrMore(q) => wrap(Repeat::Star, *q),
        Path::OneOrMore(q) => wrap(merge(Repeat::Plus), *q),
        Path::ZeroOrOne(q) => wrap(merge(Repeat::Optional), *q),
        other => wrap(outer, other),
    }
}

fn wrap(kind: Repeat, p: Path) -> Path {
    let p = Box::new(p);
    match kind {
        Repeat::Star => Path::ZeroOrMore(p),
        Repeat::Plus => Path::OneOrMore(p),
        Repeat::Optional => Path::ZeroOrOne(p),
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

/// Compiles the shapes of a [`Shapes`] model into constraints, lazily: a
/// shape is compiled once a root or a compiled shape refers to it.
struct Compiler<'g, 'a> {
    g: &'g Graph<'a>,
    model: &'g Shapes,
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
        self.shapes.push(Shape::empty(node, true));
        self.pending.push((node, ix));
        ix
    }

    /// Build every reserved shape, including those found while building. A
    /// node the model has no shape for (a literal, an IRI with no triples)
    /// has nothing to check: every value conforms to it.
    fn build_pending(&mut self) {
        let model = self.model;
        while let Some((node, ix)) = self.pending.pop() {
            let built = match model.get(node) {
                Some(s) => self.build(s),
                None => Shape::empty(node, false),
            };
            self.shapes[ix as usize] = built;
        }
    }

    fn shapes(&mut self, nodes: Vec<Id>) -> Vec<ShapeIx> {
        nodes.into_iter().map(|n| self.shape(n)).collect()
    }

    /// The literal `true` (the spec names only that one, not `"1"^^xsd:boolean`).
    fn is_true(&self, id: Id) -> bool {
        matches!(self.g.term(id).literal(), Some(("true", dt, _)) if dt == vocab::get().xsd_boolean)
    }

    /// The constraints of model shape `s`.
    fn build(&mut self, s: &model::Shape) -> Shape {
        let g = self.g;
        let model = self.model;
        let v = vocab::get();
        let node = s.node;
        // Its parameters (every SHACL triple it is the subject of), in order.
        let own: Vec<(u32, Iri, Id)> = s
            .params
            .iter()
            .map(|&i| {
                let t = g.triple_at(i);
                (i, t.p, t.o)
            })
            .collect();
        let first = |p: Iri| own.iter().find(|t| t.1 == p).map(|t| t.2);
        let path = s.path.as_ref().map(|(p, _)| merged(p.clone()));
        let deactivated = own
            .iter()
            .any(|t| t.1 == v.deactivated && self.is_true(t.2));
        let ill_formed = path.as_ref().is_some_and(|p| !well_formed(p));
        if ill_formed && !deactivated {
            self.failures.push(Failure {
                node,
                triple: s.path.as_ref().map(|p| p.1),
                message: format!(
                    "sh:path is not a well-formed property path (empty, not a path, or nested deeper than {MAX_DEPTH} levels): this shape is not validated"
                ),
                kind: FailureKind::Path,
            });
        }
        let mut shape = Shape {
            node,
            skip: ill_formed || deactivated,
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
            let Some(param) = v.param(p) else {
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
                    // The property shapes' paths that are a single IRI.
                    let mut allowed: FxHashSet<Iri> = own
                        .iter()
                        .filter(|t| t.1 == v.property)
                        .filter_map(|t| model.get(t.2)?.path.as_ref())
                        .filter_map(|&(_, i)| g.as_iri(g.triple_at(i).o))
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
        self.component_uses(&mut shape);
        shape
    }

    /// The SPARQL-based constraint `c` of `shape` (stated by triple `ti`).
    fn sparql_constraint(&mut self, shape: &mut Shape, ti: u32, c: Id) {
        let g = self.g;
        let v = vocab::get();
        if g.objects(c, v.deactivated).any(|d| self.is_true(d)) {
            return;
        }
        let path = shape.path.as_ref();
        if let Some((query, _)) = prepare(
            g,
            c,
            false,
            path,
            &sparql::constraint_role(),
            &mut self.failures,
        ) {
            let check = Check {
                batch: sparql::batchable(&query),
                query,
                ask: false,
                constraint: Some(c),
                component: component("SPARQL"),
                messages: g.objects(c, v.message).collect(),
                bindings: Vec::new(),
                source: c,
            };
            shape.constraints.push((C::Sparql(Rc::new(check)), ti));
        }
    }

    /// The SPARQL-based components whose mandatory parameters `shape` has.
    /// Their parameters may be any predicate, so all of the shape's triples
    /// are looked at.
    fn component_uses(&mut self, shape: &mut Shape) {
        if self.components.is_empty() {
            return;
        }
        let g = self.g;
        let v = vocab::get();
        let own: Vec<(u32, Iri, Id)> = g.out(shape.node).map(|(i, t)| (i, t.p, t.o)).collect();
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
            let mut names: Vec<String> = ["this", "shapesGraph", "currentShape"]
                .map(String::from)
                .into();
            if ask {
                names.push("value".into());
            }
            names.extend(comp.params.iter().map(|(_, name, _)| name.clone()));
            let Some((query, is_ask)) = prepare(
                g,
                validator,
                ask,
                shape.path.as_ref(),
                &Role::PreBound(names),
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
        let v = vocab::get();
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

/// What one validation may spend before it stops: results kept, and steps:
/// those of path evaluation (closure and sequence work, which a graph can
/// make quadratic or worse) and one per shape check (shape-valued
/// constraints past the depth limit, never cached, can make those
/// exponential).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    pub(crate) results: usize,
    pub(crate) steps: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            results: 25_000,
            steps: 40_000_000,
        }
    }
}

/// Steps between two polls of the cancel check.
const POLL_EVERY: u64 = 4096;

/// Ids kept in [`Run::memo`] at most; past that it starts over.
const MEMO_IDS: usize = 8 << 20;

/// The most focus nodes one SPARQL-based target may select.
const MAX_TARGETS: usize = 1_000_000;

#[cfg(test)]
thread_local! {
    /// Nodes looked at to find the instances of classes, to tell a run that
    /// walks the whole graph from one among a few nodes.
    pub(crate) static TARGETS_LOOKED_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// SPARQL checks run for one focus node, and run once for a batch.
    static QUERIES_RUN: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

struct Run<'r, 'g, 'a> {
    g: &'g Graph<'a>,
    shapes: &'r [Shape],
    stack: Vec<(ShapeIx, Id)>,
    supers: FxHashMap<Id, Rc<FxHashSet<Id>>>,
    conforms: FxHashMap<(ShapeIx, Id), bool>,
    /// The values along a path from a node, by path (the address of the
    /// [`Path`] in its shape), node and direction. Closures and sequences
    /// nested in other paths ask for the same ones over and over.
    memo: FxHashMap<(usize, Id, bool), Rc<[Id]>>,
    memo_ids: usize,
    /// The graph as a SPARQL dataset, when the shapes use SPARQL.
    ix: Option<&'r Index<'g>>,
    ev: sparql::Engine,
    cancel: Cancel,
    /// `cancel` said so.
    stopped: bool,
    budget: Budget,
    steps: u64,
    since_poll: u64,
    /// The step budget ran out.
    exhausted: bool,
    /// The result budget ran out.
    capped: bool,
    /// Checks cut short by the depth limit so far.
    cuts: u64,
    failures: Vec<Failure>,
    /// The focus nodes of the shape being validated: a SPARQL check that
    /// reaches one of them runs once for all of them, when there are at
    /// least `min_batch`.
    min_batch: usize,
    /// Only these nodes (sorted) can be focus nodes; targets are looked for
    /// among them rather than in the whole graph. See [`validate_report`].
    among: Option<&'r [Id]>,
    batch: Vec<Id>,
    in_batch: FxHashSet<Id>,
    /// The solutions of such runs per focus node, by shape and check (`None`
    /// when the check runs per focus node).
    batched: FxHashMap<(Id, *const Check), Option<sparql::ByFocus>>,
    json: fn(&str) -> bool,
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
    if v.len() < 2 {
        return;
    }
    let mut seen = FxHashSet::default();
    v.retain(|x| seen.insert(*x));
}

impl Run<'_, '_, '_> {
    /// Whether validation must stop: cancelled, out of steps or results.
    fn halted(&self) -> bool {
        self.stopped || self.exhausted || self.capped
    }

    /// Counts `n` steps of path evaluation, for the shape being checked.
    fn charge(&mut self, n: usize) {
        let at = self.stack.last().map(|&(si, _)| si);
        self.spend(n as u64, at);
    }

    /// Counts `n` steps made for shape `at`: polls the cancel check every
    /// few thousand, and stops everything once the budget is spent.
    fn spend(&mut self, n: u64, at: Option<ShapeIx>) {
        self.steps = self.steps.saturating_sub(n);
        self.since_poll += n;
        if self.since_poll >= POLL_EVERY {
            self.since_poll = 0;
            if (self.cancel)() {
                self.stopped = true;
            }
        }
        if self.steps == 0 && !self.exhausted {
            self.exhausted = true;
            if let Some(si) = at {
                let node = self.shapes[si as usize].node;
                self.fail_with(
                    node,
                    None,
                    format!(
                        "validation took more than {} steps (of property paths and shape checks), the limit for one validation: this shape and the ones after it are not fully validated",
                        self.budget.steps
                    ),
                    FailureKind::Cut,
                );
            }
        }
    }

    /// The values reached from `x` along `path` (backwards when `inv`).
    fn values(&mut self, path: &Path, x: Id, inv: bool) -> Rc<[Id]> {
        match path {
            Path::Predicate(p) => {
                let mut out: Vec<Id> = if inv {
                    self.g.subjects(*p, x).collect()
                } else {
                    self.g.objects(x, *p).collect()
                };
                dedup(&mut out);
                return out.into();
            }
            Path::Inverse(q) => return self.values(q, x, !inv),
            Path::Invalid => return Rc::from([]),
            _ => {}
        }
        let key = (path as *const Path as usize, x, inv);
        if let Some(v) = self.memo.get(&key) {
            return v.clone();
        }
        if self.halted() {
            return Rc::from([]);
        }
        let mut out = match path {
            Path::Sequence(ps) => {
                let mut cur = vec![x];
                let steps: Box<dyn Iterator<Item = &Path>> = if inv {
                    Box::new(ps.iter().rev())
                } else {
                    Box::new(ps.iter())
                };
                for q in steps {
                    let mut next: Vec<Id> = Vec::new();
                    for y in cur {
                        let vs = self.values(q, y, inv);
                        self.charge(vs.len() + 1);
                        next.extend_from_slice(&vs);
                    }
                    dedup(&mut next);
                    cur = next;
                }
                cur
            }
            Path::Alternative(ps) => {
                let mut out = Vec::new();
                for q in ps {
                    out.extend_from_slice(&self.values(q, x, inv));
                }
                self.charge(out.len() + 1);
                out
            }
            // Already without duplicates.
            Path::ZeroOrMore(q) => {
                let out = self.closure(q, x, inv, true);
                return self.remember(key, out);
            }
            Path::OneOrMore(q) => {
                let out = self.closure(q, x, inv, false);
                return self.remember(key, out);
            }
            Path::ZeroOrOne(q) => {
                let mut out = vec![x];
                out.extend_from_slice(&self.values(q, x, inv));
                self.charge(out.len());
                out
            }
            Path::Predicate(_) | Path::Inverse(_) | Path::Invalid => unreachable!("handled above"),
        };
        dedup(&mut out);
        self.remember(key, out)
    }

    /// `values` as the answer for `key`, kept for the next ask (unless a
    /// stop cut it short: that is not the answer).
    fn remember(&mut self, key: (usize, Id, bool), values: Vec<Id>) -> Rc<[Id]> {
        let out: Rc<[Id]> = values.into();
        if !self.halted() {
            if self.memo_ids + out.len() > MEMO_IDS {
                self.memo.clear();
                self.memo_ids = 0;
            }
            self.memo_ids += out.len() + 1;
            self.memo.insert(key, out.clone());
        }
        out
    }

    /// The nodes one step of `q` from `x` reaches, added to `into`.
    fn step(&mut self, q: &Path, x: Id, inv: bool, into: &mut Vec<Id>) {
        match q {
            Path::Predicate(p) if inv => into.extend(self.g.subjects(*p, x)),
            Path::Predicate(p) => into.extend(self.g.objects(x, *p)),
            _ => into.extend_from_slice(&self.values(q, x, inv)),
        }
    }

    fn closure(&mut self, q: &Path, x: Id, inv: bool, with_self: bool) -> Vec<Id> {
        let mut seen = FxHashSet::default();
        let mut out = Vec::new();
        if with_self {
            seen.insert(x);
            out.push(x);
        }
        let mut frontier = Vec::new();
        self.step(q, x, inv, &mut frontier);
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for y in frontier {
                self.charge(1);
                if self.halted() {
                    return out;
                }
                if seen.insert(y) {
                    out.push(y);
                    self.step(q, y, inv, &mut next);
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
        let sub = vocab::get().sub_class_of;
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
        let types: Vec<Id> = self.g.objects(x, vocab::get().rdf_type).collect();
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
        let truncated = self.check(si, focus, &mut tmp, false);
        let ok = tmp.is_empty();
        // A check the depth limit or a stop cut short has not said whether
        // the shape conforms; remembering its answer would make later
        // checks depend on which one ran first.
        if !recursive && !truncated && !self.halted() {
            self.conforms.insert((si, focus), ok);
        }
        ok
    }

    /// Validate `focus` against shape `si`, adding its results to `out`. When
    /// `collect` is false, stop at the first result. Whether the depth limit
    /// cut the check, or something it checks, short.
    fn check(
        &mut self,
        si: ShapeIx,
        focus: Id,
        out: &mut Vec<ValidationResult>,
        collect: bool,
    ) -> bool {
        let shapes = self.shapes;
        let shape = &shapes[si as usize];
        if self.halted() {
            return false;
        }
        // Checks the depth limit cut are never cached, so shape-valued
        // constraints met again and again can make checks exponential
        // without a path step: each check is a step.
        self.spend(1, Some(si));
        if self.halted() || shape.skip || self.stack.contains(&(si, focus)) {
            return false;
        }
        if self.stack.len() > MAX_DEPTH {
            self.cuts += 1;
            self.fail_with(
                shape.node,
                None,
                format!(
                    "shapes are nested deeper than {MAX_DEPTH} levels here (sh:node, sh:property, sh:and ... through the data): results below this depth are missing"
                ),
                FailureKind::Cut,
            );
            return true;
        }
        let cuts = self.cuts;
        self.stack.push((si, focus));
        let values = match &shape.path {
            None => Rc::from([focus]),
            Some(p) => self.values(p, focus, false),
        };
        for (c, param) in &shape.constraints {
            let before = out.len();
            self.constraint(shape, c, *param, focus, &values, out, collect);
            if self.halted() || (!collect && out.len() > before) {
                break;
            }
            if collect && out.len() > self.budget.results {
                self.capped = true;
                break;
            }
        }
        self.stack.pop();
        self.cuts != cuts
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
                    .is_some_and(|(lex, d, _)| d == *dt && literal::well_formed(&d, lex, self.json))
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
                let in_others: FxHashSet<Id> = others.iter().copied().collect();
                let in_values: FxHashSet<Id> = values.iter().copied().collect();
                for &x in values.iter().filter(|x| !in_others.contains(x)) {
                    emit(out, Some(x), None);
                }
                for &o in others.iter().filter(|o| !in_values.contains(o)) {
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
            // A shape checked after validation halted has not said whether
            // it conforms: nothing follows from its answer.
            C::Not(s) => {
                for &x in values {
                    let conforms = self.conforms(*s, x);
                    if self.halted() {
                        return;
                    }
                    if conforms {
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
                    if self.halted() {
                        return;
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
                    let conforms = self.conforms(*s, x);
                    if self.halted() {
                        return;
                    }
                    if !conforms {
                        emit(out, Some(x), None);
                    }
                }
            }
            C::Property(s) => {
                for &x in values {
                    self.check(*s, x, out, collect);
                    if self.halted() || (!collect && !out.is_empty()) {
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
                if self.halted() {
                    return;
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
            C::Sparql(check) => self.sparql(shape, check, param, focus, values, out, collect),
        }
    }

    fn fail(&mut self, node: Id, message: String) {
        self.fail_with(node, None, message, FailureKind::Query);
    }

    fn fail_with(&mut self, node: Id, triple: Option<u32>, message: String, kind: FailureKind) {
        let f = Failure {
            node,
            triple,
            message,
            kind,
        };
        if !self.failures.contains(&f) {
            self.failures.push(f);
        }
    }

    /// What a query that gave no answer means for `node`'s validation.
    fn query_failed(&mut self, node: Id, why: Fail) {
        match why {
            Fail::Cancelled => self.stopped = true,
            Fail::TooMany => {}
            Fail::Failed(message) => self.fail(node, message),
        }
    }

    /// Runs a SPARQL check on `focus`: a SELECT query once, an ASK query once
    /// per value node. Past the result budget it stops reading solutions.
    #[allow(clippy::too_many_arguments)]
    fn sparql(
        &mut self,
        shape: &Shape,
        check: &Check,
        param: u32,
        focus: Id,
        values: &[Id],
        out: &mut Vec<ValidationResult>,
        collect: bool,
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
            if self.halted() {
                return;
            }
            if (self.cancel)() {
                self.stopped = true;
                return;
            }
            // Room for one more result than the budget keeps, to tell
            // that there are more; a nested check needs one to say no.
            let room = if collect {
                (self.budget.results + 1).saturating_sub(out.len())
            } else {
                1
            };
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
                Some(rows) => Ok(Answer::Solutions {
                    more: rows.len() > room,
                    rows,
                }),
                None => {
                    #[cfg(test)]
                    QUERIES_RUN.with(|c| c.set((c.get().0 + 1, c.get().1)));
                    self.ev.run(ix, &check.query, bindings.clone(), room)
                }
            };
            match answer {
                Err(why) => self.query_failed(check.source, why),
                Ok(Answer::Boolean(true)) => {}
                Ok(Answer::Boolean(false)) => {
                    out.push(result(value, shape.path.clone(), fill(&lookup_base)));
                }
                Ok(Answer::Solutions { rows, more }) => {
                    self.capped |= collect && more;
                    for sol in rows.into_iter().take(room) {
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
            if collect && out.len() > self.budget.results {
                self.capped = true;
                return;
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
    /// check can run that way. The run is kept for the other focus nodes,
    /// so it may give as many solutions as a collecting check keeps, whoever
    /// asks first (a nested check needs only one, and would otherwise make
    /// every batch of two or more too many).
    fn batched(
        &mut self,
        ix: &Index,
        shape: &Shape,
        check: &Check,
        focus: Id,
        constants: &[(Variable, RdfTerm)],
    ) -> Option<Vec<QuerySolution>> {
        if !check.batch
            || self.batch.len() < self.min_batch
            || sparse(self.batch.len(), self.g.len())
            || !self.in_batch.contains(&focus)
        {
            return None;
        }
        let key = (shape.node, check as *const Check);
        let max = self.budget.results + 1;
        let found = self.batched.entry(key).or_insert_with(|| {
            #[cfg(test)]
            QUERIES_RUN.with(|c| c.set((c.get().0, c.get().1 + 1)));
            match self
                .ev
                .run_batch(ix, &check.query, constants, &self.batch, max)
            {
                Some(Ok(map)) => Some(map),
                Some(Err(Fail::Cancelled)) => {
                    self.stopped = true;
                    None
                }
                // Too many solutions, a failure, or a query that cannot be
                // batched: the focus nodes run one by one.
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
        if g.objects(target, vocab::get().deactivated)
            .any(|d| matches!(g.term(d).literal(), Some(("true", _, _))))
        {
            return;
        }
        let Some((query, _)) = prepare(g, target, false, None, &Role::Target, &mut self.failures)
        else {
            return;
        };
        let graph = NamedNode::new_unchecked(sparql::SHAPES_GRAPH);
        match self.ev.run(
            ix,
            &query,
            vec![(Variable::new_unchecked("shapesGraph"), graph.into())],
            MAX_TARGETS,
        ) {
            Ok(Answer::Solutions { rows, more }) => {
                out.extend(
                    rows.iter()
                        .filter_map(|s| s.get("this").and_then(|t| ix.id(t))),
                );
                if more {
                    self.fail_with(
                        target,
                        None,
                        format!(
                            "the target selects more than {MAX_TARGETS} nodes: the rest are not validated"
                        ),
                        FailureKind::Cut,
                    );
                }
            }
            Ok(_) => {}
            Err(why) => self.query_failed(target, why),
        }
    }

    /// The focus nodes of shape `s`: its explicit targets, and itself as a
    /// class when it is one.
    fn focus_nodes(&mut self, s: &model::Shape) -> Vec<Id> {
        let g = self.g;
        let v = vocab::get();
        let node = s.node;
        let mut out = Vec::new();
        let types: Vec<Id> = g.objects(node, v.rdf_type).collect();
        let is_class = types.iter().any(|&o| {
            g.as_iri(o)
                .is_some_and(|c| c == v.rdfs_class || c == v.owl_class)
        }) || {
            let classes = [v.rdfs_class, v.owl_class].map(|c| g.iri(c));
            types.into_iter().any(|t| {
                let s = self.supers(t);
                classes.iter().flatten().any(|c| s.contains(c))
            })
        };
        if is_class {
            out.extend(self.instances(node));
        }
        // Its targets of every kind, in the order it states them.
        for &i in &s.params {
            let t = g.triple_at(i);
            let (p, o) = (t.p, t.o);
            if p == v.target_node {
                out.push(o);
            } else if p == v.target_class {
                out.extend(self.instances(o));
            } else if p == v.target_subjects_of {
                if let Some(pi) = g.as_iri(o) {
                    match self.among {
                        None => out.extend(g.with_p(pi).map(|t| t.s)),
                        Some(among) => out.extend(
                            among
                                .iter()
                                .copied()
                                .filter(|&n| g.out(n).any(|t| t.1.p == pi)),
                        ),
                    }
                }
            } else if p == v.target_objects_of
                && let Some(pi) = g.as_iri(o)
            {
                match self.among {
                    None => out.extend(g.with_p(pi).map(|t| t.o)),
                    Some(among) => out.extend(
                        among
                            .iter()
                            .copied()
                            .filter(|&n| g.incoming(n).any(|t| t.1.p == pi)),
                    ),
                }
            } else if p == v.target {
                self.sparql_target(o, &mut out);
            }
        }
        dedup(&mut out);
        out
    }

    /// SHACL instances of `class`: typed with it or one of its subclasses.
    fn instances(&self, class: Id) -> Vec<Id> {
        let v = vocab::get();
        let mut classes = FxHashSet::default();
        let mut todo = vec![class];
        let mut out = Vec::new();
        while let Some(c) = todo.pop() {
            if classes.insert(c) {
                todo.extend(self.g.subjects(v.sub_class_of, c));
                if self.among.is_none() {
                    out.extend(self.g.subjects(v.rdf_type, c));
                }
            }
        }
        #[cfg(test)]
        TARGETS_LOOKED_AT.with(|c| c.set(c.get() + self.among.map_or(out.len(), <[Id]>::len)));
        if let Some(among) = self.among {
            // Only those nodes can be focus nodes: which of them have the type.
            out.extend(
                among
                    .iter()
                    .copied()
                    .filter(|&n| self.g.objects(n, v.rdf_type).any(|t| classes.contains(&t))),
            );
        }
        out
    }
}

/// What makes a shape one validation starts from, in the order it visits
/// them: the target predicates, then the shape classes.
fn root_kinds() -> [Iri; 7] {
    let v = vocab::get();
    [
        v.target_class,
        v.target_node,
        v.target_subjects_of,
        v.target_objects_of,
        v.target,
        v.node_shape,
        v.property_shape,
    ]
}

/// The shapes validation starts from, those with a target or a shape type
/// ([`root_kinds`]), in the order it visits them: by kind, and within a kind
/// in the order the graph's indexes list the triples that say so (an
/// overlay's own first, then the base's, each by number; see
/// [`Graph::overlay`]).
fn roots<'m>(g: &Graph, model: &'m Shapes) -> Vec<&'m model::Shape> {
    let v = vocab::get();
    let kinds = root_kinds();
    let kind = |i: Iri| kinds.iter().position(|&k| k == i);
    let own_from = g.added().next().map_or(g.len() as u32, |(t, _)| t);
    // (kind, from the base, triple number, shape).
    let mut keyed: Vec<(usize, bool, u32, usize)> = Vec::new();
    for (si, s) in model.shapes.iter().enumerate() {
        for &t in s.targets.iter().chain(&s.sparql_targets) {
            if let Some(k) = kind(g.triple_at(t).p) {
                keyed.push((k, t < own_from, t, si));
            }
        }
        if s.typed_node_shape || s.typed_property_shape {
            for (t, tr) in g.out(s.node) {
                let shape_class = |c: &Iri| *c == v.node_shape || *c == v.property_shape;
                if tr.p == v.rdf_type
                    && let Some(k) = g.as_iri(tr.o).filter(shape_class).and_then(kind)
                {
                    keyed.push((k, t < own_from, t, si));
                }
            }
        }
    }
    keyed.sort_unstable();
    let mut seen = FxHashSet::default();
    keyed
        .into_iter()
        .filter(|k| seen.insert(k.3))
        .map(|k| &model.shapes[k.3])
        .collect()
}

/// Whether anything validation acts on is declared: a target (`sh:targetClass`,
/// `sh:targetNode`, `sh:targetSubjectsOf`, `sh:targetObjectsOf`, `sh:target`)
/// or a typed shape, where `used` says whether an IRI occurs. The one answer
/// for a graph ([`validate_report`], see [`roots`]) and for a document
/// (`has_shapes` in the editor's SHACL feature).
pub(crate) fn acts_on(used: &dyn Fn(Iri) -> bool) -> bool {
    root_kinds().into_iter().any(used)
}

/// Validate `g` against the shapes in it. `cancelled` is polled between
/// focus nodes, between SPARQL queries, every few thousand steps (of path
/// evaluation and shape checks), and inside a running query; a run it
/// stops returns `None`. `json` says whether a text is JSON, for `rdf:JSON`;
/// `now` is the time, for the SPARQL time limit.
/// The [`Failure`]s are the queries and shapes that could not be validated.
///
/// With `among` (sorted), only those nodes are focus nodes, and the targets
/// are found among them, not by walking the whole graph, so a few nodes of a
/// large graph cost in proportion to themselves (a SPARQL target is the
/// exception: its query runs over the graph).
pub fn validate_report(
    g: &Graph,
    cancelled: &(impl Fn() -> bool + Send + Sync + Clone + 'static),
    among: Option<&[Id]>,
    json: fn(&str) -> bool,
    now: fn() -> Duration,
) -> Option<Report> {
    let keep = |id: Id| among.is_none_or(|a| a.binary_search(&id).is_ok());
    report_over(
        g,
        Arc::new(cancelled.clone()),
        &keep,
        among,
        MIN_BATCH,
        Budget::default(),
        json,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
fn report_over(
    g: &Graph,
    cancel: Cancel,
    keep: &dyn Fn(Id) -> bool,
    among: Option<&[Id]>,
    min_batch: usize,
    budget: Budget,
    json: fn(&str) -> bool,
    now: fn() -> Duration,
) -> Option<Report> {
    let model = Shapes::new(g);
    let mut comp = Compiler {
        g,
        model: &model,
        shapes: Vec::new(),
        index: FxHashMap::default(),
        pending: Vec::new(),
        components: components(g),
        failures: Vec::new(),
    };
    let roots: Vec<(ShapeIx, &model::Shape)> = roots(g, &model)
        .into_iter()
        .map(|s| (comp.shape(s.node), s))
        .collect();
    comp.build_pending();
    let shapes = comp.shapes;
    let uses_sparql = model.shapes.iter().any(|s| !s.sparql_targets.is_empty())
        || shapes
            .iter()
            .any(|s| s.constraints.iter().any(|c| matches!(c.0, C::Sparql(_))));
    let ix = uses_sparql.then(|| Index::new(g, Some(sparql::SHAPES_GRAPH)));
    let mut run = Run {
        g,
        shapes: &shapes,
        stack: Vec::new(),
        supers: FxHashMap::default(),
        conforms: FxHashMap::default(),
        memo: FxHashMap::default(),
        memo_ids: 0,
        ix: ix.as_ref(),
        ev: sparql::Engine::new(cancel.clone(), now),
        cancel,
        stopped: false,
        budget,
        steps: budget.steps,
        since_poll: 0,
        exhausted: false,
        capped: false,
        cuts: 0,
        failures: comp.failures,
        min_batch,
        among,
        batch: Vec::new(),
        in_batch: FxHashSet::default(),
        batched: FxHashMap::default(),
        json,
    };
    let mut out = Vec::new();
    'shapes: for (si, root) in roots {
        if shapes[si as usize].skip {
            continue;
        }
        let focus_nodes = run.focus_nodes(root);
        run.set_batch(focus_nodes.iter().copied().filter(|&f| keep(f)).collect());
        for focus in focus_nodes {
            if (run.cancel)() {
                return None;
            }
            if !keep(focus) {
                continue;
            }
            let before = out.len();
            run.check(si, focus, &mut out, true);
            if run.stopped {
                return None;
            }
            if run.exhausted {
                // The check ran out of steps part way: its values and the
                // shapes it asked about are partial, so what it found may be
                // false. The Cut failure says the rest is not validated.
                out.truncate(before);
            }
            if run.halted() {
                break 'shapes;
            }
        }
    }
    if run.stopped {
        return None;
    }
    let capped = run.capped || out.len() > budget.results;
    out.truncate(budget.results);
    let mut failures = run.failures;
    failures.dedup();
    Some(Report {
        results: out,
        failures,
        capped,
    })
}

#[cfg(test)]
mod tests;
