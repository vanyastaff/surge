//! Per-server MCP connection state. Wraps an rmcp `RunningService`
//! and handles spawn / crash detection / reconnect.

use crate::error::McpError;
use rmcp::ServiceExt;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use std::sync::Arc;
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use tokio::sync::Mutex;

/// State of a single MCP server connection.
#[derive(Debug)]
enum ConnState {
    /// Not yet connected, or fully shut down.
    Disconnected,
    /// rmcp service is alive; can dispatch calls.
    Running(Arc<RunningService<RoleClient, ()>>),
    /// Server died; in-flight calls have already been failed. The next
    /// `call_tool` triggers reconnect (if `restart_on_crash` is set).
    Crashed {
        /// Last observed exit code, if known. Stored for future
        /// observability / telemetry; not yet read by the state machine
        /// itself.
        #[allow(dead_code)]
        last_exit: Option<i32>,
    },
}

/// Per-server MCP connection. Owns an rmcp child process, the
/// protocol handshake, and a `Disconnected → Running → Crashed`
/// state machine.
///
/// Construction is cheap — the child process is not spawned until the
/// first [`call_tool`](McpServerConnection::call_tool) or
/// [`list_tools`](McpServerConnection::list_tools) call.
pub struct McpServerConnection {
    config: McpServerRef,
    state: Mutex<ConnState>,
}

impl McpServerConnection {
    /// Construct in [`Disconnected`](ConnState::Disconnected) state.
    ///
    /// The child process is not spawned until the first
    /// [`call_tool`](Self::call_tool) or [`list_tools`](Self::list_tools)
    /// invocation triggers [`ensure_connected`](Self::ensure_connected).
    #[must_use]
    pub fn new(config: McpServerRef) -> Self {
        Self {
            config,
            state: Mutex::new(ConnState::Disconnected),
        }
    }

    /// Server name as declared in the configuration.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Drive state to `Running`. Returns the `RunningService` `Arc` on
    /// success.
    ///
    /// Transition rules:
    /// - `Disconnected` → spawn + handshake → `Running`.
    /// - `Running` → return cached handle immediately.
    /// - `Crashed` + `restart_on_crash = true` → re-spawn → `Running`.
    /// - `Crashed` + `restart_on_crash = false` → `McpError::ServerNotRunning`.
    async fn ensure_connected(&self) -> Result<Arc<RunningService<RoleClient, ()>>, McpError> {
        let mut state = self.state.lock().await;
        match &*state {
            ConnState::Running(rs) => return Ok(rs.clone()),
            ConnState::Crashed { .. } if !self.config.restart_on_crash => {
                return Err(McpError::ServerNotRunning {
                    server: self.config.name.clone(),
                });
            },
            _ => {},
        }

        // Spawn child process and complete MCP handshake.
        let transport = match &self.config.transport {
            McpTransportConfig::Stdio { command, args, env } => {
                let mut tokio_cmd = tokio::process::Command::new(command);
                tokio_cmd.args(args);
                for (k, v) in env {
                    tokio_cmd.env(k, v);
                }
                TokioChildProcess::new(tokio_cmd).map_err(|e| McpError::StartFailed {
                    server: self.config.name.clone(),
                    reason: e.to_string(),
                })?
            },
            // `McpTransportConfig` is `#[non_exhaustive]`; future
            // transport variants (HTTP, socket, …) are not yet supported.
            _ => {
                return Err(McpError::StartFailed {
                    server: self.config.name.clone(),
                    reason: "unsupported transport variant".into(),
                });
            },
        };

        // `()` implements `ClientHandler` (all methods defaulted), and
        // the blanket `impl<H: ClientHandler> Service<RoleClient> for H`
        // gives it `ServiceExt::serve`.
        let service =
            ().serve(transport)
                .await
                .map_err(|e| McpError::StartFailed {
                    server: self.config.name.clone(),
                    reason: e.to_string(),
                })?;

        let rs = Arc::new(service);
        *state = ConnState::Running(rs.clone());
        Ok(rs)
    }

