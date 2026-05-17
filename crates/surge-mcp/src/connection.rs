//! Per-server MCP connection state. Wraps an rmcp `RunningService`
//! and handles spawn / crash detection / reconnect.

use crate::error::McpError;
use crate::redact::redact_line;
use rmcp::ServiceExt;
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::transport::child_process::TokioChildProcess;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

/// Max stderr lines retained per connection in the bounded tee file.
/// Documented default; not configurable in v0.1 (decide-or-defer).
const MAX_STDERR_LINES: usize = 500;

/// Restart policy constants. Documented defaults; not configurable in
/// v0.1 (decide-or-defer per the plan's Operational Notes).
const MAX_RESTART_ATTEMPTS: u32 = 5;
const BACKOFF_BASE: Duration = Duration::from_millis(500);
const BACKOFF_FACTOR: u32 = 2;
const BACKOFF_CAP: Duration = Duration::from_secs(30);

/// Exponential backoff for the Nth (1-based) consecutive failed
/// reconnect attempt: `min(BASE * FACTOR^(n-1), CAP)`. Pure and
/// deterministic so it is unit-testable without a live server.
#[must_use]
pub(crate) fn backoff_delay(attempt: u32) -> Duration {
    if attempt == 0 {
        return Duration::ZERO;
    }
    let exp = attempt.saturating_sub(1);
    // Saturating power so a large attempt count cannot overflow.
    let mult = BACKOFF_FACTOR.checked_pow(exp).unwrap_or(u32::MAX);
    BACKOFF_BASE
        .checked_mul(mult)
        .unwrap_or(BACKOFF_CAP)
        .min(BACKOFF_CAP)
}

/// Outcome of advancing the restart policy after a failed reconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartDecision {
    /// Stay crashed; retry no earlier than `delay` from now.
    Backoff { attempts: u32, delay: Duration },
    /// Capped budget exhausted; do not spawn again until reset.
    Exhausted { attempts: u32 },
}

/// Pure restart-policy transition: given the prior consecutive-failure
/// count, decide whether to back off (and for how long) or to give up.
/// Deterministic and timing-free so the policy is unit-testable.
#[must_use]
pub(crate) fn restart_decision(prior_attempts: u32) -> RestartDecision {
    let attempts = prior_attempts.saturating_add(1);
    if attempts > MAX_RESTART_ATTEMPTS {
        RestartDecision::Exhausted { attempts }
    } else {
        RestartDecision::Backoff {
            attempts,
            delay: backoff_delay(attempts),
        }
    }
}

/// Internal classification of an rmcp [`ServiceError`] for deciding
/// whether the connection should be marked crashed (and later
/// reconnected, subject to restart policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorClass {
    /// Peer is gone or transport is broken — mark crashed so the next
    /// call reconnects.
    Transport,
    /// Server returned a service-level error (bad params, tool not
    /// found, protocol error) — the server is still healthy.
    Service,
}

/// Structured classification of rmcp's `ServiceError`. Replaces the
/// former display-string heuristic: `ServiceError` is a structured
/// `#[non_exhaustive]` enum, so transport-death is detected reliably
/// instead of by substring matching.
pub(crate) fn classify_service_error(e: &ServiceError) -> ErrorClass {
    match e {
        // Transport / connection-loss — reconnect is warranted.
        ServiceError::TransportClosed | ServiceError::TransportSend(_) => ErrorClass::Transport,
        // Server-level (it answered, the call failed) or a slow /
        // cancelled call — none of these mean the child is dead.
        ServiceError::McpError(_)
        | ServiceError::UnexpectedResponse
        | ServiceError::Timeout { .. }
        | ServiceError::Cancelled { .. } => ErrorClass::Service,
        // `ServiceError` is `#[non_exhaustive]`: conservatively treat an
        // unknown variant as service-level (do not reconnect on an
        // error we cannot classify) and surface it.
        _ => {
            tracing::warn!(
                target: "mcp::supervisor",
                "unclassified rmcp ServiceError; treating as service-level (not marking crashed)"
            );
            ErrorClass::Service
        }
    }
}

