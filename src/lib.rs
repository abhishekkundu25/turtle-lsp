//! Turtle (RDF 1.2) language server.

pub mod analysis;
pub mod batch;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod clock;
pub mod commands;
pub mod features;
pub mod imports;
pub mod jsonld;
pub mod knowledge;
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
