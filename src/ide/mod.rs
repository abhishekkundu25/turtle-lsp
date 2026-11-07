//! `ide::Session`: the editor requests over one workspace, for every kind of
//! document. A request names a document by URI; the session routes it by
//! kind (RDF documents to `features`, SPARQL documents to
//! `sparql::features`) and composes the answers where one family reaches
//! the other's documents. The language server builds a session per request
//! and only decodes the params and encodes the answer.
//!
//! Each group of requests has its own module: `navigation` (definition,
//! references, rename and the like), `display` and `edit`.

use crate::analysis::{Analysis, Encoding, SparqlAnalysis};
use crate::knowledge::Knowledge;
use crate::settings::Settings;
use crate::workspace::{Workspace, normalize_uri};
use std::sync::Arc;

mod display;
mod edit;
mod navigation;

/// The workspace, as one client sees it: the position encoding it
/// negotiated, its settings, whether it resolves code actions' edits
/// lazily (`codeAction/resolve`), and the reasoning state when reasoning is
/// on.
pub(crate) struct Session<'w> {
    ws: &'w mut Workspace,
    enc: Encoding,
    settings: &'w Settings,
    resolve_edits: bool,
    reasoning: Option<&'w Arc<crate::reasoning::Shared>>,
}

/// A document, by kind.
pub(crate) enum Doc {
    Rdf(Arc<Analysis>),
    Sparql(Arc<SparqlAnalysis>),
}

impl<'w> Session<'w> {
    pub(crate) fn new(
        ws: &'w mut Workspace,
        enc: Encoding,
        settings: &'w Settings,
        resolve_edits: bool,
    ) -> Session<'w> {
        Session {
            ws,
            enc,
            settings,
            resolve_edits,
            reasoning: None,
        }
    }

    /// The session with the reasoning state (`None` with reasoning off).
    pub(crate) fn with_reasoning(
        self,
        reasoning: Option<&'w Arc<crate::reasoning::Shared>>,
    ) -> Session<'w> {
        Session { reasoning, ..self }
    }

    /// The reasoning state; `None` with reasoning off.
    #[allow(dead_code)] // Until hover reads it (ticket 03).
    pub(crate) fn reasoning(&self) -> Option<&'w Arc<crate::reasoning::Shared>> {
        self.reasoning
    }

    /// The document at `uri` (in any spelling the client uses): an open
    /// SPARQL document's parse, else the RDF analysis of the open or indexed
    /// document. `None` when the workspace has no such document.
    pub(crate) fn doc(&mut self, uri: &str) -> Option<Doc> {
        self.find(uri).map(|(_, doc)| doc)
    }

    /// The knowledge index, and the position encoding the client negotiated.
    pub(crate) fn knowledge(&mut self) -> (&Knowledge, Encoding) {
        (self.ws.knowledge(), self.enc)
    }

    /// The settings in effect.
    pub(crate) fn settings(&self) -> &Settings {
        self.settings
    }

    /// The workspace's root folders (none in the browser build).
    pub(crate) fn roots(&self) -> &[std::path::PathBuf] {
        self.ws.roots()
    }

    /// The prefixes a document at `uri` may use undeclared: its
    /// `prefixes.ttl` and the `prefixes` setting.
    pub(crate) fn external_prefixes_for(&mut self, uri: &str) -> Arc<crate::syntax::PrefixMap> {
        self.ws.external_prefixes_for(uri)
    }

    /// [`Session::doc`] with the URI the workspace keys it by.
    fn find(&mut self, uri: &str) -> Option<(String, Doc)> {
        let u = normalize_uri(uri);
        let doc = match self.ws.sparql(&u) {
            Some(q) => Doc::Sparql(q),
            None => Doc::Rdf(self.ws.analysis(&u)?),
        };
        Some((u, doc))
    }
}
