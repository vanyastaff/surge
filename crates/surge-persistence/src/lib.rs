//! Surge persistence layer for storing task state and metrics.
//!
//! This crate provides the persistence interface and implementations for
//! storing and retrieving task state, execution metrics, and other data
//! that needs to be persisted across Surge runs.

#![warn(missing_docs)]
#![warn(clippy::all)]

pub use error::{PersistenceError, Result};

/// Data models for token usage tracking
pub mod models;

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

        /// Storage error
        #[error("Storage error: {0}")]
        Storage(String),
    }

    /// Result type for persistence operations
    pub type Result<T> = std::result::Result<T, PersistenceError>;
}
