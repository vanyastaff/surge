//! SQLite-based storage for project memory and knowledge base.

use crate::memory::models::{Discovery, FileContext, Gotcha, Pattern};
use crate::memory::schema::{CREATE_MEMORY_CLAIMS_TABLE, SCHEMA_DDL, SCHEMA_VERSION};
use crate::{PersistenceError, Result};
use rusqlite::{Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use surge_core::memory::{ClaimStatus, Confidence, MemoryClaim, Provenance};
use surge_core::{ContentHash, MemoryClaimId};

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

        if let Some(mut version) = current_version {
            if version > SCHEMA_VERSION {
                return Err(PersistenceError::Storage(format!(
                    "Database schema version {version} is newer than supported version {SCHEMA_VERSION}"
                )));
            }
            // Climb one version at a time so migration steps compose —
            // never delete a step, only add the next one.
            while version < SCHEMA_VERSION {
                version = self.migrate_one_step(version)?;
            }
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

    /// Apply exactly one schema migration step, returning the version
    /// reached. Each historical version gets its own arm; a database
    /// several versions behind climbs one step at a time rather than
    /// jumping straight to `SCHEMA_VERSION`.
    ///
    /// The whole step commits as a single transaction: `CREATE TABLE`,
    /// backfill, and the version-row insert either all land or none do.
    /// A crash or error partway through never leaves the database at the
    /// old version with some claims already inserted — the next open just
    /// retries the same step from `from_version` again. That retry (or a
    /// legacy row visited twice for any other reason) is additionally a
    /// safe no-op on its own terms: [`insert_legacy_claim`] recognizes a
    /// reused id backed by the *same* legacy `source` and skips it, rather
    /// than relying solely on transaction rollback to prevent duplicates.
    fn migrate_one_step(&mut self, from_version: i32) -> Result<i32> {
        match from_version {
            1 => {
                // v1 -> v2: add `memory_claims` (a v1 database predates it)
                // and materialize every existing discovery/pattern/gotcha/
                // file-context row as an unverified, `Asserted`-confidence
                // claim. The v1 tables and their rows are left untouched —
                // no existing text is lost, and every pre-v2 caller keeps
                // working against them.
                let tx = self.conn.transaction()?;
                tx.execute(CREATE_MEMORY_CLAIMS_TABLE, [])?;
                backfill_legacy_claims(&tx)?;
                let next_version = 2;
                tx.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [next_version],
                )?;
                tx.commit()?;
                Ok(next_version)
            },
            other => Err(PersistenceError::Storage(format!(
                "no migration step defined for memory schema version {other}"
            ))),
        }
    }

    // ── Memory Claim Operations (v2) ─────────────────────────────────────

    /// Add a new memory claim to the store. Fails if `claim.id` already
    /// exists — a conflicting id here signals an actual bug, not something
    /// to paper over (contrast the migration backfill's `INSERT OR IGNORE`,
    /// where a repeat visit to the same legacy row is expected and benign).
    ///
    /// No `#[must_use]`: `Result` is already `#[must_use]` itself, so the
    /// attribute would be redundant here and trip `clippy::double_must_use`
    /// — consistent with every other `Result`-returning function in this
    /// crate (none of them carry it either).
    pub fn add_claim(&self, claim: &MemoryClaim) -> Result<()> {
        insert_claim(&self.conn, claim, ClaimConflictPolicy::Fail)
    }

    /// Fetch a memory claim by ID, if one exists.
    pub fn get_claim(&self, id: MemoryClaimId) -> Result<Option<MemoryClaim>> {
        self.conn
            .query_row(
                r#"
                SELECT id, text, source, source_hash, verified_by, verified_at,
                       confidence, status
                FROM memory_claims WHERE id = ?1
                "#,
                [id.to_string()],
                row_to_claim,
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    /// List every memory claim in the store, in insertion order.
    pub fn list_claims(&self) -> Result<Vec<MemoryClaim>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, text, source, source_hash, verified_by, verified_at,
                   confidence, status
            FROM memory_claims ORDER BY rowid
            "#,
        )?;
        let claims = stmt
            .query_map([], row_to_claim)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(claims)
    }

    /// Delete the memory claim named by `id` — the *only* way a claim is
    /// ever removed from this store. `surge memory audit` (and everything
    /// in `crate::memory::audit`) only proposes candidates; this explicit,
    /// human-issued call is what actually deletes one, and it deletes
    /// exactly the row named by `id` — never a range, a filter, or "every
    /// claim the last audit flagged".
    ///
    /// Returns whether a row was actually removed. `false` (not an error)
    /// means `id` did not name an existing claim — already gone, or never
    /// existed — since deleting something already absent still achieves
    /// the caller's goal.
    pub fn delete_claim(&self, id: MemoryClaimId) -> Result<bool> {
        let removed = self
            .conn
            .execute("DELETE FROM memory_claims WHERE id = ?1", [id.to_string()])?;
        Ok(removed > 0)
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
    pub fn search_all(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<crate::memory::fts::SearchResults> {
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
            },
            MemoryCategory::Patterns => {
                let results = self.search_patterns_fts(query, limit)?;
                Ok(CategorySearchResults::Patterns(results))
            },
            MemoryCategory::Gotchas => {
                let results = self.search_gotchas_fts(query, limit)?;
                Ok(CategorySearchResults::Gotchas(results))
            },
            MemoryCategory::FileContexts => {
                let results = self.search_file_contexts_fts(query, limit)?;
                Ok(CategorySearchResults::FileContexts(results))
            },
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
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

                Ok(Discovery {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    content: row.get(2)?,
                    task_id: row
                        .get::<_, Option<String>>(3)?
                        .and_then(|s| s.parse().ok()),
                    spec_id: row
                        .get::<_, Option<String>>(4)?
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
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

                Ok(Pattern {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    example: row.get(3)?,
                    task_id: row
                        .get::<_, Option<String>>(4)?
                        .and_then(|s| s.parse().ok()),
                    spec_id: row
                        .get::<_, Option<String>>(5)?
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
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

                Ok(Gotcha {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    description: row.get(2)?,
                    symptom: row.get(3)?,
                    solution: row.get(4)?,
                    task_id: row
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| s.parse().ok()),
                    spec_id: row
                        .get::<_, Option<String>>(6)?
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
                let key_apis: Vec<String> =
                    serde_json::from_str(&key_apis_json).unwrap_or_default();

                let dependencies_json: String = row.get(5)?;
                let dependencies: Vec<String> =
                    serde_json::from_str(&dependencies_json).unwrap_or_default();

                let tags_json: String = row.get(10)?;
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

                Ok(FileContext {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    summary: row.get(2)?,
                    key_apis,
                    description: row.get(4)?,
                    dependencies,
                    task_id: row
                        .get::<_, Option<String>>(6)?
                        .and_then(|s| s.parse().ok()),
                    spec_id: row
                        .get::<_, Option<String>>(7)?
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

/// Map a `memory_claims` row to a [`MemoryClaim`].
fn row_to_claim(row: &Row<'_>) -> rusqlite::Result<MemoryClaim> {
    let id: String = row.get(0)?;
    let text: String = row.get(1)?;
    let source: String = row.get(2)?;
    let source_hash: String = row.get(3)?;
    let verified_by: Option<String> = row.get(4)?;
    let verified_at: Option<i64> = row.get(5)?;
    let confidence: String = row.get(6)?;
    let status: String = row.get(7)?;

    let id = MemoryClaimId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let hash = ContentHash::from_str(&source_hash).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let confidence = Confidence::from_str(&confidence).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let status = ClaimStatus::from_str(&status).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;

    MemoryClaim::new(
        id,
        text,
        Provenance {
            source,
            hash,
            verified_by,
            verified_at: verified_at.map(|ms| ms as u64),
        },
        confidence,
        status,
    )
    .map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Look up the `source` of an existing `memory_claims` row by id, if any.
fn claim_source_by_id(conn: &Connection, id: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT source FROM memory_claims WHERE id = ?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .map_err(PersistenceError::from)
}

/// How to handle a `memory_claims` primary-key conflict on insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimConflictPolicy {
    /// A conflicting id is an error. The normal, real-usage path
    /// ([`MemoryStore::add_claim`]); also what [`insert_legacy_claim`] falls
    /// back to once it has already decided a fresh id is needed, so an
    /// unexpected further conflict there surfaces loudly rather than
    /// silently swallowing data.
    Fail,
    /// A conflicting id is silently skipped. [`insert_legacy_claim`] uses
    /// this only for the "no existing row with this id" case, where no
    /// conflict is expected in the first place — belt-and-suspenders
    /// against a row appearing between its own lookup and this insert,
    /// not the mechanism that makes a retried backfill idempotent (that is
    /// `insert_legacy_claim`'s own same-`source` check).
    Ignore,
}

/// Insert a claim into `memory_claims` under the given conflict policy.
fn insert_claim(
    conn: &Connection,
    claim: &MemoryClaim,
    on_conflict: ClaimConflictPolicy,
) -> Result<()> {
    let or_ignore = match on_conflict {
        ClaimConflictPolicy::Fail => "",
        ClaimConflictPolicy::Ignore => "OR IGNORE ",
    };
    conn.execute(
        &format!(
            r#"
            INSERT {or_ignore}INTO memory_claims (
                id, text, source, source_hash, verified_by, verified_at,
                confidence, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#
        ),
        rusqlite::params![
            claim.id().to_string(),
            claim.text(),
            claim.provenance().source,
            claim.provenance().hash.to_string(),
            claim.provenance().verified_by,
            claim.provenance().verified_at.map(|ms| ms as i64),
            claim.confidence().as_str(),
            claim.status().as_str(),
        ],
    )?;
    Ok(())
}

/// Materialize every existing discovery/pattern/gotcha/file-context row as
/// a `MemoryClaim`. Legacy rows carry no source hash or verification
/// history, so migration cannot invent one: every migrated claim starts at
/// `Confidence::Asserted` / `ClaimStatus::Unverified` rather than silently
/// inheriting trust it never earned. Runs against whatever `Connection` (or
/// `Transaction`) it is given, so the caller controls the commit boundary.
fn backfill_legacy_claims(conn: &Connection) -> Result<()> {
    for (legacy_id, text) in raw_discoveries_for_migration(conn)? {
        insert_legacy_claim(conn, legacy_claim(&legacy_id, "discoveries", text)?)?;
    }
    for (legacy_id, text) in raw_patterns_for_migration(conn)? {
        insert_legacy_claim(conn, legacy_claim(&legacy_id, "patterns", text)?)?;
    }
    for (legacy_id, text) in raw_gotchas_for_migration(conn)? {
        insert_legacy_claim(conn, legacy_claim(&legacy_id, "gotchas", text)?)?;
    }
    for (legacy_id, text) in raw_file_contexts_for_migration(conn)? {
        insert_legacy_claim(conn, legacy_claim(&legacy_id, "file_contexts", text)?)?;
    }
    Ok(())
}

/// Insert a claim materialized from a legacy row without ever silently
/// dropping its text.
///
/// A migrated claim's id is deterministically reused from its legacy row
/// (see [`legacy_claim`]) — unique *within* that legacy table, courtesy of
/// its own `PRIMARY KEY`, but not *across* the four of them: two
/// independently generated legacy ULIDs could still coincide. If the id
/// already names a `memory_claims` row:
/// - the same `source` means this is the very same legacy row visited
///   again (a retried migration step, or a second backfill pass) — a
///   no-op, not a duplicate;
/// - a *different* `source` means an unrelated row happens to share the
///   id — it is inserted anyway, under a freshly minted id, rather than
///   silently dropped by an `INSERT OR IGNORE` conflict.
fn insert_legacy_claim(conn: &Connection, claim: MemoryClaim) -> Result<()> {
    match claim_source_by_id(conn, &claim.id().to_string())? {
        Some(existing_source) if existing_source == claim.provenance().source => Ok(()),
        Some(_different_source) => insert_claim(
            conn,
            &claim.with_id(MemoryClaimId::new()),
            ClaimConflictPolicy::Fail,
        ),
        None => insert_claim(conn, &claim, ClaimConflictPolicy::Ignore),
    }
}

/// Read every discovery's id and full text (title + content — nothing
/// dropped) for migration into `memory_claims`, oldest first.
fn raw_discoveries_for_migration(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT id, title, content FROM discoveries ORDER BY rowid")?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let content: String = row.get(2)?;
            Ok((id, format!("{title}\n\n{content}")))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read every pattern's id and full text (name + description + example —
/// nothing dropped) for migration into `memory_claims`, oldest first.
fn raw_patterns_for_migration(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, name, description, example FROM patterns ORDER BY rowid")?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let description: String = row.get(2)?;
            let example: Option<String> = row.get(3)?;
            let text = match example {
                Some(example) => format!("{name}\n\n{description}\n\nExample:\n{example}"),
                None => format!("{name}\n\n{description}"),
            };
            Ok((id, text))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read every gotcha's id and full text (title + description + symptom +
/// solution — nothing dropped) for migration into `memory_claims`, oldest
/// first.
fn raw_gotchas_for_migration(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare("SELECT id, title, description, symptom, solution FROM gotchas ORDER BY rowid")?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let description: String = row.get(2)?;
            let symptom: Option<String> = row.get(3)?;
            let solution: String = row.get(4)?;
            let mut text = format!("{title}\n\n{description}");
            if let Some(symptom) = symptom {
                text.push_str(&format!("\n\nSymptom: {symptom}"));
            }
            text.push_str(&format!("\n\nSolution: {solution}"));
            Ok((id, text))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read every file context's id and full text (path + summary + description
/// — nothing dropped) for migration into `memory_claims`, oldest first.
fn raw_file_contexts_for_migration(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare("SELECT id, file_path, summary, description FROM file_contexts ORDER BY rowid")?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let file_path: String = row.get(1)?;
            let summary: String = row.get(2)?;
            let description: Option<String> = row.get(3)?;
            let text = match description {
                Some(description) => format!("{file_path}\n\n{summary}\n\n{description}"),
                None => format!("{file_path}\n\n{summary}"),
            };
            Ok((id, text))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Build the [`MemoryClaim`] a migrated legacy row becomes.
///
/// Legacy rows predate provenance entirely, so the honest source locator is
/// the row's own origin (`legacy:<table>:<id>`), the hash is computed over
/// the composed text itself, and confidence/status start at the lowest
/// tier — nothing here was ever verified.
///
/// Returns `Result` rather than unwrapping/expecting even though `status`
/// is hardcoded to `Unverified` here (which can never trip
/// `UnprovenVerifiedStatus`): `MemoryClaim::new`'s private infallible
/// sibling constructor lives in `surge-core` and isn't reachable across the
/// crate boundary, so this propagates the always-`Ok` result with `?`
/// instead of panicking — library code does not panic on a reachable
/// `Result`, regardless of how provably unreachable the error arm is.
fn legacy_claim(legacy_id: &str, table: &str, text: String) -> Result<MemoryClaim> {
    let hash = ContentHash::compute(text.as_bytes());
    let source = format!("legacy:{table}:{legacy_id}");
    // Legacy ids are bare ULID strings, which `MemoryClaimId::from_str`
    // parses directly (no "claim-" prefix to strip) — reusing them keeps
    // the migrated claim traceable to its origin row. Fall back to a fresh
    // id on the (unexpected) off chance a legacy id isn't a valid ULID.
    let id = MemoryClaimId::from_str(legacy_id).unwrap_or_else(|_| MemoryClaimId::new());
    MemoryClaim::new(
        id,
        text,
        Provenance::unverified(source, hash),
        Confidence::Asserted,
        ClaimStatus::Unverified,
    )
    .map_err(|e| PersistenceError::Storage(e.to_string()))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;

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
            },
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
            },
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

    // ── Memory Claim Tests (v2) ──────────────────────────────────────

    #[test]
    fn add_claim_then_get_claim_roundtrips_every_field() {
        let store = MemoryStore::in_memory().unwrap();
        let hash = ContentHash::compute(b"src/lib.rs");
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "the retry budget is 3 attempts",
            Provenance::verified("src/lib.rs", hash, "cargo test", 1_700_000_000_000),
            Confidence::Verified,
            ClaimStatus::Verified,
        )
        .unwrap();

        store.add_claim(&claim).unwrap();
        let fetched = store.get_claim(claim.id()).unwrap().unwrap();

        assert_eq!(fetched, claim);
    }

    #[test]
    fn get_claim_returns_none_for_unknown_id() {
        let store = MemoryStore::in_memory().unwrap();
        assert!(store.get_claim(MemoryClaimId::new()).unwrap().is_none());
    }

    #[test]
    fn list_claims_includes_every_added_claim() {
        let store = MemoryStore::in_memory().unwrap();
        let a = MemoryClaim::from_transcript(
            "uses tokio for the runtime",
            "transcript:run-1#turn-1",
            ContentHash::compute(b"turn 1"),
        );
        let b = MemoryClaim::from_transcript(
            "errors are thiserror in library crates",
            "transcript:run-1#turn-2",
            ContentHash::compute(b"turn 2"),
        );
        store.add_claim(&a).unwrap();
        store.add_claim(&b).unwrap();

        let listed = store.list_claims().unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|c| c.id() == a.id()));
        assert!(listed.iter().any(|c| c.id() == b.id()));
    }

    #[test]
    fn delete_claim_removes_only_the_named_claim_and_leaves_others_intact() {
        let store = MemoryStore::in_memory().unwrap();
        let keep = MemoryClaim::from_transcript(
            "uses tokio for the runtime",
            "transcript:run-1#turn-1",
            ContentHash::compute(b"turn 1"),
        );
        let remove = MemoryClaim::from_transcript(
            "errors are thiserror in library crates",
            "transcript:run-1#turn-2",
            ContentHash::compute(b"turn 2"),
        );
        store.add_claim(&keep).unwrap();
        store.add_claim(&remove).unwrap();

        let removed = store.delete_claim(remove.id()).unwrap();
        assert!(removed, "delete_claim must report that a row was removed");

        let remaining = store.list_claims().unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "exactly the named claim must be gone, not both or neither"
        );
        assert_eq!(remaining[0].id(), keep.id());
        assert!(store.get_claim(remove.id()).unwrap().is_none());
    }

    #[test]
    fn delete_claim_returns_false_for_an_unknown_id() {
        let store = MemoryStore::in_memory().unwrap();
        assert!(!store.delete_claim(MemoryClaimId::new()).unwrap());
    }

    /// Create all four v1 legacy tables plus the schema_version table on a
    /// fresh in-memory connection, leaving the caller to insert rows and
    /// set `schema_version` to 1.
    fn seed_v1_legacy_tables(conn: &Connection) {
        conn.execute(crate::memory::schema::CREATE_SCHEMA_VERSION_TABLE, [])
            .unwrap();
        conn.execute(crate::memory::schema::CREATE_DISCOVERIES_TABLE, [])
            .unwrap();
        conn.execute(crate::memory::schema::CREATE_PATTERNS_TABLE, [])
            .unwrap();
        conn.execute(crate::memory::schema::CREATE_GOTCHAS_TABLE, [])
            .unwrap();
        conn.execute(crate::memory::schema::CREATE_FILE_CONTEXTS_TABLE, [])
            .unwrap();
    }

    #[test]
    fn migration_v1_to_v2_backfills_all_four_legacy_tables_with_and_without_optional_fields() {
        let conn = Connection::open_in_memory().unwrap();
        seed_v1_legacy_tables(&conn);

        let discovery_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO discoveries (id, title, content, created_at, updated_at)
             VALUES (?1, 'ADR-0002', 'Profile trust reuses the existing approval path.', 1000, 1000)",
            [&discovery_id],
        )
        .unwrap();

        let pattern_with_example_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO patterns (id, name, description, example, created_at, updated_at)
             VALUES (?1, 'Retry Pattern', 'Retry with backoff', 'let x = retry(3);', 1000, 1000)",
            [&pattern_with_example_id],
        )
        .unwrap();
        let pattern_without_example_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO patterns (id, name, description, example, created_at, updated_at)
             VALUES (?1, 'Simple Pattern', 'Just do it', NULL, 1000, 1000)",
            [&pattern_without_example_id],
        )
        .unwrap();

        let gotcha_with_symptom_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO gotchas (id, title, description, symptom, solution, created_at, updated_at)
             VALUES (?1, 'Blocking Gotcha', 'Do not block the runtime', 'Runtime panics under load', 'Use spawn_blocking', 1000, 1000)",
            [&gotcha_with_symptom_id],
        )
        .unwrap();
        let gotcha_without_symptom_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO gotchas (id, title, description, symptom, solution, created_at, updated_at)
             VALUES (?1, 'Silent Gotcha', 'Fails without a message', NULL, 'Log the error explicitly', 1000, 1000)",
            [&gotcha_without_symptom_id],
        )
        .unwrap();

        let file_with_description_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO file_contexts (id, file_path, summary, description, created_at, updated_at)
             VALUES (?1, 'src/lib.rs', 'Crate root', 'Re-exports the public surface', 1000, 1000)",
            [&file_with_description_id],
        )
        .unwrap();
        let file_without_description_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO file_contexts (id, file_path, summary, description, created_at, updated_at)
             VALUES (?1, 'src/main.rs', 'Entry point', NULL, 1000, 1000)",
            [&file_without_description_id],
        )
        .unwrap();

        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
            .unwrap();

        let mut store = MemoryStore {
            conn,
            path: PathBuf::from(":memory:"),
        };
        store.initialize_schema().unwrap();

        let claims = store.list_claims().unwrap();
        assert_eq!(claims.len(), 7);
        let find = |legacy_id: &str| -> &MemoryClaim {
            let expected: MemoryClaimId = legacy_id.parse().unwrap();
            claims.iter().find(|c| c.id() == expected).unwrap()
        };

        let discovery_claim = find(&discovery_id);
        assert!(discovery_claim.text().contains("ADR-0002"));
        assert!(
            discovery_claim
                .text()
                .contains("Profile trust reuses the existing approval path.")
        );

        let pattern_with = find(&pattern_with_example_id);
        assert!(pattern_with.text().contains("Retry Pattern"));
        assert!(pattern_with.text().contains("Retry with backoff"));
        assert!(pattern_with.text().contains("Example:"));
        assert!(pattern_with.text().contains("let x = retry(3);"));

        let pattern_without = find(&pattern_without_example_id);
        assert!(pattern_without.text().contains("Simple Pattern"));
        assert!(pattern_without.text().contains("Just do it"));
        assert!(!pattern_without.text().contains("Example:"));

        let gotcha_with = find(&gotcha_with_symptom_id);
        assert!(gotcha_with.text().contains("Blocking Gotcha"));
        assert!(
            gotcha_with
                .text()
                .contains("Symptom: Runtime panics under load")
        );
        assert!(gotcha_with.text().contains("Solution: Use spawn_blocking"));

        let gotcha_without = find(&gotcha_without_symptom_id);
        assert!(gotcha_without.text().contains("Silent Gotcha"));
        assert!(!gotcha_without.text().contains("Symptom:"));
        assert!(
            gotcha_without
                .text()
                .contains("Solution: Log the error explicitly")
        );

        let file_with = find(&file_with_description_id);
        assert!(file_with.text().contains("src/lib.rs"));
        assert!(file_with.text().contains("Crate root"));
        assert!(file_with.text().contains("Re-exports the public surface"));

        let file_without = find(&file_without_description_id);
        assert_eq!(file_without.text(), "src/main.rs\n\nEntry point");

        for claim in &claims {
            assert_eq!(claim.status(), ClaimStatus::Unverified);
            assert_eq!(claim.confidence(), Confidence::Asserted);
        }
    }

    #[test]
    fn migration_v1_to_v2_reuses_legacy_ulid_and_records_legacy_source() {
        let conn = Connection::open_in_memory().unwrap();
        seed_v1_legacy_tables(&conn);

        let legacy_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO discoveries (id, title, content, created_at, updated_at)
             VALUES (?1, 'ADR-0002', 'Profile trust reuses the existing approval path.', 1000, 1000)",
            [&legacy_id],
        )
        .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
            .unwrap();

        let mut store = MemoryStore {
            conn,
            path: PathBuf::from(":memory:"),
        };
        store.initialize_schema().unwrap();

        let claims = store.list_claims().unwrap();
        assert_eq!(claims.len(), 1);
        let claim = &claims[0];

        let expected_id: MemoryClaimId = legacy_id.parse().unwrap();
        assert_eq!(claim.id(), expected_id, "legacy ULID must be reused as-is");
        assert_eq!(
            claim.provenance().source,
            format!("legacy:discoveries:{legacy_id}")
        );
    }

    #[test]
    fn migration_v1_to_v2_step_rolls_back_atomically_when_backfill_fails_partway() {
        // A v1 database missing the `patterns` table entirely: backfill
        // processes `discoveries` first (succeeds, inserting a claim) and
        // then hits a genuine SQL error scanning the missing `patterns`
        // table, partway through the same migration step.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(crate::memory::schema::CREATE_SCHEMA_VERSION_TABLE, [])
            .unwrap();
        conn.execute(crate::memory::schema::CREATE_DISCOVERIES_TABLE, [])
            .unwrap();
        let discovery_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO discoveries (id, title, content, created_at, updated_at)
             VALUES (?1, 'ADR-0002', 'Profile trust reuses the existing approval path.', 1000, 1000)",
            [&discovery_id],
        )
        .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
            .unwrap();

        let mut store = MemoryStore {
            conn,
            path: PathBuf::from(":memory:"),
        };
        let result = store.initialize_schema();
        assert!(
            result.is_err(),
            "missing `patterns` table must fail the migration step"
        );

        // Still v1: the failed step's version-row insert never committed.
        let version: i32 = store
            .conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 1);

        // `memory_claims` was created (and the discoveries claim inserted
        // into it) inside the very same transaction that then failed on
        // `patterns` — so neither the table nor the row survives rollback.
        let claims_table_exists: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memory_claims'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            claims_table_exists, 0,
            "a partial migration attempt must leave no trace, not a half-populated table"
        );
    }

    #[test]
    fn migration_v1_to_v2_disambiguates_a_cross_table_ulid_collision_without_losing_text() {
        // Two different legacy rows, in different tables, that happen to
        // share a ULID (simulating the astronomically unlikely but
        // possible case of two independently generated ids colliding).
        let conn = Connection::open_in_memory().unwrap();
        seed_v1_legacy_tables(&conn);

        let shared_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO discoveries (id, title, content, created_at, updated_at)
             VALUES (?1, 'Discovery Title', 'Discovery body text', 1000, 1000)",
            [&shared_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO patterns (id, name, description, example, created_at, updated_at)
             VALUES (?1, 'Pattern Name', 'Pattern description text', NULL, 1000, 1000)",
            [&shared_id],
        )
        .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
            .unwrap();

        let mut store = MemoryStore {
            conn,
            path: PathBuf::from(":memory:"),
        };
        store.initialize_schema().unwrap();

        let claims = store.list_claims().unwrap();
        assert_eq!(
            claims.len(),
            2,
            "both rows must survive despite sharing a legacy ULID"
        );

        let discovery_claim = claims
            .iter()
            .find(|c| c.text().contains("Discovery body text"))
            .unwrap();
        let pattern_claim = claims
            .iter()
            .find(|c| c.text().contains("Pattern description text"))
            .unwrap();

        assert_ne!(
            discovery_claim.id(),
            pattern_claim.id(),
            "colliding ids must be disambiguated, not merged"
        );
        assert!(discovery_claim.text().contains("Discovery Title"));
        assert!(pattern_claim.text().contains("Pattern Name"));
    }

    #[test]
    fn backfill_legacy_claims_is_a_noop_when_run_again_over_the_same_rows() {
        let conn = Connection::open_in_memory().unwrap();
        seed_v1_legacy_tables(&conn);
        let discovery_id = Ulid::new().to_string();
        conn.execute(
            "INSERT INTO discoveries (id, title, content, created_at, updated_at)
             VALUES (?1, 'ADR-0002', 'Profile trust reuses the existing approval path.', 1000, 1000)",
            [&discovery_id],
        )
        .unwrap();
        conn.execute(crate::memory::schema::CREATE_MEMORY_CLAIMS_TABLE, [])
            .unwrap();

        backfill_legacy_claims(&conn).unwrap();
        backfill_legacy_claims(&conn).unwrap(); // second pass, same rows

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_claims", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "re-running backfill over the same legacy rows must not duplicate claims"
        );
    }

    #[test]
    fn add_claim_fails_on_duplicate_id() {
        let store = MemoryStore::in_memory().unwrap();
        let hash = ContentHash::compute(b"src/lib.rs");
        let claim_a = MemoryClaim::new(
            MemoryClaimId::new(),
            "first text",
            Provenance::unverified("src/lib.rs", hash),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();
        store.add_claim(&claim_a).unwrap();

        let claim_b = MemoryClaim::new(
            claim_a.id(),
            "different text, same id",
            Provenance::unverified("src/lib.rs", hash),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();

        let err = store
            .add_claim(&claim_b)
            .expect_err("add_claim must fail on a duplicate id, not silently ignore it");
        // Assert *why* it failed, not just that it did: this must be
        // SQLite rejecting the repeated `id` under the table's `PRIMARY
        // KEY` constraint specifically, not some unrelated database error
        // (a malformed statement, a lock timeout) that would leave this
        // test green even if the uniqueness check itself stopped firing.
        match err {
            PersistenceError::Database(rusqlite::Error::SqliteFailure(sqlite_err, _)) => {
                assert_eq!(
                    sqlite_err.code,
                    rusqlite::ErrorCode::ConstraintViolation,
                    "duplicate id must fail as a uniqueness constraint violation, got: {sqlite_err:?}"
                );
            },
            other => panic!(
                "expected a SQLite uniqueness-constraint violation, got a different error: {other:?}"
            ),
        }
    }

    #[test]
    fn insert_claim_with_ignore_policy_silently_keeps_the_first_write_on_conflict() {
        let store = MemoryStore::in_memory().unwrap();
        let hash = ContentHash::compute(b"src/lib.rs");
        let id = MemoryClaimId::new();
        let original = MemoryClaim::new(
            id,
            "original text",
            Provenance::unverified("src/lib.rs", hash),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();
        insert_claim(&store.conn, &original, ClaimConflictPolicy::Fail).unwrap();

        let conflicting = MemoryClaim::new(
            id,
            "conflicting text, must be dropped",
            Provenance::unverified("src/lib.rs", hash),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();
        insert_claim(&store.conn, &conflicting, ClaimConflictPolicy::Ignore).unwrap();

        let fetched = store.get_claim(id).unwrap().unwrap();
        assert_eq!(
            fetched.text(),
            "original text",
            "Ignore policy must not overwrite an existing row"
        );
    }
}
