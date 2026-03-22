# Phase 1: Spec System — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the spec system — TOML-based task definitions with validation, dependency graphs, builder pattern, built-in templates, and CLI commands.

**Architecture:** New `surge-spec` crate depends on `surge-core` types (Spec, Subtask, etc.). It adds: file I/O (parser), programmatic construction (builder), validation (cycles, refs), dependency graph via `petgraph` (topological batching for parallel execution), and built-in templates. CLI gets `surge spec` subcommands.

**Tech Stack:** Rust 2024, serde/toml, petgraph, surge-core types, clap 4

---

### Task 1: Create surge-spec crate scaffold

**Files:**
- Create: `crates/surge-spec/Cargo.toml`
- Create: `crates/surge-spec/src/lib.rs`
- Modify: `Cargo.toml` (workspace)

**Step 1: Create the crate directory and Cargo.toml**

Create `crates/surge-spec/Cargo.toml`:

```toml
[package]
name = "surge-spec"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
surge-core = { workspace = true }
serde = { workspace = true }
toml = { workspace = true }
thiserror = { workspace = true }
petgraph = "0.7"
chrono = { version = "0.4", features = ["serde"] }
```

**Step 2: Create lib.rs**

Create `crates/surge-spec/src/lib.rs`:

```rust
//! Spec system for Surge — parsing, building, validation, and dependency graphs.

pub mod parser;
pub mod builder;
pub mod templates;
pub mod validation;
pub mod graph;
```

