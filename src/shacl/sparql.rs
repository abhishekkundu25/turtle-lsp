//! SHACL-SPARQL: preparing the queries of SPARQL-based constraints,
//! constraint components and targets, and running them with `spareval` over
//! the validation [`Graph`] in place (through [`Index`], no copy of the data).
//!
//! Pre-binding follows the SHACL specification: every occurrence of a
//! pre-bound variable is replaced by its value (see [`Engine`]), and the
//! constructs that would break that (MINUS, VALUES, SERVICE, `AS ?var` for a
//! pre-bound variable, a subquery that does not return the pre-bound
//! variables) are rejected. `$PATH` is replaced by the shape's path in SPARQL
//! syntax before parsing.
//!
//! The data graph is the default graph; the same graph is also the named
//! graph `$shapesGraph` names.

use super::Path;
use super::graph::{Graph, Id, Term as GTerm};
use crate::syntax::{Iri, Node};
use rustc_hash::FxHashMap;
use spareval::{InternalQuad, QueryEvaluator, QueryResults, QuerySolution, QueryableDataset};
use spargebra::algebra::{
    AggregateExpression, Expression, Function, GraphPattern, OrderExpression,
};
use spargebra::term::{
    BlankNode, Literal, NamedNode, NamedNodePattern, NamedOrBlankNode, Term, TermPattern, Triple,
    TriplePattern, Variable,
};
use spargebra::{Query, SparqlParser};
use std::convert::Infallible;

/// The IRI `$shapesGraph` is bound to.
pub const SHAPES_GRAPH: &str = "urn:x-shacl:shapes-graph";

/// A term as the evaluator sees it: a graph term by id when the graph has it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum STerm {
    Id(Id),
    /// An IRI the graph uses as a predicate only.
    Iri(Iri),
    ShapesGraph,
    /// A term the graph does not have.
    Other(Box<Term>),
}

/// The validation graph as a SPARQL dataset.
pub struct Index<'a> {
    g: &'a Graph<'a>,
    literals: FxHashMap<(&'a str, Iri, Option<Iri>), Id>,
}

impl<'a> Index<'a> {
    /// One pass over the graph's terms, to find literals by value.
    pub fn new(g: &'a Graph<'a>) -> Index<'a> {
        let literals = g
            .terms()
            .filter_map(|(id, t)| t.literal().map(|(v, d, l)| ((v, d, l), id)))
            .collect();
        Index { g, literals }
    }

    pub fn term(&self, id: Id) -> Term {
        match self.g.term(id) {
            GTerm::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
            GTerm::Blank(b) => BlankNode::new_unchecked(format!("g{b}")).into(),
            GTerm::Value(n) => node_term(n),
        }
    }

    /// The graph id of `t`, if the graph has it.
    pub fn id(&self, t: &Term) -> Option<Id> {
        match self.internal(t) {
            STerm::Id(id) => Some(id),
            _ => None,
        }
    }

    fn internal(&self, t: &Term) -> STerm {
        let found = match t {
            Term::NamedNode(n) if n.as_str() == SHAPES_GRAPH => return STerm::ShapesGraph,
            Term::NamedNode(n) if n.as_str().starts_with(BLANK) => n.as_str()[BLANK.len()..]
                .strip_prefix('g')
                .and_then(|n| n.parse().ok())
                .and_then(|n| self.g.id(GTerm::Blank(n))),
            Term::NamedNode(n) => match Iri::get(n.as_str()) {
                Some(i) => return self.g.iri(i).map_or(STerm::Iri(i), STerm::Id),
                None => None,
            },
            Term::BlankNode(b) => b
                .as_str()
                .strip_prefix('g')
                .and_then(|n| n.parse().ok())
                .and_then(|n| self.g.id(GTerm::Blank(n))),
            Term::Literal(l) => {
                let lang = match l.language() {
                    Some(l) => Iri::get(l).map(Some),
                    None => Some(None),
                };
                match (Iri::get(l.datatype().as_str()), lang) {
                    (Some(dt), Some(lang)) => self.literals.get(&(l.value(), dt, lang)).copied(),
                    _ => None,
                }
            }
            _ => None,
        };
        found.map_or_else(|| STerm::Other(Box::new(t.clone())), STerm::Id)
    }

    fn predicate(&self, p: Iri) -> STerm {
        self.g.iri(p).map_or(STerm::Iri(p), STerm::Id)
    }

    fn quads(
        &'a self,
        s: Option<&STerm>,
        p: Option<&STerm>,
        o: Option<&STerm>,
        graph: Option<Option<&STerm>>,
    ) -> Option<Box<dyn Iterator<Item = Result<InternalQuad<STerm>, Infallible>> + 'a>> {
        let graph_name = match graph {
            Some(None) => None,
            Some(Some(STerm::ShapesGraph)) | None => Some(STerm::ShapesGraph),
            Some(Some(_)) => return None,
        };
        let id = |t: Option<&STerm>| match t {
            None => Some(None),
            Some(STerm::Id(i)) => Some(Some(*i)),
            Some(_) => None,
        };
        let (s, o) = (id(s)?, id(o)?);
        let p = match p {
            None => None,
            Some(STerm::Id(i)) => Some(self.g.as_iri(*i)?),
            Some(STerm::Iri(i)) => Some(*i),
            Some(_) => return None,
        };
        let g = self.g;
        let triples: Box<dyn Iterator<Item = &'a super::graph::GTriple> + 'a> = match (s, o, p) {
            (Some(s), _, _) => Box::new(g.out(s).map(|t| t.1)),
            (None, Some(o), _) => Box::new(g.incoming(o).map(|t| t.1)),
            (None, None, Some(p)) => Box::new(g.with_p(p)),
            (None, None, None) => Box::new(g.triples.iter()),
        };
        Some(Box::new(
            triples
                .filter(move |t| {
                    s.is_none_or(|s| t.s == s)
                        && o.is_none_or(|o| t.o == o)
                        && p.is_none_or(|p| t.p == p)
                })
                .map(move |t| {
                    Ok(InternalQuad {
                        subject: STerm::Id(t.s),
                        predicate: self.predicate(t.p),
                        object: STerm::Id(t.o),
                        graph_name: graph_name.clone(),
                    })
                }),
        ))
    }
}

