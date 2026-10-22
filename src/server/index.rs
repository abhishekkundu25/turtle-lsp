//! The workspace index as it arrives from its threads: the initial index,
//! built in the background at startup, and the indexed files parsed again
//! after their prefixes changed.

use super::worker::{Fed, Feed};
use crate::analysis::Analysis;
use crossbeam_channel::RecvError;
#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::{Receiver, Sender, unbounded};

/// The initial index: the files parsed, or why it failed.
pub(super) type Initial = Result<Vec<(String, Analysis)>, String>;

/// Indexed files parsed again, or why that failed.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type Reparsed = Result<Vec<crate::workspace::Reparsed>, String>;

pub(super) struct Index {
    initial: Feed<Initial>,
    /// The initial index has been merged.
    merged: bool,
    /// Indexed files parsed again after prefixes changed. This holds the
    /// sender, so the channel never disconnects.
    #[cfg(not(target_arch = "wasm32"))]
    reparsed_tx: Sender<Reparsed>,
    #[cfg(not(target_arch = "wasm32"))]
    reparsed_rx: Receiver<Reparsed>,
}

impl Index {
    pub(super) fn new() -> Index {
        #[cfg(not(target_arch = "wasm32"))]
        let (reparsed_tx, reparsed_rx) = unbounded();
        Index {
            initial: Feed::default(),
            merged: false,
            #[cfg(not(target_arch = "wasm32"))]
            reparsed_tx,
            #[cfg(not(target_arch = "wasm32"))]
            reparsed_rx,
        }
    }

    /// The initial index comes on `rx`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn expect(&mut self, rx: Receiver<Initial>) {
        self.initial = Feed::open(rx);
    }

    /// The initial index's channel, for the main loop to select on.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn rx(&self) -> Receiver<Initial> {
        self.initial.rx()
    }

    /// What a receive on [`Index::rx`] found.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn take(&mut self, got: Result<Initial, RecvError>) -> Fed<Initial> {
        self.initial.take(got)
    }

    /// Wait for the initial index (Gone at once when nothing will send it).
    pub(super) fn wait(&mut self) -> Fed<Initial> {
        let got = if self.initial.is_open() {
            self.initial.rx().recv()
        } else {
            Err(RecvError)
        };
        self.initial.take(got)
    }

    /// The initial index has been merged.
    pub(super) fn merged(&self) -> bool {
        self.merged
    }

    /// The initial index was merged. Its sender is gone now, and a
    /// disconnected channel is always ready, which would wake the main loop
    /// forever: stop listening.
    pub(super) fn set_merged(&mut self) {
        self.merged = true;
        self.initial.close();
    }

    /// Files parsed again, for the main loop to select on.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn reparsed(&self) -> Receiver<Reparsed> {
        self.reparsed_rx.clone()
    }

    /// Where a thread sends the files it parsed again.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn reparsed_sender(&self) -> Sender<Reparsed> {
        self.reparsed_tx.clone()
    }
}
