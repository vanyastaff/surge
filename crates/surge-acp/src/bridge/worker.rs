//! Bridge worker — owns the session map, dispatches commands.
//! Runs on the dedicated bridge thread inside a `LocalSet`.

use crate::sdk_v1::ClientConnection;
use agent_client_protocol::schema::ProtocolVersion;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{Implementation, InitializeRequest, NewSessionRequest};
use std::collections::BTreeMap;
use surge_core::SessionId;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::broadcast;

use tracing::{debug, warn};

use super::error::OpenSessionError;
use super::event::BridgeEvent;
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
    pub effect_fence: Option<Arc<dyn super::HostEffectFence>>,
    pub secrets: Arc<SecretsRedactor>,
    pub session_id: SessionId,
    pub agent_label: String,
    pub connection: Option<Rc<ClientConnection>>,
    pub io_task_handle: Option<tokio::task::JoinHandle<()>>,
    pub child: Option<Child>,
    pub task_handles: Vec<tokio::task::JoinHandle<()>>,
    pub inner: Rc<RefCell<SessionStateInner>>,
    pub kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub waiter: Option<tokio::task::JoinHandle<super::lifecycle::ProcessExit>>,
    pub cancel: tokio_util::sync::CancellationToken,
    pub prompt_running: bool,
    pub prompt_done: tokio_util::sync::CancellationToken,
    pub tail: Rc<RefCell<Vec<u8>>>,
    pub events: broadcast::Sender<BridgeEvent>,
    pub established: Option<BridgeEvent>,
    pub opened: surge_core::execution_recovery::OpenedSession,
}

pub(crate) type SessionMap = Rc<RefCell<HashMap<SessionId, AcpSession>>>;

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
    notif: agent_client_protocol::schema::v1::SessionNotification,
) {
    use crate::bridge::event::AgentMessageMeta;
    use crate::bridge::tokens::extract_usage;
    use agent_client_protocol::schema::v1::SessionUpdate;

    if !state.borrow().live_ingress || state.borrow().closing {
        return;
    }

    // Update last_token_usage if this notification carries it.
    if let Some(snap) = extract_usage(&notif.update) {
        let snap_clone = snap.clone();
        {
            let mut s = state.borrow_mut();
            s.last_token_usage = Some(snap);
            s.last_token_usage_emitted = false;
        }
        let _ = event_tx.send(BridgeEvent::TokenUsage {
            session: *session_id,
            prompt_tokens: snap_clone.prompt_tokens,
            output_tokens: snap_clone.output_tokens,
            cache_hits: snap_clone.cache_hits,
            model: snap_clone.model,
        });
        state.borrow_mut().last_token_usage_emitted = true;
    }

    match notif.update {
        // Stable v1 schema shape: AgentMessageChunk(ContentChunk) where ContentChunk
        // has field `content: ContentBlock` (not a `{ content }` struct pattern).
        SessionUpdate::AgentMessageChunk(chunk) => {
            let text = secrets.redact_json(&content_block_to_string(&chunk.content));
            let _ = event_tx.send(BridgeEvent::AgentMessage {
                session: *session_id,
                chunk: text,
                meta: Some(AgentMessageMeta {
                    model: None,
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                }),
            });
        },
        // Stable v1 schema shape: ToolCall(ToolCall) where ToolCall has:
        //   - tool_call_id: ToolCallId  (not `.id`)
        //   - title: String             (not `.fields.title`)
        //   - raw_input: Option<serde_json::Value>
        SessionUpdate::ToolCall(tool_call) => {
            handle_tool_call(session_id, event_tx, state, sandbox, secrets, tool_call).await;
        },
        // Other variants (AgentThoughtChunk, ToolCallUpdate, Plan,
        // AvailableCommandsUpdate, CurrentModeUpdate, ConfigOptionUpdate,
        // SessionInfoUpdate, UsageUpdate) are deferred to Phase 10
        // observability tests.
        _ => {},
    }
}

fn content_block_to_string(b: &agent_client_protocol::schema::v1::ContentBlock) -> String {
    match b {
        agent_client_protocol::schema::v1::ContentBlock::Text(t) => t.text.clone(),
        _ => String::new(),
    }
}

async fn handle_tool_call(
    session_id: &SessionId,
    event_tx: &broadcast::Sender<BridgeEvent>,
    _state: &Rc<RefCell<SessionStateInner>>,
    _sandbox: &dyn super::sandbox::Sandbox,
    secrets: &Arc<crate::shared::secrets::SecretsRedactor>,
    tool_call: agent_client_protocol::schema::v1::ToolCall,
) {
    let args = tool_call
        .raw_input
        .unwrap_or(serde_json::Value::Null)
        .to_string();
    // Best-effort, chunk-local redaction is not an information-flow boundary.
    // Provider-controlled call_id remains observational metadata; encoded or
    // split secrets in arbitrary provider output are not universally scrubbed.
    let _ = event_tx.send(BridgeEvent::ToolObserved {
        session: *session_id,
        call_id: tool_call.tool_call_id.0.to_string(),
        title: secrets.redact_json(&tool_call.title),
        args_redacted_json: secrets.redact_json(&args),
    });
}

/// Resolve a program name to a concrete path via PATH + PATHEXT.
///
/// `Command::new("npx")` fails with "program not found" on Windows because
/// the standard library searches PATH for `npx`/`npx.exe` but does NOT
/// append the other `PATHEXT` extensions, so `.cmd`/`.bat` shims (npx, the
/// npm-installed agent CLIs) are never found. `which` performs the full
/// PATHEXT-aware lookup. Falls back to the original name (absolute paths,
/// or when resolution fails) so spawn still surfaces a clear NotFound.
fn resolve_program(binary: &Path) -> PathBuf {
    which::which(binary).unwrap_or_else(|_| binary.to_path_buf())
}

