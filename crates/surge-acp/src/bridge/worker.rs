//! Bridge worker — owns the session map, dispatches commands.
//! Runs on the dedicated bridge thread inside a `LocalSet`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Arc;

use agent_client_protocol::{
    Agent, ClientCapabilities, ClientSideConnection, Implementation, InitializeRequest,
    NewSessionRequest, ProtocolVersion,
};
use surge_core::SessionId;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use super::command::BridgeCommand;
use super::error::OpenSessionError;
use super::event::{BridgeEvent, SessionEndReason};
use super::sandbox::{Sandbox, SandboxDecision};
use super::session::{AgentKind, SessionConfig};
use super::session_inner::SessionStateInner;
use super::tools::{ToolDef, build_injected_tools};
use crate::bridge::client::BridgeClient;
use crate::shared::secrets::SecretsRedactor;

/// Per-session state held by the worker. Phase 6.1 ships the minimal shape;
/// Phase 8.1 starts inserting; Phase 8.2 expands with the live ACP connection
/// + handles to the spawned waiter / drainer / io tasks.
pub(crate) struct AcpSession {
    pub session_id: SessionId,
    pub agent_label: String,

    /// The live ACP `ClientSideConnection`. **Must be held here**, not dropped
    /// at the end of `open_session_impl`, or the protocol channel dies (the
    /// `outgoing_tx` inside the connection closes → io_task drains → handle_incoming
    /// exits → no more send_message / session_notification possible).
    /// Held in `Option` so `close_session_impl` (Phase 8.3) can `.take()` it
    /// for graceful shutdown.
    pub connection: Option<agent_client_protocol::ClientSideConnection>,

    /// JoinHandle for the SDK's io_task pump. Not strictly required for
    /// correctness (io_task exits on its own when `connection` is dropped),
    /// but capturing it lets close paths optionally `.abort()` or `.await`
    /// the task for clean shutdown.
    pub io_task_handle: Option<tokio::task::JoinHandle<()>>,

    /// Subprocess handle. `None` once `subprocess_waiter` has consumed it
    /// for `child.wait()`. `close_session_impl` (Phase 8.3) checks this
    /// before deciding whether to call `start_kill()` on graceful-timeout.
    pub child: Option<tokio::process::Child>,

    /// Bridge-side LocalSet handles for the observer + waiter + drainer tasks.
    /// Aborting them cancels work cleanly when the session closes.
    pub task_handles: Vec<tokio::task::JoinHandle<()>>,

    /// Per-session inner state (shared with BridgeClient via Rc<RefCell<...>>).
    pub inner: std::rc::Rc<std::cell::RefCell<SessionStateInner>>,
}

#[allow(dead_code)] // wired in Task 7.1 via AcpSession construction
pub(crate) type SessionMap = Rc<RefCell<HashMap<SessionId, AcpSession>>>;

