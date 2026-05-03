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

    /// JoinHandle for the SDK's io_task pump.
    ///
    /// **Required for clean close — not optional.** The ACP SDK's `RpcConnection`
    /// internally clones `outgoing_tx` for `handle_incoming` and the io_task,
    /// creating a circular dependency: dropping `connection` alone leaves clones
    /// alive, so io_task never sees its `outgoing_rx` close. `close_session_impl`
    /// MUST `.abort()` this handle to drop `outgoing_bytes` (the child's stdin
    /// write end), which causes the agent to see EOF and exit cleanly. Without
    /// the abort, `close_session` hangs until the 5s grace timeout. See
    /// `close_session_impl` for the inline diagnosis.
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

    /// Sender used by `close_session_impl` on grace-timeout to instruct
    /// `subprocess_waiter` to forcibly SIGKILL the child process.
    ///
    /// **Lifecycle:** created in `open_session_impl`, stored here as `Some`.
    /// `close_session_impl` calls `.take()` on timeout and sends `()` to
    /// trigger the kill. `subprocess_waiter` may also consume the receiver
    /// side (via `kill_rx`) when it exits normally; in that case the sender
    /// drops and any subsequent `close_session_impl` send would return `Err`
    /// (harmless — the child is already gone). Set to `None` after the kill
    /// has been requested or `subprocess_waiter` exits first.
    pub kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
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
            BridgeCommand::SendMessage { session, content, reply } => {
                let result = send_message_impl(&sessions, session, content).await;
                let _ = reply.send(result);
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
                let result = close_session_impl(&sessions, &event_tx, session).await;
                let _ = reply.send(result);
            }
            BridgeCommand::Shutdown { reply } => {
                close_all_sessions(&sessions, &event_tx, SessionEndReason::ForcedClose).await;
                let _ = reply.send(());
                info!("bridge worker shutting down");
                return;
            }
            #[cfg(any(test, feature = "test-helpers"))]
            BridgeCommand::TestPanic => {
                panic!("bridge worker test-panic injected");
            }
        }
    }

    debug!("command channel closed; bridge worker exiting");
}

/// Routes incoming `SessionNotification` (agent messages, tool calls, token
/// usage) to `BridgeEvent` emissions. Phase 8.3 implements the real dispatch.
///
/// `_sandbox` is taken as `&dyn Sandbox` rather than `&Box<dyn Sandbox>` so
/// that strict clippy (`borrowed_box`) is happy and the call site can pass
/// `&*self.sandbox`.
pub(crate) async fn handle_session_notification(
    session_id: &SessionId,
    event_tx: &broadcast::Sender<BridgeEvent>,
    state: &Rc<RefCell<SessionStateInner>>,
    sandbox: &dyn super::sandbox::Sandbox,
    secrets: &Arc<crate::shared::secrets::SecretsRedactor>,
    notif: agent_client_protocol::SessionNotification,
) {
    use agent_client_protocol::SessionUpdate;
    use crate::bridge::tokens::extract_usage;
    use crate::bridge::event::AgentMessageMeta;

    // Update last_token_usage if this notification carries it.
    if let Some(snap) = extract_usage(&notif.update) {
        let snap_clone = snap.clone();
        {
            let mut s = state.borrow_mut();
            s.last_token_usage = Some(snap);
            s.last_token_usage_emitted = false;
        }
        let _ = event_tx.send(BridgeEvent::TokenUsage {
            session: session_id.clone(),
            prompt_tokens: snap_clone.prompt_tokens,
            output_tokens: snap_clone.output_tokens,
            cache_hits: snap_clone.cache_hits,
            model: snap_clone.model,
        });
        state.borrow_mut().last_token_usage_emitted = true;
    }

    match notif.update {
        // SDK 0.10.2 shape: AgentMessageChunk(ContentChunk) where ContentChunk
        // has field `content: ContentBlock` (not a `{ content }` struct pattern).
        SessionUpdate::AgentMessageChunk(chunk) => {
            let text = content_block_to_string(&chunk.content);
            let _ = event_tx.send(BridgeEvent::AgentMessage {
                session: session_id.clone(),
                chunk: text,
                meta: Some(AgentMessageMeta {
                    model: None,
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                }),
            });
        }
        // SDK 0.10.2 shape: ToolCall(ToolCall) where ToolCall has:
        //   - tool_call_id: ToolCallId  (not `.id`)
        //   - title: String             (not `.fields.title`)
        //   - raw_input: Option<serde_json::Value>
        SessionUpdate::ToolCall(tool_call) => {
            handle_tool_call(session_id, event_tx, state, sandbox, secrets, tool_call).await;
        }
        // Other variants (AgentThoughtChunk, ToolCallUpdate, Plan,
        // AvailableCommandsUpdate, CurrentModeUpdate, ConfigOptionUpdate,
        // SessionInfoUpdate, UsageUpdate) are deferred to Phase 10
        // observability tests.
        _ => {}
    }
}

