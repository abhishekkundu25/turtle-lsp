//! SHACL: the shapes in a graph ([`model`]), read by the shape lint; the
//! IRIs the SHACL code compares against ([`vocab`]); SHACL Core validation
//! ([`validate`]) over the union of many documents ([`crate::rdf::graph`]).

pub(crate) mod literal;
pub(crate) mod model;
pub(crate) mod sparql;
pub mod validate;
pub(crate) mod vocab;

pub use model::Path;
pub(crate) use model::{Shape, ShapeKind, Shapes};
pub use validate::validate_report;
pub(crate) use validate::{Failure, FailureKind, ValidationResult, validate_report_among};

pub const SH: &str = "http://www.w3.org/ns/shacl#";
