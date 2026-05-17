//! MCP server reference types — the run-level registry of MCP server
//! definitions. Per-stage `ToolOverride::mcp_add` then references
//! these by name.

use crate::sandbox::SandboxMode;
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
    #[serde(
        default = "McpServerRef::default_call_timeout",
        with = "humantime_serde"
    )]
    pub call_timeout: Duration,
    /// Whether the engine should re-spawn the server child process if
    /// it exits while still configured. Default true.
    #[serde(default = "McpServerRef::default_restart_on_crash")]
    pub restart_on_crash: bool,
    /// Per-server sandbox intent override. `None` (default) inherits
    /// the run's sandbox mode. Resolved at one canonical site
    /// (`mcp_spawn_policy`): `ReadOnly` denies MCP entirely; deeper
    /// OS-level enforcement of the child is delegated to the runtime
    /// per ADR-0006 (see ADR-0014).
    #[serde(default)]
    pub sandbox: Option<SandboxMode>,
}

impl McpServerRef {
    /// Construct with explicit values for every field.
    ///
    /// Provided so external crates can create instances despite
    /// `#[non_exhaustive]` being in effect.
    #[must_use]
    pub fn new(
        name: String,
        transport: McpTransportConfig,
        allowed_tools: Option<Vec<String>>,
        call_timeout: Duration,
        restart_on_crash: bool,
    ) -> Self {
        Self {
            name,
            transport,
            allowed_tools,
            call_timeout,
            restart_on_crash,
            sandbox: None,
        }
    }

    /// Builder-style setter for the per-server sandbox override.
    /// Provided so existing `new(..)` call sites keep compiling despite
    /// the added field (`#[non_exhaustive]`).
    #[must_use]
    pub fn with_sandbox(mut self, sandbox: Option<SandboxMode>) -> Self {
        self.sandbox = sandbox;
        self
    }

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

impl McpTransportConfig {
    /// Construct a `Stdio` variant with explicit values.
    ///
    /// Provided so external crates can create instances despite
    /// `#[non_exhaustive]` being in effect.
    #[must_use]
    pub fn stdio(command: PathBuf, args: Vec<String>, env: HashMap<String, String>) -> Self {
        Self::Stdio { command, args, env }
    }
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
            sandbox: None,
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
        // New field defaults to None (inherit run intent) and is
        // back-compatible with configs written before it existed.
        assert_eq!(r.sandbox, None);
    }

    #[test]
    fn sandbox_override_roundtrips_and_setter_works() {
        let r = McpServerRef::new(
            "fs".into(),
            McpTransportConfig::stdio(PathBuf::from("npx"), vec![], HashMap::new()),
            None,
            Duration::from_secs(60),
            true,
        )
        .with_sandbox(Some(SandboxMode::ReadOnly));
        assert_eq!(r.sandbox, Some(SandboxMode::ReadOnly));
        let s = toml::to_string(&r).unwrap();
        let parsed: McpServerRef = toml::from_str(&s).unwrap();
        assert_eq!(r, parsed);
        assert_eq!(parsed.sandbox, Some(SandboxMode::ReadOnly));
    }
}
