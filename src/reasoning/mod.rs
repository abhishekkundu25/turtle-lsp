//! Reasoning over the workspace with TurbOWL, an OWL 2 RL reasoner
//! (`turtle.reasoning.rules`: `off`, `default` or `complete`).
//!
//! - [`Base`] (`base.rs`): the workspace's triples, imports included, as
//!   TurbOWL reads them, each mapped back to its document, range and line.
//! - [`Shared`] (`state.rs`): the reasoner after its last run, with that
//!   run's base, its generation and the explainer's cache, behind a
//!   read-write lock that only the reasoning thread writes; and the read
//!   hover makes, a term's inferred types with their proofs
//!   ([`Shared::inferred_types`]), which waits at most a given time.
//! - [`Worker`] and [`run_inline`] (`worker.rs`): the reasoning thread, one
//!   run at a time, the newest job first, panics caught, as SHACL
//!   validation's; `run_inline` for the embedded server.
//! - `diagnostics.rs`: what a run shows in the documents.
//!
//! With reasoning off nothing here is built: the server holds no
//! [`Shared`] and starts no thread.
//!
//! TurbOWL's types stay inside this module: callers see the IRIs, the
//! [`Origin`]s, the LSP diagnostics and the [`Proof`] type, whose contents
//! only this module reads.

// The server schedules runs (ticket 02) and hover reads them (ticket 03);
// until both are wired, most of this is used only by its tests.
#![allow(dead_code, unused_imports)]

mod base;
mod diagnostics;
mod state;
mod worker;

use base::Base;
pub(crate) use base::Origin;
pub(crate) use diagnostics::{ByUri, CODE_PREFIX, SKIPPED_CODE};
pub(crate) use state::{HOVER_WAIT, Inferred, Proof, Shared, Types};
pub(crate) use worker::{Job, Outcome, Worker, run_inline};

use crate::settings::Reasoning;

/// TurbOWL's switches for `rules`; `None` when reasoning is off.
fn rules(rules: Reasoning) -> Option<turbowl::Switches> {
    match rules {
        Reasoning::Off => None,
        Reasoning::Default => Some(turbowl::Switches::default()),
        Reasoning::Complete => Some(turbowl::Switches::complete()),
    }
}
