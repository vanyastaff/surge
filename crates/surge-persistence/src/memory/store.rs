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
}
