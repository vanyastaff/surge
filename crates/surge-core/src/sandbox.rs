//! Sandbox configuration for nodes and profiles.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SandboxConfig {
    pub mode: SandboxMode,
    #[serde(default)]
    pub writable_roots: Vec<PathBuf>,
    #[serde(default)]
    pub network_allowlist: Vec<String>,
    #[serde(default)]
    pub shell_allowlist: Vec<String>,
    #[serde(default)]
    pub protected_paths: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::WorkspaceWrite,
            writable_roots: Vec::new(),
            network_allowlist: Vec::new(),
            shell_allowlist: Vec::new(),
            protected_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    WorkspaceNetwork,
    FullAccess,
    Custom,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_workspace_write() {
        let cfg = SandboxConfig::default();
        assert_eq!(cfg.mode, SandboxMode::WorkspaceWrite);
        assert!(cfg.network_allowlist.is_empty());
    }

    #[test]
    fn mode_serializes_kebab_case() {
        let json = serde_json::json!(SandboxMode::WorkspaceNetwork);
        assert_eq!(json, "workspace-network");
    }

    #[test]
    fn config_toml_roundtrip() {
        let original = SandboxConfig {
            mode: SandboxMode::WorkspaceWrite,
            writable_roots: vec![PathBuf::from("/tmp/work")],
            network_allowlist: vec!["crates.io".into()],
            shell_allowlist: vec!["cargo".into()],
            protected_paths: vec![".git".into()],
        };
        let toml_s = toml::to_string(&original).unwrap();
        let parsed: SandboxConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(original, parsed);
    }
}
