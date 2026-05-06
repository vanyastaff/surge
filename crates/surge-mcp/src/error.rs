//! Errors produced by the MCP integration layer.

use std::time::Duration;

/// Errors produced by the surge-mcp client surface.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// The named server is not in the run-level
    /// `RunConfig::mcp_servers` registry.
    #[error("server '{0}' not configured in run-level mcp_servers registry")]
    ServerNotConfigured(String),
    /// The server's child process failed to start.
    #[error("server '{server}' failed to start: {reason}")]
    StartFailed {
        /// Server name from the configuration.
        server: String,
        /// Underlying error message from the spawn / handshake.
        reason: String,
    },
    /// The server's child process exited mid-call (or before the call
    /// completed). In-flight calls fail with this; subsequent calls
    /// will trigger a re-spawn if `restart_on_crash` is true.
    #[error("server '{server}' crashed (exit code {exit_code:?})")]
    ServerCrashed {
        /// Server name from the configuration.
        server: String,
        /// Exit code if known (`None` if the OS didn't report one).
        exit_code: Option<i32>,
    },
    /// The server is in `Crashed` state and `restart_on_crash` is
    /// `false`.
    #[error("server '{server}' is not running (restart_on_crash=false)")]
    ServerNotRunning {
        /// Server name from the configuration.
        server: String,
    },
    /// The server reported the requested tool name in `tools/list`
    /// once but it has since gone away (rare; usually a server bug).
    #[error("server '{server}' tool '{tool}' not found")]
    ToolNotFound {
        /// Server name.
        server: String,
        /// Tool name the engine attempted to call.
        tool: String,
    },
    /// The call exceeded `McpServerRef::call_timeout`.
    #[error("MCP call timed out after {0:?}")]
    Timeout(Duration),
    /// rmcp transport-level error (socket / pipe / handshake).
    #[error("rmcp transport error: {0}")]
    Transport(String),
    /// rmcp service-level error (peer rejected, protocol violation).
    #[error("rmcp service error: {0}")]
    Service(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_format_includes_duration() {
        let e = McpError::Timeout(Duration::from_mins(1));
        let s = format!("{e}");
        assert!(
            s.contains("60s"),
            "expected '60s' in error display, got: {s}"
        );
    }
}