/// Main worker loop. Drains commands from `cmd_rx`, dispatches them, and
/// emits `BridgeEvent`s to subscribers. Returns when `Shutdown` is processed
/// or the channel closes.
///
/// Phase 6 ships a skeleton: most commands return immediate stub errors;
/// Phase 7+ replaces those arms with real handlers (`open_session_impl` etc).
#[allow(dead_code)] // wired in Task 6.2 via AcpBridge::spawn
pub(crate) async fn bridge_loop(
    mut cmd_rx: mpsc::Receiver<BridgeCommand>,
    event_tx: broadcast::Sender<BridgeEvent>,
) {
    info!("bridge worker entering main loop");
    let sessions: SessionMap = Rc::default();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BridgeCommand::OpenSession { config, reply } => {
                let result = open_session_impl(&sessions, &event_tx, config).await;
                let _ = reply.send(result);
            }
            BridgeCommand::SendMessage { session, reply, .. } => {
                let _ = reply.send(Err(super::error::SendMessageError::SessionNotFound { session }));
            }
            BridgeCommand::GetSessionState { session, reply } => {
                // Phase 6 stub: returns the bridge-observable state if the session
                // exists, else `BridgeError::ReplyDropped` as a stand-in. Phase 7
                // replaces this with proper not-found semantics once `BridgeError`
                // gains a `SessionNotFound` variant or `session_state` switches to
                // `Result<Option<SessionState>, _>`.
                let state = sessions
                    .borrow()
                    .get(&session)
                    .map(|s| super::session::SessionState {
                        session_id: s.session_id.clone(),
                        agent_label: s.agent_label.clone(),
                        status: super::session::SessionStatus::Open,
                        bindings: Default::default(),
                    });
                let _ = reply.send(state.ok_or(super::error::BridgeError::ReplyDropped));
            }
            BridgeCommand::CloseSession { session, reply } => {
                let _ = reply.send(Err(super::error::CloseSessionError::SessionNotFound { session }));
            }
            BridgeCommand::Shutdown { reply } => {
                close_all_sessions(&sessions, &event_tx, SessionEndReason::ForcedClose).await;
                let _ = reply.send(());
                info!("bridge worker shutting down");
                return;
            }
            #[cfg(test)]
            BridgeCommand::TestPanic => {
                panic!("bridge worker test-panic injected");
            }
        }
    }

    debug!("command channel closed; bridge worker exiting");
}

/// Routes incoming `SessionNotification` (agent messages, tool calls, token
/// usage) to `BridgeEvent` emissions. Phase 8.3 implements the real dispatch;
/// Phase 7 ships a stub so `BridgeClient::session_notification` can compile
/// and the wiring is verified.
///
/// `_sandbox` is taken as `&dyn Sandbox` rather than `&Box<dyn Sandbox>` so
/// that strict clippy (`borrowed_box`) is happy and the call site can pass
/// `&*self.sandbox`.
#[allow(dead_code)] // wired in Task 8.3 with real dispatch logic
pub(crate) async fn handle_session_notification(
    _session_id: &SessionId,
    _event_tx: &broadcast::Sender<BridgeEvent>,
    _state: &std::rc::Rc<std::cell::RefCell<super::session_inner::SessionStateInner>>,
    _sandbox: &dyn super::sandbox::Sandbox,
    _secrets: &std::sync::Arc<crate::shared::secrets::SecretsRedactor>,
    _notif: agent_client_protocol::SessionNotification,
) {
    // Phase 8.3 implements: routes SessionUpdate variants to BridgeEvent emission.
}

/// Emit `SessionEnded` for every open session, abort their spawned tasks,
/// drop their connections, and clear the map. Used by `Shutdown` and (later)
/// by failure paths in Phase 7+.
#[allow(dead_code)] // wired in Task 7.1+ failure paths and called from bridge_loop
pub(crate) async fn close_all_sessions(
    sessions: &SessionMap,
    event_tx: &broadcast::Sender<BridgeEvent>,
    reason: SessionEndReason,
) {
    let to_close: Vec<SessionId> = sessions.borrow().keys().cloned().collect();
    for sid in to_close {
        // Drop connection first so io_task starts winding down.
        let session = sessions.borrow_mut().remove(&sid);
        if let Some(mut s) = session {
            // Abort spawned tasks (drainer, waiter). Best-effort.
            for handle in s.task_handles.drain(..) {
                handle.abort();
            }
            if let Some(io) = s.io_task_handle.take() {
                io.abort();
            }
            // Drop connection explicitly (the explicit drop is the protocol
            // close trigger).
            drop(s.connection.take());
            // If subprocess_waiter didn't already consume `child` (e.g. session
            // never reached the open state), best-effort SIGKILL so we don't
            // orphan it.
            if let Some(mut child) = s.child.take() {
                let _ = child.start_kill();
            }
            // Mark end_emitted to suppress any racing SessionEnded from
            // subprocess_waiter.
            s.inner.borrow_mut().end_emitted = Some(reason.clone());
        }

        // Emit terminal event.
        let _ = event_tx.send(BridgeEvent::SessionEnded {
            session: sid.clone(),
            reason: reason.clone(),
        });
    }
}

