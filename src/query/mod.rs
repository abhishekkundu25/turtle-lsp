//! Running a SPARQL query over documents: the engine behind `turtle-lsp
//! query`, the "Run query" code lens and the browser's Run button.
//!
//! [`run`] takes the documents (their analyses, as SHACL validation reads
//! them), the query text with its base and the prefixes it may use without
//! declaring them (prefixes.ttl and settings, as the editor gives a `.rq`
//! file), and [`Limits`]. It returns an [`Answer`] or a [`QueryError`].
//!
//! The dataset:
//!
//! - the default graph is the union of every triple of every document,
//!   named-graph triples included (TriG, N-Quads, JSON-LD), with blank nodes
//!   kept apart per document ([`crate::rdf::Graph`]);
//! - there are no named graphs: `GRAPH` patterns and `FROM NAMED` match
//!   nothing;
//! - `FROM` is ignored (the default graph stays the union).
//!
//! Terms are `spargebra`'s (`oxrdf`'s) [`Term`]: an IRI, a blank node, a
//! literal or an RDF 1.2 triple term. Blank nodes of the data are labelled
//! `g<n>`, numbered across the graph; those a CONSTRUCT template makes get
//! fresh labels.
//!
//! The time limit and the cancellation flag are checked as the evaluator
//! reads the data (every quad it asks for), between results, and through the
//! evaluator's cancellation token, which its joins ask every thousand rows
//! they combine in memory (a patch of `spareval`, in vendor/spareval), so a
//! query that runs away stops at the limit. UPDATE is not run.
//!
//! The renderers are in [`render`].

pub mod render;

use crate::analysis::Analysis;
use crate::clock::Instant;
use crate::rdf::dataset::{Index, STerm};
use crate::rdf::graph::Graph;
use crate::sparql::{Document, Level};
use crate::syntax::{PrefixMap, Triple as DocTriple};
use rustc_hash::FxHashSet;
use spareval::{
    InternalQuad, QueryEvaluationError, QueryEvaluator, QueryResults, QueryableDataset,
};
use spargebra::term::{Term, Triple};
use spargebra::{Query, SparqlParser};
use std::fmt;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// What a query returned.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// SELECT.
    Solutions(Solutions),
    /// ASK.
    Boolean(bool),
    /// CONSTRUCT or DESCRIBE: the graph, each triple once, in the order the
    /// evaluator gave them.
    Triples(Triples),
}

/// The solutions of a SELECT query.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Solutions {
    /// The projected variables, in order, without `?`.
    pub variables: Vec<String>,
    /// One value per variable; `None` when unbound. At most
    /// [`Limits::max_rows`] of them.
    pub rows: Vec<Vec<Option<Term>>>,
    /// The rows counted past [`Limits::max_rows`] and not kept.
    pub omitted: usize,
}

impl Solutions {
    /// Every row the query returned, those not kept included.
    pub fn total(&self) -> usize {
        self.rows.len() + self.omitted
    }
}

/// The graph of a CONSTRUCT or DESCRIBE query. Derefs to its triples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Triples {
    /// Each triple once, at most [`Limits::max_rows`] of them.
    pub triples: Vec<Triple>,
    /// The distinct triples counted past [`Limits::max_rows`] and not kept.
    pub omitted: usize,
}

impl Triples {
    /// Every distinct triple the query returned, those not kept included.
    pub fn total(&self) -> usize {
        self.triples.len() + self.omitted
    }
}

impl From<Vec<Triple>> for Triples {
    fn from(triples: Vec<Triple>) -> Triples {
        Triples {
            triples,
            omitted: 0,
        }
    }
}

impl std::ops::Deref for Triples {
    type Target = [Triple];
    fn deref(&self) -> &[Triple] {
        &self.triples
    }
}

impl<'a> IntoIterator for &'a Triples {
    type Item = &'a Triple;
    type IntoIter = std::slice::Iter<'a, Triple>;
    fn into_iter(self) -> Self::IntoIter {
        self.triples.iter()
    }
}