(Create empty files for each module — they'll be populated in subsequent tasks.)

**Step 3: Add to workspace**

In root `Cargo.toml`, add `"crates/surge-spec"` to `[workspace.members]` and add workspace deps:

```toml
# In [workspace.members]
"crates/surge-spec",

# In [workspace.dependencies]
petgraph = "0.7"
chrono = { version = "0.4", features = ["serde"] }
surge-spec = { path = "crates/surge-spec" }
```

**Step 4: Add surge-spec dependency to surge-cli**

In `crates/surge-cli/Cargo.toml`, add:

```toml
surge-spec = { workspace = true }
```

**Step 5: Verify compilation**

Run: `cargo check --workspace`
Expected: Compiles (empty modules are fine)

**Step 6: Commit**

```bash
git add crates/surge-spec/ Cargo.toml crates/surge-cli/Cargo.toml
git commit -m "feat(spec): create surge-spec crate scaffold"
```

---

### Task 2: SpecFile and parser.rs — TOML file I/O

**Files:**
- Create: `crates/surge-spec/src/parser.rs`
- Modify: `crates/surge-spec/src/lib.rs`

**Step 1: Write parser.rs with SpecFile and loading/saving**

`SpecFile` wraps `surge_core::Spec` with on-disk metadata. The TOML format looks like:

```toml
[spec]
id = "spec-01JEXAMPLE"
title = "Add user auth"
description = "Implement authentication flow"
complexity = "standard"

[[spec.subtasks]]
id = "sub-01JEXAMPLE1"
title = "Add login endpoint"
description = "POST /api/login"
complexity = "simple"
files = ["src/api/auth.rs"]

[[spec.subtasks.acceptance_criteria]]
description = "Returns JWT on success"

[[spec.subtasks]]
id = "sub-01JEXAMPLE2"
title = "Add middleware"
description = "Auth middleware"
complexity = "simple"
depends_on = ["sub-01JEXAMPLE1"]
```

Create `crates/surge-spec/src/parser.rs`:

```rust
//! Spec file parsing and I/O.

use std::path::{Path, PathBuf};
use surge_core::spec::Spec;
use surge_core::SurgeError;

/// On-disk spec file wrapping a Spec with file metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpecFile {
    /// The spec definition.
    pub spec: Spec,
}

impl SpecFile {
    /// Load a spec from a TOML file.
    pub fn load(path: &Path) -> Result<Self, SurgeError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| SurgeError::Spec(format!("Failed to read {}: {e}", path.display())))?;
        Self::from_toml(&content)
    }

    /// Parse a spec from a TOML string.
    pub fn from_toml(content: &str) -> Result<Self, SurgeError> {
        toml::from_str(content)
            .map_err(|e| SurgeError::Spec(format!("Failed to parse spec TOML: {e}")))
    }

    /// Serialize to TOML string.
    pub fn to_toml(&self) -> Result<String, SurgeError> {
        toml::to_string_pretty(self)
            .map_err(|e| SurgeError::Spec(format!("Failed to serialize spec: {e}")))
    }

    /// Save spec to a TOML file.
    pub fn save(&self, path: &Path) -> Result<(), SurgeError> {
        let content = self.to_toml()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get the default spec directory (.surge/specs/).
    pub fn specs_dir() -> Result<PathBuf, SurgeError> {
        let cwd = std::env::current_dir()?;
        Ok(cwd.join(".surge").join("specs"))
    }

    /// List all spec files in the specs directory.
    pub fn list_all() -> Result<Vec<(PathBuf, SpecFile)>, SurgeError> {
        let dir = Self::specs_dir()?;
        if !dir.exists() {
            return Ok(vec![]);
        }

        let mut specs = vec![];
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                match Self::load(&path) {
                    Ok(spec_file) => specs.push((path, spec_file)),
                    Err(e) => {
                        tracing::warn!("Skipping invalid spec file {}: {e}", path.display());
                    }
                }
            }
        }
        Ok(specs)
    }

    /// Save this spec to the default specs directory using spec ID as filename.
    pub fn save_to_specs_dir(&self) -> Result<PathBuf, SurgeError> {
        let dir = Self::specs_dir()?;
        let filename = format!("{}.toml", self.spec.id);
        let path = dir.join(filename);
        self.save(&path)?;
        Ok(path)
    }
}
```

Add `tracing` dependency to `crates/surge-spec/Cargo.toml`:

```toml
tracing = { workspace = true }
```

**Step 2: Write tests**

Add at the bottom of `parser.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::spec::{AcceptanceCriteria, Complexity, Subtask};
    use surge_core::id::{SpecId, SubtaskId};

    fn sample_spec() -> Spec {
        let sub1_id = SubtaskId::new();
        Spec {
            id: SpecId::new(),
            title: "Test feature".to_string(),
            description: "A test feature spec".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![
                Subtask {
                    id: sub1_id,
                    title: "First step".to_string(),
                    description: "Do the first thing".to_string(),
                    complexity: Complexity::Simple,
                    files: vec!["src/lib.rs".to_string()],
                    acceptance_criteria: vec![
                        AcceptanceCriteria {
                            description: "Compiles".to_string(),
                            met: false,
                        },
                    ],
                    depends_on: vec![],
                },
                Subtask {
                    id: SubtaskId::new(),
                    title: "Second step".to_string(),
                    description: "Do the second thing".to_string(),
                    complexity: Complexity::Simple,
                    files: vec![],
                    acceptance_criteria: vec![],
                    depends_on: vec![sub1_id],
                },
            ],
        }
    }

    #[test]
    fn test_specfile_toml_roundtrip() {
        let spec_file = SpecFile { spec: sample_spec() };
        let toml_str = spec_file.to_toml().unwrap();
        let parsed = SpecFile::from_toml(&toml_str).unwrap();
        assert_eq!(parsed.spec.title, "Test feature");
        assert_eq!(parsed.spec.subtasks.len(), 2);
        assert_eq!(parsed.spec.subtasks[0].acceptance_criteria.len(), 1);
    }

    #[test]
    fn test_specfile_save_load() {
        let temp_dir = std::env::temp_dir().join("surge_test_spec_save");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let path = temp_dir.join("test-spec.toml");
        let spec_file = SpecFile { spec: sample_spec() };

        spec_file.save(&path).unwrap();
        assert!(path.exists());

        let loaded = SpecFile::load(&path).unwrap();
        assert_eq!(loaded.spec.title, spec_file.spec.title);
        assert_eq!(loaded.spec.subtasks.len(), 2);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_specfile_invalid_toml() {
        let result = SpecFile::from_toml("this is not valid {{{");
        assert!(result.is_err());
    }

    #[test]
    fn test_specfile_load_nonexistent() {
        let result = SpecFile::load(Path::new("/nonexistent/path/spec.toml"));
        assert!(result.is_err());
    }
}
```

**Step 3: Update lib.rs exports**

```rust
pub use parser::SpecFile;
```

**Step 4: Run tests**

Run: `cargo test -p surge-spec`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add crates/surge-spec/
git commit -m "feat(spec): add SpecFile parser — TOML load/save/list"
```

---

### Task 3: SpecBuilder — programmatic spec construction

**Files:**
- Create: `crates/surge-spec/src/builder.rs`

**Step 1: Write builder.rs**

```rust
//! Builder pattern for constructing specs programmatically.

use surge_core::id::{SpecId, SubtaskId};
use surge_core::spec::{AcceptanceCriteria, Complexity, Spec, Subtask};
use surge_core::SurgeError;

/// Builder for constructing Spec instances.
pub struct SpecBuilder {
    title: Option<String>,
    description: Option<String>,
    complexity: Complexity,
    subtasks: Vec<Subtask>,
}

impl SpecBuilder {
    /// Create a new SpecBuilder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            title: None,
            description: None,
            complexity: Complexity::Standard,
            subtasks: vec![],
        }
    }

    /// Set the spec title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the spec description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the overall complexity.
    #[must_use]
    pub fn complexity(mut self, complexity: Complexity) -> Self {
        self.complexity = complexity;
        self
    }

    /// Add a subtask using a SubtaskBuilder.
    #[must_use]
    pub fn subtask(mut self, subtask: Subtask) -> Self {
        self.subtasks.push(subtask);
        self
    }

    /// Build the final Spec.
    pub fn build(self) -> Result<Spec, SurgeError> {
        let title = self.title
            .ok_or_else(|| SurgeError::Spec("Spec title is required".to_string()))?;
        let description = self.description
            .ok_or_else(|| SurgeError::Spec("Spec description is required".to_string()))?;

        Ok(Spec {
            id: SpecId::new(),
            title,
            description,
            complexity: self.complexity,
            subtasks: self.subtasks,
        })
    }
}