fn content_block_to_string(b: &agent_client_protocol::ContentBlock) -> String {
    match b {
        agent_client_protocol::ContentBlock::Text(t) => t.text.clone(),
        _ => String::new(),
    }
}

async fn handle_tool_call(
    session_id: &SessionId,
    event_tx: &broadcast::Sender<BridgeEvent>,
    // Reserved for Phase 10: when generic tool dispatch lands, track
    // open tool calls in `state.open_tool_calls` for correlation with
    // tool/result notifications.
    _state: &Rc<RefCell<SessionStateInner>>,
    sandbox: &dyn super::sandbox::Sandbox,
    secrets: &Arc<crate::shared::secrets::SecretsRedactor>,
    tool_call: agent_client_protocol::ToolCall,
) {
    use crate::bridge::event::{ToolCallMeta, ToolResultPayload};
    use crate::bridge::tools::{REPORT_STAGE_OUTCOME, REQUEST_HUMAN_INPUT};

    // SDK 0.10.2 ToolCall shape:
    //   tool_call_id: ToolCallId  (ToolCallId wraps Arc<str>)
    //   title: String
    //   raw_input: Option<serde_json::Value>
    let tool_name = tool_call.title.clone();
    let call_id = tool_call.tool_call_id.0.to_string();
    let args_json = serde_json::to_string(&tool_call.raw_input.unwrap_or(serde_json::Value::Null))
        .unwrap_or_default();
    let args_redacted = secrets.redact_json(&args_json);

    if tool_name == REPORT_STAGE_OUTCOME {
        match parse_outcome_args(&args_json) {
            Ok((outcome, summary, artifacts)) => {
                let _ = event_tx.send(BridgeEvent::OutcomeReported {
                    session: session_id.clone(),
                    outcome,
                    summary,
                    artifacts_produced: artifacts,
                });
            }
            Err(e) => {
                let _ = event_tx.send(BridgeEvent::Error {
                    session: Some(session_id.clone()),
                    error: format!("report_stage_outcome args parse failed: {e}"),
                });
            }
        }
        return;
    }

    if tool_name == REQUEST_HUMAN_INPUT {
        match parse_human_input_args(&args_json) {
            Ok((question, context)) => {
                let _ = event_tx.send(BridgeEvent::HumanInputRequested {
                    session: session_id.clone(),
                    call_id,
                    question,
                    context,
                });
            }
            Err(e) => {
                let _ = event_tx.send(BridgeEvent::Error {
                    session: Some(session_id.clone()),
                    error: format!("request_human_input args parse failed: {e}"),
                });
            }
        }
        return;
    }

    // Generic tool call — emit ToolCall + auto-reply Unsupported (M3 stub per spec §5.3).
    let decision = sandbox.allows_tool(&tool_name, None);
    let _ = event_tx.send(BridgeEvent::ToolCall {
        session: session_id.clone(),
        call_id: call_id.clone(),
        tool: tool_name.clone(),
        args_redacted_json: args_redacted,
        sandbox_decision: decision,
        meta: ToolCallMeta { mcp_id: None, injected: false },
    });
    let _ = event_tx.send(BridgeEvent::ToolResult {
        session: session_id.clone(),
        call_id,
        payload: ToolResultPayload::Unsupported,
    });
}

fn parse_outcome_args(
    args_json: &str,
) -> Result<(surge_core::OutcomeKey, String, Vec<String>), String> {
    let v: serde_json::Value = serde_json::from_str(args_json).map_err(|e| e.to_string())?;
    let outcome_str = v.get("outcome").and_then(|o| o.as_str())
        .ok_or_else(|| "missing or non-string `outcome`".to_string())?;
    let outcome = surge_core::OutcomeKey::try_from(outcome_str)
        .map_err(|e| format!("invalid OutcomeKey '{outcome_str}': {e}"))?;
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let artifacts = v.get("artifacts_produced")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    Ok((outcome, summary, artifacts))
}

