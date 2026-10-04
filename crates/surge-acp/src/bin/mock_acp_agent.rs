//! Mock ACP agent for deterministic integration tests of `surge-acp::bridge`.
//!
//! Speaks real ACP via `agent-client-protocol`'s `Agent` trait.  Behavior is
//! selected via env vars and CLI args:
//!
//! **Scenarios (selected via `--scenario <name>`)**
//!
//! - `echo`              — echo user messages back as `AgentMessageChunk`
//! - `report_done`       — emit `report_stage_outcome(outcome="done")` after first message
//! - `report_outcome=K`  — emit `report_stage_outcome(outcome=K)` after first message
//! - `crash_after=N`     — process N prompts then call `std::process::exit(137)`
//! - `human_input`       — emit `request_human_input` tool call notification
//! - `long_streaming`    — emit 20 `AgentMessageChunk` notifications with 50 ms delays
//! - `frozen`            — process prompts but ignore stdin EOF (for close_timeout test)
//!
//! **Flags (CLI args — preferred for tests; env vars retained for back-compat)**
//!
//! - `--usage`               — equivalent to `MOCK_ACP_USAGE=on`
//! - `--handshake-fail`      — equivalent to `MOCK_ACP_HANDSHAKE_FAIL=1`
//! - `--log-stderr`          — equivalent to `MOCK_ACP_LOG=stderr`
//!
//! **Env var flags (honored for back-compat; use CLI flags in tests)**
//!
//! - `MOCK_ACP_USAGE=on`            — emit a `SessionUpdate::UsageUpdate`
//!   notification per prompt turn (and also attach `Usage` to the
//!   `PromptResponse`)
//! - `MOCK_ACP_HANDSHAKE_FAIL=1`    — exit(1) before creating the ACP connection
//! - `MOCK_ACP_LOG=stderr`          — write verbose diagnostics to stderr

#[path = "mock_acp_agent/sdk_v1.rs"]
mod sdk_v1;
#[path = "mock_acp_agent/stage_mcp.rs"]
mod stage_mcp;

use std::cell::Cell;
use std::env;
use std::time::Duration;

use agent_client_protocol::schema::v1 as acp;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

// ── Scenario ────────────────────────────────────────────────────────────────

/// Which behaviour the mock should exhibit during a `session/prompt` turn.
#[derive(Debug, Clone)]
enum Scenario {
    /// Echo user text back as `AgentMessageChunk`.
    Echo,
    /// Call `report_stage_outcome` with `outcome = "done"`.
    ReportDone,
    /// Call `report_stage_outcome` with a caller-supplied outcome key.
    ReportOutcome(String),
    /// Process *N* prompts, then `exit(137)`.
    CrashAfter(u32),
    /// Emit `request_human_input` tool-call notification.
    HumanInput,
    /// Emit 20 `AgentMessageChunk` notifications with 50 ms delays.
    LongStreaming,
    /// Process prompts normally but ignore stdin EOF — when the bridge drops
    /// the connection, the mock's `handle_io` returns but the process keeps
    /// running on an infinite sleep so `child.wait()` in the bridge's
    /// subprocess waiter never resolves. Used by Task 10.4
    /// `bridge_close_timeout` to exercise the 5s grace path.
    Frozen,
    /// Reject every `prompt` call with a scripted JSON-RPC error instead of
    /// processing normally. `key` selects a canned provider rate-limit /
    /// quota-exhaustion error shape (see [`provider_error_for_key`]). Used
    /// by Task 12 M0's real-ACP-wire measurement of whether
    /// `surge-acp::bridge::worker::classify_prompt_dispatch_error` can
    /// recover a `retry_after` from what actually crosses the JSON-RPC
    /// boundary (not just from a hand-built `String` in a unit test).
    PromptError(String),
}

