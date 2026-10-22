//! SPARQL documents (queries and updates, SPARQL 1.1 and 1.2): tokens with
//! spans, the document's structure, and the checks and formatting that need
//! no knowledge of the workspace. Independent of the RDF parser. The language
//! features built on it live in `features`.
//!
//! [`Document::parse`] runs three layers, each on the text as typed (broken
//! or not):
//!
//! - **Tokens** ([`lexer`]): every token of SPARQL 1.2 with its byte span,
//!   comments included, never failing (a character no token starts with, an
//!   unterminated string or a word that is no keyword is a
//!   [`TokenKind::Error`] token). Keywords are matched case-insensitively.
//! - **Structure** ([`Structure`], from [`Document::structure`]), built from
//!   the tokens by a tolerant walk:
//!   - the prologue: [`Structure::prefixes`], [`Structure::bases`] and
//!     [`Structure::versions`] with the spans of the whole declaration, the
//!     name and the IRI (an update repeats a prologue before any operation;
//!     all of them are listed, in order);
//!   - [`Structure::is_update`]: an update when the first query or update
//!     keyword is an update's (`INSERT`, `LOAD`, ...), not by file
//!     extension; a document without any is an (empty) update when nothing
//!     but a prologue is written, else a query;
//!   - [`Structure::operations`]: the query, or each update operation, with
//!     its kind, first keyword, span and clauses (`WHERE`, `GROUP BY`,
//!     `DELETE`, `USING`, ...) for the outline;
//!   - [`Structure::groups`]: every `{ }` with its kind (`WHERE`, `OPTIONAL`,
//!     `GRAPH`, subquery, template, `VALUES` rows, ...), the keyword that
//!     opened it, its parent and depth, for folding, indentation and
//!     context-dependent completion; an unclosed group runs to the last
//!     token;
//!   - [`Structure::selects`]: the query's `SELECT` and each subquery's,
//!     with its projection (`*`, or each `?v` / `(expr AS ?v)`) and its
//!     `WHERE` group;
//!   - [`Structure::scopes`] and [`Structure::vars`]: every variable
//!     occurrence with its role and scope. A group pattern is one scope (the
//!     groups nested in a `WHERE` share it); a subquery is its own scope,
//!     whose projected variables belong to the enclosing scope. Occurrences
//!     with the same name and [`VarOcc::scope`] are one variable; its
//!     definition is the first of them (the projection, when projected);
//!   - [`Structure::refs`]: every IRI, prefixed name and `a`, with its
//!     resolved IRI, its prefix's [`PrefixSource`] (which `PREFIX` line, the
//!     external prefixes, or undeclared) and [`TermRef::prefix_span`], and
//!     its [`TermRole`] (subject, predicate incl. property paths, object,
//!     graph, datatype, function, value), whether it is inside a triple term,
//!     and for predicates and objects the span of the triple's subject;
//!   - [`Structure::triples`]: each triple of a pattern or template as far as
//!     it is written (subject, predicate, object spans), for completion by
//!     position and by the subject's `a` type.
//! - **Checks** ([`Document::diagnostics`]): syntax by `spargebra`
//!   (`parse_query` or `parse_update`, with the document's prefixes, the
//!   external prefixes and the base), located by [`validate`] at the token
//!   where the text stops making sense; undeclared, unused, duplicate and
//!   redefined prefixes; projected variables the `WHERE` never binds. The
//!   codes and levels are the Turtle ones where the meaning is the same.
//!   Every undeclared prefix is reported on its prefixed name and never also
//!   as a syntax error, and syntax errors after it are still found (see
//!   [`diagnostics`]). The structure also rejects three constructs
//!   `spargebra` accepts: a triple term or literal as the subject of a
//!   triple term, and an aggregate inside an aggregate.
//!
//! Knowledge-based checks (unknown or deprecated vocabulary terms, own terms
//! nothing describes) belong to the features, which have the workspace: they
//! use [`Structure::refs`].
//!
//! Undeclared-prefix messages are `Undeclared prefix 'foaf:'`; the server
//! appends the namespace to add (`(add PREFIX foaf: <ns>)`) from the
//! project's bindings and the vocabularies, as for Turtle.

pub(crate) mod diagnostics;
pub mod features;
mod format;
pub mod lexer;
pub(crate) mod structure;
pub mod validate;

