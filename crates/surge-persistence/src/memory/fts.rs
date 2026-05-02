//! Full-text search functionality for memory store using SQLite FTS5.

use crate::memory::models::{Discovery, FileContext, Gotcha, Pattern};
use serde::{Deserialize, Serialize};

// ── Search Result Types ─────────────────────────────────────────────

/// Search results across all memory categories.
///
/// Aggregates search hits from discoveries, patterns, gotchas, and file contexts,
/// ordered by FTS5 relevance ranking (BM25 algorithm).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResults {
    /// Matching discoveries.
    pub discoveries: Vec<Discovery>,

    /// Matching patterns.
    pub patterns: Vec<Pattern>,

    /// Matching gotchas.
    pub gotchas: Vec<Gotcha>,

    /// Matching file contexts.
    pub file_contexts: Vec<FileContext>,
}

impl SearchResults {
    /// Create empty search results.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            discoveries: Vec::new(),
            patterns: Vec::new(),
            gotchas: Vec::new(),
            file_contexts: Vec::new(),
        }
    }

    /// Get total number of results across all categories.
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.discoveries.len() + self.patterns.len() + self.gotchas.len() + self.file_contexts.len()
    }

    /// Check if there are any results.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_count() == 0
    }
}

/// Category-specific search results.
///
/// Used for filtering searches to a specific memory category.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CategorySearchResults {
    /// Discovery results.
    Discoveries(Vec<Discovery>),

    /// Pattern results.
    Patterns(Vec<Pattern>),

    /// Gotcha results.
    Gotchas(Vec<Gotcha>),

    /// File context results.
    FileContexts(Vec<FileContext>),
}

impl CategorySearchResults {
    /// Get the count of results.
    #[must_use]
    pub fn count(&self) -> usize {
        match self {
            Self::Discoveries(items) => items.len(),
            Self::Patterns(items) => items.len(),
            Self::Gotchas(items) => items.len(),
            Self::FileContexts(items) => items.len(),
        }
    }

    /// Check if there are any results.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }
}

/// Memory category for filtering searches.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryCategory {
    /// Architectural decisions and reasoning.
    Discoveries,

    /// Coding patterns and conventions.
    Patterns,

    /// Known pitfalls and errors.
    Gotchas,

    /// File-level context and APIs.
    FileContexts,
}

impl MemoryCategory {
    /// Get the category name as a string.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Discoveries => "discoveries",
            Self::Patterns => "patterns",
            Self::Gotchas => "gotchas",
            Self::FileContexts => "file_contexts",
        }
    }
}