impl Scenario {
    /// Parse the scenario flag from `args`.  Supports both `--scenario X` (two
    /// tokens) and `--scenario=X` (single token).  Falls back to `Echo` with a
    /// stderr warning if the value is missing or unrecognised.
    fn parse(args: &[String]) -> Self {
        let value = args.iter().enumerate().find_map(|(i, arg)| {
            if let Some(v) = arg.strip_prefix("--scenario=") {
                Some(v.to_string())
            } else if arg == "--scenario" {
                args.get(i + 1).cloned()
            } else {
                None
            }
        });

        let Some(value) = value else {
            return Self::ReportDone;
        };

        if let Some(k) = value.strip_prefix("report_outcome=") {
            return Self::ReportOutcome(k.to_string());
        }
        if let Some(k) = value.strip_prefix("prompt_error=") {
            return Self::PromptError(k.to_string());
        }
        if let Some(n) = value.strip_prefix("crash_after=") {
            let parsed = n.parse().unwrap_or_else(|e| {
                eprintln!("[mock_acp_agent] bad crash_after value {n:?}: {e}; defaulting to 1");
                1
            });
            return Self::CrashAfter(parsed);
        }
        match value.as_str() {
            "echo" => Self::Echo,
            "report_done" => Self::ReportDone,
            "human_input" => Self::HumanInput,
            "long_streaming" => Self::LongStreaming,
            "frozen" => Self::Frozen,
            other => {
                eprintln!("[mock_acp_agent] unknown scenario {other:?}; defaulting to echo");
                Self::Echo
            },
        }
    }
}

/// Canned JSON-RPC error bodies for [`Scenario::PromptError`], keyed by
/// name. Each mirrors a realistic ACP-error-text shape a rate-limited or
/// quota-exhausted provider could produce; `"429_retry_after"` is the one
/// shape `surge-acp::bridge::worker::classify_prompt_dispatch_error` can
/// recover a `retry_after` from (see the M0 measurement table in
/// `surge-acp/src/bridge/worker.rs`'s test module) and is what the
/// `#[ignore]`d `bridge_rate_limit_classification` integration test spawns
/// this mock with — proving the real wire preserves that text, not just
/// that the pure classifier function does.
fn provider_error_for_key(key: &str) -> acp::Error {
    match key {
        "429_retry_after" => acp::Error::new(-32000, "429 Too Many Requests: Retry-After: 30"),
        "429_no_reset" => acp::Error::new(-32000, "429 Too Many Requests"),
        other => acp::Error::internal_error().data(json!({ "unknown_prompt_error_key": other })),
    }
}

// ── Notification channel item ───────────────────────────────────────────────

/// One notification to send to the client, plus a one-shot ack channel so the
/// sender can wait until the send is flushed before continuing.
type NotifItem = (acp::SessionNotification, oneshot::Sender<()>);
type PermissionItem = (
    acp::RequestPermissionRequest,
    oneshot::Sender<acp::Result<acp::RequestPermissionResponse>>,
);

// ── MockAgent ───────────────────────────────────────────────────────────────

struct MockAgent {
    scenario: Scenario,
    usage_on: bool,
    verbose: bool,
    /// Counts how many `prompt` calls have been received (for `crash_after=N`).
    prompt_count: Cell<u32>,
    cancel_prompt: tokio_util::sync::CancellationToken,
    stage_peer: std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<stage_mcp::Peer>>>>,
    /// Channel to push `SessionNotification`s to the background sender task.
    notif_tx: mpsc::UnboundedSender<NotifItem>,
    permission_tx: mpsc::UnboundedSender<PermissionItem>,
}

impl MockAgent {
    fn new(
        scenario: Scenario,
        usage_on: bool,
        verbose: bool,
        notif_tx: mpsc::UnboundedSender<NotifItem>,
        permission_tx: mpsc::UnboundedSender<PermissionItem>,
    ) -> Self {
        Self {
            scenario,
            usage_on,
            verbose,
            prompt_count: Cell::new(0),
            cancel_prompt: tokio_util::sync::CancellationToken::new(),
            stage_peer: Default::default(),
            notif_tx,
            permission_tx,
        }
    }

