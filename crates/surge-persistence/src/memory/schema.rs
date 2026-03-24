//! Database schema for project memory and knowledge base.
//!
//! Uses SQLite with FTS5 (Full-Text Search) virtual tables for fast text search
//! across discoveries, patterns, gotchas, and file contexts.

// ── Schema Version ──────────────────────────────────────────────────

/// Current schema version for memory database.
pub const SCHEMA_VERSION: i32 = 1;

/// Schema version table DDL.
pub const CREATE_SCHEMA_VERSION_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY
)
"#;

// ── Discovery Tables ────────────────────────────────────────────────

/// Discoveries table: stores architectural decisions and reasoning.
pub const CREATE_DISCOVERIES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS discoveries (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    task_id TEXT,
    spec_id TEXT,
    category TEXT,
    tags TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
)
"#;

/// FTS5 virtual table for full-text search on discoveries.
pub const CREATE_DISCOVERIES_FTS_TABLE: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS discoveries_fts USING fts5(
    title,
    content,
    category,
    tags,
    content='discoveries',
    content_rowid='rowid'
)
"#;

/// Trigger to keep FTS5 in sync when inserting discoveries.
pub const CREATE_DISCOVERIES_FTS_INSERT_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS discoveries_fts_insert AFTER INSERT ON discoveries BEGIN
    INSERT INTO discoveries_fts(rowid, title, content, category, tags)
    VALUES (new.rowid, new.title, new.content, new.category, new.tags);
END
"#;

/// Trigger to keep FTS5 in sync when updating discoveries.
pub const CREATE_DISCOVERIES_FTS_UPDATE_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS discoveries_fts_update AFTER UPDATE ON discoveries BEGIN
    UPDATE discoveries_fts
    SET title = new.title,
        content = new.content,
        category = new.category,
        tags = new.tags
    WHERE rowid = new.rowid;
END
"#;

/// Trigger to keep FTS5 in sync when deleting discoveries.
pub const CREATE_DISCOVERIES_FTS_DELETE_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS discoveries_fts_delete AFTER DELETE ON discoveries BEGIN
    DELETE FROM discoveries_fts WHERE rowid = old.rowid;
END
"#;

// ── Pattern Tables ──────────────────────────────────────────────────

/// Patterns table: stores coding patterns and conventions.
pub const CREATE_PATTERNS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS patterns (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    example TEXT,
    task_id TEXT,
    spec_id TEXT,
    language TEXT,
    category TEXT,
    tags TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
)
"#;

/// FTS5 virtual table for full-text search on patterns.
pub const CREATE_PATTERNS_FTS_TABLE: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS patterns_fts USING fts5(
    name,
    description,
    example,
    language,
    category,
    tags,
    content='patterns',
    content_rowid='rowid'
)
"#;

/// Trigger to keep FTS5 in sync when inserting patterns.
pub const CREATE_PATTERNS_FTS_INSERT_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS patterns_fts_insert AFTER INSERT ON patterns BEGIN
    INSERT INTO patterns_fts(rowid, name, description, example, language, category, tags)
    VALUES (new.rowid, new.name, new.description, new.example, new.language, new.category, new.tags);
END
"#;

/// Trigger to keep FTS5 in sync when updating patterns.
pub const CREATE_PATTERNS_FTS_UPDATE_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS patterns_fts_update AFTER UPDATE ON patterns BEGIN
    UPDATE patterns_fts
    SET name = new.name,
        description = new.description,
        example = new.example,
        language = new.language,
        category = new.category,
        tags = new.tags
    WHERE rowid = new.rowid;
END
"#;

/// Trigger to keep FTS5 in sync when deleting patterns.
pub const CREATE_PATTERNS_FTS_DELETE_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS patterns_fts_delete AFTER DELETE ON patterns BEGIN
    DELETE FROM patterns_fts WHERE rowid = old.rowid;
END
"#;

// ── Gotcha Tables ───────────────────────────────────────────────────

/// Gotchas table: stores known pitfalls and errors from QA.
pub const CREATE_GOTCHAS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS gotchas (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    symptom TEXT,
    solution TEXT NOT NULL,
    task_id TEXT,
    spec_id TEXT,
    severity TEXT,
    category TEXT,
    tags TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
)
"#;

/// FTS5 virtual table for full-text search on gotchas.
pub const CREATE_GOTCHAS_FTS_TABLE: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS gotchas_fts USING fts5(
    title,
    description,
    symptom,
    solution,
    severity,
    category,
    tags,
    content='gotchas',
    content_rowid='rowid'
)
"#;