/// State of a single MCP server connection.
#[derive(Debug)]
enum ConnState {
    /// Not yet connected, or fully shut down.
    Disconnected,
    /// rmcp service is alive; can dispatch calls.
    Running(Arc<RunningService<RoleClient, ()>>),
    /// Server died; the next `ensure_connected` attempts a re-spawn
    /// subject to `restart_on_crash` and the exponential-backoff
    /// policy. `attempts`/`next_retry_at` are runtime-only — never
    /// persisted, never in any event payload (replay determinism).
    Crashed {
        /// Last observed exit code, if known.
        #[allow(dead_code)]
        last_exit: Option<i32>,
        /// Consecutive failed (re)connect attempts so far.
        attempts: u32,
        /// Earliest instant the next reconnect may be attempted.
        /// `None` means "retry immediately" (first crash).
        next_retry_at: Option<Instant>,
    },
    /// The restart policy exhausted its capped attempt budget. The
    /// connection will not spawn again until reset. The ERROR
    /// escalation line is emitted exactly once, on entry to this state.
    Exhausted {
        /// Total consecutive failed attempts when the budget ran out.
        attempts: u32,
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
    /// Working directory pinned for the child process. `Some` for
    /// run-scoped connections (the run worktree); `None` for daemon
    /// diagnostic probes (they inherit no run cwd).
    cwd: Option<PathBuf>,
    state: Mutex<ConnState>,
}

impl McpServerConnection {
    /// Construct in the disconnected state.
    ///
    /// `cwd` pins the child process working directory and roots the
    /// captured-stderr file. The child process is not spawned until the
    /// first [`call_tool`](Self::call_tool) / [`list_tools`](Self::list_tools)
    /// invocation triggers a lazy connect.
    #[must_use]
    pub fn new(config: McpServerRef, cwd: Option<PathBuf>) -> Self {
        Self {
            config,
            cwd,
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
    /// Transition rules (U1 baseline; U2 layers the backoff policy onto
    /// the `Crashed` branch):
    /// - `Disconnected` → spawn + handshake → `Running`.
    /// - `Running` → return cached handle immediately.
    /// - `Crashed` + `restart_on_crash = true` → re-spawn → `Running`.
    /// - `Crashed` + `restart_on_crash = false` → `McpError::ServerNotRunning`.
    ///
    /// Note: the state mutex is held across the child-process spawn and
    /// the rmcp handshake. Concurrent callers to the same connection
    /// during a cold start are serialized (documented M7+ limitation,
    /// out of scope for this milestone).
    async fn ensure_connected(&self) -> Result<Arc<RunningService<RoleClient, ()>>, McpError> {
        let mut state = self.state.lock().await;
        let prior_attempts = match &*state {
            ConnState::Running(rs) => return Ok(rs.clone()),
            ConnState::Exhausted { attempts } => {
                return Err(McpError::RestartExhausted {
                    server: self.config.name.clone(),
                    attempts: *attempts,
                });
            },
            ConnState::Crashed { .. } if !self.config.restart_on_crash => {
                return Err(McpError::ServerNotRunning {
                    server: self.config.name.clone(),
                });
            },
            ConnState::Crashed {
                attempts,
                next_retry_at,
                ..
            } => {
                // Fast-return while still in backoff: no spawn, no
                // attempt-count change. The no-hot-loop guarantee
                // depends on rate-limited callers (agent stage; U11
                // monitor with interval >= backoff cap).
                if let Some(t) = next_retry_at
                    && Instant::now() < *t
                {
                    return Err(McpError::Transport(format!(
                        "server '{}' in restart backoff (attempt {}/{})",
                        self.config.name, attempts, MAX_RESTART_ATTEMPTS
                    )));
                }
                *attempts
            },
            ConnState::Disconnected => 0,
        };

        match self.spawn_and_serve().await {
            Ok(service) => {
                let rs = Arc::new(service);
                *state = ConnState::Running(rs.clone());
                Ok(rs)
            },
            Err(e) => {
                match restart_decision(prior_attempts) {
                    RestartDecision::Backoff { attempts, delay } => {
                        *state = ConnState::Crashed {
                            last_exit: None,
                            attempts,
                            next_retry_at: Some(Instant::now() + delay),
                        };
                    },
                    RestartDecision::Exhausted { attempts } => {
                        *state = ConnState::Exhausted { attempts };
                        // Single-site escalation: emitted exactly once,
                        // on entry to `Exhausted` (subsequent calls hit
                        // the early-return above and do not re-log).
                        tracing::error!(
                            target: "mcp::supervisor",
                            server = %self.config.name,
                            attempts,
                            "mcp_supervisor_gave_up"
                        );
                        return Err(McpError::RestartExhausted {
                            server: self.config.name.clone(),
                            attempts,
                        });
                    },
                }
                Err(e)
            },
        }
    }

    /// Spawn the child + complete the rmcp handshake. Does not touch
    /// connection state — `ensure_connected` owns the state machine and
    /// the backoff policy.
    async fn spawn_and_serve(&self) -> Result<RunningService<RoleClient, ()>, McpError> {
        // Build the child command with env / cwd hygiene, then spawn via
        // the stderr-capturing builder.
        let (transport, stderr) = match &self.config.transport {
            McpTransportConfig::Stdio { command, args, env } => {
                let mut tokio_cmd = tokio::process::Command::new(command);
                tokio_cmd.args(args);
                // Hygiene: do not leak the full host environment to an
                // arbitrary MCP child. Start from a minimal essential
                // set, then layer the declared env on top.
                tokio_cmd.env_clear();
                for (k, v) in minimal_child_env() {
                    tokio_cmd.env(k, v);
                }
                for (k, v) in env {
                    tokio_cmd.env(k, v);
                }
                if let Some(dir) = &self.cwd {
                    tokio_cmd.current_dir(dir);
                }
                TokioChildProcess::builder(tokio_cmd)
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| McpError::StartFailed {
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

        // Forward child stderr to `tracing` + a bounded, redacted,
        // run-scoped file. The task ends when the pipe closes (child
        // exit / shutdown); it holds no handle to the connection.
        if let Some(stderr) = stderr {
            let server = self.config.name.clone();
            let path = stderr_file_path(self.cwd.as_deref(), &server);
            tokio::spawn(stderr_forwarder(stderr, server, path));
        }

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

        Ok(service)
    }

    /// List all tools the server reports via the MCP `tools/list` verb.
    ///
    /// Triggers a lazy connect on first call. On failure, classifies
    /// the error structurally: transport failures mark the connection
    /// crashed (so the next call reconnects); service-level errors
    /// leave the connection alive.
    pub async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, McpError> {
        let rs = self.ensure_connected().await?;
        match rs.list_all_tools().await {
            Ok(tools) => Ok(tools),
            Err(e) => Err(self.handle_service_error(e).await),
        }
    }

    /// Call a named tool with the supplied JSON arguments, honouring
    /// the configured `call_timeout`.
    ///
    /// - Timeout elapses → [`McpError::Timeout`] (not marked crashed —
    ///   a slow server is not necessarily dead).
    /// - Transport-class error → connection marked crashed,
    ///   [`McpError::Transport`].
    /// - Service-level error → [`McpError::Service`], connection stays
    ///   alive.
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let rs = self.ensure_connected().await?;
        let timeout = self.config.call_timeout;

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
            Ok(Err(e)) => Err(self.handle_service_error(e).await),
            Err(_elapsed) => Err(McpError::Timeout(timeout)),
        }
    }

    /// Map an rmcp [`ServiceError`] to an [`McpError`], marking the
    /// connection crashed on transport-class failures.
    async fn handle_service_error(&self, e: ServiceError) -> McpError {
        let msg = e.to_string();
        match classify_service_error(&e) {
            ErrorClass::Transport => {
                self.mark_crashed(None).await;
                McpError::Transport(msg)
            },
            ErrorClass::Service => McpError::Service(msg),
        }
    }

    /// Transition to `Crashed` after a transport-class failure on a
    /// live connection. The first reconnect is immediate
    /// (`next_retry_at: None`); backoff accrues only on *failed*
    /// reconnect attempts. An already-`Crashed`/`Exhausted` connection
    /// keeps its accrued attempt count.
    async fn mark_crashed(&self, exit_code: Option<i32>) {
        let mut state = self.state.lock().await;
        let attempts = match &*state {
            ConnState::Crashed { attempts, .. } | ConnState::Exhausted { attempts } => *attempts,
            _ => 0,
        };
        *state = ConnState::Crashed {
            last_exit: exit_code,
            attempts,
            next_retry_at: None,
        };
    }
}

impl std::fmt::Debug for McpServerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerConnection")
            .field("name", &self.config.name)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

/// A minimal, secret-free environment for spawned MCP children. Avoids
/// leaking arbitrary host env vars (e.g. cloud credentials) while still
/// providing what most runtimes need to start.
fn minimal_child_env() -> Vec<(String, String)> {
    let mut keep: Vec<&str> = vec!["PATH", "HOME", "TMPDIR", "TMP", "TEMP", "LANG", "LC_ALL"];
    if cfg!(windows) {
        keep.extend([
            "SystemRoot",
            "windir",
            "SystemDrive",
            "NUMBER_OF_PROCESSORS",
            "PATHEXT",
            "COMSPEC",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
        ]);
    }
    keep.into_iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect()
}

/// Resolve the bounded stderr-capture file for a connection.
///
/// Run-scoped connections write under the run worktree
/// (`<cwd>/.surge/mcp-stderr/<server>.log`); daemon diagnostic probes
/// (`cwd == None`) fall back to a daemon-scoped temp directory.
fn stderr_file_path(cwd: Option<&Path>, server: &str) -> PathBuf {
    let safe: String = server
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let base = match cwd {
        Some(dir) => dir.join(".surge").join("mcp-stderr"),
        None => std::env::temp_dir().join("surge-mcp-stderr"),
    };
    base.join(format!("{safe}.log"))
}

/// Stream a child's stderr to `tracing` and a bounded, redacted file.
async fn stderr_forwarder(stderr: tokio::process::ChildStderr, server: String, path: PathBuf) {
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let mut lines = BufReader::new(stderr).lines();
    let mut ring: VecDeque<String> = VecDeque::with_capacity(MAX_STDERR_LINES);
    while let Ok(Some(line)) = lines.next_line().await {
        let red = redact_line(&line);
        tracing::info!(target: "mcp::child::stderr", server = %server, "{red}");
        if ring.len() == MAX_STDERR_LINES {
            ring.pop_front();
        }
        ring.push_back(red);
        // Rewrite the bounded window. MCP stderr is low-volume
        // (startup banner + occasional warnings); an exact last-N tail
        // is worth the rewrite. Best-effort: a write failure must not
        // kill the forwarder.
        let body = ring.iter().cloned().collect::<Vec<_>>().join("\n");
        let _ = tokio::fs::write(&path, body).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    fn fake_server_ref() -> McpServerRef {
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
    fn backoff_delay_monotonic_and_capped() {
        let mut prev = Duration::ZERO;
        for n in 1..=12u32 {
            let d = backoff_delay(n);
            assert!(d >= prev, "backoff must be non-decreasing at {n}");
            assert!(d <= BACKOFF_CAP, "backoff must clamp at cap at {n}");
            prev = d;
        }
        assert_eq!(backoff_delay(0), Duration::ZERO);
        assert_eq!(backoff_delay(1), BACKOFF_BASE);
        // Large attempt counts saturate to the cap, never overflow.
        assert_eq!(backoff_delay(u32::MAX), BACKOFF_CAP);
    }

    #[test]
    fn restart_decision_backs_off_then_exhausts() {
        for prior in 0..MAX_RESTART_ATTEMPTS {
            match restart_decision(prior) {
                RestartDecision::Backoff { attempts, delay } => {
                    assert_eq!(attempts, prior + 1);
                    assert_eq!(delay, backoff_delay(prior + 1));
                },
                RestartDecision::Exhausted { .. } => {
                    panic!("attempt {} should still back off", prior + 1)
                },
            }
        }
        // The (MAX+1)th consecutive failure gives up.
        match restart_decision(MAX_RESTART_ATTEMPTS) {
            RestartDecision::Exhausted { attempts } => {
                assert_eq!(attempts, MAX_RESTART_ATTEMPTS + 1);
            },
            RestartDecision::Backoff { .. } => panic!("should be exhausted"),
        }
    }

    #[tokio::test]
    async fn fast_returns_during_backoff_without_respawning() {
        let c = McpServerConnection::new(fake_server_ref(), None);
        // First call: spawn of a missing binary fails → StartFailed,
        // state transitions to Crashed with a future next_retry_at.
        match c.call_tool("t", serde_json::Value::Null).await {
            Err(McpError::StartFailed { .. }) => {},
            other => panic!("expected StartFailed, got {other:?}"),
        }
        // Second call (immediately): must fast-return the backoff error
        // without attempting another spawn — and quickly.
        let start = Instant::now();
        let r = c.call_tool("t", serde_json::Value::Null).await;
        let elapsed = start.elapsed();
        match r {
            Err(McpError::Transport(msg)) => {
                assert!(msg.contains("restart backoff"), "got: {msg}");
                assert!(msg.contains("attempt 1/"), "attempt count preserved: {msg}");
            },
            other => panic!("expected Transport backoff, got {other:?}"),
        }
        assert!(
            elapsed < Duration::from_millis(100),
            "fast-return must not spawn (took {elapsed:?})"
        );
    }

    #[test]
    fn classify_transport_variants() {
        assert_eq!(
            classify_service_error(&ServiceError::TransportClosed),
            ErrorClass::Transport
        );
        assert_eq!(
            classify_service_error(&ServiceError::UnexpectedResponse),
            ErrorClass::Service
        );
        assert_eq!(
            classify_service_error(&ServiceError::Cancelled { reason: None }),
            ErrorClass::Service
        );
        assert_eq!(
            classify_service_error(&ServiceError::Timeout {
                timeout: Duration::from_secs(1)
            }),
            ErrorClass::Service
        );
    }

    #[test]
    fn stderr_path_is_run_scoped_when_cwd_present() {
        let p = stderr_file_path(Some(Path::new("/work/tree")), "play/wright");
        assert!(p.ends_with("play_wright.log"));
        assert!(p.to_string_lossy().contains("mcp-stderr"));
    }

    #[test]
    fn stderr_path_falls_back_to_temp_for_daemon_probe() {
        let p = stderr_file_path(None, "github");
        assert!(p.ends_with("github.log"));
        assert!(p.starts_with(std::env::temp_dir()));
    }

    #[tokio::test]
    async fn new_starts_disconnected() {
        let c = McpServerConnection::new(fake_server_ref(), None);
        assert_eq!(c.name(), "x");
    }

    #[tokio::test]
    async fn call_tool_on_bad_command_returns_start_failed() {
        let c = McpServerConnection::new(fake_server_ref(), None);
        let result = c.call_tool("any_tool", serde_json::Value::Null).await;
        match result {
            Err(McpError::StartFailed { server, .. }) => assert_eq!(server, "x"),
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
            false,
        );
        let c = McpServerConnection::new(config, None);
        c.mark_crashed(Some(1)).await;
        let result = c.call_tool("any_tool", serde_json::Value::Null).await;
        match result {
            Err(McpError::ServerNotRunning { server }) => assert_eq!(server, "x"),
            other => panic!("expected ServerNotRunning, got {other:?}"),
        }
    }
}