    /// Helper: send a `SessionNotification` and wait for the ack.
    async fn send_notification(&self, notif: acp::SessionNotification) -> Result<(), acp::Error> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.notif_tx
            .send((notif, ack_tx))
            .map_err(|_| acp::Error::internal_error())?;
        // `ack_rx` returns `Err(RecvError)` if the background notification
        // task has exited (e.g. `session_notification` errored and the loop
        // broke), which we map to `internal_error`.  No deadlock risk:
        // `RecvError` fires as soon as the corresponding `ack_tx` is dropped.
        ack_rx.await.map_err(|_| acp::Error::internal_error())
    }

    async fn report(&self, session: acp::SessionId, outcome: &str) -> acp::Result<()> {
        let mut arguments = json!({"call_id":"report-1","outcome":outcome,"summary":"controlled stage report","artifacts_produced":[]});
        if let Ok(report) = env::var("SURGE_TEST_VERIFICATION_REPORT") {
            arguments["verification_report"] = serde_json::from_str(&report)
                .map_err(|error| acp::Error::new(-32000, error.to_string()))?;
        }
        let peer = self.stage_peer.borrow().clone();
        if let Some(peer) = peer {
            let reply = peer.call("report_stage_outcome", arguments).await?;
            if reply.get("isError") == Some(&serde_json::Value::Bool(true)) {
                return Err(acp::Error::new(-32000, "stage candidate rejected"));
            }
            return Ok(());
        }
        self.send_notification(acp::SessionNotification::new(
            session,
            acp::SessionUpdate::ToolCall(
                acp::ToolCall::new("call-report", "report_stage_outcome")
                    .status(acp::ToolCallStatus::Completed)
                    .raw_input(arguments),
            ),
        ))
        .await
    }

    fn log(&self, msg: &str) {
        if self.verbose {
            eprintln!("[mock_acp_agent] {msg}");
        }
    }
}

fn argument_value(flag: &str) -> Option<String> {
    let args: Vec<_> = env::args().collect();
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| args.get(index + 1).cloned())
}
fn record_wire(kind: &str, request: &impl serde::Serialize) -> Result<(), acp::Error> {
    use std::io::Write;
    if let Some(path) = argument_value("--wire-log") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|_| acp::Error::internal_error())?;
        writeln!(
            file,
            "{}",
            serde_json::json!({"operation":kind,"request":request})
        )
        .map_err(|_| acp::Error::internal_error())?;
    }
    Ok(())
}
fn create_provider_identity(cwd: &std::path::Path) -> Result<String, acp::Error> {
    let Some(path) = argument_value("--session-store") else {
        return Ok("mock-session-1".into());
    };
    let mut sessions: std::collections::BTreeMap<String, std::path::PathBuf> =
        if std::path::Path::new(&path).exists() {
            serde_json::from_slice(&std::fs::read(&path).map_err(|_| acp::Error::internal_error())?)
                .map_err(|_| acp::Error::internal_error())?
        } else {
            Default::default()
        };
    let id = format!("mock-provider-{}", surge_core::RunId::new());
    sessions.insert(
        id.clone(),
        cwd.canonicalize()
            .map_err(|_| acp::Error::internal_error())?,
    );
    std::fs::write(
        path,
        serde_json::to_vec(&sessions).map_err(|_| acp::Error::internal_error())?,
    )
    .map_err(|_| acp::Error::internal_error())?;
    Ok(id)
}