/// Why a query gave no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// The text is not a query. `offset` is a byte offset into the text (its
    /// length when the text ends too early); `line` and `column` are 1-based,
    /// the column in characters.
    Syntax {
        message: String,
        offset: usize,
        line: u32,
        column: u32,
    },
    /// A form that is not run (UPDATE).
    Unsupported(String),
    /// The time limit passed.
    Timeout,
    /// The evaluation kept more than this many rows in memory.
    TooManyRows(usize),
    /// The caller's cancellation flag was set.
    Cancelled,
    /// The evaluator failed (an unsupported custom function or `SERVICE`).
    Evaluation(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::Syntax {
                message,
                line,
                column,
                ..
            } => write!(f, "syntax error at {line}:{column}: {message}"),
            QueryError::Unsupported(why) => f.write_str(why),
            QueryError::Timeout => f.write_str("the query was stopped at the time limit"),
            QueryError::TooManyRows(n) => write!(
                f,
                "the query was stopped: it kept more than {n} rows in memory"
            ),
            QueryError::Cancelled => f.write_str("the query was cancelled"),
            QueryError::Evaluation(why) => write!(f, "the query failed: {why}"),
        }
    }
}

impl std::error::Error for QueryError {}

/// When to stop a query, and how much of its answer to keep.
#[derive(Clone, Debug, Default)]
pub struct Limits {
    /// From the start of [`run`]; `None` for no limit.
    pub timeout: Option<Duration>,
    /// Stops the query when set (from another thread).
    pub cancel: Option<Arc<AtomicBool>>,
    /// Keep at most this many rows (or triples); the rest are counted, not
    /// kept ([`Solutions::omitted`], [`Triples::omitted`]). `None` keeps all.
    pub max_rows: Option<usize>,
    /// Stop once the evaluation keeps more than this many rows in memory (to
    /// sort, deduplicate, group or join them); `None` for no limit.
    pub max_buffered_rows: Option<usize>,
}

/// Runs `query` over the union of `docs`. `base` resolves relative IRIs;
/// `prefixes` are the prefixes the query may use without declaring them.
pub fn run(
    docs: &[&Analysis],
    query: &str,
    base: Option<&str>,
    prefixes: &PrefixMap,
    limits: &Limits,
) -> Result<Answer, QueryError> {
    let watch = Arc::new(Watch::new(limits));
    let parsed = parse(query, base, prefixes)?;
    let triples: Vec<&[DocTriple]> = docs.iter().map(|a| a.parsed.triples.as_slice()).collect();
    let graph = Graph::new(&triples);
    let index = Index::new(&graph, None);
    watch.check().map_err(QueryError::from)?;
    evaluate(&index, &parsed, &watch, limits)
}

/// The prefixes to show results with: `external` and the query's own
/// `PREFIX` declarations (the later ones win).
pub fn display_prefixes(query: &str, external: &PrefixMap) -> PrefixMap {
    let doc = Document::parse(query, external, None);
    let mut out = external.clone();
    for p in &doc.structure().prefixes {
        if let Some(ns) = &p.resolved {
            out.insert(p.name.clone(), ns.clone());
        }
    }
    out
}

/// Parses `text` as the editor checks it: the same diagnostics, the first
/// error reported.
fn parse(text: &str, base: Option<&str>, prefixes: &PrefixMap) -> Result<Query, QueryError> {
    let doc = Document::parse(text, prefixes, base);
    if doc.is_update() {
        return Err(QueryError::Unsupported(
            "SPARQL UPDATE is not supported: only queries (SELECT, ASK, CONSTRUCT, DESCRIBE) run"
                .into(),
        ));
    }
    if let Some(d) = doc.diagnostics().iter().find(|d| d.level == Level::Error) {
        return Err(syntax_error(text, d.message.clone(), d.span.start as usize));
    }
    let mut parser = SparqlParser::new();
    if let Some(b) = base
        && let Ok(p) = parser.clone().with_base_iri(b)
    {
        parser = p;
    }
    for (name, ns) in prefixes {
        if let Ok(p) = parser.clone().with_prefix(name, ns) {
            parser = p;
        }
    }
    let fixed = crate::sparql::validate::text_for_spargebra(text, doc.tokens());
    // The diagnostics found nothing (they should have): locate it here.
    parser
        .clone()
        .parse_query(&fixed)
        .map_err(|_| located(text, doc.tokens(), parser))
}

/// The syntax error of `text` (tokenized as `tokens`), which `parser`
/// rejects, at its place in `text`: the parser is given a rewritten copy.
fn located(text: &str, tokens: &[crate::sparql::Token], parser: SparqlParser) -> QueryError {
    let e = crate::sparql::validate::check_tokens(text, tokens, parser, false);
    let offset = match e.as_ref().map(|e| e.at) {
        Some(crate::sparql::validate::ErrorAt::Token(s)) => s.start as usize,
        _ => text.len(),
    };
    syntax_error(text, e.map_or_else(String::new, |e| e.message), offset)
}

