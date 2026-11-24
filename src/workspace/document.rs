//! An open document and how it is analyzed: its text, kind and analysis,
//! the analysis of an edit through the statement splice, and the stand-in
//! for an analysis that panicked.

use super::DocKind;
use crate::analysis::{Analysis, LineIndex, LineStarts, ParsePath, SparqlAnalysis};
use crate::features::kept::Kept;
use crate::jsonld::ContextLoader;
use crate::jsonld::loader::Bundled;
use crate::knowledge::DocFacts;
use crate::syntax::splice::{Edit, Window};
use crate::syntax::{PrefixMap, Severity, Span, Syntax, SyntaxError};
use crate::worker::catch;
use std::sync::Arc;

pub(crate) struct Doc {
    pub(crate) text: String,
    pub(crate) version: i32,
    pub(crate) kind: DocKind,
    /// Where `kind` comes from.
    pub(super) from: KindFrom,
    /// For an RDF document: its analysis, or what the next one builds on.
    pub(super) analyzed: Analyzed,
    /// For a SPARQL document: its parse, until the next edit.
    pub(super) query: Option<Arc<SparqlAnalysis>>,
    /// A cached copy of an import: the import already contributes its facts.
    pub(super) cached_import: bool,
    /// For an RDF document: the findings of the checks it keeps between
    /// edits ([`Workspace::diagnostics`](super::Workspace::diagnostics)).
    pub(super) kept: Kept,
}

/// Where an open document's kind comes from.
pub(super) enum KindFrom {
    /// The caller named it ([`Workspace::open_doc_as`](super::Workspace::open_doc_as)): it stays.
    Given,
    /// [`DocKind::of`] with this language id, the URI and the text: decided
    /// again whenever the text changes, so an `.owl` whose text turns from
    /// RDF/XML to Turtle is parsed as Turtle, as a fresh open would.
    Detected(Option<Box<str>>),
}

/// An open RDF document's analysis against its text: current, or what the
/// next analysis can build on.
pub(super) enum Analyzed {
    /// Nothing to build on: not analyzed yet, or the whole text was replaced
    /// since (the next analysis starts from the text alone).
    No,
    /// Of the current text.
    Current(Arc<Analysis>),
    /// Of the current text, but its analysis panicked: the stand-in
    /// ([`failed_analysis`]) is not of the text, so it is nothing to build on.
    Failed(Arc<Analysis>),
    /// Of the current text, with prefixes or contexts that changed since.
    Outdated(Arc<Analysis>),
    /// Of the text before `edit` (the client's changes since, collapsed);
    /// `starts` are the line starts of the text now.
    Edited {
        last: Arc<Analysis>,
        edit: Edit,
        starts: LineStarts,
    },
}

impl Analyzed {
    /// The analysis of the current text, if it is up to date.
    pub(super) fn current(&self) -> Option<&Arc<Analysis>> {
        match self {
            Analyzed::Current(a) | Analyzed::Failed(a) => Some(a),
            Analyzed::No | Analyzed::Outdated(_) | Analyzed::Edited { .. } => None,
        }
    }

    /// The prefixes or contexts the analysis used changed: it is to be made
    /// again, from the same text.
    pub(super) fn outdate(&mut self) {
        *self = match std::mem::replace(self, Analyzed::No) {
            Analyzed::Current(a) => Analyzed::Outdated(a),
            Analyzed::Failed(_) | Analyzed::No => Analyzed::No,
            other @ (Analyzed::Outdated(_) | Analyzed::Edited { .. }) => other,
        };
    }

    /// The analysis before the client's edits since, and those edits.
    pub(super) fn edited(&self) -> Option<(&Analysis, Edit)> {
        match self {
            Analyzed::Edited { last, edit, .. } => Some((last, *edit)),
            _ => None,
        }
    }

    /// The line starts of the document's text, when this keeps them.
    pub(super) fn starts(&self) -> Option<&LineStarts> {
        match self {
            Analyzed::Current(a) | Analyzed::Outdated(a) => Some(a.lines.starts()),
            Analyzed::Edited { starts, .. } => Some(starts),
            Analyzed::No | Analyzed::Failed(_) => None,
        }
    }