impl Default for SpecBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for constructing Subtask instances.
pub struct SubtaskBuilder {
    title: Option<String>,
    description: Option<String>,
    complexity: Complexity,
    files: Vec<String>,
    acceptance_criteria: Vec<AcceptanceCriteria>,
    depends_on: Vec<SubtaskId>,
}

impl SubtaskBuilder {
    /// Create a new SubtaskBuilder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            title: None,
            description: None,
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![],
        }
    }

    /// Set the subtask title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the subtask description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the subtask complexity.
    #[must_use]
    pub fn complexity(mut self, complexity: Complexity) -> Self {
        self.complexity = complexity;
        self
    }

    /// Add a file this subtask will touch.
    #[must_use]
    pub fn file(mut self, path: impl Into<String>) -> Self {
        self.files.push(path.into());
        self
    }

    /// Add an acceptance criterion.
    #[must_use]
    pub fn criterion(mut self, description: impl Into<String>) -> Self {
        self.acceptance_criteria.push(AcceptanceCriteria {
            description: description.into(),
            met: false,
        });
        self
    }

    /// Add a dependency on another subtask.
    #[must_use]
    pub fn depends_on(mut self, id: SubtaskId) -> Self {
        self.depends_on.push(id);
        self
    }

    /// Build the Subtask.
    pub fn build(self) -> Result<Subtask, SurgeError> {
        let title = self.title
            .ok_or_else(|| SurgeError::Spec("Subtask title is required".to_string()))?;
        let description = self.description
            .ok_or_else(|| SurgeError::Spec("Subtask description is required".to_string()))?;

        Ok(Subtask {
            id: SubtaskId::new(),
            title,
            description,
            complexity: self.complexity,
            files: self.files,
            acceptance_criteria: self.acceptance_criteria,
            depends_on: self.depends_on,
        })
    }
}

impl Default for SubtaskBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_builder_basic() {
        let spec = SpecBuilder::new()
            .title("My feature")
            .description("Build a feature")
            .complexity(Complexity::Complex)
            .build()
            .unwrap();

        assert_eq!(spec.title, "My feature");
        assert_eq!(spec.description, "Build a feature");
        assert_eq!(spec.complexity, Complexity::Complex);
        assert!(spec.subtasks.is_empty());
    }

    #[test]
    fn test_spec_builder_with_subtasks() {
        let sub1 = SubtaskBuilder::new()
            .title("Step 1")
            .description("Do step 1")
            .file("src/lib.rs")
            .criterion("Compiles")
            .build()
            .unwrap();

        let sub1_id = sub1.id;

        let sub2 = SubtaskBuilder::new()
            .title("Step 2")
            .description("Do step 2")
            .depends_on(sub1_id)
            .build()
            .unwrap();

        let spec = SpecBuilder::new()
            .title("Feature")
            .description("A feature")
            .subtask(sub1)
            .subtask(sub2)
            .build()
            .unwrap();

        assert_eq!(spec.subtasks.len(), 2);
        assert_eq!(spec.subtasks[0].files, vec!["src/lib.rs"]);
        assert_eq!(spec.subtasks[0].acceptance_criteria.len(), 1);
        assert_eq!(spec.subtasks[1].depends_on, vec![sub1_id]);
    }

    #[test]
    fn test_spec_builder_missing_title() {
        let result = SpecBuilder::new()
            .description("No title")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_subtask_builder_missing_title() {
        let result = SubtaskBuilder::new()
            .description("No title")
            .build();
        assert!(result.is_err());
    }
}
```

**Step 2: Update lib.rs**

Add `pub use builder::{SpecBuilder, SubtaskBuilder};`

**Step 3: Run tests**

Run: `cargo test -p surge-spec -- builder`
Expected: All 4 tests PASS

**Step 4: Commit**

```bash
git add crates/surge-spec/src/builder.rs crates/surge-spec/src/lib.rs
git commit -m "feat(spec): add SpecBuilder and SubtaskBuilder"
```

---

### Task 4: templates.rs — built-in spec templates

**Files:**
- Create: `crates/surge-spec/src/templates.rs`

**Step 1: Write templates.rs**

```rust
//! Built-in spec templates for common task types.

use surge_core::spec::Complexity;
use crate::builder::{SpecBuilder, SubtaskBuilder};
use crate::parser::SpecFile;
use surge_core::SurgeError;

/// Available built-in template types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Feature,
    Bugfix,
    Refactor,
}

impl TemplateKind {
    /// Parse from string.
    pub fn from_str(s: &str) -> Result<Self, SurgeError> {
        match s.to_lowercase().as_str() {
            "feature" => Ok(Self::Feature),
            "bugfix" | "fix" => Ok(Self::Bugfix),
            "refactor" => Ok(Self::Refactor),
            _ => Err(SurgeError::Spec(format!(
                "Unknown template '{}'. Available: feature, bugfix, refactor",
                s
            ))),
        }
    }

    /// List all available template names.
    pub fn all() -> &'static [&'static str] {
        &["feature", "bugfix", "refactor"]
    }
}

