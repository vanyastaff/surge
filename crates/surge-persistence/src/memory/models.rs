//! Data models for project memory and knowledge base.

use serde::{Deserialize, Serialize};
use surge_core::id::{SpecId, TaskId};

// ── Discovery Models ────────────────────────────────────────────────

/// Architectural decision or reasoning captured during task execution.
///
/// Stores high-level architectural decisions and their rationale, enabling
/// agents to understand "why" decisions were made and maintain consistency
/// across tasks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Discovery {
    /// Unique identifier for this discovery.
    pub id: String,

    /// Title or short summary of the discovery.
    pub title: String,

    /// Detailed content describing the architectural decision or reasoning.
    pub content: String,

    /// Task that led to this discovery (if any).
    pub task_id: Option<TaskId>,

    /// Spec this discovery is associated with (if any).
    pub spec_id: Option<SpecId>,

    /// Category or domain of this discovery (e.g., "architecture", "design", "performance").
    pub category: Option<String>,

    /// Tags for categorization and search.
    pub tags: Vec<String>,

    /// Unix timestamp in milliseconds when this discovery was created.
    pub created_at: u64,

    /// Unix timestamp in milliseconds when this discovery was last updated.
    pub updated_at: u64,
}

impl Discovery {
    /// Create a new discovery with the given title and content.
    #[must_use]
    pub fn new(title: String, content: String, timestamp_ms: u64) -> Self {
        let id = ulid::Ulid::new().to_string();
        Self {
            id,
            title,
            content,
            task_id: None,
            spec_id: None,
            category: None,
            tags: Vec::new(),
            created_at: timestamp_ms,
            updated_at: timestamp_ms,
        }
    }

    /// Set the task ID for this discovery.
    #[must_use]
    pub fn with_task_id(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    /// Set the spec ID for this discovery.
    #[must_use]
    pub fn with_spec_id(mut self, spec_id: SpecId) -> Self {
        self.spec_id = Some(spec_id);
        self
    }

    /// Set the category for this discovery.
    #[must_use]
    pub fn with_category(mut self, category: String) -> Self {
        self.category = Some(category);
        self
    }

    /// Add tags to this discovery.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Update the content and timestamp.
    pub fn update_content(&mut self, content: String, timestamp_ms: u64) {
        self.content = content;
        self.updated_at = timestamp_ms;
    }
}

// ── Pattern Models ──────────────────────────────────────────────────

/// Coding pattern or convention discovered by agents.
///
/// Captures coding patterns, idioms, and conventions that should be followed
/// for consistency across the codebase. Enables agents to learn from past
/// implementations and maintain stylistic coherence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pattern {
    /// Unique identifier for this pattern.
    pub id: String,

    /// Name or title of the pattern.
    pub name: String,

    /// Detailed description of the pattern and when to use it.
    pub description: String,

    /// Example code demonstrating the pattern.
    pub example: Option<String>,

    /// Task where this pattern was discovered (if any).
    pub task_id: Option<TaskId>,

    /// Spec this pattern is associated with (if any).
    pub spec_id: Option<SpecId>,

    /// Language or technology this pattern applies to (e.g., "rust", "typescript").
    pub language: Option<String>,

    /// Category of pattern (e.g., "error-handling", "async", "testing").
    pub category: Option<String>,

    /// Tags for categorization and search.
    pub tags: Vec<String>,

    /// Unix timestamp in milliseconds when this pattern was created.
    pub created_at: u64,

    /// Unix timestamp in milliseconds when this pattern was last updated.
    pub updated_at: u64,
}

impl Pattern {
    /// Create a new pattern with the given name and description.
    #[must_use]
    pub fn new(name: String, description: String, timestamp_ms: u64) -> Self {
        let id = ulid::Ulid::new().to_string();
        Self {
            id,
            name,
            description,
            example: None,
            task_id: None,
            spec_id: None,
            language: None,
            category: None,
            tags: Vec::new(),
            created_at: timestamp_ms,
            updated_at: timestamp_ms,
        }
    }

    /// Set an example for this pattern.
    #[must_use]
    pub fn with_example(mut self, example: String) -> Self {
        self.example = Some(example);
        self
    }

    /// Set the task ID for this pattern.
    #[must_use]
    pub fn with_task_id(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    /// Set the spec ID for this pattern.
    #[must_use]
    pub fn with_spec_id(mut self, spec_id: SpecId) -> Self {
        self.spec_id = Some(spec_id);
        self
    }

    /// Set the language for this pattern.
    #[must_use]
    pub fn with_language(mut self, language: String) -> Self {
        self.language = Some(language);
        self
    }

    /// Set the category for this pattern.
    #[must_use]
    pub fn with_category(mut self, category: String) -> Self {
        self.category = Some(category);
        self
    }

    /// Add tags to this pattern.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Update the description and timestamp.
    pub fn update_description(&mut self, description: String, timestamp_ms: u64) {
        self.description = description;
        self.updated_at = timestamp_ms;
    }
}

// ── Gotcha Models ───────────────────────────────────────────────────

/// Known pitfall or error captured from QA failures.
///
/// Records common mistakes, edge cases, and gotchas discovered during QA
/// review or task execution. Prevents agents from repeating the same errors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gotcha {
    /// Unique identifier for this gotcha.
    pub id: String,

    /// Title or short summary of the gotcha.
    pub title: String,

    /// Detailed description of the problem and how to avoid it.
    pub description: String,

    /// The error message or symptom that indicates this gotcha.
    pub symptom: Option<String>,

    /// Solution or workaround for this gotcha.
    pub solution: String,

