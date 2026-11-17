//! The hover held until its proofs are found.
//!
//! A hover on a resource waits a short time for the proofs of its inferred
//! types (`reasoning::HOVER_WAIT`); past it, the proof search goes on in the
//! background. Rather than answer then that the proofs are still being
//! worked out, which an editor shows until the user hovers again, the
//! server holds the request: a thread waits for the search to end, and the
//! main loop answers the hover again, now with the proofs, or with what the
//! search found by its time limit. A hover held behind another term's search
//! is held again for its own, until its deadline; past it, it is answered
//! as it is.
//!
//! One hover is held at a time: a newer hover request answers the one held
//! as cancelled ([`Hovers::supersede`]). Neovim sends no `$/cancelRequest`
//! when the cursor moves, so a held hover would otherwise open late, about
//! the term left behind; and two held hovers would race for one search's
//! answer. `$/cancelRequest` answers it as cancelled too, as VS Code sends
//! when the cursor moves away. Only on threads: inline (the browser build)
//! the search runs within the hover, which never waits.

use crate::clock::Instant;
use crate::server::worker::Spawn;
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_server::RequestId;
use lsp_types::Position;
use std::time::Duration;

/// The stack of a thread that only waits on a channel.
const WAITER_STACK: usize = 64 << 10;

/// A held hover: what it asked, and until when it may be held.
pub(super) struct Held {
    pub(super) uri: String,
    pub(super) pos: Position,
    pub(super) deadline: Instant,
}

/// The hover held, by request id, and the channel on which the threads
/// waiting for searches wake the main loop.
pub(super) struct Hovers {
    held: Option<(RequestId, Held)>,
    tx: Sender<RequestId>,
    rx: Receiver<RequestId>,
}

impl Default for Hovers {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Hovers { held: None, tx, rx }
    }
}

impl Hovers {
    /// The deadline of a hover asked now, whose proof searches may each run
    /// `limit`: one for a search going on beside it, one for its own, and
    /// a margin for the answers to come back.
    pub(super) fn deadline(limit: Duration) -> Instant {
        Instant::now() + limit * 2 + Duration::from_millis(500)
    }

    /// Hold `held` as request `id` until `ended` disconnects (its search
    /// ended) or its deadline passes, whichever comes first, in place of
    /// any hover held ([`Hovers::supersede`] answers that one first). False
    /// when it cannot be held: its deadline has passed, or no thread could
    /// start to wait.
    pub(super) fn hold(&mut self, id: RequestId, held: Held, ended: Receiver<()>) -> bool {
        let left = held.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        let (tx, woken) = (self.tx.clone(), id.clone());
        let waited = Spawn::Thread.run(
            "hover-wait",
            WAITER_STACK,
            move || {
                // Disconnected when the search ends; nothing is ever sent.
                let _ = ended.recv_timeout(left);
            },
            move |_| {
                let _ = tx.send(woken);
            },
        );
        if waited.is_err() {
            return false;
        }
        self.held = Some((id, held));
        true
    }

    /// Where the waiting threads say a held hover can be answered. A woken
    /// id that is no longer held was answered already.
    pub(super) fn woken(&self) -> &Receiver<RequestId> {
        &self.rx
    }

    /// The held hover `id`, no longer held; `None` when it is not the one
    /// held (answered already).
    pub(super) fn take(&mut self, id: &RequestId) -> Option<Held> {
        self.held.take_if(|(held, _)| held == id).map(|(_, h)| h)
    }

    /// The id of the hover held.
    pub(super) fn held_id(&self) -> Option<&RequestId> {
        self.held.as_ref().map(|(id, _)| id)
    }

    /// The id of the hover held, no longer held: a newer hover request
    /// came. Its caller answers it as cancelled.
    pub(super) fn supersede(&mut self) -> Option<RequestId> {
        self.held.take().map(|(id, _)| id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(deadline: Instant) -> Held {
        Held {
            uri: "file:///a.ttl".into(),
            pos: Position::new(0, 0),
            deadline,
        }
    }

    #[test]
    fn a_held_hover_wakes_when_its_search_ends_and_is_taken_once() {
        let mut h = Hovers::default();
        let (end, ended) = crossbeam_channel::bounded::<()>(0);
        assert!(h.hold(
            7.into(),
            held(Hovers::deadline(Duration::from_secs(5))),
            ended
        ));
        assert!(
            h.woken().try_recv().is_err(),
            "woken before its search ended"
        );
        drop(end);
        let id = h.woken().recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(id, RequestId::from(7));
        assert!(h.take(&id).is_some());
        assert!(h.take(&id).is_none(), "taken twice");
    }

    #[test]
    fn a_held_hover_wakes_at_its_deadline_when_its_search_goes_on() {
        let mut h = Hovers::default();
        let (_end, ended) = crossbeam_channel::bounded::<()>(0);
        let deadline = Instant::now() + Duration::from_millis(50);
        assert!(h.hold(1.into(), held(deadline), ended));
        let id = h.woken().recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(id, RequestId::from(1));
        assert!(Instant::now() >= deadline);
    }

    #[test]
    fn a_hover_past_its_deadline_or_without_a_thread_is_not_held() {
        let mut h = Hovers::default();
        let (_end, ended) = crossbeam_channel::bounded::<()>(0);
        assert!(!h.hold(1.into(), held(Instant::now()), ended.clone()));
        let later = Hovers::deadline(Duration::from_secs(5));
        assert!(!crate::server::worker::failing_starts(|| h.hold(
            2.into(),
            held(later),
            ended
        )));
        assert_eq!(h.supersede(), None);
    }

    #[test]
    fn a_newer_hover_supersedes_the_one_held_whose_wake_is_then_ignored() {
        let mut h = Hovers::default();
        let (end, ended) = crossbeam_channel::bounded::<()>(0);
        let later = Hovers::deadline(Duration::from_secs(5));
        assert!(h.hold(1.into(), held(later), ended.clone()));
        assert_eq!(h.supersede(), Some(RequestId::from(1)));
        assert!(h.hold(2.into(), held(later), ended));
        drop(end);
        // Both waiting threads wake; only the one held is answered.
        let mut woken = [
            h.woken().recv_timeout(Duration::from_secs(5)).unwrap(),
            h.woken().recv_timeout(Duration::from_secs(5)).unwrap(),
        ];
        woken.sort_by_key(|id| id.to_string());
        assert_eq!(woken, [RequestId::from(1), RequestId::from(2)]);
        assert!(h.take(&1.into()).is_none());
        assert!(h.take(&2.into()).is_some());
    }
}
