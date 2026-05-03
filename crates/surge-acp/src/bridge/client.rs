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
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    ReleaseTerminalResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, Result as AcpResult, SelectedPermissionOutcome, SessionNotification,
    TerminalExitStatus, TerminalId, TerminalOutputRequest, TerminalOutputResponse,
    WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use surge_core::SessionId;
use tokio::sync::{Mutex, broadcast};
use tracing::debug;

use crate::shared::path_guard::ensure_in_worktree;
use crate::shared::secrets::SecretsRedactor;
use crate::terminal::{
    Terminals, terminal_get_output, terminal_kill, terminal_release, terminal_wait_for_exit,
};

use super::event::BridgeEvent;
use super::sandbox::{Sandbox, SandboxDecision};
use super::session_inner::SessionStateInner;

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
    pub(crate) bindings: BTreeMap<String, String>,
    /// Canonicalized worktree root used for path-guard checks.
    pub(crate) worktree_root: PathBuf,
    /// Terminal process manager for this session.
    pub(crate) terminals: Arc<Mutex<Terminals>>,
}

impl BridgeClient {
    /// Construct a new `BridgeClient`.
    ///
    /// `terminals` is initialised from `worktree_root` so that every spawned
    /// process is rooted at the correct worktree for this session.
    #[allow(dead_code)] // wired by Phase 8.1 open_session_impl when constructing per-session client
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
    /// Consult the `Sandbox` and return Allow or Deny without prompting the
    /// user. Both `Deny` and `Elevate` decisions resolve to a "deny" option
    /// here; the elevation flow is handled at tool-dispatch time (Phase 8).
    async fn request_permission(
        &self,
        req: RequestPermissionRequest,
    ) -> AcpResult<RequestPermissionResponse> {
        // The tool name lives at req.tool_call.fields.title (Option<String>).
        // Fall back to the empty string if the agent omitted it.
        let tool_name = req
            .tool_call
            .fields
            .title
            .clone()
            .unwrap_or_default();
        let mcp_id: Option<String> = None; // Phase 8 fills in once tool dispatch lands

        let decision = self.sandbox.allows_tool(&tool_name, mcp_id.as_deref());
        debug!(
            session = %self.session_id,
            tool = %tool_name,
            decision = ?decision,
            "request_permission via Sandbox"
        );

        let outcome = match decision {
            SandboxDecision::Allow => {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    PermissionOptionId::new("allow"),
                ))
            }
            SandboxDecision::Deny { .. } | SandboxDecision::Elevate { .. } => {
                // Both Deny and Elevate result in a denial in M3. Elevate routes
                // to the engine via `BridgeEvent::ToolCall::sandbox_decision`
                // attached at tool-dispatch time (Phase 8); request_permission
                // here is a fast denial path.
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    PermissionOptionId::new("deny"),
                ))
            }
        };

        Ok(RequestPermissionResponse::new(outcome))
    }

    /// Write a text file within the worktree. Path-guard enforced before any IO.
    async fn write_text_file(
        &self,
        req: WriteTextFileRequest,
    ) -> AcpResult<WriteTextFileResponse> {
        // SDK Error::invalid_params() takes no message arg (0.10.2 shape).
        ensure_in_worktree(&self.worktree_root, &req.path)
            .map_err(|_| agent_client_protocol::Error::invalid_params())?;
        // Redact secrets from the content before writing? No — content is what
        // the agent wants persisted. Redaction applies to *event payloads*, not
        // file contents.
        tokio::fs::write(&req.path, &req.content)
            .await
            .map_err(|_| agent_client_protocol::Error::internal_error())?;
        Ok(WriteTextFileResponse::new())
    }

    /// Read a text file within the worktree. Path-guard enforced before any IO.
    async fn read_text_file(
        &self,
        req: ReadTextFileRequest,
    ) -> AcpResult<ReadTextFileResponse> {
        ensure_in_worktree(&self.worktree_root, &req.path)
            .map_err(|_| agent_client_protocol::Error::invalid_params())?;
        let content = tokio::fs::read_to_string(&req.path)
            .await
            .map_err(|_| agent_client_protocol::Error::internal_error())?;
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
            .map_err(|_| agent_client_protocol::Error::internal_error())?;

        Ok(CreateTerminalResponse::new(TerminalId::new(terminal_id)))
    }

    /// Get current output and exit status of a terminal without blocking.
    async fn terminal_output(
        &self,
        req: TerminalOutputRequest,
    ) -> AcpResult<TerminalOutputResponse> {
        let id = req.terminal_id.0.as_ref();
        let (output, truncated, exit_opt) =
            terminal_get_output(&self.terminals, id)
                .await
                .map_err(|_| agent_client_protocol::Error::internal_error())?;

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
            .map_err(|_| agent_client_protocol::Error::internal_error())?;

        // Build response via constructors (non_exhaustive structs).
        let exit_status = TerminalExitStatus::new()
            .exit_code(s.exit_code)
            .signal(s.signal);
        Ok(WaitForTerminalExitResponse::new(exit_status))
    }

    /// Kill the terminal process without releasing it.
    async fn kill_terminal(
        &self,
        req: KillTerminalRequest,
    ) -> AcpResult<KillTerminalResponse> {
        let id = req.terminal_id.0.as_ref();
        terminal_kill(&self.terminals, id)
            .await
            .map_err(|_| agent_client_protocol::Error::internal_error())?;
        Ok(KillTerminalResponse::new())
    }

    /// Release a terminal (kills the process if still running, then drops it).
    async fn release_terminal(
        &self,
        req: ReleaseTerminalRequest,
    ) -> AcpResult<ReleaseTerminalResponse> {
        let id = req.terminal_id.0.as_ref();
        terminal_release(&self.terminals, id)
            .await
            .map_err(|_| agent_client_protocol::Error::internal_error())?;
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
            &self.sandbox,
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
            }
            other => panic!("expected Selected(allow), got {other:?}"),
        }
    }
}