impl MockAgent {
    async fn initialize(
        &self,
        req: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        self.log(&format!("initialize: {req:?}"));
        if env::args().any(|arg| arg == "--wire-all") {
            record_wire("initialize", &req)?;
        }
        let args: Vec<_> = env::args().collect();
        if let Some(index) = args.iter().position(|arg| arg == "--capabilities-file") {
            let path = args.get(index + 1).ok_or_else(acp::Error::internal_error)?;
            let bytes = serde_json::to_vec(&req.client_capabilities)
                .map_err(|_| acp::Error::internal_error())?;
            std::fs::write(path, bytes).map_err(|_| acp::Error::internal_error())?;
        }
        if env::args().any(|arg| arg == "--stall-initialize") {
            std::future::pending::<()>().await;
        }
        if let Some(marker) = argument_value("--stall-initialize-once")
            && std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)
                .is_ok()
        {
            std::future::pending::<()>().await;
        }
        let mut response =
            acp::InitializeResponse::new(agent_client_protocol::schema::ProtocolVersion::V1)
                .agent_info(acp::Implementation::new("mock-acp-agent", "0.0.1"));
        let mode = argument_value("--session-capability-policy-file")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_else(|| argument_value("--session-capabilities").unwrap_or_default());
        response.agent_capabilities.load_session = matches!(mode.as_str(), "load" | "both");
        if matches!(mode.as_str(), "resume" | "both") {
            response.agent_capabilities.session_capabilities.resume =
                Some(acp::SessionResumeCapabilities::default());
        }
        Ok(response)
    }

    async fn authenticate(
        &self,
        _req: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        self.log("authenticate");
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        req: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        record_wire("new_session", &req)?;
        self.log(&format!("new_session: {req:?}"));
        if env::args().any(|arg| arg == "--stage-mcp") || req.mcp_servers.iter().any(|server| matches!(server, acp::McpServer::Stdio(server) if server.name == "surge-stage")) {
            let peer = stage_mcp::Peer::connect(&req).await?;
            if !peer
                .catalog
                .get("tools")
                .is_some_and(serde_json::Value::is_array)
            {
                return Err(acp::Error::new(-32000, "invalid MCP tools/list response"));
            }
            *self.stage_peer.borrow_mut() = Some(peer);
        }
        if env::args().any(|arg| arg == "--stall-new-session") {
            std::future::pending::<()>().await;
        }
        // `--stall-new-session-once <marker>`: hang only on the first launch
        // (creates the marker), answer normally once it exists — a transient
        // adapter hang that tests must treat as uncertain establishment.
        let args: Vec<_> = env::args().collect();
        if let Some(index) = args
            .iter()
            .position(|arg| arg == "--stall-new-session-once")
        {
            let marker = args.get(index + 1).ok_or_else(acp::Error::internal_error)?;
            if std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)
                .is_ok()
            {
                std::future::pending::<()>().await;
            }
        }
        let response =
            acp::NewSessionResponse::new(acp::SessionId::new(create_provider_identity(&req.cwd)?));
        // `--config-options`: advertise a model and a reasoning-level select
        // (ACP `configOptions`) so clients can exercise set_config_option.
        if env::args().any(|arg| arg == "--config-options") {
            return Ok(response.config_options(mock_config_options()));
        }
        Ok(response)
    }

    async fn restore_provider(
        &self,
        id: &acp::SessionId,
        cwd: &std::path::Path,
        servers: Vec<acp::McpServer>,
    ) -> Result<(), acp::Error> {
        let path = argument_value("--session-store")
            .ok_or_else(|| acp::Error::new(-32001, "unknown provider session"))?;
        let sessions: std::collections::BTreeMap<String, std::path::PathBuf> =
            serde_json::from_slice(&std::fs::read(path).map_err(|_| acp::Error::internal_error())?)
                .map_err(|_| acp::Error::internal_error())?;
        if sessions.get(id.0.as_ref())
            != Some(
                &cwd.canonicalize()
                    .map_err(|_| acp::Error::internal_error())?,
            )
        {
            return Err(acp::Error::new(
                -32001,
                "unknown provider session or changed cwd",
            ));
        }
        if !servers.is_empty() {
            *self.stage_peer.borrow_mut() = Some(
                stage_mcp::Peer::connect(&acp::NewSessionRequest::new(cwd).mcp_servers(servers))
                    .await?,
            );
        }
        Ok(())
    }
    async fn load_session(
        &self,
        req: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        record_wire("load_session", &req)?;
        self.restore_provider(&req.session_id, &req.cwd, req.mcp_servers.clone())
            .await?;
        if env::args().any(|arg| arg == "--load-history-permission") {
            let request = acp::RequestPermissionRequest::new(
                req.session_id.clone(),
                acp::ToolCallUpdate::new(
                    "historical-permission",
                    acp::ToolCallUpdateFields::new().title("write_file"),
                ),
                vec![acp::PermissionOption::new(
                    "allow",
                    "Allow",
                    acp::PermissionOptionKind::AllowOnce,
                )],
            );
            let (reply, received) = oneshot::channel();
            self.permission_tx
                .send((request, reply))
                .map_err(|_| acp::Error::internal_error())?;
            let response = received.await.map_err(|_| acp::Error::internal_error())??;
            record_wire("historical_permission_response", &response)?;
        }
        Ok(acp::LoadSessionResponse::new())
    }
    async fn resume_session(
        &self,
        req: acp::ResumeSessionRequest,
    ) -> Result<acp::ResumeSessionResponse, acp::Error> {
        record_wire("resume_session", &req)?;
        self.restore_provider(&req.session_id, &req.cwd, req.mcp_servers.clone())
            .await?;
        Ok(acp::ResumeSessionResponse::new())
    }
    async fn prompt(&self, req: acp::PromptRequest) -> Result<acp::PromptResponse, acp::Error> {
        record_wire("prompt", &req)?;
        record_marker("--prompt-file")?;
        let count = self.prompt_count.get() + 1;
        self.prompt_count.set(count);
        self.log(&format!("prompt #{count}: session={:?}", req.session_id));

        if env::args().any(|arg| arg == "--stage-mcp")
            && !matches!(&self.scenario, Scenario::PromptError(_))
        {
            let peer = self
                .stage_peer
                .borrow()
                .clone()
                .ok_or_else(acp::Error::internal_error)?;
            let case = env::args()
                .find_map(|arg| arg.strip_prefix("--stage-mcp-case=").map(str::to_owned))
                .unwrap_or_else(|| "valid".into());
            if matches!(case.as_str(), "retry" | "retry-exhaust") && count > 1 {
                let prompt =
                    serde_json::to_string(&req.prompt).map_err(|_| acp::Error::internal_error())?;
                if !prompt.contains("rejected by validation") {
                    return Err(acp::Error::new(-32000, "missing rejection feedback"));
                }
                if case == "retry" {
                    tokio::fs::write("repair-ready", "corrected")
                        .await
                        .map_err(|_| acp::Error::internal_error())?;
                }
            }
            // `silent-first` / `silent-always`: end turns without calling
            // report_stage_outcome, as real agents do after finishing work.
            let silent = case == "silent-always" || (case == "silent-first" && count == 1);
            if silent {
                return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
            }
            if case == "silent-first" {
                let prompt =
                    serde_json::to_string(&req.prompt).map_err(|_| acp::Error::internal_error())?;
                if !prompt.contains("without an accepted report_stage_outcome call") {
                    return Err(acp::Error::new(-32000, "missing outcome reminder"));
                }
            }
            peer.exercise(&case, count).await?;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }
        let sid = req.session_id.clone();
        if env::args().any(|arg| arg == "--permission-flood") {
            return self.permission_flood(sid).await;
        }
        if env::args().any(|arg| arg == "--permission") {
            let request = acp::RequestPermissionRequest::new(
                sid.clone(),
                acp::ToolCallUpdate::new(
                    acp::ToolCallId::new("permission-call"),
                    acp::ToolCallUpdateFields::new().title(permission_title()),
                ),
                vec![acp::PermissionOption::new(
                    "allow",
                    "Allow",
                    acp::PermissionOptionKind::AllowOnce,
                )],
            );
            let (tx, rx) = oneshot::channel();
            self.permission_tx
                .send((request, tx))
                .map_err(|_| acp::Error::internal_error())?;
            let response = rx.await.map_err(|_| acp::Error::internal_error())??;
            record_wire("current_permission_response", &response)?;
            if !matches!(response.outcome, acp::RequestPermissionOutcome::Selected(_)) {
                return Err(acp::Error::internal_error());
            }
        }
        if env::args().any(|arg| arg == "--stall-prompt") {
            self.cancel_prompt.cancelled().await;
            return Ok(acp::PromptResponse::new(acp::StopReason::Cancelled));
        }
        if env::args().any(|arg| arg == "--exit-during-prompt") {
            std::process::exit(17);
        }

        if env::args().any(|arg| arg == "--stream-forever") {
            loop {
                self.send_notification(acp::SessionNotification::new(
                    req.session_id.clone(),
                    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                        acp::ContentBlock::from("still working"),
                    )),
                ))
                .await?;
            }
        }
        match &self.scenario {
            // ── prompt_error=KEY ─────────────────────────────────────────────
            Scenario::PromptError(key) => {
                if let Some(marker) = argument_value("--prompt-error-once")
                    && std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(marker)
                        .is_err()
                {
                    self.report(sid, "done").await?;
                } else {
                    self.log(&format!(
                        "prompt_error: returning scripted error for key {key:?}"
                    ));
                    return Err(provider_error_for_key(key));
                }
            },

            // ── echo ────────────────────────────────────────────────────────
            Scenario::Echo => {
                // Gather user text from the prompt blocks.
                let text: String = req
                    .prompt
                    .iter()
                    .filter_map(|block| {
                        if let acp::ContentBlock::Text(t) = block {
                            Some(t.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");

                let chunk =
                    acp::ContentChunk::new(acp::ContentBlock::from(format!("echo: {text}")));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::AgentMessageChunk(chunk),
                ))
                .await?;
            },

            // ── report_done ─────────────────────────────────────────────────
            Scenario::ReportDone => {
                self.report(sid, "done").await?;
            },
            Scenario::ReportOutcome(outcome) => {
                self.report(sid, outcome).await?;
            },

            // ── crash_after=N ────────────────────────────────────────────────
            Scenario::CrashAfter(n) => {
                // `count > n` semantics: `crash_after=0` crashes on prompt 1
                // (count=1, n=0); `crash_after=N` crashes on prompt N+1 after
                // serving the first N prompts normally.
                if count > *n {
                    eprintln!(
                        "[mock_acp_agent] crash_after={n} threshold reached at prompt #{count}; exiting 137"
                    );
                    std::process::exit(137);
                }
                // For prompts up to the threshold, emit a simple echo so the
                // bridge has something to receive.
                let chunk = acp::ContentChunk::new(acp::ContentBlock::from(format!(
                    "ok prompt {count}/{n}"
                )));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::AgentMessageChunk(chunk),
                ))
                .await?;
            },

            // ── human_input ──────────────────────────────────────────────────
            Scenario::HumanInput => {
                let tool_call = acp::ToolCall::new("call-human-input", "request_human_input")
                    .status(acp::ToolCallStatus::Pending)
                    .raw_input(json!({
                        "question": "mock: what should I do next?",
                        "context": "integration test"
                    }));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::ToolCall(tool_call),
                ))
                .await?;
            },

            // ── long_streaming ───────────────────────────────────────────────
            Scenario::LongStreaming => {
                for i in 0_u32..20 {
                    let chunk =
                        acp::ContentChunk::new(acp::ContentBlock::from(format!("chunk {i}")));
                    self.send_notification(acp::SessionNotification::new(
                        sid.clone(),
                        acp::SessionUpdate::AgentMessageChunk(chunk),
                    ))
                    .await?;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },

            // ── frozen ───────────────────────────────────────────────────────
            // Process the prompt normally (emit a small echo) so the bridge
            // sees a successful turn, then return.  The "freeze on stdin EOF"
            // behaviour lives in `run_agent` below — we keep the prompt path
            // responsive so the bridge can still establish the session and
            // exchange one message before close_session is invoked.
            Scenario::Frozen => {
                let chunk =
                    acp::ContentChunk::new(acp::ContentBlock::from("frozen ack".to_string()));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::AgentMessageChunk(chunk),
                ))
                .await?;
            },
        }

        // Emit a `UsageUpdate` notification so the bridge's token tracker can
        // observe a `TokenUsage` event when `extract_usage` is updated to read
        // it (Task 10.6 scope).  Also attach `Usage` to the `PromptResponse`
        // for symmetry — Bridge consumers that read response-attached usage
        // see the same numbers.
        if self.usage_on {
            self.send_notification(acp::SessionNotification::new(
                req.session_id.clone(),
                acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(100, 200_000)),
            ))
            .await?;
        }

        if env::args().any(|arg| arg == "--error-after-outcome") {
            // Leave a clear notification-before-error interval for lifecycle tests.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            return Err(acp::Error::internal_error()
                .data(json!({ "scripted": "error after reported outcome" })));
        }
        let mut resp = acp::PromptResponse::new(acp::StopReason::EndTurn);

        if self.usage_on {
            // `unstable_end_turn_token_usage` feature is enabled in the workspace.
            resp = resp.usage(acp::Usage::new(100, 80, 20));
        }

        Ok(resp)
    }

    async fn cancel(&self, _req: acp::CancelNotification) -> Result<(), acp::Error> {
        record_marker("--cancel-file")?;
        self.log("cancel");
        if !env::args().any(|arg| arg == "--ignore-cancel") {
            self.cancel_prompt.cancel();
        }
        Ok(())
    }
}

