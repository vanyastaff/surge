//! Agent transport abstraction.
//!
//! [`AgentTransport`] decouples the ACP connection setup from the underlying
//! I/O channel. Adding a new transport (TCP, WebSocket) requires only a new
//! implementor — [`AgentConnection`] and [`AgentPool`] need no changes.
//!
//! # Current transports
//!
//! | Type | Status |
//! |---|---|
//! | [`StdioTransport`] | Production — default for all local agents |
//! | [`TcpTransport`] | Stub — returns error (Phase 7 roadmap) |
//!
//! [`AgentConnection`]: super::connection::AgentConnection

use std::path::Path;
use surge_core::SurgeError;
use surge_core::config::{AgentConfig, McpServerConfig, Transport};
use tokio::process::Child;
use tracing::{debug, info, warn};

// ── AgentIo ──────────────────────────────────────────────────────────

/// Raw I/O handles for communicating with an agent.
///
/// The reader and writer carry the framed ACP byte stream; the caller
/// ([`super::connection::AgentConnection`]) layers the ACP protocol on top.
pub struct AgentIo {
    /// Writable channel to the agent (process stdin, socket write-half, etc.).
    pub writer: Box<dyn futures::AsyncWrite + Unpin>,
    /// Readable channel from the agent (process stdout, socket read-half, etc.).
    pub reader: Box<dyn futures::AsyncRead + Unpin>,
    /// Child process handle — present only for local-process transports.
    ///
    /// [`AgentConnection`] uses this for graceful shutdown and kill.
    pub child: Option<Child>,
}

// ── Trait ────────────────────────────────────────────────────────────

/// Establishes a raw I/O connection to an agent.
///
/// Implementations may use `spawn_local` freely.
///
/// # Implementing a new transport
///
/// ```rust,ignore
/// pub struct MyTransport;
///
/// impl AgentTransport for MyTransport {
///     async fn connect(
///         name: &str,
///         config: &AgentConfig,
///         worktree_root: &Path,
///     ) -> Result<AgentIo, SurgeError> {
///         // … connect, wrap in futures-compatible reader/writer …
///         Ok(AgentIo { writer, reader, child: None })
///     }
/// }
/// ```
/// All implementations run inside a [`tokio::task::LocalSet`] — `Send` is
/// intentionally not required on the returned futures.
#[allow(async_fn_in_trait)]
pub trait AgentTransport {
    /// Connect to the agent described by `config` and return raw I/O handles.
    ///
    /// `name` is used for logging only.
    async fn connect(
        name: &str,
        config: &AgentConfig,
        worktree_root: &Path,
    ) -> Result<AgentIo, SurgeError>;
}

// ── StdioTransport ───────────────────────────────────────────────────

/// Spawns a local subprocess and communicates over its stdin/stdout.
///
/// This is the default transport for Claude Code, Copilot CLI, Zed Agent,
/// and any other agent that speaks ACP over stdio.
pub struct StdioTransport;

impl AgentTransport for StdioTransport {
    async fn connect(
        name: &str,
        config: &AgentConfig,
        worktree_root: &Path,
    ) -> Result<AgentIo, SurgeError> {
        use std::process::Stdio;
        use tokio::process::Command;
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

        info!("Spawning agent '{}' with command: {}", name, config.command);

        // On Windows, script-based commands (npx, npm, etc.) need cmd /C
        // because CreateProcessW doesn't resolve .cmd/.bat via PATHEXT.
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&config.command);
            c.args(&config.args);
            c.creation_flags(0x08000000); // CREATE_NO_WINDOW
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = Command::new(&config.command);
            c.args(&config.args);
            c
        };

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.current_dir(worktree_root);

        // Strip git environment variables inherited from the parent process.
        // If Surge itself runs inside a git repo, the agent would otherwise
        // inherit GIT_DIR / GIT_WORK_TREE and its git commands would target
        // the wrong repository.
        cmd.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES");

        // MCP server pass-through: write config to a temp file and set the
        // agent-specific env var before spawning.
        if !config.mcp_servers.is_empty() {
            setup_mcp_env(name, &config.mcp_servers, &mut cmd);
        }

        debug!("Spawning command: {:?}", cmd);

        let mut child = cmd.spawn().map_err(|e| {
            SurgeError::AgentConnection(format!(
                "Failed to spawn agent '{}' ({}): {}",
                name, config.command, e
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            SurgeError::AgentConnection("Failed to capture agent stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SurgeError::AgentConnection("Failed to capture agent stdout".to_string())
        })?;

        // Drain stderr to tracing::warn in background.
        // Uses tokio::spawn (not spawn_local) since ChildStderr is Send.
        if let Some(stderr) = child.stderr.take() {
            let agent_name = name.to_string();
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(agent = %agent_name, "[stderr] {}", line);
                }
            });
        }

        // Wrap in futures-compatible reader/writer as required by the ACP SDK.
        let writer = Box::new(stdin.compat_write()) as Box<dyn futures::AsyncWrite + Unpin>;
        let reader = Box::new(stdout.compat()) as Box<dyn futures::AsyncRead + Unpin>;

        Ok(AgentIo {
            writer,
            reader,
            child: Some(child),
        })
    }
}

// ── MCP pass-through ─────────────────────────────────────────────────