/// Resolve `AgentKind` to a `tokio::process::Command` ready to spawn.
/// `Mock` short-circuits to `CARGO_BIN_EXE_mock_acp_agent` (set by Cargo
/// during `cargo test`); falls back to `<CARGO_TARGET_DIR>/debug/mock_acp_agent`
/// for non-test invocations.
///
/// `env` is the caller-resolved per-agent environment (see
/// [`crate::agent_env`]) — already concrete values, applied after the
/// inherited environment so they override it. Never logged: the `Command`'s
/// `Debug` prints only the program and argv.
fn build_agent_command(
    kind: &AgentKind,
    working_dir: &Path,
    env: &BTreeMap<String, String>,
) -> Result<Command, std::io::Error> {
    let mut cmd = match kind {
        AgentKind::ClaudeCode { binary, extra_args } => {
            let mut c = Command::new(resolve_program(binary));
            c.arg("--acp");
            c.args(extra_args);
            c
        },
        AgentKind::Codex { binary, extra_args } => {
            let mut c = Command::new(resolve_program(binary));
            c.arg("acp");
            c.args(extra_args);
            c
        },
        AgentKind::GeminiCli { binary, extra_args } => {
            let mut c = Command::new(resolve_program(binary));
            c.arg("--acp");
            c.args(extra_args);
            c
        },
        AgentKind::Custom { binary, args } => {
            let mut c = Command::new(resolve_program(binary));
            c.args(args);
            c
        },
        AgentKind::Mock { args } => {
            // `CARGO_BIN_EXE_mock_acp_agent` is set by Cargo during `cargo test`.
            // Outside of tests, fall back to `<CARGO_TARGET_DIR>/debug/mock_acp_agent`.
            // No `?` on `VarError` since `VarError` isn't convertible to `io::Error`;
            // instead we always produce a `PathBuf` and let Command::spawn fail with
            // a clear `NotFound` if the binary is missing.
            let path = std::env::var("CARGO_BIN_EXE_mock_acp_agent")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let target =
                        std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
                    // On Windows, Command::new won't find the binary without the
                    // .exe suffix when the CARGO_BIN_EXE env var is absent.
                    #[cfg(windows)]
                    let bin = "mock_acp_agent.exe";
                    #[cfg(not(windows))]
                    let bin = "mock_acp_agent";
                    PathBuf::from(target).join("debug").join(bin)
                });
            let mut c = Command::new(path);
            c.args(args);
            c
        },
    };
    cmd.current_dir(working_dir);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    Ok(cmd)
}

/// Open a new ACP session — the production-real path that replaces the
/// Phase 6 stub. Sequence:
///
/// 1. Validate the `SessionConfig` (rejects empty `declared_outcomes`, etc.).
/// 2. Compose the visible tool list (caller tools + engine-injected, then
///    sandbox-filtered).
/// 3. Spawn the agent subprocess with piped stdio.
/// 4. Hand stdio to the SDK to construct a `ClientConnection` with our
///    `BridgeClient` as the trait impl, then run the ACP handshake
///    (`initialize` + `new_session`).
/// 5. Store the ACP-side session id in the per-session inner state and
///    register the session in the worker's map.
/// 6. Emit `BridgeEvent::SessionEstablished` carrying the visible tool names
///    and engine-supplied bindings.
/// 7. Spawn the stderr drainer and subprocess waiter; store connection,
///    io_task_handle, and all handles in `AcpSession`.
///
/// **SDK shape note:** `ClientConnection::new` returns `(connection, io_task)`.
/// The `io_task` is spawned via `tokio::task::spawn_local` (same pattern as
/// legacy `connection.rs`). Only `initialize` + `new_session` are called during
/// the handshake; `new_session` is the correct ACP method (see `pool.rs`).
///
pub(crate) async fn open_session_impl(
    event_tx: &broadcast::Sender<BridgeEvent>,
    config: SessionConfig,
    shutdown: &tokio_util::sync::CancellationToken,
    reply: &mut tokio::sync::oneshot::Sender<
        Result<surge_core::execution_recovery::OpenedSession, OpenSessionError>,
    >,
    handshake_timeout: Duration,
) -> Result<AcpSession, OpenSessionError> {
    // A launcher-started adapter (npx) occasionally never answers the
    // handshake — observed live under machine load at `session/new` — and a
    // single hang used to fail the whole run. The failed attempt's process
    // group is already reaped; start the agent once more with the same
    // config. Only a handshake timeout is retried: spawn, protocol and
    // option errors are deterministic and returned as they are.
    let mut attempt = 1;
    loop {
        match open_session_attempt(event_tx, &config, shutdown, reply, handshake_timeout).await {
            Err(OpenSessionError::HandshakeTimedOut { phase, timeout })
                if phase == "initialize"
                    && attempt < HANDSHAKE_ATTEMPTS
                    && !shutdown.is_cancelled()
                    && !reply.is_closed() =>
            {
                warn!(
                    kind = config.agent_kind.label(),
                    phase,
                    ?timeout,
                    attempt,
                    "agent did not finish the ACP handshake; restarting it once"
                );
                attempt += 1;
            },
            result => return result,
        }
    }
}

/// Launch attempts per session open (see [`open_session_impl`]).
const HANDSHAKE_ATTEMPTS: u32 = 2;

