//! Batch analysis: a set of files analysed as one editor workspace, without
//! the server's event loop. The browser toolbox (web/engine) and the
//! command-line `check`, `format` and `query` use it.
//!
//! Every file is opened in a fresh [`Workspace`], so knowledge crosses files
//! as it does in an editor. Files marked context inform the others but are
//! not fixed and not returned by [`fix_all`]. SHACL runs synchronously.
//! In the browser imports are never loaded: an import the files do not
//! declare themselves is reported as unresolved. On disk ([`Disk`]) they are
//! loaded as the editor loads them.
//!
//! A file has a `name`, which keys the results and the edits of fixes, and a
//! `uri`, at which it is opened. [`BatchFile::named`] places a name below
//! `file:///work/` (the browser's convention); [`BatchFile::at_path`] opens a
//! file on disk at its own `file:` URI.

use crate::analysis::{Analysis, Encoding, SparqlAnalysis, offset_in};
use crate::diagnose::{self, DiagnosticInputs, Doc};
use crate::features::quickfix::Fixer;
use crate::features::shacl_validate;
use crate::features::{format, lint};
use crate::imports;
use crate::knowledge::Source;
use crate::query::{self, Answer, Limits, QueryError};
use crate::settings::Settings;
use crate::sparql::features as sparql_features;
use crate::syntax::{PrefixMap, Severity, Syntax};
use crate::workspace::{DocKind, Workspace};
use line_index::{LineIndex, TextSize};
use lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    Diagnostic, DiagnosticSeverity, Position, Range, TextDocumentIdentifier, TextEdit, Uri,
};
use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Rounds of [`fix_all`] before giving up on fixes that keep producing fixes.
const MAX_FIX_ROUNDS: usize = 20;

/// Diagnostic codes whose fixes rename a term to a "did you mean" candidate.
/// The rename may change what the data says, so these fixes are unsafe.
const RENAME_CODES: [&str; 2] = ["unknown-term", lint::CODE];

/// One input file.
#[derive(Clone, Debug)]
pub struct BatchFile {
    /// What results and fix edits call the file. Unique within a batch.
    pub name: String,
    /// The URI the file is opened at.
    pub uri: String,
    pub text: String,
    /// Context only: what the file says counts, but [`fix_all`] neither
    /// fixes it nor returns it.
    pub context: bool,
}

impl BatchFile {
    /// A file named by a '/'-separated relative path (e.g. `shapes/a.ttl`),
    /// opened at that path below `file:///work/`.
    pub fn named(name: &str, text: String, context: bool) -> BatchFile {
        BatchFile {
            name: name.to_string(),
            uri: work_uri(name),
            text,
            context,
        }
    }

    /// A file on disk, opened at the `file:` URI of `path` (absolute) and
    /// called `name` in the results. None when `path` has no `file:` URI
    /// (a relative path, or the browser build, which has no file system).
    pub fn at_path(path: &Path, name: &str, text: String, context: bool) -> Option<BatchFile> {
        Some(BatchFile {
            name: name.to_string(),
            uri: crate::workspace::path_to_uri(path)?,
            text,
            context,
        })
    }
}

/// How positions in results count columns. Lines start at 0 in all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Columns {
    /// UTF-8 bytes.
    Utf8,
    /// UTF-16 code units, as the LSP and JavaScript count.
    Utf16,
    /// Unicode code points, as command-line tools count.
    CodePoints,
}

impl Columns {
    /// The encoding the analysis runs in; code points are converted from
    /// UTF-8 afterwards.
    fn encoding(self) -> Encoding {
        match self {
            Columns::Utf16 => Encoding::Utf16,
            Columns::Utf8 | Columns::CodePoints => Encoding::Utf8,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Run SHACL validation (also subject to `Settings::shacl_enabled`).
    pub shacl: bool,
    pub columns: Columns,
    /// Work out the quick fixes of each problem ([`Problem::fixes`]). Off,
    /// they are empty: nothing is computed for them. [`fix_all`] finds its
    /// own either way.
    pub fixes: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            shacl: true,
            columns: Columns::Utf16,
            fixes: true,
        }
    }
}

/// A workspace on disk (the command line), where the browser has none.
#[derive(Clone, Debug, Default)]
pub struct Disk {
    /// The workspace roots: a file's `prefixes.ttl` is looked for up to its
    /// root, and import catalogs under them.
    pub roots: Vec<PathBuf>,
    /// Fetch imports that are not in the import cache.
    pub network: bool,
}

/// What [`analyse`] found.
#[derive(Clone, Debug)]
pub struct Report {
    /// One per input file, in input order (context files included).
    pub files: Vec<FileReport>,
    /// Whether some file declares SHACL shapes.
    pub has_shapes: bool,
}

#[derive(Clone, Debug)]
pub struct FileReport {
    pub name: String,
    /// `turtle`, `trig`, `ntriples`, `nquads` or `sparql`.
    pub kind: &'static str,
    /// The file's diagnostics, SHACL results last.
    pub problems: Vec<Problem>,
    /// None when the file could not be analysed.
    pub format: Option<Formatted>,
}

/// A diagnostic and its quick fixes.
#[derive(Clone, Debug)]
pub struct Problem {
    pub diagnostic: Diagnostic,
    /// Preferred fixes first; empty unless [`Options::fixes`].
    pub fixes: Vec<Fix>,
}