/// Generate a spec from a template.
pub fn generate(kind: TemplateKind, description: &str) -> Result<SpecFile, SurgeError> {
    let spec = match kind {
        TemplateKind::Feature => {
            let sub1 = SubtaskBuilder::new()
                .title("Design and plan")
                .description("Define the approach, identify files to modify, write acceptance criteria")
                .complexity(Complexity::Simple)
                .criterion("Approach documented")
                .build()?;

            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Implement core logic")
                .description("Write the main implementation code")
                .complexity(Complexity::Standard)
                .criterion("Core logic works")
                .criterion("Tests pass")
                .depends_on(sub1_id)
                .build()?;

            let sub2_id = sub2.id;

            let sub3 = SubtaskBuilder::new()
                .title("Integration and tests")
                .description("Wire up the feature, add integration tests, update docs")
                .complexity(Complexity::Simple)
                .criterion("Integration tests pass")
                .criterion("Documentation updated")
                .depends_on(sub2_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Feature: {description}"))
                .complexity(Complexity::Standard)
                .subtask(sub1)
                .subtask(sub2)
                .subtask(sub3)
                .build()?
        }
        TemplateKind::Bugfix => {
            let sub1 = SubtaskBuilder::new()
                .title("Reproduce and diagnose")
                .description("Write a failing test that reproduces the bug")
                .complexity(Complexity::Simple)
                .criterion("Failing test exists")
                .build()?;

            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Fix the bug")
                .description("Implement the minimal fix to make the test pass")
                .complexity(Complexity::Simple)
                .criterion("Failing test now passes")
                .criterion("No regressions")
                .depends_on(sub1_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Bugfix: {description}"))
                .complexity(Complexity::Simple)
                .subtask(sub1)
                .subtask(sub2)
                .build()?
        }
        TemplateKind::Refactor => {
            let sub1 = SubtaskBuilder::new()
                .title("Ensure test coverage")
                .description("Add tests for current behavior before refactoring")
                .complexity(Complexity::Simple)
                .criterion("Existing behavior covered by tests")
                .build()?;

            let sub1_id = sub1.id;

            let sub2 = SubtaskBuilder::new()
                .title("Refactor")
                .description("Apply the refactoring, keep tests green")
                .complexity(Complexity::Standard)
                .criterion("All tests still pass")
                .criterion("Code is cleaner")
                .depends_on(sub1_id)
                .build()?;

            SpecBuilder::new()
                .title(description)
                .description(format!("Refactor: {description}"))
                .complexity(Complexity::Simple)
                .subtask(sub1)
                .subtask(sub2)
                .build()?
        }
    };

    Ok(SpecFile { spec })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_kind_from_str() {
        assert_eq!(TemplateKind::from_str("feature").unwrap(), TemplateKind::Feature);
        assert_eq!(TemplateKind::from_str("bugfix").unwrap(), TemplateKind::Bugfix);
        assert_eq!(TemplateKind::from_str("fix").unwrap(), TemplateKind::Bugfix);
        assert_eq!(TemplateKind::from_str("refactor").unwrap(), TemplateKind::Refactor);
        assert!(TemplateKind::from_str("unknown").is_err());
    }

    #[test]
    fn test_feature_template() {
        let spec_file = generate(TemplateKind::Feature, "Add user auth").unwrap();
        assert_eq!(spec_file.spec.title, "Add user auth");
        assert_eq!(spec_file.spec.subtasks.len(), 3);
        // Second subtask depends on first
        assert_eq!(spec_file.spec.subtasks[1].depends_on.len(), 1);
        assert_eq!(spec_file.spec.subtasks[1].depends_on[0], spec_file.spec.subtasks[0].id);
    }

    #[test]
    fn test_bugfix_template() {
        let spec_file = generate(TemplateKind::Bugfix, "Fix login crash").unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 2);
        assert_eq!(spec_file.spec.complexity, Complexity::Simple);
    }

    #[test]
    fn test_refactor_template() {
        let spec_file = generate(TemplateKind::Refactor, "Extract auth module").unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 2);
    }

    #[test]
    fn test_template_toml_roundtrip() {
        let spec_file = generate(TemplateKind::Feature, "Test feature").unwrap();
        let toml_str = spec_file.to_toml().unwrap();
        let parsed = SpecFile::from_toml(&toml_str).unwrap();
        assert_eq!(parsed.spec.title, "Test feature");
        assert_eq!(parsed.spec.subtasks.len(), 3);
    }
}
```

**Step 2: Update lib.rs**

Add `pub use templates::{TemplateKind, generate as generate_template};`

**Step 3: Run tests**

Run: `cargo test -p surge-spec -- templates`
Expected: 5 tests PASS

**Step 4: Commit**

```bash
git add crates/surge-spec/src/templates.rs crates/surge-spec/src/lib.rs
git commit -m "feat(spec): add built-in spec templates (feature, bugfix, refactor)"
```

---

### Task 5: validation.rs — spec validation

**Files:**
- Create: `crates/surge-spec/src/validation.rs`

**Step 1: Write validation.rs**

```rust
//! Spec validation — check integrity, references, and cycles.

use std::collections::{HashMap, HashSet};
use surge_core::id::SubtaskId;
use surge_core::spec::Spec;
use surge_core::SurgeError;