/// Resolve `AgentKind` to a `tokio::process::Command` ready to spawn.
/// `Mock` short-circuits to `CARGO_BIN_EXE_mock_acp_agent` (set by Cargo
/// during `cargo test`); falls back to `<CARGO_TARGET_DIR>/debug/mock_acp_agent`
/// for non-test invocations.
fn build_agent_command(kind: &AgentKind, working_dir: &Path) -> Result<Command, std::io::Error> {
    let mut cmd = match kind {
        AgentKind::ClaudeCode { binary, extra_args } => {
            let mut c = Command::new(binary);
            c.arg("--acp");
            c.args(extra_args);
            c
        }
        AgentKind::Codex { binary, extra_args } => {
            let mut c = Command::new(binary);
            c.arg("acp");
            c.args(extra_args);
            c
        }
        AgentKind::GeminiCli { binary, extra_args } => {
            let mut c = Command::new(binary);
            c.arg("--acp");
            c.args(extra_args);
            c
        }
        AgentKind::Custom { binary, args } => {
            let mut c = Command::new(binary);
            c.args(args);
            c
        }
        AgentKind::Mock { args } => {
            // `CARGO_BIN_EXE_mock_acp_agent` is set by Cargo during `cargo test`.
            // Outside of tests, fall back to `<CARGO_TARGET_DIR>/debug/mock_acp_agent`.
            // No `?` on `VarError` since `VarError` isn't convertible to `io::Error`;
            // instead we always produce a `PathBuf` and let Command::spawn fail with
            // a clear `NotFound` if the binary is missing.
            let path = std::env::var("CARGO_BIN_EXE_mock_acp_agent")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let target = std::env::var("CARGO_TARGET_DIR")
                        .unwrap_or_else(|_| "target".to_string());
                    PathBuf::from(target).join("debug").join("mock_acp_agent")
                });
            let mut c = Command::new(path);
            c.args(args);
            c
        }
    };
    cmd.current_dir(working_dir);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}

