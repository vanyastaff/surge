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
    /// Maximum time from child spawn to a completed MCP `initialize`
    /// handshake. Independent of `call_timeout`: the handshake includes
    /// process and interpreter startup, which a per-RPC budget does not
    /// model. `None` (the default) resolves through
    /// [`Self::effective_startup_timeout`]. Omitted from serialization when
    /// `None`, so snapshots written before this field existed keep their
    /// exact bytes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "humantime_serde"
    )]
    pub startup_timeout: Option<Duration>,
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
            startup_timeout: None,
            restart_on_crash,
            sandbox: None,
        }
    }

    /// Floor of the startup deadline when `startup_timeout` is unset.
    pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

    /// Builder-style setter for the explicit startup deadline.
    #[must_use]
    pub fn with_startup_timeout(mut self, startup_timeout: Option<Duration>) -> Self {
        self.startup_timeout = startup_timeout;
        self
    }

    /// Deadline for spawn plus the MCP `initialize` handshake.
    ///
    /// An explicit `startup_timeout` is used exactly. When unset, the
    /// deadline is the larger of [`Self::DEFAULT_STARTUP_TIMEOUT`] and
    /// `call_timeout`, so no configuration gets a shorter handshake than
    /// it had when the handshake shared `call_timeout`.
    #[must_use]
    pub fn effective_startup_timeout(&self) -> Duration {
        self.startup_timeout
            .unwrap_or_else(|| Self::DEFAULT_STARTUP_TIMEOUT.max(self.call_timeout))
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
            startup_timeout: Some(Duration::from_secs(45)),
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
        assert_eq!(r.startup_timeout, None);
    }

    #[test]
    fn unset_startup_timeout_never_shortens_the_legacy_handshake() {
        let server = |call: Duration| {
            McpServerRef::new(
                "s".into(),
                McpTransportConfig::stdio(PathBuf::from("python3"), vec![], HashMap::new()),
                None,
                call,
                true,
            )
        };
        let short = server(Duration::from_millis(275));
        assert_eq!(
            short.effective_startup_timeout(),
            McpServerRef::DEFAULT_STARTUP_TIMEOUT
        );
        let long = server(Duration::from_secs(120));
        assert_eq!(long.effective_startup_timeout(), Duration::from_secs(120));
        let explicit = short.with_startup_timeout(Some(Duration::from_millis(50)));
        assert_eq!(
            explicit.effective_startup_timeout(),
            Duration::from_millis(50)
        );
    }

    #[test]
    fn unset_startup_timeout_is_absent_from_serialized_bytes() {
        let r = McpServerRef::new(
            "s".into(),
            McpTransportConfig::stdio(PathBuf::from("python3"), vec![], HashMap::new()),
            None,
            Duration::from_secs(60),
            true,
        );
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("startup_timeout").is_none());
        let set = r.with_startup_timeout(Some(Duration::from_secs(90)));
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["startup_timeout"], "1m 30s");
        assert_eq!(serde_json::from_value::<McpServerRef>(json).unwrap(), set);
        let toml_text = toml::to_string(&set).unwrap();
        assert_eq!(toml::from_str::<McpServerRef>(&toml_text).unwrap(), set);
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
