//! Spec file parsing and I/O.

use std::path::{Path, PathBuf};
use surge_core::id::SubtaskId;
use surge_core::spec::{Spec, SubtaskState};
use surge_core::SurgeError;

/// On-disk spec file wrapping a Spec with file metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpecFile {
    /// The spec definition.
    pub spec: Spec,
    /// Path where this spec is stored. None if not yet saved.
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl SpecFile {
    /// Load a spec from a TOML file.
    pub fn load(path: &Path) -> Result<Self, SurgeError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| SurgeError::Spec(format!("Failed to read {}: {e}", path.display())))?;
        let mut spec_file = Self::from_toml(&content)?;
        spec_file.path = Some(path.to_path_buf());
        Ok(spec_file)
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

    /// Save to the stored path, or error if not set.
    pub fn save_in_place(&self) -> Result<(), SurgeError> {
        let path = self.path.as_ref().ok_or_else(|| {
            SurgeError::Spec("SpecFile has no path — use save(path)".to_string())
        })?;
        self.save(path)
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

    /// Update the state of a specific subtask and save to disk.
    pub fn update_subtask_state(
        &mut self,
        path: &Path,
        subtask_id: SubtaskId,
        state: SubtaskState,
    ) -> Result<(), SurgeError> {
        let subtask = self
            .spec
            .subtasks
            .iter_mut()
            .find(|s| s.id == subtask_id)
            .ok_or_else(|| SurgeError::Spec(format!("Subtask {subtask_id} not found")))?;
        subtask.execution.state = state;
        self.save(path)
    }

    /// Reorder a subtask to a new index.
    ///
    /// Validates the spec after reordering. If validation fails, the original
    /// order is restored and an error is returned.
    pub fn reorder_subtask(
        &mut self,
        subtask_id: SubtaskId,
        new_index: usize,
    ) -> Result<(), SurgeError> {
        let current_index = self
            .spec
            .subtasks
            .iter()
            .position(|s| s.id == subtask_id)
            .ok_or_else(|| SurgeError::Spec(format!("Subtask {subtask_id} not found")))?;

        let max_index = self.spec.subtasks.len().saturating_sub(1);
        let new_index = new_index.min(max_index);

        if current_index == new_index {
            return Ok(());
        }

        let subtask = self.spec.subtasks.remove(current_index);
        self.spec.subtasks.insert(new_index, subtask);

        let validation = crate::validation::validate(&self.spec);
        if !validation.is_ok() {
            // rollback
            let moved = self.spec.subtasks.remove(new_index);
            self.spec.subtasks.insert(current_index, moved);
            return Err(SurgeError::Spec(format!(
                "Reorder violates spec constraints: {}",
                validation.errors.join(", ")
            )));
        }

        Ok(())
    }

    /// Cancel a subtask by marking it as skipped.
    pub fn cancel_subtask(&mut self, subtask_id: SubtaskId) -> Result<(), SurgeError> {
        let subtask = self
            .spec
            .subtasks
            .iter_mut()
            .find(|s| s.id == subtask_id)
            .ok_or_else(|| SurgeError::Spec(format!("Subtask {subtask_id} not found")))?;
        subtask.execution.state = SubtaskState::Skipped;
        Ok(())
    }

    /// Update a subtask's title and/or description.
    pub fn update_subtask(
        &mut self,
        subtask_id: SubtaskId,
        title: Option<String>,
        description: Option<String>,
    ) -> Result<(), SurgeError> {
        let subtask = self
            .spec
            .subtasks
            .iter_mut()
            .find(|s| s.id == subtask_id)
            .ok_or_else(|| SurgeError::Spec(format!("Subtask {subtask_id} not found")))?;
        if let Some(t) = title {
            subtask.title = t;
        }
        if let Some(d) = description {
            subtask.description = d;
        }
        Ok(())
    }

    /// Insert a subtask after the given subtask ID, or at the start if `after` is None.
    pub fn insert_subtask(
        &mut self,
        subtask: surge_core::spec::Subtask,
        after: Option<SubtaskId>,
    ) -> Result<(), SurgeError> {
        let index = match after {
            None => 0,
            Some(id) => self
                .spec
                .subtasks
                .iter()
                .position(|s| s.id == id)
                .map(|i| i + 1)
                .ok_or_else(|| SurgeError::Spec(format!("Subtask {id} not found")))?,
        };
        self.spec.subtasks.insert(index, subtask);
        Ok(())
    }

    /// Remove a subtask.
    ///
    /// Fails if other subtasks depend on it. The original state is restored on error.
    pub fn remove_subtask(&mut self, subtask_id: SubtaskId) -> Result<(), SurgeError> {
        let index = self
            .spec
            .subtasks
            .iter()
            .position(|s| s.id == subtask_id)
            .ok_or_else(|| SurgeError::Spec(format!("Subtask {subtask_id} not found")))?;

        let removed = self.spec.subtasks.remove(index);

        let validation = crate::validation::validate(&self.spec);
        if !validation.is_ok() {
            // rollback
            self.spec.subtasks.insert(index, removed);
            return Err(SurgeError::Spec(format!(
                "Cannot remove subtask: {}",
                validation.errors.join(", ")
            )));
        }

        Ok(())
    }

    /// Mark an acceptance criterion as met and save to disk.
    pub fn mark_criterion_met(
        &mut self,
        path: &Path,
        subtask_id: SubtaskId,
        criterion_index: usize,
    ) -> Result<(), SurgeError> {
        let subtask = self
            .spec
            .subtasks
            .iter_mut()
            .find(|s| s.id == subtask_id)
            .ok_or_else(|| SurgeError::Spec(format!("Subtask {subtask_id} not found")))?;
        if let Some(ac) = subtask.acceptance_criteria.get_mut(criterion_index) {
            ac.met = true;
        }
        self.save(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::{SpecId, SubtaskId};
    use surge_core::spec::{AcceptanceCriteria, Complexity, Subtask, SubtaskExecution};
    use crate::builder::SubtaskBuilder;

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
                    acceptance_criteria: vec![AcceptanceCriteria {
                        description: "Compiles".to_string(),
                        met: false,
                    }],
                    depends_on: vec![],
                    story_file: None,
                    agent: None,
                    execution: SubtaskExecution::default(),
                },
                Subtask {
                    id: SubtaskId::new(),
                    title: "Second step".to_string(),
                    description: "Do the second thing".to_string(),
                    complexity: Complexity::Simple,
                    files: vec![],
                    acceptance_criteria: vec![],
                    depends_on: vec![sub1_id],
                    story_file: None,
                    agent: None,
                    execution: SubtaskExecution::default(),
                },
            ],
        }
    }

    #[test]
    fn test_specfile_toml_roundtrip() {
        let spec_file = SpecFile { spec: sample_spec(), path: None };
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
        let spec_file = SpecFile { spec: sample_spec(), path: None };

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

    #[test]
    fn test_path_set_after_load() {
        let temp_dir = std::env::temp_dir().join("surge_test_spec_path");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let path = temp_dir.join("path-test.toml");
        let spec_file = SpecFile { spec: sample_spec(), path: None };
        spec_file.save(&path).unwrap();

        let loaded = SpecFile::load(&path).unwrap();
        assert_eq!(loaded.path.as_deref(), Some(path.as_path()));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_save_in_place() {
        let temp_dir = std::env::temp_dir().join("surge_test_spec_in_place");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let path = temp_dir.join("in-place.toml");
        let spec_file = SpecFile { spec: sample_spec(), path: None };
        spec_file.save(&path).unwrap();

        let mut loaded = SpecFile::load(&path).unwrap();
        loaded.spec.title = "Updated title".to_string();
        loaded.save_in_place().unwrap();

        let reloaded = SpecFile::load(&path).unwrap();
        assert_eq!(reloaded.spec.title, "Updated title");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_save_in_place_no_path_errors() {
        let spec_file = SpecFile { spec: sample_spec(), path: None };
        assert!(spec_file.save_in_place().is_err());
    }

    #[test]
    fn test_update_subtask_state_roundtrip() {
        let temp_dir = std::env::temp_dir().join("surge_test_subtask_state");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let path = temp_dir.join("state-test.toml");
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let subtask_id = spec_file.spec.subtasks[0].id;

        spec_file.update_subtask_state(&path, subtask_id, SubtaskState::Running).unwrap();

        let loaded = SpecFile::load(&path).unwrap();
        assert_eq!(loaded.spec.subtasks[0].execution.state, SubtaskState::Running);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_update_subtask_state_unknown_id_errors() {
        let temp_dir = std::env::temp_dir().join("surge_test_subtask_state_err");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let path = temp_dir.join("state-err.toml");
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let fake_id = SubtaskId::new();

        let result = spec_file.update_subtask_state(&path, fake_id, SubtaskState::Running);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_reorder_subtask() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let first_id = spec_file.spec.subtasks[0].id;
        let second_id = spec_file.spec.subtasks[1].id;

        spec_file.reorder_subtask(first_id, 1).unwrap();
        assert_eq!(spec_file.spec.subtasks[0].id, second_id);
        assert_eq!(spec_file.spec.subtasks[1].id, first_id);
    }

    #[test]
    fn test_reorder_subtask_noop_same_index() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let first_id = spec_file.spec.subtasks[0].id;
        spec_file.reorder_subtask(first_id, 0).unwrap();
        assert_eq!(spec_file.spec.subtasks[0].id, first_id);
    }

    #[test]
    fn test_reorder_subtask_unknown_id_errors() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        assert!(spec_file.reorder_subtask(SubtaskId::new(), 0).is_err());
    }

    #[test]
    fn test_cancel_subtask() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let id = spec_file.spec.subtasks[0].id;
        spec_file.cancel_subtask(id).unwrap();
        assert_eq!(spec_file.spec.subtasks[0].execution.state, SubtaskState::Skipped);
    }

    #[test]
    fn test_cancel_subtask_unknown_id_errors() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        assert!(spec_file.cancel_subtask(SubtaskId::new()).is_err());
    }

    #[test]
    fn test_update_subtask_title_and_description() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let id = spec_file.spec.subtasks[0].id;
        spec_file
            .update_subtask(id, Some("New title".to_string()), Some("New desc".to_string()))
            .unwrap();
        assert_eq!(spec_file.spec.subtasks[0].title, "New title");
        assert_eq!(spec_file.spec.subtasks[0].description, "New desc");
    }

    #[test]
    fn test_update_subtask_partial() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let id = spec_file.spec.subtasks[0].id;
        let original_desc = spec_file.spec.subtasks[0].description.clone();
        spec_file.update_subtask(id, Some("Only title".to_string()), None).unwrap();
        assert_eq!(spec_file.spec.subtasks[0].title, "Only title");
        assert_eq!(spec_file.spec.subtasks[0].description, original_desc);
    }

    #[test]
    fn test_insert_subtask_after() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let first_id = spec_file.spec.subtasks[0].id;
        let new_sub = SubtaskBuilder::new()
            .title("Middle step")
            .description("In between")
            .build()
            .unwrap();
        let new_id = new_sub.id;

        spec_file.insert_subtask(new_sub, Some(first_id)).unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 3);
        assert_eq!(spec_file.spec.subtasks[1].id, new_id);
    }

    #[test]
    fn test_insert_subtask_at_start() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let new_sub = SubtaskBuilder::new()
            .title("First")
            .description("New first step")
            .build()
            .unwrap();
        let new_id = new_sub.id;

        spec_file.insert_subtask(new_sub, None).unwrap();
        assert_eq!(spec_file.spec.subtasks[0].id, new_id);
        assert_eq!(spec_file.spec.subtasks.len(), 3);
    }

    #[test]
    fn test_remove_subtask_no_dependents() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        // subtask[1] depends on subtask[0], so removing subtask[1] is fine
        let second_id = spec_file.spec.subtasks[1].id;
        spec_file.remove_subtask(second_id).unwrap();
        assert_eq!(spec_file.spec.subtasks.len(), 1);
    }

    #[test]
    fn test_remove_subtask_with_dependents_fails_and_rolls_back() {
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        // subtask[0] is depended on by subtask[1] — removing it should fail
        let first_id = spec_file.spec.subtasks[0].id;
        let result = spec_file.remove_subtask(first_id);
        assert!(result.is_err());
        assert_eq!(spec_file.spec.subtasks.len(), 2, "rollback should restore original state");
    }

    #[test]
    fn test_mark_criterion_met_roundtrip() {
        let temp_dir = std::env::temp_dir().join("surge_test_criterion");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let path = temp_dir.join("criterion-test.toml");
        let mut spec_file = SpecFile { spec: sample_spec(), path: None };
        let subtask_id = spec_file.spec.subtasks[0].id;

        assert!(!spec_file.spec.subtasks[0].acceptance_criteria[0].met);
        spec_file.mark_criterion_met(&path, subtask_id, 0).unwrap();

        let loaded = SpecFile::load(&path).unwrap();
        assert!(loaded.spec.subtasks[0].acceptance_criteria[0].met);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