/// Validation result with warnings and errors.
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    /// Returns true if validation passed (no errors).
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Convert to Result — Ok if no errors, Err with combined error message.
    pub fn into_result(self) -> Result<Vec<String>, SurgeError> {
        if self.errors.is_empty() {
            Ok(self.warnings)
        } else {
            Err(SurgeError::Spec(format!(
                "Spec validation failed:\n{}",
                self.errors.join("\n")
            )))
        }
    }
}

/// Validate a spec for correctness.
pub fn validate(spec: &Spec) -> ValidationResult {
    let mut result = ValidationResult::default();

    // Check title not empty
    if spec.title.trim().is_empty() {
        result.errors.push("Spec title is empty".to_string());
    }

    // Check description not empty
    if spec.description.trim().is_empty() {
        result.errors.push("Spec description is empty".to_string());
    }

    // Warn if no subtasks
    if spec.subtasks.is_empty() {
        result.warnings.push("Spec has no subtasks".to_string());
        return result;
    }

    // Build set of valid subtask IDs
    let valid_ids: HashSet<SubtaskId> = spec.subtasks.iter().map(|s| s.id).collect();

    // Check for duplicate subtask IDs
    if valid_ids.len() != spec.subtasks.len() {
        result.errors.push("Duplicate subtask IDs found".to_string());
    }

    // Check all depends_on references point to existing subtasks
    for subtask in &spec.subtasks {
        if subtask.title.trim().is_empty() {
            result.errors.push(format!("Subtask {} has empty title", subtask.id));
        }
        if subtask.description.trim().is_empty() {
            result.warnings.push(format!("Subtask '{}' has empty description", subtask.title));
        }
        for dep_id in &subtask.depends_on {
            if !valid_ids.contains(dep_id) {
                result.errors.push(format!(
                    "Subtask '{}' depends on non-existent subtask {}",
                    subtask.title, dep_id
                ));
            }
        }
        // Self-dependency check
        if subtask.depends_on.contains(&subtask.id) {
            result.errors.push(format!(
                "Subtask '{}' depends on itself",
                subtask.title
            ));
        }
    }

    // Check for cycles using DFS
    if has_cycle(spec) {
        result.errors.push("Dependency cycle detected among subtasks".to_string());
    }

    result
}

/// Check if the dependency graph has cycles using DFS.
fn has_cycle(spec: &Spec) -> bool {
    let id_to_deps: HashMap<SubtaskId, &Vec<SubtaskId>> = spec
        .subtasks
        .iter()
        .map(|s| (s.id, &s.depends_on))
        .collect();

    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();

    for subtask in &spec.subtasks {
        if !visited.contains(&subtask.id)
            && dfs_has_cycle(subtask.id, &id_to_deps, &mut visited, &mut in_stack)
        {
            return true;
        }
    }

    false
}

fn dfs_has_cycle(
    node: SubtaskId,
    graph: &HashMap<SubtaskId, &Vec<SubtaskId>>,
    visited: &mut HashSet<SubtaskId>,
    in_stack: &mut HashSet<SubtaskId>,
) -> bool {
    visited.insert(node);
    in_stack.insert(node);

    if let Some(deps) = graph.get(&node) {
        for dep in *deps {
            if !visited.contains(dep) {
                if dfs_has_cycle(*dep, graph, visited, in_stack) {
                    return true;
                }
            } else if in_stack.contains(dep) {
                return true;
            }
        }
    }

    in_stack.remove(&node);
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::SpecId;
    use surge_core::spec::{Complexity, Subtask};

    fn make_subtask(title: &str, depends_on: Vec<SubtaskId>) -> Subtask {
        Subtask {
            id: SubtaskId::new(),
            title: title.to_string(),
            description: format!("Do {title}"),
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on,
        }
    }

    #[test]
    fn test_valid_spec() {
        let sub1 = make_subtask("Step 1", vec![]);
        let sub2 = make_subtask("Step 2", vec![sub1.id]);
        let spec = Spec {
            id: SpecId::new(),
            title: "Valid spec".to_string(),
            description: "A valid spec".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![sub1, sub2],
        };
        let result = validate(&spec);
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn test_empty_title() {
        let spec = Spec {
            id: SpecId::new(),
            title: "".to_string(),
            description: "Desc".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![],
        };
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("title is empty")));
    }

    #[test]
    fn test_invalid_dependency_ref() {
        let fake_id = SubtaskId::new();
        let sub1 = make_subtask("Step 1", vec![fake_id]);
        let spec = Spec {
            id: SpecId::new(),
            title: "Bad refs".to_string(),
            description: "Has bad refs".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![sub1],
        };
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("non-existent")));
    }

    #[test]
    fn test_self_dependency() {
        let mut sub1 = make_subtask("Step 1", vec![]);
        sub1.depends_on = vec![sub1.id];
        let spec = Spec {
            id: SpecId::new(),
            title: "Self dep".to_string(),
            description: "Self dependency".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![sub1],
        };
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("depends on itself")));
    }

    #[test]
    fn test_cycle_detection() {
        let id_a = SubtaskId::new();
        let id_b = SubtaskId::new();
        let sub_a = Subtask {
            id: id_a,
            title: "A".to_string(),
            description: "A".to_string(),
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![id_b],
        };
        let sub_b = Subtask {
            id: id_b,
            title: "B".to_string(),
            description: "B".to_string(),
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![id_a],
        };
        let spec = Spec {
            id: SpecId::new(),
            title: "Cycle".to_string(),
            description: "Has cycle".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![sub_a, sub_b],
        };
        let result = validate(&spec);
        assert!(result.errors.iter().any(|e| e.contains("cycle")));
    }

    #[test]
    fn test_no_subtasks_warning() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Empty".to_string(),
            description: "No subtasks".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![],
        };
        let result = validate(&spec);
        assert!(result.is_ok());
        assert!(result.warnings.iter().any(|w| w.contains("no subtasks")));
    }
}
```

**Step 2: Update lib.rs**

Add `pub use validation::{validate as validate_spec, ValidationResult};`

**Step 3: Run tests**

Run: `cargo test -p surge-spec -- validation`
Expected: 6 tests PASS

**Step 4: Commit**

```bash
git add crates/surge-spec/src/validation.rs crates/surge-spec/src/lib.rs
git commit -m "feat(spec): add spec validation — refs, cycles, required fields"
```

---

### Task 6: graph.rs — dependency graph and topological batching

**Files:**
- Create: `crates/surge-spec/src/graph.rs`

**Step 1: Write graph.rs**

```rust
//! Dependency graph for spec subtasks — topological sorting and batch grouping.

