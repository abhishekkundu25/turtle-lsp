//! RDF data other features read: the union of many documents' triples
//! ([`graph`]) and that union as a SPARQL dataset ([`dataset`]).

pub mod dataset;
pub mod graph;

pub use dataset::Index;
pub use graph::Graph;
