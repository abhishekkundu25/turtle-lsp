//! SHACL validation of the whole workspace, off the main thread.
//!
//! The server hands a [`Job`] (the analyses of every workspace and external
//! source, shared, so the snapshot is cheap) to a [`Worker`] on save and once
//! after the initial index. The worker validates the union of the Turtle
//! documents' triples (and of JSON-LD ones without named graphs), and each
//! TriG / N-Triples / N-Quads dataset (and JSON-LD one with named graphs)
//! together with them, as data and shapes graph and sends back diagnostics per
//! document. The shared graph is built once and each dataset is read as an
//! overlay on it ([`Graph::overlay`]), with only the nodes the dataset mentions
//! as focus nodes, so a run costs the shared triples plus each dataset's own;
//! `shacl.maxTriples` bounds that total (the shared triples and every dataset
//! validated), not each dataset together with the shared ones. Each job carries
//! a generation; a newer job makes the worker
//! abandon the current one (also inside one long path or query), and the
//! server drops results whose generation is not the latest.
//!
//! What validation could not do is shown, not passed over: a query that
//! failed or timed out, a shape cut short (nested too deep, or past the path
//! step budget) is a warning (`shacl-failure`, `shacl-sparql-timeout`) on its
//! `sh:select` / `sh:ask` or its shape; a shape whose `sh:path` cannot be
//! read is an error (`shacl-invalid-path`). Past the result budget validation
//! stops with one note; a file shows at most [`MAX_PER_DOC`] results, the most
//! severe first.
//!
//! A result is shown on the triple (focus node, first path predicate, value)
//! when the workspace has it, else on the focus node's first description,
//! else on the constraint; never in an external source. In JSON-LD the
//! triple's predicate is its key and its value the JSON value; a node is
//! described at its `@id`, or, without one, at its own object's opening brace. Positions come from
//! the analyses validated, so after further edits they stay where they were
//! until the next save.

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::SOURCE;
use crate::features::hover::short;
use crate::features::navigation;
use crate::knowledge::Source;
use crate::rdf::graph::{Graph, Id, Term};
use crate::shacl::sparql::{Cancel, TIMED_OUT};
use crate::shacl::{
    Failure, FailureKind, Path, SH, ValidationResult, validate_report, validate_report_among,
};
use crate::syntax::{Iri, PrefixMap, RDF, Role, Span, Syntax, XSD};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const CODE: &str = "shacl-violation";
pub(crate) const SKIPPED_CODE: &str = "shacl-skipped";
pub(crate) const TIMEOUT_CODE: &str = "shacl-sparql-timeout";
/// A query or shape that could not be validated to the end.
pub(crate) const FAILURE_CODE: &str = "shacl-failure";
/// A shape skipped for its `sh:path`.
pub(crate) const INVALID_PATH_CODE: &str = "shacl-invalid-path";
/// Results shown per document; the rest are summarized in one diagnostic.
const MAX_PER_DOC: usize = 1000;

pub(crate) type ByUri = FxHashMap<String, Vec<Diagnostic>>;

pub struct Job {
    pub generation: u64,
    pub docs: Vec<(Source, Arc<Analysis>)>,
    pub max_triples: u64,
    pub enc: Encoding,
    /// The workspace's `owl:imports` that are not loaded. While any is,
    /// errors that the missing data could change are shown as warnings
    /// (unless `soften` is false).
    pub unresolved: Vec<String>,
    /// Soften results while imports are unresolved (the `imports.unresolved`
    /// setting); false treats them as empty.
    pub soften: bool,
}

/// How to soften errors while imports are unresolved.
struct Soften {
    /// Appended to a softened message.
    note: String,
    /// Namespaces some loaded document defines a term in.
    described: FxHashSet<String>,
}

impl Soften {
    fn new(job: &Job) -> Option<Soften> {
        if !job.soften || job.unresolved.is_empty() {
            return None;
        }
        let mut named = job
            .unresolved
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if job.unresolved.len() > 3 {
            named.push_str(&format!(" and {} more", job.unresolved.len() - 3));
        }
        let described = job
            .docs
            .iter()
            .flat_map(|(_, a)| a.defs.keys())
            .map(|i| crate::knowledge::namespace_of(i.as_str()))
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect();
        Some(Soften {
            note: format!(" (may change once these imports resolve: {named})"),
            described,
        })
    }

    /// Whether data the unresolved imports may hold could change `r`: a
    /// constraint that more triples can satisfy, or a focus node or value in
    /// a namespace no loaded document describes.
    fn applies(&self, g: &Graph, r: &ValidationResult) -> bool {
        let component = r.component.as_str().strip_prefix(SH).unwrap_or("");
        if matches!(
            component,
            "ClassConstraintComponent"
                | "NodeConstraintComponent"
                | "MinCountConstraintComponent"
                | "QualifiedMinCountConstraintComponent"
                | "QualifiedMaxCountConstraintComponent"
        ) {
            return true;
        }
        let unknown = |id: Id| match g.term(id) {
            Term::Iri(i) => {
                let ns = crate::knowledge::namespace_of(i.as_str());
                !ns.is_empty() && !self.described.contains(ns)
            }
            _ => false,
        };
        unknown(r.focus) || r.value.is_some_and(unknown)
    }
}

pub(crate) struct Outcome {
    pub(crate) generation: u64,
    pub(crate) diagnostics: ByUri,
}

/// The validation thread. It blocks on its channel while idle and ends
/// when the worker is dropped.
pub(crate) struct Worker {
    jobs: Sender<Job>,
    latest: Arc<AtomicU64>,
}

impl Worker {
    /// Start the thread; Err when it cannot be started.
    pub(crate) fn start() -> std::io::Result<(Worker, Receiver<Outcome>)> {
        let (jobs, job_rx) = unbounded::<Job>();
        let (out_tx, out_rx) = unbounded();
        let latest = Arc::new(AtomicU64::new(0));
        let seen = latest.clone();
        // Always a thread: the loop waits for jobs. Embedded, the server
        // validates inline instead and never starts one.
        crate::server::worker::Spawn::Thread.run(
            "shacl",
            // Defence in depth: recursion is bounded, but generously.
            16 << 20,
            move || {
                while let Ok(mut job) = job_rx.recv() {
                    // Only the newest queued job matters.
                    while let Ok(newer) = job_rx.try_recv() {
                        job = newer;
                    }
                    let generation = job.generation;
                    let latest = seen.clone();
                    let cancelled = move || latest.load(Ordering::Relaxed) != generation;
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run(&job, &cancelled)
                    }));
                    let diagnostics = match result {
                        Ok(Some(d)) => d,
                        Ok(None) => continue,
                        // A bug: say so on the shape files rather than leave
                        // the last results standing with no explanation.
                        Err(e) => crashed(&job, &e),
                    };
                    if out_tx
                        .send(Outcome {
                            generation,
                            diagnostics,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            },
            // A job's panic is caught above; the loop's own cannot reach
            // the main loop but as the channel's end.
            |_| {},
        )?;
        Ok((Worker { jobs, latest }, out_rx))
    }

    /// Abandon the running job, if any, in favour of `generation`.
    pub(crate) fn cancel(&self, generation: u64) {
        self.latest.store(generation, Ordering::Relaxed);
    }

    pub(crate) fn submit(&self, job: Job) {
        self.latest.store(job.generation, Ordering::Relaxed);
        let _ = self.jobs.send(job);
    }
}

/// Diagnostics for a validation that panicked: one error per shape file.
fn crashed(job: &Job, panic: &(dyn std::any::Any + Send)) -> ByUri {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    job.docs
        .iter()
        .filter(|(s, a)| *s != Source::External && has_shapes(a))
        .map(|(_, a)| {
            let d = Diagnostic {
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String(CODE.into())),
                source: Some(SOURCE.into()),
                message: format!(
                    "turtle-lsp internal error in SHACL validation: {detail}. Results are not shown; please report this at https://github.com/abhishekkundu25/turtle-lsp/issues"
                ),
                ..Default::default()
            };
            (a.uri.clone(), vec![d])
        })
        .collect()
}

/// Whether `a` declares shapes validation would act on (a target, or a
/// typed shape; see [`crate::shacl::validate::acts_on`]). A few hash lookups.
pub(crate) fn has_shapes(a: &Analysis) -> bool {
    crate::shacl::validate::acts_on(&|i| a.refs_by_iri.contains_key(&i))
}

