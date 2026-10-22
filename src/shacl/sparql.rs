//! SHACL-SPARQL: preparing the queries of SPARQL-based constraints,
//! constraint components and targets, and running them with `spareval` over
//! the validation [`Graph`](crate::rdf::Graph) in place (through [`Index`], no copy of the data).
//!
//! Pre-binding follows the SHACL specification: every occurrence of a
//! pre-bound variable is replaced by its value (see [`Engine`]), and the
//! constructs that would break that (MINUS, VALUES, SERVICE, `AS ?var` for a
//! pre-bound variable, a subquery that does not return the pre-bound
//! variables) are rejected. `$PATH` is replaced by the shape's path in SPARQL
//! syntax before parsing.
//!
//! The data graph is the default graph; the same graph is also the named
//! graph `$shapesGraph` names ([`SHAPES_GRAPH`]).

use super::Path;
use crate::clock::Instant;
use crate::rdf::dataset::{BLANK, Index, STerm};
use crate::rdf::graph::Id;
use rustc_hash::{FxHashMap, FxHashSet};
use spareval::{
    CancellationToken, QueryEvaluationError, QueryEvaluator, QueryResults, QuerySolution,
};
use spargebra::algebra::{
    AggregateExpression, Expression, Function, GraphPattern, OrderExpression,
};
use spargebra::term::{
    BlankNode, GroundTerm, Literal, NamedNode, NamedNodePattern, Term, TermPattern, TriplePattern,
    Variable,
};
use spargebra::{Query, SparqlParser};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

/// The IRI `$shapesGraph` is bound to.
pub const SHAPES_GRAPH: &str = "urn:x-shacl:shapes-graph";

/// How long one SHACL-SPARQL query may run (in unit tests, long enough to
/// tell a stop from a run of minutes).
pub const TIME_LIMIT: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(10)
};

/// The failure of a query stopped at [`TIME_LIMIT`].
pub const TIMED_OUT: &str = "SHACL-SPARQL query stopped after 10 s, its time limit: the results of this constraint are missing";

/// What a query is for, which decides its pre-bound variables and the
/// restrictions on it.
pub enum Role {
    /// A SPARQL-based target: only `$shapesGraph` is pre-bound, as a constant,
    /// so none of the restrictions that keep `$this` and the other
    /// pre-bound variables meaningful apply.
    Target,
    /// A constraint or validator: these variables are pre-bound. A subquery
    /// must return all but `currentShape` and `shapesGraph`.
    PreBound(Vec<String>),
}

impl Role {
    /// The variables pre-bound in a query of this role.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Role::Target => vec!["shapesGraph"],
            Role::PreBound(names) => names.iter().map(String::as_str).collect(),
        }
    }
}

/// The pre-bound variables of a SPARQL constraint (`sh:sparql`).
pub fn constraint_role() -> Role {
    Role::PreBound(
        ["this", "shapesGraph", "currentShape"]
            .map(String::from)
            .into(),
    )
}