/// Open a new ACP session — the production-real path that replaces the
/// Phase 6 stub. Sequence:
///
/// 1. Validate the `SessionConfig` (rejects empty `declared_outcomes`, etc.).
/// 2. Compose the visible tool list (caller tools + engine-injected, then
///    sandbox-filtered).
/// 3. Spawn the agent subprocess with piped stdio.
/// 4. Hand stdio to the SDK to construct a `ClientSideConnection` with our
///    `BridgeClient` as the trait impl, then run the ACP handshake
///    (`initialize` + `new_session`).
/// 5. Store the ACP-side session id in the per-session inner state and
///    register the session in the worker's map.
/// 6. Emit `BridgeEvent::SessionEstablished` carrying the visible tool names
///    and engine-supplied bindings.
/// 7. Spawn the stderr drainer and subprocess waiter; store connection,
///    io_task_handle, and all handles in `AcpSession`.
///
/// **SDK shape note:** `ClientSideConnection::new` returns `(connection, io_task)`.
/// The `io_task` is spawned via `tokio::task::spawn_local` (same pattern as
/// legacy `connection.rs`). Only `initialize` + `new_session` are called during
/// the handshake; `new_session` is the correct ACP method (see `pool.rs`).
///
/// **Approach chosen:** SDK calls are inlined directly into this function (option
/// (a) from the spec) rather than extracted into helpers — there is only one
/// call site, so helpers would only add indirection without clarity benefit.
pub(crate) async fn open_session_impl(
    sessions: &SessionMap,
    event_tx: &broadcast::Sender<BridgeEvent>,
    config: SessionConfig,
) -> Result<SessionId, OpenSessionError> {
    // Step 1: validate config (rejects empty declared_outcomes etc.).
    config.validate()?;

    // Step 2: build full tool list = caller tools + engine-injected, then sandbox-filter.
    let injected = build_injected_tools(&config.declared_outcomes, config.allows_escalation);
    let mut combined: Vec<ToolDef> = config.tools.iter().cloned().collect();
    combined.extend(injected.iter().cloned());
    let (visible, hidden_names) = filter_visible_tools(combined, config.sandbox.as_ref());

    // Step 3: spawn agent subprocess.
    let mut cmd =
        build_agent_command(&config.agent_kind, &config.working_dir).map_err(|e| {
            warn!(
                kind = config.agent_kind.label(),
                working_dir = %config.working_dir.display(),
                error = %e,
                "build_agent_command failed before spawn"
            );
            OpenSessionError::AgentSpawnFailed { kind: config.agent_kind.label().into(), source: e }
        })?;
    let mut child: Child = cmd.spawn().map_err(|e| {
        warn!(
            kind = config.agent_kind.label(),
            working_dir = %config.working_dir.display(),
            error = %e,
            "agent subprocess spawn failed"
        );
        OpenSessionError::AgentSpawnFailed {
            kind: config.agent_kind.label().into(),
            source: e,
        }
    })?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Step 4: ACP handshake — mirrors the pattern in legacy connection.rs.
    //
    // ClientSideConnection::new(client, writer, reader, executor_fn) → (connection, io_task).
    // The executor closure is called synchronously inside `new`; it spawns the io_task
    // onto the current LocalSet so the ACP IO loop runs concurrently with the handshake.
    let session_id = SessionId::new();
    let inner = Rc::new(RefCell::new(SessionStateInner::new(String::new())));

    let bridge_client = BridgeClient::new(
        session_id.clone(),
        event_tx.clone(),
        inner.clone(),
        config.sandbox.boxed_clone(),
        Arc::new(SecretsRedactor::new()),
        config.bindings.clone(),
        config.working_dir.clone(),
    );

    // The ACP SDK requires `futures::AsyncWrite + Unpin` / `futures::AsyncRead + Unpin`.
    // Tokio's ChildStdin/ChildStdout implement tokio's AsyncWrite/AsyncRead, so we wrap
    // them with `tokio_util::compat` (same pattern as legacy `transport.rs`).
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
    let writer = stdin.compat_write();
    let reader = stdout.compat();

    // Construct ClientSideConnection; executor spawns the io_task on the LocalSet.
    let (connection, io_task) =
        ClientSideConnection::new(bridge_client, writer, reader, |fut| {
            #[allow(clippy::let_underscore_future)]
            let _ = tokio::task::spawn_local(fut);
        });

    // Drive the io_task in the background (same pattern as legacy connection.rs).
    // We capture the JoinHandle so it can be moved into `AcpSession` for proper
    // shutdown coordination (abort on close_all_sessions / close_session_impl).
    let io_task_handle = tokio::task::spawn_local(async move {
        if let Err(e) = io_task.await {
            tracing::error!("ACP IO task failed: {:?}", e);
        }
    });

    // initialize — declare client capabilities and identity.
    let mut init_request = InitializeRequest::new(ProtocolVersion::V1);
    init_request.client_capabilities = ClientCapabilities::new().terminal(true);
    init_request.client_info =
        Some(Implementation::new("surge-bridge", env!("CARGO_PKG_VERSION")));

    let init_resp = match connection.initialize(init_request).await {
        Ok(r) => r,
        Err(e) => {
            warn!(
                session = %session_id,
                error = ?e,
                "ACP initialize handshake failed"
            );
            // Best-effort SIGKILL so the orphaned subprocess doesn't become a
            // zombie. tokio::process::Child's implicit Drop on Unix detaches
            // rather than killing — match the legacy AgentConnection::Drop
            // pattern (start_kill is synchronous, no await needed).
            let _ = child.start_kill();
            return Err(OpenSessionError::HandshakeFailed { reason: format!("{e:?}") });
        }
    };

    // new_session — create an ACP session scoped to the working directory.
    let new_resp = match connection
        .new_session(NewSessionRequest::new(&config.working_dir))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(
                session = %session_id,
                working_dir = %config.working_dir.display(),
                error = ?e,
                "ACP new_session handshake failed"
            );
            let _ = child.start_kill();
            return Err(OpenSessionError::HandshakeFailed { reason: format!("{e:?}") });
        }
    };

    // Step 5: store the ACP-side session string in the inner state.
    let acp_session_str = new_resp.session_id.to_string();
    inner.borrow_mut().acp_session_id = acp_session_str;

    // Step 6: emit SessionEstablished.
    let _ = event_tx.send(BridgeEvent::SessionEstablished {
        session: session_id.clone(),
        agent: config.agent_kind.label().into(),
        bindings: config.bindings.clone(),
        tools_visible: visible.iter().map(|t| t.name.clone()).collect(),
    });

    debug!(
        session = %session_id,
        hidden_count = hidden_names.len(),
        "session established with sandbox-filtered tools"
    );

    // Step 7: Spawn the stderr drainer + subprocess waiter on the LocalSet.
    let tail_storage: std::rc::Rc<std::cell::RefCell<Vec<u8>>> = std::rc::Rc::default();

    let drainer_handle = tokio::task::spawn_local(stderr_drainer(
        stderr,
        tail_storage.clone(),
        session_id.clone(),
    ));

    let waiter_handle = tokio::task::spawn_local(subprocess_waiter(
        child,
        event_tx.clone(),
        inner.clone(),
        session_id.clone(),
        tail_storage.clone(),
        sessions.clone(),
    ));

    // Insert the session — connection and io_task_handle go IN, not into a
    // suppression. Dropping connection here would close the protocol channel.
    sessions.borrow_mut().insert(
        session_id.clone(),
        AcpSession {
            session_id: session_id.clone(),
            agent_label: config.agent_kind.label().into(),
            connection: Some(connection),
            io_task_handle: Some(io_task_handle),
            child: None, // moved into subprocess_waiter
            task_handles: vec![drainer_handle, waiter_handle],
            inner: inner.clone(),
        },
    );

    // init_resp held only for handshake-time validation; nothing references it later.
    let _ = init_resp;

    Ok(session_id)
}