async fn open_session_attempt(
    event_tx: &broadcast::Sender<BridgeEvent>,
    config: &SessionConfig,
    shutdown: &tokio_util::sync::CancellationToken,
    reply: &mut tokio::sync::oneshot::Sender<
        Result<surge_core::execution_recovery::OpenedSession, OpenSessionError>,
    >,
    handshake_timeout: Duration,
) -> Result<AcpSession, OpenSessionError> {
    if shutdown.is_cancelled() || reply.is_closed() {
        return Err(OpenSessionError::Cancelled);
    }
    config.validate()?;

    // Step 2: build full tool list = caller tools + engine-injected, then sandbox-filter.
    let injected = build_injected_tools(&config.declared_outcomes, config.allows_escalation);
    let mut combined: Vec<ToolDef> = config.tools.to_vec();
    combined.extend(injected.iter().cloned());
    let (mut visible, hidden_names) = filter_visible_tools(combined, config.sandbox.as_ref());
    if config.stage_mcp.is_some() {
        visible = config.stage_tools();
    }

    // Step 3: spawn agent subprocess.
    let mut cmd = build_agent_command(&config.agent_kind, &config.working_dir, &config.env)
        .map_err(|e| {
            warn!(
                kind = config.agent_kind.label(),
                working_dir = %config.working_dir.display(),
                error = %e,
                "build_agent_command failed before spawn"
            );
            OpenSessionError::AgentSpawnFailed {
                kind: config.agent_kind.label().into(),
                source: e,
            }
        })?;
    cmd.kill_on_drop(true);
    // npx and shell launchers spawn the actual agent as a descendant. Own a
    // separate group so forced cleanup reaches the adapter, not just its shim.
    #[cfg(unix)]
    cmd.process_group(0);
    super::effect_fence::check(config.effect_fence.as_ref())?;
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
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        reap_failed_open(&mut child).await;
        return Err(OpenSessionError::HandshakeFailed {
            reason: "agent stdio pipes unavailable".into(),
        });
    };

    // Step 4: ACP handshake — mirrors the pattern in legacy connection.rs.
    //
    // ClientConnection::new(client, writer, reader, executor_fn) → (connection, io_task).
    // The executor closure is called synchronously inside `new`; it spawns the io_task
    // onto the current LocalSet so the ACP IO loop runs concurrently with the handshake.
    let session_id = config
        .stage_mcp
        .as_ref()
        .map_or_else(SessionId::new, |stage| stage.session);
    let inner = Rc::new(RefCell::new(SessionStateInner::new(String::new())));
    let tail: Rc<RefCell<Vec<u8>>> = Rc::default();
    let secrets = Arc::new(SecretsRedactor::with_literal(
        config
            .stage_mcp
            .as_ref()
            .and_then(|stage| stage.authentication()),
    ));
    let drainer = tokio::task::spawn_local(stderr_drainer(
        stderr,
        tail.clone(),
        session_id,
        secrets.clone(),
    ));
    let deadline = tokio::time::Instant::now() + handshake_timeout;

    // Canonicalize working_dir once so path_guard's bounds checks work correctly.
    // If working_dir contains symlinks or non-canonical components, the canonicalized
    // request paths from ensure_in_worktree / resolve_for_write won't `starts_with`
    // the non-canonical root, causing spurious rejection of valid in-worktree paths.
    // Falls back to the raw path with a warning if canonicalization fails (matches
    // SurgeClient::new behavior in legacy code).
    let worktree_root_canonical = config.working_dir.canonicalize().unwrap_or_else(|e| {
        warn!(
            worktree = %config.working_dir.display(),
            error = %e,
            "failed to canonicalize worktree root; path bounds checks may be unreliable",
        );
        config.working_dir.clone()
    });

    let mut bridge_client = BridgeClient::new(
        session_id,
        event_tx.clone(),
        inner.clone(),
        config.sandbox.boxed_clone(),
        secrets.clone(),
        config.bindings.clone(),
        worktree_root_canonical,
    );
    bridge_client.effect_fence = config.effect_fence.clone();
    inner.borrow_mut().live_ingress = false;

    // The ACP SDK requires `futures::AsyncWrite + Unpin` / `futures::AsyncRead + Unpin`.
    // Tokio's ChildStdin/ChildStdout implement tokio's AsyncWrite/AsyncRead, so we wrap
    // them with `tokio_util::compat` (same pattern as legacy `transport.rs`).
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
    let writer = stdin.compat_write();
    let reader = stdout.compat();

    let (mut connection, io_task) = ClientConnection::new(bridge_client, writer, reader);
    connection.set_effect_fence(config.effect_fence.clone());

    // Drive the io_task in the background (same pattern as legacy connection.rs).
    // We capture the JoinHandle so it can be moved into `AcpSession` for proper
    // shutdown coordination (abort on close_all_sessions / close_session_impl).
    let io_secrets = secrets.clone();
    let io_task_handle = tokio::task::spawn_local(async move {
        if let Err(e) = io_task.await {
            tracing::error!(error = %io_secrets.redact_json(&e.to_string()), "ACP IO task failed");
        }
    });

    // initialize — declare client capabilities and identity.
    let mut init_request = InitializeRequest::new(ProtocolVersion::V1);
    init_request.client_capabilities =
        crate::connection::surge_client_capabilities(&config.permission_policy);
    init_request.client_info = Some(Implementation::new(
        "surge-bridge",
        env!("CARGO_PKG_VERSION"),
    ));

    let handshake = async {
        use surge_core::execution_recovery::{SessionOpening, SessionOpenMode, SessionRestoreCapabilities};
        let initialized = handshake_step(connection.initialize(init_request), "initialize", shutdown, reply, deadline, handshake_timeout, config.effect_fence.as_ref()).await?;
        let capabilities = SessionRestoreCapabilities { resume: initialized.agent_capabilities.session_capabilities.resume.is_some(), load: initialized.agent_capabilities.load_session };
        let servers = config.stage_mcp.as_ref().map(|stage| vec![agent_client_protocol::schema::v1::McpServer::Stdio(stage.server.clone())]).unwrap_or_default();
        let canonical_cwd = config.working_dir.canonicalize().map_err(|_| OpenSessionError::HandshakeFailed { reason: "session directory unavailable".into() })?;
        let launch_hash = surge_core::ContentHash::compute(format!("{:?}", config.agent_kind).as_bytes());
        let (response, mode) = match &config.opening {
            SessionOpening::New => (handshake_step(connection.new_session(NewSessionRequest::new(&canonical_cwd).mcp_servers(servers)), "new_session", shutdown, reply, deadline, handshake_timeout, config.effect_fence.as_ref()).await?, SessionOpenMode::New),
            SessionOpening::Continue(saved) => {
                if saved.cwd() != canonical_cwd || saved.runtime() != config.runtime || saved.launch_hash() != &launch_hash || saved.invocation() != config.invocation {
                    return Err(OpenSessionError::HandshakeFailed { reason: "saved provider session differs from pinned runtime, launch, invocation or cwd".into() });
                }
                let provider_id = agent_client_protocol::schema::v1::SessionId::new(saved.provider_session_id().as_str());
                let (options, mode) = if capabilities.resume {
                    let restored = handshake_step(connection.resume_session(agent_client_protocol::schema::v1::ResumeSessionRequest::new(provider_id.clone(), &canonical_cwd).mcp_servers(servers)), "resume_session", shutdown, reply, deadline, handshake_timeout, config.effect_fence.as_ref()).await?;
                    (restored.config_options, SessionOpenMode::Resume)
                } else if capabilities.load {
                    let restored = handshake_step(connection.load_session(agent_client_protocol::schema::v1::LoadSessionRequest::new(provider_id.clone(), &canonical_cwd).mcp_servers(servers)), "load_session", shutdown, reply, deadline, handshake_timeout, config.effect_fence.as_ref()).await?;
                    (restored.config_options, SessionOpenMode::Load)
                } else { return Err(OpenSessionError::HandshakeFailed { reason: "provider supports neither session/resume nor session/load; explicit replacement decision required".into() }); };
                let mut response = agent_client_protocol::schema::v1::NewSessionResponse::new(provider_id);
                response.config_options = options;
                (response, mode)
            }
        };
        Ok::<_, OpenSessionError>((response, capabilities, mode, canonical_cwd, launch_hash))
    }.await;
    let (response, capabilities, mode, canonical_cwd, launch_hash) = match handshake {
        Ok(response) => response,
        Err(error) => {
            let error = match error {
                OpenSessionError::HandshakeFailed { reason } => OpenSessionError::HandshakeFailed {
                    reason: secrets.redact_json(&reason),
                },
                other => other,
            };
            connection.stop();
            let _ = io_task_handle.await;
            drop(connection);
            reap_failed_open(&mut child).await;
            drainer.abort();
            let _ = drainer.await;
            return Err(error);
        },
    };
    // Apply requested session options (model, reasoning level) through the
    // standard `session/set_config_option`, before any prompt is sent.
    if !config.config_selections.is_empty() {
        let offered = response.config_options.clone().unwrap_or_default();
        let applied = async {
            for selection in &config.config_selections {
                let (config_id, value) = match resolve_config_selection(&offered, selection) {
                    Ok(resolved) => resolved,
                    Err(error) if selection.best_effort => {
                        tracing::info!(%error, "skipping best-effort session option");
                        continue;
                    },
                    Err(error) => return Err(error),
                };
                handshake_step(
                    connection.set_session_config_option(
                        agent_client_protocol::schema::v1::SetSessionConfigOptionRequest::new(
                            response.session_id.clone(),
                            config_id,
                            value,
                        ),
                    ),
                    "set_config_option",
                    shutdown,
                    reply,
                    deadline,
                    handshake_timeout,
                    config.effect_fence.as_ref(),
                )
                .await?;
            }
            Ok::<(), OpenSessionError>(())
        }
        .await;
        if let Err(error) = applied {
            let error = match error {
                OpenSessionError::HandshakeFailed { reason } => OpenSessionError::HandshakeFailed {
                    reason: secrets.redact_json(&reason),
                },
                other => other,
            };
            connection.stop();
            let _ = io_task_handle.await;
            drop(connection);
            reap_failed_open(&mut child).await;
            drainer.abort();
            let _ = drainer.await;
            return Err(error);
        }
    }
    let opened = (|| {
        let descriptor = surge_core::execution_recovery::ProviderSessionDescriptor::new(
            surge_core::execution_recovery::ProviderSessionId::new(
                response.session_id.to_string(),
            )?,
            config.invocation,
            config.runtime.clone(),
            launch_hash,
            canonical_cwd,
            capabilities,
        )?;
        let mut opened =
            surge_core::execution_recovery::OpenedSession::new(session_id, descriptor, mode)?;
        opened.execution_writer = Some(
            surge_core::execution_recovery::process::ExecutionWriterObservation::new(
                config.writer_id,
                child.id().and_then(|pid| {
                    crate::process_evidence::observe_container(
                        pid,
                        surge_core::execution_recovery::process::WriterCoverage::GroupOnly,
                    )
                    .ok()
                }),
            )?,
        );
        Ok::<_, surge_core::execution_recovery::RecoveryIdentityError>(opened)
    })();
    let opened = match opened {
        Ok(opened) => opened,
        Err(error) => {
            connection.stop();
            let _ = io_task_handle.await;
            drop(connection);
            reap_failed_open(&mut child).await;
            drainer.abort();
            let _ = drainer.await;
            return Err(OpenSessionError::HandshakeFailed {
                reason: error.to_string(),
            });
        },
    };
    inner.borrow_mut().acp_session_id = response.session_id.to_string();
    debug!(session = %session_id, hidden_count = hidden_names.len(), "ACP handshake completed");
    Ok(AcpSession {
        effect_fence: config.effect_fence.clone(),
        opened,
        secrets,
        session_id,
        agent_label: config.agent_kind.label().into(),
        connection: Some(Rc::new(connection)),
        io_task_handle: Some(io_task_handle),
        child: Some(child),
        task_handles: vec![drainer],
        inner,
        kill_tx: None,
        waiter: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        prompt_running: false,
        prompt_done: tokio_util::sync::CancellationToken::new(),
        tail,
        events: event_tx.clone(),
        established: Some(BridgeEvent::SessionEstablished {
            session: session_id,
            agent: config.agent_kind.label().into(),
            bindings: config.bindings.clone(),
            tools_visible: visible.iter().map(|tool| tool.name.clone()).collect(),
        }),
    })
}