/// Diagnostics for every workspace document, or `None` when cancelled.
///
/// Turtle documents are fragments of one workspace graph, so they are
/// validated together. A TriG, N-Triples or N-Quads document is a dataset of
/// its own (generated runs, dumps, snapshots that reuse the same IRIs): it is
/// validated with the Turtle documents (ontology, shapes, shared data) but
/// never merged with another dataset, which would turn two versions of a node
/// into one node with two values. A dataset shows only the results its data
/// changes: one the shared graph has on its own is shown there, once.
pub fn run<C>(job: &Job, cancelled: &C) -> Option<ByUri>
where
    C: Fn() -> bool + Send + Sync + Clone + 'static,
{
    let cancelled: Cancel = Arc::new(cancelled.clone());
    let cancelled = &cancelled;
    let mut docs: Vec<&(Source, Arc<Analysis>)> = job.docs.iter().collect();
    docs.sort_by(|a, b| (a.0, &a.1.uri).cmp(&(b.0, &b.1.uri)));
    let dataset = |d: &&(Source, Arc<Analysis>)| d.0 != Source::External && is_dataset(&d.1);
    let shared = Shared::new(docs.iter().copied().filter(|d| !dataset(d)).collect());
    let mut out = ByUri::default();
    let soften = Soften::new(job);
    let soften = soften.as_ref();
    // The shared graph, its results placed in the Turtle documents.
    let mut shared_results = FxHashSet::default();
    let mut spent = 0;
    validate_unit(
        job,
        &shared,
        None,
        &mut spent,
        soften,
        cancelled,
        &mut shared_results,
        &mut out,
    )?;
    for d in docs.iter().copied().filter(dataset) {
        // Read with the shared graph, not merged into it; only the dataset's
        // own nodes are focus nodes (the shared ones were validated above).
        validate_unit(
            job,
            &shared,
            Some(d),
            &mut spent,
            soften,
            cancelled,
            &mut shared_results,
            &mut out,
        )?;
    }
    for list in out.values_mut() {
        let mut seen = FxHashSet::default();
        list.retain(|d| seen.insert((d.range.start, d.range.end, d.message.clone())));
        if list.len() > MAX_PER_DOC {
            // The most severe first, so an error is not what goes unshown;
            // within a severity, in the order found.
            list.sort_by_key(|d| severity_rank(d.severity));
            let hidden = list.len() - MAX_PER_DOC;
            list.truncate(MAX_PER_DOC);
            list.push(Diagnostic {
                severity: Some(DiagnosticSeverity::INFORMATION),
                code: Some(NumberOrString::String(CODE.into())),
                source: Some(SOURCE.into()),
                message: format!("{hidden} more SHACL results in this file are not shown"),
                ..Default::default()
            });
        }
    }
    Some(out)
}

/// Errors first.
fn severity_rank(s: Option<DiagnosticSeverity>) -> u8 {
    match s {
        Some(DiagnosticSeverity::WARNING) => 1,
        Some(DiagnosticSeverity::INFORMATION) => 2,
        Some(DiagnosticSeverity::HINT) => 3,
        _ => 0,
    }
}

/// The note that validation stopped after `kept` results.
fn capped_note(kept: usize) -> Diagnostic {
    Diagnostic {
        severity: Some(DiagnosticSeverity::INFORMATION),
        code: Some(NumberOrString::String(CODE.into())),
        source: Some(SOURCE.into()),
        message: format!("SHACL validation stopped after {kept} results; more are not shown"),
        ..Default::default()
    }
}

/// Whether `a` is validated as a dataset of its own rather than as part of
/// the shared graph: TriG, N-Triples and N-Quads always; JSON-LD when it has
/// a named graph (`@graph` with an `@id`), as TriG with one would. A JSON-LD
/// document without one is a fragment of the workspace graph, as Turtle is.
fn is_dataset(a: &Analysis) -> bool {
    match a.parsed.syntax {
        Syntax::Turtle => false,
        Syntax::JsonLd => a.parsed.triples.iter().any(|t| t.graph.is_some()),
        Syntax::TriG | Syntax::NTriples | Syntax::NQuads => true,
    }
}

/// A validation result in terms that do not depend on the graph it came from,
/// to tell a dataset's results that the shared graph also has.
#[derive(PartialEq, Eq, Hash)]
struct ResultKey<'j> {
    /// The constraint: its parameter triple as (document URI, index).
    param: (&'j str, u32),
    focus: TermKey<'j>,
    value: Option<TermKey<'j>>,
    path: String,
    component: Iri,
    messages: Vec<(String, Option<Iri>)>,
}

#[derive(PartialEq, Eq, Hash)]
enum TermKey<'j> {
    Iri(Iri),
    /// (document URI, the blank node's number in it)
    Blank(&'j str, u32),
    Value(crate::syntax::Node),
}

fn result_key<'j>(
    g: &Graph,
    docs: &[&'j (Source, Arc<Analysis>)],
    r: &ValidationResult,
) -> ResultKey<'j> {
    let uri = |d: u32| docs[d as usize].1.uri.as_str();
    let term = |id: Id| match g.term(id) {
        Term::Iri(i) => TermKey::Iri(i),
        Term::Blank(b) => {
            let (d, n) = g.blank_origin(b);
            TermKey::Blank(uri(d), n)
        }
        Term::Value(n) => TermKey::Value(n.clone()),
    };
    let (d, i) = g.origin(r.param);
    ResultKey {
        param: (uri(d), i),
        focus: term(r.focus),
        value: r.value.map(term),
        path: format!("{:?}", r.path),
        component: r.component,
        messages: r.computed_messages.clone(),
    }
}

/// The Turtle (and JSON-LD fragment) documents of a run, whose graph is built
/// once, when a unit needs it, and read by every unit.
struct Shared<'j> {
    docs: Vec<&'j (Source, Arc<Analysis>)>,
    /// Their triples.
    total: u64,
    graph: std::cell::OnceCell<Arc<Graph<'j>>>,
}

impl<'j> Shared<'j> {
    fn new(docs: Vec<&'j (Source, Arc<Analysis>)>) -> Shared<'j> {
        let total = docs.iter().map(|d| d.1.parsed.triples.len() as u64).sum();
        Shared {
            docs,
            total,
            graph: std::cell::OnceCell::new(),
        }
    }

    fn graph(&self) -> &Arc<Graph<'j>> {
        self.graph.get_or_init(|| {
            let triples: Vec<&[crate::syntax::Triple]> = self
                .docs
                .iter()
                .map(|d| d.1.parsed.triples.as_slice())
                .collect();
            Arc::new(Graph::new(&triples))
        })
    }
}

/// Validate the shared graph, or with a `dataset` that dataset read together
/// with it (an overlay: the shared graph is not rebuilt). With a dataset,
/// only nodes it mentions are focus nodes, results in `shared_results` are
/// dropped and the rest are placed in the dataset; without, results are added
/// to `shared_results`. The run reads at most [`Job::max_triples`] triples in
/// all: the shared ones, and each dataset validated, counted in `spent`.
/// `None` when cancelled.
#[allow(clippy::too_many_arguments)]
fn validate_unit<'j>(
    job: &Job,
    shared: &Shared<'j>,
    dataset: Option<&'j (Source, Arc<Analysis>)>,
    spent: &mut u64,
    soften: Option<&Soften>,
    cancelled: &Cancel,
    shared_results: &mut FxHashSet<ResultKey<'j>>,
    out: &mut ByUri,
) -> Option<()> {
    let workspace = |s: Source| s != Source::External;
    let shaped = |(s, a): &&&(Source, Arc<Analysis>)| workspace(*s) && has_shapes(a);
    // The dataset is the last document, as in the graph.
    let mut docs = shared.docs.clone();
    docs.extend(dataset);
    if !docs.iter().any(|d| shaped(&d)) {
        return Some(());
    }
    let own = dataset.map_or(0, |d| d.1.parsed.triples.len() as u64);
    let total = shared.total + *spent + own;
    if total > job.max_triples {
        let skip = NumberOrString::String(SKIPPED_CODE.into());
        // A dataset over the limit is the one to tell; else the shape files.
        let told: Vec<&Arc<Analysis>> = match dataset {
            Some(a) => vec![&a.1],
            None => docs.iter().filter(|d| shaped(d)).map(|d| &d.1).collect(),
        };
        for a in told {
            let list = out.entry(a.uri.clone()).or_default();
            if !list.iter().any(|d| d.code.as_ref() == Some(&skip)) {
                list.push(skipped(a, job.enc, total, job.max_triples));
            }
        }
        return Some(());
    }
    *spent += own;
    let overlay;
    let g: &Graph = match dataset {
        Some(d) => {
            overlay = Graph::overlay(shared.graph(), d.1.parsed.triples.as_slice());
            &overlay
        }
        None => shared.graph(),
    };
    // Only the nodes a dataset mentions are focus nodes.
    let report = match dataset {
        Some(_) => {
            let mut mine: Vec<Id> = g.added().flat_map(|(_, t)| [t.s, t.o]).collect();
            mine.sort_unstable();
            mine.dedup();
            validate_report_among(g, &forward(cancelled), &mine)?
        }
        None => validate_report(g, &forward(cancelled), &|_| true)?,
    };
    let mut place = Placer {
        g,
        docs: &docs,
        enc: job.enc,
        prefixes: FxHashMap::default(),
        soften,
        dataset: dataset.map(|_| docs.len() as u32 - 1),
    };
    for r in &report.results {
        let key = result_key(g, &docs, r);
        if dataset.is_some() {
            if shared_results.contains(&key) {
                continue;
            }
        } else {
            shared_results.insert(key);
        }
        if let Some((uri, d)) = place.diagnostic(r) {
            out.entry(uri).or_default().push(d);
        }
    }
    for f in &report.failures {
        if let Some((uri, d)) = place.failure(f) {
            let list = out.entry(uri).or_default();
            if !list.contains(&d) {
                list.push(d);
            }
        }
    }
    if report.capped {
        // Told once: on the dataset, else on a file with shapes.
        let told = match dataset {
            Some(a) => Some(&a.1),
            None => docs.iter().find(|d| shaped(d)).map(|d| &d.1),
        };
        if let Some(a) = told {
            let list = out.entry(a.uri.clone()).or_default();
            let note = capped_note(report.results.len());
            if !list.contains(&note) {
                list.push(note);
            }
        }
    }
    Some(())
}