fn parse_human_input_args(args_json: &str) -> Result<(String, Option<String>), String> {
    let v: serde_json::Value = serde_json::from_str(args_json).map_err(|e| e.to_string())?;
    let question = v.get("question").and_then(|q| q.as_str())
        .ok_or_else(|| "missing or non-string `question`".to_string())?
        .to_string();
    let context = v.get("context").and_then(|c| c.as_str()).map(String::from);
    Ok((question, context))
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
        let session = sessions.borrow_mut().remove(&sid);
        if let Some(mut s) = session {
            // Wind down in this order:
            // 1. Send kill signal so subprocess_waiter can SIGKILL the child cleanly
            //    before we abort it (belt-and-suspenders alongside child.take below).
            // 2. Abort the spawned helper tasks (drainer + waiter won't observe drop).
            // 3. Abort the SDK's io_task pump (drops stdin write end → agent sees EOF).
            // 4. Drop `connection` (protocol-close trigger).
            // 5. Best-effort SIGKILL the child if the waiter didn't already consume it.
            if let Some(kill_tx) = s.kill_tx.take() {
                let _ = kill_tx.send(());
            }
            for handle in s.task_handles.drain(..) {
                handle.abort();
            }
            if let Some(io) = s.io_task_handle.take() {
                io.abort();
            }
            drop(s.connection.take());
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

    // Create the kill channel: close_session_impl sends on kill_tx when the
    // grace timeout fires; subprocess_waiter listens on kill_rx and SIGKILL the
    // child, then awaits its actual exit so the OS fully reaps it. This avoids
    // the "Drop doesn't kill" footgun with tokio::process::Child.
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

    let waiter_handle = tokio::task::spawn_local(subprocess_waiter(
        child,
        event_tx.clone(),
        inner.clone(),
        session_id.clone(),
        tail_storage.clone(),
        sessions.clone(),
        kill_rx,
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
            kill_tx: Some(kill_tx),
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

/// Grace period (ms) that `close_session_impl` waits for the agent to exit
/// cleanly after the stdin pipe is closed. After this window the kill_tx
/// signal fires and `subprocess_waiter` force-kills the child.
pub(crate) const GRACE_MS: u64 = 5_000;

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
                // Belt-and-suspenders: the TAIL_CAP check above always brings tail.len() to
                // STDERR_TAIL_CAP (2 KiB) which is < STDERR_RING_CAP (8 KiB), so this branch
                // is logically unreachable. Retained as a defensive bound in case the caps
                // ever decouple. See spec §5.6 unified-buffer design rationale.
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
///
/// The `kill_rx` channel allows `close_session_impl` to request a force-kill
/// when the grace timeout expires: it sends `()` and this function calls
/// `child.start_kill()` then waits for the child to actually exit, so the
/// session is always cleaned up promptly even for frozen/hung agents.
async fn subprocess_waiter(
    mut child: tokio::process::Child,
    event_tx: tokio::sync::broadcast::Sender<BridgeEvent>,
    state: std::rc::Rc<std::cell::RefCell<SessionStateInner>>,
    session_id: SessionId,
    tail_storage: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    sessions: SessionMap,
    mut kill_rx: tokio::sync::oneshot::Receiver<()>,
) {
    // `force_killed` is set to true when kill_rx fires, so that subprocess_waiter
    // can emit SessionEnded::Timeout rather than AgentCrashed — from the bridge's
    // perspective a grace-timeout kill is a timeout event, not a crash.
    let (exit_status, force_killed) = tokio::select! {
        // Normal path: child exits on its own (e.g. clean ACP shutdown via EOF).
        status = child.wait() => (status, false),
        // Forced-kill path: close_session_impl hit the grace timeout and sent
        // the kill signal. `start_kill` is synchronous and non-blocking; we
        // then await the child's actual exit so the OS fully reaps it.
        _ = &mut kill_rx => {
            let _ = child.start_kill();
            (child.wait().await, true)
        }
    };

    {
        let s = state.borrow();
        if s.end_emitted.is_some() {
            return;
        }
    }

    flush_pending_token_usage(&event_tx, &state, &session_id);

    let stderr_tail = read_stderr_tail(&tail_storage);
    // If force_killed (close_session sent the kill signal), emit Timeout rather
    // than AgentCrashed: the agent didn't crash, it was killed due to a grace
    // timeout. close_session_impl polls end_emitted and returns GracefulTimedOut
    // once this fires.
    let reason = if force_killed {
        SessionEndReason::Timeout { duration_ms: GRACE_MS }
    } else {
        match exit_status {
            Ok(s) if s.success() => SessionEndReason::Normal,
            Ok(s) => SessionEndReason::AgentCrashed {
                exit_code: s.code(),
                stderr_tail,
            },
            Err(_) => SessionEndReason::AgentCrashed {
                exit_code: None,
                stderr_tail,
            },
        }
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

/// Send a message to an existing ACP session.
///
/// Borrows the session's `ClientSideConnection` from the map and calls
/// `connection.prompt(request)`. The borrow is held across the `.await`
/// but is safe because all mutations to the session map happen on the same
/// LocalSet thread and are serialized through `bridge_loop`'s sequential
/// command dispatch.
pub(crate) async fn send_message_impl(
    sessions: &SessionMap,
    session: SessionId,
    content: crate::bridge::session::MessageContent,
) -> Result<(), super::error::SendMessageError> {
    use crate::bridge::session::MessageContent;
    use agent_client_protocol::Agent;

    // Resolve acp_session_id and confirm connection is present.
    let (connection_present, acp_session_str) = {
        let map = sessions.borrow();
        let s = map.get(&session).ok_or_else(|| super::error::SendMessageError::SessionNotFound {
            session: session.clone(),
        })?;
        let acp_id = s.inner.borrow().acp_session_id.clone();
        (s.connection.is_some(), acp_id)
    };

    if !connection_present {
        // Session is closing — connection has been .take()n by close_session_impl.
        return Err(super::error::SendMessageError::SessionEnded {
            session: session.clone(),
            reason: super::event::SessionEndReason::Normal,
        });
    }

    let blocks = match content {
        MessageContent::Text(s) => crate::shared::content_block::text_vec(s),
        MessageContent::Blocks(b) => b,
    };

    // Build ACP-side session id from the stored string.
    let acp_session_id =
        agent_client_protocol::SessionId::new(acp_session_str.as_str());
    let req = agent_client_protocol::PromptRequest::new(acp_session_id, blocks);

    // SAFETY: holding `sessions.borrow()` across the `prompt(...).await` below
    // is safe ONLY because `bridge_loop` dispatches commands sequentially on a
    // single LocalSet thread — no other command can attempt `sessions.borrow_mut()`
    // concurrently. If `bridge_loop` ever moves to concurrent command processing
    // (e.g. a future `tokio::select!` over multiple cmd_rx receivers), this
    // pattern becomes a `RefCell` panic at runtime. Refactor to clone an
    // `Arc<ClientSideConnection>` out of the entry and release the borrow
    // before await would be the right fix at that point.
    let map = sessions.borrow();
    let session_entry = map.get(&session).ok_or_else(|| {
        super::error::SendMessageError::SessionNotFound { session: session.clone() }
    })?;
    let connection = session_entry.connection.as_ref().ok_or_else(|| {
        super::error::SendMessageError::SessionEnded {
            session: session.clone(),
            reason: super::event::SessionEndReason::Normal,
        }
    })?;

    connection.prompt(req).await.map_err(|e| {
        warn!(session = %session, error = %e, "ACP prompt dispatch failed");
        super::error::SendMessageError::Bridge(
            super::error::BridgeError::CommandSendFailed(e.to_string()),
        )
    })?;

    Ok(())
}

/// Close a session gracefully.
///
/// **The mechanism that actually closes the agent's stdin pipe is aborting
/// `io_task_handle`, not dropping `connection`.** See the inline comment in
/// the take-and-abort block for the full SDK-internal deadlock chain. The
/// connection is taken anyway so subsequent `send_message` calls return
/// `SessionEnded` rather than racing the close.
///
/// On graceful close (the common case), aborting io_task drops the child's
/// stdin write end → mock sees EOF → exits status 0 → `subprocess_waiter`
/// emits `SessionEnded::Normal` within ~100ms.
///
/// On grace-timeout (agent ignores EOF or hangs), sends `kill_tx` so
/// `subprocess_waiter` force-kills the child, then waits up to 1s for
/// the waiter to emit `SessionEnded`. If the waiter still hasn't reacted
/// (very rare), forcibly removes the session, aborts tasks, and emits
/// `SessionEnded::Timeout` directly. Returns `GracefulTimedOut { killed: true }`.
pub(crate) async fn close_session_impl(
    sessions: &SessionMap,
    event_tx: &broadcast::Sender<BridgeEvent>,
    session: SessionId,
) -> Result<(), super::error::CloseSessionError> {
    use std::time::Duration;

    // Take connection + io_task_handle and mark closing. The RefCell borrow_mut
    // is released before any await point (scoped block).
    let (inner, took_connection, io_task) = {
        let mut map = sessions.borrow_mut();
        let s = map.get_mut(&session).ok_or_else(|| {
            super::error::CloseSessionError::SessionNotFound { session: session.clone() }
        })?;
        // Mark closing so observer/notification handlers drain quickly.
        s.inner.borrow_mut().closing = true;
        // Take connection — dropping it closes outgoing_tx (one sender). The SDK's
        // RpcConnection::handle_incoming also holds a cloned outgoing_tx sender, so
        // dropping the connection alone is NOT enough to cause the io_task to exit:
        // handle_incoming keeps its sender alive until incoming_rx closes, which only
        // happens when io_task drops incoming_tx, which only happens when io_task
        // exits, which requires ALL outgoing_tx senders to be dropped — a deadlock.
        //
        // Fix: take the io_task_handle and abort it below (after the borrow_mut is
        // released). Aborting drops the io_task future, which drops outgoing_bytes
        // (the child's stdin write end). The child then sees EOF on its stdin, its
        // handle_io returns, run_agent completes, and the subprocess exits → the
        // subprocess_waiter fires SessionEnded::Normal.
        let conn = s.connection.take();
        let io = s.io_task_handle.take();
        (s.inner.clone(), conn.is_some(), io)
    };

    if !took_connection {
        // Already closed/closing.
        return Ok(());
    }

    // Abort the SDK io_task immediately after releasing the borrow_mut so the
    // child's stdin write-end closes and the agent subprocess can exit cleanly.
    // This is the only way to break the handle_incoming↔handle_io cycle
    // described in the comment above (see also: bridge close-session design note
    // in docs/superpowers/specs/2026-05-03-surge-acp-bridge-m3-design.md §5.4).
    if let Some(io) = io_task {
        io.abort();
    }

    // Flush pending TokenUsage before SessionEnded (spec §5.7 ordering).
    flush_pending_token_usage(event_tx, &inner, &session);

    // Bound the wait for the waiter task to observe child exit.
    let start = tokio::time::Instant::now();
    while start.elapsed() < Duration::from_millis(GRACE_MS) {
        if inner.borrow().end_emitted.is_some() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Timeout: send the kill signal to subprocess_waiter so it can call
    // start_kill() and wait for the child to actually exit. The child handle
    // was moved into subprocess_waiter at session-open time, so close_session
    // cannot call start_kill() directly — the kill_tx channel is the correct
    // indirection. subprocess_waiter will call start_kill(), await child.wait(),
    // emit SessionEnded, and remove the session from the map.
    let mut killed = false;
    if let Some(s) = sessions.borrow_mut().get_mut(&session) {
        if let Some(kill_tx) = s.kill_tx.take() {
            if kill_tx.send(()).is_ok() {
                killed = true;
            }
        }
    }

    // Short-wait for subprocess_waiter to react (it emits SessionEnded once the
    // child is dead). On a well-functioning system this resolves within tens of ms.
    let post_kill_deadline = tokio::time::Instant::now() + Duration::from_millis(1000);
    while tokio::time::Instant::now() < post_kill_deadline {
        if inner.borrow().end_emitted.is_some() {
            // Waiter already emitted SessionEnded; we're done.
            return Err(super::error::CloseSessionError::GracefulTimedOut {
                session,
                killed,
            });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Waiter still hasn't reacted (very rare — kill_tx send failed AND waiter is
    // wedged). Force-remove and emit Timeout ourselves as a last resort.
    if let Some(mut s) = sessions.borrow_mut().remove(&session) {
        for handle in s.task_handles.drain(..) {
            handle.abort();
        }
        if let Some(io) = s.io_task_handle.take() {
            io.abort();
        }
    }

    let reason = super::event::SessionEndReason::Timeout { duration_ms: GRACE_MS };
    let _ = event_tx.send(BridgeEvent::SessionEnded {
        session: session.clone(),
        reason: reason.clone(),
    });
    // Set after send so the broadcast slot is taken before any racing waiter
    // could observe the end_emitted flag. Safe in current single-threaded
    // LocalSet (no .await between send and this assignment).
    inner.borrow_mut().end_emitted = Some(reason);

    Err(super::error::CloseSessionError::GracefulTimedOut {
        session,
        killed,
    })
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
