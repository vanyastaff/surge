//! Surge persistence layer for storing task state and metrics.
//!
//! This crate provides the persistence interface and implementations for
//! storing and retrieving task state, execution metrics, and other data
//! that needs to be persisted across Surge runs.

#![warn(missing_docs)]
#![warn(clippy::all)]
// Pre-existing legacy code; M5 does not modify the legacy modules.
// These allows suppress pedantic lints that fire when clippy::pedantic is
// requested transitively by surge-orchestrator.
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::explicit_iter_loop)]
#![allow(clippy::if_not_else)]
#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::implicit_clone)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::missing_fields_in_debug)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::redundant_else)]
#![allow(clippy::single_match_else)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::unnested_or_patterns)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::unused_async)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::needless_raw_string_hashes)]
#![allow(clippy::bool_to_int_with_if)]
#![allow(clippy::unused_self)]

pub use error::{PersistenceError, Result};

/// Event aggregator for token usage
pub mod aggregator;

/// Budget tracking and alerting
pub mod budget;

/// Storage layer for external ticket intake
pub mod intake;

/// Data models for token usage tracking
pub mod models;

/// Project memory and knowledge base
pub mod memory;

/// Pricing models for AI providers
pub mod pricing;

/// SQLite-based storage implementation
pub mod store;

/// New M2 Surge storage layer (per-run event log, registry, worktree integration).
pub mod runs;

pub use runs::inbox_queue;

/// Persistence error types
pub mod error {
    use thiserror::Error;

    /// Errors that can occur during persistence operations
    #[derive(Debug, Error)]
    pub enum PersistenceError {
        /// I/O error
        #[error("I/O error: {0}")]
        Io(#[from] std::io::Error),

        /// Serialization error
        #[error("Serialization error: {0}")]
        Serialization(#[from] serde_json::Error),

        /// Database error
        #[error("Database error: {0}")]
        Database(#[from] rusqlite::Error),

        /// Storage error
        #[error("Storage error: {0}")]
        Storage(String),
    }

    /// Result type for persistence operations
    pub type Result<T> = std::result::Result<T, PersistenceError>;
}