fn node_term(n: &Node) -> Term {
    match n {
        Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
        Node::Blank(b) => BlankNode::new_unchecked(format!("t{}", b.0)).into(),
        Node::Literal {
            value,
            datatype,
            lang,
        } => match lang {
            Some(l) => {
                Literal::new_language_tagged_literal_unchecked(value.as_ref(), l.as_str()).into()
            }
            None => Literal::new_typed_literal(
                value.as_ref(),
                NamedNode::new_unchecked(datatype.as_str()),
            )
            .into(),
        },
        Node::Triple(t) => {
            let s: NamedOrBlankNode = match node_term(&t.0) {
                Term::NamedNode(n) => n.into(),
                Term::BlankNode(b) => b.into(),
                _ => BlankNode::default().into(),
            };
            Term::Triple(Box::new(Triple::new(
                s,
                NamedNode::new_unchecked(t.1.as_str()),
                node_term(&t.2),
            )))
        }
    }
}

impl<'a> QueryableDataset<'a> for &'a Index<'a> {
    type InternalTerm = STerm;
    type Error = Infallible;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&STerm>,
        predicate: Option<&STerm>,
        object: Option<&STerm>,
        graph_name: Option<Option<&STerm>>,
    ) -> impl Iterator<Item = Result<InternalQuad<STerm>, Infallible>> + use<'a> {
        let ix: &'a Index<'a> = self;
        ix.quads(subject, predicate, object, graph_name)
            .into_iter()
            .flatten()
    }

    fn internal_named_graphs(&self) -> impl Iterator<Item = Result<STerm, Infallible>> + use<'a> {
        std::iter::once(Ok(STerm::ShapesGraph))
    }

    fn contains_internal_graph_name(&self, graph_name: &STerm) -> Result<bool, Infallible> {
        Ok(*graph_name == STerm::ShapesGraph)
    }

    fn internalize_term(&self, term: Term) -> Result<STerm, Infallible> {
        Ok(self.internal(&term))
    }

    fn externalize_term(&self, term: STerm) -> Result<Term, Infallible> {
        Ok(match term {
            STerm::Id(id) => self.term(id),
            STerm::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
            STerm::ShapesGraph => NamedNode::new_unchecked(SHAPES_GRAPH).into(),
            STerm::Other(t) => *t,
        })
    }
}