#[derive(Clone, Debug)]
pub struct Fix {
    pub title: String,
    pub preferred: bool,
    /// Preferred and not a term rename ([`RENAME_CODES`]): applying it does
    /// not change what the data means.
    pub safe: bool,
    /// The edits, by file name, in input order of the files.
    pub edits: Vec<(String, Vec<TextEdit>)>,
}

/// The formatter's verdict on a file.
#[derive(Clone, Debug)]
pub struct Formatted {
    /// What formatting preserves: "same graph" or "same SPARQL algebra".
    pub guarantee: &'static str,
    pub outcome: FormatOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatOutcome {
    Unchanged,
    /// The formatted text.
    Changed(String),
    /// Why the file is not formatted.
    Refused(String),
}

/// What [`fix_all`] did.
#[derive(Clone, Debug)]
pub struct FixOutcome {
    /// The files not marked context, in input order, with their new texts.
    pub files: Vec<BatchFile>,
    /// How many fixes were applied.
    pub applied: usize,
}

/// Analyse `files` as one workspace: per file its diagnostics with their
/// fixes, and the formatter's verdict.
pub fn analyse(files: &[BatchFile], settings: &Settings, opts: &Options) -> Report {
    analyse_in(files, settings, opts, None)
}

/// [`analyse`] for files on disk: project prefixes come from `prefixes.ttl`
/// up to the roots, and `owl:imports` are loaded as the editor loads them.
/// A context file gets only its SHACL results, without fixes, and no
/// formatter verdict.
pub fn analyse_on_disk(
    files: &[BatchFile],
    settings: &Settings,
    opts: &Options,
    disk: &Disk,
) -> Report {
    analyse_in(files, settings, opts, Some(disk))
}

fn analyse_in(
    files: &[BatchFile],
    settings: &Settings,
    opts: &Options,
    disk: Option<&Disk>,
) -> Report {
    let enc = opts.columns.encoding();
    let mut input = Input::new(files, settings, enc, disk, None);
    let (has_shapes, mut shacl) = input.shacl(opts.shacl);
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let uri = &f.uri;
        if disk.is_some() && f.context {
            let problems = shacl.remove(uri).unwrap_or_default();
            out.push(FileReport {
                name: f.name.clone(),
                kind: input.kind(uri),
                problems: problems
                    .into_iter()
                    .map(|diagnostic| Problem {
                        diagnostic,
                        fixes: Vec::new(),
                    })
                    .collect(),
                format: None,
            });
            continue;
        }
        let own_shacl = shacl.remove(uri).unwrap_or_default();
        let diags = input.diagnostics(uri, &own_shacl);
        let mut fixes = if opts.fixes {
            input.fixes(uri, &diags)
        } else {
            Vec::new()
        };
        fixes.resize_with(diags.len(), Vec::new);
        let problems = diags
            .into_iter()
            .zip(fixes)
            .map(|(diagnostic, fixes)| Problem { diagnostic, fixes })
            .collect();
        out.push(FileReport {
            name: f.name.clone(),
            kind: input.kind(uri),
            problems,
            format: input.format(uri),
        });
    }
    if opts.columns == Columns::CodePoints {
        let lines = LineIndexes::new(files);
        for report in &mut out {
            to_code_points(report, files, &lines);
        }
    }
    Report {
        files: out,
        has_shapes,
    }
}

/// What [`query`] gave.
#[derive(Clone, Debug)]
pub struct QueryOutcome {
    pub answer: Result<Answer, QueryError>,
    /// The prefixes to show the results with: the project's (prefixes.ttl and
    /// the `prefixes` setting, as the query sees them) and the query's own
    /// `PREFIX` lines.
    pub prefixes: PrefixMap,
}

/// Run the query `q` over `files` as one workspace: the default graph is the
/// union of the RDF files' triples. SPARQL files, and a file at the query's
/// own URI, are not data. The query and headerless data files get the
/// project's prefixes as the editor gives them; `q.uri` is the query's base.
pub fn query(
    files: &[BatchFile],
    q: &BatchFile,
    settings: &Settings,
    limits: &Limits,
) -> QueryOutcome {
    query_in(files, q, settings, limits, None)
}

/// [`query`] for files on disk: project prefixes come from `prefixes.ttl` up
/// to the roots. Imports are not loaded: the data is the files given.
pub fn query_on_disk(
    files: &[BatchFile],
    q: &BatchFile,
    settings: &Settings,
    limits: &Limits,
    disk: &Disk,
) -> QueryOutcome {
    query_in(files, q, settings, limits, Some(disk))
}

