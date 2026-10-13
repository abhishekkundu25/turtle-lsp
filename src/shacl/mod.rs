//! SHACL: the shapes in a set of triples ([`shapes`]), shared by the shape
//! lint and validation.

pub mod shapes;

pub use shapes::{Path, Shape, ShapeKind, Shapes};

pub const SH: &str = "http://www.w3.org/ns/shacl#";