/// The variables a query may see pre-bound, and which of them a subquery
/// must return.
pub struct PreBound<'s> {
    pub names: &'s [&'s str],
    /// Must be projected by every subquery (all but `currentShape` and
    /// `shapesGraph`).
    pub required: &'s [&'s str],
}

/// Parses `text` with `prefixes` and checks it against the SHACL-SPARQL
/// restrictions; `$PATH` becomes `path`. The top-level projection is extended
/// with every pre-bound variable so the bindings reach the whole query.
pub fn prepare(
    text: &str,
    prefixes: &[(String, String)],
    path: Option<&Path>,
    pre: &PreBound,
) -> Result<Query, String> {
    let text = match path {
        Some(p) => replace_path(text, &path_sparql(p)),
        None => text.to_string(),
    };
    let mut parser = SparqlParser::new();
    for (name, ns) in prefixes {
        parser = parser
            .with_prefix(name, ns)
            .map_err(|e| format!("namespace <{ns}> of prefix {name}: {e}"))?;
    }
    let mut query = parser.parse_query(&text).map_err(|e| e.to_string())?;
    let vars: Vec<Variable> = pre
        .names
        .iter()
        .map(|n| Variable::new_unchecked(*n))
        .collect();
    match &mut query {
        // Both have the projection on top (ASK projects its in-scope variables).
        Query::Select { pattern, .. } | Query::Ask { pattern, .. } => {
            check(pattern, true, pre)?;
            project(pattern, &vars);
        }
        _ => return Err("only SELECT and ASK queries are supported here".into()),
    }
    Ok(query)
}

fn project(p: &mut GraphPattern, vars: &[Variable]) {
    match p {
        GraphPattern::Slice { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner } => project(inner, vars),
        GraphPattern::Project { variables, .. } => {
            for v in vars {
                if !variables.contains(v) {
                    variables.push(v.clone());
                }
            }
        }
        _ => {}
    }
}

/// The SHACL-SPARQL restrictions on queries with pre-bound variables.
/// `top`: `p` is the query's own projection (or above it).
fn check(p: &GraphPattern, top: bool, pre: &PreBound) -> Result<(), String> {
    use GraphPattern as G;
    match p {
        G::Bgp { .. } | G::Path { .. } => Ok(()),
        G::Join { left, right } | G::Union { left, right } => {
            check(left, false, pre)?;
            check(right, false, pre)
        }
        G::LeftJoin {
            left,
            right,
            expression,
        } => {
            check(left, false, pre)?;
            check(right, false, pre)?;
            expression.as_ref().map_or(Ok(()), |e| check_expr(e, pre))
        }
        G::Filter { expr, inner } => {
            check_expr(expr, pre)?;
            check(inner, false, pre)
        }
        G::Graph { inner, .. } | G::Group { inner, .. } => check(inner, false, pre),
        G::Extend {
            inner,
            variable,
            expression,
        } => {
            if pre.names.contains(&variable.as_str()) {
                return Err(format!(
                    "AS ?{} assigns a pre-bound variable",
                    variable.as_str()
                ));
            }
            check_expr(expression, pre)?;
            check(inner, false, pre)
        }
        G::Lateral { .. } => Err("LATERAL is not standard SPARQL".into()),
        G::Minus { .. } => Err("MINUS is not allowed with pre-bound variables".into()),
        G::Values { .. } => Err("VALUES is not allowed with pre-bound variables".into()),
        G::Service { .. } => Err("SERVICE is not allowed with pre-bound variables".into()),
        G::OrderBy { inner, .. } => check(inner, top, pre),
        G::Distinct { inner } | G::Reduced { inner } | G::Slice { inner, .. } => {
            check(inner, top, pre)
        }
        G::Project { inner, variables } => {
            if !top
                && let Some(missing) = pre
                    .required
                    .iter()
                    .find(|r| !variables.iter().any(|v| v.as_str() == **r))
            {
                return Err(format!("a subquery must return ${missing}"));
            }
            check(inner, false, pre)
        }
        #[allow(unreachable_patterns)]
        _ => Ok(()),
    }
}

