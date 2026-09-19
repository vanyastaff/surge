//! Queue scheduling — pure ordering policy for the project task queue.
//!
//! The daemon's scheduler mirrors `.surge/roadmap.toml` into registry rows and
//! asks [`QueuePolicy::next`] what to dispatch. The policy itself is pure: it
//! takes the rows the caller joined (including each dependency's state) and
//! returns a decision. No I/O, no clock, no database — which is what makes the
//! ordering testable as a set of laws rather than through a running daemon.

pub mod policy;

pub use policy::{Blocked, EffectivePriority, QueueDecision, QueueEntry, QueuePolicy};
