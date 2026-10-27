//! How wide a query's joins are: `spareval` plans a join in up to quartic
//! time in its width and checks no clock while it plans, so `query::run` and
//! the SHACL-SPARQL validator refuse a query wider than [`MAX_JOIN_WIDTH`]
//! before planning it.

use spargebra::Query;
use spargebra::algebra::{Expression, GraphPattern, OrderExpression, PropertyPathExpression};

/// The most triple patterns, path steps and joined groups one join may hold.
///
/// Planning a join checks no clock. When the patterns form a chain (each
/// shares a variable with the next) it is about cubic in the width; when
/// every pattern shares one variable (a star, or the SPARQL 1.2 reifiers of
/// one triple, `?s ?p ?o ~ ~ ~`) sparopt re-estimates the whole join for
/// every candidate and it is about quartic. Measured with `turtle-lsp
/// query` on the release build (Apple M6, load average 2 to 2.5), one
/// triple in the data, an `ASK` of N patterns, median of 3 runs, in
/// seconds:
///
/// | N        | 50    | 60    | 70    | 80    | 100   | 150   | 199   |
/// |----------|-------|-------|-------|-------|-------|-------|-------|
/// | chain    | 0.004 | 0.005 | 0.006 | 0.009 | 0.014 | 0.04  | 0.09  |
/// | star     | 0.017 | 0.032 | 0.06  | 0.098 | 0.24  | 1.2   | 3.9   |
/// | reifiers | 0.027 | 0.054 | 0.1   | 0.17  | 0.41  | 2.1   | 6.8   |
///
/// A 1 000-step sequence path with `--timeout 1` ran 22 s. The reifiers are
/// the slowest shape and set the limit: 70 plans in about 0.1 s, well below
/// any real query: a hand-written one has tens of patterns. Unions and
/// `OPTIONAL` chains plan in linear time and do not count.
pub(crate) const MAX_JOIN_WIDTH: usize = 70;

/// Why `query` is refused: its planning would run for seconds before the
/// first clock check ([`MAX_JOIN_WIDTH`]).
pub(crate) fn too_wide(query: &Query) -> Option<String> {
    let (Query::Select { pattern, .. }
    | Query::Construct { pattern, .. }
    | Query::Describe { pattern, .. }
    | Query::Ask { pattern, .. }) = query;
    let mut widest = 0;
    width(pattern, &mut widest);
    (widest > MAX_JOIN_WIDTH).then(|| {
        format!(
            "it joins {widest} patterns or path steps at once (the most allowed is {MAX_JOIN_WIDTH})"
        )
    })
}

/// How many patterns and path steps `p` adds to the join it is part of; the
/// widest join met anywhere in it (`EXISTS` included) is raised into `widest`.
fn width(p: &GraphPattern, widest: &mut usize) -> usize {
    let w = match p {
        GraphPattern::Bgp { patterns } => patterns.len(),
        GraphPattern::Path { path, .. } => steps(path),
        GraphPattern::Join { left, right } => width(left, widest) + width(right, widest),
        GraphPattern::Lateral { left, right } => width(left, widest) + width(right, widest),
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            if let Some(e) = expression {
                expression_width(e, widest);
            }
            width(left, widest).max(width(right, widest))
        }
        GraphPattern::Union { left, right } | GraphPattern::Minus { left, right } => {
            width(left, widest).max(width(right, widest))
        }
        GraphPattern::Filter { expr, inner } => {
            expression_width(expr, widest);
            width(inner, widest)
        }
        GraphPattern::Extend {
            inner, expression, ..
        } => {
            expression_width(expression, widest);
            width(inner, widest)
        }
        GraphPattern::OrderBy { inner, expression } => {
            for OrderExpression::Asc(e) | OrderExpression::Desc(e) in expression {
                expression_width(e, widest);
            }
            width(inner, widest)
        }
        GraphPattern::Graph { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. }
        | GraphPattern::Service { inner, .. } => width(inner, widest),
        GraphPattern::Values { .. } => 1,
    };
    *widest = (*widest).max(w);
    w
}

/// The steps of a path that are joined: a sequence adds up, an alternative
/// is as wide as its widest side.
fn steps(p: &PropertyPathExpression) -> usize {
    match p {
        PropertyPathExpression::Sequence(a, b) => steps(a) + steps(b),
        PropertyPathExpression::Alternative(a, b) => steps(a).max(steps(b)),
        PropertyPathExpression::Reverse(a)
        | PropertyPathExpression::ZeroOrMore(a)
        | PropertyPathExpression::OneOrMore(a)
        | PropertyPathExpression::ZeroOrOne(a) => steps(a),
        PropertyPathExpression::NamedNode(_) | PropertyPathExpression::NegatedPropertySet(_) => 1,
    }
}

/// The patterns inside `EXISTS` in `e`, measured into `widest`.
fn expression_width(e: &Expression, widest: &mut usize) {
    match e {
        Expression::Exists(p) => {
            width(p, widest);
        }
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
            expression_width(a, widest);
            expression_width(b, widest);
        }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => {
            expression_width(a, widest);
        }
        Expression::If(a, b, c) => {
            expression_width(a, widest);
            expression_width(b, widest);
            expression_width(c, widest);
        }
        Expression::In(a, rest) => {
            expression_width(a, widest);
            rest.iter().for_each(|e| expression_width(e, widest));
        }
        Expression::Coalesce(args) | Expression::FunctionCall(_, args) => {
            args.iter().for_each(|e| expression_width(e, widest));
        }
        Expression::NamedNode(_)
        | Expression::Literal(_)
        | Expression::Variable(_)
        | Expression::Bound(_) => {}
    }
}
