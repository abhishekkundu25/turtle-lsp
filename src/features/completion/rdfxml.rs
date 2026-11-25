//! Completion in RDF/XML: none yet (ticket 05 fills it).

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use lsp_types::CompletionResponse;

pub(super) fn complete(
    _a: &Analysis,
    _k: &Knowledge,
    _enc: Encoding,
    _offset: usize,
    _snippets: bool,
) -> CompletionResponse {
    CompletionResponse::Array(vec![])
}