/// `cancel` as a closure of its own.
fn forward(cancel: &Cancel) -> impl Fn() -> bool + Send + Sync + Clone + 'static + use<> {
    let cancel = cancel.clone();
    move || cancel()
}

fn skipped(a: &Analysis, enc: Encoding, total: u64, max: u64) -> Diagnostic {
    let span = a
        .parsed
        .refs
        .iter()
        .find(|r| r.iri.starts_with(SH))
        .map_or(Span::default(), |r| r.span);
    Diagnostic {
        range: a.range(enc, span),
        severity: Some(DiagnosticSeverity::INFORMATION),
        code: Some(NumberOrString::String(SKIPPED_CODE.into())),
        source: Some(SOURCE.into()),
        message: format!(
            "SHACL validation skipped: the workspace has {total} triples, more than the limit of {max} (setting turtle.shacl.maxTriples)"
        ),
        ..Default::default()
    }
}

struct Placer<'p, 'g, 'a> {
    g: &'g Graph<'a>,
    docs: &'p [&'p (Source, Arc<Analysis>)],
    enc: Encoding,
    prefixes: FxHashMap<u32, PrefixMap>,
    soften: Option<&'p Soften>,
    /// The number of the dataset (the last of `docs`) whose results all go in it.
    dataset: Option<u32>,
}

/// The first predicate a path follows, and whether it is followed backwards.
fn first_step(p: &Path) -> Option<(Iri, bool)> {
    match p {
        Path::Predicate(i) => Some((*i, false)),
        Path::Inverse(q) => match &**q {
            Path::Predicate(i) => Some((*i, true)),
            _ => None,
        },
        Path::Sequence(ps) => first_step(ps.first()?),
        _ => None,
    }
}

