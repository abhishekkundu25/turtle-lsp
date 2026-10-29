//! A language server and command-line checker for RDF 1.2 (Turtle, TriG, N-Triples, N-Quads, JSON-LD) and SPARQL, with SHACL validation.
//!
//! # Modules
//!
//! | Module | Owns | Called by |
//! |---|---|---|
//! | [`syntax`] | Turtle, TriG, N-Triples and N-Quads lexer and parser | every feature |
//! | [`sparql`] | SPARQL documents: tokens, structure, checks, formatting | `ide`, [`analysis`], [`query`], [`shacl`] |
//! | [`jsonld`] | JSON-LD 1.1 frontend producing the same parse as Turtle | [`analysis`] |
//! | [`analysis`] | One document's parse, positions and per-IRI indexes | [`features`], `ide`, [`server`] |
//! | `workspace` | Open documents, indexed files and project prefixes | [`server`], `ide`, [`batch`], [`cli`], [`features`], `diagnose`, [`imports`], [`jsonld`] |
//! | [`knowledge`] | What the workspace says about each IRI | [`features`] |
//! | `vocab` | Embedded well-known vocabularies | [`knowledge`], [`features`] |
//! | [`imports`] | `owl:imports` resolution and the import cache | `workspace`, [`server`], [`batch`] |
//! | `fetch` | Web requests: redirects, the address rule, size, time and request budgets, the cache file layout | [`imports`], [`jsonld`] |
//! | [`features`] | Completion, hover, diagnostics, formatting and the other editor features | [`server`], `ide`, [`batch`] |
//! | `diagnose` | The diagnostics composer: one document's diagnostics, the same for the editor and `check` | `ide`, [`server`], [`batch`] |
//! | [`shacl`] | Shapes, SHACL Core and SHACL-SPARQL validation | [`features`], [`knowledge`] |
//! | [`rdf`] | The union of documents' triples as a graph and a SPARQL dataset | [`query`], [`shacl`] |
//! | [`query`] | Running SPARQL queries and rendering results | [`cli`], [`server`], [`batch`], the browser engine |
//! | [`settings`] | Client settings and their layers | [`server`], [`cli`] |
//! | `commands` | `workspace/executeCommand` commands | [`server`] |
//! | [`server`] | The LSP main loop, and the worker threads other modules start | `main`, the browser engine |
//! | [`batch`] | Files analysed as one workspace without the event loop | [`cli`], the browser engine |
//! | [`cli`] | `check`, `format` and `query` | `main` |
//! | [`clock`] | The time the server's timers read | [`server`], [`query`], [`shacl`] |
//! | `ide` | `ide::Session`: the editor requests over one workspace, routed by document kind | [`server`] |
//! | `layout` | The layout layer the Turtle and SPARQL formatters share: options, token document, triple terms, printer | [`features`], [`sparql`], `ide`, [`batch`]; [`jsonld`] and [`query`] take its options |
//!
//! `commands`, `diagnose`, `fetch`, `ide`, `layout`, `vocab` and `workspace`
//! are private to the crate: no consumer uses them.
//!
//! # Public interface
//!
//! The consumers are the browser engine (`web/engine`), the examples
//! (`examples/`), the integration tests (`tests/`, with `tests/support/`) and
//! the binary (`src/main.rs`). Below is every path they use, one per line,
//! with the consumers that use it. The list comes from a grep of their
//! sources for `turtle_lsp::`, both in `use` statements and inline, and for
//! paths through a module they imported. Methods, fields and enum variants of
//! the listed types are not listed. The list is a doctest, so `cargo test`
//! fails when one of the paths stops being public. An item that only
//! consumers use stays `pub`.
//!
//! ```
//! use turtle_lsp::VERSION as _; // web/engine
//! use turtle_lsp::analysis::Analysis as _; // tests
//! use turtle_lsp::analysis::Encoding as _; // tests
//! use turtle_lsp::analysis::ParsePath as _; // tests
//! use turtle_lsp::analysis::SparqlAnalysis as _; // tests
//! use turtle_lsp::batch::BatchFile as _; // tests, web/engine
//! use turtle_lsp::batch::Columns as _; // web/engine
//! use turtle_lsp::batch::Fix as _; // web/engine
//! use turtle_lsp::batch::FormatOutcome as _; // web/engine
//! use turtle_lsp::batch::Formatted as _; // web/engine
//! use turtle_lsp::batch::Options as _; // tests, web/engine
//! use turtle_lsp::batch::analyse as _; // tests, web/engine
//! use turtle_lsp::batch::fix_all as _; // tests, web/engine
//! use turtle_lsp::batch::query as _; // web/engine
//! use turtle_lsp::cli::USAGE as _; // main
//! use turtle_lsp::cli::run as _; // main
//! use turtle_lsp::clock::Instant as _; // web/engine
//! use turtle_lsp::features::completion::complete as _; // tests
//! use turtle_lsp::features::diagnostics::Options as _; // tests
//! use turtle_lsp::features::diagnostics::diagnostics as _; // tests
//! use turtle_lsp::features::diagnostics::diagnostics_with as _; // tests
//! use turtle_lsp::features::format::Options as _; // examples, tests
//! use turtle_lsp::features::format::format as _; // examples, tests
//! use turtle_lsp::features::format::formatting as _; // tests
//! use turtle_lsp::features::hierarchy::prepare as _; // tests
//! use turtle_lsp::features::hierarchy::subtypes as _; // tests
//! use turtle_lsp::features::hierarchy::supertypes as _; // tests
//! use turtle_lsp::features::hover::hover as _; // tests
//! use turtle_lsp::features::inlay::inlay_hints as _; // tests
//! use turtle_lsp::features::navigation::PrefixScope as _; // tests
//! use turtle_lsp::features::navigation::definition as _; // tests
//! use turtle_lsp::features::navigation::highlights as _; // tests
//! use turtle_lsp::features::navigation::implementations as _; // tests
//! use turtle_lsp::features::navigation::prepare_rename as _; // tests
//! use turtle_lsp::features::navigation::references as _; // tests
//! use turtle_lsp::features::navigation::rename as _; // tests
//! use turtle_lsp::features::navigation::type_definition as _; // tests
//! use turtle_lsp::features::quickfix::code_actions as _; // tests
//! use turtle_lsp::features::refactor::apply as _; // tests
//! use turtle_lsp::features::refactor::code_actions as _; // tests
//! use turtle_lsp::features::refactor::edits_of as _; // tests
//! use turtle_lsp::features::semantic::full as _; // tests
//! use turtle_lsp::features::semantic::range as _; // tests
//! use turtle_lsp::features::shacl_validate::Job as _; // tests
//! use turtle_lsp::features::shacl_validate::run as _; // tests
//! use turtle_lsp::features::symbols::document_symbols as _; // tests
//! use turtle_lsp::features::symbols::folding_ranges as _; // tests
//! use turtle_lsp::features::symbols::workspace_symbols as _; // tests
//! use turtle_lsp::imports::Status as _; // tests
//! use turtle_lsp::imports::diagnostics_with as _; // tests
//! use turtle_lsp::imports::parse_catalog as _; // tests
//! use turtle_lsp::imports::rdf_xml_to_ntriples as _; // tests
//! use turtle_lsp::imports::sniff as _; // tests
//! use turtle_lsp::jsonld::ContextLoader as _; // tests
//! use turtle_lsp::jsonld::LoadError as _; // tests
//! use turtle_lsp::jsonld::NoNetwork as _; // tests
//! use turtle_lsp::jsonld::RemoteDocument as _; // tests
//! use turtle_lsp::jsonld::format::format as _; // tests
//! use turtle_lsp::jsonld::json::Json as _; // tests
//! use turtle_lsp::jsonld::json::TokenKind as _; // tests
//! use turtle_lsp::jsonld::json::Value as _; // tests
//! use turtle_lsp::jsonld::json::parse as _; // tests
//! use turtle_lsp::jsonld::json::tokens as _; // tests
//! use turtle_lsp::jsonld::parse as _; // tests
//! use turtle_lsp::knowledge::Knowledge as _; // tests
//! use turtle_lsp::knowledge::Source as _; // tests
//! use turtle_lsp::oracle::Batch as _; // tests
//! use turtle_lsp::oracle::change as _; // tests
//! use turtle_lsp::oracle::check as _; // tests
//! use turtle_lsp::query::Answer as _; // tests, web/engine
//! use turtle_lsp::query::Limits as _; // tests, web/engine
//! use turtle_lsp::query::QueryError as _; // tests, web/engine
//! use turtle_lsp::query::render::term as _; // web/engine
//! use turtle_lsp::query::render::turtle as _; // web/engine
//! use turtle_lsp::query::run as _; // tests
//! use turtle_lsp::rdf::graph::Graph as _; // examples, tests
//! use turtle_lsp::rdf::graph::Id as _; // examples, tests
//! use turtle_lsp::rdf::graph::Term as _; // examples, tests
//! use turtle_lsp::server::Embedded as _; // tests, web/engine
//! use turtle_lsp::server::MAIN_STACK as _; // main
//! use turtle_lsp::server::run as _; // main
//! use turtle_lsp::settings::InlayLabels as _; // tests
//! use turtle_lsp::settings::Layer as _; // web/engine
//! use turtle_lsp::settings::Settings as _; // tests, web/engine
//! use turtle_lsp::settings::seconds as _; // web/engine
//! use turtle_lsp::shacl::Path as _; // tests
//! use turtle_lsp::shacl::SH as _; // tests
//! use turtle_lsp::shacl::validate::validate_report as _; // examples
//! use turtle_lsp::shacl::validate_report as _; // tests
//! use turtle_lsp::sparql::Document as _; // examples, tests
//! use turtle_lsp::sparql::Level as _; // examples, tests
//! use turtle_lsp::sparql::TokenKind as _; // examples, tests
//! use turtle_lsp::sparql::features::code_actions as _; // tests
//! use turtle_lsp::sparql::features::completion as _; // tests
//! use turtle_lsp::sparql::features::definition as _; // tests
//! use turtle_lsp::sparql::features::diagnostics as _; // tests
//! use turtle_lsp::sparql::features::document_symbols as _; // tests
//! use turtle_lsp::sparql::features::folding_ranges as _; // tests
//! use turtle_lsp::sparql::features::formatting as _; // tests
//! use turtle_lsp::sparql::features::highlights as _; // tests
//! use turtle_lsp::sparql::features::hover as _; // tests
//! use turtle_lsp::sparql::features::inlay_hints as _; // tests
//! use turtle_lsp::sparql::features::prepare_rename as _; // tests
//! use turtle_lsp::sparql::features::rdf_term_rename as _; // tests
//! use turtle_lsp::sparql::features::references as _; // tests
//! use turtle_lsp::sparql::features::rename as _; // tests
//! use turtle_lsp::sparql::features::resolve_code_action as _; // tests
//! use turtle_lsp::sparql::features::semantic_tokens_full as _; // tests
//! use turtle_lsp::sparql::features::semantic_tokens_range as _; // tests
//! use turtle_lsp::sparql::features::term_references as _; // tests
//! use turtle_lsp::sparql::format as _; // examples, tests
//! use turtle_lsp::sparql::lexer::tokenize as _; // examples, tests
//! use turtle_lsp::sparql::validate::text_for_spargebra as _; // examples
//! use turtle_lsp::syntax::BlankId as _; // tests
//! use turtle_lsp::syntax::Direction as _; // tests
//! use turtle_lsp::syntax::Form as _; // tests
//! use turtle_lsp::syntax::GraphBlock as _; // tests
//! use turtle_lsp::syntax::GraphName as _; // tests
//! use turtle_lsp::syntax::Iri as _; // tests
//! use turtle_lsp::syntax::Node as _; // examples, tests
//! use turtle_lsp::syntax::Parsed as _; // examples, tests
//! use turtle_lsp::syntax::PrefixDecl as _; // tests
//! use turtle_lsp::syntax::PrefixMap as _; // examples, tests
//! use turtle_lsp::syntax::PrefixUse as _; // tests
//! use turtle_lsp::syntax::RDF as _; // tests
//! use turtle_lsp::syntax::Ref as _; // tests
//! use turtle_lsp::syntax::Role as _; // tests
//! use turtle_lsp::syntax::Severity as _; // examples, tests
//! use turtle_lsp::syntax::Span as _; // tests
//! use turtle_lsp::syntax::Statement as _; // tests
//! use turtle_lsp::syntax::Syntax as _; // examples, tests
//! use turtle_lsp::syntax::SyntaxError as _; // tests
//! use turtle_lsp::syntax::Triple as _; // examples, tests
//! use turtle_lsp::syntax::XSD as _; // tests
//! use turtle_lsp::syntax::lexer::Kind as _; // examples, tests
//! use turtle_lsp::syntax::lexer::Token as _; // tests
//! use turtle_lsp::syntax::lexer::tokenize as _; // examples, tests
//! use turtle_lsp::syntax::parse as _; // examples, tests
//! use turtle_lsp::syntax::parse_with_base as _; // examples, tests
//! ```

pub mod analysis;
pub mod batch;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod clock;
pub(crate) mod commands;
pub(crate) mod diagnose;
pub mod features;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod fetch;
pub(crate) mod ide;
pub mod imports;
pub mod jsonld;
pub mod knowledge;
pub(crate) mod layout;
#[doc(hidden)]
pub mod oracle;
pub mod query;
pub mod rdf;
pub mod server;
pub mod settings;
pub mod shacl;
pub mod sparql;
pub mod syntax;
pub(crate) mod vocab;
pub(crate) mod workspace;

/// The version of this build, as the server reports it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