fn query_in(
    files: &[BatchFile],
    q: &BatchFile,
    settings: &Settings,
    limits: &Limits,
    disk: Option<&Disk>,
) -> QueryOutcome {
    let roots = disk.map(|d| d.roots.clone()).unwrap_or_default();
    let mut ws = Workspace::new(roots, settings.prefixes.clone());
    ws.set_network(disk.is_some_and(|d| d.network));
    // Each file is analyzed once: tokens are made only if a feature asks.
    ws.set_keep_tokens(false);
    let data: Vec<&BatchFile> = files
        .iter()
        .filter(|f| f.uri != q.uri && DocKind::detect(None, &f.uri) != DocKind::Sparql)
        .collect();
    for f in &data {
        ws.open_doc(f.uri.clone(), f.text.clone(), 1);
    }
    ws.open_doc_as(q.uri.clone(), q.text.clone(), 1, DocKind::Sparql);
    let sparql = ws.sparql(&q.uri).expect("the query was just opened");
    let analyses: Vec<Arc<Analysis>> = data.iter().filter_map(|f| ws.analysis(&f.uri)).collect();
    let docs: Vec<&Analysis> = analyses.iter().map(Arc::as_ref).collect();
    QueryOutcome {
        answer: query::run(
            &docs,
            &q.text,
            Some(sparql.base()),
            &sparql.external,
            limits,
        ),
        prefixes: query::display_prefixes(&q.text, &sparql.external),
    }
}

/// Apply every preferred fix, round after round, until none is left; unsafe
/// ones only with `include_unsafe`. A fix that touches text another fix of
/// the round touches (or inserts at the same place) waits for the next
/// round, analysed afresh.
///
/// Files marked context are part of the workspace but are not fixed: their
/// own problems are skipped, a fix that would edit one is not applied, and
/// they are left out of the outcome.
pub fn fix_all(files: &[BatchFile], settings: &Settings, include_unsafe: bool) -> FixOutcome {
    fix_all_in(files, settings, include_unsafe, None)
}

/// [`fix_all`] for files on disk, as [`analyse_on_disk`] sees them. Imports
/// are loaded once, before the first round.
pub fn fix_all_on_disk(
    files: &[BatchFile],
    settings: &Settings,
    include_unsafe: bool,
    disk: &Disk,
) -> FixOutcome {
    fix_all_in(files, settings, include_unsafe, Some(disk))
}

fn fix_all_in(
    files: &[BatchFile],
    settings: &Settings,
    include_unsafe: bool,
    disk: Option<&Disk>,
) -> FixOutcome {
    // Any encoding works here; edits are mapped back to byte offsets.
    let enc = Encoding::Utf16;
    let mut files = files.to_vec();
    let mut applied = 0;
    let mut loaded = None;
    for _ in 0..MAX_FIX_ROUNDS {
        let mut input = Input::new(&files, settings, enc, disk, loaded.take());
        loaded = input.imports.clone();
        // (file index, start, end, new text) of the edits taken this round.
        let mut taken: Vec<(usize, usize, usize, String)> = Vec::new();
        let lines = LineIndexes::new(&files);
        for f in files.iter().filter(|f| !f.context) {
            let diags = input.diagnostics(&f.uri, &[]);
            for fixes in input.fixes(&f.uri, &diags) {
                let Some(fix) = fixes
                    .into_iter()
                    .find(|x| x.preferred && (x.safe || include_unsafe))
                else {
                    continue;
                };
                let Some(edits) = edits_of(&fix, &files, &lines, enc) else {
                    continue;
                };
                if edits.iter().any(|(i, ..)| files[*i].context) {
                    continue;
                }
                let clash = edits.iter().any(|(i, s, e, _)| {
                    taken
                        .iter()
                        .any(|(j, ts, te, _)| i == j && ((s < te && ts < e) || s == ts))
                });
                if clash {
                    continue;
                }
                taken.extend(edits);
                applied += 1;
            }
        }
        if taken.is_empty() {
            break;
        }
        taken.sort_by_key(|t| std::cmp::Reverse((t.0, t.1)));
        for (i, s, e, new) in taken {
            files[i].text.replace_range(s..e, &new);
        }
    }
    files.retain(|f| !f.context);
    FixOutcome { files, applied }
}

/// The imports the input does not declare itself: in the order
/// [`imports::unresolved_without_loading`] gives them, and each with its status.
struct Unresolved {
    iris: Vec<String>,
    status: HashMap<String, imports::Status>,
}

/// The input as an editor workspace.
struct Input<'a> {
    ws: Workspace,
    /// (name, URI) per file, in input order.
    names: Vec<(String, String)>,
    settings: &'a Settings,
    enc: Encoding,
    /// [`Input::unresolved`], computed once: the texts never change under
    /// an `Input` (each round of "fix all" makes a new one).
    unresolved: Option<Unresolved>,
    /// On disk: the imports, loaded into the knowledge index.
    imports: Option<Arc<imports::Resolved>>,
}

