//! SHACL: the shapes in a graph ([`model`]), read by the shape lint; the
//! IRIs the SHACL code compares against ([`vocab`]); SHACL Core validation
//! ([`validate`]) over the union of many documents ([`crate::rdf::graph`]).

pub mod literal;
pub mod model;
pub mod sparql;
pub mod validate;
pub mod vocab;

pub use model::{Path, Shape, ShapeKind, Shapes};
pub use validate::{
    Failure, FailureKind, Report, ValidationResult, validate_report, validate_report_among,
};

pub const SH: &str = "http://www.w3.org/ns/shacl#";
