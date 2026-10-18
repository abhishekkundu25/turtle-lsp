//! SPARQL documents (queries and updates, SPARQL 1.1 and 1.2): tokens with
//! spans, the document's structure, and the checks and formatting that need
//! no knowledge of the workspace. The language features built on it live in
//! `features`.
//!
//! The API below is the seam the W3C SPARQL harness and the corpus use; the
//! bodies are filled in by the SPARQL slices of `docs/specs/nquads-sparql.md`.

use crate::features::format::Options;
use crate::syntax::{PrefixMap, Span};

/// How serious a diagnostic is. The codes and levels match the Turtle ones
/// where the meaning is the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub level: Level,
    /// `syntax`, `undeclared-prefix`, `unused-prefix`, `duplicate-prefix`,
    /// `redefined-prefix` or `unbound-projection`.
    pub code: &'static str,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Iri,
    PName,
    Var,
    String,
    Number,
    Keyword,
    Punct,
    Comment,
    /// A character no SPARQL token starts with.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct Document {}

impl Document {
    /// Parses `text` with the prefixes the document does not declare itself
    /// (project and setting prefixes) and the base IRI it is resolved against.
    pub fn parse(_text: &str, _prefixes: &PrefixMap, _base: Option<&str>) -> Document {
        todo!("SPARQL core slice")
    }

    /// Every token, comments included, in order.
    pub fn tokens(&self) -> &[Token] {
        todo!("SPARQL core slice")
    }

    /// An update (by its first keyword after the prologue), else a query.
    pub fn is_update(&self) -> bool {
        todo!("SPARQL core slice")
    }

    /// The checks that need no workspace knowledge: syntax, prefixes and
    /// unbound projections.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        todo!("SPARQL core slice")
    }
}

/// The formatted text, or why it was not formatted (syntax errors, or a
/// safety check that failed).
pub fn format(
    _text: &str,
    _prefixes: &PrefixMap,
    _base: Option<&str>,
    _opts: &Options,
) -> Result<String, String> {
    todo!("SPARQL formatter slice")
}
