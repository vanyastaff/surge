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