use std::collections::HashMap;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::algo::toposort;
use petgraph::Direction;
use surge_core::id::SubtaskId;
use surge_core::spec::Spec;
use surge_core::SurgeError;

/// A dependency graph of subtasks.
pub struct DependencyGraph {
    graph: DiGraph<SubtaskId, ()>,
    id_to_node: HashMap<SubtaskId, NodeIndex>,
    node_to_id: HashMap<NodeIndex, SubtaskId>,
}

impl DependencyGraph {
    /// Build a dependency graph from a spec.
    ///
    /// Edges point from dependency TO dependent (dep → subtask that needs it).
    pub fn from_spec(spec: &Spec) -> Result<Self, SurgeError> {
        let mut graph = DiGraph::new();
        let mut id_to_node = HashMap::new();
        let mut node_to_id = HashMap::new();

        // Add all subtasks as nodes
        for subtask in &spec.subtasks {
            let node = graph.add_node(subtask.id);
            id_to_node.insert(subtask.id, node);
            node_to_id.insert(node, subtask.id);
        }

        // Add dependency edges
        for subtask in &spec.subtasks {
            let target = id_to_node[&subtask.id];
            for dep_id in &subtask.depends_on {
                let source = id_to_node.get(dep_id).ok_or_else(|| {
                    SurgeError::Spec(format!(
                        "Subtask '{}' depends on unknown subtask {}",
                        subtask.title, dep_id
                    ))
                })?;
                graph.add_edge(*source, target, ());
            }
        }

        Ok(Self { graph, id_to_node, node_to_id })
    }

    /// Get topologically sorted subtask IDs.
    pub fn topological_order(&self) -> Result<Vec<SubtaskId>, SurgeError> {
        let sorted = toposort(&self.graph, None)
            .map_err(|_| SurgeError::Spec("Dependency cycle detected".to_string()))?;

        Ok(sorted.into_iter().map(|n| self.node_to_id[&n]).collect())
    }

    /// Group subtasks into batches for parallel execution.
    ///
    /// Each batch contains subtasks that can execute in parallel.
    /// Batch N+1 only starts after all tasks in batch N complete.
    pub fn topological_batches(&self) -> Result<Vec<Vec<SubtaskId>>, SurgeError> {
        // Compute "depth" for each node = max depth of dependencies + 1
        let sorted = toposort(&self.graph, None)
            .map_err(|_| SurgeError::Spec("Dependency cycle detected".to_string()))?;

        let mut depths: HashMap<NodeIndex, usize> = HashMap::new();
        let mut max_depth = 0;

        for node in &sorted {
            let depth = self.graph
                .neighbors_directed(*node, Direction::Incoming)
                .map(|dep| depths.get(&dep).copied().unwrap_or(0) + 1)
                .max()
                .unwrap_or(0);
            depths.insert(*node, depth);
            max_depth = max_depth.max(depth);
        }

        // Group by depth
        let mut batches: Vec<Vec<SubtaskId>> = vec![vec![]; max_depth + 1];
        for (node, depth) in &depths {
            batches[*depth].push(self.node_to_id[node]);
        }

        // Remove empty batches (shouldn't happen, but be safe)
        batches.retain(|b| !b.is_empty());

        Ok(batches)
    }