/// Filter a combined tool list through the sandbox's `visibility` decision.
///
/// Returns `(visible_tools, hidden_tool_names)`. The hidden list is used for
/// debug logging only — the bridge does not surface it to callers.
fn filter_visible_tools(
    tools: Vec<ToolDef>,
    sandbox: &dyn Sandbox,
) -> (Vec<ToolDef>, Vec<String>) {
    let mut visible = Vec::with_capacity(tools.len());
    let mut hidden_names = Vec::new();
    for t in tools {
        let mcp_id = t.category.mcp_id();
        match sandbox.visibility(&t.name, mcp_id) {
            SandboxDecision::Allow | SandboxDecision::Elevate { .. } => visible.push(t),
            SandboxDecision::Deny { .. } => hidden_names.push(t.name.clone()),
        }
    }
    (visible, hidden_names)
}

const STDERR_RING_CAP: usize = 8 * 1024;
const STDERR_TAIL_CAP: usize = 2 * 1024;

/// Continuously read stderr into a bounded ring buffer; on session end the
/// last `STDERR_TAIL_CAP` bytes are returned for inclusion in
/// `SessionEndReason::AgentCrashed::stderr_tail`.
async fn stderr_drainer(
    mut stderr: tokio::process::ChildStderr,
    tail_storage: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    session_id: SessionId,
) {
    let mut scratch = vec![0u8; 4096];
    loop {
        match stderr.read(&mut scratch).await {
            Ok(0) => break,
            Ok(n) => {
                if let Ok(s) = std::str::from_utf8(&scratch[..n]) {
                    warn!(session = %session_id, "agent stderr: {}", s.trim_end());
                }
                let mut tail = tail_storage.borrow_mut();
                tail.extend_from_slice(&scratch[..n]);
                if tail.len() > STDERR_TAIL_CAP {
                    let drop_n = tail.len() - STDERR_TAIL_CAP;
                    tail.drain(..drop_n);
                }
                if tail.len() > STDERR_RING_CAP {
                    let drop_n = tail.len() - STDERR_RING_CAP;
                    tail.drain(..drop_n);
                }
            }
            Err(e) => {
                warn!(session = %session_id, "stderr read failed: {e}");
                break;
            }
        }
    }
}

