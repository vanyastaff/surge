//! Surge persistence layer for storing task state and metrics.
//!
//! This crate provides the persistence interface and implementations for
//! storing and retrieving task state, execution metrics, and other data
//! that needs to be persisted across Surge runs.

#![warn(missing_docs)]
#![warn(clippy::all)]

pub use error::{PersistenceError, Result};

/// Event aggregator for token usage
pub mod aggregator;

/// Budget tracking and alerting
pub mod budget;

/// Data models for token usage tracking
pub mod models;

/// Project memory and knowledge base
pub mod memory;

/// Pricing models for AI providers
pub mod pricing;

/// SQLite-based storage implementation
pub mod store;

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
