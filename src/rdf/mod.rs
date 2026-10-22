//! RDF data other features read: the union of many documents' triples
//! ([`graph`]) and that union as a SPARQL dataset ([`dataset`]).

pub(crate) mod dataset;
pub mod graph;

pub(crate) use dataset::Index;
pub(crate) use graph::Graph;
