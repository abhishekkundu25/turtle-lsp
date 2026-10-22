//! Background threads and the channels that bring their answers back.
//!
//! One way to start a thread ([`spawn_guarded`]): a panic in it is caught and
//! handed back as a message instead of ending the thread silently. One way to
//! read a channel ([`Feed`]): when the thread behind it is gone, the feed
//! says so once and then never wakes the main loop again (a disconnected
//! channel is always ready, so reading it again would spin).

use crossbeam_channel::{Receiver, RecvError, never};

/// What a panic carried, for a message.
fn panic_text(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

/// Run `f`, turning a panic into an error message. The default panic hook has
/// already printed the details to stderr (stdout carries the protocol).
pub(crate) fn catch<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|e| format!("turtle-lsp internal error in {what}: {}", panic_text(&*e)))
}

/// Run `work` on a new thread named `name`, with a stack of `stack` bytes,
/// and pass what it returned to `deliver`: `Err` with the panic's message
/// when `work` panicked. `deliver` runs on that thread, after `work`, in
/// either case; the default panic hook has already printed the details to
/// stderr (stdout carries the protocol). Fails only when the thread cannot be
/// started.
pub(crate) fn spawn_guarded<T, W, D>(
    name: &str,
    stack: usize,
    work: W,
    deliver: D,
) -> std::io::Result<()>
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    D: FnOnce(Result<T, String>) + Send + 'static,
{
    let label = name.to_string();
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(stack)
        .spawn(move || deliver(catch(&label, work)))
        .map(|_| ())
}

/// What a read of a [`Feed`] found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fed<T> {
    /// The thread sent this.
    Got(T),
    /// The thread is gone: it will send nothing more. Reported once.
    Gone,
}

/// The answers of one background thread. The main loop selects on
/// [`Feed::rx`] and passes what the select returned to [`Feed::take`]; a
/// closed feed, or one never opened, is a channel that never fires.
pub(crate) struct Feed<T> {
    rx: Option<Receiver<T>>,
}

impl<T> Default for Feed<T> {
    fn default() -> Self {
        Feed { rx: None }
    }
}

impl<T> Feed<T> {
    pub(crate) fn open(rx: Receiver<T>) -> Feed<T> {
        Feed { rx: Some(rx) }
    }

    /// The channel to select on.
    pub(crate) fn rx(&self) -> Receiver<T> {
        self.rx.clone().unwrap_or_else(never)
    }

    /// Whether a thread may still answer.
    pub(crate) fn is_open(&self) -> bool {
        self.rx.is_some()
    }

    /// Stop listening (the thread's work was dropped or replaced).
    pub(crate) fn close(&mut self) {
        self.rx = None;
    }

    /// The outcome of a receive on [`Feed::rx`]. A disconnected channel
    /// closes the feed.
    pub(crate) fn take(&mut self, got: Result<T, RecvError>) -> Fed<T> {
        match got {
            Ok(v) => Fed::Got(v),
            Err(_) => {
                self.rx = None;
                Fed::Gone
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    #[test]
    fn a_panic_is_delivered_as_a_message_and_a_result_as_itself() {
        let (tx, rx) = unbounded();
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let t = tx.clone();
        spawn_guarded(
            "w-panic",
            1 << 20,
            || -> u8 { panic!("boom") },
            move |r| {
                let _ = t.send(r);
            },
        )
        .unwrap();
        let t = tx.clone();
        spawn_guarded(
            "w-ok",
            1 << 20,
            || 7u8,
            move |r| {
                let _ = t.send(r);
            },
        )
        .unwrap();
        let mut got: Vec<Result<u8, String>> = (0..2).map(|_| rx.recv().unwrap()).collect();
        std::panic::set_hook(hook);
        got.sort();
        assert_eq!(got[0], Ok(7));
        let msg = got[1].clone().unwrap_err();
        assert!(msg.contains("w-panic") && msg.contains("boom"), "{msg}");
    }

    #[test]
    fn a_feed_reports_a_gone_thread_once_and_then_stays_quiet() {
        let (tx, rx) = unbounded::<u8>();
        let mut feed = Feed::open(rx);
        tx.send(3).unwrap();
        assert_eq!(feed.take(feed.rx().recv()), Fed::Got(3));
        drop(tx);
        assert!(feed.is_open());
        assert_eq!(feed.take(feed.rx().recv()), Fed::Gone);
        assert!(!feed.is_open());
        // A closed feed never fires.
        assert!(feed.rx().try_recv().is_err());
        assert_eq!(
            feed.rx().recv_timeout(std::time::Duration::from_millis(20)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout)
        );
    }
}
