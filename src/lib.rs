//! Turtle (RDF 1.2) language server.
//!
//! # Modules
//!
//! | Module | Owns | Called by |
//! |---|---|---|
//! | [`syntax`] | Turtle, TriG, N-Triples and N-Quads lexer and parser | every feature |
//! | [`sparql`] | SPARQL documents: tokens, structure, checks, formatting | server, [`query`], [`shacl`] |
//! | [`jsonld`] | JSON-LD 1.1 frontend producing the same parse as Turtle | [`analysis`] |
//! | [`analysis`] | One document's parse, positions and per-IRI indexes | [`features`], [`server`] |
//! | [`workspace`] | Open documents, indexed files and project prefixes | [`server`], [`batch`], [`cli`] |
//! | [`knowledge`] | What the workspace says about each IRI | [`features`] |
//! | [`vocab`] | Embedded well-known vocabularies | [`knowledge`], [`features`] |
//! | [`imports`] | `owl:imports` resolution and the import cache | [`server`], [`cli`] |
//! | [`fetch`] | Network fetching and its limits (stub) | [`imports`], [`jsonld`] |
//! | [`features`] | Completion, hover, diagnostics, formatting and the other editor features | [`server`], [`batch`] |
//! | [`diagnose`] | The diagnostics composer (stub) | [`server`], [`batch`] |
//! | [`shacl`] | Shapes, SHACL Core and SHACL-SPARQL validation | [`features`] |
//! | [`rdf`] | The union of documents' triples as a graph and a SPARQL dataset | [`query`], [`shacl`] |
//! | [`query`] | Running SPARQL queries and rendering results | [`cli`], [`server`], the browser engine |
//! | [`settings`] | Client settings and their layers | [`server`], [`cli`] |
//! | [`commands`] | `workspace/executeCommand` commands | [`server`] |
//! | [`server`] | The LSP main loop | `main` |
//! | [`batch`] | Files analysed as one workspace without the event loop | [`cli`], the browser engine |
//! | [`cli`] | `check`, `format` and `query` | `main` |
//! | [`clock`] | The time the server's timers read | [`server`], [`imports`] |
//! | [`ide`] | Shared editor session state (stub) | [`server`], the browser engine |
//! | [`layout`] | Shared text layout engine (stub) | [`features`] |

pub mod analysis;
pub mod batch;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod clock;
pub mod commands;
pub mod diagnose;
pub mod features;
#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
pub mod ide;
pub mod imports;
pub mod jsonld;
pub mod knowledge;
pub mod layout;
pub mod query;
pub mod rdf;
pub mod server;
pub mod settings;
pub mod shacl;
pub mod sparql;
pub mod syntax;
pub mod vocab;
pub mod workspace;

/// The version of this build, as the server reports it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
