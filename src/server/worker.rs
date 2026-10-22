//! Background threads and the channels that bring their answers back.
//!
//! One execution mode for the server's jobs ([`Spawn`]): on threads of their
//! own, or inline when the server is embedded in a host without threads. One
//! way to start a thread ([`spawn_guarded`]): a panic in it is caught and
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

/// How the server's jobs run. The server holds one and hands it to each job.
/// Each job decides what running inline means for it: imports resolve
/// without loading, SHACL validation runs with a time limit of its own,
/// workspace diagnostics are off, a query run shows its summary as a
/// message, reindexing parses at once, and JSON-LD contexts get no
/// background loader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Spawn {
    /// On threads of their own (the stdio server).
    Thread,
    /// Here and now, on the caller's thread: the host has no threads (the
    /// browser build, through `server::Embedded`).
    Inline,
}

impl Spawn {
    /// Run `work`, then pass what it returned to `deliver`: `Err` with the
    /// panic's message when `work` panicked. [`Spawn::Thread`] runs both on a
    /// new thread named `name`, with a stack of `stack` bytes (see
    /// [`spawn_guarded`]); [`Spawn::Inline`] runs both here, before this
    /// returns. Fails only when a thread cannot be started.
    pub(crate) fn run<T, W, D>(
        self,
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
        match self {
            Spawn::Thread => {
                #[cfg(test)]
                if FAIL_STARTS.with(std::cell::Cell::get) {
                    return Err(std::io::Error::other("thread start refused by a test"));
                }
                spawn_guarded(name, stack, work, deliver)
            }
            Spawn::Inline => {
                deliver(catch(name, work));
                Ok(())
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: every [`Spawn::Thread`] start on this thread fails.
    static FAIL_STARTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Tests: run `f` with every [`Spawn::Thread`] start on this thread failing,
/// as when the system has no thread to spare.
#[cfg(test)]
pub(crate) fn failing_starts<T>(f: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAIL_STARTS.with(|c| c.set(false));
        }
    }
    FAIL_STARTS.with(|c| c.set(true));
    let _reset = Reset;
    f()
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
    fn inline_runs_the_job_before_it_returns_and_delivers_its_answer() {
        let (tx, rx) = unbounded();
        let t = tx.clone();
        let here = std::thread::current().id();
        Spawn::Inline
            .run(
                "w-inline",
                1 << 20,
                move || (7u8, std::thread::current().id()),
                move |r| {
                    let _ = t.send(r);
                },
            )
            .unwrap();
        // Already there: nothing to wait for.
        assert_eq!(rx.try_recv(), Ok(Ok((7, here))));
        // A panic is delivered as a message, inline too.
        let quiet = crate::workspace::tests::QuietPanics::new();
        Spawn::Inline
            .run(
                "w-inline-panic",
                1 << 20,
                || -> (u8, std::thread::ThreadId) { panic!("boom") },
                move |r| {
                    let _ = tx.send(r);
                },
            )
            .unwrap();
        drop(quiet);
        let msg = rx.try_recv().unwrap().unwrap_err();
        assert!(
            msg.contains("w-inline-panic") && msg.contains("boom"),
            "{msg}"
        );
    }

    #[test]
    fn a_thread_delivers_a_panic_as_a_message_and_a_result_as_itself() {
        let (tx, rx) = unbounded();
        let quiet = crate::workspace::tests::QuietPanics::new();
        let here = std::thread::current().id();
        for (name, fail) in [("w-thread-panic", true), ("w-thread-ok", false)] {
            let t = tx.clone();
            Spawn::Thread
                .run(
                    name,
                    1 << 20,
                    move || {
                        assert!(!fail, "boom");
                        std::thread::current().id()
                    },
                    move |r| {
                        let _ = t.send((fail, r));
                    },
                )
                .unwrap();
        }
        let mut got: Vec<_> = (0..2)
            .map(|_| rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap())
            .collect();
        drop(quiet);
        got.sort_by_key(|(fail, _)| *fail);
        let ran_on = got[0].1.clone().unwrap();
        assert_ne!(ran_on, here, "it ran on a thread of its own");
        let msg = got[1].1.clone().unwrap_err();
        assert!(
            msg.contains("w-thread-panic") && msg.contains("boom"),
            "{msg}"
        );
    }

    #[test]
    fn a_thread_that_cannot_start_runs_nothing_and_says_so() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (work_ran, delivered) = (ran.clone(), ran.clone());
        let started = failing_starts(|| {
            Spawn::Thread.run(
                "w-refused",
                1 << 20,
                move || work_ran.store(true, std::sync::atomic::Ordering::Relaxed),
                move |_| delivered.store(true, std::sync::atomic::Ordering::Relaxed),
            )
        });
        assert!(started.is_err());
        assert!(!ran.load(std::sync::atomic::Ordering::Relaxed));
        // Only while the test asks for it; inline runs are not threads.
        assert!(Spawn::Thread.run("w-ok", 1 << 20, || (), |_| {}).is_ok());
        assert!(failing_starts(|| Spawn::Inline.run("w-inline", 1 << 20, || (), |_| {})).is_ok());
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
