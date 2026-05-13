//! `BridgeClient` — ACP `Client` trait impl emitting `BridgeEvent`s.
//! See spec §5.9.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use agent_client_protocol::{
    Client, CreateTerminalRequest, CreateTerminalResponse, ExtNotification, ExtRequest,
    ExtResponse, KillTerminalRequest, KillTerminalResponse, PermissionOptionId,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    Result as AcpResult, SelectedPermissionOutcome, SessionNotification, TerminalExitStatus,
    TerminalId, TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use surge_core::SessionId;
use tokio::sync::{Mutex, broadcast, oneshot};
use tracing::{debug, info, warn};
use ulid::Ulid;

use crate::shared::path_guard::ensure_in_worktree;
use crate::shared::secrets::SecretsRedactor;
use crate::terminal::{
    Terminals, terminal_get_output, terminal_kill, terminal_release, terminal_wait_for_exit,
};

use super::event::BridgeEvent;
use super::sandbox::{Sandbox, SandboxDecision};
use super::session_inner::SessionStateInner;

/// Pending-permissions registry size that triggers a `warn` log. Hit usually
/// means the engine isn't draining decisions fast enough or that approval
/// channels are mis-configured.
const PENDING_PERMISSION_WARN_THRESHOLD: usize = 32;

/// Bridge-side ACP `Client` trait impl. One instance per open session.
///
/// Methods follow a uniform shape: validate (via `path_guard` or `Sandbox`),
/// do the work, emit `BridgeEvent` only when relevant. Most IO methods are
/// silent — ACP carries its own observability for those.
pub(crate) struct BridgeClient {
    /// Surge-side session identifier (distinct from the ACP session id).
    pub(crate) session_id: SessionId,
    /// Broadcast channel for emitting `BridgeEvent`s to subscribers.
    pub(crate) event_tx: broadcast::Sender<BridgeEvent>,
    /// Shared per-session mutable state (LocalSet-bound, no cross-thread sharing).
    pub(crate) state: Rc<RefCell<SessionStateInner>>,
    /// Sandbox policy consulted on every tool call.
    pub(crate) sandbox: Box<dyn Sandbox>,
    /// Secrets redactor applied to event payloads (not file contents).
    pub(crate) secrets: Arc<SecretsRedactor>,
    /// Key-value bindings (environment overrides / session metadata).
    #[allow(dead_code)] // wired in Task 8.3 SessionEstablished emit
    pub(crate) bindings: BTreeMap<String, String>,
    /// Canonicalized worktree root used for path-guard checks.
    pub(crate) worktree_root: PathBuf,
    /// Terminal process manager for this session.
    pub(crate) terminals: Arc<Mutex<Terminals>>,
}

/// Pick an `option_id` for the canned Allow/Deny response.
///
/// Tries to match `desired_id` (`"allow"` / `"deny"`) against the option IDs
/// the agent offered in the request. Falls back to constructing a fresh
/// `PermissionOptionId::new(desired_id)` when the agent omitted the
/// corresponding option — the SDK will reject genuinely unknown IDs at the
/// next layer.
fn pick_option(req: &RequestPermissionRequest, desired_id: &str) -> PermissionOptionId {
    for opt in &req.options {
        if opt.option_id.0.as_ref() == desired_id {
            return opt.option_id.clone();
        }
    }
    PermissionOptionId::new(desired_id)
}

