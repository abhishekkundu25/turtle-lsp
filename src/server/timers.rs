//! The server's one-shot timers, keyed by what they are for. The main loop
//! waits for the earliest; an embedded host asks when that is
//! ([`Timers::next`]) and fires what is due ([`Timers::take_due`]).

use crate::clock::Instant;

/// What a timer is waiting to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Timer {
    /// The described set changed and typing may have paused: refresh the
    /// other documents.
    Described,
    /// Typing paused: validate the workspace (`shacl.onType`).
    ShaclTyping,
    /// A SHACL run has lasted long enough to show progress.
    ShaclProgress,
}

/// Every kind of timer, in the order due ones fire.
const ALL: [Timer; 3] = [Timer::Described, Timer::ShaclTyping, Timer::ShaclProgress];

impl Timer {
    fn slot(self) -> usize {
        ALL.iter().position(|t| *t == self).unwrap_or_default()
    }
}

#[derive(Default)]
pub(crate) struct Timers {
    due: [Option<Instant>; ALL.len()],
}

impl Timers {
    /// Fire `timer` at `at`, replacing a pending one of the same kind.
    pub(crate) fn set(&mut self, timer: Timer, at: Instant) {
        self.due[timer.slot()] = Some(at);
    }

    pub(crate) fn clear(&mut self, timer: Timer) {
        self.due[timer.slot()] = None;
    }

    pub(crate) fn is_set(&self, timer: Timer) -> bool {
        self.due[timer.slot()].is_some()
    }

    /// The earliest time any timer is due.
    pub(crate) fn next(&self) -> Option<Instant> {
        self.due
            .iter()
            .flatten()
            .copied()
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// The timers due by `now`, cleared, in firing order.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<Timer> {
        ALL.into_iter()
            .filter(|t| {
                let slot = &mut self.due[t.slot()];
                let fire = slot.is_some_and(|at| at <= now);
                if fire {
                    *slot = None;
                }
                fire
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timers_are_keyed_replaced_and_taken_once_when_due() {
        let now = Instant::now();
        let mut t = Timers::default();
        assert!(t.next().is_none() && t.take_due(now).is_empty());
        t.set(Timer::ShaclTyping, now + Duration::from_secs(5));
        t.set(Timer::Described, now + Duration::from_secs(1));
        assert_eq!(t.next(), Some(now + Duration::from_secs(1)));
        // The same kind replaces itself.
        t.set(Timer::ShaclTyping, now + Duration::from_secs(2));
        assert!(t.is_set(Timer::ShaclTyping) && !t.is_set(Timer::ShaclProgress));
        assert!(t.take_due(now).is_empty());
        assert_eq!(
            t.take_due(now + Duration::from_secs(3)),
            [Timer::Described, Timer::ShaclTyping]
        );
        assert!(t.next().is_none());
        t.set(Timer::ShaclProgress, now);
        t.clear(Timer::ShaclProgress);
        assert!(t.take_due(now + Duration::from_secs(9)).is_empty());
    }
}
