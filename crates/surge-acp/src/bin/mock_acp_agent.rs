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
//! **Env var flags**
//!
//! - `MOCK_ACP_USAGE=on`            — emit a `SessionUpdate::UsageUpdate`
//!   notification per prompt turn (and also attach `Usage` to the
//!   `PromptResponse`)
//! - `MOCK_ACP_HANDSHAKE_FAIL=1`    — exit(1) before creating the ACP connection
//! - `MOCK_ACP_LOG=stderr`          — write verbose diagnostics to stderr

use std::cell::Cell;
use std::env;
use std::time::Duration;

use agent_client_protocol::{self as acp, Client as _};
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

        let Some(value) = value else { return Self::Echo };

        if let Some(k) = value.strip_prefix("report_outcome=") {
            return Self::ReportOutcome(k.to_string());
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
            }
        }
    }
}

// ── Notification channel item ───────────────────────────────────────────────

/// One notification to send to the client, plus a one-shot ack channel so the
/// sender can wait until the send is flushed before continuing.
type NotifItem = (acp::SessionNotification, oneshot::Sender<()>);

// ── MockAgent ───────────────────────────────────────────────────────────────

struct MockAgent {
    scenario: Scenario,
    usage_on: bool,
    verbose: bool,
    /// Counts how many `prompt` calls have been received (for `crash_after=N`).
    prompt_count: Cell<u32>,
    /// Channel to push `SessionNotification`s to the background sender task.
    notif_tx: mpsc::UnboundedSender<NotifItem>,
}

impl MockAgent {
    fn new(
        scenario: Scenario,
        usage_on: bool,
        verbose: bool,
        notif_tx: mpsc::UnboundedSender<NotifItem>,
    ) -> Self {
        Self {
            scenario,
            usage_on,
            verbose,
            prompt_count: Cell::new(0),
            notif_tx,
        }
    }

    /// Helper: send a `SessionNotification` and wait for the ack.
    async fn send_notification(
        &self,
        notif: acp::SessionNotification,
    ) -> Result<(), acp::Error> {
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

    fn log(&self, msg: &str) {
        if self.verbose {
            eprintln!("[mock_acp_agent] {msg}");
        }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for MockAgent {
    async fn initialize(
        &self,
        req: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        self.log(&format!("initialize: {req:?}"));
        Ok(
            acp::InitializeResponse::new(acp::ProtocolVersion::V1)
                .agent_info(acp::Implementation::new("mock-acp-agent", "0.0.1")),
        )
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
        self.log(&format!("new_session: {req:?}"));
        Ok(acp::NewSessionResponse::new(acp::SessionId::new("mock-session-1")))
    }

    async fn prompt(
        &self,
        req: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        let count = self.prompt_count.get() + 1;
        self.prompt_count.set(count);
        self.log(&format!("prompt #{count}: session={:?}", req.session_id));

        let sid = req.session_id.clone();

        match &self.scenario {
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

                let chunk = acp::ContentChunk::new(acp::ContentBlock::from(
                    format!("echo: {text}"),
                ));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::AgentMessageChunk(chunk),
                ))
                .await?;
            }

            // ── report_done ─────────────────────────────────────────────────
            Scenario::ReportDone => {
                let tool_call = acp::ToolCall::new("call-report-done", "report_stage_outcome")
                    .status(acp::ToolCallStatus::Completed)
                    .raw_input(json!({
                        "outcome": "done",
                        "summary": "mock report_done"
                    }));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::ToolCall(tool_call),
                ))
                .await?;
            }

