//! Project memory and knowledge base subsystem.
//!
//! This module provides persistent storage for project knowledge accumulated
//! across tasks, including architectural decisions, coding patterns, known
//! gotchas, and file-level context.

/// Data models for memory entries
pub mod models;

/// Database schema for memory storage
pub mod schema;

/// SQLite-based memory store
pub mod store;

pub use models::{Discovery, FileContext, Gotcha, Pattern};
pub use store::MemoryStore;
