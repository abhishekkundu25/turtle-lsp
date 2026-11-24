//! JSON-LD contexts: the loader the workspace analyzes JSON-LD with, the
//! contexts each document asked for, and the documents analyzed or parsed
//! again when a context changes.

#[cfg(not(target_arch = "wasm32"))]
use super::Effects;
use super::{Doc, DocKind, Workspace, normalize_uri, uri_to_path};
use crate::jsonld::loader::{self, Bundled};
use crate::jsonld::{ContextLoader, LoadError, RemoteDocument};
use crate::syntax::Syntax;
use rustc_hash::FxHashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub(super) type ContextUses = Mutex<FxHashMap<String, Vec<String>>>;

/// Record the contexts `uri`'s analysis asked for. A document that asked
/// for none, where none did, costs nothing.
fn record_contexts(uses: &mut FxHashMap<String, Vec<String>>, uri: &str, asked: Vec<String>) {
    if !asked.is_empty() {
        uses.insert(uri.to_string(), asked);
    } else if !uses.is_empty() {
        uses.remove(uri);
    }
}

/// What a workspace hands to another one parsing files for it (the initial
/// index, built on another thread): its context loader.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub(crate) struct SharedContexts {
    loader: Option<Arc<dyn ContextLoader>>,
}

/// The contexts of one JSON-LD analysis: a `file:` URL is the open
/// document's text, else the file's, inside the workspace roots only; other
/// URLs go to `web`. Every URL asked for is recorded.
pub(super) struct DocContexts<'a> {
    open: &'a FxHashMap<String, Doc>,
    roots: &'a [PathBuf],
    web: &'a dyn ContextLoader,
    asked: Mutex<Vec<String>>,
}

impl<'a> DocContexts<'a> {
    pub(super) fn new(
        open: &'a FxHashMap<String, Doc>,
        roots: &'a [PathBuf],
        web: &'a dyn ContextLoader,
    ) -> DocContexts<'a> {
        DocContexts {
            open,
            roots,
            web,
            asked: Mutex::default(),
        }
    }

    /// Record in `uses` what `uri`'s analysis asked for: nothing unless it
    /// is JSON-LD (a document whose content turned from JSON-LD to another
    /// syntax uses no context any more).
    pub(super) fn record(self, uses: &ContextUses, uri: &str, syntax: Syntax) {
        let asked = self.asked(syntax);
        if let Ok(mut uses) = uses.lock() {
            record_contexts(&mut uses, uri, asked);
        }
    }

    /// What `uri`'s analysis asked for, for the caller to record if it keeps
    /// the analysis.
    pub(super) fn asked(self, syntax: Syntax) -> Vec<String> {
        match syntax {
            Syntax::JsonLd => self.asked.into_inner().unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn file(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        let key = normalize_uri(url);
        if let Some(d) = self.open.get(&key) {
            return Ok(RemoteDocument {
                url: url.to_string(),
                text: Arc::from(d.text.as_str()),
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Nothing asks the file system about the file (a host, a share,
            // a path outside the roots) before the roots have said yes.
            let path = crate::imports::fs_policy(url, self.roots).map_err(LoadError::Failed)?;
            loader::read_context_file(&path).map(|text| RemoteDocument {
                url: url.to_string(),
                text: text.into(),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = self.roots;
            Err(LoadError::NotFound)
        }
    }
}

impl ContextLoader for DocContexts<'_> {
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        if let Ok(mut asked) = self.asked.lock()
            && !asked.iter().any(|u| u == url)
        {
            asked.push(url.to_string());
        }
        if url.starts_with("file:") {
            self.file(url)
        } else {
            self.web.load(url)
        }
    }
}

impl Workspace {
    /// Load JSON-LD contexts with [`loader::for_platform`]: the network is
    /// used only when `network` (`imports.network`, `--network`) is on.
    pub(crate) fn set_network(&mut self, network: bool) {
        self.set_context_loader(loader::for_platform(network));
    }

    /// Load JSON-LD contexts with `loader` from now on: open JSON-LD
    /// documents are analyzed again, and indexed ones that use a web
    /// context are parsed again.
    pub(crate) fn set_context_loader(&mut self, loader: Arc<dyn ContextLoader>) {
        self.contexts = Some(loader);
        self.context_changes += 1;
        for d in self.open.values_mut() {
            if d.kind == DocKind::Rdf(Syntax::JsonLd) {
                d.analyzed.outdate();
            }
        }
        let on_web: Vec<PathBuf> = self
            .context_users(|url| !url.starts_with("file:"))
            .into_iter()
            .filter(|u| !self.open.contains_key(u) && self.disk.contains_key(u))
            .filter_map(|u| uri_to_path(&u))
            .collect();
        if !on_web.is_empty() {
            self.index_paths(on_web);
        }
    }

    pub(super) fn contexts(&self) -> Arc<dyn ContextLoader> {
        self.contexts.clone().unwrap_or_else(|| Arc::new(Bundled))
    }

    /// The context loader, for a workspace that parses files for this one
    /// ([`Workspace::use_shared_contexts`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn shared_contexts(&self) -> SharedContexts {
        SharedContexts {
            loader: self.contexts.clone(),
        }
    }

    /// Load contexts as the workspace that shared them does.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn use_shared_contexts(&mut self, shared: SharedContexts) {
        self.contexts = shared.loader;
    }

    /// Record what the analysis of `uri` the workspace keeps asked for.
    pub(super) fn record_uses(&mut self, uri: &str, asked: Vec<String>) {
        let uses = self
            .context_uses
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        record_contexts(uses, uri, asked);
    }

    /// The documents whose last analysis asked for a context URL `wanted`
    /// accepts.
    fn context_users(&self, wanted: impl Fn(&str) -> bool) -> Vec<String> {
        let Ok(uses) = self.context_uses.lock() else {
            return Vec::new();
        };
        uses.iter()
            .filter(|(_, urls)| urls.iter().any(|u| wanted(u)))
            .map(|(doc, _)| doc.clone())
            .collect()
    }

    /// The JSON-LD context at `url` was fetched in the background (or the
    /// fetch failed): the documents that use it are analyzed or parsed
    /// again.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn context_fetched(&mut self, url: &str) -> Effects {
        if self.context_changed(url, true) {
            Effects {
                data: true,
                ..Effects::knowledge()
            }
        } else {
            Effects::default()
        }
    }

    /// The context at `url` changed (an open context file was edited,
    /// saved or closed, a file changed on disk, a fetch arrived): the open
    /// documents that use it are analyzed again, and, with `disk`, the
    /// indexed ones are parsed again. Whether any document uses it.
    pub(crate) fn context_changed(&mut self, url: &str, disk: bool) -> bool {
        if disk {
            self.context_changes += 1;
        }
        let key = normalize_uri(url);
        let users =
            self.context_users(|u| u == url || (u.starts_with("file:") && normalize_uri(u) == key));
        let mut paths = Vec::new();
        for u in &users {
            if let Some(d) = self.open.get_mut(u) {
                d.analyzed.outdate();
            } else if disk && self.disk.contains_key(u) {
                paths.extend(uri_to_path(u));
            }
        }
        if !paths.is_empty() {
            self.index_paths(paths);
        }
        !users.is_empty()
    }
}
