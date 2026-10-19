//! SHACL validation of the whole workspace, off the main thread.
//!
//! The server hands a [`Job`] (the analyses of every workspace and external
//! source, shared, so the snapshot is cheap) to a [`Worker`] on save and once
//! after the initial index. The worker validates the union of the Turtle
//! documents' triples, and each TriG / N-Triples / N-Quads dataset together
//! with them, as data and shapes graph and sends back diagnostics per
//! document. Each job carries a generation; a newer job makes the worker
//! abandon the current one, and the server drops results whose generation is
//! not the latest.
//!
//! A result is shown on the triple (focus node, first path predicate, value)
//! when the workspace has it, else on the focus node's first description,
//! else on the constraint; never in an external source. Positions come from
//! the analyses validated, so after further edits they stay where they were
//! until the next save.

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::SOURCE;
use crate::features::hover::short;
use crate::features::navigation;
use crate::knowledge::Source;
use crate::shacl::graph::{Graph, Id, Term};
use crate::shacl::{Path, SH, ValidationResult, validate_focus};
use crate::syntax::{Iri, PrefixMap, Role, Span, XSD};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::*;
use rustc_hash::FxHashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub const CODE: &str = "shacl-violation";
pub const SKIPPED_CODE: &str = "shacl-skipped";
/// Results shown per document; the rest are summarized in one diagnostic.
const MAX_PER_DOC: usize = 1000;

pub type ByUri = FxHashMap<String, Vec<Diagnostic>>;

pub struct Job {
    pub generation: u64,
    pub docs: Vec<(Source, Arc<Analysis>)>,
    pub max_triples: u64,
    pub enc: Encoding,
    /// The workspace's `owl:imports` that are not loaded. While any is,
    /// errors that the missing data could change are shown as warnings.
    pub unresolved: Vec<String>,
}

/// How to soften errors while imports are unresolved.
struct Soften {
    /// Appended to a softened message.
    note: String,
    /// Namespaces some loaded document defines a term in.
    described: rustc_hash::FxHashSet<String>,
}

impl Soften {
    fn new(job: &Job) -> Option<Soften> {
        if job.unresolved.is_empty() {
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
/// into one node with two values.
pub fn run(job: &Job, cancelled: &dyn Fn() -> bool) -> Option<ByUri> {
    let mut docs: Vec<&(Source, Arc<Analysis>)> = job.docs.iter().collect();
    docs.sort_by(|a, b| (a.0, &a.1.uri).cmp(&(b.0, &b.1.uri)));
    let dataset = |d: &&(Source, Arc<Analysis>)| {
        d.0 != Source::External && d.1.parsed.syntax != crate::syntax::Syntax::Turtle
    };
    let shared: Vec<&(Source, Arc<Analysis>)> =
        docs.iter().copied().filter(|d| !dataset(d)).collect();
    let mut out = ByUri::default();
    let soften = Soften::new(job);
    let soften = soften.as_ref();
    // The shared graph, its results placed in the Turtle documents.
    validate_unit(job, &shared, None, soften, cancelled, &mut out)?;
    for d in docs.iter().copied().filter(dataset) {
        // The dataset first, so results it causes are placed in it; only its
        // own nodes are focus nodes (the shared ones were validated above).
        let mut unit = vec![d];
        unit.extend(shared.iter().copied());
        validate_unit(job, &unit, Some(&d.1), soften, cancelled, &mut out)?;
    }
    for list in out.values_mut() {
        let mut seen = rustc_hash::FxHashSet::default();
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

/// Validate one data graph (`docs` merged). With `dataset` (then `docs[0]`),
/// only nodes that dataset mentions are focus nodes. `None` when cancelled.
fn validate_unit(
    job: &Job,
    docs: &[&(Source, Arc<Analysis>)],
    dataset: Option<&Arc<Analysis>>,
    soften: Option<&Soften>,
    cancelled: &dyn Fn() -> bool,
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
    let mine: Option<rustc_hash::FxHashSet<Id>> = dataset.map(|_| {
        g.triples
            .iter()
            .zip(&g.origin)
            .filter(|(_, o)| o.0 == 0)
            .flat_map(|(t, _)| [t.s, t.o])
            .collect()
    });
    let keep = |id: Id| mine.as_ref().is_none_or(|m| m.contains(&id));
    let results = validate_focus(&g, cancelled, &keep)?;
    let mut place = Placer {
        g: &g,
        docs,
        enc: job.enc,
        prefixes: FxHashMap::default(),
        soften,
    };
    for r in &results {
        if let Some((uri, d)) = place.diagnostic(r) {
            out.entry(uri).or_default().push(d);
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

    /// Where a result goes: (document, span).
    fn location(&self, r: &ValidationResult) -> Option<(u32, Span)> {
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
        match self.g.term(node) {
            Term::Iri(i) => (0..self.docs.len() as u32)
                .filter(|&d| self.in_workspace(d))
                .find_map(|d| Some((d, *self.doc(d).defs.get(&i)?.first()?)))
                .or_else(|| {
                    // Only referenced: its first occurrence.
                    (0..self.docs.len() as u32)
                        .filter(|&d| self.in_workspace(d))
                        .find_map(|d| {
                            let a = self.doc(d);
                            let r = a.refs_of(&i).find(|r| r.role != Role::Predicate)?;
                            Some((d, r.span))
                        })
                }),
            Term::Blank(_) => {
                let ws = |t: &(u32, &crate::shacl::graph::GTriple)| {
                    self.in_workspace(self.g.origin[t.0 as usize].0)
                };
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
        }
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
    }
}