            // ── report_outcome=K ─────────────────────────────────────────────
            Scenario::ReportOutcome(outcome) => {
                let tool_call = acp::ToolCall::new("call-report-outcome", "report_stage_outcome")
                    .status(acp::ToolCallStatus::Completed)
                    .raw_input(json!({
                        "outcome": outcome,
                        "summary": format!("mock report_outcome={}", outcome)
                    }));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::ToolCall(tool_call),
                ))
                .await?;
            }

            // ── crash_after=N ────────────────────────────────────────────────
            Scenario::CrashAfter(n) => {
                // `count > n` semantics: `crash_after=0` crashes on prompt 1
                // (count=1, n=0); `crash_after=N` crashes on prompt N+1 after
                // serving the first N prompts normally.
                if count > *n {
                    eprintln!("[mock_acp_agent] crash_after={n} threshold reached at prompt #{count}; exiting 137");
                    std::process::exit(137);
                }
                // For prompts up to the threshold, emit a simple echo so the
                // bridge has something to receive.
                let chunk = acp::ContentChunk::new(acp::ContentBlock::from(
                    format!("ok prompt {count}/{n}"),
                ));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::AgentMessageChunk(chunk),
                ))
                .await?;
            }

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
            }

            // ── long_streaming ───────────────────────────────────────────────
            Scenario::LongStreaming => {
                for i in 0_u32..20 {
                    let chunk = acp::ContentChunk::new(acp::ContentBlock::from(
                        format!("chunk {i}"),
                    ));
                    self.send_notification(acp::SessionNotification::new(
                        sid.clone(),
                        acp::SessionUpdate::AgentMessageChunk(chunk),
                    ))
                    .await?;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }

            // ── frozen ───────────────────────────────────────────────────────
            // Process the prompt normally (emit a small echo) so the bridge
            // sees a successful turn, then return.  The "freeze on stdin EOF"
            // behaviour lives in `run_agent` below — we keep the prompt path
            // responsive so the bridge can still establish the session and
            // exchange one message before close_session is invoked.
            Scenario::Frozen => {
                let chunk = acp::ContentChunk::new(acp::ContentBlock::from(
                    "frozen ack".to_string(),
                ));
                self.send_notification(acp::SessionNotification::new(
                    sid,
                    acp::SessionUpdate::AgentMessageChunk(chunk),
                ))
                .await?;
            }
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

        let mut resp = acp::PromptResponse::new(acp::StopReason::EndTurn);

        if self.usage_on {
            // `unstable_session_usage` feature is enabled in the workspace.
            resp = resp.usage(acp::Usage::new(100, 80, 20));
        }

        Ok(resp)
    }

    async fn cancel(&self, _req: acp::CancelNotification) -> Result<(), acp::Error> {
        self.log("cancel");
        Ok(())
    }
}

// ── run_agent ────────────────────────────────────────────────────────────────

async fn run_agent(
    scenario: Scenario,
    usage_on: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();

    let (notif_tx, mut notif_rx) = mpsc::unbounded_channel::<NotifItem>();

    let is_frozen = matches!(scenario, Scenario::Frozen);
    let agent = MockAgent::new(scenario, usage_on, verbose, notif_tx);

    let (conn, handle_io) = acp::AgentSideConnection::new(agent, outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });

    // Background task: drain the notification channel and forward each
    // `SessionNotification` to the client via `conn.session_notification()`.
    tokio::task::spawn_local(async move {
        while let Some((notif, ack)) = notif_rx.recv().await {
            if let Err(e) = conn.session_notification(notif).await {
                eprintln!("[mock_acp_agent] session_notification error: {e}");
                break;
            }
            // Signal the agent's send_notification helper that the send is flushed.
            let _ = ack.send(());
        }
    });

    handle_io.await?;

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

    // ── MOCK_ACP_HANDSHAKE_FAIL: exit before building the connection ─────
    if env::var("MOCK_ACP_HANDSHAKE_FAIL").as_deref() == Ok("1") {
        eprintln!("[mock_acp_agent] MOCK_ACP_HANDSHAKE_FAIL=1: exiting before handshake");
        std::process::exit(1);
    }

    let scenario = Scenario::parse(&args);
    let usage_on = env::var("MOCK_ACP_USAGE").as_deref() == Ok("on");
    let verbose = env::var("MOCK_ACP_LOG").as_deref() == Ok("stderr");

    if verbose {
        eprintln!("[mock_acp_agent] starting scenario={scenario:?} usage_on={usage_on}");
    }

    let local = tokio::task::LocalSet::new();
    local
        .run_until(run_agent(scenario, usage_on, verbose))
        .await?;

    Ok(())
}