impl<'a> Input<'a> {
    /// On disk, `loaded` are the imports resolved for an earlier input of the
    /// same workspace; None resolves them afresh.
    fn new(
        files: &[BatchFile],
        settings: &'a Settings,
        enc: Encoding,
        disk: Option<&Disk>,
        loaded: Option<Arc<imports::Resolved>>,
    ) -> Input<'a> {
        let roots = disk.map(|d| d.roots.clone()).unwrap_or_default();
        let mut ws = Workspace::new(roots, settings.prefixes.clone());
        ws.set_network(disk.is_some_and(|d| d.network));
        // Each file is analyzed once: tokens are made only if a feature asks.
        ws.set_keep_tokens(false);
        for f in files {
            ws.open_doc(f.uri.clone(), f.text.clone(), 1);
        }
        let imports = disk.map(|d| {
            let r = loaded.unwrap_or_else(|| {
                Arc::new(imports::resolve_now(ws.knowledge(), &d.roots, d.network))
            });
            let k = ws.knowledge_mut();
            for (key, a) in &r.loaded {
                k.add_external(key, a.clone());
            }
            r
        });
        Input {
            imports,
            ws,
            names: files
                .iter()
                .map(|f| (f.name.clone(), f.uri.clone()))
                .collect(),
            settings,
            enc,
            unresolved: None,
        }
    }

    /// The input index of the file at `uri`.
    fn index_of(&self, uri: &str) -> Option<usize> {
        self.names.iter().position(|(_, u)| u == uri)
    }

    /// The imports that are not loaded: on disk, those [`imports::resolve_now`]
    /// could not load; else every one the input does not declare itself.
    fn unresolved(&mut self) -> &Unresolved {
        let ws = &mut self.ws;
        let loaded = &self.imports;
        self.unresolved.get_or_insert_with(|| {
            if let Some(r) = loaded {
                return Unresolved {
                    iris: r.unresolved(),
                    status: r
                        .status
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                };
            }
            let iris = imports::unresolved_without_loading(ws.knowledge());
            let status = iris
                .iter()
                .map(|iri| {
                    let note = imports::unloaded_note(iri);
                    (iri.clone(), imports::Status::Unresolved(note))
                })
                .collect();
            Unresolved { iris, status }
        })
    }

    /// Diagnostics of `uri` as the server computes them once indexed, with
    /// its SHACL results `shacl` last (see [`Input::shacl`]). None for a
    /// file that has no analysis.
    fn diagnostics(&mut self, uri: &str, shacl: &[Diagnostic]) -> Vec<Diagnostic> {
        self.try_diagnostics(uri, shacl).unwrap_or_default()
    }

    fn try_diagnostics(&mut self, uri: &str, shacl: &[Diagnostic]) -> Option<Vec<Diagnostic>> {
        enum Opened {
            Rdf(Arc<Analysis>),
            Sparql(Arc<SparqlAnalysis>),
        }
        let opened = if self.ws.is_sparql(uri) {
            Opened::Sparql(self.ws.sparql(uri)?)
        } else {
            Opened::Rdf(self.ws.analysis(uri)?)
        };
        self.ws.knowledge();
        self.unresolved();
        let unresolved = &self.unresolved.as_ref().expect("computed above").status;
        let status = |iri: &str| unresolved.get(iri);
        let inputs = DiagnosticInputs {
            import_status: &status,
            shacl,
            ..DiagnosticInputs::new(self.settings)
        };
        let k = self.ws.knowledge_mut();
        Some(match &opened {
            Opened::Rdf(a) => diagnose::document(Doc::Rdf(a), k, self.enc, &inputs),
            Opened::Sparql(q) => diagnose::document(Doc::Sparql(q), k, self.enc, &inputs),
        })
    }

    /// Whether some file declares shapes, and if so (and `run`, and SHACL is
    /// enabled) the SHACL results by URI.
    fn shacl(&mut self, run: bool) -> (bool, shacl_validate::ByUri) {
        let docs: Vec<_> = self
            .ws
            .knowledge()
            .docs()
            .map(|(_, s, a)| (s, a.clone()))
            .collect();
        let shapes = docs
            .iter()
            .any(|(s, a)| *s != Source::External && shacl_validate::has_shapes(a));
        if !shapes || !run || !self.settings.shacl_enabled {
            return (shapes, Default::default());
        }
        let job = shacl_validate::Job {
            generation: 0,
            docs,
            max_triples: self.settings.shacl_max_triples,
            enc: self.enc,
            // Results a missing import could change soften.
            unresolved: self.unresolved().iris.clone(),
            soften: self.settings.imports_unresolved == crate::settings::UnresolvedImports::Soften,
        };
        (
            true,
            shacl_validate::run(&job, &|| false).unwrap_or_default(),
        )
    }

    /// The quick fixes of each of `diags` in `uri`, preferred first, one
    /// list per diagnostic. Empty for a file that has no analysis.
    fn fixes(&mut self, uri: &str, diags: &[Diagnostic]) -> Vec<Vec<Fix>> {
        let enc = self.enc;
        let Ok(doc_uri) = uri.parse::<Uri>() else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(diags.len());
        if self.ws.is_sparql(uri) {
            let Some(q) = self.ws.sparql(uri) else {
                return Vec::new();
            };
            for d in diags {
                let params = CodeActionParams {
                    text_document: TextDocumentIdentifier {
                        uri: doc_uri.clone(),
                    },
                    range: d.range,
                    context: CodeActionContext {
                        diagnostics: vec![d.clone()],
                        only: Some(vec![CodeActionKind::QUICKFIX]),
                        trigger_kind: None,
                    },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                let actions =
                    sparql_features::code_actions(&q, self.ws.knowledge(), enc, &params, false);
                // A fix for this diagnostic, not "add all missing prefixes".
                let mine = actions.into_iter().filter_map(|action| match action {
                    CodeActionOrCommand::CodeAction(c)
                        if c.diagnostics
                            .as_ref()
                            .is_some_and(|ds| ds.len() == 1 && ds[0] == *d) =>
                    {
                        Some(c)
                    }
                    _ => None,
                });
                out.push(self.as_fixes(d, mine));
            }
            return out;
        }
        let Some(a) = self.ws.analysis(uri) else {
            return Vec::new();
        };
        let k = self.ws.knowledge();
        let fixer = Fixer::new(&a, k, enc, &doc_uri);
        let fixes: Vec<Vec<CodeAction>> = diags.iter().map(|d| fixer.for_diagnostic(d)).collect();
        for (d, actions) in diags.iter().zip(fixes) {
            out.push(self.as_fixes(d, actions));
        }
        out
    }

    /// `actions`, the code actions for `d`, as fixes: the quick fixes that
    /// edit files of the input, preferred first.
    fn as_fixes(&self, d: &Diagnostic, actions: impl IntoIterator<Item = CodeAction>) -> Vec<Fix> {
        let rename = matches!(
            &d.code,
            Some(lsp_types::NumberOrString::String(c)) if RENAME_CODES.contains(&c.as_str())
        );
        let mut out: Vec<Fix> = Vec::new();
        for c in actions {
            let Some(changes) = c.edit.and_then(|e| e.changes) else {
                continue;
            };
            if c.kind != Some(CodeActionKind::QUICKFIX) {
                continue;
            }
            let mut edits: Vec<(usize, Vec<TextEdit>)> = changes
                .into_iter()
                .filter_map(|(u, list)| Some((self.index_of(u.as_str())?, list)))
                .collect();
            edits.sort_by_key(|(i, _)| *i);
            let preferred = c.is_preferred == Some(true);
            out.push(Fix {
                title: c.title,
                preferred,
                safe: preferred && !rename,
                edits: edits
                    .into_iter()
                    .map(|(i, list)| (self.names[i].0.clone(), list))
                    .collect(),
            });
        }
        out.sort_by_key(|f| !f.preferred);
        out
    }

    /// The formatter's verdict on `uri`, or None when it has no analysis.
    fn format(&mut self, uri: &str) -> Option<Formatted> {
        let opts = format::Options {
            line_width: self.settings.format_line_width as usize,
            ..format::Options::default()
        };
        if self.ws.is_sparql(uri) {
            let q = self.ws.sparql(uri)?;
            let text = q.text();
            let guarantee = "same SPARQL algebra";
            // The formatter refuses a query with errors; say how many.
            if q.doc.has_errors() {
                let errors = self
                    .diagnostics(uri, &[])
                    .iter()
                    .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
                    .count()
                    .max(1);
                let s = if errors == 1 { "" } else { "s" };
                let reason = format!("the query has {errors} error{s}; fix {} first", it(errors));
                return Some(Formatted {
                    guarantee,
                    outcome: FormatOutcome::Refused(reason),
                });
            }
            let outcome = match crate::sparql::format(text, &q.external, Some(q.base()), &opts) {
                Ok(t) if t == text => FormatOutcome::Unchanged,
                Ok(t) => FormatOutcome::Changed(t),
                Err(reason) => FormatOutcome::Refused(reason),
            };
            return Some(Formatted { guarantee, outcome });
        }
        let a = self.ws.analysis(uri)?;
        let jsonld = a.parsed.syntax == Syntax::JsonLd;
        let opts = if jsonld {
            // JSON convention; the editor passes its own indent.
            format::Options {
                tab_size: 2,
                ..opts
            }
        } else {
            opts
        };
        let guarantee = "same graph";
        let errors = a
            .parsed
            .errors
            .iter()
            .filter(|e| {
                if jsonld {
                    e.code == "json-syntax"
                } else {
                    e.severity == Severity::Error
                }
            })
            .count();
        let outcome = if errors > 0 {
            let s = if errors == 1 { "" } else { "s" };
            FormatOutcome::Refused(format!(
                "the file has {errors} syntax error{s}; fix {} first",
                it(errors)
            ))
        } else {
            match format::format_analysis(&a, &opts) {
                Some(t) if t == *a.text => FormatOutcome::Unchanged,
                Some(t) => FormatOutcome::Changed(t),
                None => FormatOutcome::Refused(
                    "the formatted text would not have the same graph, so it is not formatted"
                        .to_string(),
                ),
            }
        };
        Some(Formatted { guarantee, outcome })
    }

    fn kind(&self, uri: &str) -> &'static str {
        match self.ws.doc_kind(uri) {
            Some(DocKind::Sparql) => "sparql",
            Some(DocKind::Rdf(Syntax::TriG)) => "trig",
            Some(DocKind::Rdf(Syntax::NTriples)) => "ntriples",
            Some(DocKind::Rdf(Syntax::NQuads)) => "nquads",
            Some(DocKind::Rdf(Syntax::JsonLd)) => "jsonld",
            _ => "turtle",
        }
    }
}

