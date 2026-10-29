//! Curation lints: mistakes in how a workspace's own terms are labelled,
//! defined and published, which the syntax does not catch.
//!
//! On by default: `duplicate-label`, `label-whitespace` and
//! `multiple-labels`. Off until `lint.enable` lists them ([`OPT_IN`]): the
//! house-style checks. `lint.ignore` switches any of them off; the
//! composer ([`crate::diagnose::document`]) applies both lists.

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use lsp_types::Diagnostic;

// shortcut: unused until the label checks land; the slices remove the allow.
#[allow(dead_code)]
pub(crate) const DUPLICATE_LABEL: &str = "duplicate-label";
#[allow(dead_code)]
pub(crate) const LABEL_WHITESPACE: &str = "label-whitespace";
#[allow(dead_code)]
pub(crate) const MULTIPLE_LABELS: &str = "multiple-labels";
pub(crate) const MISSING_LABEL: &str = "missing-label";
pub(crate) const MISSING_DEFINITION: &str = "missing-definition";
pub(crate) const MULTIPLE_DEFINITIONS: &str = "multiple-definitions";
pub(crate) const DUPLICATE_DEFINITION: &str = "duplicate-definition";
pub(crate) const LOWERCASE_DEFINITION: &str = "lowercase-definition";
pub(crate) const MISSING_ONTOLOGY_TITLE: &str = "missing-ontology-title";
pub(crate) const MISSING_ONTOLOGY_DESCRIPTION: &str = "missing-ontology-description";
pub(crate) const MISSING_ONTOLOGY_LICENSE: &str = "missing-ontology-license";

/// The checks that are off until `lint.enable` lists them: house style,
/// which projects that do not follow it should not see.
pub(crate) const OPT_IN: &[&str] = &[
    MISSING_LABEL,
    MISSING_DEFINITION,
    MULTIPLE_DEFINITIONS,
    DUPLICATE_DEFINITION,
    LOWERCASE_DEFINITION,
    MISSING_ONTOLOGY_TITLE,
    MISSING_ONTOLOGY_DESCRIPTION,
    MISSING_ONTOLOGY_LICENSE,
];

/// The curation diagnostics of `a`; the [`OPT_IN`] checks only with
/// `house_style`.
pub(crate) fn diagnostics(
    _a: &Analysis,
    _k: &Knowledge,
    _enc: Encoding,
    _house_style: bool,
) -> Vec<Diagnostic> {
    Vec::new()
}