/// Read the current contents of the stderr tail buffer as a String.
fn read_stderr_tail(tail_storage: &std::rc::Rc<std::cell::RefCell<Vec<u8>>>) -> String {
    let buf = tail_storage.borrow();
    String::from_utf8_lossy(&buf).into_owned()
}

/// Wait for the agent subprocess to exit. On exit:
/// - If the session was already cleanly closed (`state.end_emitted` set), exit silently.
/// - Otherwise flush any pending TokenUsage (spec §5.7 ordering) then emit
///   `SessionEnded` with the appropriate reason (`Normal` for clean exit;
///   `AgentCrashed` with exit_code + stderr_tail for non-zero / signal exits).
async fn subprocess_waiter(
    mut child: tokio::process::Child,
    event_tx: tokio::sync::broadcast::Sender<BridgeEvent>,
    state: std::rc::Rc<std::cell::RefCell<SessionStateInner>>,
    session_id: SessionId,
    tail_storage: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    sessions: SessionMap,
) {
    let exit_status = child.wait().await;

    {
        let s = state.borrow();
        if s.end_emitted.is_some() {
            return;
        }
    }

    flush_pending_token_usage(&event_tx, &state, &session_id);

    let stderr_tail = read_stderr_tail(&tail_storage);
    let reason = match exit_status {
        Ok(s) if s.success() => SessionEndReason::Normal,
        Ok(s) => SessionEndReason::AgentCrashed {
            exit_code: s.code(),
            stderr_tail,
        },
        Err(_) => SessionEndReason::AgentCrashed {
            exit_code: None,
            stderr_tail,
        },
    };

    let _ = event_tx.send(BridgeEvent::SessionEnded {
        session: session_id.clone(),
        reason: reason.clone(),
    });
    state.borrow_mut().end_emitted = Some(reason);
    sessions.borrow_mut().remove(&session_id);
}

/// Emit a TokenUsage event if there's an unemitted snapshot. Called from
/// session-end paths to honor the spec §5.7 ordering guarantee.
#[allow(dead_code)] // wired in Task 8.3 close_session_impl
pub(crate) fn flush_pending_token_usage(
    event_tx: &tokio::sync::broadcast::Sender<BridgeEvent>,
    state: &std::rc::Rc<std::cell::RefCell<SessionStateInner>>,
    session_id: &SessionId,
) {
    let snapshot = {
        let s = state.borrow();
        if s.last_token_usage_emitted {
            None
        } else {
            s.last_token_usage.clone()
        }
    };
    if let Some(u) = snapshot {
        let _ = event_tx.send(BridgeEvent::TokenUsage {
            session: session_id.clone(),
            prompt_tokens: u.prompt_tokens,
            output_tokens: u.output_tokens,
            cache_hits: u.cache_hits,
            model: u.model,
        });
        state.borrow_mut().last_token_usage_emitted = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::sandbox::DenyListSandbox;
    use crate::bridge::tools::{ToolCategory, ToolDef};
    use serde_json::json;

    #[test]
    fn filter_removes_denied_tools() {
        let tools = vec![
            ToolDef::new("read_file", "d", ToolCategory::Builtin, json!({})),
            ToolDef::new("shell_exec", "d", ToolCategory::Mcp("ops".into()), json!({})),
        ];
        let s = DenyListSandbox::deny_tools(["shell_exec"]);
        let (visible, hidden) = filter_visible_tools(tools, &s);
        let names: Vec<_> = visible.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_file"]);
        assert_eq!(hidden, vec!["shell_exec"]);
    }
}
