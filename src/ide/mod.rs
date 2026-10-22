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
use crate::settings::Settings;
use crate::workspace::{Workspace, normalize_uri};
use std::sync::Arc;

mod display;
mod edit;
mod navigation;

/// The workspace, as one client sees it: the position encoding it
/// negotiated, its settings, and whether it resolves code actions' edits
/// lazily (`codeAction/resolve`).
pub struct Session<'w> {
    ws: &'w mut Workspace,
    enc: Encoding,
    settings: &'w Settings,
    resolve_edits: bool,
}

/// A document, by kind.
pub enum Doc {
    Rdf(Arc<Analysis>),
    Sparql(Arc<SparqlAnalysis>),
}

impl<'w> Session<'w> {
    pub fn new(
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
        }
    }

    /// The document at `uri` (in any spelling the client uses): an open
    /// SPARQL document's parse, else the RDF analysis of the open or indexed
    /// document. `None` when the workspace has no such document.
    pub fn doc(&mut self, uri: &str) -> Option<Doc> {
        self.find(uri).map(|(_, doc)| doc)
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