/// Options advertised under `--config-options`.
pub(crate) fn mock_config_options() -> Vec<acp::SessionConfigOption> {
    vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "sonnet",
            vec![
                acp::SessionConfigSelectOption::new("sonnet", "Mock Sonnet"),
                acp::SessionConfigSelectOption::new("opus", "Mock Opus"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Model),
        acp::SessionConfigOption::select(
            "effort",
            "Reasoning",
            "medium",
            vec![
                acp::SessionConfigSelectOption::new("low", "Low"),
                acp::SessionConfigSelectOption::new("high", "High"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::ThoughtLevel),
    ]
}

/// Append `id=value` for each `session/set_config_option` to the file named
/// by `--config-file <path>`.
pub(crate) fn record_config_choice(line: &str) -> acp::Result<()> {
    use std::io::Write as _;
    let args: Vec<_> = env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--config-file") {
        let path = args.get(index + 1).ok_or_else(acp::Error::internal_error)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|_| acp::Error::internal_error())?;
        writeln!(file, "{line}").map_err(|_| acp::Error::internal_error())?;
    }
    Ok(())
}

fn record_marker(flag: &str) -> acp::Result<()> {
    let args: Vec<_> = env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == flag) {
        let path = args.get(index + 1).ok_or_else(acp::Error::internal_error)?;
        std::fs::write(path, b"observed").map_err(|_| acp::Error::internal_error())?;
    }
    Ok(())
}

