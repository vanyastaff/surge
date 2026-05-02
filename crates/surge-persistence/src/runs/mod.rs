//! Per-run SQLite event store and registry for the vibe-flow architecture.
//!
//! This module is the M2 milestone of the surge-persistence layer. It lives
//! alongside the existing legacy persistence (aggregator/budget/memory/
//! pricing/store) and does not interact with it.
//!
//! See `docs/superpowers/specs/2026-05-02-surge-persistence-m2-design.md`
//! for the full design.

pub mod clock;
pub mod error;
pub mod migrations;
pub mod pragmas;
pub mod process;
pub mod reader;
pub mod registry;
pub mod seq;
pub mod types;

pub use clock::{Clock, MockClock, SystemClock};
pub use error::{CloseError, OpenError, StorageError, WriterError};
pub use reader::{ReadEvent, RunReader};
pub use registry::{RunFilter, RunSummary};
pub use seq::EventSeq;
pub use types::{ArtifactRecord, CostSummary, PendingApproval, StageExecution};
