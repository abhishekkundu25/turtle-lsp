//! When the client is told to ask again: pulled diagnostics
//! (`workspace/diagnostic/refresh`) and what open documents display
//! (`workspace/{inlayHint,semanticTokens,codeLens}/refresh`).
//!
//! Pulled reports carry a result id made of the document version, the
//! knowledge generation and the described generation. Anything outside a
//! document that can change its diagnostics bumps the knowledge generation
//! ([`Refresh::knowledge_changed`]); the client is asked to pull again only
//! when a report went out since it was last asked.

use super::client::Client;
use lsp_types::request::Request as _;

pub(super) struct Refresh {
    /// The client pulls diagnostics; nothing is pushed to it.
    pull: bool,
    /// The client supports `workspace/diagnostic/refresh`.
    refresh_support: bool,
    /// Bumped whenever knowledge outside a document changes (prefixes.ttl,
    /// the disk index, settings). Part of every pulled report's result id.
    generation: u64,
    /// A pulled report went out since the last `workspace/diagnostic/refresh`.
    reported: bool,
    /// The generation moved after a report went out: refresh at end of batch.
    refresh_needed: bool,
    /// The knowledge index's described generation last acted on.
    described_seen: u64,
    /// `workspace/{inlayHint,semanticTokens,codeLens}/refresh`, those the
    /// client supports.
    display_refresh: Vec<&'static str>,
    /// Knowledge changed since the last display refresh.
    display_stale: bool,
}

impl Refresh {
    pub(super) fn new(
        pull: bool,
        refresh_support: bool,
        display_refresh: Vec<&'static str>,
    ) -> Refresh {
        Refresh {
            pull,
            refresh_support,
            generation: 0,
            reported: false,
            refresh_needed: false,
            described_seen: 0,
            display_refresh,
            display_stale: false,
        }
    }

    /// The client pulls diagnostics.
    pub(super) fn pull(&self) -> bool {
        self.pull
    }

    /// Knowledge outside any one document changed: pulled reports handed
    /// out so far are stale, and so are the labels and term kinds shown in
    /// open documents.
    pub(super) fn knowledge_changed(&mut self) {
        self.generation += 1;
        if self.reported {
            self.refresh_needed = true;
        }
        self.display_stale = true;
    }

    /// A pulled report of the document at `version` goes out: its result id.
    /// The described set is part of it: other documents' edits that change
    /// it may not have bumped the generation yet (debounced).
    pub(super) fn report_id(&mut self, version: &str, described: u64) -> String {
        self.reported = true;
        format!("{version}:{}:{described}", self.generation)
    }

    /// The described generation is not the one last acted on.
    pub(super) fn described_moved(&self, described: u64) -> bool {
        described != self.described_seen
    }

    /// Act on described generation `described`: true when it moved since
    /// last acted on (and knowledge changed).
    pub(super) fn described_settled(&mut self, described: u64) -> bool {
        if !self.described_moved(described) {
            return false;
        }
        self.described_seen = described;
        true
    }

    /// End of a batch: ask the client for the refreshes due.
    pub(super) fn send(&mut self, client: &mut Client) {
        if std::mem::take(&mut self.display_stale) {
            for &m in &self.display_refresh {
                client.request(m, ());
            }
        }
        if !std::mem::take(&mut self.refresh_needed) {
            return;
        }
        self.reported = false;
        if self.refresh_support {
            client.request(lsp_types::request::WorkspaceDiagnosticRefresh::METHOD, ());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::wire::{Outbox, memory};
    use lsp_server::Message;

    fn requests(rx: &Outbox) -> Vec<String> {
        rx.try_iter()
            .map(|m| match m {
                Message::Request(r) => r.method,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_generation_bump_asks_for_a_refresh_only_after_a_report_went_out() {
        let (tx, rx) = memory();
        let mut client = Client::new(tx);
        let mut r = Refresh::new(true, true, vec![]);
        r.knowledge_changed();
        r.send(&mut client);
        assert!(requests(&rx).is_empty(), "no report went out yet");
        r.report_id("1", 0);
        r.send(&mut client);
        assert!(
            requests(&rx).is_empty(),
            "the generation did not move since"
        );
        r.knowledge_changed();
        r.send(&mut client);
        assert_eq!(requests(&rx), ["workspace/diagnostic/refresh"]);
        // Once asked, the client is not asked again until it pulls.
        r.knowledge_changed();
        r.send(&mut client);
        assert!(requests(&rx).is_empty());
        // A client without refresh support is never asked.
        let mut r = Refresh::new(false, false, vec![]);
        r.report_id("1", 0);
        r.knowledge_changed();
        r.send(&mut client);
        assert!(requests(&rx).is_empty());
    }

    #[test]
    fn display_refreshes_go_out_once_per_knowledge_change() {
        let (tx, rx) = memory();
        let mut client = Client::new(tx);
        let mut r = Refresh::new(false, false, vec!["workspace/inlayHint/refresh"]);
        r.send(&mut client);
        assert!(requests(&rx).is_empty());
        r.knowledge_changed();
        r.send(&mut client);
        r.send(&mut client);
        assert_eq!(requests(&rx), ["workspace/inlayHint/refresh"]);
    }

    #[test]
    fn the_pulled_result_id_changes_with_the_generation_and_the_described_generation() {
        let mut r = Refresh::new(true, true, vec![]);
        assert_eq!(r.report_id("3", 0), "3:0:0");
        assert_eq!(r.report_id("3", 0), "3:0:0");
        r.knowledge_changed();
        assert_eq!(r.report_id("3", 0), "3:1:0");
        assert_eq!(r.report_id("3", 7), "3:1:7");
        assert_eq!(r.report_id("disk", 7), "disk:1:7");
        // The described generation is acted on once per move.
        assert!(r.described_moved(7));
        assert!(r.described_settled(7));
        assert!(!r.described_moved(7) && !r.described_settled(7));
    }
}
