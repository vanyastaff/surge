//! Project-memory search shared by the operator surfaces.

use surge_core::SpecId;
use surge_persistence::memory::{MemoryStore, SearchResults};

use crate::operator::error::OperatorError;

/// FTS5 search over `store`, filtered by spec and tags.
///
/// A query FTS5 cannot parse is retried once as a quoted exact phrase; if that
/// also fails, the original error is returned.
///
/// # Errors
/// Returns [`OperatorError::MemoryStore`] if the store query fails.
pub fn query_memory(
    store: &MemoryStore,
    query: &str,
    spec_id: Option<&SpecId>,
    tags_filter: &[String],
    limit: usize,
) -> Result<SearchResults, OperatorError> {
    let mut results = match store.search_all(query, Some(limit)) {
        Ok(results) => results,
        Err(first_error) => store
            .search_all(&format!("\"{query}\""), Some(limit))
            .map_err(|_| OperatorError::MemoryStore(first_error))?,
    };

    if let Some(sid) = spec_id {
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