impl Placer<'_, '_, '_> {
    fn doc(&self, d: u32) -> &Arc<Analysis> {
        &self.docs[d as usize].1
    }

    /// The documents in the order a description is looked for: the dataset
    /// first, then the shared ones.
    fn order(&self) -> impl Iterator<Item = u32> + use<> {
        let n = self.docs.len() as u32;
        let first = self.dataset;
        first
            .into_iter()
            .chain((0..n).filter(move |&d| Some(d) != first))
    }

    fn in_workspace(&self, d: u32) -> bool {
        self.docs[d as usize].0 != Source::External
    }

    /// The (document, triple) behind graph triple `t`.
    fn origin(&self, t: u32) -> (u32, &crate::syntax::Triple) {
        let (d, i) = self.g.origin(t);
        (d, &self.doc(d).parsed.triples[i as usize])
    }

    /// A diagnostic for a failure, on the `sh:select` or `sh:ask` of the
    /// constraint it is about (or the triple that states it), else on the
    /// shape: a warning, but an error when the shape was skipped.
    fn failure(&self, f: &Failure) -> Option<(String, Diagnostic)> {
        let query = [
            Iri::intern(&format!("{SH}select")),
            Iri::intern(&format!("{SH}ask")),
        ];
        let triple = f.triple.or_else(|| {
            let (t, _) = self.g.out(f.node).find(|(_, t)| query.contains(&t.p))?;
            Some(t)
        });
        let (d, span) = match triple {
            Some(t) => {
                let (d, triple) = self.origin(t);
                let span = Span::new(
                    triple.p_span.start,
                    triple.o_span.end.max(triple.p_span.end),
                );
                (d, span)
            }
            None => self.description(f.node)?,
        };
        if !self.in_workspace(d) {
            return None;
        }
        let a = self.doc(d);
        let (severity, code, message) = match f.kind {
            FailureKind::Path => (
                DiagnosticSeverity::ERROR,
                INVALID_PATH_CODE,
                f.message.clone(),
            ),
            _ if f.message == TIMED_OUT => {
                (DiagnosticSeverity::WARNING, TIMEOUT_CODE, f.message.clone())
            }
            FailureKind::Query => (
                DiagnosticSeverity::WARNING,
                FAILURE_CODE,
                format!("SHACL-SPARQL query failed: {}", f.message),
            ),
            FailureKind::Cut => (DiagnosticSeverity::WARNING, FAILURE_CODE, f.message.clone()),
        };
        Some((
            a.uri.clone(),
            Diagnostic {
                range: a.range(self.enc, span),
                severity: Some(severity),
                code: Some(NumberOrString::String(code.into())),
                source: Some(SOURCE.into()),
                message,
                ..Default::default()
            },
        ))
    }

    /// Where a result goes: (document, span). A dataset's result, caused by
    /// its data, goes in the dataset (which mentions every focus node).
    fn location(&self, r: &ValidationResult) -> Option<(u32, Span)> {
        let at = self.anywhere(r);
        if let Some(own) = self.dataset
            && at.is_none_or(|(d, _)| d != own)
        {
            return self.description_in(own, r.focus).or(at);
        }
        at
    }

    fn anywhere(&self, r: &ValidationResult) -> Option<(u32, Span)> {
        let g = self.g;
        let step = r.path.as_ref().and_then(first_step);
        if let Some((p, inverse)) = step {
            let single = matches!(r.path, Some(Path::Predicate(_)))
                || matches!(&r.path, Some(Path::Inverse(q)) if matches!(**q, Path::Predicate(_)));
            let candidates: Vec<(u32, bool)> = if inverse {
                g.incoming(r.focus)
                    .filter(|t| t.1.p == p)
                    .map(|t| (t.0, single && Some(t.1.s) == r.value))
                    .collect()
            } else {
                g.out(r.focus)
                    .filter(|t| t.1.p == p)
                    .map(|t| (t.0, single && Some(t.1.o) == r.value))
                    .collect()
            };
            let ws = |&&(t, _): &&(u32, bool)| self.in_workspace(self.g.origin(t).0);
            let exact = candidates.iter().filter(ws).find(|c| c.1);
            if let Some(&(t, _)) = exact {
                let (d, tr) = self.origin(t);
                return Some((d, if inverse { tr.p_span } else { tr.o_span }));
            }
            if (r.value.is_none() || !single)
                && let Some(&(t, _)) = candidates.iter().find(ws)
            {
                let (d, tr) = self.origin(t);
                return Some((d, tr.p_span));
            }
        }
        self.description(r.focus).or_else(|| {
            let (d, tr) = self.origin(r.param);
            self.in_workspace(d).then_some((d, tr.p_span))
        })
    }

    /// The first place the workspace describes `node`.
    fn description(&self, node: Id) -> Option<(u32, Span)> {
        self.described(node, &|d| self.in_workspace(d))
    }

    /// The first place document `d` describes `node`.
    fn description_in(&self, d: u32, node: Id) -> Option<(u32, Span)> {
        self.described(node, &|e| e == d && self.in_workspace(e))
    }

    /// The first place a document `docs` accepts describes `node`.
    fn described(&self, node: Id, docs: &dyn Fn(u32) -> bool) -> Option<(u32, Span)> {
        match self.g.term(node) {
            Term::Iri(i) => self
                .order()
                .filter(|&d| docs(d))
                .find_map(|d| Some((d, *self.doc(d).defs.get(&i)?.first()?)))
                .or_else(|| {
                    // Only referenced: its first occurrence.
                    self.order().filter(|&d| docs(d)).find_map(|d| {
                        let a = self.doc(d);
                        let r = a.refs_of(&i).find(|r| r.role != Role::Predicate)?;
                        Some((d, r.span))
                    })
                }),
            Term::Blank(b) => {
                let ws = |t: &(u32, &crate::rdf::graph::GTriple)| docs(self.g.origin(t.0).0);
                // A JSON-LD node object without an `@id`: its opening brace,
                // that of its own object even when it is nested. List nodes
                // have none (their triples are the enclosing object's).
                let (bd, _) = self.g.blank_origin(b);
                if docs(bd) && self.doc(bd).parsed.syntax == Syntax::JsonLd {
                    let list = |p: Iri| {
                        p.strip_prefix(RDF)
                            .is_some_and(|l| l == "first" || l == "rest")
                    };
                    let own = self.g.out(node).filter(ws).find(|(_, gt)| !list(gt.p));
                    if let Some((t, _)) = own {
                        let (d, tr) = self.origin(t);
                        let object = self.doc(d).parsed.statements[tr.statement as usize].span;
                        return Some((d, Span::new(object.start, object.start + 1)));
                    }
                }
                if let Some((t, _)) = self.g.incoming(node).find(ws) {
                    let (d, tr) = self.origin(t);
                    return Some((d, tr.o_span));
                }
                let (t, _) = self.g.out(node).find(ws)?;
                let (d, tr) = self.origin(t);
                Some((d, tr.p_span))
            }
            Term::Value(_) => None,
        }
    }

    fn name(&mut self, d: u32, id: Id) -> String {
        let a = self.doc(d).clone();
        let prefixes = self.prefixes.entry(d).or_insert_with(|| a.prefix_map());
        term_name(self.g, prefixes, id)
    }

    fn path_name(&mut self, d: u32, p: &Path) -> String {
        let a = self.doc(d).clone();
        let prefixes = self.prefixes.entry(d).or_insert_with(|| a.prefix_map());
        path_name(prefixes, p)
    }

    fn diagnostic(&mut self, r: &ValidationResult) -> Option<(String, Diagnostic)> {
        let (d, span) = self.location(r)?;
        let a = self.doc(d).clone();
        let computed = r
            .computed_messages
            .iter()
            .map(|(text, lang)| (text.as_str(), *lang));
        let mut message = match pick(computed).or_else(|| pick_message(self.g, &r.messages)) {
            Some(m) => m,
            None => self.component_message(d, r),
        };
        let mut severity = match r.severity.as_str().strip_prefix(SH) {
            Some("Warning") => DiagnosticSeverity::WARNING,
            Some("Info") => DiagnosticSeverity::INFORMATION,
            _ => DiagnosticSeverity::ERROR,
        };
        if severity == DiagnosticSeverity::ERROR
            && let Some(s) = self.soften
            && s.applies(self.g, r)
        {
            severity = DiagnosticSeverity::WARNING;
            message.push_str(&s.note);
        }
        let (pd, ptr) = self.origin(r.param);
        let param_span = Span::new(ptr.p_span.start, ptr.o_span.end.max(ptr.p_span.end));
        let related = if self.in_workspace(pd) {
            let pa = self.doc(pd).clone();
            let shape = self.name(pd, r.shape);
            Some(vec![DiagnosticRelatedInformation {
                location: Location {
                    uri: navigation::uri(&pa.uri),
                    range: pa.range(self.enc, param_span),
                },
                message: format!("Constraint of shape {shape}"),
            }])
        } else {
            None
        };
        Some((
            a.uri.clone(),
            Diagnostic {
                range: a.range(self.enc, span),
                severity: Some(severity),
                code: Some(NumberOrString::String(CODE.into())),
                source: Some(SOURCE.into()),
                message,
                related_information: related,
                ..Default::default()
            },
        ))
    }

    /// A readable message for a result without `sh:message`.
    fn component_message(&mut self, d: u32, r: &ValidationResult) -> String {
        let g = self.g;
        let param = g.triple_at(r.param).o;
        let p = self.name(d, param);
        let value = r.value.map(|v| self.name(d, v)).unwrap_or_default();
        let focus = self.name(d, r.focus);
        let path = r.path.as_ref().map(|p| self.path_name(d, p));
        let of = path
            .as_deref()
            .map(|p| format!("{p} of {focus}"))
            .unwrap_or_else(|| focus.clone());
        let component = r.component.as_str();
        let name = component
            .strip_prefix(SH)
            .and_then(|c| c.strip_suffix("ConstraintComponent"))
            .unwrap_or(component);
        match name {
            "Class" => format!("{value} ({of}) is not an instance of {p}"),
            "Datatype" => format!("{value} ({of}) is not a valid {p} literal"),
            "NodeKind" => format!("{value} ({of}) is not of node kind {p}"),
            "MinCount" => format!("{of}: expected at least {p} value(s)"),
            "MaxCount" => format!("{of}: expected at most {p} value(s)"),
            "MinExclusive" => format!("{value} ({of}) is not greater than {p}"),
            "MinInclusive" => format!("{value} ({of}) is less than {p}"),
            "MaxExclusive" => format!("{value} ({of}) is not less than {p}"),
            "MaxInclusive" => format!("{value} ({of}) is greater than {p}"),
            "MinLength" => format!("{value} ({of}) is shorter than {p} characters"),
            "MaxLength" => format!("{value} ({of}) is longer than {p} characters"),
            "Pattern" => format!("{value} ({of}) does not match the pattern {p}"),
            "LanguageIn" => format!("{value} ({of}) has a language tag not in sh:languageIn"),
            "UniqueLang" => format!("{of}: more than one value with the same language tag"),
            "Equals" => format!("{value} ({of}) is not also a value of {p}, or the reverse"),
            "Disjoint" => format!("{value} ({of}) is also a value of {p}"),
            "LessThan" => format!("{value} ({of}) is not less than every value of {p}"),
            "LessThanOrEquals" => {
                format!("{value} ({of}) is not less than or equal to every value of {p}")
            }
            "Not" => format!("{value} ({of}) conforms to {p}, which sh:not excludes"),
            "And" => format!("{value} ({of}) does not conform to every shape of sh:and"),
            "Or" => format!("{value} ({of}) does not conform to any shape of sh:or"),
            "Xone" => format!("{value} ({of}) does not conform to exactly one shape of sh:xone"),
            "Node" => format!("{value} ({of}) does not conform to the shape {p}"),
            "QualifiedMinCount" => {
                format!("{of}: fewer than {p} value(s) conform to the qualified value shape")
            }
            "QualifiedMaxCount" => {
                format!("{of}: more than {p} value(s) conform to the qualified value shape")
            }
            "Closed" => {
                let pred = path.unwrap_or_default();
                format!(
                    "{pred} is not allowed on {focus} by the closed shape {}",
                    self.name(d, r.shape)
                )
            }
            "HasValue" => format!("{of}: expected the value {p}"),
            "In" => format!("{value} ({of}) is not one of the values of sh:in"),
            other => format!("{value} ({of}) violates {other}"),
        }
    }
}

/// `sh:message`: the one without a language tag or in English, else the first.
fn pick_message(g: &Graph, messages: &[Id]) -> Option<String> {
    pick(
        messages
            .iter()
            .filter_map(|&m| g.term(m).literal().map(|l| (l.0, l.2))),
    )
}

/// Of (text, language tag) messages, the one without a tag or in English,
/// else the first.
fn pick<'m>(messages: impl Iterator<Item = (&'m str, Option<Iri>)>) -> Option<String> {
    let lits: Vec<(&str, Option<Iri>)> = messages.collect();
    let english = |l: &Option<Iri>| l.is_none_or(|t| t.as_str() == "en" || t.starts_with("en-"));
    lits.iter()
        .find(|l| english(&l.1))
        .or(lits.first())
        .map(|l| l.0.to_string())
}