/// Find the advertised select option and value for `selection`. Value ids
/// and display names both match, case-insensitively, so an operator can
/// write `opus` or `claude-opus-4-7` or the display name the agent shows.
pub(crate) fn resolve_config_selection(
    offered: &[agent_client_protocol::schema::v1::SessionConfigOption],
    selection: &super::session::ConfigSelection,
) -> Result<
    (
        agent_client_protocol::schema::v1::SessionConfigId,
        agent_client_protocol::schema::v1::SessionConfigValueId,
    ),
    OpenSessionError,
> {
    use super::session::ConfigCategory;
    use agent_client_protocol::schema::v1::{
        SessionConfigKind, SessionConfigOptionCategory, SessionConfigSelectOptions,
    };
    let (wanted, label) = match selection.category {
        ConfigCategory::Model => (SessionConfigOptionCategory::Model, "model"),
        ConfigCategory::ThoughtLevel => {
            (SessionConfigOptionCategory::ThoughtLevel, "reasoning level")
        },
    };
    let requested = selection.value.trim();
    let mut offered_values = Vec::new();
    for option in offered
        .iter()
        .filter(|o| o.category.as_ref() == Some(&wanted))
    {
        let SessionConfigKind::Select(select) = &option.kind else {
            continue;
        };
        let choices: Vec<_> = match &select.options {
            SessionConfigSelectOptions::Ungrouped(options) => options.iter().collect(),
            SessionConfigSelectOptions::Grouped(groups) => {
                groups.iter().flat_map(|g| g.options.iter()).collect()
            },
            _ => Vec::new(),
        };
        for choice in choices {
            offered_values.push(choice.value.to_string());
            if choice.value.to_string().eq_ignore_ascii_case(requested)
                || choice.name.eq_ignore_ascii_case(requested)
            {
                return Ok((option.id.clone(), choice.value.clone()));
            }
        }
    }
    Err(OpenSessionError::ConfigOptionUnavailable {
        category: label,
        requested: requested.to_string(),
        offered: if offered_values.is_empty() {
            "none".into()
        } else {
            offered_values.join(", ")
        },
    })
}

async fn reap_failed_open(child: &mut Child) {
    if let Err(error) = kill_owned_child(child) {
        warn!(%error, "handshake cleanup kill failed; awaiting actual exit");
    }
    loop {
        match child.wait().await {
            Ok(_) => return,
            Err(error) => {
                warn!(%error, "handshake child reap unconfirmed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            },
        }
    }
}

/// Terminate the owned agent launcher and, on Unix, its isolated process group.
pub(crate) fn kill_owned_child(child: &mut Child) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(id) = child.id() {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        let pid = i32::try_from(id).map_err(std::io::Error::other)?;
        match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {},
            Err(error) => return Err(std::io::Error::from_raw_os_error(error as i32)),
        }
    }
    child.start_kill()
}

async fn handshake_step<T>(
    future: impl std::future::Future<Output = crate::sdk_v1::CallResult<T>>,
    phase: &'static str,
    shutdown: &tokio_util::sync::CancellationToken,
    reply: &mut tokio::sync::oneshot::Sender<
        Result<surge_core::execution_recovery::OpenedSession, OpenSessionError>,
    >,
    deadline: tokio::time::Instant,
    timeout: Duration,
    fence: Option<&Arc<dyn super::HostEffectFence>>,
) -> Result<T, OpenSessionError> {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => Err(OpenSessionError::Cancelled),
        () = reply.closed() => Err(OpenSessionError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(OpenSessionError::HandshakeTimedOut { phase, timeout }),
        result = super::effect_fence::admitted(fence, future) => result?.map_err(|error| match error {
            crate::sdk_v1::SdkCallError::HostEffectRefused(error) => error.into(),
            crate::sdk_v1::SdkCallError::Protocol(error) => OpenSessionError::HandshakeFailed { reason: format!("{phase}: {error}") },
        }),
    }
}