fn syntax_error(text: &str, message: String, offset: usize) -> QueryError {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    QueryError::Syntax {
        message,
        offset,
        line: before.matches('\n').count() as u32 + 1,
        column: before[line_start..].chars().count() as u32 + 1,
    }
}

/// Evaluates `query`, stopped by `watch`. Natively a stop during a data read
/// unwinds out of the evaluator at once ([`halt`]): the evaluator keeps going
/// after a read error (a join or an aggregate collects errors and finishes
/// its input), so an error alone would not stop work already buffered.
fn evaluate(
    index: &Index,
    query: &Query,
    watch: &Arc<Watch>,
    limits: &Limits,
) -> Result<Answer, QueryError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        evaluate_inner(index, query, watch, limits)
    })) {
        Ok(answer) => answer,
        Err(payload) => match payload.downcast_ref::<Stop>() {
            Some(s) => Err(QueryError::from(*s)),
            None => std::panic::resume_unwind(payload),
        },
    }
}

fn evaluate_inner(
    index: &Index,
    query: &Query,
    watch: &Arc<Watch>,
    limits: &Limits,
) -> Result<Answer, QueryError> {
    let max = limits.max_rows.unwrap_or(usize::MAX);
    let mut token = {
        let watch = watch.clone();
        spareval::CancellationToken::with_check(move || watch.check().is_err())
    };
    if let Some(n) = limits.max_buffered_rows {
        token = token.with_max_kept_tuples(n);
    }
    let evaluator = QueryEvaluator::new().with_cancellation_token(token.clone());
    let mut prepared = evaluator.prepare(query);
    // FROM and FROM NAMED are ignored: the default graph is the union.
    *prepared.dataset_mut() = spareval::QueryDatasetSpecification::new();
    let data = Watched { index, watch };
    let too_many = || QueryError::TooManyRows(limits.max_buffered_rows.unwrap_or(usize::MAX));
    let fail = |e: QueryEvaluationError| {
        if token.kept_too_many() {
            return too_many();
        }
        watch.stopped().unwrap_or_else(|| evaluation(e))
    };
    let answer = match prepared.execute(data).map_err(fail)? {
        QueryResults::Solutions(solutions) => {
            let variables = solutions
                .variables()
                .iter()
                .map(|v| v.as_str().to_owned())
                .collect();
            let mut rows = Vec::new();
            let mut omitted = 0;
            for s in solutions {
                watch.check()?;
                let s = s.map_err(fail)?;
                if rows.len() < max {
                    rows.push(s.values().to_vec());
                    if limits.max_buffered_rows.is_some_and(|n| rows.len() > n) {
                        return Err(too_many());
                    }
                } else {
                    omitted += 1;
                }
            }
            Answer::Solutions(Solutions {
                variables,
                rows,
                omitted,
            })
        }
        QueryResults::Boolean(b) => Answer::Boolean(b),
        QueryResults::Graph(triples) => {
            // The triples kept, and a hash of each one past the cap: enough
            // to count distinct triples without keeping them. Both count
            // against the buffered rows.
            let mut seen = FxHashSet::default();
            let mut past = FxHashSet::default();
            let mut out = Vec::new();
            for t in triples {
                watch.check()?;
                let t = t.map_err(fail)?;
                if seen.contains(&t) {
                    continue;
                }
                if out.len() < max {
                    seen.insert(t.clone());
                    out.push(t);
                } else {
                    past.insert(rustc_hash::FxBuildHasher.hash_one(&t));
                }
                if limits
                    .max_buffered_rows
                    .is_some_and(|n| out.len() + past.len() > n)
                {
                    return Err(too_many());
                }
            }
            Answer::Triples(Triples {
                triples: out,
                omitted: past.len(),
            })
        }
    };
    // A stop whose error the evaluator dropped (MINUS ignores the errors of
    // its right side) is not a partial answer.
    if token.kept_too_many() {
        return Err(too_many());
    }
    Ok(answer)
}

fn evaluation(e: QueryEvaluationError) -> QueryError {
    match e {
        QueryEvaluationError::Cancelled => QueryError::Cancelled,
        e => QueryError::Evaluation(e.to_string()),
    }
}

