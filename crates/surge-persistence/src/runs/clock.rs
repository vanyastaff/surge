//! Test-injectable wall clock.
//!
//! Production code uses [`SystemClock`]. Tests use [`MockClock`] for
//! deterministic timestamps in snapshot tests and reproducible event logs.
//!
//! This is intentionally a small infrastructure utility — not the kind of
//! storage-backend trait abstraction that the spec's "no traits for mocking"
//! guidance argues against.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// Wall-clock abstraction with deterministic test impl.
pub trait Clock: Send + Sync + 'static {
    /// Current time as Unix epoch milliseconds.
    fn now_ms(&self) -> i64;
}

/// Production clock backed by `chrono::Utc::now()`.
#[derive(Debug, Default, Clone)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

/// Test clock with explicit time control.
#[derive(Debug, Clone)]
pub struct MockClock {
    inner: Arc<AtomicI64>,
}

impl MockClock {
    /// Creates a new mock clock at the given epoch-ms.
    #[must_use]
    pub fn new(initial_ms: i64) -> Self {
        Self {
            inner: Arc::new(AtomicI64::new(initial_ms)),
        }
    }

    /// Advances the clock by the given number of milliseconds.
    pub fn advance(&self, by_ms: i64) {
        self.inner.fetch_add(by_ms, Ordering::SeqCst);
    }

    /// Sets the clock to the given epoch-ms (replacing whatever was there).
    pub fn set(&self, ms: i64) {
        self.inner.store(ms, Ordering::SeqCst);
    }
}

impl Clock for MockClock {
    fn now_ms(&self) -> i64 {
        self.inner.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_clock_advances() {
        let c = MockClock::new(1_700_000_000_000);
        assert_eq!(c.now_ms(), 1_700_000_000_000);
        c.advance(50);
        assert_eq!(c.now_ms(), 1_700_000_000_050);
    }

    #[test]
    fn system_clock_returns_recent_time() {
        let c = SystemClock;
        let now = c.now_ms();
        assert!(now > 1_700_000_000_000, "clock should be after 2023");
    }
}
