//! Per-run SQLite event store and registry for the Surge architecture.
//!
//! This module is the M2 milestone of the surge-persistence layer. It lives
//! alongside the existing legacy persistence (aggregator/budget/memory/
//! pricing/store) and does not interact with it.
//!
//! See `docs/ARCHITECTURE.md`
//! for the full design.

#![warn(clippy::pedantic)]
#![allow(
    // Errors are documented at the type level (StorageError variants).
    clippy::missing_errors_doc,
    // Panics are documented at type level; some helpers use `expect` only on
    // invariant violations the caller cannot trigger.
    clippy::missing_panics_doc,
    // Re-export names match the underlying type — intentional.
    clippy::module_name_repetitions,
    // SQLite naturally returns i64; storage transforms to/from u32/u64/usize
    // are bounded by the schema and acceptable.
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::cast_possible_wrap,
    // Async APIs intentionally take owned handles for ergonomics.
    clippy::needless_pass_by_value,
    // RunId is Copy; pre-existing `.clone()` calls are noise but harmless.
    // Replacing them en masse touches many lines; keep unchanged.
    clippy::clone_on_copy,
    // Doc strings sometimes refer to plain words like SQLite or NULL where
    // backticks add noise without value.
    clippy::doc_markdown,
    // Some Storage / writer methods are necessarily long-bodied state machines.
    clippy::too_many_lines,
    // Public Storage / writer surface methods are intentionally `async fn` for
    // API consistency even when their current bodies don't await — leaves room
    // to evolve them into truly-async impls without an SemVer break.
    clippy::unused_async,
    // Block nesting in async match arms / writer task is acceptable.
    clippy::excessive_nesting,
    // views.rs exhaustive match + wildcard fallback: both arms are intentional.
    clippy::match_same_arms,
)]

pub mod clock;
pub mod config;
pub mod error;
pub(crate) mod file_lock;
pub mod inbox_queue;
mod macros;
pub mod migrations;
pub mod pragmas;
pub mod process;
pub mod query;
pub mod reader;
mod reader_views;
pub mod registry;
pub mod run_writer;
pub mod seq;
pub mod storage;
pub mod subscribe;
pub mod types;
pub mod views;
pub mod writer;
pub(crate) mod writer_slot;

pub use clock::{Clock, MockClock, SystemClock};
pub use config::StorageConfig;
pub use error::{CloseError, OpenError, StorageError, WriterError};
pub use query::{RunStatusSnapshot, aggregate_status, current_status};
pub use reader::{ReadEvent, RunReader};
pub use registry::{RunFilter, RunSummary};
pub use run_writer::RunWriter;
pub use seq::EventSeq;
pub use storage::{ActiveRunRow, Storage};
pub use subscribe::SUBSCRIBE_BATCH_MAX;
pub use types::{ArtifactRecord, CostSummary, PendingApproval, RoadmapPatchRecord, StageExecution};
pub use writer::{DEFAULT_CHANNEL_CAPACITY, WriterCommand, WriterConfig};