/// Trigger to keep FTS5 in sync when inserting gotchas.
pub const CREATE_GOTCHAS_FTS_INSERT_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS gotchas_fts_insert AFTER INSERT ON gotchas BEGIN
    INSERT INTO gotchas_fts(rowid, title, description, symptom, solution, severity, category, tags)
    VALUES (new.rowid, new.title, new.description, new.symptom, new.solution, new.severity, new.category, new.tags);
END
"#;

/// Trigger to keep FTS5 in sync when updating gotchas.
pub const CREATE_GOTCHAS_FTS_UPDATE_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS gotchas_fts_update AFTER UPDATE ON gotchas BEGIN
    UPDATE gotchas_fts
    SET title = new.title,
        description = new.description,
        symptom = new.symptom,
        solution = new.solution,
        severity = new.severity,
        category = new.category,
        tags = new.tags
    WHERE rowid = new.rowid;
END
"#;

/// Trigger to keep FTS5 in sync when deleting gotchas.
pub const CREATE_GOTCHAS_FTS_DELETE_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS gotchas_fts_delete AFTER DELETE ON gotchas BEGIN
    DELETE FROM gotchas_fts WHERE rowid = old.rowid;
END
"#;

// ── File Context Tables ─────────────────────────────────────────────

/// File contexts table: stores file-level metadata and API documentation.
pub const CREATE_FILE_CONTEXTS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS file_contexts (
    id TEXT PRIMARY KEY,
    file_path TEXT NOT NULL UNIQUE,
    summary TEXT NOT NULL,
    key_apis TEXT,
    description TEXT,
    dependencies TEXT,
    task_id TEXT,
    spec_id TEXT,
    language TEXT,
    module_category TEXT,
    tags TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
)
"#;

/// FTS5 virtual table for full-text search on file contexts.
pub const CREATE_FILE_CONTEXTS_FTS_TABLE: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS file_contexts_fts USING fts5(
    file_path,
    summary,
    key_apis,
    description,
    language,
    module_category,
    tags,
    content='file_contexts',
    content_rowid='rowid'
)
"#;

/// Trigger to keep FTS5 in sync when inserting file contexts.
pub const CREATE_FILE_CONTEXTS_FTS_INSERT_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS file_contexts_fts_insert AFTER INSERT ON file_contexts BEGIN
    INSERT INTO file_contexts_fts(rowid, file_path, summary, key_apis, description, language, module_category, tags)
    VALUES (new.rowid, new.file_path, new.summary, new.key_apis, new.description, new.language, new.module_category, new.tags);
END
"#;

/// Trigger to keep FTS5 in sync when updating file contexts.
pub const CREATE_FILE_CONTEXTS_FTS_UPDATE_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS file_contexts_fts_update AFTER UPDATE ON file_contexts BEGIN
    UPDATE file_contexts_fts
    SET file_path = new.file_path,
        summary = new.summary,
        key_apis = new.key_apis,
        description = new.description,
        language = new.language,
        module_category = new.module_category,
        tags = new.tags
    WHERE rowid = new.rowid;
END
"#;

/// Trigger to keep FTS5 in sync when deleting file contexts.
pub const CREATE_FILE_CONTEXTS_FTS_DELETE_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS file_contexts_fts_delete AFTER DELETE ON file_contexts BEGIN
    DELETE FROM file_contexts_fts WHERE rowid = old.rowid;
END
"#;

// ── Indexes ─────────────────────────────────────────────────────────

/// Index discoveries by spec_id for fast lookup by spec.
pub const CREATE_DISCOVERIES_SPEC_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_discoveries_spec ON discoveries(spec_id)";

/// Index discoveries by task_id for fast lookup by task.
pub const CREATE_DISCOVERIES_TASK_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_discoveries_task ON discoveries(task_id)";

/// Index discoveries by category for fast filtering.
pub const CREATE_DISCOVERIES_CATEGORY_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_discoveries_category ON discoveries(category)";

/// Index discoveries by created_at for chronological ordering.
pub const CREATE_DISCOVERIES_CREATED_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_discoveries_created ON discoveries(created_at)";

/// Index patterns by spec_id for fast lookup by spec.
pub const CREATE_PATTERNS_SPEC_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_patterns_spec ON patterns(spec_id)";

/// Index patterns by task_id for fast lookup by task.
pub const CREATE_PATTERNS_TASK_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_patterns_task ON patterns(task_id)";

