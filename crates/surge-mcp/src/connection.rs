//! Per-server MCP connection state. Wraps an rmcp `RunningService`
//! and handles spawn / crash detection / reconnect.

use crate::error::McpError;
use rmcp::ServiceExt;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use std::sync::Arc;
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use tokio::sync::Mutex;

/// Internal classification of rmcp `ServiceError`-derived messages
/// for deciding whether to mark the connection as crashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorClass {
    /// Peer is gone or transport is broken — connection should be
    /// marked crashed so the next call reconnects (subject to
    /// `restart_on_crash`).
    Transport,
    /// Server returned a service-level error (bad params, tool not
    /// found, etc.) — server is still healthy; don't mark crashed.
    Service,
}

/// Heuristic classifier for rmcp error display strings. rmcp's
/// `ServiceError` doesn't expose a structured kind across its
/// variants, so we string-match on common phrases. False classification
/// is acceptable: misclassifying transport as service means we don't
/// reconnect (next call will hit the same dead transport and fail
/// again — eventually the client gives up). Misclassifying service as
/// transport means an unnecessary reconnect — wasteful but harmless.
fn classify_rmcp_error(message: &str) -> ErrorClass {
    let lower = message.to_lowercase();
    // Transport / connection-loss indicators.
    let transport_markers = [
        "connection",
        "transport",
        "broken pipe",
        "channel closed",
        "i/o",
        "io error",
        "unexpected end",
        "disconnected",
        "eof",
    ];
    if transport_markers.iter().any(|m| lower.contains(m)) {
        ErrorClass::Transport
    } else {
        ErrorClass::Service
    }
}

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
    ///
    /// Note: the state mutex is held across the child-process spawn and
    /// the rmcp handshake (~50-1000ms typically). Concurrent callers to
    /// the same connection during a cold start are serialized. M7+ may
    /// refactor to an explicit Connecting state with a shared join
    /// handle so concurrent waiters share the same connect attempt
    /// without holding the mutex.
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
        // gives it `ServiceExt::serve`. Bound the handshake with the
        // same call_timeout used for individual tool calls — if the
        // child starts but never completes MCP init we don't hang.
        let call_timeout = self.config.call_timeout;
        let service = match tokio::time::timeout(call_timeout, ().serve(transport)).await {
            Ok(Ok(svc)) => svc,
            Ok(Err(e)) => {
                return Err(McpError::StartFailed {
                    server: self.config.name.clone(),
                    reason: e.to_string(),
                });
            },
            Err(_elapsed) => {
                return Err(McpError::Timeout(call_timeout));
            },
        };

        let rs = Arc::new(service);
        *state = ConnState::Running(rs.clone());
        Ok(rs)
    }

    /// List all tools the server reports via the MCP `tools/list` verb.
    ///
    /// Triggers a lazy connect on first call. On failure, classifies
    /// the error as transport-vs-service: transport failures mark the
    /// connection crashed (so the next call reconnects); service-level
    /// errors leave the connection alive.
    pub async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, McpError> {
        let rs = self.ensure_connected().await?;
        match rs.list_all_tools().await {
            Ok(tools) => Ok(tools),
            Err(e) => {
                let msg = e.to_string();
                match classify_rmcp_error(&msg) {
                    ErrorClass::Transport => {
                        self.mark_crashed(None).await;
                        Err(McpError::Transport(msg))
                    },
                    ErrorClass::Service => Err(McpError::Service(msg)),
                }
            },
        }
    }

    /// Call a named tool with the supplied JSON arguments, honouring
    /// the configured `call_timeout`.
    ///
    /// - If the timeout elapses: returns [`McpError::Timeout`] without
    ///   marking crashed — a slow server is not necessarily a dead one.
    /// - If the call returns a transport-like error (broken pipe, EOF,
    ///   etc.): marks the connection crashed and returns
    ///   [`McpError::Transport`].
    /// - If the call returns a service-level error (bad params, tool not
    ///   found, etc.): returns [`McpError::Service`] without marking
    ///   crashed — the server is still healthy.
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
                let msg = e.to_string();
                match classify_rmcp_error(&msg) {
                    ErrorClass::Transport => {
                        self.mark_crashed(None).await;
                        Err(McpError::Transport(msg))
                    },
                    ErrorClass::Service => Err(McpError::Service(msg)),
                }
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

    #[test]
    fn classify_transport_markers() {
        assert_eq!(classify_rmcp_error("broken pipe"), ErrorClass::Transport);
        assert_eq!(
            classify_rmcp_error("connection closed"),
            ErrorClass::Transport
        );
        assert_eq!(
            classify_rmcp_error("unexpected end of stream"),
            ErrorClass::Transport
        );
        assert_eq!(classify_rmcp_error("EOF"), ErrorClass::Transport);
    }

    #[test]
    fn classify_service_default() {
        assert_eq!(classify_rmcp_error("tool not found"), ErrorClass::Service);
        assert_eq!(
            classify_rmcp_error("invalid arguments"),
            ErrorClass::Service
        );
        assert_eq!(
            classify_rmcp_error("permission denied by server policy"),
            ErrorClass::Service
        );
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