// ── run_agent ────────────────────────────────────────────────────────────────

async fn run_agent(
    scenario: Scenario,
    usage_on: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();

    let (notif_tx, notif_rx) = mpsc::unbounded_channel::<NotifItem>();

    let is_frozen = matches!(scenario, Scenario::Frozen);
    let (permission_tx, permission_rx) = mpsc::unbounded_channel::<PermissionItem>();
    let agent = MockAgent::new(scenario, usage_on, verbose, notif_tx, permission_tx);

    let stage_peer = agent.stage_peer.clone();
    let result = sdk_v1::run(agent, outgoing, incoming, notif_rx, permission_rx).await;
    let peer = stage_peer.borrow_mut().take();
    if let Some(peer) = peer {
        peer.close().await?;
    }
    result?;

    // ── frozen scenario: never exit ─────────────────────────────────────
    // After `handle_io` returns (because the bridge dropped the connection and
    // the mock's stdin got EOF), normally `main` returns and the process exits.
    // For `Scenario::Frozen` we instead block forever so the bridge's
    // `subprocess_waiter` never observes child exit, exercising the 5s
    // grace-timeout path in `close_session_impl`. The bridge will eventually
    // emit `SessionEnded::Timeout` and the test process will reap us when it
    // tears down (or the OS reaps the orphan when the bridge thread dies).
    if is_frozen {
        eprintln!("[mock_acp_agent] frozen: handle_io returned; sleeping forever");
        std::future::pending::<()>().await;
    }

    Ok(())
}

