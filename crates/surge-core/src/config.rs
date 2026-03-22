//! Surge configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
}
