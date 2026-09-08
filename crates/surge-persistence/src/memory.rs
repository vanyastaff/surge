//! Project memory and knowledge base subsystem.
//!
//! This module provides persistent storage for project knowledge accumulated
//! across tasks, including architectural decisions, coding patterns, known
//! gotchas, and file-level context.

/// Read-only claim audit: staleness and failed-run correlation
pub mod audit;

/// Data models for memory entries
pub mod models;

/// Database schema for memory storage
pub mod schema;

/// SQLite-based memory store
pub mod store;

/// Full-text search functionality
pub mod fts;

pub use audit::{
    AuditReport, RunCorrelatedClaim, StaleClaim, StaleReason, UnverifiableClaim, run_audit,
};
pub use fts::{CategorySearchResults, MemoryCategory, SearchResults};
pub use models::{Discovery, FileContext, Gotcha, Pattern};
pub use store::MemoryStore;
