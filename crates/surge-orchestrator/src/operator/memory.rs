//! Project-memory search shared by the operator surfaces.

use std::path::Path;

use surge_core::SpecId;
use surge_persistence::memory::{MemoryStore, SearchResults};

use crate::operator::error::OperatorError;

/// A project-memory search.
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    /// FTS5 query text.
    pub text: String,
    /// Keep only entries recorded for this spec.
    pub spec_id: Option<SpecId>,
    /// Keep only entries carrying at least one of these tags (OR); empty keeps
    /// every entry.
    pub tags: Vec<String>,
    /// Maximum results fetched from the index, per category.
    pub limit: usize,
}

/// FTS5 search over the project-memory store at `store_path`.
///
/// The index is searched first (`limit` per category); the `spec_id` filter
/// applies next, then the `tags` filter, both to the fetched rows — so a
/// filtered search can return fewer than `limit` results. A tag filter is an
/// OR: an entry matches when it carries any of the tags.
///
/// A query FTS5 cannot parse is retried once as a quoted exact phrase (an
/// embedded `"` doubled, FTS5's escape); if that also fails, the original
/// error is returned.
///
/// Blocking (SQLite): call from `spawn_blocking` in async code.
///
/// # Errors
/// Returns [`OperatorError::MemoryStore`] if the store cannot be opened or the
/// query fails.
pub fn query_memory(
    store_path: &Path,
    query: &MemoryQuery,
) -> Result<SearchResults, OperatorError> {
    let store = MemoryStore::open(store_path).map_err(OperatorError::MemoryStore)?;
    let mut results = match store.search_all(&query.text, Some(query.limit)) {
        Ok(results) => results,
        Err(first_error) => store
            .search_all(&quoted_phrase(&query.text), Some(query.limit))
            .map_err(|_| OperatorError::MemoryStore(first_error))?,
    };
    let tags_filter = query.tags.as_slice();

    if let Some(sid) = query.spec_id.as_ref() {
        results
            .discoveries
            .retain(|d| d.spec_id.as_ref() == Some(sid));
        results.patterns.retain(|p| p.spec_id.as_ref() == Some(sid));
        results.gotchas.retain(|g| g.spec_id.as_ref() == Some(sid));
        results
            .file_contexts
            .retain(|f| f.spec_id.as_ref() == Some(sid));
    }

    if !tags_filter.is_empty() {
        results
            .discoveries
            .retain(|d| tags_filter.iter().any(|tag| d.tags.contains(tag)));
        results
            .patterns
            .retain(|p| tags_filter.iter().any(|tag| p.tags.contains(tag)));
        results
            .gotchas
            .retain(|g| tags_filter.iter().any(|tag| g.tags.contains(tag)));
        results
            .file_contexts
            .retain(|f| tags_filter.iter().any(|tag| f.tags.contains(tag)));
    }
    Ok(results)
}

/// `text` as one FTS5 phrase: wrapped in quotes, embedded quotes doubled.
fn quoted_phrase(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::quoted_phrase;

    #[test]
    fn quoted_phrase_doubles_embedded_quotes() {
        assert_eq!(quoted_phrase("a b"), "\"a b\"");
        assert_eq!(quoted_phrase("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