fn term_name(g: &Graph, prefixes: &PrefixMap, id: Id) -> String {
    match g.term(id) {
        Term::Iri(i) => short(&i, prefixes),
        Term::Blank(_) => "a blank node".into(),
        t => match t.literal() {
            Some((lex, dt, lang)) => {
                if let Some(l) = lang {
                    match t.direction() {
                        Some(d) => format!("{lex:?}@{}--{d}", l.as_str()),
                        None => format!("{lex:?}@{}", l.as_str()),
                    }
                } else if dt.as_str() == format!("{XSD}string") {
                    format!("{lex:?}")
                } else if ["integer", "decimal", "double", "boolean"]
                    .iter()
                    .any(|l| dt.strip_prefix(XSD) == Some(l))
                    && crate::shacl::literal::well_formed(&dt, lex)
                {
                    // As Turtle writes them.
                    lex.to_string()
                } else {
                    format!("{lex:?}^^{}", short(&dt, prefixes))
                }
            }
            None => "a triple term".into(),
        },
    }
}

/// A path in SPARQL syntax.
fn path_name(prefixes: &PrefixMap, p: &Path) -> String {
    let group = |q: &Path| match q {
        Path::Sequence(_) | Path::Alternative(_) => format!("({})", path_name(prefixes, q)),
        _ => path_name(prefixes, q),
    };
    match p {
        Path::Predicate(i) => short(i, prefixes),
        Path::Inverse(q) => format!("^{}", group(q)),
        Path::Sequence(ps) => ps.iter().map(group).collect::<Vec<_>>().join("/"),
        Path::Alternative(ps) => ps.iter().map(group).collect::<Vec<_>>().join("|"),
        Path::ZeroOrMore(q) => format!("{}*", group(q)),
        Path::OneOrMore(q) => format!("{}+", group(q)),
        Path::ZeroOrOne(q) => format!("{}?", group(q)),
        Path::Invalid => "?".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis(uri: &str, text: &str) -> (Source, Arc<Analysis>) {
        (
            Source::Disk,
            Arc::new(Analysis::new(
                uri.into(),
                text,
                Arc::new(PrefixMap::default()),
                true,
            )),
        )
    }

    const HEAD: &str =
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n";

    fn job(docs: Vec<(Source, Arc<Analysis>)>, max: u64) -> Job {
        Job {
            generation: 1,
            docs,
            max_triples: max,
            enc: Encoding::Utf16,
            unresolved: vec![],
            soften: true,
        }
    }

    #[test]
    fn a_query_stopped_at_the_time_limit_is_a_warning_on_its_constraint() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:S sh:targetNode ex:n0 ;\n  sh:sparql [ sh:prefixes ex:P ;\n    sh:select \"SELECT $this WHERE {{ ?a ex:next* ?b . ?c ex:next* ?d . FILTER (STRLEN(CONCAT(STR(?a), STR(?d))) < 0) }}\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n"
            ),
        );
        let mut chain = HEAD.to_string();
        for i in 0..400 {
            chain.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
        }
        let data = analysis("file:///w/data.ttl", &chain);
        let out = run(&job(vec![shapes, data], 100_000), &|| false).unwrap();
        let d = &out["file:///w/shapes.ttl"];
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, Some(NumberOrString::String(TIMEOUT_CODE.into())));
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(d[0].range.start.line, 4, "on the sh:select line");
    }

    const SLOW: &str = "SELECT $this WHERE {{ ?a ex:next* ?b . ?c ex:next* ?d . FILTER (STRLEN(CONCAT(STR(?a), STR(?d))) < 0) }}";

    fn chain(n: usize) -> String {
        let mut chain = HEAD.to_string();
        for i in 0..n {
            chain.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
        }
        chain
    }

    /// A job superseded while its query runs ends there: the worker sends
    /// nothing for it and is free for the next one (it used to finish the
    /// query, to its time limit, and send the stale outcome first).
    #[test]
    fn a_superseded_job_stops_inside_its_query() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:S sh:targetNode ex:n0 ;\n  sh:sparql [ sh:prefixes ex:P ;\n    sh:select \"{SLOW}\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n"
            ),
        );
        let data = analysis("file:///w/data.ttl", &chain(400));
        let (worker, outcomes) = Worker::start().unwrap();
        let mut slow = job(vec![shapes.clone(), data.clone()], 100_000);
        slow.generation = 1;
        worker.submit(slow);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut next = job(
            vec![analysis(
                "file:///w/b.ttl",
                &format!("{HEAD}ex:a ex:p 1 .\n"),
            )],
            100,
        );
        next.generation = 2;
        worker.submit(next);
        let first = outcomes
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(first.generation, 2);
    }

    #[test]
    fn an_untyped_sparql_target_shape_is_validated() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:S sh:target [ a sh:SPARQLTarget ; sh:select \"SELECT ?this WHERE {{ ?this <http://example.com/p> 1 }}\" ] ;\n  sh:property [ sh:path ex:q ; sh:minCount 1 ] .\n"
            ),
        );
        // Nothing else marks it: no sh:targetClass, no sh:NodeShape.
        assert!(has_shapes(&shapes.1));
        let data = analysis(
            "file:///w/data.ttl",
            &format!("{HEAD}ex:a ex:p 1 .\nex:b ex:p 2 .\n"),
        );
        let out = run(&job(vec![shapes, data], 1000), &|| false).unwrap();
        let d = &out["file:///w/data.ttl"];
        assert_eq!(d.len(), 1, "{d:#?}");
        assert!(d[0].message.contains("ex:q of ex:a"), "{}", d[0].message);
    }

    /// Every failure is shown on its query: one that does not parse, and a
    /// solution that says `?failure true`.
    #[test]
    fn a_failed_query_is_a_warning_on_its_select() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:A sh:targetNode ex:a ;\n  sh:sparql [\n    sh:select \"SELECT $this WHERE {{ $this undeclared:p ?o }}\" ] .\nex:B sh:targetNode ex:a ;\n  sh:sparql [\n    sh:select \"SELECT $this ?failure WHERE {{ $this <http://example.com/p> ?o . BIND (true AS ?failure) }}\" ] .\n"
            ),
        );
        let data = analysis("file:///w/data.ttl", &format!("{HEAD}ex:a ex:p 1 .\n"));
        let out = run(&job(vec![shapes, data], 1000), &|| false).unwrap();
        let d = &out["file:///w/shapes.ttl"];
        assert_eq!(d.len(), 2, "{d:#?}");
        for (diag, line) in d.iter().zip([4, 7]) {
            assert_eq!(diag.code, Some(NumberOrString::String(FAILURE_CODE.into())));
            assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
            assert_eq!(diag.range.start.line, line, "{diag:?}");
        }
        assert!(d[0].message.contains("failed"), "{}", d[0].message);
        assert!(d[1].message.contains("?failure"), "{}", d[1].message);
        assert!(!out.contains_key("file:///w/data.ttl"));
    }

    #[test]
    fn an_ill_formed_path_is_an_error_on_the_shape() {
        // 20,000 nested inverse steps, as flat triples; the shape is skipped
        // and says so (it used to be skipped without a word).
        let mut text = format!(
            "{HEAD}ex:S sh:targetNode ex:a ;\n  sh:property [ sh:path _:b0 ; sh:minCount 1 ] .\n"
        );
        for i in 0..20_000 {
            text.push_str(&format!("_:b{i} sh:inversePath _:b{} .\n", i + 1));
        }
        text.push_str("_:b20000 sh:inversePath ex:p .\n");
        let shapes = analysis("file:///w/shapes.ttl", &text);
        let data = analysis("file:///w/data.ttl", &format!("{HEAD}ex:a ex:q 1 .\n"));
        let out = run(&job(vec![shapes, data], 100_000), &|| false).unwrap();
        let d = &out["file:///w/shapes.ttl"];
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(
            d[0].code,
            Some(NumberOrString::String(INVALID_PATH_CODE.into()))
        );
        assert_eq!(d[0].range.start.line, 3, "on the sh:path line");

        // 10 steps are read and validated.
        let mut text = format!(
            "{HEAD}ex:S sh:targetNode ex:a ;\n  sh:property [ sh:path _:b0 ; sh:minCount 1 ] .\n"
        );
        for i in 0..10 {
            text.push_str(&format!("_:b{i} sh:inversePath _:b{} .\n", i + 1));
        }
        text.push_str("_:b10 sh:inversePath ex:p .\n");
        let shapes = analysis("file:///w/shapes.ttl", &text);
        let data = analysis("file:///w/data.ttl", &format!("{HEAD}ex:a ex:q 1 .\n"));
        let out = run(&job(vec![shapes, data], 100_000), &|| false).unwrap();
        // The value is missing: a violation, not an error about the path.
        let d = &out["file:///w/data.ttl"];
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].code, Some(NumberOrString::String(CODE.into())));
        assert!(!out.contains_key("file:///w/shapes.ttl"));
    }

    /// A cartesian SELECT over a few hundred triples has far more solutions
    /// than the budget keeps: it stops, once, with a note.
    #[test]
    fn results_past_the_budget_are_one_note() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:S sh:targetNode ex:x ;\n  sh:sparql [ sh:select \"SELECT $this ?value WHERE {{ ?a ?p ?o . ?value ?q ?r }}\" ] .\n"
            ),
        );
        let mut data = HEAD.to_string();
        for i in 0..400 {
            data.push_str(&format!("ex:s{i} ex:p {i} .\n"));
        }
        let data = analysis("file:///w/data.ttl", &data);
        let start = std::time::Instant::now();
        let out = run(&job(vec![shapes.clone(), data.clone()], 100_000), &|| false).unwrap();
        assert!(start.elapsed().as_secs_f64() < 10.0);
        let all: Vec<&Diagnostic> = out.values().flatten().collect();
        let notes: Vec<&&Diagnostic> = all
            .iter()
            .filter(|d| d.message.starts_with("SHACL validation stopped after"))
            .collect();
        assert_eq!(notes.len(), 1, "{notes:#?}");
        assert_eq!(notes[0].severity, Some(DiagnosticSeverity::INFORMATION));
        assert!(all.len() <= 2 * (MAX_PER_DOC + 1), "{}", all.len());
    }

    /// The per-file limit keeps the most severe results, not the first.
    #[test]
    fn the_per_file_limit_keeps_errors_over_warnings() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:W sh:targetClass ex:W ; sh:property [ sh:path ex:p ; sh:minCount 1 ; sh:severity sh:Warning ] .\nex:E sh:targetClass ex:E ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\n"
            ),
        );
        let mut data = HEAD.to_string();
        for i in 0..1500 {
            data.push_str(&format!("ex:w{i} a ex:W .\n"));
        }
        for i in 0..10 {
            data.push_str(&format!("ex:e{i} a ex:E .\n"));
        }
        let data = analysis("file:///w/data.ttl", &data);
        let out = run(&job(vec![shapes, data], 100_000), &|| false).unwrap();
        let d = &out["file:///w/data.ttl"];
        assert_eq!(d.len(), MAX_PER_DOC + 1);
        let errors = d
            .iter()
            .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
            .count();
        assert_eq!(errors, 10);
        assert_eq!(
            d.last().unwrap().message,
            "510 more SHACL results in this file are not shown"
        );
    }

    #[test]
    fn violation_on_the_triple_with_related_shape() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:S a sh:NodeShape ; sh:targetClass ex:P ;\n  sh:property [ sh:path ex:age ; sh:datatype <http://www.w3.org/2001/XMLSchema#integer> ; sh:message \"Age must be an integer\" ] ;\n  sh:property [ sh:path ex:name ; sh:minCount 1 ] .\n"
            ),
        );
        let data = analysis(
            "file:///w/data.ttl",
            &format!("{HEAD}ex:a a ex:P ;\n  ex:age \"old\" .\n"),
        );
        let out = run(&job(vec![shapes, data], 1000), &|| false).unwrap();
        let d = &out["file:///w/data.ttl"];
        assert_eq!(d.len(), 2, "{d:#?}");
        let age = d
            .iter()
            .find(|d| d.message == "Age must be an integer")
            .unwrap();
        assert_eq!(age.range.start.line, 3);
        assert_eq!(age.range.start.character, 9);
        assert_eq!(age.severity, Some(DiagnosticSeverity::ERROR));
        let rel = &age.related_information.as_ref().unwrap()[0];
        assert_eq!(rel.location.uri.as_str(), "file:///w/shapes.ttl");
        let name = d.iter().find(|d| d.message.contains("ex:name")).unwrap();
        assert_eq!(
            name.message,
            "ex:name of ex:a: expected at least 1 value(s)"
        );
        // On the focus node's description.
        assert_eq!(name.range.start.line, 2);
        assert!(!out.contains_key("file:///w/shapes.ttl"));
    }

    #[test]
    fn a_dataset_shows_only_the_results_its_data_changes() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:S a sh:NodeShape ; sh:targetClass ex:P ;\n  sh:property [ sh:path ex:name ; sh:minCount 1 ] ;\n  sh:property [ sh:path ex:age ; sh:datatype <http://www.w3.org/2001/XMLSchema#integer> ] ;\n  sh:property [ sh:path ex:friend ; sh:nodeKind sh:IRI ] ;\n  sh:sparql [ sh:message \"no ex:bad\" ; sh:select \"SELECT $this ?value WHERE {{ $this <http://example.com/bad> ?value }}\" ] .\n"
            ),
        );
        let data = analysis(
            "file:///w/data.ttl",
            &format!(
                "{HEAD}ex:a a ex:P ;\n  ex:age \"x\" ;\n  ex:bad 1 ; ex:friend [ ] .\nex:b ex:age \"y\" ; ex:name \"B\" .\n"
            ),
        );
        let trig = |text: &str| analysis("file:///w/g.trig", &format!("{HEAD}{text}"));
        let check = |g: &str| {
            let docs = vec![shapes.clone(), data.clone(), trig(g)];
            run(&job(docs, 1000), &|| false).unwrap()
        };
        let messages = |out: &ByUri, uri: &str| {
            let mut m: Vec<String> = out
                .get(uri)
                .into_iter()
                .flatten()
                .map(|d| d.message.clone())
                .collect();
            m.sort();
            m
        };
        let shared = vec![
            "\"x\" (ex:age of ex:a) is not a valid xsd:integer literal".to_string(),
            "a blank node (ex:friend of ex:a) is not of node kind sh:IRI".into(),
            "ex:name of ex:a: expected at least 1 value(s)".into(),
            "no ex:bad".into(),
        ];
        // Unrelated to ex:a, or mentioning it without changing its results
        // (its own blank node numbers those of the shared files differently):
        // shown once, in the shared file.
        for g in [
            "ex:g { ex:c ex:knows ex:d . }",
            "ex:g { ex:a ex:knows [ ] . }",
        ] {
            let out = check(g);
            assert_eq!(messages(&out, "file:///w/data.ttl"), shared, "{g}");
            assert!(!out.contains_key("file:///w/g.trig"), "{g}: {out:#?}");
        }
        // Results the dataset adds are shown in it.
        let out = check("ex:g { ex:a ex:age \"z\" ; ex:bad 2 . }");
        assert_eq!(messages(&out, "file:///w/data.ttl"), shared);
        assert_eq!(
            messages(&out, "file:///w/g.trig"),
            [
                "\"z\" (ex:age of ex:a) is not a valid xsd:integer literal",
                "no ex:bad"
            ]
        );
        // Also one on a shared triple: on the node in the dataset.
        let out = check("ex:g { ex:b a ex:P . }");
        assert_eq!(messages(&out, "file:///w/data.ttl"), shared);
        let d = &out["file:///w/g.trig"];
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(
            d[0].message,
            "\"y\" (ex:age of ex:b) is not a valid xsd:integer literal"
        );
        assert_eq!(d[0].range.start.line, 2);
    }

    #[test]
    fn severity_paths_and_skips() {
        let doc = analysis(
            "file:///w/a.ttl",
            &format!(
                "{HEAD}ex:S sh:targetNode ex:a ;\n  sh:property [ sh:path ( ex:p [ sh:inversePath ex:q ] ) ; sh:maxCount 0 ; sh:severity sh:Warning ] .\nex:a ex:p ex:b . ex:c ex:q ex:b .\n"
            ),
        );
        let out = run(&job(vec![doc.clone()], 1000), &|| false).unwrap();
        let d = &out["file:///w/a.ttl"];
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d[0].message,
            "ex:p/^ex:q of ex:a: expected at most 0 value(s)"
        );
        let out = run(&job(vec![doc.clone()], 2), &|| false).unwrap();
        assert_eq!(
            out["file:///w/a.ttl"][0].code,
            Some(NumberOrString::String(SKIPPED_CODE.into()))
        );
        assert!(run(&job(vec![doc], 1000), &|| true).is_none());
        let plain = analysis("file:///w/b.ttl", &format!("{HEAD}ex:a ex:p ex:b .\n"));
        assert!(run(&job(vec![plain], 0), &|| false).unwrap().is_empty());
    }

    #[test]
    fn unresolved_imports_soften_what_missing_data_could_change() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:S a sh:NodeShape ; sh:targetClass ex:P ;\n  sh:property [ sh:path ex:age ; sh:datatype <http://www.w3.org/2001/XMLSchema#integer> ] ;\n  sh:property [ sh:path ex:kind ; sh:class ex:Kind ] ;\n  sh:property [ sh:path ex:unit ; sh:in ( ex:metre ) ] ;\n  sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:severity sh:Info ] .\n"
            ),
        );
        let data = analysis(
            "file:///w/data.ttl",
            &format!(
                "{HEAD}ex:a a ex:P ;\n  ex:age \"old\" ;\n  ex:kind ex:k ;\n  ex:unit <http://other.org/u#m> .\n"
            ),
        );
        let by_message = |out: &ByUri, needle: &str| {
            out["file:///w/data.ttl"]
                .iter()
                .find(|d| d.message.contains(needle))
                .cloned()
                .unwrap()
        };
        let mut j = job(vec![shapes, data], 1000);
        let plain = run(&j, &|| false).unwrap();
        assert_eq!(
            by_message(&plain, "ex:kind").severity,
            Some(DiagnosticSeverity::ERROR)
        );
        j.unresolved = ["a", "b", "c", "d"]
            .map(|s| format!("http://x.org/{s}"))
            .to_vec();
        let out = run(&j, &|| false).unwrap();
        let class = by_message(&out, "ex:kind");
        assert_eq!(class.severity, Some(DiagnosticSeverity::WARNING));
        assert!(
            class.message.ends_with(
                " (may change once these imports resolve: http://x.org/a, http://x.org/b, http://x.org/c and 1 more)"
            ),
            "{}",
            class.message
        );
        // A value in a namespace nothing loaded describes.
        let unit = by_message(&out, "ex:unit");
        assert_eq!(unit.severity, Some(DiagnosticSeverity::WARNING));
        // A literal on a described node: no import can fix it.
        let age = by_message(&out, "ex:age");
        assert_eq!(age.severity, Some(DiagnosticSeverity::ERROR));
        assert!(!age.message.contains("imports"));
        // Never raised, never annotated.
        let name = by_message(&out, "ex:name");
        assert_eq!(name.severity, Some(DiagnosticSeverity::INFORMATION));
        assert!(!name.message.contains("imports"));

        // `imports.unresolved: "ignore"`: as without unresolved imports.
        j.soften = false;
        let ignored = run(&j, &|| false).unwrap();
        assert_eq!(ignored, plain);
        for needle in ["ex:kind", "ex:unit"] {
            let d = by_message(&ignored, needle);
            assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR), "{needle}");
            assert!(!d.message.contains("imports"), "{}", d.message);
        }
    }

    /// The text each result in `uri` covers (ASCII `text`), sorted.
    fn covered(out: &ByUri, uri: &str, text: &str) -> Vec<String> {
        let lines: Vec<&str> = text.split('\n').collect();
        let offset = |p: Position| {
            lines[..p.line as usize]
                .iter()
                .map(|l| l.len() + 1)
                .sum::<usize>()
                + p.character as usize
        };
        let mut c: Vec<String> = out
            .get(uri)
            .into_iter()
            .flatten()
            .map(|d| text[offset(d.range.start)..offset(d.range.end)].to_string())
            .collect();
        c.sort();
        c
    }

    const PERSON_SHAPES: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:P ;\n  sh:property [ sh:path ex:name ; sh:minCount 1 ] ;\n  sh:property [ sh:path ex:age ; sh:datatype <http://www.w3.org/2001/XMLSchema#integer> ] ;\n  sh:property [ sh:path ex:knows ; sh:class ex:P ] ;\n  sh:property [ sh:path ex:nick ; sh:maxCount 1 ] .\n";

    #[test]
    fn jsonld_results_on_the_key_value_id_or_brace() {
        let shapes = analysis("file:///w/shapes.ttl", &format!("{HEAD}{PERSON_SHAPES}"));
        // ann conforms. bob: no name (his @id). cy: age not an integer (the
        // value); knows a non-P (the value). A node without @id and two
        // nicks (the key). A nested node with an @id and no name (its own
        // @id, not the outer one); a nested one without (its own brace).
        let text = r#"{
  "@context": { "@vocab": "http://example.com/" },
  "@graph": [
    { "@id": "http://example.com/ann", "@type": "P", "name": "Ann", "age": 1 },
    { "@id": "http://example.com/bob", "@type": "P" },
    { "@id": "http://example.com/cy", "@type": "P", "name": "Cy", "age": "old",
      "knows": { "@id": "http://example.com/zed", "label": "Z" } },
    { "@type": "P", "name": "Ed", "nick": ["E", "Eddie"] },
    { "@id": "http://example.com/fay", "@type": "P", "name": "Fay",
      "knows": { "@id": "http://example.com/gil", "@type": "P" } },
    { "@id": "http://example.com/hal", "@type": "P", "name": "Hal",
      "knows": { "@type": "P", "age": 2 } }
  ]
}
"#;
        let data = analysis("file:///w/people.jsonld", text);
        let out = run(&job(vec![shapes, data], 1000), &|| false).unwrap();
        assert_eq!(
            covered(&out, "file:///w/people.jsonld", text),
            [
                "\"old\"",
                "http://example.com/bob",
                "http://example.com/gil",
                "nick",
                "{",
                // zed is not a P: the value, the nested object.
                "{ \"@id\": \"http://example.com/zed\", \"label\": \"Z\" }",
            ],
            "{out:#?}"
        );
        // The brace is the nested node's own, on the last line but two.
        let brace = out["file:///w/people.jsonld"]
            .iter()
            .find(|d| d.range.end.character == d.range.start.character + 1)
            .unwrap();
        assert_eq!(
            (brace.range.start.line, brace.range.start.character),
            (11, 15)
        );
        assert!(!out.contains_key("file:///w/shapes.ttl"));
    }

    #[test]
    fn jsonld_without_named_graphs_is_shared_with_them_a_dataset() {
        let jsonld = |uri: &str, text: &str| analysis(uri, text).1;
        let plain = jsonld(
            "file:///w/a.jsonld",
            r#"{ "@id": "http://example.com/a", "http://example.com/p": 1 }"#,
        );
        assert!(!is_dataset(&plain));
        let named = jsonld(
            "file:///w/g.jsonld",
            r#"{ "@id": "http://example.com/g", "@graph": [ { "@id": "http://example.com/a", "http://example.com/p": 1 } ] }"#,
        );
        assert!(is_dataset(&named));
        // A default-graph @graph (no @id) is not a named graph.
        let default = jsonld(
            "file:///w/d.jsonld",
            r#"{ "@graph": [ { "@id": "http://example.com/a", "http://example.com/p": 1 } ] }"#,
        );
        assert!(!is_dataset(&default));
        assert!(is_dataset(&analysis("file:///w/x.trig", "").1));
        assert!(!is_dataset(&analysis("file:///w/x.ttl", "").1));

        // Shapes in plain JSON-LD validate Turtle data (one shared unit),
        // and plain JSON-LD data is merged with Turtle data.
        let shapes = analysis(
            "file:///w/shapes.jsonld",
            r#"{
  "@context": { "sh": "http://www.w3.org/ns/shacl#", "ex": "http://example.com/" },
  "@id": "ex:S", "@type": "sh:NodeShape",
  "sh:targetClass": { "@id": "ex:P" },
  "sh:property": { "sh:path": { "@id": "ex:name" }, "sh:maxCount": 1 }
}"#,
        );
        let ttl = format!("{HEAD}ex:t a ex:P ; ex:name \"T\" .\n");
        let data = analysis("file:///w/data.ttl", &ttl);
        let more = r#"{ "@id": "http://example.com/t", "http://example.com/name": "U" }"#;
        let docs = vec![
            shapes.clone(),
            data.clone(),
            analysis("file:///w/more.jsonld", more),
        ];
        let out = run(&job(docs, 1000), &|| false).unwrap();
        // Two names once merged: on the first key, in the Turtle file.
        assert_eq!(
            covered(&out, "file:///w/data.ttl", &ttl),
            ["ex:name"],
            "{out:#?}"
        );
        assert!(!out.contains_key("file:///w/more.jsonld"));
        // The same name in a named graph: a dataset, its result in it, and
        // never merged with another dataset.
        let run_g = |g: &str| {
            format!(
                r#"{{ "@id": "http://example.com/{g}", "@graph": [ {{ "@id": "http://example.com/t", "http://example.com/name": "U" }} ] }}"#
            )
        };
        let (g1, g2) = (run_g("g1"), run_g("g2"));
        let docs = vec![
            shapes,
            data,
            analysis("file:///w/g1.jsonld", &g1),
            analysis("file:///w/g2.jsonld", &g2),
        ];
        let out = run(&job(docs, 1000), &|| false).unwrap();
        assert_eq!(
            covered(&out, "file:///w/g1.jsonld", &g1),
            ["http://example.com/name"]
        );
        assert_eq!(
            covered(&out, "file:///w/g2.jsonld", &g2),
            ["http://example.com/name"]
        );
        assert!(!out.contains_key("file:///w/data.ttl"), "{out:#?}");
    }

    #[test]
    fn sparql_batches_jsonld_focus_nodes() {
        let shapes = analysis(
            "file:///w/shapes.ttl",
            &format!(
                "{HEAD}ex:S a sh:NodeShape ; sh:targetClass ex:P ;\n  sh:sparql [ sh:message \"no ex:bad\" ; sh:select \"SELECT $this ?value WHERE {{ $this <http://example.com/bad> ?value }}\" ] .\n"
            ),
        );
        // 40 focus nodes, more than a batch needs: 20 with an @id, 20
        // without; every other one is bad.
        let nodes: Vec<String> = (0..40)
            .map(|i| {
                let id = if i < 20 {
                    format!("\"@id\": \"ex:n{i}\", ")
                } else {
                    String::new()
                };
                let bad = if i % 2 == 0 { ", \"ex:bad\": 1" } else { "" };
                format!("    {{ {id}\"@type\": \"ex:P\"{bad} }}")
            })
            .collect();
        let text = format!(
            "{{\n  \"@context\": {{ \"ex\": \"http://example.com/\" }},\n  \"@graph\": [\n{}\n  ]\n}}\n",
            nodes.join(",\n")
        );
        let data = analysis("file:///w/many.jsonld", &text);
        let out = run(&job(vec![shapes, data], 100_000), &|| false).unwrap();
        let d = &out["file:///w/many.jsonld"];
        assert_eq!(d.len(), 20, "{d:#?}");
        assert!(d.iter().all(|d| d.message == "no ex:bad"));
        let mut expected: Vec<String> = (0..20).step_by(2).map(|i| format!("ex:n{i}")).collect();
        expected.extend(std::iter::repeat_n("{".to_string(), 10));
        expected.sort();
        assert_eq!(covered(&out, "file:///w/many.jsonld", &text), expected);
        // Each brace on its own node's line (lines 3.. hold the nodes).
        let mut lines: Vec<u32> = d.iter().map(|d| d.range.start.line).collect();
        lines.sort();
        assert_eq!(lines, (3..43).step_by(2).collect::<Vec<u32>>());
    }

    /// A shape over `persons` shared people, and `datasets` TriG datasets of
    /// three people each, one of them wrong.
    fn many_datasets(persons: usize, datasets: usize) -> Vec<(Source, Arc<Analysis>)> {
        let mut shared = format!(
            "{HEAD}ex:S a sh:NodeShape ; sh:targetClass ex:P ;\n  sh:property [ sh:path ex:name ; sh:minCount 1 ] .\n"
        );
        for i in 0..persons {
            shared.push_str(&format!("ex:s{i} a ex:P ; ex:name \"S{i}\" .\n"));
        }
        let mut docs = vec![analysis("file:///w/shared.ttl", &shared)];
        for d in 0..datasets {
            let text = format!(
                "{HEAD}ex:g{d} {{ ex:d{d}a a ex:P ; ex:name \"A\" . ex:d{d}b a ex:P ; ex:name \"B\" . ex:d{d}c a ex:P . }}\n"
            );
            docs.push(analysis(&format!("file:///w/d{d:03}.trig"), &text));
        }
        docs
    }

    /// Every dataset used to rebuild a graph of all the shared triples and
    /// itself (200 datasets over 75,000 shared triples cost four times the run
    /// with none). The shared graph is built once and each dataset is read
    /// over it: the triples indexed are the shared ones and each dataset's,
    /// once.
    #[test]
    fn datasets_are_read_over_one_shared_graph() {
        let datasets = 60;
        let docs = many_datasets(500, datasets);
        let triples: u64 = docs.iter().map(|d| d.1.parsed.triples.len() as u64).sum();
        let before = crate::rdf::graph::INDEXED.with(|c| c.get());
        let out = run(&job(docs.clone(), 1_000_000), &|| false).unwrap();
        let indexed = crate::rdf::graph::INDEXED.with(|c| c.get()) - before;
        assert_eq!(indexed, triples, "triples indexed in the run");
        // The same results as one dataset at a time: the one wrong node of
        // each, in its dataset; none for the shared people.
        assert!(!out.contains_key("file:///w/shared.ttl"));
        for d in 0..datasets {
            let list = &out[&format!("file:///w/d{d:03}.trig")];
            assert_eq!(list.len(), 1, "{list:#?}");
            assert_eq!(
                list[0].message,
                format!("ex:name of ex:d{d}c: expected at least 1 value(s)")
            );
        }
    }

    /// 200 datasets over the shared graph cost about as much as none, in the
    /// work counted (a wall-time bound would be noise on a loaded machine).
    #[test]
    fn two_hundred_datasets_index_no_more_than_their_own_triples() {
        let docs = many_datasets(2000, 200);
        let shared: u64 = docs[0].1.parsed.triples.len() as u64;
        let own: u64 = docs[1..]
            .iter()
            .map(|d| d.1.parsed.triples.len() as u64)
            .sum();
        let before = crate::rdf::graph::INDEXED.with(|c| c.get());
        let out = run(&job(docs, 10_000_000), &|| false).unwrap();
        let indexed = crate::rdf::graph::INDEXED.with(|c| c.get()) - before;
        assert_eq!(indexed, shared + own);
        assert!(indexed <= 2 * shared, "{indexed} vs {shared}");
        assert_eq!(out.len(), 200);
    }

    /// A dataset's focus nodes are found among the nodes it mentions, not by
    /// walking the shared graph's (25,000 instances for each of 200 datasets).
    #[test]
    fn a_dataset_looks_for_its_targets_among_its_own_nodes() {
        let docs = many_datasets(2000, 200);
        let before = crate::shacl::validate::TARGETS_LOOKED_AT.with(|c| c.get());
        let out = run(&job(docs, 10_000_000), &|| false).unwrap();
        let looked = crate::shacl::validate::TARGETS_LOOKED_AT.with(|c| c.get()) - before;
        // The shared run walks its 2,000 people; each dataset its few nodes.
        assert!(looked <= 2000 + 200 * 20, "{looked} nodes looked at");
        assert_eq!(out.len(), 200);
    }

    fn validated_in(out: &ByUri, uri: &str) -> bool {
        out.get(uri)
            .into_iter()
            .flatten()
            .any(|d| d.code == Some(NumberOrString::String(CODE.into())))
    }

    /// `shacl.maxTriples` bounds the whole run: the shared triples and every
    /// dataset validated, not each dataset alone with the shared ones.
    #[test]
    fn max_triples_bounds_the_whole_run() {
        let docs = many_datasets(10, 6);
        let shared: u64 = docs[0].1.parsed.triples.len() as u64;
        let each: u64 = docs[1].1.parsed.triples.len() as u64;
        // Room for the shared triples and two datasets.
        let out = run(&job(docs.clone(), shared + 2 * each), &|| false).unwrap();
        let skipped = |uri: &str| {
            out.get(uri).into_iter().flatten().any(|d| {
                d.code == Some(NumberOrString::String(SKIPPED_CODE.into()))
                    && d.message.contains(&format!("{}", shared + 3 * each))
            })
        };
        let validated = |uri: &str| validated_in(&out, uri);
        for d in 0..2 {
            assert!(
                validated(&format!("file:///w/d{d:03}.trig")),
                "{d}: {out:#?}"
            );
        }
        for d in 2..6 {
            let uri = format!("file:///w/d{d:03}.trig");
            assert!(skipped(&uri), "{d}: {out:#?}");
            assert!(!validated(&uri));
        }
        // Each dataset alone would have fit: the limit is on the run.
        let alone = run(
            &job(vec![docs[0].clone(), docs[3].clone()], shared + each),
            &|| false,
        )
        .unwrap();
        assert!(validated_in(&alone, "file:///w/d002.trig"), "{alone:#?}");
    }
}