impl BridgeClient {
    /// Park the permission request in `SessionStateInner::pending_permissions`,
    /// broadcast a `PermissionRequested` event, and await the engine's
    /// decision via `reply_to_permission`.
    ///
    /// When the receiver side drops without a value (session closed / engine
    /// failure) the agent receives `Cancelled` so it never blocks forever.
    async fn await_elevation(
        &self,
        req: RequestPermissionRequest,
        capability: String,
    ) -> AcpResult<RequestPermissionResponse> {
        let request_id = Ulid::new().to_string();
        let tool_name = req.tool_call.fields.title.clone().unwrap_or_default();
        let options: Vec<String> = req
            .options
            .iter()
            .map(|o| o.option_id.0.as_ref().to_string())
            .collect();

        let (tx, rx) = oneshot::channel::<RequestPermissionResponse>();
        {
            let mut state = self.state.borrow_mut();
            let pending_count = state.pending_permissions.len() + 1;
            state.pending_permissions.insert(request_id.clone(), tx);
            if pending_count >= PENDING_PERMISSION_WARN_THRESHOLD {
                warn!(
                    target: "surge_acp.bridge.client",
                    session = %self.session_id,
                    pending_count,
                    "pending permission registry growing — engine may be slow to respond"
                );
            }
        }

        info!(
            target: "surge_acp.bridge.client",
            session = %self.session_id,
            request_id = %request_id,
            tool = %tool_name,
            capability = %capability,
            "elevation requested by agent — awaiting engine decision"
        );

        let _ = self.event_tx.send(BridgeEvent::PermissionRequested {
            session: self.session_id,
            request_id: request_id.clone(),
            tool: tool_name,
            capability,
            options,
        });

        match rx.await {
            Ok(response) => {
                debug!(
                    target: "surge_acp.bridge.client",
                    session = %self.session_id,
                    request_id = %request_id,
                    "permission resolved by engine"
                );
                Ok(response)
            },
            Err(_recv_err) => {
                // The oneshot sender was dropped before the engine replied —
                // typically because the session is closing. Clear any stale
                // entry (`reply_to_permission` removes the entry before
                // sending; we only reach here if nothing was sent at all) and
                // tell the agent the request was cancelled.
                self.state
                    .borrow_mut()
                    .pending_permissions
                    .remove(&request_id);
                warn!(
                    target: "surge_acp.bridge.client",
                    session = %self.session_id,
                    request_id = %request_id,
                    "permission oneshot dropped before engine replied; reporting Cancelled to agent"
                );
                Ok(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            },
        }
    }

    /// Construct a new `BridgeClient`.
    ///
    /// `terminals` is initialised from `worktree_root` so that every spawned
    /// process is rooted at the correct worktree for this session.
    pub(crate) fn new(
        session_id: SessionId,
        event_tx: broadcast::Sender<BridgeEvent>,
        state: Rc<RefCell<SessionStateInner>>,
        sandbox: Box<dyn Sandbox>,
        secrets: Arc<SecretsRedactor>,
        bindings: BTreeMap<String, String>,
        worktree_root: PathBuf,
    ) -> Self {
        let terminals = Arc::new(Mutex::new(Terminals::new(worktree_root.clone())));
        Self {
            session_id,
            event_tx,
            state,
            sandbox,
            secrets,
            bindings,
            worktree_root,
            terminals,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Client for BridgeClient {
    /// Handle an ACP `request_permission` from the agent.
    ///
    /// Routing:
    /// - The local `Sandbox` is consulted first for fast-path Allow/Deny
    ///   decisions on tools surge already classifies. `Allow` and `Deny`
    ///   short-circuit so we don't roundtrip through the operator for
    ///   well-understood cases (e.g. read-only mode + read tool).
    /// - Everything else — `Elevate { .. }` from a sandbox that classifies
    ///   the call as elevation-worthy — broadcasts
    ///   [`BridgeEvent::PermissionRequested`], parks a `oneshot` in
    ///   `SessionStateInner::pending_permissions`, and awaits the engine's
    ///   `AcpBridge::reply_to_permission` decision. Sessions that end with
    ///   pending requests resolve them as `Cancelled` so the agent never
    ///   blocks past session lifetime.
    async fn request_permission(
        &self,
        req: RequestPermissionRequest,
    ) -> AcpResult<RequestPermissionResponse> {
        let tool_name = req.tool_call.fields.title.clone().unwrap_or_default();
        let mcp_id: Option<String> = None;
        let decision = self.sandbox.allows_tool(&tool_name, mcp_id.as_deref());
        debug!(
            target: "surge_acp.bridge.client",
            session = %self.session_id,
            tool = %tool_name,
            decision = ?decision,
            "request_permission via Sandbox"
        );

        match decision {
            SandboxDecision::Allow => Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(pick_option(
                    &req, "allow",
                ))),
            )),
            SandboxDecision::Deny { .. } => Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(pick_option(
                    &req, "deny",
                ))),
            )),
            SandboxDecision::Elevate { capability } => self.await_elevation(req, capability).await,
        }
    }

    /// Write a text file within the worktree. Path-guard enforced before any IO.
    ///
    /// Uses `resolve_for_write` rather than `ensure_in_worktree` so that the
    /// agent can create new files. `ensure_in_worktree` calls `canonicalize`
    /// which requires the path to already exist; `resolve_for_write` mirrors
    /// the legacy `SurgeClient::resolve_path` pattern — for non-existent paths
    /// it canonicalizes the parent directory, joins the filename, then enforces
    /// the worktree bound.
    async fn write_text_file(&self, req: WriteTextFileRequest) -> AcpResult<WriteTextFileResponse> {
        let safe_path = match resolve_for_write(&self.worktree_root, &req.path) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    session = %self.session_id,
                    error = %e,
                    "path guard rejected write_text_file",
                );
                return Err(agent_client_protocol::Error::invalid_params());
            },
        };
        // Redact secrets from the content before writing? No — content is what
        // the agent wants persisted. Redaction applies to *event payloads*, not
        // file contents.
        tokio::fs::write(&safe_path, &req.content)
            .await
            .map_err(|e| {
                warn!(
                    session = %self.session_id,
                    path = %safe_path.display(),
                    error = %e,
                    "write_text_file IO failure",
                );
                agent_client_protocol::Error::internal_error()
            })?;
        Ok(WriteTextFileResponse::new())
    }

    /// Read a text file within the worktree. Path-guard enforced before any IO.
    async fn read_text_file(&self, req: ReadTextFileRequest) -> AcpResult<ReadTextFileResponse> {
        ensure_in_worktree(&self.worktree_root, &req.path).map_err(|e| {
            warn!(
                session = %self.session_id,
                error = %e,
                "path guard rejected file access in read_text_file",
            );
            agent_client_protocol::Error::invalid_params()
        })?;
        let content = tokio::fs::read_to_string(&req.path).await.map_err(|e| {
            debug!(
                session = %self.session_id,
                path = %req.path.display(),
                error = %e,
                "read_text_file IO failure",
            );
            agent_client_protocol::Error::internal_error()
        })?;
        Ok(ReadTextFileResponse::new(content))
    }

    /// Spawn a new terminal process and return its `TerminalId`.
    async fn create_terminal(
        &self,
        req: CreateTerminalRequest,
    ) -> AcpResult<CreateTerminalResponse> {
        // Convert ACP EnvVariable list to the (name, value) tuples legacy Terminals expects.
        let env: Vec<(String, String)> = req
            .env
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect();

        let terminal_id = self
            .terminals
            .lock()
            .await
            .spawn(
                &req.command,
                &req.args,
                &env,
                req.cwd.as_ref(),
                req.output_byte_limit,
            )
            .map_err(|e| {
                warn!(
                    session = %self.session_id,
                    op = "create_terminal",
                    command = %req.command,
                    error = %e,
                    "terminal operation failed",
                );
                agent_client_protocol::Error::internal_error()
            })?;

        Ok(CreateTerminalResponse::new(TerminalId::new(terminal_id)))
    }

    /// Get current output and exit status of a terminal without blocking.
    async fn terminal_output(
        &self,
        req: TerminalOutputRequest,
    ) -> AcpResult<TerminalOutputResponse> {
        let id = req.terminal_id.0.as_ref();
        let (output, truncated, exit_opt) = terminal_get_output(&self.terminals, id)
            .await
            .map_err(|e| {
                debug!(
                    session = %self.session_id,
                    op = "terminal_output",
                    terminal_id = %id,
                    error = %e,
                    "terminal operation failed",
                );
                agent_client_protocol::Error::internal_error()
            })?;

        // Build exit status via builder (non_exhaustive struct).
        let exit_status = exit_opt.map(|s| {
            TerminalExitStatus::new()
                .exit_code(s.exit_code)
                .signal(s.signal)
        });

        Ok(TerminalOutputResponse::new(output, truncated).exit_status(exit_status))
    }

    /// Block until the terminal's process exits and return its exit status.
    async fn wait_for_terminal_exit(
        &self,
        req: WaitForTerminalExitRequest,
    ) -> AcpResult<WaitForTerminalExitResponse> {
        let id = req.terminal_id.0.as_ref();
        let s = terminal_wait_for_exit(&self.terminals, id)
            .await
            .map_err(|e| {
                warn!(
                    session = %self.session_id,
                    op = "wait_for_terminal_exit",
                    terminal_id = %id,
                    error = %e,
                    "terminal operation failed",
                );
                agent_client_protocol::Error::internal_error()
            })?;

        // Build response via constructors (non_exhaustive structs).
        let exit_status = TerminalExitStatus::new()
            .exit_code(s.exit_code)
            .signal(s.signal);
        Ok(WaitForTerminalExitResponse::new(exit_status))
    }

    /// Kill the terminal process without releasing it.
    async fn kill_terminal(&self, req: KillTerminalRequest) -> AcpResult<KillTerminalResponse> {
        let id = req.terminal_id.0.as_ref();
        terminal_kill(&self.terminals, id).await.map_err(|e| {
            warn!(
                session = %self.session_id,
                op = "kill_terminal",
                terminal_id = %id,
                error = %e,
                "terminal operation failed",
            );
            agent_client_protocol::Error::internal_error()
        })?;
        Ok(KillTerminalResponse::new())
    }

    /// Release a terminal (kills the process if still running, then drops it).
    async fn release_terminal(
        &self,
        req: ReleaseTerminalRequest,
    ) -> AcpResult<ReleaseTerminalResponse> {
        let id = req.terminal_id.0.as_ref();
        terminal_release(&self.terminals, id).await.map_err(|e| {
            warn!(
                session = %self.session_id,
                op = "release_terminal",
                terminal_id = %id,
                error = %e,
                "terminal operation failed",
            );
            agent_client_protocol::Error::internal_error()
        })?;
        Ok(ReleaseTerminalResponse::new())
    }

    /// Route incoming session notifications to `bridge::worker::handle_session_notification`.
    async fn session_notification(&self, notif: SessionNotification) -> AcpResult<()> {
        // SessionNotification carries SessionUpdate variants (agent messages,
        // tool calls, token usage). Phase 8 routes these to BridgeEvent emissions
        // via `bridge::worker::handle_session_notification`. For now, log and accept.
        debug!(
            session = %self.session_id,
            "session_notification: {:?}",
            std::mem::discriminant(&notif.update)
        );
        crate::bridge::worker::handle_session_notification(
            &self.session_id,
            &self.event_tx,
            &self.state,
            &*self.sandbox,
            &self.secrets,
            notif,
        )
        .await;
        Ok(())
    }

    /// Extension requests are not supported in M3.
    async fn ext_method(&self, _req: ExtRequest) -> AcpResult<ExtResponse> {
        // Ext methods are vendor extensions. Bridge does not implement any in M3.
        Err(agent_client_protocol::Error::method_not_found())
    }

    /// Extension notifications are accepted silently in M3.
    async fn ext_notification(&self, _notif: ExtNotification) -> AcpResult<()> {
        // No-op accept — bridge does not consume any ext notifications.
        Ok(())
    }
}

