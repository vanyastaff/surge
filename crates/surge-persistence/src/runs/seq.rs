//! Strongly-typed event sequence number.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Monotonic, gap-free event sequence number assigned by SQLite ROWID alias.
///
/// Always starts at 1 for the first appended event; `EventSeq::ZERO` is the
/// sentinel for "before any event was written" used as the initial cursor in
/// subscribe streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventSeq(pub u64);

impl EventSeq {
    /// Sentinel for "before any event was written".
    pub const ZERO: EventSeq = EventSeq(0);

    /// Returns the next sequence number (`self + 1`).
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Returns the underlying u64 value.
    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EventSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<u64> for EventSeq {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<i64> for EventSeq {
    fn from(v: i64) -> Self {
        Self(v as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_works_as_u64() {
        let a = EventSeq(1);
        let b = EventSeq(2);
        assert!(a < b);
        assert_eq!(a.next(), b);
    }

    #[test]
    fn zero_is_initial() {
        assert_eq!(EventSeq::ZERO.as_u64(), 0);
    }
}
