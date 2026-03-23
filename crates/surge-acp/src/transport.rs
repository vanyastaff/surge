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
use surge_core::config::{AgentConfig, Transport};
use surge_core::SurgeError;
use tokio::process::Child;
use tracing::{debug, info};

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
}
