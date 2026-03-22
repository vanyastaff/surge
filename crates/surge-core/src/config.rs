//! Surge configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurgeConfig {
    pub default_agent: String,
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
    #[serde(default)]
    pub pipeline: PipelineConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_transport")]
    pub transport: Transport,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Stdio,
    Tcp {
        host: String,
        port: u16,
    },
}

fn default_transport() -> Transport {
    Transport::Stdio
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_max_qa_iterations")]
    pub max_qa_iterations: u32,
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
    #[serde(default)]
    pub gates: GateConfig,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_qa_iterations: default_max_qa_iterations(),
            max_parallel: default_max_parallel(),
            gates: GateConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    #[serde(default = "default_true")]
    pub after_spec: bool,
    #[serde(default = "default_true")]
    pub after_plan: bool,
    #[serde(default)]
    pub after_each_subtask: bool,
    #[serde(default = "default_true")]
    pub after_qa: bool,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            after_spec: true,
            after_plan: true,
            after_each_subtask: false,
            after_qa: true,
        }
    }
}

fn default_max_qa_iterations() -> u32 {
    10
}
fn default_max_parallel() -> usize {
    3
}
fn default_true() -> bool {
    true
}

impl SurgeConfig {
    /// Load config from a TOML file at the given path.
    pub fn load(path: &PathBuf) -> Result<Self, crate::SurgeError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::SurgeError::Config(format!("Failed to read {}: {e}", path.display())))?;
        toml::from_str(&content)
            .map_err(|e| crate::SurgeError::Config(format!("Failed to parse {}: {e}", path.display())))
    }

    /// Discover surge.toml by searching current directory and parent directories.
    pub fn discover() -> Result<Self, crate::SurgeError> {
        let start_dir = std::env::current_dir()
            .map_err(|e| crate::SurgeError::Config(format!("Failed to get current directory: {e}")))?;

        let config_path = Self::find_config_file(&start_dir)?;
        Self::load(&config_path)
    }

    /// Find surge.toml by walking up from the given directory.
    fn find_config_file(start_dir: &Path) -> Result<PathBuf, crate::SurgeError> {
        let mut current = start_dir;

        loop {
            let candidate = current.join("surge.toml");
            if candidate.exists() {
                return Ok(candidate);
            }

            // Move to parent directory
            match current.parent() {
                Some(parent) => current = parent,
                None => {
                    return Err(crate::SurgeError::Config(
                        format!("surge.toml not found in {} or any parent directory", start_dir.display())
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_config_discovery() {
        // Create a temporary directory structure
        let temp_dir = std::env::temp_dir().join("surge_test_discovery");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        let nested_dir = temp_dir.join("subdir").join("nested");
        fs::create_dir_all(&nested_dir).unwrap();

        // Create a surge.toml in the temp_dir
        let config_path = temp_dir.join("surge.toml");
        fs::write(&config_path, r#"
default_agent = "test-agent"

[agents.test-agent]
command = "test"
"#).unwrap();

        // Test finding from nested directory
        let found_path = SurgeConfig::find_config_file(&nested_dir).unwrap();
        assert_eq!(found_path, config_path);

        // Test finding from the directory containing surge.toml
        let found_path = SurgeConfig::find_config_file(&temp_dir).unwrap();
        assert_eq!(found_path, config_path);

        // Test error when not found
        let non_existent_dir = std::env::temp_dir().join("surge_test_no_config");
        fs::create_dir_all(&non_existent_dir).unwrap();
        let result = SurgeConfig::find_config_file(&non_existent_dir);
        assert!(result.is_err());

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_dir_all(&non_existent_dir);
    }
}
