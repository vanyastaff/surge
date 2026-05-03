//! Bridge events broadcast to subscribers. See spec §4.5.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surge_core::{OutcomeKey, SessionId};

use super::sandbox::SandboxDecision;

/// Everything observable about a session is one of these. Final event for
/// any `SessionId` is `SessionEnded`; subscribers can free per-session state
/// after observing it.
///
/// Wire format: `serde(tag = "type")` — JSON objects are tagged with a
/// discriminator so subscribers can decode without external context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BridgeEvent {
    /// Emitted once after ACP handshake succeeds and tools are declared.
    SessionEstablished {
        /// Identifier the bridge generated for this session.
        session: SessionId,
        /// Agent kind label (e.g. `"claude-code"`, `"codex"`, `"mock"`).
        agent: String,
        /// Engine-supplied opaque labels passed through from `SessionConfig::bindings`.
        bindings: BTreeMap<String, String>,
        /// Tool names the agent will see after sandbox-visibility filtering.
        /// Useful for observability — confirms denied tools are absent.
        tools_visible: Vec<String>,
    },

    /// Streaming agent output. Multiple events per session.
    AgentMessage {
        session: SessionId,
        /// One chunk of the agent's text output. Concatenate across consecutive
        /// `AgentMessage` events to reconstruct the full message.
        chunk: String,
        /// Optional metadata (model name, timestamp). `None` if the underlying
        /// agent didn't supply usage metadata.
        meta: Option<AgentMessageMeta>,
    },

    /// Cumulative token usage. Bridge guarantees all `TokenUsage` for a given
    /// session precede `SessionEnded` for that session (spec §5.7).
    TokenUsage {
        session: SessionId,
        /// Cumulative input tokens consumed since session start.
        prompt_tokens: u32,
        /// Cumulative output tokens produced since session start.
        output_tokens: u32,
        /// Cumulative cache hits.
        cache_hits: u32,
        /// Model name as reported by the agent (e.g. `"claude-opus-4-7"`).
        model: String,
    },

    /// Generic tool call (not the engine-injected ones). Bridge auto-replies
    /// `Unsupported` in M3; M5 will install a real dispatcher (spec §5.3).
    ToolCall {
        session: SessionId,
        /// ACP-supplied call id; correlates with the matching `ToolResult`.
        call_id: String,
        /// Tool name as the agent invoked it.
        tool: String,
        /// JSON-encoded arguments after secrets redaction. Safe to log.
        args_redacted_json: String,
        /// Sandbox's decision (Allow/Deny/Elevate) for this specific invocation.
        sandbox_decision: SandboxDecision,
        /// Source category and MCP id metadata.
        meta: ToolCallMeta,
    },

    /// Result going back to the agent.
    ToolResult {
        session: SessionId,
        /// ACP-supplied call id matching the corresponding `ToolCall`.
        call_id: String,
        /// Result payload (success / error / unsupported).
        payload: ToolResultPayload,
    },

    /// Engine-injected `report_stage_outcome` was called. Routed as a first-class
    /// event so M5 can fold directly into `EventPayload::OutcomeReported`.
    OutcomeReported {
        session: SessionId,
        /// One of the outcomes declared in `SessionConfig::declared_outcomes`.
        outcome: OutcomeKey,
        /// Agent's 1–3 sentence explanation of what it did and why.
        summary: String,
        /// File paths the agent created or modified, as reported.
        artifacts_produced: Vec<String>,
    },

    /// Engine-injected `request_human_input` was called.
    HumanInputRequested {
        session: SessionId,
        /// ACP-supplied call id; M5 will use this to route the human's reply.
        call_id: String,
        /// Question the agent wants the human to answer.
        question: String,
        /// Optional context the agent supplied to help the human respond.
        context: Option<String>,
    },

    /// Final event for the session. After this, `SessionId` is gone from the
    /// bridge's internal map.
    SessionEnded {
        session: SessionId,
        /// Why the session terminated. See `SessionEndReason`.
        reason: SessionEndReason,
    },

    /// Bridge-level error.
    ///
    /// **Emit conditions** (the exhaustive list — `Error` is not a generic
    /// dumping ground):
    /// 1. ACP protocol violation that did not kill the session (recoverable
    ///    parse failure on a non-critical frame).
    /// 2. Tool dispatch failed but session continues (M3: only fires on JSON
    ///    parse failure of injected-tool args before `OutcomeReported` /
    ///    `HumanInputRequested` can be emitted).
    /// 3. Token extraction failed (malformed `unstable_session_usage` metadata).
    ///
    /// Errors that end the session emit `SessionEnded` instead, not `Error`.
    /// If both apply, `Error` is emitted first, then `SessionEnded`.
    Error {
        /// Session that caused the error, or `None` for bridge-level errors
        /// not tied to a specific session.
        session: Option<SessionId>,
        /// Human-readable error message.
        error: String,
    },
}