    /// Render dependency graph as ASCII text.
    pub fn to_ascii(&self, spec: &Spec) -> String {
        let title_map: HashMap<SubtaskId, &str> = spec
            .subtasks
            .iter()
            .map(|s| (s.id, s.title.as_str()))
            .collect();

        let Ok(batches) = self.topological_batches() else {
            return "Error: cycle in dependency graph".to_string();
        };

        let mut lines = vec![];

        for (i, batch) in batches.iter().enumerate() {
            lines.push(format!("Batch {}:", i + 1));
            for id in batch {
                let title = title_map.get(id).unwrap_or(&"???");
                let deps: Vec<&str> = spec.subtasks.iter()
                    .find(|s| s.id == *id)
                    .map(|s| {
                        s.depends_on.iter()
                            .filter_map(|d| title_map.get(d).copied())
                            .collect()
                    })
                    .unwrap_or_default();

                if deps.is_empty() {
                    lines.push(format!("  ├── {title}"));
                } else {
                    lines.push(format!("  ├── {title} (after: {})", deps.join(", ")));
                }
            }
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::SpecId;
    use surge_core::spec::{Complexity, Subtask};

    fn make_subtask(title: &str, depends_on: Vec<SubtaskId>) -> Subtask {
        Subtask {
            id: SubtaskId::new(),
            title: title.to_string(),
            description: format!("Do {title}"),
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on,
        }
    }

    #[test]
    fn test_no_dependencies_single_batch() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Parallel".to_string(),
            description: "All parallel".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![
                make_subtask("A", vec![]),
                make_subtask("B", vec![]),
                make_subtask("C", vec![]),
            ],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let batches = graph.topological_batches().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 3);
    }

    #[test]
    fn test_linear_chain_n_batches() {
        let a = make_subtask("A", vec![]);
        let b = make_subtask("B", vec![a.id]);
        let c = make_subtask("C", vec![b.id]);

        let spec = Spec {
            id: SpecId::new(),
            title: "Linear".to_string(),
            description: "Linear chain".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![a, b, c],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let batches = graph.topological_batches().unwrap();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[1].len(), 1);
        assert_eq!(batches[2].len(), 1);
    }

    #[test]
    fn test_diamond_dependency() {
        //   A
        //  / \
        // B   C
        //  \ /
        //   D
        let a = make_subtask("A", vec![]);
        let b = make_subtask("B", vec![a.id]);
        let c = make_subtask("C", vec![a.id]);
        let d = make_subtask("D", vec![b.id, c.id]);

        let spec = Spec {
            id: SpecId::new(),
            title: "Diamond".to_string(),
            description: "Diamond".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![a, b, c, d],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let batches = graph.topological_batches().unwrap();

        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 1); // A
        assert_eq!(batches[1].len(), 2); // B, C parallel
        assert_eq!(batches[2].len(), 1); // D
    }