/// Filter a combined tool list through the sandbox's `visibility` decision.
///
/// Returns `(visible_tools, hidden_tool_names)`. The hidden list is used for
/// debug logging only — the bridge does not surface it to callers.
pub(super) fn filter_visible_tools(
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
    secrets: Arc<SecretsRedactor>,
) {
    let mut raw_tail = Vec::new();
    let mut scratch = vec![0u8; 4096];
    loop {
        match stderr.read(&mut scratch).await {
            Ok(0) => break,
            Ok(n) => {
                if !secrets.has_literals()
                    && let Ok(s) = std::str::from_utf8(&scratch[..n])
                {
                    warn!(session = %session_id, "agent stderr: {}", s.trim_end());
                }
                let mut tail = tail_storage.borrow_mut();
                raw_tail.extend_from_slice(&scratch[..n]);
                if raw_tail.len() > STDERR_RING_CAP {
                    let excess = raw_tail.len() - STDERR_RING_CAP;
                    raw_tail.drain(..excess);
                }
                *tail = secrets
                    .redact_json(&String::from_utf8_lossy(&raw_tail))
                    .into_bytes();
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
            },
            Err(e) => {
                warn!(session = %session_id, "stderr read failed: {e}");
                break;
            },
        }
    }
}

/// Read the current contents of the stderr tail buffer as a String.
pub(super) fn read_stderr_tail(tail_storage: &std::rc::Rc<std::cell::RefCell<Vec<u8>>>) -> String {
    let buf = tail_storage.borrow();
    String::from_utf8_lossy(&buf).into_owned()
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
            session: *session_id,
            prompt_tokens: u.prompt_tokens,
            output_tokens: u.output_tokens,
            cache_hits: u.cache_hits,
            model: u.model,
        });
        state.borrow_mut().last_token_usage_emitted = true;
    }
}

/// Handle `BridgeCommand::ReplyToTool` — engine-driven tool reply routing.
///
/// Looks up the session and the `call_id` in the per-session
/// `open_tool_calls` map (populated by `handle_tool_call` when the agent
/// fired the matching `SessionUpdate::ToolCall` notification). On a hit:
/// removes the entry and broadcasts `BridgeEvent::ToolResult` carrying the
/// engine-supplied payload. On a miss: returns `SessionGone` (unknown
/// session) or `UnknownCallId` (session present but call_id not pending).
///
/// **ACP semantics caveat.** The ACP protocol (v1, SDK 0.10.4) has no
/// client→agent "tool result" RPC method — `SessionUpdate::ToolCall` is a
/// one-way agent→client notification. Surge's `reply_to_tool` therefore
/// does *not* deliver the payload to the agent subprocess at the wire
/// level; the agent has already moved on. The reply API exists so that the
/// engine can correlate dispatcher results with the originating tool-call
/// event for run-event persistence and observability subscribers. If a
/// future Surge milestone needs out-of-band tool delivery to the agent,
/// the natural extension point is `connection.ext_notification(...)` with
/// a vendor-specific method — not covered by M5.1.
///
/// Synchronous (no async work needed) since broadcast emission is
/// non-blocking. Kept as a free function to mirror the `*_impl` pattern of
/// the other worker arms.
pub(super) fn reply_to_tool_impl(
    sessions: &SessionMap,
    event_tx: &broadcast::Sender<BridgeEvent>,
    session: SessionId,
    call_id: String,
    payload: super::event::ToolResultPayload,
) -> Result<(), super::error::ReplyToToolError> {
    use super::error::ReplyToToolError;

    // Look up the session entry and clone the inner-state Rc out from under
    // the immutable borrow before mutating, so we never hold both borrows of
    // the sessions map at once.
    let inner = {
        let map = sessions.borrow();
        let Some(s) = map.get(&session) else {
            return Err(ReplyToToolError::SessionGone);
        };
        s.inner.clone()
    };

    // Remove the call_id from open_tool_calls. If absent, this is either an
    // engine bug (replying twice) or a stale call_id; surface as
    // UnknownCallId so the caller can decide.
    let removed = inner.borrow_mut().open_tool_calls.remove(&call_id);
    if removed.is_none() {
        return Err(ReplyToToolError::UnknownCallId(call_id));
    }

    // Broadcast the result for observability subscribers (engine
    // execute_agent_stage, run-event persisters, telemetry).
    let _ = event_tx.send(BridgeEvent::ToolResult {
        session,
        call_id,
        payload,
    });

    Ok(())
}

/// Worker-side implementation of `AcpBridge::reply_to_permission`.
///
/// Removes the `request_id` from `SessionStateInner::pending_permissions`
/// and fulfils its oneshot with the engine's response.
///
/// Failure modes:
/// - `SessionGone` when the session is missing from the session map.
/// - `UnknownRequestId` when no pending entry exists (already replied, timed
///   out, or never issued).
pub(super) fn reply_to_permission_impl(
    sessions: &SessionMap,
    session: SessionId,
    request_id: String,
    response: agent_client_protocol::schema::v1::RequestPermissionResponse,
) -> Result<(), super::error::ReplyToPermissionError> {
    use super::error::ReplyToPermissionError;

    let inner = {
        let map = sessions.borrow();
        let Some(s) = map.get(&session) else {
            return Err(ReplyToPermissionError::SessionGone);
        };
        s.inner.clone()
    };

    let Some(tx) = inner.borrow_mut().pending_permissions.remove(&request_id) else {
        return Err(ReplyToPermissionError::UnknownRequestId(request_id));
    };

    // If the agent already gave up (very short timeout, agent crash) the
    // receiver may have been dropped — swallow the SendError; the agent is
    // already gone and there is nothing more to do here.
    if tx.send(response).is_err() {
        warn!(
            target: "surge_acp.bridge.worker",
            session = %session,
            request_id = %request_id,
            "permission oneshot receiver dropped before reply; agent already cancelled"
        );
    }
    Ok(())
}

