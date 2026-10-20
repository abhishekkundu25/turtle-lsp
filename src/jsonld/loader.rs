//! Where remote `@context` documents come from. Context processing asks a
//! [`ContextLoader`] for each context URL; the loader decides whether a
//! document is available (a file next to the document, the imports cache,
//! the bundled common contexts) and whether the network may be used.
//!
//! [`NoNetwork`] serves only the documents given to it: tests and the W3C
//! remote-doc runner inject documents through it.

use rustc_hash::FxHashMap;
use std::sync::Arc;

/// A loaded context document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDocument {
    /// The URL the document was finally read from (after redirects): the
    /// base for relative IRIs inside it.
    pub url: String,
    /// The document's JSON text.
    pub text: Arc<str>,
}

/// Why a context document could not be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Fetching it would need the network, which is not allowed
    /// (`imports.network` / `--network` is off).
    NetworkNotAllowed,
    /// No such document (a missing file, an HTTP 404).
    NotFound,
    /// Any other failure (I/O, HTTP, not JSON), described for the user.
    Failed(String),
}

/// Loads context documents by absolute URL. Synchronous: parsing a document
/// waits for its contexts. Shared across the threads that parse documents.
pub trait ContextLoader: Send + Sync {
    /// The document at `url` (absolute, already resolved against the
    /// referencing document's base).
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError>;
}

/// A loader that never touches the network or the disk: it serves the
/// documents given with [`NoNetwork::with`], and refuses the rest
/// (`http(s):` URLs with [`LoadError::NetworkNotAllowed`], others with
/// [`LoadError::NotFound`]).
#[derive(Clone, Debug, Default)]
pub struct NoNetwork {
    docs: FxHashMap<String, Arc<str>>,
}

impl NoNetwork {
    /// Serve `text` as the document at `url`.
    pub fn with(mut self, url: impl Into<String>, text: impl Into<Arc<str>>) -> NoNetwork {
        self.docs.insert(url.into(), text.into());
        self
    }
}

impl ContextLoader for NoNetwork {
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        match self.docs.get(url) {
            Some(text) => Ok(RemoteDocument {
                url: url.to_string(),
                text: text.clone(),
            }),
            None if url.starts_with("http://") || url.starts_with("https://") => {
                Err(LoadError::NetworkNotAllowed)
            }
            None => Err(LoadError::NotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_network_serves_only_what_it_was_given() {
        let l = NoNetwork::default().with("https://example.org/ctx", "{}");
        let doc = l.load("https://example.org/ctx").unwrap();
        assert_eq!(
            (doc.url.as_str(), &*doc.text),
            ("https://example.org/ctx", "{}")
        );
        assert_eq!(
            l.load("https://schema.org/"),
            Err(LoadError::NetworkNotAllowed)
        );
        assert_eq!(l.load("file:///x/ctx.jsonld"), Err(LoadError::NotFound));
    }
}
