//! The language features of SPARQL documents: one function per request the
//! server routes for a SPARQL document. The server never runs the RDF
//! features on a SPARQL document, nor these on an RDF one.
//!
//! Each function gets the document as a [`SparqlAnalysis`]: the parse
//! (`q.doc`: tokens, structure, diagnostics), its URI, the external prefixes
//! it was parsed with, and the position helpers `q.offset(enc, position)`,
//! `q.position(enc, offset)` and `q.range(enc, span)` (UTF-8 or UTF-16, as
//! the client negotiated; the same mapping RDF analyses use). The workspace
//! knowledge comes as `&Knowledge` (open RDF documents and indexed files;
//! never SPARQL documents).
//!
//! Workspace queries: [`references`] and [`rename`] get `others`, every
//! SPARQL document of the workspace (open ones from memory, the rest read
//! from disk; the requesting document included), but only when the cursor
//! is on an IRI, prefixed name or `a` (a term): the server reads them only
//! then. For RDF documents the server calls [`term_references`] and
//! [`rdf_term_rename`] with the same list when the cursor is on a term, and
//! merges what they return into the RDF answer.
//!
//! Requests with no SPARQL meaning (type hierarchy, type definition,
//! implementation) answer empty for SPARQL documents in the server, and
//! SPARQL documents add nothing to workspace symbols.

pub mod diagnostics;
pub mod edits;

use crate::analysis::{Analysis, Encoding, SparqlAnalysis};
use crate::features::format::Options;
use crate::knowledge::Knowledge;
use crate::settings::InlayLabels;
use lsp_types::*;
use std::sync::Arc;

/// `textDocument/completion`. `params.context` tells a `?` / `$` trigger.
/// Items whose resolve needs SPARQL-specific work set `data.sparql = true`
/// (they then go to [`resolve_completion`]); other items carrying
/// `data.iri` are resolved by the Turtle resolver (term documentation).
pub fn completion(
    _q: &SparqlAnalysis,
    _k: &Knowledge,
    _enc: Encoding,
    _params: &CompletionParams,
) -> Option<CompletionResponse> {
    None
}

/// `completionItem/resolve` for items with `data.sparql = true`.
pub fn resolve_completion(item: CompletionItem, _k: &Knowledge) -> CompletionItem {
    item
}

/// `textDocument/hover`.
pub fn hover(_q: &SparqlAnalysis, _k: &Knowledge, _enc: Encoding, _pos: Position) -> Option<Hover> {
    None
}

/// `textDocument/definition`. `prefix_file` is the `prefixes.ttl` supplying
/// prefixes to the document's folder, if any, for a prefix that comes from it.
pub fn definition(
    _q: &SparqlAnalysis,
    _k: &Knowledge,
    _enc: Encoding,
    _pos: Position,
    _prefix_file: Option<Arc<Analysis>>,
) -> Vec<Location> {
    Vec::new()
}

/// `textDocument/references`. `others` is empty unless the cursor is on a
/// term (see the module documentation).
pub fn references(
    _q: &SparqlAnalysis,
    _k: &Knowledge,
    _enc: Encoding,
    _pos: Position,
    _include_declaration: bool,
    _others: &[Arc<SparqlAnalysis>],
) -> Vec<Location> {
    Vec::new()
}

/// `textDocument/documentHighlight`.
pub fn highlights(_q: &SparqlAnalysis, _enc: Encoding, _pos: Position) -> Vec<DocumentHighlight> {
    Vec::new()
}

/// `textDocument/prepareRename`.
pub fn prepare_rename(
    _q: &SparqlAnalysis,
    _enc: Encoding,
    _pos: Position,
) -> Option<PrepareRenameResponse> {
    None
}

/// `textDocument/rename`. An `Err` is refused with its message. `prefix_file`
/// as for [`definition`]; `others` is empty unless the cursor is on a term.
/// The RDF documents to rename a term in come from `k` (as for the Turtle
/// rename in `features::navigation`).
pub fn rename(
    _q: &SparqlAnalysis,
    _k: &Knowledge,
    _enc: Encoding,
    _pos: Position,
    _new_name: &str,
    _prefix_file: Option<Arc<Analysis>>,
    _others: &[Arc<SparqlAnalysis>],
) -> Result<WorkspaceEdit, String> {
    Err("nothing to rename here".into())
}