/// Complete one prompt without holding session-map borrows.
pub(crate) async fn send_message_impl(
    connection: Option<Rc<ClientConnection>>,
    acp_session_str: String,
    session: SessionId,
    content: crate::bridge::session::MessageContent,
    secrets: Arc<SecretsRedactor>,
    effect_fence: Option<Arc<dyn super::HostEffectFence>>,
) -> Result<(), super::error::SendMessageError> {
    use crate::bridge::session::MessageContent;
    let connection =
        connection.ok_or(super::error::SendMessageError::SessionNotFound { session })?;
    let blocks = match content {
        MessageContent::Text(text) => crate::shared::content_block::text_vec(text),
        MessageContent::Blocks(blocks) => blocks,
    };
    let req = agent_client_protocol::schema::v1::PromptRequest::new(
        agent_client_protocol::schema::v1::SessionId::new(acp_session_str),
        blocks,
    );
    let response = super::effect_fence::admitted(effect_fence.as_ref(), connection.prompt(req)).await?.map_err(|error| {
        let e = match error {
            crate::sdk_v1::SdkCallError::HostEffectRefused(error) => return error.into(),
            crate::sdk_v1::SdkCallError::Protocol(error) => error,
        };
        let classified = classify_prompt_dispatch_error(secrets.redact_json(&e.to_string()));
        match &classified {
            super::error::SendMessageError::AgentAuthenticationFailed { .. } => {
                warn!(session = %session, error = %classified, "ACP prompt dispatch failed: agent authentication error");
            },
            super::error::SendMessageError::RateLimited { retry_after, .. } => {
                warn!(session = %session, error = %classified, retry_after = ?retry_after, "ACP prompt dispatch failed: rate limit / quota exhaustion");
            },
            _ => {
                warn!(session = %session, error = %classified, "ACP prompt dispatch failed");
            },
        }
        classified
    })?;

    if response.stop_reason == agent_client_protocol::schema::v1::StopReason::Cancelled {
        return Err(super::error::SendMessageError::SessionEnded {
            session,
            reason: super::SessionEndReason::ForcedClose,
        });
    }
    Ok(())
}

/// Map a failed `connection.prompt(...)` error into the most accurate
/// `SendMessageError`.
///
/// Arm order is load-bearing and must stay auth → rate-limit → generic: an
/// authentication failure (HTTP 401 / `authentication_error`) is checked
/// first so a 401 whose text also happens to contain rate-limit vocabulary
/// (e.g. "too many requests") still classifies as
/// [`super::error::SendMessageError::AgentAuthenticationFailed`] rather than
/// [`super::error::SendMessageError::RateLimited`] — checking the arms in
/// the other order would tell an operator to wait out a rate limit when the
/// real problem is that the agent runtime isn't logged in.
///
/// A rate limit / quota-exhaustion signal
/// ([`surge_core::capacity::looks_like_rate_limit`]) becomes
/// [`super::error::SendMessageError::RateLimited`], carrying whatever
/// `retry_after` the same text yields via
/// [`surge_core::capacity::parse_retry_after_secs`] (`None` when the text
/// carries no recoverable delay — this classifier never invents one).
///
/// Every other failure stays a generic bridge transport error, preserving
/// the previous behaviour.
/// Whether the agent's error says the configured model is unavailable to the
/// account (observed live from codex-acp: "The 'x' model is not supported when
/// using Codex with a ChatGPT account"). Deliberately narrow: it must name a
/// model *and* say it is unsupported or missing.
fn looks_like_unsupported_model(details: &str) -> bool {
    let text = details.to_ascii_lowercase();
    text.contains("model_not_found")
        || (text.contains("model")
            && (text.contains("is not supported") || text.contains("does not exist")))
}