    /// Task where this gotcha was discovered (if any).
    pub task_id: Option<TaskId>,

    /// Spec this gotcha is associated with (if any).
    pub spec_id: Option<SpecId>,

    /// Severity level (e.g., "critical", "high", "medium", "low").
    pub severity: Option<String>,

    /// Category of gotcha (e.g., "concurrency", "memory", "api").
    pub category: Option<String>,

    /// Tags for categorization and search.
    pub tags: Vec<String>,

    /// Unix timestamp in milliseconds when this gotcha was created.
    pub created_at: u64,

    /// Unix timestamp in milliseconds when this gotcha was last updated.
    pub updated_at: u64,
}

impl Gotcha {
    /// Create a new gotcha with the given title, description, and solution.
    #[must_use]
    pub fn new(title: String, description: String, solution: String, timestamp_ms: u64) -> Self {
        let id = ulid::Ulid::new().to_string();
        Self {
            id,
            title,
            description,
            symptom: None,
            solution,
            task_id: None,
            spec_id: None,
            severity: None,
            category: None,
            tags: Vec::new(),
            created_at: timestamp_ms,
            updated_at: timestamp_ms,
        }
    }

    /// Set the symptom for this gotcha.
    #[must_use]
    pub fn with_symptom(mut self, symptom: String) -> Self {
        self.symptom = Some(symptom);
        self
    }

    /// Set the task ID for this gotcha.
    #[must_use]
    pub fn with_task_id(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    /// Set the spec ID for this gotcha.
    #[must_use]
    pub fn with_spec_id(mut self, spec_id: SpecId) -> Self {
        self.spec_id = Some(spec_id);
        self
    }

    /// Set the severity for this gotcha.
    #[must_use]
    pub fn with_severity(mut self, severity: String) -> Self {
        self.severity = Some(severity);
        self
    }

    /// Set the category for this gotcha.
    #[must_use]
    pub fn with_category(mut self, category: String) -> Self {
        self.category = Some(category);
        self
    }

    /// Add tags to this gotcha.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Update the solution and timestamp.
    pub fn update_solution(&mut self, solution: String, timestamp_ms: u64) {
        self.solution = solution;
        self.updated_at = timestamp_ms;
    }
}

// ── File Context Models ─────────────────────────────────────────────

/// File-level context describing module purpose and key APIs.
///
/// Stores metadata about what each module/file does, its key APIs and contracts,
/// and its role in the overall architecture. Enables context-aware prompt injection
/// based on which files are involved in a subtask.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileContext {
    /// Unique identifier for this file context.
    pub id: String,

    /// File path relative to project root.
    pub file_path: String,

    /// Summary of what this file/module does.
    pub summary: String,

    /// Key APIs, functions, or exports from this file.
    pub key_apis: Vec<String>,

    /// Detailed description of the module's role and responsibilities.
    pub description: Option<String>,

    /// Dependencies or related files.
    pub dependencies: Vec<String>,

    /// Task that created or updated this context (if any).
    pub task_id: Option<TaskId>,

    /// Spec this context is associated with (if any).
    pub spec_id: Option<SpecId>,

    /// Programming language of this file.
    pub language: Option<String>,

    /// Module category (e.g., "core", "cli", "persistence", "acp").
    pub module_category: Option<String>,

    /// Tags for categorization and search.
    pub tags: Vec<String>,

    /// Unix timestamp in milliseconds when this context was created.
    pub created_at: u64,

    /// Unix timestamp in milliseconds when this context was last updated.
    pub updated_at: u64,
}

impl FileContext {
    /// Create a new file context with the given file path and summary.
    #[must_use]
    pub fn new(file_path: String, summary: String, timestamp_ms: u64) -> Self {
        let id = ulid::Ulid::new().to_string();
        Self {
            id,
            file_path,
            summary,
            key_apis: Vec::new(),
            description: None,
            dependencies: Vec::new(),
            task_id: None,
            spec_id: None,
            language: None,
            module_category: None,
            tags: Vec::new(),
            created_at: timestamp_ms,
            updated_at: timestamp_ms,
        }
    }

    /// Add key APIs to this file context.
    #[must_use]
    pub fn with_key_apis(mut self, apis: Vec<String>) -> Self {
        self.key_apis = apis;
        self
    }

    /// Set a detailed description for this file context.
    #[must_use]
    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    /// Add dependencies to this file context.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: Vec<String>) -> Self {
        self.dependencies = dependencies;
        self
    }

    /// Set the task ID for this file context.
    #[must_use]
    pub fn with_task_id(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    /// Set the spec ID for this file context.
    #[must_use]
    pub fn with_spec_id(mut self, spec_id: SpecId) -> Self {
        self.spec_id = Some(spec_id);
        self
    }

    /// Set the language for this file context.
    #[must_use]
    pub fn with_language(mut self, language: String) -> Self {
        self.language = Some(language);
        self
    }

    /// Set the module category for this file context.
    #[must_use]
    pub fn with_module_category(mut self, category: String) -> Self {
        self.module_category = Some(category);
        self
    }

    /// Add tags to this file context.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Update the summary and timestamp.
    pub fn update_summary(&mut self, summary: String, timestamp_ms: u64) {
        self.summary = summary;
        self.updated_at = timestamp_ms;
    }

    /// Add a key API to this file context.
    pub fn add_key_api(&mut self, api: String) {
        if !self.key_apis.contains(&api) {
            self.key_apis.push(api);
        }
    }

    /// Add a dependency to this file context.
    pub fn add_dependency(&mut self, dependency: String) {
        if !self.dependencies.contains(&dependency) {
            self.dependencies.push(dependency);
        }
    }
}
