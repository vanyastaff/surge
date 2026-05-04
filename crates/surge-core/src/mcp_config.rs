//! MCP server reference types — the run-level registry of MCP server
//! definitions. Per-stage `ToolOverride::mcp_add` then references
//! these by name.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Run-level definition of a single MCP server.
///
/// `name` identifies the server in `ToolOverride::mcp_add` allowlists.
/// `transport` describes how the engine spawns / connects to it.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerRef {
    /// Identifier referenced from per-stage allowlists.
    pub name: String,
    /// How the engine reaches this server.
    pub transport: McpTransportConfig,
    /// Optional whitelist of tool names. If `None`, all tools the
    /// server reports via `tools/list` are exposed.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Maximum time a single `tools/call` may take. Default 60 s.
    #[serde(default = "McpServerRef::default_call_timeout", with = "humantime_serde")]
    pub call_timeout: Duration,
    /// Whether the engine should re-spawn the server child process if
    /// it exits while still configured. Default true.
    #[serde(default = "McpServerRef::default_restart_on_crash")]
    pub restart_on_crash: bool,
}

impl McpServerRef {
    fn default_call_timeout() -> Duration {
        Duration::from_secs(60)
    }
    fn default_restart_on_crash() -> bool {
        true
    }
}

/// How a `surge` engine reaches an MCP server. M7 supports stdio
/// child-process only.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportConfig {
    /// Spawn `command args` and talk MCP over its stdio.
    Stdio {
        command: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_server_ref_toml_roundtrips() {
        let r = McpServerRef {
            name: "playwright".into(),
            transport: McpTransportConfig::Stdio {
                command: PathBuf::from("/usr/local/bin/mcp-playwright"),
                args: vec!["--headless".into()],
                env: HashMap::new(),
            },
            allowed_tools: Some(vec!["browser_navigate".into()]),
            call_timeout: Duration::from_secs(120),
            restart_on_crash: true,
        };
        let s = toml::to_string(&r).unwrap();
        let parsed: McpServerRef = toml::from_str(&s).unwrap();
        assert_eq!(r, parsed);
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let s = r#"
            name = "github"
            transport = { kind = "stdio", command = "npx", args = ["@github/mcp-server"] }
        "#;
        let r: McpServerRef = toml::from_str(s).unwrap();
        assert_eq!(r.allowed_tools, None);
        assert_eq!(r.call_timeout, Duration::from_secs(60));
        assert!(r.restart_on_crash);
    }
}