    /// List all tools the server reports via the MCP `tools/list` verb.
    ///
    /// Triggers a lazy connect on first call.
    pub async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, McpError> {
        let rs = self.ensure_connected().await?;
        // `RunningService` derefs to `Peer<R>`, so methods are available
        // directly. We use the deref path here.
        rs.list_all_tools()
            .await
            .map_err(|e| McpError::Service(e.to_string()))
    }

    /// Call a named tool with the supplied JSON arguments, honouring
    /// the configured `call_timeout`.
    ///
    /// - If the timeout elapses: returns [`McpError::Timeout`].
    /// - If the call returns a service-level error: marks the
    ///   connection as [`Crashed`](ConnState::Crashed) and returns
    ///   [`McpError::Service`].
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let rs = self.ensure_connected().await?;
        let timeout = self.config.call_timeout;

        // Build `CallToolRequestParams` using the constructor + builder API.
        // `name` accepts `impl Into<Cow<'static, str>>`; owned `String`
        // satisfies that bound.
        let mut params = rmcp::model::CallToolRequestParams::new(tool.to_string());
        if let Some(map) = match arguments {
            serde_json::Value::Object(m) => Some(m),
            serde_json::Value::Null => None,
            other => {
                let mut m = serde_json::Map::new();
                m.insert("input".into(), other);
                Some(m)
            },
        } {
            params = params.with_arguments(map);
        }

        match tokio::time::timeout(timeout, rs.call_tool(params)).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => {
                self.mark_crashed(None).await;
                Err(McpError::Service(e.to_string()))
            },
            Err(_elapsed) => Err(McpError::Timeout(timeout)),
        }
    }

    /// Transition to `Crashed` state so the next `ensure_connected`
    /// attempts a re-spawn (subject to `restart_on_crash`).
    async fn mark_crashed(&self, exit_code: Option<i32>) {
        let mut state = self.state.lock().await;
        *state = ConnState::Crashed {
            last_exit: exit_code,
        };
    }
}

impl std::fmt::Debug for McpServerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerConnection")
            .field("name", &self.config.name)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    fn fake_server_ref() -> McpServerRef {
        // Use the constructors because both `McpServerRef` and
        // `McpTransportConfig` are `#[non_exhaustive]`.
        McpServerRef::new(
            "x".into(),
            McpTransportConfig::stdio(
                PathBuf::from("nonexistent_command_xyz"),
                vec![],
                HashMap::new(),
            ),
            None,
            Duration::from_millis(100),
            true,
        )
    }

    #[tokio::test]
    async fn new_starts_disconnected() {
        let c = McpServerConnection::new(fake_server_ref());
        assert_eq!(c.name(), "x");
        // Verify that construction does not attempt a connection.
        // `ConnState` is private; confirming `name()` and no panic is
        // sufficient for the unit-level contract.
    }

    #[tokio::test]
    async fn call_tool_on_bad_command_returns_start_failed() {
        let c = McpServerConnection::new(fake_server_ref());
        let result = c.call_tool("any_tool", serde_json::Value::Null).await;
        match result {
            Err(McpError::StartFailed { server, .. }) => {
                assert_eq!(server, "x");
            },
            other => panic!("expected StartFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn crashed_with_no_restart_returns_server_not_running() {
        let config = McpServerRef::new(
            "x".into(),
            McpTransportConfig::stdio(
                PathBuf::from("nonexistent_command_xyz"),
                vec![],
                HashMap::new(),
            ),
            None,
            Duration::from_millis(100),
            false, // restart_on_crash = false
        );
        let c = McpServerConnection::new(config);
        // Force the crashed state directly.
        c.mark_crashed(Some(1)).await;
        let result = c.call_tool("any_tool", serde_json::Value::Null).await;
        match result {
            Err(McpError::ServerNotRunning { server }) => {
                assert_eq!(server, "x");
            },
            other => panic!("expected ServerNotRunning, got {other:?}"),
        }
    }
}