    #[test]
    fn test_topological_order() {
        let a = make_subtask("A", vec![]);
        let b = make_subtask("B", vec![a.id]);
        let a_id = a.id;
        let b_id = b.id;

        let spec = Spec {
            id: SpecId::new(),
            title: "Order".to_string(),
            description: "Order test".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![a, b],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let order = graph.topological_order().unwrap();
        let a_pos = order.iter().position(|id| *id == a_id).unwrap();
        let b_pos = order.iter().position(|id| *id == b_id).unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn test_ascii_output() {
        let a = make_subtask("Setup", vec![]);
        let b = make_subtask("Implement", vec![a.id]);

        let spec = Spec {
            id: SpecId::new(),
            title: "Test".to_string(),
            description: "Test".to_string(),
            complexity: Complexity::Simple,
            subtasks: vec![a, b],
        };

        let graph = DependencyGraph::from_spec(&spec).unwrap();
        let ascii = graph.to_ascii(&spec);
        assert!(ascii.contains("Batch 1:"));
        assert!(ascii.contains("Setup"));
        assert!(ascii.contains("Implement"));
        assert!(ascii.contains("after: Setup"));
    }
}
```

**Step 2: Update lib.rs**

Add `pub use graph::DependencyGraph;`

**Step 3: Run tests**

Run: `cargo test -p surge-spec -- graph`
Expected: 5 tests PASS

**Step 4: Commit**

```bash
git add crates/surge-spec/src/graph.rs crates/surge-spec/src/lib.rs
git commit -m "feat(spec): add dependency graph with topological batching and ASCII output"
```

---

### Task 7: CLI spec commands

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Add Spec subcommand to Commands enum**

Add to `Commands` enum:

```rust
/// Manage specs
Spec {
    #[command(subcommand)]
    command: SpecCommands,
},
```

Add new enum:

```rust
#[derive(Subcommand)]
enum SpecCommands {
    /// Create a new spec from a template
    Create {
        /// Description of the spec
        description: String,
        /// Template to use (feature, bugfix, refactor)
        #[arg(short, long)]
        template: Option<String>,
    },
    /// List all specs
    List,
    /// Show spec details
    Show {
        /// Spec ID or filename
        id: String,
    },
    /// Validate a spec
    Validate {
        /// Spec ID or filename
        id: String,
    },
}
```

**Step 2: Add handlers**

Add the `Commands::Spec` match arm:

```rust
Commands::Spec { command } => match command {
    SpecCommands::Create { description, template } => {
        let kind = template.as_deref().unwrap_or("feature");
        let template_kind = surge_spec::TemplateKind::from_str(kind)?;
        let spec_file = surge_spec::generate_template(template_kind, &description)?;

        let path = spec_file.save_to_specs_dir()?;
        println!("⚡ Created spec: {}", spec_file.spec.title);
        println!("   ID: {}", spec_file.spec.id);
        println!("   File: {}", path.display());
        println!("   Subtasks: {}", spec_file.spec.subtasks.len());
    }
    SpecCommands::List => {
        let specs = surge_spec::SpecFile::list_all()?;

        if specs.is_empty() {
            println!("No specs found. Create one with: surge spec create \"description\"");
        } else {
            println!("⚡ Specs:\n");
            for (path, sf) in &specs {
                let filename = path.file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();
                println!("  {} — {} ({} subtasks)",
                    filename,
                    sf.spec.title,
                    sf.spec.subtasks.len()
                );
            }
        }
    }
    SpecCommands::Show { id } => {
        let spec_file = load_spec_by_id(&id)?;
        let spec = &spec_file.spec;

        println!("⚡ Spec: {}\n", spec.title);
        println!("ID: {}", spec.id);
        println!("Complexity: {:?}", spec.complexity);
        println!("Description: {}", spec.description);
        println!("\nSubtasks ({}):", spec.subtasks.len());

        for (i, sub) in spec.subtasks.iter().enumerate() {
            println!("  {}. {} [{:?}]", i + 1, sub.title, sub.complexity);
            if !sub.acceptance_criteria.is_empty() {
                for ac in &sub.acceptance_criteria {
                    let mark = if ac.met { "✅" } else { "⬜" };
                    println!("     {mark} {}", ac.description);
                }
            }
        }

        if !spec.subtasks.is_empty() {
            match surge_spec::DependencyGraph::from_spec(spec) {
                Ok(graph) => {
                    println!("\nDependency Graph:");
                    println!("{}", graph.to_ascii(spec));
                }
                Err(e) => println!("\nGraph error: {e}"),
            }
        }
    }
    SpecCommands::Validate { id } => {
        let spec_file = load_spec_by_id(&id)?;
        let result = surge_spec::validate_spec(&spec_file.spec);

        if result.is_ok() {
            println!("✅ Spec '{}' is valid", spec_file.spec.title);
            for w in &result.warnings {
                println!("   ⚠️  {w}");
            }
        } else {
            println!("❌ Spec '{}' has errors:", spec_file.spec.title);
            for e in &result.errors {
                println!("   ❌ {e}");
            }
            for w in &result.warnings {
                println!("   ⚠️  {w}");
            }
            std::process::exit(1);
        }
    }
},
```

**Step 3: Add helper function**

Add this helper function above `main()`:

```rust
/// Load a spec by ID or filename.
fn load_spec_by_id(id: &str) -> anyhow::Result<surge_spec::SpecFile> {
    // Try as a direct file path first
    let path = std::path::Path::new(id);
    if path.exists() {
        return Ok(surge_spec::SpecFile::load(path)?);
    }

    // Try in .surge/specs/ directory
    let specs_dir = surge_spec::SpecFile::specs_dir()?;

    // Try with .toml extension
    let with_ext = specs_dir.join(format!("{id}.toml"));
    if with_ext.exists() {
        return Ok(surge_spec::SpecFile::load(&with_ext)?);
    }

    // Search by ID prefix in specs directory
    let specs = surge_spec::SpecFile::list_all()?;
    for (spec_path, spec_file) in specs {
        if spec_file.spec.id.to_string().contains(id) {
            return Ok(surge_spec::SpecFile::load(&spec_path)?);
        }
    }

    anyhow::bail!("Spec '{}' not found. Check surge spec list.", id)
}
```

**Step 4: Verify compilation**

Run: `cargo check -p surge-cli`
Expected: Compiles

**Step 5: Commit**

```bash
git add crates/surge-cli/src/main.rs
git commit -m "feat(cli): add spec commands — create, list, show, validate"
```

---

### Task 8: Final verification

**Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests PASS

**Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: No warnings

**Step 3: Manual smoke test**

```bash
# Create a spec from template
cargo run -p surge-cli -- spec create "Add user authentication" --template feature

# List specs
cargo run -p surge-cli -- spec list

# Show spec (use ID from create output)
cargo run -p surge-cli -- spec show <spec-id>

# Validate spec
cargo run -p surge-cli -- spec validate <spec-id>
```

**Step 4: Commit if any fixes needed**

```bash
git add -A
git commit -m "test: Phase 1 final verification"
```

---

## Dependency Graph

```
Task 1 (scaffold) → Task 2 (parser) ──→ Task 4 (templates) ─┐
                  → Task 3 (builder) ──→ Task 4 (templates)  ├→ Task 7 (CLI) → Task 8 (verify)
                  → Task 5 (validation) ─────────────────────┤
                  → Task 6 (graph) ──────────────────────────┘
```

- Task 1 is the prerequisite for all others (crate must exist)
- Tasks 2, 3, 5, 6 can be done in any order after Task 1
- Task 4 depends on Task 2 (SpecFile) and Task 3 (builders)
- Task 7 depends on all (needs all features for CLI commands)
- Task 8 is final verification