/// Why [`Watch`] stopped a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Timeout,
    Cancelled,
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stop::Timeout => "time limit",
            Stop::Cancelled => "cancelled",
        })
    }
}

impl std::error::Error for Stop {}

impl From<Stop> for QueryError {
    fn from(s: Stop) -> QueryError {
        match s {
            Stop::Timeout => QueryError::Timeout,
            Stop::Cancelled => QueryError::Cancelled,
        }
    }
}

/// The limits of one run, shared with the evaluator's cancellation token.
/// Once stopped, it stays stopped.
struct Watch {
    start: Instant,
    timeout: Option<Duration>,
    cancel: Option<Arc<AtomicBool>>,
    stopped: OnceLock<Stop>,
    checks: AtomicU32,
}

impl Watch {
    fn new(limits: &Limits) -> Watch {
        Watch {
            start: Instant::now(),
            timeout: limits.timeout,
            cancel: limits.cancel.clone(),
            stopped: OnceLock::new(),
            checks: AtomicU32::new(0),
        }
    }

    fn check(&self) -> Result<(), Stop> {
        if let Some(s) = self.stopped.get() {
            return Err(*s);
        }
        if self
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
        {
            return self.stop(Stop::Cancelled);
        }
        // The clock at every 64th check: a check is a data read, a result or
        // a thousand rows a join combines, so checks come close together.
        if let Some(limit) = self.timeout
            && self
                .checks
                .fetch_add(1, Ordering::Relaxed)
                .is_multiple_of(64)
            && Instant::now().saturating_duration_since(self.start) >= limit
        {
            return self.stop(Stop::Timeout);
        }
        Ok(())
    }

    fn stop(&self, s: Stop) -> Result<(), Stop> {
        Err(*self.stopped.get_or_init(|| s))
    }

    fn stopped(&self) -> Option<QueryError> {
        self.stopped.get().copied().map(QueryError::from)
    }
}

/// Stops evaluation at a data read: natively by unwinding to [`evaluate`]
/// (with the [`Stop`] as payload, not a panic: no panic hook runs); in the
/// browser build, which aborts on panic, by the read error (the evaluator then
/// finishes what it has buffered, reading nothing more).
fn halt(s: Stop) -> Stop {
    #[cfg(not(target_arch = "wasm32"))]
    std::panic::resume_unwind(Box::new(s));
    #[cfg(target_arch = "wasm32")]
    s
}

/// The [`Index`] with each quad read checked against the [`Watch`].
#[derive(Clone, Copy)]
struct Watched<'a> {
    index: &'a Index<'a>,
    watch: &'a Watch,
}

impl<'a> QueryableDataset<'a> for Watched<'a> {
    type InternalTerm = STerm;
    type Error = Stop;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&STerm>,
        predicate: Option<&STerm>,
        object: Option<&STerm>,
        graph_name: Option<Option<&STerm>>,
    ) -> impl Iterator<Item = Result<InternalQuad<STerm>, Stop>> + use<'a> {
        let watch = self.watch;
        let fail = watch.check().err();
        let quads = (fail.is_none()).then(|| {
            self.index
                .internal_quads_for_pattern(subject, predicate, object, graph_name)
        });
        fail.map(Err)
            .into_iter()
            .chain(quads.into_iter().flatten().map(move |q| {
                watch.check().map_err(halt)?;
                Ok(q.unwrap_or_else(|e| match e {}))
            }))
    }

    fn internal_named_graphs(&self) -> impl Iterator<Item = Result<STerm, Stop>> + use<'a> {
        self.index
            .internal_named_graphs()
            .map(|g| Ok(g.unwrap_or_else(|e| match e {})))
    }

    fn contains_internal_graph_name(&self, graph_name: &STerm) -> Result<bool, Stop> {
        Ok(self
            .index
            .contains_internal_graph_name(graph_name)
            .unwrap_or_else(|e| match e {}))
    }

    fn internalize_term(&self, term: Term) -> Result<STerm, Stop> {
        self.watch.check().map_err(halt)?;
        Ok(self.index.internal(&term))
    }

    fn externalize_term(&self, term: STerm) -> Result<Term, Stop> {
        Ok(self
            .index
            .externalize_term(term)
            .unwrap_or_else(|e| match e {}))
    }
}

#[cfg(test)]
mod tests;