/// Index patterns by language for fast filtering.
pub const CREATE_PATTERNS_LANGUAGE_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_patterns_language ON patterns(language)";

/// Index patterns by category for fast filtering.
pub const CREATE_PATTERNS_CATEGORY_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_patterns_category ON patterns(category)";

/// Index gotchas by spec_id for fast lookup by spec.
pub const CREATE_GOTCHAS_SPEC_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_gotchas_spec ON gotchas(spec_id)";

/// Index gotchas by task_id for fast lookup by task.
pub const CREATE_GOTCHAS_TASK_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_gotchas_task ON gotchas(task_id)";

/// Index gotchas by severity for prioritization.
pub const CREATE_GOTCHAS_SEVERITY_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_gotchas_severity ON gotchas(severity)";

/// Index gotchas by category for fast filtering.
pub const CREATE_GOTCHAS_CATEGORY_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_gotchas_category ON gotchas(category)";

/// Index file contexts by spec_id for fast lookup by spec.
pub const CREATE_FILE_CONTEXTS_SPEC_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_file_contexts_spec ON file_contexts(spec_id)";

/// Index file contexts by task_id for fast lookup by task.
pub const CREATE_FILE_CONTEXTS_TASK_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_file_contexts_task ON file_contexts(task_id)";

/// Index file contexts by language for fast filtering.
pub const CREATE_FILE_CONTEXTS_LANGUAGE_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_file_contexts_language ON file_contexts(language)";

/// Index file contexts by module_category for fast filtering.
pub const CREATE_FILE_CONTEXTS_MODULE_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_file_contexts_module ON file_contexts(module_category)";

// ── Schema Initialization ───────────────────────────────────────────

/// All DDL statements required to initialize the memory database schema.
///
/// Includes tables, FTS5 virtual tables, triggers, and indexes in the correct
/// order for creation.
pub const SCHEMA_DDL: &[&str] = &[
    // Version tracking
    CREATE_SCHEMA_VERSION_TABLE,
    // Discoveries
    CREATE_DISCOVERIES_TABLE,
    CREATE_DISCOVERIES_FTS_TABLE,
    CREATE_DISCOVERIES_FTS_INSERT_TRIGGER,
    CREATE_DISCOVERIES_FTS_UPDATE_TRIGGER,
    CREATE_DISCOVERIES_FTS_DELETE_TRIGGER,
    CREATE_DISCOVERIES_SPEC_INDEX,
    CREATE_DISCOVERIES_TASK_INDEX,
    CREATE_DISCOVERIES_CATEGORY_INDEX,
    CREATE_DISCOVERIES_CREATED_INDEX,
    // Patterns
    CREATE_PATTERNS_TABLE,
    CREATE_PATTERNS_FTS_TABLE,
    CREATE_PATTERNS_FTS_INSERT_TRIGGER,
    CREATE_PATTERNS_FTS_UPDATE_TRIGGER,
    CREATE_PATTERNS_FTS_DELETE_TRIGGER,
    CREATE_PATTERNS_SPEC_INDEX,
    CREATE_PATTERNS_TASK_INDEX,
    CREATE_PATTERNS_LANGUAGE_INDEX,
    CREATE_PATTERNS_CATEGORY_INDEX,
    // Gotchas
    CREATE_GOTCHAS_TABLE,
    CREATE_GOTCHAS_FTS_TABLE,
    CREATE_GOTCHAS_FTS_INSERT_TRIGGER,
    CREATE_GOTCHAS_FTS_UPDATE_TRIGGER,
    CREATE_GOTCHAS_FTS_DELETE_TRIGGER,
    CREATE_GOTCHAS_SPEC_INDEX,
    CREATE_GOTCHAS_TASK_INDEX,
    CREATE_GOTCHAS_SEVERITY_INDEX,
    CREATE_GOTCHAS_CATEGORY_INDEX,
    // File Contexts
    CREATE_FILE_CONTEXTS_TABLE,
    CREATE_FILE_CONTEXTS_FTS_TABLE,
    CREATE_FILE_CONTEXTS_FTS_INSERT_TRIGGER,
    CREATE_FILE_CONTEXTS_FTS_UPDATE_TRIGGER,
    CREATE_FILE_CONTEXTS_FTS_DELETE_TRIGGER,
    CREATE_FILE_CONTEXTS_SPEC_INDEX,
    CREATE_FILE_CONTEXTS_TASK_INDEX,
    CREATE_FILE_CONTEXTS_LANGUAGE_INDEX,
    CREATE_FILE_CONTEXTS_MODULE_INDEX,
];