    /// The bytes `range` of the document's text were replaced by `new`,
    /// leaving `text`. `scratch` holds the line starts when this keeps none
    /// ([`Analyzed::starts`]) and is moved instead.
    pub(super) fn replaced(
        &mut self,
        text: &str,
        range: std::ops::Range<usize>,
        new: &str,
        scratch: &mut Option<LineStarts>,
    ) {
        *self = match std::mem::replace(self, Analyzed::No) {
            Analyzed::Current(last) | Analyzed::Outdated(last) => {
                let mut starts = last.lines.starts().clone();
                starts.replace(text, range.clone(), new);
                let edit = Edit::new(range, new.len());
                Analyzed::Edited { last, edit, starts }
            }
            Analyzed::Edited {
                last,
                edit,
                mut starts,
            } => {
                starts.replace(text, range.clone(), new);
                let edit = edit.then(range, new.len());
                Analyzed::Edited { last, edit, starts }
            }
            Analyzed::No | Analyzed::Failed(_) => {
                if let Some(starts) = scratch {
                    starts.replace(text, range, new);
                }
                Analyzed::No
            }
        };
    }

    /// The line index of `text`, the document's text: from the last
    /// analysis's where there is one.
    fn lines(&self, text: &str) -> LineIndex {
        match self {
            Analyzed::Current(a) | Analyzed::Outdated(a) => a.lines.clone(),
            Analyzed::Edited { last, edit, starts } => last.lines.edited(text, edit, starts),
            Analyzed::No | Analyzed::Failed(_) => LineIndex::new(text),
        }
    }
}

/// Text of a document that makes analysis panic, in tests (see
/// [`analyze`]).
#[cfg(test)]
pub(crate) const PANIC_IN_ANALYSIS: &str = "turtle-lsp-test: panic in analysis";
/// Text of a document whose facts cannot be computed, in tests (see
/// [`prepared`]).
#[cfg(test)]
pub(crate) const PANIC_IN_FACTS: &str = "turtle-lsp-test: panic in facts";

/// [`Analysis::with_loader`], but a panic in it is the analysis of a document
/// that failed ([`failed_analysis`]) rather than the end of the server: one
/// document's bug must not take every other document down.
pub(super) fn analyze(
    uri: String,
    text: &str,
    external: Arc<PrefixMap>,
    keep_tokens: bool,
    syntax: Syntax,
    loader: &dyn ContextLoader,
) -> Analysis {
    let attempt = caught(text, || {
        let (uri, external) = (uri.clone(), external.clone());
        Analysis::with_loader(uri, text, external, keep_tokens, syntax, loader)
    });
    attempt.unwrap_or_else(|msg| failed_analysis(uri, syntax, external, &msg))
}

/// What `work` returns from analyzing `text`, or the message of its panic.
fn caught<T>(text: &str, work: impl FnOnce() -> T) -> Result<T, String> {
    catch("analysis", || {
        #[cfg(test)]
        if text.contains(PANIC_IN_ANALYSIS) {
            panic!("injected analysis panic");
        }
        #[cfg(not(test))]
        let _ = text;
        work()
    })
}

/// How the workspace parses an open document after an edit.
#[derive(Clone, Copy, Default)]
pub(super) struct Reparse {
    /// In full every time (the setting `parse.incremental` off).
    pub(super) off: bool,
    /// Check every splice against a full parse, keep the full one when they
    /// differ, and record each analysis's path
    /// ([`Workspace::set_check_incremental`](super::Workspace::set_check_incremental)).
    pub(super) check: bool,
}

/// How one analysis of an open document was parsed, recorded when the
/// workspace checks the splice ([`Workspace::set_check_incremental`](super::Workspace::set_check_incremental)).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParseRecord {
    pub(crate) uri: String,
    pub(crate) path: ParsePath,
    /// The first field in which the spliced analysis differed from the
    /// full one, which the document got instead.
    pub(crate) difference: Option<String>,
}

/// An analysis of an open document: what the document keeps, its facts
/// unless it is a cached import, and how it was parsed.
pub(super) struct OpenAnalysis {
    pub(super) analyzed: Analyzed,
    pub(super) facts: Option<DocFacts>,
    pub(super) path: ParsePath,
    pub(super) difference: Option<String>,
    /// What the splice parsed again, when the analysis is the splice's.
    pub(super) window: Option<Window>,
}

