//! Project-memory search shared by the operator surfaces.

use std::path::Path;

use surge_core::SpecId;
use surge_persistence::memory::{MemoryStore, SearchFilter, SearchResults};

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
    /// Maximum matching results, per category.
    pub limit: usize,
}

/// FTS5 search over the project-memory store at `store_path`.
///
/// The `spec_id` and `tags` filters are part of the index query, so `limit`
/// (per category) counts matching rows: a filtered search returns up to `limit`
/// matches, not whatever was left of the top `limit` unfiltered hits. A tag
/// filter is an OR: an entry matches when it carries any of the tags.
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
    let filter = SearchFilter::new(query.spec_id.as_ref(), &query.tags);
    let results = match store.search_all_filtered(&query.text, &filter, Some(query.limit)) {
        Ok(results) => results,
        Err(first_error) => store
            .search_all_filtered(&quoted_phrase(&query.text), &filter, Some(query.limit))
            .map_err(|_| OperatorError::MemoryStore(first_error))?,
    };
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