/// Parses `text` with `prefixes` and checks it against the SHACL-SPARQL
/// restrictions; `$PATH` becomes `path`. The top-level projection is extended
/// with every pre-bound variable so the bindings reach the whole query.
pub fn prepare(
    text: &str,
    prefixes: &[(String, String)],
    path: Option<&Path>,
    role: &Role,
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
    if let Some(why) = crate::sparql::too_complex(&text) {
        return Err(why);
    }
    let text =
        crate::sparql::validate::text_for_spargebra(&text, &crate::sparql::lexer::tokenize(&text));
    let mut query = parser.parse_query(&text).map_err(|e| e.to_string())?;
    let vars: Vec<Variable> = role
        .names()
        .into_iter()
        .map(Variable::new_unchecked)
        .collect();
    if let Some(why) = restrictions(&query, role).first() {
        return Err(why);
    }
    match &mut query {
        // Both have the projection on top (ASK projects its in-scope variables).
        Query::Select { pattern, .. } | Query::Ask { pattern, .. } => project(pattern, &vars),
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

/// What the restrictions on queries with pre-bound variables rule out in a
/// query (see [`restrictions`]).
#[derive(Default)]
pub struct Found {
    pub minus: bool,
    pub values: bool,
    pub service: bool,
    pub lateral: bool,
    /// Pre-bound variables assigned with `AS`.
    pub assigned: Vec<String>,
    /// Subqueries so far.
    pub subqueries: usize,
    /// Subqueries (by number) that leave out a pre-bound variable.
    pub dropped: Vec<(usize, String)>,
}

impl Found {
    /// The first problem, as a failure message.
    pub fn first(&self) -> Option<String> {
        let words = [
            (self.minus, "MINUS"),
            (self.values, "VALUES"),
            (self.service, "SERVICE"),
        ];
        if let Some((_, w)) = words.iter().find(|w| w.0) {
            return Some(format!("{w} is not allowed with pre-bound variables"));
        }
        if self.lateral {
            return Some("LATERAL is not standard SPARQL".into());
        }
        if let Some(v) = self.assigned.first() {
            return Some(format!("AS ?{v} assigns a pre-bound variable"));
        }
        let (_, v) = self.dropped.first()?;
        Some(format!("a subquery must return ${v}"))
    }
}

/// The SHACL-SPARQL restrictions on queries with pre-bound variables, shared
/// by validation and the lint: MINUS, VALUES, SERVICE and LATERAL, `AS ?var`
/// for a pre-bound variable, and a subquery that does not return the
/// pre-bound variables. A [`Role::Target`] has none.
pub fn restrictions(q: &Query, role: &Role) -> Found {
    let mut f = Found::default();
    let Role::PreBound(vars) = role else {
        return f;
    };
    let (Query::Select { pattern, .. }
    | Query::Ask { pattern, .. }
    | Query::Construct { pattern, .. }
    | Query::Describe { pattern, .. }) = q;
    walk(pattern, true, vars, &mut f);
    f
}

fn walk(p: &GraphPattern, top: bool, vars: &[String], f: &mut Found) {
    match p {
        GraphPattern::Bgp { .. } | GraphPattern::Path { .. } => {}
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } => {
            walk(left, false, vars, f);
            walk(right, false, vars, f);
        }
        GraphPattern::Lateral { left, right } => {
            f.lateral = true;
            walk(left, false, vars, f);
            walk(right, false, vars, f);
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            walk(left, false, vars, f);
            walk(right, false, vars, f);
            if let Some(e) = expression {
                expr(e, vars, f);
            }
        }
        GraphPattern::Filter { expr: e, inner } => {
            expr(e, vars, f);
            walk(inner, false, vars, f);
        }
        GraphPattern::Graph { inner, .. } => walk(inner, false, vars, f),
        GraphPattern::Extend {
            inner,
            variable,
            expression,
        } => {
            if vars.iter().any(|v| v == variable.as_str())
                && !f.assigned.iter().any(|v| v == variable.as_str())
            {
                f.assigned.push(variable.as_str().to_owned());
            }
            expr(expression, vars, f);
            walk(inner, top, vars, f);
        }
        GraphPattern::Minus { left, right } => {
            f.minus = true;
            walk(left, false, vars, f);
            walk(right, false, vars, f);
        }
        GraphPattern::Values { .. } => f.values = true,
        GraphPattern::OrderBy { inner, expression } => {
            for o in expression {
                let (OrderExpression::Asc(e) | OrderExpression::Desc(e)) = o;
                expr(e, vars, f);
            }
            walk(inner, top, vars, f);
        }
        GraphPattern::Project { inner, variables } => {
            if !top {
                if let Some(v) = vars
                    .iter()
                    .filter(|v| *v != "shapesGraph" && *v != "currentShape")
                    .find(|v| !variables.iter().any(|x| x.as_str() == v.as_str()))
                {
                    f.dropped.push((f.subqueries, v.clone()));
                }
                f.subqueries += 1;
            }
            walk(inner, false, vars, f);
        }
        GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => {
            walk(inner, top, vars, f);
        }
        GraphPattern::Group {
            inner, aggregates, ..
        } => {
            for (_, a) in aggregates {
                if let AggregateExpression::FunctionCall { expr: e, .. } = a {
                    expr(e, vars, f);
                }
            }
            walk(inner, top, vars, f);
        }
        GraphPattern::Service { inner, .. } => {
            f.service = true;
            walk(inner, false, vars, f);
        }
    }
}

/// Graph patterns inside expressions (`EXISTS`, `NOT EXISTS`).
fn expr(e: &Expression, vars: &[String], f: &mut Found) {
    match e {
        Expression::Exists(p) => walk(p, false, vars, f),
        Expression::Or(a, b)
        | Expression::And(a, b)
        | Expression::Equal(a, b)
        | Expression::SameTerm(a, b)
        | Expression::Greater(a, b)
        | Expression::GreaterOrEqual(a, b)
        | Expression::Less(a, b)
        | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b)
        | Expression::Subtract(a, b)
        | Expression::Multiply(a, b)
        | Expression::Divide(a, b) => {
            expr(a, vars, f);
            expr(b, vars, f);
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
            expr(a, vars, f);
        }
        Expression::In(a, list) => {
            expr(a, vars, f);
            list.iter().for_each(|x| expr(x, vars, f));
        }
        Expression::If(a, b, c) => {
            expr(a, vars, f);
            expr(b, vars, f);
            expr(c, vars, f);
        }
        Expression::Coalesce(list) | Expression::FunctionCall(_, list) => {
            list.iter().for_each(|x| expr(x, vars, f));
        }
        _ => {}
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

/// The job's cancel check, polled while a query runs.
pub type Cancel = Arc<dyn Fn() -> bool + Send + Sync>;

/// What a query returned.
pub enum Answer {
    /// At most the `max` solutions asked for; `more` when there were more.
    Solutions {
        rows: Vec<QuerySolution>,
        more: bool,
    },
    Boolean(bool),
}

/// Why a query gave no answer.
pub enum Fail {
    /// The job's cancel check said so.
    Cancelled,
    /// A batch has more solutions than were asked for.
    TooMany,
    /// A failure to report, [`TIMED_OUT`] among them.
    Failed(String),
}

/// Runs queries with pre-bound variables. The pre-bound values are written
/// into the query as constants so the optimizer can plan the query around
/// them; queries whose values cannot be written as constants (RDF-star
/// triples) run unoptimized with the values as initial bindings. Planning
/// costs more than running a typical check on one focus node, so
/// [`Engine::run_batch`] plans a check once for many focus nodes.
pub struct Engine {
    optimized: QueryEvaluator,
    plain: QueryEvaluator,
    /// The queries that hit [`TIME_LIMIT`]: they fail at once from then on,
    /// so a check does not take the limit again for every focus node.
    timed_out: RefCell<FxHashSet<Query>>,
    /// Stops a running query, beside the time limit.
    cancel: Cancel,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new(Arc::new(|| false))
    }
}

impl Engine {
    /// An engine whose queries also stop once `cancel` says so.
    pub fn new(cancel: Cancel) -> Engine {
        Engine {
            optimized: QueryEvaluator::new().with_custom_function(
                NamedNode::new_unchecked(BLANK),
                |args: &[Term]| match args {
                    [Term::Literal(l)] => Some(BlankNode::new_unchecked(l.value()).into()),
                    _ => None,
                },
            ),
            plain: QueryEvaluator::new().without_optimizations(),
            timed_out: RefCell::default(),
            cancel,
        }
    }
}

/// A query's stop: at [`TIME_LIMIT`], or when the job is cancelled.
struct Limit {
    /// Whether the cancel check, not the clock, stopped the query.
    by_cancel: Arc<AtomicBool>,
}

impl Engine {
    /// `evaluator`, stopped at [`TIME_LIMIT`] from now or when the job is
    /// cancelled (each checked every 64th time the evaluation asks).
    fn limited(&self, evaluator: &QueryEvaluator) -> (QueryEvaluator, Limit) {
        let deadline = Instant::now() + TIME_LIMIT;
        let checks = AtomicU32::new(0);
        let cancel = self.cancel.clone();
        let by_cancel = Arc::new(AtomicBool::new(false));
        let flag = by_cancel.clone();
        let limited = evaluator
            .clone()
            .with_cancellation_token(CancellationToken::with_check(move || {
                if !checks.fetch_add(1, Ordering::Relaxed).is_multiple_of(64) {
                    return false;
                }
                if cancel() {
                    flag.store(true, Ordering::Relaxed);
                    return true;
                }
                Instant::now() >= deadline
            }));
        (limited, Limit { by_cancel })
    }

    /// Runs `query` over `ix` with `bindings` pre-bound, keeping at most
    /// `max` solutions (it stops reading at the one after).
    pub fn run(
        &self,
        ix: &Index,
        query: &Query,
        bindings: Vec<(Variable, Term)>,
        max: usize,
    ) -> Result<Answer, Fail> {
        if self.timed_out(query) {
            return Err(Fail::Failed(TIMED_OUT.into()));
        }
        let substituted = substitute(query, &bindings, None);
        let evaluator;
        let limit;
        let prepared = match &substituted {
            Some(q) => {
                (evaluator, limit) = self.limited(&self.optimized);
                evaluator.prepare(q)
            }
            None => {
                (evaluator, limit) = self.limited(&self.plain);
                let mut prepared = evaluator.prepare(query);
                for (v, t) in bindings {
                    prepared = prepared.substitute_variable(v, t);
                }
                prepared
            }
        };
        let fail = |e: QueryEvaluationError| self.error(Some(query), e, &limit);
        match prepared.execute(ix).map_err(fail)? {
            QueryResults::Solutions(s) => {
                let mut rows = Vec::new();
                for sol in s {
                    if rows.len() == max {
                        return Ok(Answer::Solutions { rows, more: true });
                    }
                    rows.push(sol.map_err(fail)?);
                }
                Ok(Answer::Solutions { rows, more: false })
            }
            QueryResults::Boolean(b) => Ok(Answer::Boolean(b)),
            QueryResults::Graph(_) => Err(Fail::Failed("not a SELECT or ASK query".into())),
        }
    }

    /// Runs a [`batchable`] query once for many focus nodes: `$this` stays a
    /// variable joined with the focus nodes (a `VALUES` block), the other
    /// pre-bound variables in `bindings` are written in as constants. Each
    /// focus node gets the solutions [`Engine::run`] would give it, without
    /// `$this`. Focus nodes that cannot be passed this way (RDF-star triples)
    /// are left out of the map; `None` when `bindings` cannot be substituted.
    /// More than `max` solutions in all is [`Fail::TooMany`].
    pub fn run_batch(
        &self,
        ix: &Index,
        query: &Query,
        bindings: &[(Variable, Term)],
        focus: &[Id],
        max: usize,
    ) -> Option<Result<ByFocus, Fail>> {
        if self.timed_out(query) {
            return Some(Err(Fail::Failed(TIMED_OUT.into())));
        }
        let this = Variable::new_unchecked("this");
        let mut out = ByFocus::default();
        let mut rows = Vec::new();
        for &f in focus {
            let value = match ix.term(f) {
                Term::NamedNode(n) => GroundTerm::NamedNode(n),
                Term::Literal(l) => GroundTerm::Literal(l),
                Term::BlankNode(b) => GroundTerm::NamedNode(Substitution::iri_for(&b.into())),
                Term::Triple(_) => continue,
            };
            let back = match &value {
                GroundTerm::NamedNode(n) => ix.internal(&n.clone().into()),
                GroundTerm::Literal(l) => ix.internal(&l.clone().into()),
                GroundTerm::Triple(_) => continue,
            };
            if back == STerm::Id(f) && out.insert(f, Vec::new()).is_none() {
                rows.push(vec![Some(value)]);
            }
        }
        if rows.is_empty() {
            return Some(Ok(out));
        }
        let query = substitute(query, bindings, Some((&this, &rows)))?;
        let (evaluator, limit) = self.limited(&self.optimized);
        let solutions = match evaluator.prepare(&query).execute(ix) {
            Ok(QueryResults::Solutions(s)) => s,
            Ok(_) => return Some(Err(Fail::Failed("not a SELECT query".into()))),
            Err(e) => return Some(Err(self.error(None, e, &limit))),
        };
        let mut vars: Option<Arc<[Variable]>> = None;
        let mut count = 0;
        for sol in solutions {
            let sol = match sol {
                Ok(s) => s,
                Err(e) => return Some(Err(self.error(None, e, &limit))),
            };
            count += 1;
            if count > max {
                return Some(Err(Fail::TooMany));
            }
            let Some(list) = sol
                .get(&this)
                .and_then(|t| ix.id(t))
                .and_then(|f| out.get_mut(&f))
            else {
                return Some(Err(Fail::Failed("a solution without a focus node".into())));
            };
            let vars = vars.get_or_insert_with(|| {
                sol.variables()
                    .iter()
                    .filter(|v| **v != this)
                    .cloned()
                    .collect()
            });
            let values: Vec<Option<Term>> = sol
                .variables()
                .iter()
                .zip(sol.values())
                .filter(|(v, _)| **v != this)
                .map(|(_, t)| t.clone())
                .collect();
            list.push(QuerySolution::from((Arc::clone(vars), values)));
        }
        Some(Ok(out))
    }

    /// Whether `query` hit the time limit before (no hashing while none did).
    fn timed_out(&self, query: &Query) -> bool {
        let set = self.timed_out.borrow();
        !set.is_empty() && set.contains(query)
    }

    /// The failure `e` of `query`, or of a batch when `None`; a stop is the
    /// job's cancellation or the time limit, the only things that cancel
    /// these queries. A batch that stops leaves its query unmarked: each
    /// focus node may be fast alone.
    fn error(&self, query: Option<&Query>, e: QueryEvaluationError, limit: &Limit) -> Fail {
        match e {
            QueryEvaluationError::Cancelled if limit.by_cancel.load(Ordering::Relaxed) => {
                Fail::Cancelled
            }
            QueryEvaluationError::Cancelled => {
                if let Some(q) = query {
                    self.timed_out.borrow_mut().insert(q.clone());
                }
                Fail::Failed(TIMED_OUT.into())
            }
            e => Fail::Failed(e.to_string()),
        }
    }
}

/// Solutions by focus node.
pub type ByFocus = FxHashMap<Id, Vec<QuerySolution>>;

type Rows = Vec<Vec<Option<GroundTerm>>>;

/// Whether `query` can run once for many focus nodes ([`Engine::run_batch`])
/// with the same solutions per focus node as with `$this` pre-bound: the
/// focus nodes, as a `VALUES` block, can be joined in ([`push`]).
pub fn batchable(query: &Query) -> bool {
    match query {
        Query::Select { pattern, .. } => {
            push(&mut pattern.clone(), &Variable::new_unchecked("this"), None)
        }
        _ => false,
    }
}

/// Joins `p` with `rows` (values of `v`) so that the result equals `p` with
/// each value written in for `v`, joined with that value. The join goes
/// around the largest part that is [`joinable`] as it is (evaluated once,
/// then matched with the values), else deeper, below a FILTER, BIND or
/// OPTIONAL that then sees `v` bound. `None` checks only; false (and `p`
/// unchanged) when there is no such place.
fn push(p: &mut GraphPattern, v: &Variable, rows: Option<&Rows>) -> bool {
    use GraphPattern as G;
    if joinable(p, v) {
        if let Some(rows) = rows {
            let body = std::mem::replace(p, G::Bgp { patterns: vec![] });
            *p = G::Join {
                left: Box::new(G::Values {
                    variables: vec![v.clone()],
                    bindings: rows.clone(),
                }),
                right: Box::new(body),
            };
        }
        return true;
    }
    match p {
        G::Filter { expr, inner } => expr_joinable(expr, v) && push(inner, v, rows),
        G::Extend {
            inner,
            variable,
            expression,
        } => variable != v && expr_joinable(expression, v) && push(inner, v, rows),
        G::LeftJoin {
            left,
            right,
            expression,
        } => {
            joinable(right, v)
                && expression.as_ref().is_none_or(|e| expr_joinable(e, v))
                && push(left, v, rows)
        }
        G::Join { left, right } => {
            if joinable(right, v) && push(left, v, None) {
                push(left, v, rows)
            } else {
                joinable(left, v) && push(right, v, rows)
            }
        }
        G::Union { left, right } => {
            push(left, v, None) && push(right, v, None) && {
                push(left, v, rows);
                push(right, v, rows)
            }
        }
        G::Graph { name, inner } => {
            !matches!(name, NamedNodePattern::Variable(n) if n == v) && push(inner, v, rows)
        }
        G::OrderBy { inner, expression } => {
            expression.iter().all(|o| match o {
                OrderExpression::Asc(e) | OrderExpression::Desc(e) => expr_joinable(e, v),
            }) && push(inner, v, rows)
        }
        G::Project { inner, variables } => variables.contains(v) && push(inner, v, rows),
        G::Distinct { inner } | G::Reduced { inner } => push(inner, v, rows),
        G::Group {
            inner,
            variables,
            aggregates,
        } => {
            variables.contains(v)
                && aggregates.iter().all(|(_, a)| match a {
                    AggregateExpression::CountSolutions { .. } => true,
                    AggregateExpression::FunctionCall { expr, .. } => expr_joinable(expr, v),
                })
                && push(inner, v, rows)
        }
        _ => false,
    }
}

/// Every solution of `p` binds `v`.
fn binds(p: &GraphPattern, v: &Variable) -> bool {
    use GraphPattern as G;
    match p {
        G::Bgp { patterns } => patterns.iter().any(|tp| triple_mentions(tp, v)),
        G::Path {
            subject, object, ..
        } => term_mentions(subject, v) || term_mentions(object, v),
        G::Join { left, right } | G::Lateral { left, right } => binds(left, v) || binds(right, v),
        G::Union { left, right } => binds(left, v) && binds(right, v),
        G::LeftJoin { left, .. } | G::Minus { left, .. } => binds(left, v),
        G::Graph { name, inner } => {
            matches!(name, NamedNodePattern::Variable(n) if n == v) || binds(inner, v)
        }
        G::Project { inner, variables } => variables.contains(v) && binds(inner, v),
        G::Group {
            inner, variables, ..
        } => variables.contains(v) && binds(inner, v),
        G::Filter { inner, .. }
        | G::Extend { inner, .. }
        | G::OrderBy { inner, .. }
        | G::Distinct { inner }
        | G::Reduced { inner }
        | G::Slice { inner, .. } => binds(inner, v),
        G::Values { .. } | G::Service { .. } => false,
    }
}

/// `p` joined with a binding of `v` gives the solutions of `p` with `v`
/// written in, plus that binding.
fn joinable(p: &GraphPattern, v: &Variable) -> bool {
    use GraphPattern as G;
    if !mentions(p, v) {
        return true;
    }
    match p {
        G::Bgp { .. } | G::Path { .. } => true,
        G::Join { left, right } | G::Union { left, right } => {
            joinable(left, v) && joinable(right, v)
        }
        G::LeftJoin {
            left,
            right,
            expression,
        } => {
            binds(left, v)
                && joinable(left, v)
                && joinable(right, v)
                && expression.as_ref().is_none_or(|e| expr_joinable(e, v))
        }
        G::Filter { expr, inner } => {
            (binds(inner, v) || !expr_mentions(expr, v))
                && expr_joinable(expr, v)
                && joinable(inner, v)
        }
        G::Extend {
            inner,
            variable,
            expression,
        } => {
            variable != v
                && (binds(inner, v) || !expr_mentions(expression, v))
                && expr_joinable(expression, v)
                && joinable(inner, v)
        }
        G::OrderBy { inner, expression } => {
            binds(inner, v)
                && expression.iter().all(|o| match o {
                    OrderExpression::Asc(e) | OrderExpression::Desc(e) => expr_joinable(e, v),
                })
                && joinable(inner, v)
        }
        G::Graph { name, inner } => {
            !matches!(name, NamedNodePattern::Variable(n) if n == v) && joinable(inner, v)
        }
        G::Project { inner, variables } => variables.contains(v) && joinable(inner, v),
        G::Distinct { inner } | G::Reduced { inner } => joinable(inner, v),
        G::Group {
            inner,
            variables,
            aggregates,
        } => {
            variables.contains(v)
                && binds(inner, v)
                && joinable(inner, v)
                && aggregates.iter().all(|(_, a)| match a {
                    AggregateExpression::CountSolutions { .. } => true,
                    AggregateExpression::FunctionCall { expr, .. } => expr_joinable(expr, v),
                })
        }
        G::Slice { .. }
        | G::Minus { .. }
        | G::Lateral { .. }
        | G::Values { .. }
        | G::Service { .. } => false,
    }
}

/// `e`, evaluated on solutions that bind `v`, gives the same value with `v`
/// written in: its EXISTS patterns are [`joinable`] (an implementation may
/// join them with the solution rather than substitute it).
fn expr_joinable(e: &Expression, v: &Variable) -> bool {
    let mut ok = true;
    walk_expr(e, &mut |x| {
        if let Expression::Exists(p) = x {
            ok &= joinable(p, v);
        }
    });
    ok
}

fn term_mentions(t: &TermPattern, v: &Variable) -> bool {
    match t {
        TermPattern::Variable(x) => x == v,
        TermPattern::Triple(tp) => triple_mentions(tp, v),
        _ => false,
    }
}

fn triple_mentions(tp: &TriplePattern, v: &Variable) -> bool {
    term_mentions(&tp.subject, v)
        || matches!(&tp.predicate, NamedNodePattern::Variable(x) if x == v)
        || term_mentions(&tp.object, v)
}

/// `p` uses `v` anywhere, subqueries and expressions included.
fn mentions(p: &GraphPattern, v: &Variable) -> bool {
    use GraphPattern as G;
    match p {
        G::Bgp { .. } | G::Path { .. } => binds(p, v),
        G::Join { left, right }
        | G::Lateral { left, right }
        | G::Union { left, right }
        | G::Minus { left, right } => mentions(left, v) || mentions(right, v),
        G::LeftJoin {
            left,
            right,
            expression,
        } => {
            mentions(left, v)
                || mentions(right, v)
                || expression.as_ref().is_some_and(|e| expr_mentions(e, v))
        }
        G::Filter { expr, inner } => expr_mentions(expr, v) || mentions(inner, v),
        G::Extend {
            inner,
            variable,
            expression,
        } => variable == v || expr_mentions(expression, v) || mentions(inner, v),
        G::OrderBy { inner, expression } => {
            mentions(inner, v)
                || expression.iter().any(|o| match o {
                    OrderExpression::Asc(e) | OrderExpression::Desc(e) => expr_mentions(e, v),
                })
        }
        G::Graph { name, inner } => {
            matches!(name, NamedNodePattern::Variable(n) if n == v) || mentions(inner, v)
        }
        G::Service { name, inner, .. } => {
            matches!(name, NamedNodePattern::Variable(n) if n == v) || mentions(inner, v)
        }
        G::Project { inner, variables } => variables.contains(v) || mentions(inner, v),
        G::Distinct { inner } | G::Reduced { inner } | G::Slice { inner, .. } => mentions(inner, v),
        G::Group {
            inner,
            variables,
            aggregates,
        } => {
            variables.contains(v)
                || mentions(inner, v)
                || aggregates.iter().any(|(x, a)| {
                    x == v
                        || match a {
                            AggregateExpression::CountSolutions { .. } => false,
                            AggregateExpression::FunctionCall { expr, .. } => {
                                expr_mentions(expr, v)
                            }
                        }
                })
        }
        G::Values { variables, .. } => variables.contains(v),
    }
}

fn expr_mentions(e: &Expression, v: &Variable) -> bool {
    let mut found = false;
    walk_expr(e, &mut |x| match x {
        Expression::Variable(x) | Expression::Bound(x) => found |= x == v,
        Expression::Exists(p) => found |= mentions(p, v),
        _ => {}
    });
    found
}

/// Calls `f` on `e` and each expression in it (not inside EXISTS patterns).
fn walk_expr(e: &Expression, f: &mut dyn FnMut(&Expression)) {
    use Expression as E;
    f(e);
    match e {
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
            walk_expr(a, f);
            walk_expr(b, f);
        }
        E::UnaryPlus(a) | E::UnaryMinus(a) | E::Not(a) => walk_expr(a, f),
        E::In(a, list) => {
            walk_expr(a, f);
            list.iter().for_each(|x| walk_expr(x, f));
        }
        E::If(a, b, c) => {
            walk_expr(a, f);
            walk_expr(b, f);
            walk_expr(c, f);
        }
        E::Coalesce(list) | E::FunctionCall(_, list) => list.iter().for_each(|x| walk_expr(x, f)),
        _ => {}
    }
}

