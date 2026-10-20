//! Batch analysis: a set of files analysed as one editor workspace, without
//! the server's event loop. The browser toolbox (web/engine) and the
//! command-line check both use it.
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

use crate::analysis::Encoding;
use crate::features::lint;
use crate::features::shacl_validate;
use crate::features::{diagnostics, format};
use crate::imports;
use crate::knowledge::Source;
use crate::settings::Settings;
use crate::sparql::features as sparql_features;
use crate::syntax::{Severity, Syntax};
use crate::workspace::{DocKind, Workspace};
use lsp_types::{
    CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams, Diagnostic,
    DiagnosticSeverity, Position, Range, TextDocumentIdentifier, TextEdit, Uri,
};
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
}

impl Default for Options {
    fn default() -> Options {
        Options {
            shacl: true,
            columns: Columns::Utf16,
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
    /// Preferred fixes first.
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
        let mut diags = input.diagnostics(uri);
        diags.extend(shacl.remove(uri).unwrap_or_default());
        let problems = diags
            .into_iter()
            .map(|d| {
                let fixes = input.fixes(uri, &d);
                Problem {
                    diagnostic: d,
                    fixes,
                }
            })
            .collect();
        out.push(FileReport {
            name: f.name.clone(),
            kind: input.kind(uri),
            problems,
            format: input.format(uri),
        });
    }
    if opts.columns == Columns::CodePoints {
        for report in &mut out {
            to_code_points(report, files);
        }
    }
    Report {
        files: out,
        has_shapes,
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
        for f in files.iter().filter(|f| !f.context) {
            for d in input.diagnostics(&f.uri) {
                let Some(fix) = input
                    .fixes(&f.uri, &d)
                    .into_iter()
                    .find(|x| x.preferred && (x.safe || include_unsafe))
                else {
                    continue;
                };
                let Some(edits) = edits_of(&fix, &files, enc) else {
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

    /// Diagnostics of `uri` as the server computes them once indexed,
    /// without SHACL (see [`Input::shacl`]).
    fn diagnostics(&mut self, uri: &str) -> Vec<Diagnostic> {
        let enc = self.enc;
        let opts = diagnostics::Options::from_settings(self.settings);
        if self.ws.is_sparql(uri) {
            let Some(q) = self.ws.sparql(uri) else {
                return Vec::new();
            };
            return sparql_features::diagnostics::diagnostics(&q, self.ws.knowledge(), enc, &opts);
        }
        let Some(a) = self.ws.analysis(uri) else {
            return Vec::new();
        };
        let mut d = diagnostics::diagnostics_with(&a, self.ws.knowledge(), enc, &opts);
        let unresolved = &self.unresolved().status;
        d.extend(imports::diagnostics_with(&a, enc, |iri| {
            unresolved.get(iri)
        }));
        d
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
        };
        (
            true,
            shacl_validate::run(&job, &|| false).unwrap_or_default(),
        )
    }

    /// The quick fixes for `d` in `uri`, preferred first.
    fn fixes(&mut self, uri: &str, d: &Diagnostic) -> Vec<Fix> {
        let enc = self.enc;
        let Ok(doc_uri) = uri.parse::<Uri>() else {
            return Vec::new();
        };
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            range: d.range,
            context: CodeActionContext {
                diagnostics: vec![d.clone()],
                only: Some(vec![CodeActionKind::QUICKFIX]),
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let actions = if self.ws.is_sparql(uri) {
            let Some(q) = self.ws.sparql(uri) else {
                return Vec::new();
            };
            sparql_features::code_actions(&q, self.ws.knowledge(), enc, &params, false)
        } else {
            let Some(a) = self.ws.analysis(uri) else {
                return Vec::new();
            };
            diagnostics::code_actions(&a, self.ws.knowledge(), &a.text, enc, &params)
        };
        let rename = matches!(
            &d.code,
            Some(lsp_types::NumberOrString::String(c)) if RENAME_CODES.contains(&c.as_str())
        );
        let mut out: Vec<Fix> = Vec::new();
        for action in actions {
            let CodeActionOrCommand::CodeAction(c) = action else {
                continue;
            };
            // A fix for this diagnostic, not "add all missing prefixes".
            let mine = c
                .diagnostics
                .as_ref()
                .is_some_and(|ds| ds.len() == 1 && ds[0] == *d);
            let Some(changes) = c.edit.and_then(|e| e.changes) else {
                continue;
            };
            if !mine || c.kind != Some(CodeActionKind::QUICKFIX) {
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

    /// The formatter's verdict on `uri`, or None when it has no analysis or
    /// is JSON-LD (not formatted yet).
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
                    .diagnostics(uri)
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
        if a.parsed.syntax == Syntax::JsonLd {
            return None;
        }
        let guarantee = "same graph";
        let errors = a
            .parsed
            .errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .count();
        let outcome = if errors > 0 {
            let s = if errors == 1 { "" } else { "s" };
            FormatOutcome::Refused(format!(
                "the file has {errors} syntax error{s}; fix {} first",
                it(errors)
            ))
        } else {
            match format::format(&a.text, &a.parsed, &a.external, &opts) {
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
        match self.ws.open.get(uri).map(|d| d.kind) {
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

/// A fix's edits as (file index, start byte, end byte, new text), with
/// positions in `enc`.
fn edits_of(
    fix: &Fix,
    files: &[BatchFile],
    enc: Encoding,
) -> Option<Vec<(usize, usize, usize, String)>> {
    let mut out = Vec::new();
    for (name, list) in &fix.edits {
        let i = files.iter().position(|f| f.name == *name)?;
        for e in list {
            let s = byte_offset(&files[i].text, e.range.start, enc)?;
            let end = byte_offset(&files[i].text, e.range.end, enc)?;
            out.push((i, s, end.max(s), e.new_text.clone()));
        }
    }
    Some(out)
}

/// The byte offset of a position in `text`, clamped to the end of its line.
fn byte_offset(text: &str, pos: Position, enc: Encoding) -> Option<usize> {
    let mut start = 0;
    for _ in 0..pos.line {
        start += text[start..].find('\n')? + 1;
    }
    let line = &text[start..];
    let line = &line[..line.find('\n').unwrap_or(line.len())];
    let mut units = 0;
    for (i, c) in line.char_indices() {
        if units >= pos.character as usize {
            return Some(start + i);
        }
        units += match enc {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
        };
    }
    Some(start + line.len())
}

/// Convert a report's UTF-8 columns to code points. Edits convert against
/// the file they edit. Each file's line starts are found once: a large file
/// can have thousands of problems.
fn to_code_points(report: &mut FileReport, files: &[BatchFile]) {
    let mut lines: HashMap<&str, Lines> = HashMap::new();
    let mut convert = |name: &str, r: Range| -> Option<Range> {
        if !lines.contains_key(name) {
            let f = files.iter().find(|f| f.name == name)?;
            lines.insert(&f.name, Lines::new(&f.text));
        }
        Some(lines[name].range(r))
    };
    let own = report.name.clone();
    for p in &mut report.problems {
        let Some(r) = convert(&own, p.diagnostic.range) else {
            return;
        };
        p.diagnostic.range = r;
        for fix in &mut p.fixes {
            for (name, list) in &mut fix.edits {
                for e in list {
                    if let Some(r) = convert(name, e.range) {
                        e.range = r;
                    }
                }
            }
        }
    }
}

/// A text and where each of its lines starts.
struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Lines<'a> {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Lines { text, starts }
    }

    fn range(&self, r: Range) -> Range {
        Range::new(self.position(r.start), self.position(r.end))
    }

    /// A position whose column counts UTF-8 bytes, with the column in code points.
    fn position(&self, pos: Position) -> Position {
        let line = match self.starts.get(pos.line as usize) {
            Some(&start) => {
                let end = self
                    .starts
                    .get(pos.line as usize + 1)
                    .map_or(self.text.len(), |e| e - 1);
                &self.text[start..end]
            }
            None => "",
        };
        let mut col = (pos.character as usize).min(line.len());
        while !line.is_char_boundary(col) {
            col -= 1;
        }
        Position::new(pos.line, line[..col].chars().count() as u32)
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
                    shacl: true,
                    columns,
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
                    columns: Columns::Utf16,
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
    fn the_formatter_gives_its_verdict() {
        let text = format!("{RDFS}<http://x/a>   rdfs:label \"a\" .\n");
        let r = report(&[file("a.ttl", &text)], &Options::default());
        let f = r.files[0].format.as_ref().unwrap();
        assert_eq!(f.guarantee, "same graph");
        assert!(matches!(f.outcome, FormatOutcome::Changed(_)));
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