/// Worktree-rooted path resolution that allows new (non-existent) files.
///
/// For paths that already exist, behaves identically to `ensure_in_worktree`
/// (full canonicalization + bounds check). For paths that do not yet exist,
/// canonicalizes the parent directory, joins the filename, then enforces the
/// worktree bound. This mirrors the legacy `SurgeClient::resolve_path` behavior
/// so that an ACP agent can create new files without receiving a spurious
/// `InvalidParams` error.
///
/// `read_text_file` continues to use `ensure_in_worktree` directly; reading a
/// non-existent file should fail at the IO layer in any case.
fn resolve_for_write(
    worktree_root: &std::path::Path,
    path: &std::path::Path,
) -> Result<std::path::PathBuf, crate::shared::path_guard::PathGuardError> {
    use crate::shared::path_guard::{PathGuardError, ensure_in_worktree};
    if path.exists() {
        return ensure_in_worktree(worktree_root, path);
    }
    if !path.is_absolute() {
        return Err(PathGuardError::NotAbsolute {
            path: path.to_path_buf(),
        });
    }
    let parent = path.parent().ok_or_else(|| PathGuardError::NotAbsolute {
        path: path.to_path_buf(),
    })?;
    let canonical_parent = parent.canonicalize().map_err(|source| PathGuardError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(worktree_root) {
        return Err(PathGuardError::Escapes {
            path: canonical_parent.clone(),
            worktree: worktree_root.to_path_buf(),
        });
    }
    let filename = path
        .file_name()
        .ok_or_else(|| PathGuardError::NotAbsolute {
            path: path.to_path_buf(),
        })?;
    Ok(canonical_parent.join(filename))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::sandbox::AlwaysAllowSandbox;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn make_client() -> BridgeClient {
        let (tx, _) = broadcast::channel(16);
        let state = Rc::new(RefCell::new(SessionStateInner::new("acp-sess-1".into())));
        BridgeClient::new(
            SessionId::new(),
            tx,
            state,
            Box::new(AlwaysAllowSandbox),
            Arc::new(SecretsRedactor::new()),
            BTreeMap::new(),
            std::env::temp_dir(),
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_permission_allow_returns_allow_option() {
        use agent_client_protocol::{
            SessionId as AcpSessionId, ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
        };

        let client = make_client();
        let req = RequestPermissionRequest::new(
            AcpSessionId::new("sess"),
            ToolCallUpdate::new(
                ToolCallId::new("c1"),
                ToolCallUpdateFields::new().title("read_file".to_string()),
            ),
            vec![],
        );
        let resp = client.request_permission(req).await.unwrap();
        match resp.outcome {
            RequestPermissionOutcome::Selected(s) => {
                assert_eq!(s.option_id.0.as_ref(), "allow");
            },
            other => panic!("expected Selected(allow), got {other:?}"),
        }
    }
}