fn check_expr(e: &Expression, pre: &PreBound) -> Result<(), String> {
    use Expression as E;
    match e {
        E::Exists(p) => check(p, false, pre),
        E::Or(a, b)
        | E::And(a, b)
        | E::Equal(a, b)
        | E::SameTerm(a, b)
        | E::Greater(a, b)
        | E::GreaterOrEqual(a, b)
        | E::Less(a, b)
        | E::LessOrEqual(a, b)
        | E::Add(a, b)
        | E::Subtract(a, b)
        | E::Multiply(a, b)
        | E::Divide(a, b) => {
            check_expr(a, pre)?;
            check_expr(b, pre)
        }
        E::UnaryPlus(a) | E::UnaryMinus(a) | E::Not(a) => check_expr(a, pre),
        E::In(a, list) => {
            check_expr(a, pre)?;
            list.iter().try_for_each(|x| check_expr(x, pre))
        }
        E::If(a, b, c) => {
            check_expr(a, pre)?;
            check_expr(b, pre)?;
            check_expr(c, pre)
        }
        E::Coalesce(list) | E::FunctionCall(_, list) => {
            list.iter().try_for_each(|x| check_expr(x, pre))
        }
        _ => Ok(()),
    }
}

/// `path` in SPARQL property path syntax.
pub fn path_sparql(p: &Path) -> String {
    let join = |ps: &[Path], sep: &str| {
        let parts: Vec<String> = ps.iter().map(path_sparql).collect();
        format!("({})", parts.join(sep))
    };
    match p {
        Path::Predicate(i) => format!("<{}>", i.as_str()),
        Path::Inverse(q) => format!("^{}", group(q)),
        Path::Sequence(ps) => join(ps, "/"),
        Path::Alternative(ps) => join(ps, "|"),
        Path::ZeroOrMore(q) => format!("{}*", group(q)),
        Path::OneOrMore(q) => format!("{}+", group(q)),
        Path::ZeroOrOne(q) => format!("{}?", group(q)),
        Path::Invalid => "<urn:x-shacl:invalid-path>".into(),
    }
}

fn group(p: &Path) -> String {
    match p {
        Path::Predicate(_) | Path::Sequence(_) | Path::Alternative(_) => path_sparql(p),
        _ => format!("({})", path_sparql(p)),
    }
}

