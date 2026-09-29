//! Full-text search functionality for memory store using SQLite FTS5.

use crate::memory::models::{Discovery, FileContext, Gotcha, Pattern};
use serde::{Deserialize, Serialize};

// ── Search filter ───────────────────────────────────────────────────

/// Narrows a full-text search by spec and tags **inside the query**, so the
/// `limit` counts matching rows rather than rows fetched before filtering.
///
/// `tags` is an OR: a row matches when it carries any of them. An empty filter
/// matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchFilter {
    /// Bound as the row's `spec_id` column (`None` keeps every row).
    pub(crate) spec_id: Option<String>,
    /// JSON array bound to `json_each` (`None` when no tag filter is set).
    pub(crate) tags_json: Option<String>,
}

impl SearchFilter {
    /// A filter that keeps everything.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Keep only rows recorded for `spec_id` and, when `tags` is non-empty,
    /// carrying at least one of `tags`.
    #[must_use]
    pub fn new(spec_id: Option<&surge_core::SpecId>, tags: &[String]) -> Self {
        Self {
            spec_id: spec_id.map(ToString::to_string),
            tags_json: (!tags.is_empty())
                .then(|| serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_owned())),
        }
    }
}

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
