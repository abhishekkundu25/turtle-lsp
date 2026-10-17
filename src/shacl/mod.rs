//! SHACL: the shapes in a set of triples ([`shapes`]), shared by the shape
//! lint and validation; SHACL Core validation ([`validate`]) over the union
//! of many documents ([`graph`]).

pub mod graph;
pub mod literal;
pub mod shapes;
pub mod sparql;
pub mod validate;

pub use graph::Graph;
pub use shapes::{Path, Shape, ShapeKind, Shapes};
pub use validate::{Failure, Report, ValidationResult, validate, validate_focus, validate_report};

pub const SH: &str = "http://www.w3.org/ns/shacl#";