fn it(n: usize) -> &'static str {
    if n == 1 { "it" } else { "them" }
}

/// The line index of each input file, made when first asked for: the texts
/// do not change while one is in use.
struct LineIndexes<'a> {
    files: &'a [BatchFile],
    lines: Vec<OnceCell<LineIndex>>,
}

impl<'a> LineIndexes<'a> {
    fn new(files: &'a [BatchFile]) -> LineIndexes<'a> {
        LineIndexes {
            files,
            lines: files.iter().map(|_| OnceCell::new()).collect(),
        }
    }

    fn of(&self, file: usize) -> &LineIndex {
        self.lines[file].get_or_init(|| LineIndex::new(&self.files[file].text))
    }

    /// The byte offset of `pos` in file `file`, as [`offset_in`] clamps it.
    fn offset(&self, file: usize, enc: Encoding, pos: Position) -> usize {
        let text = &self.files[file].text;
        offset_in(self.of(file), text, enc, pos).map_or(text.len(), |o| o as usize)
    }

    /// `pos`, whose column counts UTF-8 bytes, with the column in code points.
    fn code_point_position(&self, file: usize, pos: Position) -> Position {
        let text = &self.files[file].text;
        let off = self.offset(file, Encoding::Utf8, pos);
        let lc = self.of(file).line_col(TextSize::from(off as u32));
        let line_start = off - lc.col as usize;
        Position::new(lc.line, text[line_start..off].chars().count() as u32)
    }
}

/// A fix's edits as (file index, start byte, end byte, new text), with
/// positions in `enc`.
fn edits_of(
    fix: &Fix,
    files: &[BatchFile],
    lines: &LineIndexes,
    enc: Encoding,
) -> Option<Vec<(usize, usize, usize, String)>> {
    let mut out = Vec::new();
    for (name, list) in &fix.edits {
        let i = files.iter().position(|f| f.name == *name)?;
        for e in list {
            let s = lines.offset(i, enc, e.range.start);
            let end = lines.offset(i, enc, e.range.end);
            out.push((i, s, end.max(s), e.new_text.clone()));
        }
    }
    Some(out)
}

/// Convert a report's UTF-8 columns to code points. Edits convert against
/// the file they edit. Each file's line starts are found once: a large file
/// can have thousands of problems.
fn to_code_points(report: &mut FileReport, files: &[BatchFile], lines: &LineIndexes) {
    let index_of = |name: &str| files.iter().position(|f| f.name == name);
    let convert = |file: Option<usize>, r: Range| -> Option<Range> {
        let i = file?;
        Some(Range::new(
            lines.code_point_position(i, r.start),
            lines.code_point_position(i, r.end),
        ))
    };
    let own = index_of(&report.name);
    for p in &mut report.problems {
        let Some(r) = convert(own, p.diagnostic.range) else {
            return;
        };
        p.diagnostic.range = r;
        for fix in &mut p.fixes {
            for (name, list) in &mut fix.edits {
                let file = index_of(name);
                for e in list {
                    if let Some(r) = convert(file, e.range) {
                        e.range = r;
                    }
                }
            }
        }
    }
}

/// The URI a named file is opened at: its name as a path below
/// `file:///work/`, each '/'-separated segment escaped as JavaScript's
/// `encodeURIComponent` escapes it, so the browser editor (web/app/editor.js)
/// opens a file at the same URI.
pub fn work_uri(name: &str) -> String {
    let mut out = String::from("file:///work/");
    for (i, segment) in name.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        for b in segment.bytes() {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RDFS: &str = "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n";

    fn file(name: &str, text: &str) -> BatchFile {
        BatchFile::named(name, text.to_string(), false)
    }

    fn report(files: &[BatchFile], opts: &Options) -> Report {
        analyse(files, &Settings::default(), opts)
    }

    fn code(p: &Problem) -> Option<&str> {
        match &p.diagnostic.code {
            Some(lsp_types::NumberOrString::String(s)) => Some(s),
            _ => None,
        }
    }

    #[test]
    fn undeclared_prefix_has_a_safe_fix() {
        let files = [file("a.ttl", "<http://x/a> rdfs:label \"a\" .\n")];
        let r = report(&files, &Options::default());
        let f = &r.files[0];
        assert_eq!((f.name.as_str(), f.kind), ("a.ttl", "turtle"));
        let fix = f
            .problems
            .iter()
            .flat_map(|p| &p.fixes)
            .find(|x| x.preferred)
            .expect("a preferred fix");
        assert!(fix.safe, "{fix:?}");
        assert_eq!(fix.edits[0].0, "a.ttl");
        assert!(fix.edits[0].1[0].new_text.contains("rdfs:"));
    }

    #[test]
    fn a_term_rename_is_unsafe() {
        let text = format!("{RDFS}<http://x/a> rdfs:labl \"a\" .\n");
        let r = report(&[file("a.ttl", &text)], &Options::default());
        let p = r.files[0]
            .problems
            .iter()
            .find(|p| code(p) == Some("unknown-term"))
            .expect("unknown-term");
        let fix = &p.fixes[0];
        assert!(fix.preferred && !fix.safe, "{fix:?}");
        assert_eq!(fix.edits[0].1[0].new_text, "rdfs:label");
    }

    #[test]
    fn fix_all_applies_unsafe_fixes_only_when_asked() {
        let text = "<http://x/a> rdfs:labl \"a\" .\n";
        let files = [file("a.ttl", text)];
        let settings = Settings::default();
        let safe = fix_all(&files, &settings, false);
        assert!(safe.applied >= 1);
        assert!(safe.files[0].text.contains("rdfs:labl"));
        assert!(safe.files[0].text.starts_with("@prefix rdfs:"));
        let all = fix_all(&files, &settings, true);
        assert!(all.applied > safe.applied);
        assert!(all.files[0].text.contains("rdfs:label \"a\""));
    }

    #[test]
    fn context_files_are_neither_fixed_nor_returned() {
        let broken = "<http://x/a> rdfs:label \"a\" .\n";
        let files = [
            file("a.ttl", broken),
            BatchFile::named("ctx.ttl", broken.to_string(), true),
        ];
        let out = fix_all(&files, &Settings::default(), true);
        assert_eq!(out.files.len(), 1);
        assert_eq!(out.files[0].name, "a.ttl");
        assert!(out.files[0].text.starts_with("@prefix rdfs:"));
    }

    #[test]
    fn columns_count_as_asked() {
        // "é" is 2 UTF-8 bytes and 1 UTF-16 unit; "😀" 4 bytes, 2 units.
        let text = format!("{RDFS}<http://x/é😀> rdfs:labl \"a\" .\n");
        let col = |columns| {
            let r = report(
                &[file("a.ttl", &text)],
                &Options {
                    columns,
                    ..Options::default()
                },
            );
            let p = r.files[0]
                .problems
                .iter()
                .find(|p| code(p) == Some("unknown-term"))
                .expect("unknown-term")
                .clone();
            (
                p.diagnostic.range.start,
                p.fixes[0].edits[0].1[0].range.start,
            )
        };
        let (d8, e8) = col(Columns::Utf8);
        let (d16, e16) = col(Columns::Utf16);
        let (dcp, ecp) = col(Columns::CodePoints);
        assert_eq!((d8.line, d8.character), (1, 18));
        assert_eq!((d16.line, d16.character), (1, 15));
        assert_eq!((dcp.line, dcp.character), (1, 14));
        assert_eq!((e8, e16, ecp), (d8, d16, dcp));
    }

    #[test]
    fn shacl_runs_unless_turned_off() {
        let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
            @prefix ex: <http://example.org/> .\n\
            ex:S a sh:NodeShape ; sh:targetClass ex:C ;\n\
            \x20 sh:property [ sh:path ex:p ; sh:minCount 1 ] .\n\
            ex:n a ex:C .\n";
        let files = [file("s.ttl", text)];
        let shacl = |run| {
            let r = report(
                &files,
                &Options {
                    shacl: run,
                    ..Options::default()
                },
            );
            assert!(r.has_shapes);
            r.files[0]
                .problems
                .iter()
                .filter(|p| code(p).is_some_and(|c| c.starts_with("shacl-")))
                .count()
        };
        assert_eq!(shacl(false), 0);
        assert!(shacl(true) > 0);
    }

    #[test]
    fn batch_analyses_keep_no_tokens() {
        let text = format!("{RDFS}<http://x/a> rdfs:label \"a\" . # c\n");
        let files = [file("a.ttl", &text)];
        let settings = Settings::default();
        let mut input = Input::new(&files, &settings, Encoding::Utf16, None, None);
        let a = input.ws.analysis(&files[0].uri).unwrap();
        assert!(!a.has_tokens());
        // Made when a feature asks, the same as kept ones.
        let kept = Analysis::new(files[0].uri.clone(), &text, Default::default(), true);
        let key = |a: &Analysis| -> Vec<_> {
            a.tokens()
                .iter()
                .map(|t| (t.kind, t.start, t.end))
                .collect()
        };
        assert_eq!(key(&a), key(&kept));
        assert!(a.has_tokens());
    }

    #[test]
    fn the_formatter_gives_its_verdict() {
        let text = format!("{RDFS}<http://x/a>   rdfs:label \"a\" .\n");
        let r = report(&[file("a.ttl", &text)], &Options::default());
        let f = r.files[0].format.as_ref().unwrap();
        assert_eq!(f.guarantee, "same graph");
        assert!(matches!(f.outcome, FormatOutcome::Changed(_)));
    }

    #[test]
    fn a_query_runs_over_the_rdf_files_with_the_project_prefixes() {
        let settings = Settings::from_layers([(
            crate::settings::Layer::Client,
            &serde_json::json!({"prefixes": {"ex": "http://example.org/"}}),
        )]);
        // A headerless data file, a SPARQL file (not data) and the query.
        let files = [
            file("a.ttl", "ex:a ex:p 1 .\n"),
            file(
                "b.ttl",
                "@prefix ex: <http://example.org/> .\nex:b ex:p 2 .\n",
            ),
            file("other.rq", "SELECT * WHERE { ?s ?p ?o }\n"),
        ];
        let q = file(
            "q.rq",
            "PREFIX my: <http://my/>\nSELECT ?s WHERE { ?s ex:p ?o } ORDER BY ?s\n",
        );
        let out = query(&files, &q, &settings, &Limits::default());
        let Ok(Answer::Solutions(s)) = out.answer else {
            panic!("{:?}", out.answer)
        };
        let shown: Vec<String> = s
            .rows
            .iter()
            .map(|r| query::render::term(r[0].as_ref().unwrap(), &out.prefixes))
            .collect();
        assert_eq!(shown, ["ex:a", "ex:b"]);
        assert_eq!(
            out.prefixes.get("my").map(String::as_str),
            Some("http://my/")
        );
    }

    #[test]
    fn a_query_error_comes_back() {
        let q = file("q.rq", "SELECT * WHERE { ?s ?p }\n");
        let out = query(&[], &q, &Settings::default(), &Limits::default());
        assert!(matches!(
            out.answer,
            Err(QueryError::Syntax { line: 1, .. })
        ));
        let q = file("u.rq", "INSERT DATA { <a:a> <a:b> <a:c> }\n");
        let out = query(&[], &q, &Settings::default(), &Limits::default());
        assert!(matches!(out.answer, Err(QueryError::Unsupported(_))));
    }

    #[test]
    fn fixes_are_found_only_when_asked() {
        let files = [file("a.ttl", "<http://x/a> rdfs:labl \"a\" .\n")];
        let count = |fixes| {
            let opts = Options {
                fixes,
                ..Options::default()
            };
            let r = report(&files, &opts);
            assert!(!r.files[0].problems.is_empty());
            r.files[0]
                .problems
                .iter()
                .map(|p| p.fixes.len())
                .sum::<usize>()
        };
        assert_eq!(count(false), 0);
        assert!(count(true) > 0);
        // "Fix all" finds its own whatever the options.
        let out = fix_all(&files, &Settings::default(), true);
        assert!(out.applied > 0);
    }

    #[test]
    fn fix_all_on_crlf_text() {
        let text = "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\r\n\
            @prefix unused: <http://u/> .\r\n\
            <http://x/a> rdfs:labl \"a\" ;\r\n  foaf:name \"b\" .\r\n";
        let out = fix_all(&[file("a.ttl", text)], &Settings::default(), true);
        let fixed = &out.files[0].text;
        assert_eq!(
            fixed.matches('\n').count(),
            fixed.matches("\r\n").count(),
            "{fixed:?}"
        );
        assert!(fixed.contains("rdfs:label \"a\""), "{fixed:?}");
        assert!(fixed.contains("@prefix foaf: <http://xmlns.com/foaf/0.1/> .\r\n"));
        assert!(!fixed.contains("unused:"), "{fixed:?}");
    }

    #[test]
    fn an_edit_is_placed_by_the_documents_own_line_conversion() {
        let fix = |edits: Vec<TextEdit>| Fix {
            title: String::new(),
            preferred: true,
            safe: true,
            edits: vec![("a.ttl".to_string(), edits)],
        };
        let insert = |line, character, new_text: &str| TextEdit {
            range: Range::new(
                Position::new(line, character),
                Position::new(line, character),
            ),
            new_text: new_text.to_string(),
        };
        let files = [file("a.ttl", "ab\r\ncd\r\n")];
        let lines = LineIndexes::new(&files);
        let at = |edit| edits_of(&fix(vec![edit]), &files, &lines, Encoding::Utf16).unwrap()[0].1;
        // A column past the end of a CRLF line stops before the line break.
        assert_eq!(at(insert(0, 99, "x")), 2);
        assert_eq!(at(insert(1, 1, "x")), 5);
        // A line past the last one is the end of the text, not a dropped fix.
        assert_eq!(at(insert(7, 0, "x")), 8);
    }

    #[test]
    fn code_point_columns_follow_the_line_index() {
        // CRLF, a column past the end of its line, and a line past the end.
        let text = "é😀 rdfs:labl\r\nx\r\n";
        let files = [file("a.ttl", text)];
        let lines = LineIndexes::new(&files);
        let pos = |l, c| lines.code_point_position(0, Position::new(l, c));
        assert_eq!(pos(0, 0), Position::new(0, 0));
        // 2 + 4 UTF-8 bytes are 2 code points.
        assert_eq!(pos(0, 6), Position::new(0, 2));
        assert_eq!(pos(0, 99), Position::new(0, 2 + 1 + 9));
        assert_eq!(pos(1, 1), Position::new(1, 1));
        assert_eq!(pos(9, 0), Position::new(2, 0));
    }

    /// 20,000 undeclared uses of one prefix: with no fixes asked for, `check`
    /// reads them in a blink; with fixes the cost stays linear too.
    #[cfg(not(debug_assertions))]
    #[test]
    fn many_undeclared_uses_are_not_quadratic() {
        let text: String = (0..20_000)
            .map(|i| format!("<http://x/s{i}> foaf:name \"n{i}\" .\n"))
            .collect();
        let files = [file("a.ttl", &text)];
        for (fixes, limit) in [(false, 1.0), (true, 2.0)] {
            let opts = Options {
                fixes,
                columns: Columns::CodePoints,
                ..Options::default()
            };
            let start = std::time::Instant::now();
            let r = report(&files, &opts);
            let took = start.elapsed().as_secs_f64();
            assert_eq!(r.files[0].problems.len(), 20_000);
            assert_eq!(r.files[0].problems[0].fixes.is_empty(), !fixes);
            assert!(took < limit, "fixes={fixes}: {took:.2}s");
        }
    }

    #[cfg(unix)]
    #[test]
    fn at_path_opens_the_file_uri() {
        let f = BatchFile::at_path(Path::new("/tmp/a b.ttl"), "a b.ttl", String::new(), false);
        assert_eq!(f.unwrap().uri, "file:///tmp/a%20b.ttl");
        assert!(
            BatchFile::at_path(Path::new("rel.ttl"), "rel.ttl", String::new(), false).is_none()
        );
    }
}
