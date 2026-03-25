//! SQLite-based storage for project memory and knowledge base.

use crate::memory::models::{Discovery, FileContext, Gotcha, Pattern};
use crate::memory::schema::{SCHEMA_DDL, SCHEMA_VERSION};
use crate::{PersistenceError, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};

// ── Store ───────────────────────────────────────────────────────────

/// SQLite-based storage for project memory and knowledge base.
///
/// Provides persistent storage for discoveries, patterns, gotchas, and file
/// contexts using SQLite with FTS5 full-text search. Handles schema creation,
/// migrations, and CRUD operations.
pub struct MemoryStore {
    conn: Connection,
    #[allow(dead_code)]
    path: PathBuf,
}

impl MemoryStore {
    /// Open or create a memory store at the given path.
    ///
    /// Creates the database file and initializes the schema if it doesn't exist.
    /// If the database exists, verifies the schema version.
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let mut store = Self {
            conn,
            path: path.to_path_buf(),
        };

        store.initialize_schema()?;
        Ok(store)
    }

    /// Create an in-memory store (for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self {
            conn,
            path: PathBuf::from(":memory:"),
        };

        store.initialize_schema()?;
        Ok(store)
    }

    /// Get the path to the default store location (~/.surge/memory.db).
    pub fn default_path() -> Result<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| PersistenceError::Storage("Cannot determine home directory".into()))?;
        Ok(home.join(".surge").join("memory.db"))
    }

    /// Initialize or verify the database schema.
    fn initialize_schema(&mut self) -> Result<()> {
        // Create schema version table first if it doesn't exist
        self.conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            )
            "#,
            [],
        )?;

        // Check current schema version
        let current_version: Option<i32> = self
            .conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(version) = current_version {
            if version > SCHEMA_VERSION {
                return Err(PersistenceError::Storage(format!(
                    "Database schema version {version} is newer than supported version {SCHEMA_VERSION}"
                )));
            }
            // Future: handle migrations here if version < SCHEMA_VERSION
        } else {
            // Initialize new database - execute all DDL statements
            for ddl in SCHEMA_DDL {
                self.conn.execute(ddl, [])?;
            }

            self.conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [SCHEMA_VERSION],
            )?;
        }

        Ok(())
    }

    // ── Discovery Operations ────────────────────────────────────────────

    /// Add a new discovery to the store.
    ///
    /// Inserts the discovery into the database. The FTS5 triggers will
    /// automatically index the content for full-text search.
    pub fn add_discovery(&self, discovery: &Discovery) -> Result<()> {
        let tags_json = serde_json::to_string(&discovery.tags)?;

        self.conn.execute(
            r#"
            INSERT INTO discoveries (
                id, title, content, task_id, spec_id, category, tags,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            rusqlite::params![
                discovery.id,
                discovery.title,
                discovery.content,
                discovery.task_id.as_ref().map(|id| id.to_string()),
                discovery.spec_id.as_ref().map(|id| id.to_string()),
                discovery.category,
                tags_json,
                discovery.created_at as i64,
                discovery.updated_at as i64,
            ],
        )?;

        Ok(())
    }

    // ── Pattern Operations ──────────────────────────────────────────────

    /// Add a new pattern to the store.
    ///
    /// Inserts the pattern into the database. The FTS5 triggers will
    /// automatically index the content for full-text search.
    pub fn add_pattern(&self, pattern: &Pattern) -> Result<()> {
        let tags_json = serde_json::to_string(&pattern.tags)?;

        self.conn.execute(
            r#"
            INSERT INTO patterns (
                id, name, description, example, task_id, spec_id,
                language, category, tags, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            rusqlite::params![
                pattern.id,
                pattern.name,
                pattern.description,
                pattern.example,
                pattern.task_id.as_ref().map(|id| id.to_string()),
                pattern.spec_id.as_ref().map(|id| id.to_string()),
                pattern.language,
                pattern.category,
                tags_json,
                pattern.created_at as i64,
                pattern.updated_at as i64,
            ],
        )?;

        Ok(())
    }

    // ── Gotcha Operations ───────────────────────────────────────────────

    /// Add a new gotcha to the store.
    ///
    /// Inserts the gotcha into the database. The FTS5 triggers will
    /// automatically index the content for full-text search.
    pub fn add_gotcha(&self, gotcha: &Gotcha) -> Result<()> {
        let tags_json = serde_json::to_string(&gotcha.tags)?;

        self.conn.execute(
            r#"
            INSERT INTO gotchas (
                id, title, description, symptom, solution, task_id, spec_id,
                severity, category, tags, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            rusqlite::params![
                gotcha.id,
                gotcha.title,
                gotcha.description,
                gotcha.symptom,
                gotcha.solution,
                gotcha.task_id.as_ref().map(|id| id.to_string()),
                gotcha.spec_id.as_ref().map(|id| id.to_string()),
                gotcha.severity,
                gotcha.category,
                tags_json,
                gotcha.created_at as i64,
                gotcha.updated_at as i64,
            ],
        )?;

        Ok(())
    }

    // ── File Context Operations ─────────────────────────────────────────

    /// Add a new file context to the store.
    ///
    /// Inserts the file context into the database. The FTS5 triggers will
    /// automatically index the content for full-text search.
    pub fn add_file_context(&self, context: &FileContext) -> Result<()> {
        let key_apis_json = serde_json::to_string(&context.key_apis)?;
        let dependencies_json = serde_json::to_string(&context.dependencies)?;
        let tags_json = serde_json::to_string(&context.tags)?;

        self.conn.execute(
            r#"
            INSERT INTO file_contexts (
                id, file_path, summary, key_apis, description, dependencies,
                task_id, spec_id, language, module_category, tags,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
            rusqlite::params![
                context.id,
                context.file_path,
                context.summary,
                key_apis_json,
                context.description,
                dependencies_json,
                context.task_id.as_ref().map(|id| id.to_string()),
                context.spec_id.as_ref().map(|id| id.to_string()),
                context.language,
                context.module_category,
                tags_json,
                context.created_at as i64,
                context.updated_at as i64,
            ],
        )?;

        Ok(())
    }

    // ── Full-Text Search Operations ─────────────────────────────────────

    /// Search across all memory categories using FTS5 full-text search.
    ///
    /// Searches discoveries, patterns, gotchas, and file contexts using SQLite's
    /// FTS5 engine. Results are ranked by BM25 relevance. Returns up to `limit`
    /// results per category (default: 10).
    ///
    /// # Query Syntax
    ///
    /// FTS5 supports advanced query syntax:
    /// - `term1 term2` - both terms must appear (AND)
    /// - `term1 OR term2` - either term must appear
    /// - `"exact phrase"` - exact phrase match
    /// - `term*` - prefix match
    /// - `-term` - exclude term
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use surge_persistence::memory::MemoryStore;
    /// # let store = MemoryStore::in_memory()?;
    /// // Search for "async" or "error handling"
    /// let results = store.search_all("async OR \"error handling\"", Some(5))?;
    /// println!("Found {} total results", results.total_count());
    /// # Ok::<(), surge_persistence::PersistenceError>(())
    /// ```
    pub fn search_all(&self, query: &str, limit: Option<usize>) -> Result<crate::memory::fts::SearchResults> {
        use crate::memory::fts::SearchResults;

        let limit = limit.unwrap_or(10) as i64;

        // Search discoveries
        let discoveries = self.search_discoveries_fts(query, limit)?;

        // Search patterns
        let patterns = self.search_patterns_fts(query, limit)?;

        // Search gotchas
        let gotchas = self.search_gotchas_fts(query, limit)?;

        // Search file contexts
        let file_contexts = self.search_file_contexts_fts(query, limit)?;

        Ok(SearchResults {
            discoveries,
            patterns,
            gotchas,
            file_contexts,
        })
    }

    /// Search within a specific memory category.
    ///
    /// Performs FTS5 full-text search on a single category. More efficient than
    /// `search_all()` when you know which category you need.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use surge_persistence::memory::{MemoryStore, MemoryCategory};
    /// # let store = MemoryStore::in_memory()?;
    /// // Search only patterns for "error handling"
    /// let results = store.search_by_category(
    ///     MemoryCategory::Patterns,
    ///     "error handling",
    ///     Some(10),
    /// )?;
    /// # Ok::<(), surge_persistence::PersistenceError>(())
    /// ```
    pub fn search_by_category(
        &self,
        category: crate::memory::fts::MemoryCategory,
        query: &str,
        limit: Option<usize>,
    ) -> Result<crate::memory::fts::CategorySearchResults> {
        use crate::memory::fts::{CategorySearchResults, MemoryCategory};

        let limit = limit.unwrap_or(10) as i64;

        match category {
            MemoryCategory::Discoveries => {
                let results = self.search_discoveries_fts(query, limit)?;
                Ok(CategorySearchResults::Discoveries(results))
            }
            MemoryCategory::Patterns => {
                let results = self.search_patterns_fts(query, limit)?;
                Ok(CategorySearchResults::Patterns(results))
            }
            MemoryCategory::Gotchas => {
                let results = self.search_gotchas_fts(query, limit)?;
                Ok(CategorySearchResults::Gotchas(results))
            }
            MemoryCategory::FileContexts => {
                let results = self.search_file_contexts_fts(query, limit)?;
                Ok(CategorySearchResults::FileContexts(results))
            }
        }
    }

    // ── FTS5 Helper Methods ─────────────────────────────────────────────

    /// Search discoveries using FTS5.
    fn search_discoveries_fts(&self, query: &str, limit: i64) -> Result<Vec<Discovery>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT d.id, d.title, d.content, d.task_id, d.spec_id, d.category,
                   d.tags, d.created_at, d.updated_at
            FROM discoveries d
            INNER JOIN discoveries_fts fts ON d.rowid = fts.rowid
            WHERE discoveries_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            "#,
        )?;

        let discoveries = stmt
            .query_map(rusqlite::params![query, limit], |row| {
                let tags_json: String = row.get(6)?;
                // Note: unwrap_or_default is intentional here — we're inside a
                // rusqlite row callback that can only return rusqlite::Error, not
                // our PersistenceError. Corrupted JSON gracefully degrades to [].
                let tags: Vec<String> = serde_json::from_str(&tags_json)
                    .unwrap_or_default();

                Ok(Discovery {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    content: row.get(2)?,
                    task_id: row.get::<_, Option<String>>(3)?
                        .and_then(|s| s.parse().ok()),
                    spec_id: row.get::<_, Option<String>>(4)?
                        .and_then(|s| s.parse().ok()),
                    category: row.get(5)?,
                    tags,
                    created_at: row.get::<_, i64>(7)? as u64,
                    updated_at: row.get::<_, i64>(8)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(discoveries)
    }

    /// Search patterns using FTS5.
    fn search_patterns_fts(&self, query: &str, limit: i64) -> Result<Vec<Pattern>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT p.id, p.name, p.description, p.example, p.task_id, p.spec_id,
                   p.language, p.category, p.tags, p.created_at, p.updated_at
            FROM patterns p
            INNER JOIN patterns_fts fts ON p.rowid = fts.rowid
            WHERE patterns_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            "#,
        )?;

        let patterns = stmt
            .query_map(rusqlite::params![query, limit], |row| {
                let tags_json: String = row.get(8)?;
                let tags: Vec<String> = serde_json::from_str(&tags_json)
                    .unwrap_or_default();

                Ok(Pattern {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    example: row.get(3)?,
                    task_id: row.get::<_, Option<String>>(4)?
                        .and_then(|s| s.parse().ok()),
                    spec_id: row.get::<_, Option<String>>(5)?
                        .and_then(|s| s.parse().ok()),
                    language: row.get(6)?,
                    category: row.get(7)?,
                    tags,
                    created_at: row.get::<_, i64>(9)? as u64,
                    updated_at: row.get::<_, i64>(10)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(patterns)
    }

    /// Search gotchas using FTS5.
    fn search_gotchas_fts(&self, query: &str, limit: i64) -> Result<Vec<Gotcha>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT g.id, g.title, g.description, g.symptom, g.solution, g.task_id,
                   g.spec_id, g.severity, g.category, g.tags, g.created_at, g.updated_at
            FROM gotchas g
            INNER JOIN gotchas_fts fts ON g.rowid = fts.rowid
            WHERE gotchas_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            "#,
        )?;

        let gotchas = stmt
            .query_map(rusqlite::params![query, limit], |row| {
                let tags_json: String = row.get(9)?;
                let tags: Vec<String> = serde_json::from_str(&tags_json)
                    .unwrap_or_default();

                Ok(Gotcha {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    description: row.get(2)?,
                    symptom: row.get(3)?,
                    solution: row.get(4)?,
                    task_id: row.get::<_, Option<String>>(5)?
                        .and_then(|s| s.parse().ok()),
                    spec_id: row.get::<_, Option<String>>(6)?
                        .and_then(|s| s.parse().ok()),
                    severity: row.get(7)?,
                    category: row.get(8)?,
                    tags,
                    created_at: row.get::<_, i64>(10)? as u64,
                    updated_at: row.get::<_, i64>(11)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(gotchas)
    }

    /// Search file contexts using FTS5.
    fn search_file_contexts_fts(&self, query: &str, limit: i64) -> Result<Vec<FileContext>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT fc.id, fc.file_path, fc.summary, fc.key_apis, fc.description,
                   fc.dependencies, fc.task_id, fc.spec_id, fc.language,
                   fc.module_category, fc.tags, fc.created_at, fc.updated_at
            FROM file_contexts fc
            INNER JOIN file_contexts_fts fts ON fc.rowid = fts.rowid
            WHERE file_contexts_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            "#,
        )?;

        let contexts = stmt
            .query_map(rusqlite::params![query, limit], |row| {
                let key_apis_json: String = row.get(3)?;
                let key_apis: Vec<String> = serde_json::from_str(&key_apis_json)
                    .unwrap_or_default();

                let dependencies_json: String = row.get(5)?;
                let dependencies: Vec<String> = serde_json::from_str(&dependencies_json)
                    .unwrap_or_default();

                let tags_json: String = row.get(10)?;
                let tags: Vec<String> = serde_json::from_str(&tags_json)
                    .unwrap_or_default();

                Ok(FileContext {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    summary: row.get(2)?,
                    key_apis,
                    description: row.get(4)?,
                    dependencies,
                    task_id: row.get::<_, Option<String>>(6)?
                        .and_then(|s| s.parse().ok()),
                    spec_id: row.get::<_, Option<String>>(7)?
                        .and_then(|s| s.parse().ok()),
                    language: row.get(8)?,
                    module_category: row.get(9)?,
                    tags,
                    created_at: row.get::<_, i64>(11)? as u64,
                    updated_at: row.get::<_, i64>(12)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(contexts)
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_timestamp() -> u64 {
        1640000000000 // Fixed timestamp for testing
    }

    #[test]
    fn test_open_in_memory() {
        let store = MemoryStore::in_memory();
        assert!(store.is_ok());
    }

    #[test]
    fn test_add_discovery() {
        let store = MemoryStore::in_memory().unwrap();

        let discovery = Discovery::new(
            "Test Discovery".into(),
            "This is a test discovery about architecture".into(),
            test_timestamp(),
        )
        .with_category("architecture".into())
        .with_tags(vec!["test".into(), "architecture".into()]);

        let result = store.add_discovery(&discovery);
        assert!(result.is_ok());
    }

    #[test]
    fn test_add_pattern() {
        let store = MemoryStore::in_memory().unwrap();

        let pattern = Pattern::new(
            "Error Handling Pattern".into(),
            "Always use ? for error handling in library code".into(),
            test_timestamp(),
        )
        .with_example("fn foo() -> Result<()> { Ok(()) }".into())
        .with_language("rust".into())
        .with_category("error-handling".into())
        .with_tags(vec!["rust".into(), "errors".into()]);

        let result = store.add_pattern(&pattern);
        assert!(result.is_ok());
    }

    #[test]
    fn test_add_gotcha() {
        let store = MemoryStore::in_memory().unwrap();

        let gotcha = Gotcha::new(
            "Avoid unwrap in libraries".into(),
            "Using unwrap() can cause panics in library code".into(),
            "Use ? operator or explicit error handling instead".into(),
            test_timestamp(),
        )
        .with_symptom("thread 'main' panicked at...".into())
        .with_severity("high".into())
        .with_category("error-handling".into())
        .with_tags(vec!["rust".into(), "errors".into()]);

        let result = store.add_gotcha(&gotcha);
        assert!(result.is_ok());
    }

    #[test]
    fn test_add_file_context() {
        let store = MemoryStore::in_memory().unwrap();

        let context = FileContext::new(
            "crates/surge-core/src/lib.rs".into(),
            "Core types and traits for Surge".into(),
            test_timestamp(),
        )
        .with_key_apis(vec!["SpecId".into(), "TaskId".into(), "TaskState".into()])
        .with_description("Main entry point for surge-core crate".into())
        .with_dependencies(vec!["ulid".into(), "serde".into()])
        .with_language("rust".into())
        .with_module_category("core".into())
        .with_tags(vec!["core".into(), "types".into()]);

        let result = store.add_file_context(&context);
        assert!(result.is_ok());
    }

    #[test]
    fn test_schema_initialization() {
        let store = MemoryStore::in_memory().unwrap();

        // Verify schema version is set
        let version: i32 = store
            .conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn test_multiple_entries() {
        let store = MemoryStore::in_memory().unwrap();

        // Add multiple discoveries
        for i in 0..5 {
            let discovery = Discovery::new(
                format!("Discovery {i}"),
                format!("Content for discovery {i}"),
                test_timestamp(),
            );
            store.add_discovery(&discovery).unwrap();
        }

        // Add multiple patterns
        for i in 0..5 {
            let pattern = Pattern::new(
                format!("Pattern {i}"),
                format!("Description for pattern {i}"),
                test_timestamp(),
            );
            store.add_pattern(&pattern).unwrap();
        }

        // Verify entries were added by counting rows
        let discovery_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM discoveries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(discovery_count, 5);

        let pattern_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM patterns", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pattern_count, 5);
    }

    // ── FTS5 Search Tests ───────────────────────────────────────────────

    #[test]
    fn test_search_all_empty() {
        let store = MemoryStore::in_memory().unwrap();

        let results = store.search_all("test", None).unwrap();
        assert!(results.is_empty());
        assert_eq!(results.total_count(), 0);
    }

    #[test]
    fn test_search_discoveries() {
        let store = MemoryStore::in_memory().unwrap();

        // Add some discoveries
        let discovery1 = Discovery::new(
            "Async Architecture".into(),
            "We use tokio for async runtime in all services".into(),
            test_timestamp(),
        )
        .with_category("architecture".into())
        .with_tags(vec!["async".into(), "tokio".into()]);

        let discovery2 = Discovery::new(
            "Error Handling Strategy".into(),
            "Use thiserror for library crates and anyhow for binaries".into(),
            test_timestamp(),
        )
        .with_category("architecture".into())
        .with_tags(vec!["errors".into()]);

        store.add_discovery(&discovery1).unwrap();
        store.add_discovery(&discovery2).unwrap();

        // Search for "async"
        let results = store.search_all("async", None).unwrap();
        assert_eq!(results.discoveries.len(), 1);
        assert_eq!(results.discoveries[0].title, "Async Architecture");

        // Search for "error"
        let results = store.search_all("error", None).unwrap();
        assert_eq!(results.discoveries.len(), 1);
        assert_eq!(results.discoveries[0].title, "Error Handling Strategy");

        // Search for "architecture" (in category field)
        let results = store.search_all("architecture", None).unwrap();
        assert_eq!(results.discoveries.len(), 2);
    }

    #[test]
    fn test_search_patterns() {
        let store = MemoryStore::in_memory().unwrap();

        // Add some patterns
        let pattern1 = Pattern::new(
            "Result Type Pattern".into(),
            "Always use Result<T> for fallible operations".into(),
            test_timestamp(),
        )
        .with_language("rust".into())
        .with_category("error-handling".into());

        let pattern2 = Pattern::new(
            "Async Function Pattern".into(),
            "Use async fn for I/O operations and tokio runtime".into(),
            test_timestamp(),
        )
        .with_language("rust".into())
        .with_category("async".into());

        store.add_pattern(&pattern1).unwrap();
        store.add_pattern(&pattern2).unwrap();

        // Search for "async"
        let results = store.search_all("async", None).unwrap();
        assert_eq!(results.patterns.len(), 1);
        assert_eq!(results.patterns[0].name, "Async Function Pattern");

        // Search for "Result"
        let results = store.search_all("Result", None).unwrap();
        assert_eq!(results.patterns.len(), 1);
        assert_eq!(results.patterns[0].name, "Result Type Pattern");
    }

    #[test]
    fn test_search_gotchas() {
        let store = MemoryStore::in_memory().unwrap();

        // Add some gotchas
        let gotcha1 = Gotcha::new(
            "Mutex Deadlock".into(),
            "Avoid holding mutex guards across await points".into(),
            "Use tokio::sync::Mutex instead of std::sync::Mutex for async code".into(),
            test_timestamp(),
        )
        .with_severity("critical".into())
        .with_category("concurrency".into());

        let gotcha2 = Gotcha::new(
            "Unwrap Panic".into(),
            "Never use unwrap() in library code".into(),
            "Use ? operator or explicit error handling".into(),
            test_timestamp(),
        )
        .with_severity("high".into())
        .with_category("error-handling".into());

        store.add_gotcha(&gotcha1).unwrap();
        store.add_gotcha(&gotcha2).unwrap();

        // Search for "mutex"
        let results = store.search_all("mutex", None).unwrap();
        assert_eq!(results.gotchas.len(), 1);
        assert_eq!(results.gotchas[0].title, "Mutex Deadlock");

        // Search for "unwrap"
        let results = store.search_all("unwrap", None).unwrap();
        assert_eq!(results.gotchas.len(), 1);
        assert_eq!(results.gotchas[0].title, "Unwrap Panic");
    }

    #[test]
    fn test_search_file_contexts() {
        let store = MemoryStore::in_memory().unwrap();

        // Add some file contexts
        let context1 = FileContext::new(
            "crates/surge-core/src/state.rs".into(),
            "Task state machine implementation using state pattern".into(),
            test_timestamp(),
        )
        .with_language("rust".into())
        .with_module_category("core".into())
        .with_key_apis(vec!["TaskState".into(), "transition".into()]);

        let context2 = FileContext::new(
            "crates/surge-acp/src/client.rs".into(),
            "ACP client implementation for agent communication".into(),
            test_timestamp(),
        )
        .with_language("rust".into())
        .with_module_category("acp".into())
        .with_key_apis(vec!["AcpClient".into(), "send_message".into()]);

        store.add_file_context(&context1).unwrap();
        store.add_file_context(&context2).unwrap();

        // Search for "state machine"
        let results = store.search_all("\"state machine\"", None).unwrap();
        assert_eq!(results.file_contexts.len(), 1);
        assert_eq!(
            results.file_contexts[0].file_path,
            "crates/surge-core/src/state.rs"
        );

        // Search for "ACP"
        let results = store.search_all("ACP", None).unwrap();
        assert_eq!(results.file_contexts.len(), 1);
        assert_eq!(
            results.file_contexts[0].file_path,
            "crates/surge-acp/src/client.rs"
        );
    }

    #[test]
    fn test_search_by_category_discoveries() {
        use crate::memory::fts::{CategorySearchResults, MemoryCategory};

        let store = MemoryStore::in_memory().unwrap();

        // Add a discovery and a pattern with same keyword
        let discovery = Discovery::new(
            "Async Strategy".into(),
            "We use async/await for all I/O".into(),
            test_timestamp(),
        );

        let pattern = Pattern::new(
            "Async Pattern".into(),
            "Use async fn for I/O operations".into(),
            test_timestamp(),
        );

        store.add_discovery(&discovery).unwrap();
        store.add_pattern(&pattern).unwrap();

        // Search only in discoveries category
        let results = store
            .search_by_category(MemoryCategory::Discoveries, "async", None)
            .unwrap();

        match results {
            CategorySearchResults::Discoveries(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].title, "Async Strategy");
            }
            _ => panic!("Expected Discoveries results"),
        }
    }

    #[test]
    fn test_search_by_category_patterns() {
        use crate::memory::fts::{CategorySearchResults, MemoryCategory};

        let store = MemoryStore::in_memory().unwrap();

        // Add a discovery and a pattern with same keyword
        let discovery = Discovery::new(
            "Async Strategy".into(),
            "We use async/await for all I/O".into(),
            test_timestamp(),
        );

        let pattern = Pattern::new(
            "Async Pattern".into(),
            "Use async fn for I/O operations".into(),
            test_timestamp(),
        );

        store.add_discovery(&discovery).unwrap();
        store.add_pattern(&pattern).unwrap();

        // Search only in patterns category
        let results = store
            .search_by_category(MemoryCategory::Patterns, "async", None)
            .unwrap();

        match results {
            CategorySearchResults::Patterns(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].name, "Async Pattern");
            }
            _ => panic!("Expected Patterns results"),
        }
    }

    #[test]
    fn test_search_limit() {
        let store = MemoryStore::in_memory().unwrap();

        // Add 15 discoveries with "test" in content
        for i in 0..15 {
            let discovery = Discovery::new(
                format!("Discovery {i}"),
                "This is a test discovery".into(),
                test_timestamp(),
            );
            store.add_discovery(&discovery).unwrap();
        }

        // Search with limit of 5
        let results = store.search_all("test", Some(5)).unwrap();
        assert_eq!(results.discoveries.len(), 5);

        // Search with default limit (10)
        let results = store.search_all("test", None).unwrap();
        assert_eq!(results.discoveries.len(), 10);
    }

    #[test]
    fn test_search_phrase_query() {
        let store = MemoryStore::in_memory().unwrap();

        let discovery1 = Discovery::new(
            "Exact Phrase".into(),
            "This contains error handling as exact phrase".into(),
            test_timestamp(),
        );

        let discovery2 = Discovery::new(
            "Separate Words".into(),
            "This has error in one place and handling in another".into(),
            test_timestamp(),
        );

        store.add_discovery(&discovery1).unwrap();
        store.add_discovery(&discovery2).unwrap();

        // Phrase search should only match exact phrase
        let results = store.search_all("\"error handling\"", None).unwrap();
        assert_eq!(results.discoveries.len(), 1);
        assert_eq!(results.discoveries[0].title, "Exact Phrase");
    }

    #[test]
    fn test_search_multiple_categories() {
        let store = MemoryStore::in_memory().unwrap();

        // Add entries across different categories with same keyword
        let discovery = Discovery::new(
            "Async Discovery".into(),
            "We use async runtime".into(),
            test_timestamp(),
        );

        let pattern = Pattern::new(
            "Async Pattern".into(),
            "Use async fn".into(),
            test_timestamp(),
        );

        let gotcha = Gotcha::new(
            "Async Gotcha".into(),
            "Don't block in async".into(),
            "Use spawn_blocking".into(),
            test_timestamp(),
        );

        store.add_discovery(&discovery).unwrap();
        store.add_pattern(&pattern).unwrap();
        store.add_gotcha(&gotcha).unwrap();

        // Search should find all three
        let results = store.search_all("async", None).unwrap();
        assert_eq!(results.discoveries.len(), 1);
        assert_eq!(results.patterns.len(), 1);
        assert_eq!(results.gotchas.len(), 1);
        assert_eq!(results.total_count(), 3);
    }
}