/// Metadata accompanying an `AgentMessage` chunk when the agent reports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessageMeta {
    /// Model name as the agent reports it. May be `None` for agents that
    /// don't supply this in chunked messages.
    pub model: Option<String>,
    /// Unix timestamp (milliseconds) when the chunk was emitted by the bridge.
    pub timestamp_ms: i64,
}

/// Why a session ended. Carried inside `BridgeEvent::SessionEnded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEndReason {
    /// Closed via `close_session()` (graceful) or agent volunteered ACP shutdown.
    Normal,
    /// Subprocess exited with non-zero or by signal mid-session.
    AgentCrashed {
        /// Process exit code if available (`None` for signal-only exits).
        exit_code: Option<i32>,
        /// Last 2 KiB of the agent's stderr captured at exit time.
        stderr_tail: String,
    },
    /// `close_session()` exceeded the graceful-close grace period; the
    /// child was killed and the session is gone.
    Timeout {
        /// Grace period that elapsed before the bridge gave up, in milliseconds.
        duration_ms: u64,
    },
    /// `AcpBridge::shutdown()` triggered while the session was still open.
    ForcedClose,
}

/// Source category metadata attached to `BridgeEvent::ToolCall`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMeta {
    /// MCP server id if applicable, else `None`.
    pub mcp_id: Option<String>,
    /// `true` iff this tool came from `tools::build_injected_tools`
    /// (i.e. `report_stage_outcome` or `request_human_input`).
    pub injected: bool,
}

/// Payload of `BridgeEvent::ToolResult`.
///
/// Wire format: `serde(tag = "outcome")` — JSON objects are tagged so
/// subscribers can decode unambiguously.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ToolResultPayload {
    /// Tool returned success. `result_json` is the raw JSON payload returned
    /// to the agent.
    Ok {
        /// JSON payload returned to the agent (raw text, not re-serialized).
        result_json: String,
    },
    /// Tool returned an error. `message` is the human-readable explanation.
    Error {
        /// Error message returned to the agent.
        message: String,
    },
    /// M3 stub for non-injected tools. M5 replaces with real dispatch.
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn session_end_reason_serde_round_trip() {
        let r = SessionEndReason::AgentCrashed {
            exit_code: Some(137),
            stderr_tail: "panic at 42".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: SessionEndReason = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn outcome_reported_serde_round_trip() {
        let session = SessionId::new();
        let ev = BridgeEvent::OutcomeReported {
            session: session.clone(),
            outcome: OutcomeKey::from_str("done").unwrap(),
            summary: "did it".into(),
            artifacts_produced: vec!["a.txt".into(), "b.txt".into()],
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: BridgeEvent = serde_json::from_str(&s).unwrap();
        match back {
            BridgeEvent::OutcomeReported { session: s2, outcome, summary, artifacts_produced } => {
                assert_eq!(s2, session);
                assert_eq!(outcome.as_str(), "done");
                assert_eq!(summary, "did it");
                assert_eq!(artifacts_produced.len(), 2);
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[test]
    fn tool_result_payload_unsupported_round_trip() {
        let p = ToolResultPayload::Unsupported;
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("unsupported"));
        let back: ToolResultPayload = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, ToolResultPayload::Unsupported));
    }
}