pub(crate) use structure::{
    Form, GroupId, GroupKind, Operation, OperationKind, PrefixDecl, PrefixSource, Structure,
    TermRef, TermRole, TriplePattern, VarOcc, VarRole,
};

use crate::layout::Options;
use crate::syntax::{PrefixMap, Span};

/// Why `text` must not be given to `spargebra`, if it must not: nesting or
/// `OPTIONAL` / `MINUS` clauses deep enough to overflow its recursion. For
/// callers outside documents (SHACL-SPARQL queries).
pub(crate) fn too_complex(text: &str) -> Option<String> {
    diagnostics::too_complex(text, &lexer::tokenize(text)).map(|(_, message)| message)
}

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
    /// `<...>`.
    Iri,
    /// `prefix:local` or `prefix:`.
    PName,
    /// `?name` or `$name`.
    Var,
    /// Any string, `"..."`, `'...'`, `"""..."""` or `'''...'''`.
    String,
    /// An unsigned integer, decimal or double (`-` / `+` are [`Punct`](TokenKind::Punct)).
    Number,
    /// `true` / `false`.
    Boolean,
    /// `@en`, `@en-GB`, `@ar--rtl`.
    LangTag,
    /// `_:label`.
    BlankNode,
    /// A keyword (`SELECT`, `WHERE`, `OPTIONAL`, `NOT`, `EXISTS`, `AS`, ...),
    /// any case; [`lexer::keyword`] gives its upper-case form.
    Keyword,
    /// A built-in function or aggregate name (`STRLEN`, `isBLANK`, `COUNT`),
    /// any case; [`lexer::function`] gives its grammar spelling.
    Function,
    /// The `a` keyword (`rdf:type`).
    A,
    /// Punctuation and operators: `{ } ( ) [ ] . , ; * + - / ! ? ^ | = ~ < >`,
    /// `<= >= != && || ^^`, `<< >> <<( )>> {| |}`.
    Punct,
    /// `# ...` to the end of the line.
    Comment,
    /// A character no SPARQL token starts with, an unterminated string, or a
    /// word that is no keyword or function.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.span.start as usize..self.span.end as usize]
    }
}

pub struct Document {
    text: String,
    tokens: Vec<Token>,
    structure: Structure,
    diagnostics: Vec<Diagnostic>,
}

impl Document {
    /// Parses `text` with the prefixes the document does not declare itself
    /// (project and setting prefixes) and the base IRI it is resolved against.
    pub fn parse(text: &str, prefixes: &PrefixMap, base: Option<&str>) -> Document {
        let tokens = lexer::tokenize(text);
        let structure = structure::build(text, &tokens, prefixes, base);
        let diagnostics = diagnostics::diagnostics(text, &tokens, &structure, prefixes, base);
        Document {
            text: text.to_owned(),
            tokens,
            structure,
            diagnostics,
        }
    }

    /// [`Document::parse`] without the checks, for queries whose checks
    /// live elsewhere (SHACL-SPARQL literals: `features::shacl_lint`).
    pub(crate) fn parse_unchecked(
        text: &str,
        prefixes: &PrefixMap,
        base: Option<&str>,
    ) -> Document {
        let tokens = lexer::tokenize(text);
        let structure = structure::build(text, &tokens, prefixes, base);
        Document {
            text: text.to_owned(),
            tokens,
            structure,
            diagnostics: Vec::new(),
        }
    }

    /// The text it was parsed from.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Every token, comments included, in order.
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// The prologue, operations, groups, scopes, variables and IRIs.
    pub(crate) fn structure(&self) -> &Structure {
        &self.structure
    }

    /// An update (by its first keyword after the prologue), else a query.
    pub fn is_update(&self) -> bool {
        self.structure.is_update
    }

    /// The checks that need no workspace knowledge: syntax, prefixes and
    /// unbound projections, in text order.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Whether any diagnostic is an error (the formatter refuses then).
    pub(crate) fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.level == Level::Error)
    }
}

/// The formatted text, or why it was not formatted (syntax errors, or a
/// safety check that failed).
pub fn format(
    text: &str,
    prefixes: &PrefixMap,
    base: Option<&str>,
    opts: &Options,
) -> Result<String, String> {
    format::format(text, prefixes, base, opts)
}

#[cfg(test)]
mod tests;
