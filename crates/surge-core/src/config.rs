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

impl AgentConfig {
    /// Validate the agent configuration.
    fn validate(&self, agent_name: &str) -> Result<(), crate::SurgeError> {
        // Validate command is not empty
        if self.command.trim().is_empty() {
            return Err(crate::SurgeError::Config(format!(
                "Agent '{}' has empty command. Command must be a non-empty string",
                agent_name
            )));
        }

        // Validate TCP transport has non-empty host
        if let Transport::Tcp { host, port } = &self.transport {
            if host.trim().is_empty() {
                return Err(crate::SurgeError::Config(format!(
                    "Agent '{}' TCP transport has empty host. Host must be a non-empty string",
                    agent_name
                )));
            }
            if *port == 0 {
                return Err(crate::SurgeError::Config(format!(
                    "Agent '{}' TCP transport has invalid port 0. Port must be between 1 and 65535",
                    agent_name
                )));
            }
        }

        Ok(())
    }
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

impl PipelineConfig {
    /// Validate the pipeline configuration.
    fn validate(&self) -> Result<(), crate::SurgeError> {
        // Validate max_qa_iterations is positive
        if self.max_qa_iterations == 0 {
            return Err(crate::SurgeError::Config(
                "pipeline.max_qa_iterations must be greater than 0".to_string()
            ));
        }

        // Validate max_parallel is positive
        if self.max_parallel == 0 {
            return Err(crate::SurgeError::Config(
                "pipeline.max_parallel must be greater than 0".to_string()
            ));
        }

        Ok(())
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

impl Default for SurgeConfig {
    fn default() -> Self {
        Self {
            default_agent: "claude-code".to_string(),
            agents: HashMap::new(),
            pipeline: PipelineConfig::default(),
        }
    }
}

impl SurgeConfig {
    /// Load config from a TOML file at the given path.
    pub fn load(path: &PathBuf) -> Result<Self, crate::SurgeError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::SurgeError::Config(format!("Failed to read {}: {e}", path.display())))?;
        let config: Self = toml::from_str(&content)
            .map_err(|e| crate::SurgeError::Config(format!("Failed to parse {}: {e}", path.display())))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration and return helpful error messages.
    pub fn validate(&self) -> Result<(), crate::SurgeError> {
        // Validate default_agent exists in agents map (when agents are configured)
        if !self.agents.is_empty() && !self.agents.contains_key(&self.default_agent) {
            return Err(crate::SurgeError::Config(format!(
                "default_agent '{}' not found in agents. Available agents: {}",
                self.default_agent,
                self.agents.keys().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
            )));
        }

        // Validate each agent configuration
        for (name, agent) in &self.agents {
            agent.validate(name)?;
        }

        // Validate pipeline configuration
        self.pipeline.validate()?;

        Ok(())
    }

    /// Discover surge.toml by searching current directory and parent directories.
    /// Returns a default configuration if no file is found.
    pub fn discover() -> Result<Self, crate::SurgeError> {
        let start_dir = std::env::current_dir()
            .map_err(|e| crate::SurgeError::Config(format!("Failed to get current directory: {e}")))?;

        match Self::find_config_file(&start_dir) {
            Ok(config_path) => Self::load(&config_path),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Apply environment variable overrides to the configuration.
    /// Environment variables with the SURGE_* prefix override config values:
    /// - SURGE_DEFAULT_AGENT
    /// - SURGE_MAX_QA_ITERATIONS
    /// - SURGE_MAX_PARALLEL
    /// - SURGE_GATE_AFTER_SPEC
    /// - SURGE_GATE_AFTER_PLAN
    /// - SURGE_GATE_AFTER_EACH_SUBTASK
    /// - SURGE_GATE_AFTER_QA
    pub fn apply_env_overrides(&mut self) {
        // Override default_agent
        if let Ok(value) = std::env::var("SURGE_DEFAULT_AGENT") {
            self.default_agent = value;
        }

        // Override pipeline.max_qa_iterations
        if let Ok(value) = std::env::var("SURGE_MAX_QA_ITERATIONS") {
            if let Ok(parsed) = value.parse::<u32>() {
                self.pipeline.max_qa_iterations = parsed;
            }
        }

        // Override pipeline.max_parallel
        if let Ok(value) = std::env::var("SURGE_MAX_PARALLEL") {
            if let Ok(parsed) = value.parse::<usize>() {
                self.pipeline.max_parallel = parsed;
            }
        }

        // Override pipeline.gates.after_spec
        if let Ok(value) = std::env::var("SURGE_GATE_AFTER_SPEC") {
            if let Ok(parsed) = value.parse::<bool>() {
                self.pipeline.gates.after_spec = parsed;
            }
        }

        // Override pipeline.gates.after_plan
        if let Ok(value) = std::env::var("SURGE_GATE_AFTER_PLAN") {
            if let Ok(parsed) = value.parse::<bool>() {
                self.pipeline.gates.after_plan = parsed;
            }
        }

        // Override pipeline.gates.after_each_subtask
        if let Ok(value) = std::env::var("SURGE_GATE_AFTER_EACH_SUBTASK") {
            if let Ok(parsed) = value.parse::<bool>() {
                self.pipeline.gates.after_each_subtask = parsed;
            }
        }

        // Override pipeline.gates.after_qa
        if let Ok(value) = std::env::var("SURGE_GATE_AFTER_QA") {
            if let Ok(parsed) = value.parse::<bool>() {
                self.pipeline.gates.after_qa = parsed;
            }
        }
    }

    /// Load config by discovering surge.toml, or return default if not found.
    /// This combines discovery and default fallback in a single convenient method.
    pub fn load_or_default() -> Result<Self, crate::SurgeError> {
        Self::discover()
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

    #[test]
    fn test_default_config() {
        // Test that Default provides sensible values
        let config = SurgeConfig::default();

        assert_eq!(config.default_agent, "claude-code");
        assert!(config.agents.is_empty());
        assert_eq!(config.pipeline.max_qa_iterations, 10);
        assert_eq!(config.pipeline.max_parallel, 3);
        assert!(config.pipeline.gates.after_spec);
        assert!(config.pipeline.gates.after_plan);
        assert!(!config.pipeline.gates.after_each_subtask);
        assert!(config.pipeline.gates.after_qa);
    }

    #[test]
    fn test_load_or_default() {
        // Create a temporary directory structure
        let temp_dir = std::env::temp_dir().join("surge_test_load_or_default");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        // Test 1: When surge.toml exists, it should load it
        let config_path = temp_dir.join("surge.toml");
        fs::write(&config_path, r#"
default_agent = "custom-agent"

[pipeline]
max_qa_iterations = 5
max_parallel = 2
"#).unwrap();

        // Change to the temp directory to test load_or_default
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        let config = SurgeConfig::load_or_default().unwrap();
        assert_eq!(config.default_agent, "custom-agent");
        assert_eq!(config.pipeline.max_qa_iterations, 5);
        assert_eq!(config.pipeline.max_parallel, 2);

        // Test 2: When no surge.toml exists, it should return default
        let no_config_dir = std::env::temp_dir().join("surge_test_load_or_default_no_config");
        let _ = fs::remove_dir_all(&no_config_dir);
        fs::create_dir_all(&no_config_dir).unwrap();
        std::env::set_current_dir(&no_config_dir).unwrap();

        let config = SurgeConfig::load_or_default().unwrap();
        assert_eq!(config.default_agent, "claude-code");
        assert_eq!(config.pipeline.max_qa_iterations, 10);
        assert_eq!(config.pipeline.max_parallel, 3);

        // Restore original directory
        std::env::set_current_dir(&original_dir).unwrap();

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_dir_all(&no_config_dir);
    }

    #[test]
    fn test_config_validation() {
        // Test 1: Valid configuration passes validation
        let mut valid_config = SurgeConfig::default();
        valid_config.agents.insert("claude-code".to_string(), AgentConfig {
            command: "claude".to_string(),
            args: vec![],
            transport: Transport::Stdio,
        });
        assert!(valid_config.validate().is_ok());

        // Test 2: default_agent not in agents map fails
        let mut invalid_config = SurgeConfig::default();
        invalid_config.default_agent = "nonexistent".to_string();
        invalid_config.agents.insert("other-agent".to_string(), AgentConfig {
            command: "other".to_string(),
            args: vec![],
            transport: Transport::Stdio,
        });
        let result = invalid_config.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("default_agent 'nonexistent' not found"));
        assert!(err_msg.contains("Available agents: other-agent"));

        // Test 3: Empty command fails
        let mut config_empty_cmd = SurgeConfig::default();
        config_empty_cmd.default_agent = "bad-agent".to_string();
        config_empty_cmd.agents.insert("bad-agent".to_string(), AgentConfig {
            command: "".to_string(),
            args: vec![],
            transport: Transport::Stdio,
        });
        let result = config_empty_cmd.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Agent 'bad-agent' has empty command"));

        // Test 4: Empty TCP host fails
        let mut config_empty_host = SurgeConfig::default();
        config_empty_host.default_agent = "tcp-agent".to_string();
        config_empty_host.agents.insert("tcp-agent".to_string(), AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "".to_string(),
                port: 8080,
            },
        });
        let result = config_empty_host.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Agent 'tcp-agent' TCP transport has empty host"));

        // Test 5: Invalid TCP port 0 fails
        let mut config_invalid_port = SurgeConfig::default();
        config_invalid_port.default_agent = "tcp-agent".to_string();
        config_invalid_port.agents.insert("tcp-agent".to_string(), AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "localhost".to_string(),
                port: 0,
            },
        });
        let result = config_invalid_port.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Agent 'tcp-agent' TCP transport has invalid port 0"));

        // Test 6: max_qa_iterations = 0 fails
        let mut config_zero_qa = SurgeConfig::default();
        config_zero_qa.pipeline.max_qa_iterations = 0;
        let result = config_zero_qa.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("pipeline.max_qa_iterations must be greater than 0"));