/// `textDocument/documentSymbol`.
pub fn document_symbols(_q: &SparqlAnalysis, _enc: Encoding) -> Vec<DocumentSymbol> {
    Vec::new()
}

/// `textDocument/foldingRange` (lines only: no encoding needed).
pub fn folding_ranges(_q: &SparqlAnalysis) -> Vec<FoldingRange> {
    Vec::new()
}

/// `textDocument/semanticTokens/full`, with the server's legend
/// (`features::semantic::legend`: `variable` and `function` follow the RDF
/// token types).
pub fn semantic_tokens_full(_q: &SparqlAnalysis, _k: &Knowledge, _enc: Encoding) -> SemanticTokens {
    SemanticTokens::default()
}

/// `textDocument/semanticTokens/range`: the tokens overlapping `range`.
pub fn semantic_tokens_range(
    _q: &SparqlAnalysis,
    _k: &Knowledge,
    _enc: Encoding,
    _range: Range,
) -> SemanticTokens {
    SemanticTokens::default()
}

/// `textDocument/inlayHint` in `range`; `mode` is `turtle.inlayHints.labels`.
pub fn inlay_hints(
    _q: &SparqlAnalysis,
    _k: &Knowledge,
    _enc: Encoding,
    _range: Range,
    _mode: InlayLabels,
) -> Vec<InlayHint> {
    Vec::new()
}

/// `textDocument/codeAction`. The diagnostics in `params.context` are the
/// ones [`diagnostics::diagnostics`] made (source `turtle`; an
/// `undeclared-prefix` carries `data.prefix`, `data.namespace` and
/// `data.candidates`, an `unknown-term` / `undescribed-term` fix
/// `data.replace`). `resolve_edits`: the client resolves edits lazily
/// (`codeAction/resolve`); a lazy action's `data.uri` must name the document.
pub fn code_actions(
    _q: &SparqlAnalysis,
    _k: &Knowledge,
    _enc: Encoding,
    _params: &CodeActionParams,
    _resolve_edits: bool,
) -> Vec<CodeActionOrCommand> {
    Vec::new()
}

/// `codeAction/resolve` for an action whose `data.uri` is this document.
pub fn resolve_code_action(_q: &SparqlAnalysis, _enc: Encoding, action: CodeAction) -> CodeAction {
    action
}

/// `textDocument/formatting`: one edit replacing the whole text with
/// [`crate::sparql::format`]'s output, or none when it refuses or nothing
/// changes.
pub fn formatting(q: &SparqlAnalysis, enc: Encoding, opts: &Options) -> Vec<TextEdit> {
    let text = q.text();
    match crate::sparql::format(text, &q.external, Some(q.base()), opts) {
        Ok(new_text) if new_text != text => vec![TextEdit {
            range: Range::new(Position::new(0, 0), q.position(enc, text.len() as u32)),
            new_text,
        }],
        _ => Vec::new(),
    }
}

/// For an RDF document's `textDocument/references` on term `iri`: its uses
/// in the workspace's SPARQL documents (`docs`, see
/// `Workspace::sparql_documents`). The server appends them to the RDF
/// references.
pub fn term_references(
    _iri: &str,
    _include_declaration: bool,
    _docs: &[Arc<SparqlAnalysis>],
    _enc: Encoding,
) -> Vec<Location> {
    Vec::new()
}

/// For an RDF document's `textDocument/rename` of term `old` (in `a`, to
/// `new_name` as the user typed it): adds the edits for the workspace's
/// SPARQL documents to `edit`, which already holds the RDF edits. An `Err`
/// refuses the whole rename.
pub fn rdf_term_rename(
    _a: &Analysis,
    _old: &str,
    _new_name: &str,
    _docs: &[Arc<SparqlAnalysis>],
    _enc: Encoding,
    _edit: &mut WorkspaceEdit,
) -> Result<(), String> {
    Ok(())
}