/// `text` with each `$PATH` (not followed by a name character) replaced.
fn replace_path(text: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("$PATH") {
        let after = &rest[i + 5..];
        out.push_str(&rest[..i]);
        if after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            out.push_str("$PATH");
        } else {
            out.push_str(with);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// What a query returned.
pub enum Answer {
    Solutions(Vec<QuerySolution>),
    Boolean(bool),
}

/// Runs queries with pre-bound variables. The pre-bound values are written
/// into the query as constants so the optimizer can plan the query around
/// them; queries whose values cannot be written as constants (RDF-star
/// triples) run unoptimized with the values as initial bindings.
pub struct Engine {
    optimized: QueryEvaluator,
    plain: QueryEvaluator,
}

/// Blank nodes have no constant syntax: in triple patterns they become an IRI
/// [`Index`] maps back to the node, in expressions a call to a function that
/// returns the node.
const BLANK: &str = "urn:x-shacl:blank:";

impl Default for Engine {
    fn default() -> Self {
        Engine {
            optimized: QueryEvaluator::new().with_custom_function(
                NamedNode::new_unchecked(BLANK),
                |args: &[Term]| match args {
                    [Term::Literal(l)] => Some(BlankNode::new_unchecked(l.value()).into()),
                    _ => None,
                },
            ),
            plain: QueryEvaluator::new().without_optimizations(),
        }
    }
}

impl Engine {
    /// Runs `query` over `ix` with `bindings` pre-bound.
    pub fn run(
        &self,
        ix: &Index,
        query: &Query,
        bindings: Vec<(Variable, Term)>,
    ) -> Result<Answer, String> {
        let substituted = substitute(query, &bindings);
        let prepared = match &substituted {
            Some(q) => self.optimized.prepare(q),
            None => {
                let mut prepared = self.plain.prepare(query);
                for (v, t) in bindings {
                    prepared = prepared.substitute_variable(v, t);
                }
                prepared
            }
        };
        match prepared.execute(ix).map_err(|e| e.to_string())? {
            QueryResults::Solutions(s) => s
                .collect::<Result<Vec<_>, _>>()
                .map(Answer::Solutions)
                .map_err(|e| e.to_string()),
            QueryResults::Boolean(b) => Ok(Answer::Boolean(b)),
            QueryResults::Graph(_) => Err("not a SELECT or ASK query".into()),
        }
    }
}

/// `query` with every occurrence of a pre-bound variable replaced by its
/// value. A projection drops the variable (the caller knows its value), a
/// GROUP BY keeps it as a key bound to the value. `None` when a value or a
/// construct cannot be substituted.
fn substitute(query: &Query, bindings: &[(Variable, Term)]) -> Option<Query> {
    if bindings.iter().any(|(_, t)| matches!(t, Term::Triple(_))) {
        return None;
    }
    let mut query = query.clone();
    let pattern = match &mut query {
        Query::Select { pattern, .. } | Query::Ask { pattern, .. } => pattern,
        _ => return None,
    };
    let sub = Substitution(bindings);
    if !sub.pattern(pattern) {
        return None;
    }
    order_exists(pattern, &[]);
    Some(query)
}

/// A variable or a blank node of a pattern, by name.
type Name = String;

fn term_name(t: &TermPattern) -> Option<Name> {
    match t {
        TermPattern::Variable(v) => Some(format!("?{}", v.as_str())),
        TermPattern::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        _ => None,
    }
}

/// The variables and blank nodes `p` mentions (a hint for join ordering).
fn names(p: &GraphPattern, out: &mut Vec<Name>) {
    use GraphPattern as G;
    match p {
        G::Bgp { patterns } => {
            for tp in patterns {
                out.extend(term_name(&tp.subject));
                out.extend(term_name(&tp.object));
                if let NamedNodePattern::Variable(v) = &tp.predicate {
                    out.push(format!("?{}", v.as_str()));
                }
            }
        }
        G::Path {
            subject, object, ..
        } => {
            out.extend(term_name(subject));
            out.extend(term_name(object));
        }
        G::Join { left, right }
        | G::Lateral { left, right }
        | G::Union { left, right }
        | G::LeftJoin { left, right, .. }
        | G::Minus { left, right } => {
            names(left, out);
            names(right, out);
        }
        G::Extend {
            inner, variable, ..
        } => {
            out.push(format!("?{}", variable.as_str()));
            names(inner, out);
        }
        G::Project { variables, .. } => {
            out.extend(variables.iter().map(|v| format!("?{}", v.as_str())));
        }
        G::Filter { inner, .. }
        | G::Graph { inner, .. }
        | G::OrderBy { inner, .. }
        | G::Distinct { inner }
        | G::Reduced { inner }
        | G::Slice { inner, .. }
        | G::Group { inner, .. }
        | G::Service { inner, .. } => names(inner, out),
        G::Values { variables, .. } => {
            out.extend(variables.iter().map(|v| format!("?{}", v.as_str())));
        }
    }
}

/// The optimizer plans an EXISTS pattern without knowing which variables the
/// solution being filtered binds, and evaluates it with hash joins: every
/// part runs on its own, a `rdfs:subClassOf*` over the whole graph. Here the
/// triples and paths of a plain EXISTS pattern are put in an order that
/// starts from what the enclosing pattern binds and chained as nested loops
/// (for triples and paths, a lateral join gives the same solutions as a join).
fn order_exists(p: &mut GraphPattern, outer: &[Name]) {
    use GraphPattern as G;
    let scope = |inner: &G| {
        let mut bound = outer.to_vec();
        names(inner, &mut bound);
        bound
    };
    match p {
        G::Filter { expr, inner } => {
            let bound = scope(inner);
            order_exists_expr(expr, &bound);
            order_exists(inner, outer);
        }
        G::LeftJoin {
            left,
            right,
            expression,
        } => {
            if let Some(e) = expression {
                let mut bound = scope(left);
                names(right, &mut bound);
                order_exists_expr(e, &bound);
            }
            order_exists(left, outer);
            order_exists(right, outer);
        }
        G::Join { left, right }
        | G::Lateral { left, right }
        | G::Union { left, right }
        | G::Minus { left, right } => {
            order_exists(left, outer);
            order_exists(right, outer);
        }
        G::Extend {
            inner, expression, ..
        } => {
            let bound = scope(inner);
            order_exists_expr(expression, &bound);
            order_exists(inner, outer);
        }
        G::Graph { inner, .. }
        | G::OrderBy { inner, .. }
        | G::Project { inner, .. }
        | G::Distinct { inner }
        | G::Reduced { inner }
        | G::Slice { inner, .. }
        | G::Group { inner, .. } => order_exists(inner, outer),
        G::Bgp { .. } | G::Path { .. } | G::Values { .. } | G::Service { .. } => {}
    }
}

fn order_exists_expr(e: &mut Expression, bound: &[Name]) {
    use Expression as E;
    match e {
        E::Exists(p) => {
            let mut inner_bound = bound.to_vec();
            names(p, &mut inner_bound);
            order_exists(p, &inner_bound);
            match p.as_mut() {
                GraphPattern::Filter { inner, .. } => order_plain(inner, bound),
                other => order_plain(other, bound),
            }
        }
        E::Or(a, b)
        | E::And(a, b)
        | E::Equal(a, b)
        | E::SameTerm(a, b)
        | E::Greater(a, b)
        | E::GreaterOrEqual(a, b)
        | E::Less(a, b)
        | E::LessOrEqual(a, b)
        | E::Add(a, b)
        | E::Subtract(a, b)
        | E::Multiply(a, b)
        | E::Divide(a, b) => {
            order_exists_expr(a, bound);
            order_exists_expr(b, bound);
        }
        E::UnaryPlus(a) | E::UnaryMinus(a) | E::Not(a) => order_exists_expr(a, bound),
        E::If(a, b, c) => {
            order_exists_expr(a, bound);
            order_exists_expr(b, bound);
            order_exists_expr(c, bound);
        }
        E::In(a, list) => {
            order_exists_expr(a, bound);
            list.iter_mut().for_each(|x| order_exists_expr(x, bound));
        }
        E::Coalesce(list) | E::FunctionCall(_, list) => {
            list.iter_mut().for_each(|x| order_exists_expr(x, bound));
        }
        _ => {}
    }
}

/// Reorders `p` when it is made of triples and paths only.
fn order_plain(p: &mut GraphPattern, bound: &[Name]) {
    fn flatten(p: &GraphPattern, out: &mut Vec<GraphPattern>) -> bool {
        match p {
            GraphPattern::Bgp { patterns } => {
                out.extend(patterns.iter().map(|tp| GraphPattern::Bgp {
                    patterns: vec![tp.clone()],
                }));
                true
            }
            GraphPattern::Path { .. } => {
                out.push(p.clone());
                true
            }
            GraphPattern::Join { left, right } => flatten(left, out) && flatten(right, out),
            _ => false,
        }
    }
    let mut items = Vec::new();
    if !flatten(p, &mut items) || items.len() < 2 {
        return;
    }
    let mut bound = bound.to_vec();
    let mut chain: Option<GraphPattern> = None;
    while !items.is_empty() {
        let is_bound = |t: &TermPattern| term_name(t).is_none_or(|n| bound.contains(&n));
        let cost = |item: &GraphPattern| match item {
            GraphPattern::Bgp { patterns } => {
                let tp = &patterns[0];
                let p_bound = match &tp.predicate {
                    NamedNodePattern::NamedNode(_) => true,
                    NamedNodePattern::Variable(v) => bound.contains(&format!("?{}", v.as_str())),
                };
                match (is_bound(&tp.subject), p_bound, is_bound(&tp.object)) {
                    (true, true, true) => 1,
                    (true, true, false) => 10,
                    (true, false, true) => 2,
                    (false, true, true) => 10_000,
                    (true, false, false) => 100,
                    (false, false, true) => 100_000,
                    (false, true, false) => 1_000_000,
                    (false, false, false) => 1_000_000_000,
                }
            }
            GraphPattern::Path {
                subject, object, ..
            } => match (is_bound(subject), is_bound(object)) {
                (true, true) => 1,
                (true, false) | (false, true) => 1_000,
                (false, false) => 1_000_000_000,
            },
            _ => usize::MAX,
        };
        let next = (0..items.len())
            .min_by_key(|&i| cost(&items[i]))
            .unwrap_or(0);
        let item = items.remove(next);
        names(&item, &mut bound);
        chain = Some(match chain {
            None => item,
            Some(left) => GraphPattern::Lateral {
                left: Box::new(left),
                right: Box::new(item),
            },
        });
    }
    if let Some(c) = chain {
        *p = c;
    }
}

struct Substitution<'b>(&'b [(Variable, Term)]);

impl Substitution<'_> {
    fn get(&self, v: &Variable) -> Option<&Term> {
        self.0.iter().find(|b| b.0 == *v).map(|b| &b.1)
    }

    /// The value as a constant in a triple pattern.
    fn iri_for(t: &Term) -> NamedNode {
        match t {
            Term::NamedNode(n) => n.clone(),
            Term::BlankNode(b) => NamedNode::new_unchecked(format!("{BLANK}{}", b.as_str())),
            // Matches nothing: no predicate or graph name is a literal.
            _ => NamedNode::new_unchecked(format!("{BLANK}-")),
        }
    }

    fn expr_for(t: &Term) -> Expression {
        match t {
            Term::NamedNode(n) => Expression::NamedNode(n.clone()),
            Term::Literal(l) => Expression::Literal(l.clone()),
            Term::BlankNode(b) => Expression::FunctionCall(
                Function::Custom(NamedNode::new_unchecked(BLANK)),
                vec![Expression::Literal(Literal::new_simple_literal(b.as_str()))],
            ),
            Term::Triple(_) => unreachable!("triples are not substituted"),
        }
    }

    fn term(&self, t: &mut TermPattern) {
        match t {
            TermPattern::Variable(v) => {
                if let Some(value) = self.get(v) {
                    *t = match value {
                        Term::Literal(l) => TermPattern::Literal(l.clone()),
                        other => TermPattern::NamedNode(Self::iri_for(other)),
                    };
                }
            }
            TermPattern::Triple(tp) => self.triple(tp),
            _ => {}
        }
    }

    fn named(&self, n: &mut NamedNodePattern) {
        if let NamedNodePattern::Variable(v) = n
            && let Some(value) = self.get(v)
        {
            *n = NamedNodePattern::NamedNode(Self::iri_for(value));
        }
    }

    fn triple(&self, tp: &mut TriplePattern) {
        self.term(&mut tp.subject);
        self.named(&mut tp.predicate);
        self.term(&mut tp.object);
    }

    fn unbind(&self, vars: &mut Vec<Variable>) {
        vars.retain(|v| self.get(v).is_none());
    }

    /// False when `p` holds a construct this does not handle.
    #[must_use]
    fn pattern(&self, p: &mut GraphPattern) -> bool {
        use GraphPattern as G;
        match p {
            G::Bgp { patterns } => patterns.iter_mut().for_each(|tp| self.triple(tp)),
            G::Path {
                subject, object, ..
            } => {
                self.term(subject);
                self.term(object);
            }
            G::Join { left, right }
            | G::Lateral { left, right }
            | G::Union { left, right }
            | G::Minus { left, right } => {
                return self.pattern(left) && self.pattern(right);
            }
            G::LeftJoin {
                left,
                right,
                expression,
            } => {
                return self.pattern(left)
                    && self.pattern(right)
                    && expression.as_mut().is_none_or(|e| self.expr(e));
            }
            G::Filter { expr, inner } => return self.expr(expr) && self.pattern(inner),
            G::Graph { name, inner } => {
                self.named(name);
                return self.pattern(inner);
            }
            G::Extend {
                inner,
                variable,
                expression,
            } => {
                return self.get(variable).is_none()
                    && self.expr(expression)
                    && self.pattern(inner);
            }
            G::OrderBy { inner, expression } => {
                return expression.iter_mut().all(|o| match o {
                    OrderExpression::Asc(e) | OrderExpression::Desc(e) => self.expr(e),
                }) && self.pattern(inner);
            }
            G::Project { inner, variables } => {
                self.unbind(variables);
                return self.pattern(inner);
            }
            G::Distinct { inner } | G::Reduced { inner } | G::Slice { inner, .. } => {
                return self.pattern(inner);
            }
            G::Group {
                inner,
                variables,
                aggregates,
            } => {
                if aggregates.iter().any(|(v, _)| self.get(v).is_some())
                    || !aggregates.iter_mut().all(|(_, a)| match a {
                        AggregateExpression::CountSolutions { .. } => true,
                        AggregateExpression::FunctionCall { expr, .. } => self.expr(expr),
                    })
                    || !self.pattern(inner)
                {
                    return false;
                }
                // A key bound to the value keeps "no solutions, no groups".
                for v in variables.iter() {
                    if let Some(value) = self.get(v) {
                        let body = std::mem::replace(inner.as_mut(), G::Bgp { patterns: vec![] });
                        **inner = G::Extend {
                            inner: Box::new(body),
                            variable: v.clone(),
                            expression: Self::expr_for(value),
                        };
                    }
                }
            }
            G::Values { variables, .. } => return variables.iter().all(|v| self.get(v).is_none()),
            #[allow(unreachable_patterns)]
            _ => return false,
        }
        true
    }

    #[must_use]
    fn expr(&self, e: &mut Expression) -> bool {
        use Expression as E;
        match e {
            E::Variable(v) => {
                if let Some(value) = self.get(v) {
                    *e = Self::expr_for(value);
                }
            }
            E::Bound(v) => {
                if self.get(v).is_some() {
                    *e = E::Literal(Literal::from(true));
                }
            }
            E::Exists(p) => return self.pattern(p),
            E::Or(a, b)
            | E::And(a, b)
            | E::Equal(a, b)
            | E::SameTerm(a, b)
            | E::Greater(a, b)
            | E::GreaterOrEqual(a, b)
            | E::Less(a, b)
            | E::LessOrEqual(a, b)
            | E::Add(a, b)
            | E::Subtract(a, b)
            | E::Multiply(a, b)
            | E::Divide(a, b) => return self.expr(a) && self.expr(b),
            E::UnaryPlus(a) | E::UnaryMinus(a) | E::Not(a) => return self.expr(a),
            E::In(a, list) => return self.expr(a) && list.iter_mut().all(|x| self.expr(x)),
            E::If(a, b, c) => return self.expr(a) && self.expr(b) && self.expr(c),
            E::Coalesce(list) | E::FunctionCall(_, list) => {
                return list.iter_mut().all(|x| self.expr(x));
            }
            E::NamedNode(_) | E::Literal(_) => {}
            #[allow(unreachable_patterns)]
            _ => return false,
        }
        true
    }
}

