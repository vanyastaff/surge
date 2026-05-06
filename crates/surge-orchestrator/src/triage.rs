//! Triage Author dispatcher: assembles inputs, parses LLM output.
//!
//! This module owns the input/output schema for the bootstrap-stage 0
//! Triage Author. The actual agent invocation (LLM call via ACP) will
//! be wired in a follow-up polish task; for now this module provides
//! the typed interface.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::event::{BridgeEvent, SessionEndReason};
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::{OutcomeKey, SessionId};
use surge_intake::types::{Priority, TaskDetails, TaskId, TaskSummary, TriageDecision};

/// Full input bundle handed to Triage Author at the start of its session.
///
/// Serialised as the top-level JSON the agent receives. The agent reads
/// `task` (the new ticket), `candidates` (similar open tickets +
/// recent specs), and `active_runs` (current Surge runs).
#[derive(Debug, Clone, Serialize)]
pub struct TriageInput {
    pub task: TaskDetails,
    pub candidates: Vec<TaskSummary>,
    pub active_runs: Vec<ActiveRunSummary>,
}

/// Snapshot of an active Surge run for triage dedup context.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveRunSummary {
    pub run_id: String,
    pub task_id: Option<String>,
    pub status: String,
    pub started_at: String,
}

/// Raw JSON shape Triage Author writes to `triage_decision.json`.
///
/// The agent fills as many fields as apply for its chosen decision; this
/// type is intentionally permissive (all variant-specific fields are
/// `Option`/`#[serde(default)]`) so we can normalise into the strict
/// [`TriageDecision`] enum via [`Self::into_decision`].
#[derive(Debug, Clone, Deserialize)]
pub struct TriageJson {
    pub decision: String,
    #[serde(default)]
    pub duplicate_of: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub priority_reasoning: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
}

impl TriageJson {
    /// Convert the loose JSON into a strict [`TriageDecision`].
    ///
    /// Returns `Err(message)` if required fields for the chosen decision
    /// are missing (e.g. `duplicate` without `duplicate_of`) or the
    /// decision/priority strings are unrecognised.
    pub fn into_decision(self) -> Result<TriageDecision, String> {
        let prio_str = self.priority.as_deref().unwrap_or("medium");
        let priority = match prio_str {
            "urgent" => Priority::Urgent,
            "high" => Priority::High,
            "medium" => Priority::Medium,
            "low" => Priority::Low,
            other => return Err(format!("unknown priority: {other}")),
        };
        match self.decision.as_str() {
            "enqueued" => Ok(TriageDecision::Enqueued {
                priority,
                reasoning: self.priority_reasoning.unwrap_or_default(),
                summary: self.summary.unwrap_or_default(),
            }),
            "duplicate" => {
                let dup = self
                    .duplicate_of
                    .ok_or_else(|| "duplicate decision requires duplicate_of".to_string())?;
                let id = TaskId::try_new(dup).map_err(|e| format!("invalid duplicate_of: {e}"))?;
                Ok(TriageDecision::Duplicate {
                    of: id,
                    reasoning: self.priority_reasoning.unwrap_or_default(),
                })
            },
            "out_of_scope" => Ok(TriageDecision::OutOfScope {
                reasoning: self.priority_reasoning.unwrap_or_default(),
            }),
            "unclear" => Ok(TriageDecision::Unclear {
                question: self
                    .question
                    .unwrap_or_else(|| "no question provided".into()),
            }),
            other => Err(format!("unknown decision: {other}")),
        }
    }
}

/// Tunable parameters for [`dispatch_triage`].
///
/// Construct via [`Self::with_scratch_root`] for the typical case of
/// passing only the per-task scratch root and Claude binary path.
#[derive(Debug, Clone)]
pub struct TriageOptions {
    /// Resolved Claude binary path. If `None`, the dispatcher
    /// returns `Ok(TriageDecision::Unclear)` immediately on the
    /// first attempt with a configuration-hint message.
    pub claude_binary: Option<PathBuf>,
    /// Per-attempt timeout. Default: 5 min (matches RFC-0010 §"Bootstrap stage failures").
    pub attempt_timeout: Duration,
    /// Maximum attempts before falling back to `Unclear`. Default: 3.
    pub max_attempts: u32,
    /// Root directory for per-call scratch dirs.
    pub scratch_root: PathBuf,
    /// Whether to keep scratch on Unclear / TriageError for post-mortem.
    pub keep_scratch_on_failure: bool,
}

