//! SHACL validation of the whole workspace, off the main thread.
//!
//! The server hands a [`Job`] (the analyses of every workspace and external
//! source, shared, so the snapshot is cheap) to a [`Worker`] on save and once
//! after the initial index. The worker validates the union of the Turtle
//! documents' triples (and of JSON-LD ones without named graphs), and each
//! TriG / N-Triples / N-Quads dataset (and JSON-LD one with named graphs)
//! together with them, as data and shapes graph and sends back diagnostics per
//! document. Each job carries a generation; a newer job makes the worker
//! abandon the current one, and the server drops results whose generation is
//! not the latest.
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
use crate::shacl::sparql::TIMED_OUT;
use crate::shacl::{Path, SH, ValidationResult, validate_report};
use crate::syntax::{Iri, PrefixMap, RDF, Role, Span, Syntax, XSD};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub const CODE: &str = "shacl-violation";
pub const SKIPPED_CODE: &str = "shacl-skipped";
pub const TIMEOUT_CODE: &str = "shacl-sparql-timeout";
/// Results shown per document; the rest are summarized in one diagnostic.
const MAX_PER_DOC: usize = 1000;

pub type ByUri = FxHashMap<String, Vec<Diagnostic>>;

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

pub struct Outcome {
    pub generation: u64,
    pub diagnostics: ByUri,
}

/// The validation thread. It blocks on its channel while idle and ends
/// when the worker is dropped.
pub struct Worker {
    jobs: Sender<Job>,
    latest: Arc<AtomicU64>,
}

