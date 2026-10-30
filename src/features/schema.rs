//! The `turtle/schema` request: the workspace's classes, each with a row
//! per property, declared (`rdfs:domain`, `schema:domainIncludes`, SHACL
//! property shapes) against observed (the instances that use it), with a
//! state per row. docs/specs/schema-view.md has the whole contract.
//!
//! Each document's facts are computed once per analysis and kept in
//! [`FactsCache`]; a request merges them. Nothing else is kept between
//! requests, and the index is only read.

use crate::analysis::{Analysis, Encoding};
use crate::features::graph::InvalidParams;
use crate::knowledge::Source;
use lsp_types::{Position, Range};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The most classes a request can ask for.
pub(crate) const MAX_CLASSES: u32 = 2_000;
/// The most rows per class a request can ask for.
pub(crate) const MAX_ROWS: u32 = 512;

// ---- the request ----

/// The `turtle/schema` parameters as sent; every field is optional.
/// [`schema`] checks them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SchemaParams {
    /// Document URIs; omitted: every workspace document.
    pub(crate) files: Option<Vec<String>>,
    /// A class or property IRI.
    pub(crate) focus: Option<String>,
    /// The term at a document position (Follow cursor). The server resolves
    /// it to `focus` before the job starts (see `Session::schema_job`).
    pub(crate) at: Option<At>,
    pub(crate) max_classes: Option<u32>,
    pub(crate) max_rows: Option<u32>,
    /// List imported classes; omitted or false: count them only.
    pub(crate) imports: bool,
}

/// A position in a document, as `at` sends it.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct At {
    pub(crate) uri: String,
    pub(crate) position: Position,
}

// ---- the answer ----

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SchemaResult {
    pub(crate) classes: Vec<ClassBox>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) focus: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) focus_row: Option<u32>,
    pub(crate) total: SchemaTotal,
    /// Imported classes not listed (`imports` false).
    pub(crate) imported: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct SchemaTotal {
    /// Before the cap.
    pub(crate) classes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ClassKind {
    /// Declared or used in a workspace document.
    Class,
    /// Declared only by an imported ontology.
    Imported,
    /// Used as a class (a type, a domain, a target) and declared nowhere.
    Undefined,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClassBox {
    pub(crate) id: String,
    /// A prefixed name, else `<iri>`.
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    /// Where the class is defined, when a workspace document does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) range: Option<Range>,
    pub(crate) kind: ClassKind,
    /// Indexes into `classes` (rdfs:subClassOf), those listed.
    pub(crate) parents: Vec<u32>,
    /// Typed with the class or one of its subclasses.
    pub(crate) instances: usize,
    /// Rows whose state is not ok, plus rows with `missing` > 0.
    pub(crate) problems: usize,
    pub(crate) rows: Vec<Row>,
    /// Rows past `maxRows`.
    pub(crate) more_rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RowState {
    Ok,
    /// Declared; no instance uses it.
    Unused,
    /// Used by instances; declared for neither the class nor an ancestor.
    Undeclared,
    /// Literal values outside the one declared datatype.
    BadValues,
    /// `rdfs:range` and the shape's datatype or class disagree.
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum DeclaredBy {
    Domain,
    DomainIncludes,
    Shape,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Declared {
    pub(crate) how: DeclaredBy,
    pub(crate) uri: String,
    pub(crate) range: Range,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Row {
    pub(crate) property: String,
    pub(crate) name: String,
    /// An object property: its values are resources.
    pub(crate) link: bool,
    /// The range class's index in `classes`, when it is listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) range_name: Option<String>,
    pub(crate) declared: Vec<Declared>,
    pub(crate) used: usize,
    pub(crate) of: usize,
    pub(crate) state: RowState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bad_values: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) required: Option<Required>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) missing: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) conflict: Option<Conflict>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Required {
    pub(crate) min: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Conflict {
    /// What `rdfs:range` says.
    pub(crate) declared: String,
    /// What the shape says (`sh:datatype` or `sh:class`).
    pub(crate) shape: String,
}

/// Per-document facts, kept across requests and keyed by the analysis
/// they were computed from (a changed document has a new analysis).
/// Shared between jobs: an entry is written only by the job that computed
/// it, and two jobs computing the same entry write equal values.
#[derive(Default)]
pub(crate) struct FactsCache {
    // Filled in by the implementation (ticket 1).
}

/// `turtle/schema`: the schema of the documents in `sources` (external
/// sources are the imports), as `params` asks. `focus` is `params.focus`,
/// or the IRI the server resolved `params.at` to.
pub(crate) fn schema(
    sources: &[(String, Source, Arc<Analysis>)],
    enc: Encoding,
    params: SchemaParams,
    focus: Option<String>,
    cache: &FactsCache,
) -> Result<SchemaResult, InvalidParams> {
    let _ = (sources, enc, focus, cache);
    if params
        .max_classes
        .is_some_and(|n| n == 0 || n > MAX_CLASSES)
    {
        return Err(InvalidParams(format!(
            "maxClasses must be 1 to {MAX_CLASSES}"
        )));
    }
    if params.max_rows.is_some_and(|n| n == 0 || n > MAX_ROWS) {
        return Err(InvalidParams(format!("maxRows must be 1 to {MAX_ROWS}")));
    }
    Ok(SchemaResult::default())
}