        // Test 7: max_parallel = 0 fails
        let mut config_zero_parallel = SurgeConfig::default();
        config_zero_parallel.pipeline.max_parallel = 0;
        let result = config_zero_parallel.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("pipeline.max_parallel must be greater than 0"));

        // Test 8: Valid TCP configuration passes
        let mut config_valid_tcp = SurgeConfig::default();
        config_valid_tcp.default_agent = "tcp-agent".to_string();
        config_valid_tcp.agents.insert("tcp-agent".to_string(), AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "localhost".to_string(),
                port: 8080,
            },
        });
        assert!(config_valid_tcp.validate().is_ok());

        // Test 9: Empty agents map with default_agent is OK (default config scenario)
        let default_config = SurgeConfig::default();
        assert!(default_config.validate().is_ok());
    }

    #[test]
    fn test_env_overrides() {
        // Test environment variable overrides
        // Set environment variables
        unsafe {
            std::env::set_var("SURGE_DEFAULT_AGENT", "custom-agent");
            std::env::set_var("SURGE_MAX_QA_ITERATIONS", "20");
            std::env::set_var("SURGE_MAX_PARALLEL", "5");
            std::env::set_var("SURGE_GATE_AFTER_SPEC", "false");
            std::env::set_var("SURGE_GATE_AFTER_PLAN", "false");
            std::env::set_var("SURGE_GATE_AFTER_EACH_SUBTASK", "true");
            std::env::set_var("SURGE_GATE_AFTER_QA", "false");
        }

        // Create config with defaults
        let mut config = SurgeConfig::default();

        // Verify defaults before override
        assert_eq!(config.default_agent, "claude-code");
        assert_eq!(config.pipeline.max_qa_iterations, 10);
        assert_eq!(config.pipeline.max_parallel, 3);
        assert!(config.pipeline.gates.after_spec);
        assert!(config.pipeline.gates.after_plan);
        assert!(!config.pipeline.gates.after_each_subtask);
        assert!(config.pipeline.gates.after_qa);

        // Apply environment overrides
        config.apply_env_overrides();

        // Verify overrides were applied
        assert_eq!(config.default_agent, "custom-agent");
        assert_eq!(config.pipeline.max_qa_iterations, 20);
        assert_eq!(config.pipeline.max_parallel, 5);
        assert!(!config.pipeline.gates.after_spec);
        assert!(!config.pipeline.gates.after_plan);
        assert!(config.pipeline.gates.after_each_subtask);
        assert!(!config.pipeline.gates.after_qa);

        // Clean up environment variables
        unsafe {
            std::env::remove_var("SURGE_DEFAULT_AGENT");
            std::env::remove_var("SURGE_MAX_QA_ITERATIONS");
            std::env::remove_var("SURGE_MAX_PARALLEL");
            std::env::remove_var("SURGE_GATE_AFTER_SPEC");
            std::env::remove_var("SURGE_GATE_AFTER_PLAN");
            std::env::remove_var("SURGE_GATE_AFTER_EACH_SUBTASK");
            std::env::remove_var("SURGE_GATE_AFTER_QA");
        }

        // Test that invalid values are ignored
        unsafe {
            std::env::set_var("SURGE_MAX_QA_ITERATIONS", "invalid");
            std::env::set_var("SURGE_MAX_PARALLEL", "not-a-number");
            std::env::set_var("SURGE_GATE_AFTER_SPEC", "not-a-bool");
        }

        let mut config2 = SurgeConfig::default();
        config2.apply_env_overrides();

        // Verify invalid values were ignored (defaults remain)
        assert_eq!(config2.pipeline.max_qa_iterations, 10);
        assert_eq!(config2.pipeline.max_parallel, 3);
        assert!(config2.pipeline.gates.after_spec);

        // Clean up
        unsafe {
            std::env::remove_var("SURGE_MAX_QA_ITERATIONS");
            std::env::remove_var("SURGE_MAX_PARALLEL");
            std::env::remove_var("SURGE_GATE_AFTER_SPEC");
        }
    }
}