/// Serialises `servers` to a temp JSON file and sets the agent-specific env var
/// on `cmd` so the agent passes them on to its underlying model.
///
/// The JSON format follows the standard `mcpServers` schema:
/// ```json
/// { "mcpServers": { "name": { "command": "…", "args": [], "env": {} } } }
/// ```
///
/// For agents whose env var is not yet known, a `warn!` is emitted and no env
/// var is set — the agent starts normally but without the MCP servers.
fn setup_mcp_env(agent_name: &str, servers: &[McpServerConfig], cmd: &mut tokio::process::Command) {
    use crate::registry::AgentKind;

    let Some(env_var) = AgentKind::from_id(agent_name).and_then(|k| k.mcp_config_env_var()) else {
        warn!(
            agent = agent_name,
            servers = servers.len(),
            "MCP servers configured but no known env var for this agent type — skipping"
        );
        return;
    };

    match write_mcp_config_file(agent_name, servers) {
        Ok(path) => {
            debug!(
                agent = agent_name,
                path = %path.display(),
                env_var,
                servers = servers.len(),
                "setting MCP config env var"
            );
            cmd.env(env_var, path);
        }
        Err(e) => warn!(agent = agent_name, "failed to write MCP config: {e}"),
    }
}

/// Serialise MCP servers to the JSON format expected by agents.
fn serialize_mcp_config(servers: &[McpServerConfig]) -> Result<String, String> {
    let servers_obj: serde_json::Map<String, serde_json::Value> = servers
        .iter()
        .map(|s| {
            let val = serde_json::json!({
                "command": s.command,
                "args":    s.args,
                "env":     s.env,
            });
            (s.name.clone(), val)
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": servers_obj }))
        .map_err(|e| e.to_string())
}

/// Write the MCP config JSON to a temporary file.
///
/// File name is `surge-mcp-<agent>-<pid>.json` in the OS temp directory.
/// The file persists until the OS cleans the temp dir; no explicit cleanup is
/// needed since the agent reads it once at startup.
fn write_mcp_config_file(
    agent_name: &str,
    servers: &[McpServerConfig],
) -> Result<std::path::PathBuf, SurgeError> {
    let json = serialize_mcp_config(servers)
        .map_err(|e| SurgeError::AgentConnection(format!("Failed to serialize MCP config: {e}")))?;

    let path = std::env::temp_dir().join(format!(
        "surge-mcp-{}-{}.json",
        agent_name,
        std::process::id()
    ));

    std::fs::write(&path, &json).map_err(|e| {
        SurgeError::AgentConnection(format!("Failed to write MCP config file: {e}"))
    })?;

    Ok(path)
}

// ── TcpTransport ─────────────────────────────────────────────────────

/// TCP transport — reserved for Phase 7 remote-agent support.
///
/// Always returns an error. When implemented, it will connect to a running
/// ACP server over TCP instead of spawning a local process.
pub struct TcpTransport;

impl AgentTransport for TcpTransport {
    async fn connect(
        _name: &str,
        config: &AgentConfig,
        _worktree_root: &Path,
    ) -> Result<AgentIo, SurgeError> {
        match &config.transport {
            Transport::Tcp { host, port } => Err(SurgeError::AgentConnection(format!(
                "TCP transport not yet implemented ({}:{})",
                host, port
            ))),
            _ => unreachable!("TcpTransport::connect called with non-TCP config"),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::config::Transport;

    fn tcp_config(host: &str, port: u16) -> AgentConfig {
        AgentConfig {
            command: String::new(),
            args: vec![],
            transport: Transport::Tcp {
                host: host.to_string(),
                port,
            },
            mcp_servers: vec![],
            capabilities: vec![],
        }
    }

    #[tokio::test]
    async fn test_tcp_transport_returns_error() {
        let config = tcp_config("localhost", 9000);
        let result = TcpTransport::connect("test", &config, std::path::Path::new("/tmp")).await;
        let err = result.err().expect("expected TcpTransport to return Err");
        let msg = err.to_string();
        assert!(
            msg.contains("TCP transport not yet implemented"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_serialize_mcp_config_empty() {
        let json = serialize_mcp_config(&[]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["mcpServers"], serde_json::json!({}));
    }

    #[test]
    fn test_serialize_mcp_config_single_server() {
        use surge_core::config::McpServerConfig;
        let server = McpServerConfig {
            name: "my-tool".to_string(),
            command: "uvx".to_string(),
            args: vec!["my-tool-server".to_string()],
            env: std::collections::HashMap::new(),
        };
        let json = serialize_mcp_config(&[server]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["mcpServers"]["my-tool"]["command"], "uvx");
        assert_eq!(v["mcpServers"]["my-tool"]["args"][0], "my-tool-server");
    }

    #[test]
    fn test_serialize_mcp_config_preserves_env() {
        use surge_core::config::McpServerConfig;
        let mut env = std::collections::HashMap::new();
        env.insert("API_KEY".to_string(), "secret".to_string());
        let server = McpServerConfig {
            name: "secure-tool".to_string(),
            command: "npx".to_string(),
            args: vec![],
            env,
        };
        let json = serialize_mcp_config(&[server]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["mcpServers"]["secure-tool"]["env"]["API_KEY"], "secret");
    }
}