// ── main ─────────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--pid-file")
        && let Some(path) = args.get(index + 1)
    {
        std::fs::write(path, std::process::id().to_string())?;
    }
    if args.iter().any(|arg| arg == "--noisy-startup") {
        use std::io::Write;
        std::io::stderr().write_all(&vec![b'x'; 1024 * 1024])?;
    }

    // ── MOCK_ACP_HANDSHAKE_FAIL / --handshake-fail ──────────────────────
    // CLI flag takes precedence so tests don't need to mutate process-global env.
    let handshake_fail = env::var("MOCK_ACP_HANDSHAKE_FAIL").as_deref() == Ok("1")
        || args.iter().any(|a| a == "--handshake-fail");
    if handshake_fail {
        eprintln!("[mock_acp_agent] handshake-fail: exiting before handshake");
        std::process::exit(1);
    }

    let scenario = Scenario::parse(&args);
    // CLI flag --usage / --log-stderr accepted in addition to env vars for
    // test isolation (avoids mutating process-global env under parallel tests).
    let usage_on =
        env::var("MOCK_ACP_USAGE").as_deref() == Ok("on") || args.iter().any(|a| a == "--usage");
    let verbose = env::var("MOCK_ACP_LOG").as_deref() == Ok("stderr")
        || args.iter().any(|a| a == "--log-stderr");

    if verbose {
        eprintln!("[mock_acp_agent] starting scenario={scenario:?} usage_on={usage_on}");
    }

    let local = tokio::task::LocalSet::new();
    local
        .run_until(run_agent(scenario, usage_on, verbose))
        .await?;

    Ok(())
}

