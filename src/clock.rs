//! The time the server's timers read: the system's, or in the browser
//! build, which has no clock of its own, the page's (`env.now_ms`, which the
//! page supplies as `Date.now()`; see web/engine).

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

#[cfg(target_arch = "wasm32")]
pub use page::Instant;

#[cfg(target_arch = "wasm32")]
mod page {
    use std::time::Duration;

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        fn now_ms() -> f64;
    }

    /// Milliseconds since 1970, from the page.
    #[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
    pub struct Instant(f64);

    impl Instant {
        pub fn now() -> Instant {
            Instant(unsafe { now_ms() })
        }

        pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
            Duration::from_secs_f64(((self.0 - earlier.0) / 1000.0).max(0.0))
        }
    }

    impl std::ops::Add<Duration> for Instant {
        type Output = Instant;
        fn add(self, d: Duration) -> Instant {
            Instant(self.0 + d.as_secs_f64() * 1000.0)
        }
    }
}