/// [`analyze`] for the open document `doc`, building on its last analysis
/// where it can (its line index, and its parse through the statement
/// splice), with its facts unless it is a cached import. A panic in either
/// gives a [`failed_analysis`].
pub(super) fn analyze_open(
    uri: &str,
    doc: &Doc,
    external: Arc<PrefixMap>,
    keep_tokens: bool,
    syntax: Syntax,
    loader: &dyn ContextLoader,
    reparse: Reparse,
) -> OpenAnalysis {
    let mut path = ParsePath::Whole;
    let mut window = None;
    let attempt = caught(&doc.text, || {
        let text = &doc.text;
        let lines = doc.analyzed.lines(text);
        let parsed = match doc.analyzed.edited() {
            None => None,
            Some(_) if reparse.off => {
                path = ParsePath::Off;
                None
            }
            Some((last, edit)) => match last.splice(uri, text, edit, &external) {
                Ok((p, w)) => {
                    path = ParsePath::Spliced;
                    window = Some(w);
                    Some(p)
                }
                Err(f) => {
                    path = ParsePath::Fallback(f);
                    None
                }
            },
        };
        let (u, ext) = (uri.to_string(), external.clone());
        let a = match parsed {
            Some(p) => Analysis::with_parse(u, text, ext, keep_tokens, p, lines),
            None => Analysis::with_lines(u, text, ext, keep_tokens, syntax, loader, lines),
        };
        if !(reparse.check && path == ParsePath::Spliced) {
            return (a, None);
        }
        // From the text alone, the line index too.
        let (u, ext) = (uri.to_string(), external.clone());
        let full = Analysis::with_loader(u, text, ext, keep_tokens, syntax, loader);
        match crate::analysis::same_analysis(&a, &full) {
            Ok(()) => (a, None),
            Err(diff) => {
                window = None;
                (full, Some(diff))
            }
        }
    })
    .and_then(|(a, difference)| match doc.cached_import {
        true => Ok((a, None, difference)),
        false => facts(&a).map(|f| (a, Some(f), difference)),
    });
    match attempt {
        Ok((a, facts, difference)) => OpenAnalysis {
            analyzed: Analyzed::Current(Arc::new(a)),
            facts,
            path,
            difference,
            window,
        },
        Err(msg) => {
            let failed = failed_analysis(uri.to_string(), syntax, external, &msg);
            let facts = (!doc.cached_import).then(|| DocFacts::new(&failed));
            OpenAnalysis {
                analyzed: Analyzed::Failed(Arc::new(failed)),
                facts,
                path,
                difference: None,
                window: None,
            }
        }
    }
}

/// What a document whose analysis panicked is analyzed as: empty, with the
/// failure as its one error, so it shows as a diagnostic.
fn failed_analysis(uri: String, syntax: Syntax, external: Arc<PrefixMap>, msg: &str) -> Analysis {
    let mut a = Analysis::with_loader(uri, "", external, true, syntax, &Bundled);
    a.parsed.errors.push(SyntaxError::new(
        Span::default(),
        format!(
            "{msg}. The rest of the server keeps working; please report this file at https://github.com/abhishekkundu25/turtle-lsp/issues"
        ),
        Severity::Error,
        "internal-error",
    ));
    a
}

/// The facts of `a` for the knowledge index, or the message of the panic
/// that computing them ended in.
fn facts(a: &Analysis) -> Result<DocFacts, String> {
    catch("indexing", || {
        #[cfg(test)]
        if a.text.contains(PANIC_IN_FACTS) {
            panic!("injected facts panic");
        }
        DocFacts::new(a)
    })
}

/// `a` with its facts for the knowledge index. When the facts cannot be
/// computed, a [`failed_analysis`] stands in for it.
pub(super) fn prepared(a: Analysis) -> (Analysis, DocFacts) {
    match facts(&a) {
        Ok(facts) => (a, facts),
        Err(msg) => {
            let failed = failed_analysis(a.uri.clone(), a.parsed.syntax, a.external.clone(), &msg);
            let facts = DocFacts::new(&failed);
            (failed, facts)
        }
    }
}