impl TriageOptions {
    /// Build options with sensible defaults given a scratch root and
    /// (optional) Claude binary path.
    #[must_use]
    pub fn with_scratch_root(scratch_root: PathBuf, claude_binary: Option<PathBuf>) -> Self {
        Self {
            claude_binary,
            attempt_timeout: Duration::from_secs(300),
            max_attempts: 3,
            scratch_root,
            keep_scratch_on_failure: true,
        }
    }
}

/// Errors returned from [`dispatch_triage`] for invariant violations.
///
/// Note: retry-eligible failures (timeout, agent crash, malformed JSON)
/// are NOT surfaced as `TriageError` — they retry up to
/// `opts.max_attempts` times and on exhaustion become
/// `Ok(TriageDecision::Unclear { question })`. `TriageError` is
/// reserved for failures that prevent any forward progress.
#[derive(Debug, thiserror::Error)]
pub enum TriageError {
    /// Could not create or write to the per-call scratch directory.
    #[error("scratch dir setup failed: {0}")]
    Scratch(#[from] std::io::Error),
    /// Bridge facade itself failed at the JSON-RPC / process level
    /// (open_session / send_message / close_session).
    #[error("acp bridge: {0}")]
    Bridge(String),
}

/// Bridge-level sandbox for sessions where Surge delegates isolation
/// to the agent itself (per Vision-2026 §"Sandbox-delegated").
///
/// Returns `AlwaysAllowSandbox` — the bridge applies no tool filtering,
/// because each ACP-conformant agent (Claude Code, Codex CLI, etc.)
/// already has its own native sandbox enforcement. The profile's
/// `[sandbox] mode = ...` field is a semantic marker the agent reads,
/// not a directive the bridge enforces.
fn delegated_sandbox() -> Box<dyn surge_acp::bridge::sandbox::Sandbox> {
    Box::new(surge_acp::bridge::sandbox::AlwaysAllowSandbox)
}

fn event_session_id(event: &BridgeEvent) -> Option<SessionId> {
    match event {
        BridgeEvent::SessionEstablished { session, .. }
        | BridgeEvent::AgentMessage { session, .. }
        | BridgeEvent::TokenUsage { session, .. }
        | BridgeEvent::ToolCall { session, .. }
        | BridgeEvent::ToolResult { session, .. }
        | BridgeEvent::OutcomeReported { session, .. }
        | BridgeEvent::HumanInputRequested { session, .. }
        | BridgeEvent::SessionEnded { session, .. } => Some(*session),
        BridgeEvent::Error { session, .. } => *session,
    }
}

/// Per-attempt error type — distinguishes retryable from fatal.
enum AttemptError {
    /// Bridge facade itself failed (open/send/close); fatal.
    Bridge(String),
    /// Retryable: timeout, agent crash, malformed artifact.
    Retryable(String),
}

fn render_prompt(input: &TriageInput, feedback: Option<&str>) -> String {
    let json = serde_json::to_string_pretty(input).unwrap_or_else(|_| "{}".into());
    let mut out = String::new();
    out.push_str(crate::BOOTSTRAP_TRIAGE_AUTHOR_TOML);
    out.push_str("\n\n# Inputs\n\nThe triage input is encoded as JSON. The shape is:\n");
    out.push_str(
        "- task: TaskDetails\n- candidates: TaskSummary[]\n- active_runs: ActiveRunSummary[]\n\n",
    );
    out.push_str("The literal JSON follows:\n\n");
    out.push_str(&json);
    out.push_str(
        "\n\n# Task\n\nDecide whether this ticket is a duplicate, out-of-scope, unclear, \
                  or should be enqueued. Then, in your working directory:\n\n\
                  1. Write your structured decision to `triage_decision.json` (schema below).\n\
                  2. Write a 3-5 line markdown blurb to `inbox_summary.md` (used as the body of \
                     the inbox card on `enqueued`; safe to omit otherwise).\n\
                  3. Call `report_stage_outcome` with the matching `outcome` and \
                     `artifacts_produced = [\"triage_decision.json\", \"inbox_summary.md\"]`.\n\n",
    );
    out.push_str(
        "triage_decision.json schema:\n\
                  - decision: \"enqueued\" | \"duplicate\" | \"out_of_scope\" | \"unclear\"\n\
                  - duplicate_of: string (task id) or null\n\
                  - priority: \"urgent\" | \"high\" | \"medium\" | \"low\"\n\
                  - priority_reasoning: one sentence\n\
                  - summary: one sentence\n\
                  - question: string (only when decision = \"unclear\")\n",
    );
    if let Some(fb) = feedback {
        out.push_str("\n# Feedback from previous attempt\n\n");
        out.push_str(fb);
        out.push('\n');
    }
    out
}

async fn try_one_attempt(
    bridge: Arc<dyn BridgeFacade>,
    input: &TriageInput,
    claude_binary: &std::path::Path,
    scratch_dir: &std::path::Path,
    attempt_timeout: Duration,
    attempt: u32,
    feedback: Option<&str>,
) -> Result<TriageDecision, AttemptError> {
    use std::collections::BTreeMap;
    use surge_acp::bridge::session::{AgentKind, MessageContent, SessionConfig};
    use surge_acp::client::PermissionPolicy;

    let prompt_text = render_prompt(input, feedback);

    let declared_outcomes = vec![
        OutcomeKey::try_from("enqueued").map_err(|e| AttemptError::Bridge(e.to_string()))?,
        OutcomeKey::try_from("duplicate").map_err(|e| AttemptError::Bridge(e.to_string()))?,
        OutcomeKey::try_from("out_of_scope").map_err(|e| AttemptError::Bridge(e.to_string()))?,
        OutcomeKey::try_from("unclear").map_err(|e| AttemptError::Bridge(e.to_string()))?,
    ];

    let mut bindings = BTreeMap::new();
    bindings.insert(
        "intake.task_id".into(),
        input.task.task_id.as_str().to_string(),
    );
    bindings.insert("intake.attempt".into(), attempt.to_string());

    let cfg = SessionConfig {
        agent_kind: AgentKind::ClaudeCode {
            binary: claude_binary.to_path_buf(),
            extra_args: vec![],
        },
        working_dir: scratch_dir.to_path_buf(),
        system_prompt: prompt_text.clone(),
        declared_outcomes,
        allows_escalation: false,
        tools: vec![],
        sandbox: delegated_sandbox(),
        permission_policy: PermissionPolicy::default(),
        bindings,
    };

    let mut events = bridge.subscribe();

    let session_id = bridge
        .open_session(cfg)
        .await
        .map_err(|e| AttemptError::Bridge(format!("open_session: {e}")))?;

    bridge
        .send_message(session_id, MessageContent::Text(prompt_text))
        .await
        .map_err(|e| AttemptError::Bridge(format!("send_message: {e}")))?;

    // Drive the event loop with a single timeout.
    let outcome_result = tokio::time::timeout(attempt_timeout, async {
        loop {
            match events.recv().await {
                Ok(ev) => {
                    if event_session_id(&ev) != Some(session_id) {
                        continue;
                    }
                    match ev {
                        BridgeEvent::OutcomeReported { outcome, .. } => return Ok(outcome),
                        BridgeEvent::SessionEnded { reason, .. } => match reason {
                            SessionEndReason::AgentCrashed { .. } => {
                                return Err(format!("agent crashed: {reason:?}"));
                            },
                            SessionEndReason::Timeout { .. } => {
                                return Err("session timeout".into());
                            },
                            _ => continue,
                        },
                        _ => continue,
                    }
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err("event stream closed".into());
                },
            }
        }
    })
    .await;

    let _ = bridge.close_session(session_id).await;

    let outcome = match outcome_result {
        Err(_elapsed) => return Err(AttemptError::Retryable("timeout".into())),
        Ok(Err(msg)) => return Err(AttemptError::Retryable(msg)),
        Ok(Ok(outcome)) => outcome,
    };

    // Read the artifact and parse.
    let json_path = scratch_dir.join("triage_decision.json");
    let raw = std::fs::read(&json_path)
        .map_err(|e| AttemptError::Retryable(format!("triage_decision.json missing: {e}")))?;
    let parsed: TriageJson = serde_json::from_slice(&raw)
        .map_err(|e| AttemptError::Retryable(format!("triage_decision.json malformed: {e}")))?;
    let mut decision = parsed
        .into_decision()
        .map_err(|e| AttemptError::Retryable(format!("decision rejected: {e}")))?;

    // For Enqueued, splice in the inbox_summary.md if present.
    if let TriageDecision::Enqueued { summary, .. } = &mut decision {
        if summary.is_empty() {
            if let Ok(md) = std::fs::read_to_string(scratch_dir.join("inbox_summary.md")) {
                *summary = md;
            }
        }
    }

    // Sanity-check the agent's reported outcome string against the parsed decision.
    let outcome_str = outcome.as_str();
    let decision_kind = match &decision {
        TriageDecision::Enqueued { .. } => "enqueued",
        TriageDecision::Duplicate { .. } => "duplicate",
        TriageDecision::OutOfScope { .. } => "out_of_scope",
        TriageDecision::Unclear { .. } => "unclear",
    };
    if outcome_str != decision_kind {
        return Err(AttemptError::Retryable(format!(
            "outcome ({outcome_str}) mismatch with decision ({decision_kind})"
        )));
    }

    Ok(decision)
}

/// Dispatch a Triage Author session against the supplied bridge.
///
/// Returns `Ok(TriageDecision)` even on retry exhaustion (materialises
/// as `Unclear` with a diagnostic question). `Err(TriageError)` is
/// reserved for invariant violations that prevent any forward progress.
///
/// # Errors
/// - [`TriageError::Scratch`] if the per-call scratch directory
///   cannot be created.
/// - [`TriageError::Bridge`] for facade-level dead-bridge or
///   handshake failures.
pub async fn dispatch_triage(
    bridge: Arc<dyn BridgeFacade>,
    input: TriageInput,
    opts: TriageOptions,
) -> Result<TriageDecision, TriageError> {
    // Short-circuit if claude binary is not configured.
    let Some(claude_binary) = opts.claude_binary.clone() else {
        return Ok(TriageDecision::Unclear {
            question: "Claude binary not configured (set SURGE_CLAUDE_BINARY or install \
                       claude-code); install to enable LLM-driven triage"
                .into(),
        });
    };

    // Build a fresh scratch dir for this top-level call.
    let scratch_dir = opts.scratch_root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(&scratch_dir)?;

    let mut last_err: Option<String> = None;

    for attempt in 1..=opts.max_attempts {
        match try_one_attempt(
            Arc::clone(&bridge),
            &input,
            &claude_binary,
            &scratch_dir,
            opts.attempt_timeout,
            attempt,
            last_err.as_deref(),
        )
        .await
        {
            Ok(decision) => {
                if !opts.keep_scratch_on_failure
                    && !matches!(decision, TriageDecision::Unclear { .. })
                {
                    let _ = std::fs::remove_dir_all(&scratch_dir);
                }
                return Ok(decision);
            },
            Err(AttemptError::Bridge(e)) => return Err(TriageError::Bridge(e)),
            Err(AttemptError::Retryable(msg)) => {
                tracing::warn!(attempt, error = %msg, "triage attempt failed; will retry");
                last_err = Some(msg);
            },
        }
    }

    let question = format!(
        "Triage failed after {} attempts: {}",
        opts.max_attempts,
        last_err.as_deref().unwrap_or("unknown error")
    );
    Ok(TriageDecision::Unclear { question })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enqueued() {
        let raw = r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let dec = parsed.into_decision().unwrap();
        match dec {
            TriageDecision::Enqueued { priority, .. } => assert_eq!(priority, Priority::High),
            other => panic!("expected Enqueued, got {other:?}"),
        }
    }

    #[test]
    fn parse_duplicate_requires_duplicate_of() {
        let raw = r#"{"decision":"duplicate"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let err = parsed.into_decision().unwrap_err();
        assert!(err.contains("duplicate_of"));
    }

    #[test]
    fn parse_unknown_priority_errors() {
        let raw = r#"{"decision":"enqueued","priority":"super-urgent"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let err = parsed.into_decision().unwrap_err();
        assert!(err.contains("unknown priority"));
    }

    #[test]
    fn parse_out_of_scope() {
        let raw = r#"{"decision":"out_of_scope","priority":"low","priority_reasoning":"not a coding task"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let dec = parsed.into_decision().unwrap();
        assert!(matches!(dec, TriageDecision::OutOfScope { .. }));
    }

    #[test]
    fn parse_unclear() {
        let raw = r#"{"decision":"unclear","priority":"medium","question":"What does this mean?"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let dec = parsed.into_decision().unwrap();
        match dec {
            TriageDecision::Unclear { question } => assert_eq!(question, "What does this mean?"),
            other => panic!("expected Unclear, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_decision_errors() {
        let raw = r#"{"decision":"meow","priority":"medium"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let err = parsed.into_decision().unwrap_err();
        assert!(err.contains("unknown decision"));
    }
}
