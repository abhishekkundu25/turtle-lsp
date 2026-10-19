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

mod actions;
mod completion;
pub mod diagnostics;
mod display;
pub mod edits;
mod navigation;
mod outline;
mod rename;

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
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    params: &CompletionParams,
) -> Option<CompletionResponse> {
    completion::completion(q, k, enc, params)
}

/// `completionItem/resolve` for items with `data.sparql = true`.
pub fn resolve_completion(item: CompletionItem, k: &Knowledge) -> CompletionItem {
    completion::resolve_completion(item, k)
}

/// `textDocument/hover`.
pub fn hover(q: &SparqlAnalysis, k: &Knowledge, enc: Encoding, pos: Position) -> Option<Hover> {
    display::hover(q, k, enc, pos)
}

/// `textDocument/definition`. `prefix_file` is the `prefixes.ttl` supplying
/// prefixes to the document's folder, if any, for a prefix that comes from it.
pub fn definition(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    prefix_file: Option<Arc<Analysis>>,
) -> Vec<Location> {
    navigation::definition(q, k, enc, pos, prefix_file)
}

/// `textDocument/references`. `others` is empty unless the cursor is on a
/// term (see the module documentation).
pub fn references(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    include_declaration: bool,
    others: &[Arc<SparqlAnalysis>],
) -> Vec<Location> {
    navigation::references(q, k, enc, pos, include_declaration, others)
}

/// `textDocument/documentHighlight`.
pub fn highlights(q: &SparqlAnalysis, enc: Encoding, pos: Position) -> Vec<DocumentHighlight> {
    navigation::highlights(q, enc, pos)
}

/// `textDocument/prepareRename`.
pub fn prepare_rename(
    q: &SparqlAnalysis,
    enc: Encoding,
    pos: Position,
) -> Option<PrepareRenameResponse> {
    rename::prepare_rename(q, enc, pos)
}

/// `textDocument/rename`. An `Err` is refused with its message. `prefix_file`
/// as for [`definition`]; `others` is empty unless the cursor is on a term.
/// The RDF documents to rename a term in come from `k` (as for the Turtle
/// rename in `features::navigation`).
pub fn rename(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    new_name: &str,
    prefix_file: Option<Arc<Analysis>>,
    others: &[Arc<SparqlAnalysis>],
) -> Result<WorkspaceEdit, String> {
    rename::rename(q, k, enc, pos, new_name, prefix_file, others)
}

/// `textDocument/documentSymbol`.
pub fn document_symbols(q: &SparqlAnalysis, enc: Encoding) -> Vec<DocumentSymbol> {
    outline::document_symbols(q, enc)
}

/// `textDocument/foldingRange` (lines only: no encoding needed).
pub fn folding_ranges(q: &SparqlAnalysis) -> Vec<FoldingRange> {
    outline::folding_ranges(q)
}

/// `textDocument/semanticTokens/full`, with the server's legend
/// (`features::semantic::legend`: `variable` and `function` follow the RDF
/// token types).
pub fn semantic_tokens_full(q: &SparqlAnalysis, k: &Knowledge, enc: Encoding) -> SemanticTokens {
    display::semantic_tokens_full(q, k, enc)
}

/// `textDocument/semanticTokens/range`: the tokens overlapping `range`.
pub fn semantic_tokens_range(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    range: Range,
) -> SemanticTokens {
    display::semantic_tokens_range(q, k, enc, range)
}

/// `textDocument/inlayHint` in `range`; `mode` is `turtle.inlayHints.labels`.
pub fn inlay_hints(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    range: Range,
    mode: InlayLabels,
) -> Vec<InlayHint> {
    display::inlay_hints(q, k, enc, range, mode)
}

/// `textDocument/codeAction`. The diagnostics in `params.context` are the
/// ones [`diagnostics::diagnostics`] made (source `turtle`; an
/// `undeclared-prefix` carries `data.prefix`, `data.namespace` and
/// `data.candidates`, an `unknown-term` / `undescribed-term` fix
/// `data.replace`). `resolve_edits`: the client resolves edits lazily
/// (`codeAction/resolve`); a lazy action's `data.uri` must name the document.
pub fn code_actions(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    params: &CodeActionParams,
    resolve_edits: bool,
) -> Vec<CodeActionOrCommand> {
    actions::code_actions(q, k, enc, params, resolve_edits)
}

/// `codeAction/resolve` for an action whose `data.uri` is this document.
pub fn resolve_code_action(q: &SparqlAnalysis, enc: Encoding, action: CodeAction) -> CodeAction {
    actions::resolve_code_action(q, enc, action)
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
    iri: &str,
    include_declaration: bool,
    docs: &[Arc<SparqlAnalysis>],
    enc: Encoding,
) -> Vec<Location> {
    navigation::term_references(iri, include_declaration, docs, enc)
}

/// For an RDF document's `textDocument/rename` of term `old` (in `a`, to
/// `new_name` as the user typed it): adds the edits for the workspace's
/// SPARQL documents to `edit`, which already holds the RDF edits. An `Err`
/// refuses the whole rename.
pub fn rdf_term_rename(
    a: &Analysis,
    old: &str,
    new_name: &str,
    docs: &[Arc<SparqlAnalysis>],
    enc: Encoding,
    edit: &mut WorkspaceEdit,
) -> Result<(), String> {
    rename::rdf_term_rename(a, old, new_name, docs, enc, edit)
}