fn permission_title() -> String {
    if env::args().any(|arg| arg == "--permission-title-secret") {
        env::var("SURGE_STAGE_MCP_AUTH").unwrap_or_else(|_| "missing fixture credential".into())
    } else {
        "write_file".into()
    }
}

impl MockAgent {
    async fn permission_flood(&self, session: acp::SessionId) -> acp::Result<acp::PromptResponse> {
        use futures::{StreamExt, stream::FuturesUnordered};
        let mut replies = FuturesUnordered::new();
        for index in 0..40 {
            let request = acp::RequestPermissionRequest::new(
                session.clone(),
                acp::ToolCallUpdate::new(
                    acp::ToolCallId::new(format!("permission-{index}")),
                    acp::ToolCallUpdateFields::new().title("write_file"),
                ),
                vec![acp::PermissionOption::new(
                    "allow",
                    "Allow",
                    acp::PermissionOptionKind::AllowOnce,
                )],
            );
            let (reply, received) = oneshot::channel();
            self.permission_tx
                .send((request, reply))
                .map_err(|_| acp::Error::internal_error())?;
            replies.push(received);
        }
        while let Some(reply) = replies.next().await {
            if reply.map_err(|_| acp::Error::internal_error())?.is_err() {
                record_marker("--overload-file")?;
                self.cancel_prompt.cancelled().await;
                return Ok(acp::PromptResponse::new(acp::StopReason::Cancelled));
            }
        }
        Err(acp::Error::new(
            -32000,
            "fixture expected callback overload rejection",
        ))
    }
}