/// `query` with every occurrence of a pre-bound variable replaced by its
/// value. A projection drops the variable (the caller knows its value), a
/// GROUP BY keeps it as a key bound to the value. `None` when a value or a
/// construct cannot be substituted. With `join`, the pattern is also
/// joined with values of a variable that stays ([`push`]).
fn substitute(
    query: &Query,
    bindings: &[(Variable, Term)],
    join: Option<(&Variable, &Rows)>,
) -> Option<Query> {
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
    let mut outer = Vec::new();
    if let Some((var, rows)) = join {
        if !push(pattern, var, Some(rows)) {
            return None;
        }
        outer.push(format!("?{}", var.as_str()));
    }
    order_exists(pattern, &outer);
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

fn is_type(p: &NamedNodePattern) -> bool {
    matches!(p, NamedNodePattern::NamedNode(n) if n.as_str() == RDF_TYPE)
}

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

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
                    // Classes have many instances: another predicate first.
                    (false, true, true) if is_type(&tp.predicate) => 20_000,
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
    use crate::syntax::Iri;

    #[test]
    fn restricted_queries() {
        let pre = constraint_role();
        let bad = |q: &str| prepare(q, &[], None, &pre).is_err();
        assert!(bad(
            "SELECT $this WHERE { $this ?p ?o MINUS { $this ?p 1 } }"
        ));
        assert!(bad("SELECT $this WHERE { VALUES ?x { 1 } }"));
        let lateral = "SELECT $this WHERE { $this ?p ?o LATERAL { SELECT $this ?o WHERE { $this ?q ?o } LIMIT 1 } }";
        assert_eq!(
            prepare(lateral, &[], None, &pre).err().as_deref(),
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
                &pre
            )
            .is_ok()
        );
        // A target is held to none of it.
        let minus = "SELECT ?this WHERE { ?this ?p ?o MINUS { ?this ?p 1 } }";
        assert!(prepare(minus, &[], None, &Role::Target).is_ok());
        assert!(prepare(minus, &[], None, &pre).is_err());
    }

    #[test]
    fn batchable_queries() {
        let pre = constraint_role();
        let ok = |q: &str| batchable(&prepare(q, &[], None, &pre).unwrap());
        assert!(ok("SELECT $this WHERE { $this <http://e/p> ?o }"));
        assert!(ok(
            "SELECT $this WHERE { FILTER NOT EXISTS { $this <http://e/p> ?o } }"
        ));
        assert!(ok(
            "SELECT $this ?o WHERE { { $this <http://e/p> ?o } UNION { ?o <http://e/p> $this } }"
        ));
        assert!(ok(
            "SELECT $this (COUNT(?o) AS ?n) WHERE { $this <http://e/p> ?o } GROUP BY $this HAVING (?n > 1)"
        ));
        // LIMIT counts all focus nodes together.
        assert!(!ok("SELECT $this WHERE { $this <http://e/p> ?o } LIMIT 1"));
        // The focus nodes are joined in on the left of the OPTIONAL.
        assert!(ok(
            "SELECT $this ?o WHERE { ?o <http://e/p> 1 OPTIONAL { $this <http://e/q> ?o } }"
        ));
        assert!(!ok(
            "SELECT $this ?o WHERE { { SELECT $this ?o WHERE { $this <http://e/q> ?o } LIMIT 1 } }"
        ));
        // An aggregate over all focus nodes at once.
        assert!(!ok(
            "SELECT (COUNT(*) AS ?n) WHERE { $this <http://e/p> ?o } HAVING (COUNT(*) > 1)"
        ));
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