impl Worker {
    pub fn start() -> (Worker, Receiver<Outcome>) {
        let (jobs, job_rx) = unbounded::<Job>();
        let (out_tx, out_rx) = unbounded();
        let latest = Arc::new(AtomicU64::new(0));
        let seen = latest.clone();
        std::thread::Builder::new()
            .name("shacl".into())
            // Defence in depth: recursion is bounded, but generously.
            .stack_size(16 << 20)
            .spawn(move || {
                while let Ok(mut job) = job_rx.recv() {
                    // Only the newest queued job matters.
                    while let Ok(newer) = job_rx.try_recv() {
                        job = newer;
                    }
                    let generation = job.generation;
                    let cancelled = || seen.load(Ordering::Relaxed) != generation;
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
            })
            .expect("spawn the SHACL thread");
        (Worker { jobs, latest }, out_rx)
    }

    /// Abandon the running job, if any, in favour of `generation`.
    pub fn cancel(&self, generation: u64) {
        self.latest.store(generation, Ordering::Relaxed);
    }

    pub fn submit(&self, job: Job) {
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
/// typed shape). A few hash lookups.
pub fn has_shapes(a: &Analysis) -> bool {
    [
        "targetClass",
        "targetNode",
        "targetSubjectsOf",
        "targetObjectsOf",
        "NodeShape",
        "PropertyShape",
    ]
    .iter()
    .any(|l| Iri::get(&format!("{SH}{l}")).is_some_and(|i| a.refs_by_iri.contains_key(&i)))
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
pub fn run(job: &Job, cancelled: &dyn Fn() -> bool) -> Option<ByUri> {
    let mut docs: Vec<&(Source, Arc<Analysis>)> = job.docs.iter().collect();
    docs.sort_by(|a, b| (a.0, &a.1.uri).cmp(&(b.0, &b.1.uri)));
    let dataset = |d: &&(Source, Arc<Analysis>)| d.0 != Source::External && is_dataset(&d.1);
    let shared: Vec<&(Source, Arc<Analysis>)> =
        docs.iter().copied().filter(|d| !dataset(d)).collect();
    let mut out = ByUri::default();
    let soften = Soften::new(job);
    let soften = soften.as_ref();
    // The shared graph, its results placed in the Turtle documents.
    let mut shared_results = FxHashSet::default();
    validate_unit(
        job,
        &shared,
        None,
        soften,
        cancelled,
        &mut shared_results,
        &mut out,
    )?;
    for d in docs.iter().copied().filter(dataset) {
        // The dataset first, so results it causes are placed in it; only its
        // own nodes are focus nodes (the shared ones were validated above).
        let mut unit = vec![d];
        unit.extend(shared.iter().copied());
        validate_unit(
            job,
            &unit,
            Some(&d.1),
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
    let (d, i) = g.origin[r.param as usize];
    ResultKey {
        param: (uri(d), i),
        focus: term(r.focus),
        value: r.value.map(term),
        path: format!("{:?}", r.path),
        component: r.component,
        messages: r.computed_messages.clone(),
    }
}

/// Validate one data graph (`docs` merged). With `dataset` (then `docs[0]`),
/// only nodes that dataset mentions are focus nodes, results in
/// `shared_results` are dropped and the rest are placed in the dataset;
/// without, results are added to `shared_results`. `None` when cancelled.
fn validate_unit<'j>(
    job: &Job,
    docs: &[&'j (Source, Arc<Analysis>)],
    dataset: Option<&Arc<Analysis>>,
    soften: Option<&Soften>,
    cancelled: &dyn Fn() -> bool,
    shared_results: &mut FxHashSet<ResultKey<'j>>,
    out: &mut ByUri,
) -> Option<()> {
    let workspace = |s: Source| s != Source::External;
    let shaped = |(s, a): &&&(Source, Arc<Analysis>)| workspace(*s) && has_shapes(a);
    if !docs.iter().any(|d| shaped(&d)) {
        return Some(());
    }
    let total: u64 = docs.iter().map(|d| d.1.parsed.triples.len() as u64).sum();
    if total > job.max_triples {
        let skip = NumberOrString::String(SKIPPED_CODE.into());
        // A dataset over the limit is the one to tell; else the shape files.
        let told: Vec<&Arc<Analysis>> = match dataset {
            Some(a) => vec![a],
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
    let triples: Vec<&[crate::syntax::Triple]> =
        docs.iter().map(|d| d.1.parsed.triples.as_slice()).collect();
    let g = Graph::new(&triples);
    let mine: Option<FxHashSet<Id>> = dataset.map(|_| {
        g.triples
            .iter()
            .zip(&g.origin)
            .filter(|(_, o)| o.0 == 0)
            .flat_map(|(t, _)| [t.s, t.o])
            .collect()
    });
    let keep = |id: Id| mine.as_ref().is_none_or(|m| m.contains(&id));
    let report = validate_report(&g, cancelled, &keep)?;
    let mut place = Placer {
        g: &g,
        docs,
        enc: job.enc,
        prefixes: FxHashMap::default(),
        soften,
        in_dataset: dataset.is_some(),
    };
    for r in &report.results {
        let key = result_key(&g, docs, r);
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
    for f in report.failures.iter().filter(|f| f.message == TIMED_OUT) {
        if let Some((uri, d)) = place.timed_out(f.node) {
            let list = out.entry(uri).or_default();
            if !list.contains(&d) {
                list.push(d);
            }
        }
    }
    Some(())
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
    /// Whether `docs[0]` is a dataset whose results all go in it.
    in_dataset: bool,
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

    fn in_workspace(&self, d: u32) -> bool {
        self.docs[d as usize].0 != Source::External
    }

    /// The (document, triple) behind graph triple `t`.
    fn origin(&self, t: u32) -> (u32, &crate::syntax::Triple) {
        let (d, i) = self.g.origin[t as usize];
        (d, &self.doc(d).parsed.triples[i as usize])
    }

    /// Where a result goes: (document, span). A dataset's result, caused by
    /// its data, goes in the dataset (which mentions every focus node).
    /// A warning on the `sh:select` or `sh:ask` of constraint `node`, whose
    /// query hit the time limit.
    fn timed_out(&self, node: Id) -> Option<(String, Diagnostic)> {
        let query = [
            Iri::intern(&format!("{SH}select")),
            Iri::intern(&format!("{SH}ask")),
        ];
        let (t, _) = self.g.out(node).find(|(_, t)| query.contains(&t.p))?;
        let (d, triple) = self.origin(t);
        if !self.in_workspace(d) {
            return None;
        }
        let a = self.doc(d);
        let span = Span::new(
            triple.p_span.start,
            triple.o_span.end.max(triple.p_span.end),
        );
        Some((
            a.uri.clone(),
            Diagnostic {
                range: a.range(self.enc, span),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String(TIMEOUT_CODE.into())),
                source: Some(SOURCE.into()),
                message: TIMED_OUT.into(),
                ..Default::default()
            },
        ))
    }

    fn location(&self, r: &ValidationResult) -> Option<(u32, Span)> {
        let at = self.anywhere(r);
        if self.in_dataset && at.is_none_or(|(d, _)| d != 0) {
            return self.description_in(0, r.focus).or(at);
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
            let ws = |&&(t, _): &&(u32, bool)| self.in_workspace(self.g.origin[t as usize].0);
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
            Term::Iri(i) => (0..self.docs.len() as u32)
                .filter(|&d| docs(d))
                .find_map(|d| Some((d, *self.doc(d).defs.get(&i)?.first()?)))
                .or_else(|| {
                    // Only referenced: its first occurrence.
                    (0..self.docs.len() as u32)
                        .filter(|&d| docs(d))
                        .find_map(|d| {
                            let a = self.doc(d);
                            let r = a.refs_of(&i).find(|r| r.role != Role::Predicate)?;
                            Some((d, r.span))
                        })
                }),
            Term::Blank(b) => {
                let ws =
                    |t: &(u32, &crate::rdf::graph::GTriple)| docs(self.g.origin[t.0 as usize].0);
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
        let param = g.triples[r.param as usize].o;
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
                    format!("{lex:?}@{}", l.as_str())
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
}