pub(crate) fn classify_prompt_dispatch_error(details: String) -> super::error::SendMessageError {
    if crate::pool::is_auth_failure(&details) {
        super::error::SendMessageError::AgentAuthenticationFailed { details }
    } else if looks_like_unsupported_model(&details) {
        super::error::SendMessageError::AgentModelUnsupported { details }
    } else if surge_core::capacity::looks_like_rate_limit(&details) {
        let retry_after =
            surge_core::capacity::parse_retry_after_secs(&details).map(Duration::from_secs);
        super::error::SendMessageError::RateLimited {
            retry_after,
            details,
        }
    } else {
        super::error::SendMessageError::Bridge(super::error::BridgeError::CommandSendFailed(
            details,
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::sandbox::DenyListSandbox;
    use crate::bridge::tools::{ToolCategory, ToolDef};
    use serde_json::json;

    /// Observed live from codex-acp (2026-09-28): a verifier stage failed the
    /// whole run instead of parking because this wording was unrecognised.
    fn model_options() -> Vec<agent_client_protocol::schema::v1::SessionConfigOption> {
        use agent_client_protocol::schema::v1::{
            SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
        };
        vec![
            SessionConfigOption::select(
                "model",
                "Model",
                "sonnet",
                vec![
                    SessionConfigSelectOption::new("sonnet", "Claude Sonnet"),
                    SessionConfigSelectOption::new("opus", "Claude Opus"),
                ],
            )
            .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "effort",
                "Reasoning",
                "medium",
                vec![
                    SessionConfigSelectOption::new("low", "Low"),
                    SessionConfigSelectOption::new("high", "High"),
                ],
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ]
    }

    #[test]
    fn config_selection_matches_value_or_name_within_its_category() {
        use crate::bridge::session::{ConfigCategory, ConfigSelection};
        let offered = model_options();
        let by_value = ConfigSelection {
            category: ConfigCategory::Model,
            value: "OPUS".into(),
            best_effort: false,
        };
        let (id, value) = resolve_config_selection(&offered, &by_value).unwrap();
        assert_eq!(
            (id.to_string(), value.to_string()),
            ("model".into(), "opus".into())
        );
        let by_name = ConfigSelection {
            category: ConfigCategory::Model,
            value: "Claude Sonnet".into(),
            best_effort: false,
        };
        assert_eq!(
            resolve_config_selection(&offered, &by_name)
                .unwrap()
                .1
                .to_string(),
            "sonnet"
        );
        let effort = ConfigSelection {
            category: ConfigCategory::ThoughtLevel,
            value: "high".into(),
            best_effort: false,
        };
        let (id, _) = resolve_config_selection(&offered, &effort).unwrap();
        assert_eq!(id.to_string(), "effort");
    }

    #[test]
    fn config_selection_the_agent_does_not_offer_is_refused_with_its_choices() {
        use crate::bridge::session::{ConfigCategory, ConfigSelection};
        let wrong = ConfigSelection {
            category: ConfigCategory::Model,
            value: "gpt-9".into(),
            best_effort: false,
        };
        let error = resolve_config_selection(&model_options(), &wrong).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("gpt-9") && text.contains("sonnet, opus"),
            "{text}"
        );
        // A model value is never matched against the reasoning-level option.
        let cross = ConfigSelection {
            category: ConfigCategory::Model,
            value: "high".into(),
            best_effort: false,
        };
        assert!(resolve_config_selection(&model_options(), &cross).is_err());
        let none = resolve_config_selection(&[], &wrong)
            .unwrap_err()
            .to_string();
        assert!(none.contains("offered: none"), "{none}");
    }

    #[test]
    fn classify_prompt_error_captured_codex_usage_limit() {
        let details = "Internal error: {\n  \"message\": \"You've hit your usage limit. Visit \
                       https://chatgpt.com/codex/settings/usage to purchase more credits or try \
                       again at Oct 5th, 2026 6:19 AM.\"\n}";
        let error = classify_prompt_dispatch_error(details.into());
        assert!(
            matches!(
                error,
                super::super::error::SendMessageError::RateLimited { .. }
            ),
            "codex subscription quota must reach capacity handling: {error:?}"
        );
    }

    #[test]
    fn classify_prompt_error_captured_claude_limit() {
        let details = "Internal error: You've hit your limit · resets 1pm (America/Chicago)";
        let error = classify_prompt_dispatch_error(details.into());
        assert!(
            matches!(
                error,
                super::super::error::SendMessageError::RateLimited {
                    retry_after: None,
                    details: ref actual,
                } if actual == details
            ),
            "captured subscription quota must reach capacity handling: {error:?}"
        );
    }

    #[test]
    fn classify_prompt_error_maps_unsupported_model_to_its_own_variant() {
        use crate::bridge::error::SendMessageError;
        let live = "Internal error: {\"message\": \"{\\\"type\\\":\\\"error\\\",\\\"status\\\":400,\\\"error\\\":{\\\"message\\\":\\\"The 'gpt-6-luna' model is not supported when using Codex with a ChatGPT account.\\\"}}\"}";
        assert!(matches!(
            classify_prompt_dispatch_error(live.to_string()),
            SendMessageError::AgentModelUnsupported { .. }
        ));
        // A generic failure that merely mentions a model stays a bridge error.
        assert!(matches!(
            classify_prompt_dispatch_error("model warm-up took 3s, channel closed".into()),
            SendMessageError::Bridge(_)
        ));
    }

    #[test]
    fn classify_prompt_error_maps_401_to_agent_auth_failed() {
        let details = "Internal error: Failed to authenticate. API Error: 401 {\"type\":\"error\",\
             \"error\":{\"type\":\"authentication_error\"}}"
            .to_string();
        let err = classify_prompt_dispatch_error(details.clone());
        assert!(
            matches!(
                err,
                super::super::error::SendMessageError::AgentAuthenticationFailed { .. }
            ),
            "401 prompt error should map to AgentAuthenticationFailed, got: {err:?}"
        );
        let rendered = err.to_string();
        assert!(
            rendered.contains("failed to authenticate") && rendered.contains("logged in"),
            "auth error should carry actionable guidance, got: {rendered}"
        );
        assert!(
            rendered.contains(&details),
            "auth error should preserve the raw details for debugging, got: {rendered}"
        );
    }

    #[test]
    fn classify_prompt_error_keeps_non_auth_as_bridge_error() {
        let err = classify_prompt_dispatch_error("connection reset by peer".to_string());
        assert!(
            matches!(
                err,
                super::super::error::SendMessageError::Bridge(
                    super::super::error::BridgeError::CommandSendFailed(_)
                )
            ),
            "non-auth prompt error should stay a bridge transport error, got: {err:?}"
        );
    }

    /// Arm order is mandated: auth before rate-limit before generic. A 401
    /// message that *also* happens to contain rate-limit vocabulary must
    /// still classify as [`super::super::error::SendMessageError::AgentAuthenticationFailed`] —
    /// the reverse order would tell an operator to wait out a rate limit
    /// when the real problem is that the agent runtime isn't logged in.
    #[test]
    fn classify_prompt_error_401_wins_over_rate_limit_vocabulary() {
        let details =
            "401 Unauthorized: too many requests from an unauthenticated client".to_string();
        let err = classify_prompt_dispatch_error(details);
        assert!(
            matches!(
                err,
                super::super::error::SendMessageError::AgentAuthenticationFailed { .. }
            ),
            "a 401 message must classify as auth failure even when it also \
             contains rate-limit vocabulary, got: {err:?}"
        );
    }

    /// M0 measurement (this task's mandated deliverable): for each realistic
    /// shape a provider's rate-limit / quota-exhaustion error might take
    /// once it reaches `classify_prompt_dispatch_error` as raw ACP error
    /// text, record whether the classifier recovers a `retry_after`. Every
    /// case here MUST still classify as `RateLimited` (R37's "exhausted, no
    /// reset time known" rung); `retry_after` is allowed to be `None`
    /// case-by-case — the whole point of this table is finding out how
    /// often it actually is — but not in *every* case, which the trailing
    /// assertion guards.
    ///
    /// **Every `details` string below is reconstructed for this test, not
    /// captured from a live provider or a real agent runtime.** None of them
    /// is a response this task actually observed — this task had no live
    /// Claude Code / Codex / Gemini CLI instance to rate-limit and capture
    /// from, and the classifier in production sees `e.to_string()` of an
    /// ACP JSON-RPC error produced by *that runtime's own adapter*, not a
    /// provider's raw HTTP/JSON body directly. Treat the "reachable" /
    /// "not reachable" readings here as a plausibility argument against
    /// today's parser, not a measured fact about any specific provider or
    /// runtime — see `docs/adr/0016-capacity-parking-and-wake.md`'s
    /// Measurement section for the same caveat and the full table.
    #[test]
    fn classify_prompt_error_rate_limit_shape_measurement() {
        struct Case {
            name: &'static str,
            details: &'static str,
            expect_retry_after_secs: Option<u64>,
        }
        let cases = [
            Case {
                name: "429 with a literal 'Retry-After: N' line",
                details: "429 Too Many Requests: Retry-After: 30",
                expect_retry_after_secs: Some(30),
            },
            Case {
                name: "429 with no retry hint at all",
                details: "429 Too Many Requests",
                expect_retry_after_secs: None,
            },
            Case {
                name: "shape of Anthropic's documented rate_limit_error type (reconstructed, not captured)",
                details: "API Error: 429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\
                          \"message\":\"Number of request tokens has exceeded your per-minute rate limit\"}}",
                expect_retry_after_secs: None,
            },
            Case {
                name: "OpenAI rate_limit_exceeded (prose uses \"try again in Ns\", which the parser does not recognize)",
                details: "Rate limit reached for requests. Please try again in 20s. \
                          {\"error\":{\"type\":\"rate_limit_exceeded\"}}",
                expect_retry_after_secs: None,
            },
            Case {
                name: "OpenAI insufficient_quota without 429 (not enough evidence to park)",
                details: "You exceeded your current quota, please check your plan and billing details. \
                          {\"error\":{\"type\":\"insufficient_quota\"}}",
                expect_retry_after_secs: None,
            },
            Case {
                name: "Google RESOURCE_EXHAUSTED with explicit 429",
                details: "429 Resource has been exhausted (e.g. check quota). \
                          {\"error\":{\"code\":429,\"status\":\"RESOURCE_EXHAUSTED\"}}",
                expect_retry_after_secs: None,
            },
            Case {
                name: "Anthropic overloaded_error (529 transient overload, not quota)",
                details: "Overloaded {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\
                          \"message\":\"Overloaded\"}}",
                expect_retry_after_secs: None,
            },
            Case {
                name: "usage limit reached with explicit reset time but no 429",
                details: "Claude AI usage limit reached|1735500000",
                expect_retry_after_secs: None,
            },
            Case {
                name: "Surge's own SurgeError::RateLimit Display shape (capacity.rs's own doctest text)",
                details: "Rate limit exceeded for agent 'implementer': retry after 30s (attempt 2)",
                expect_retry_after_secs: Some(30),
            },
        ];

        let mut any_retry_after_recovered = false;
        for case in cases {
            let err = classify_prompt_dispatch_error(case.details.to_string());
            if case.name.starts_with("Anthropic overloaded_error") {
                assert!(
                    matches!(err, super::super::error::SendMessageError::Bridge(_)),
                    "case {:?}: transient overload must not become a quota park: {err:?}",
                    case.name
                );
                continue;
            }
            let super::super::error::SendMessageError::RateLimited { retry_after, .. } = err else {
                panic!("case {:?}: expected RateLimited, got {err:?}", case.name);
            };
            let expected = case.expect_retry_after_secs.map(Duration::from_secs);
            assert_eq!(
                retry_after, expected,
                "case {:?}: retry_after mismatch",
                case.name
            );
            any_retry_after_recovered |= retry_after.is_some();
        }
        assert!(
            any_retry_after_recovered,
            "measurement found NOT ONE provider-error shape yielding a recoverable \
             retry_after — R37's park-with-known-wake-time rule would be unreachable \
             in production; this must escalate, not ship silently"
        );
    }

    /// M0 measurement, documenting a real gap: bare "retry after Ns" prose
    /// with no accompanying rate-limit vocabulary never reaches the
    /// rate-limit arm at all — `looks_like_rate_limit` requires "429" or one
    /// of its known keywords first. An agent runtime that only ever emits
    /// bare retry-delay prose (never naming "429"/"rate limit"/etc.) is
    /// invisible to this classifier and stays a generic `Bridge` error.
    #[test]
    fn classify_prompt_error_bare_retry_after_prose_without_rate_limit_keyword_is_not_recognized() {
        let err =
            classify_prompt_dispatch_error("please retry after 30s and try again".to_string());
        assert!(
            matches!(
                err,
                super::super::error::SendMessageError::Bridge(
                    super::super::error::BridgeError::CommandSendFailed(_)
                )
            ),
            "bare retry-after prose with no rate-limit keyword should NOT be \
             recognized as a rate limit (measured gap), got: {err:?}"
        );
    }

    #[test]
    fn resolve_program_finds_path_extension_shim() {
        // `cargo` is always present where tests run. The key property: a bare
        // program name resolves to a concrete existing path (on Windows this
        // exercises PATHEXT — the std `Command::new("npx")` gap this guards).
        let resolved = resolve_program(Path::new("cargo"));
        assert!(
            resolved.exists(),
            "cargo should resolve to an existing path, got {resolved:?}"
        );
    }

    #[test]
    fn resolve_program_falls_back_to_original_when_missing() {
        // Unresolvable names pass through unchanged so spawn still surfaces a
        // clear NotFound rather than this helper masking the error.
        let missing = Path::new("__surge_definitely_not_a_real_program__");
        assert_eq!(resolve_program(missing), missing.to_path_buf());
    }

    /// Args (after the program) `build_agent_command` produces for a kind.
    /// The program path is resolution-dependent; the *args* are the
    /// per-runtime contract that decides whether a launch actually works.
    #[cfg(unix)]
    #[tokio::test]
    async fn forced_cleanup_closes_descendant_processes() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

        let mut command = tokio::process::Command::new("sh");
        command
            .args(["-c", "sleep 60 & echo ready; wait"])
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().expect("spawn isolated launcher");
        let mut stdout = BufReader::new(child.stdout.take().expect("launcher stdout"));
        let mut ready = String::new();
        stdout.read_line(&mut ready).await.expect("child started");
        assert_eq!(ready.trim(), "ready");
        super::kill_owned_child(&mut child).expect("kill owned group");
        child.wait().await.expect("reap launcher");
        // The descendant inherited this pipe. Killing only the launcher leaves
        // it open for 60 seconds, independently of the launcher's exit status.
        let mut remaining = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            stdout.read_to_end(&mut remaining),
        )
        .await
        .expect("descendant must close its inherited pipe")
        .expect("read EOF");
    }

    fn launch_args(kind: &AgentKind) -> Vec<String> {
        let cmd =
            build_agent_command(kind, Path::new("."), &BTreeMap::new()).expect("build command");
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn build_agent_command_sets_resolved_env_on_the_child() {
        let kind = AgentKind::Custom {
            binary: PathBuf::from("__surge_custom_test__"),
            args: vec![],
        };
        let mut env = BTreeMap::new();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            "https://ollama.com".to_string(),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), String::new());
        let cmd = build_agent_command(&kind, Path::new("."), &env).expect("build command");
        let seen: BTreeMap<String, String> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert_eq!(
            seen.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://ollama.com")
        );
        assert_eq!(seen.get("ANTHROPIC_API_KEY").map(String::as_str), Some(""));
    }

    #[test]
    fn claude_kind_injects_acp_flag_before_extra_args() {
        let kind = AgentKind::ClaudeCode {
            binary: PathBuf::from("__surge_claude_test__"),
            extra_args: vec!["--model".into(), "opus".into()],
        };
        assert_eq!(launch_args(&kind), vec!["--acp", "--model", "opus"]);
    }

    #[test]
    fn codex_kind_uses_acp_subcommand_not_flag() {
        // Codex takes a bare `acp` subcommand, NOT `--acp`. Swapping the two
        // silently breaks the launch — pin it.
        let kind = AgentKind::Codex {
            binary: PathBuf::from("__surge_codex_test__"),
            extra_args: vec![],
        };
        assert_eq!(launch_args(&kind), vec!["acp"]);
    }

    #[test]
    fn gemini_kind_injects_acp_flag() {
        let kind = AgentKind::GeminiCli {
            binary: PathBuf::from("__surge_gemini_test__"),
            extra_args: vec!["-y".into()],
        };
        assert_eq!(launch_args(&kind), vec!["--acp", "-y"]);
    }

    #[test]
    fn custom_kind_passes_args_verbatim_no_injected_flag() {
        // The npx registry model lands on `Custom`: the package args already
        // encode the full invocation, so the builder must NOT inject `--acp`
        // (doing so would double the flag and break the launch).
        let kind = AgentKind::Custom {
            binary: PathBuf::from("npx"),
            args: vec!["@zed-industries/codex-acp".into()],
        };
        assert_eq!(launch_args(&kind), vec!["@zed-industries/codex-acp"]);
    }

    #[test]
    fn filter_removes_denied_tools() {
        let tools = vec![
            ToolDef::new("read_file", "d", ToolCategory::Builtin, json!({})),
            ToolDef::new(
                "shell_exec",
                "d",
                ToolCategory::Mcp("ops".into()),
                json!({}),
            ),
        ];
        let s = DenyListSandbox::deny_tools(["shell_exec"]);
        let (visible, hidden) = filter_visible_tools(tools, &s);
        let names: Vec<_> = visible.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_file"]);
        assert_eq!(hidden, vec!["shell_exec"]);
    }
}