/// `template` with each `{?name}` / `{$name}` replaced by the value `lookup`
/// gives (IRIs in full, literals by their lexical form).
pub fn fill(template: &str, lookup: &dyn Fn(&str) -> Option<Term>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let name_end = after.find('}');
        let filled = match (after.chars().next(), name_end) {
            (Some('?' | '$'), Some(end)) => lookup(&after[1..end]).map(|t| (display(&t), end)),
            _ => None,
        };
        match filled {
            Some((text, end)) => {
                out.push_str(&text);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn display(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRE: PreBound = PreBound {
        names: &["this", "shapesGraph", "currentShape"],
        required: &["this"],
    };

    #[test]
    fn restrictions() {
        let bad = |q: &str| prepare(q, &[], None, &PRE).is_err();
        assert!(bad(
            "SELECT $this WHERE { $this ?p ?o MINUS { $this ?p 1 } }"
        ));
        assert!(bad("SELECT $this WHERE { VALUES ?x { 1 } }"));
        let lateral = "SELECT $this WHERE { $this ?p ?o LATERAL { SELECT $this ?o WHERE { $this ?q ?o } LIMIT 1 } }";
        assert_eq!(
            prepare(lateral, &[], None, &PRE).err().as_deref(),
            Some("LATERAL is not standard SPARQL")
        );
        assert!(bad("SELECT $this WHERE { BIND (1 AS $this) }"));
        assert!(bad(
            "SELECT $this WHERE { { SELECT ?o WHERE { ?s ?p ?o } } }"
        ));
        assert!(bad(
            "SELECT $this WHERE { FILTER EXISTS { SERVICE <http://e/> { ?a ?b ?c } } }"
        ));
        assert!(bad("SELECT $this WHERE { ?s ex:p ?o }"));
        assert!(!bad(
            "SELECT $this WHERE { { SELECT $this WHERE { $this ?p ?o } } }"
        ));
        assert!(
            prepare(
                "SELECT $this WHERE { $this ex:p ?o }",
                &[("ex".into(), "http://e/".into())],
                None,
                &PRE
            )
            .is_ok()
        );
    }

    #[test]
    fn paths_and_templates() {
        let p = |l: &str| Path::Predicate(Iri::intern(&format!("http://e/{l}")));
        let path = Path::Sequence(vec![p("a"), Path::Inverse(Box::new(p("b")))]);
        assert_eq!(
            replace_path("$this $PATH ?v . $PATHS", &path_sparql(&path)),
            "$this (<http://e/a>/^<http://e/b>) ?v . $PATHS"
        );
        let lookup = |n: &str| (n == "v").then(|| Term::from(Literal::from("x")));
        assert_eq!(fill("a {?v} {$v} {?w} {", &lookup), "a x x {?w} {");
    }
}
