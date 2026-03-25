//! Events emitted throughout the Surge system.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::id::{SpecId, SubtaskId, TaskId};
use crate::state::TaskState;

// ── Tool call mirror types ──────────────────────────────────────────

/// Category of tool being invoked (mirrors ACP ToolKind).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

/// Execution status of a tool call (mirrors ACP ToolCallStatus).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// File location affected by a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLocation {
    pub path: PathBuf,
    pub line: Option<u32>,
}

/// A file diff produced by a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDiff {
    pub path: PathBuf,
    pub old_text: Option<String>,
    pub new_text: String,
}

/// Priority of a plan entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanPriority {
    High,
    Medium,
    Low,
}

/// Status of a plan entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// A single entry in an agent's execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub priority: PlanPriority,
    pub status: PlanStatus,
}

/// QA verdict type (mirrors surge-orchestrator QaVerdictKind).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QaVerdictKind {
    Approved,
    Partial,
    NeedsFix,
}

// ── Events ──────────────────────────────────────────────────────────

/// Events emitted by Surge for monitoring, UI updates, and logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SurgeEvent {
    // --- Agent events ---
    /// Agent connected successfully.
    AgentConnected { agent_name: String },

    /// Agent disconnected.
    AgentDisconnected { agent_name: String },

    /// Agent requested a permission.
    PermissionRequested {
        session_id: String,
        description: String,
        tool_call_id: String,
        options: Vec<String>,
    },

    /// Permission was granted or denied.
    PermissionResolved { session_id: String, granted: bool },

    // --- Resilience events ---
    /// Circuit breaker opened due to repeated failures.
    CircuitBreakerOpened {
        agent_name: String,
        reason: String,
        failure_count: u32,
    },

    /// Circuit breaker transitioned to half-open state for testing.
    CircuitBreakerHalfOpen { agent_name: String },

    /// Circuit breaker closed after successful recovery.
    CircuitBreakerClosed { agent_name: String },

    /// Rate limit threshold reached for an agent.
    RateLimitHit {
        agent_name: String,
        limit_type: String,
        retry_after_ms: Option<u64>,
    },

    /// Rate limit window reset.
    RateLimitReset {
        agent_name: String,
        limit_type: String,
    },

    // --- Agent health monitoring events ---
    /// Agent health has degraded (high error rate or latency).
    AgentHealthDegraded {
        agent_name: String,
        error_rate: f64,
        avg_latency_ms: u64,
    },

    /// Agent hit rate limit from provider.
    AgentRateLimited {
        agent_name: String,
        retry_after_secs: u64,
    },

    /// Agent reconnection attempt in progress.
    AgentReconnecting {
        agent_name: String,
        attempt: u32,
        max_attempts: u32,
    },

    /// Agent successfully reconnected.
    AgentReconnected {
        agent_name: String,
        attempts_taken: u32,
    },

    /// Agent heartbeat check failed.
    AgentHeartbeatFailed {
        agent_name: String,
        consecutive_failures: u32,
    },

    // --- Task lifecycle events ---
    /// Task state changed.
    TaskStateChanged {
        task_id: TaskId,
        old_state: TaskState,
        new_state: TaskState,
    },

    /// Subtask started execution.
    SubtaskStarted {
        task_id: TaskId,
        subtask_id: SubtaskId,
    },

    /// Subtask completed.
    SubtaskCompleted {
        task_id: TaskId,
        subtask_id: SubtaskId,
        success: bool,
    },

    // --- QA review events ---
    /// QA review completed with a verdict.
    QaVerdictReceived {
        task_id: TaskId,
        verdict: QaVerdictKind,
        iteration: u32,
        reasoning: Option<String>,
        met_criteria: Vec<String>,
        unmet_criteria: Vec<String>,
        issues: Option<String>,
    },

    // --- Pipeline gate events ---
    /// Pipeline gate is awaiting approval.
    GateAwaitingApproval {
        task_id: TaskId,
        gate_name: String,
        reason: Option<String>,
    },

    /// Pipeline gate was approved.
    GateApproved {
        task_id: TaskId,
        gate_name: String,
        approved_by: Option<String>,
    },

    /// Pipeline gate was rejected.
    GateRejected {
        task_id: TaskId,
        gate_name: String,
        rejected_by: Option<String>,
        reason: Option<String>,
    },

    // --- File events ---
    /// File operation performed by agent.
    FileOperation { operation: String, path: PathBuf },

    // --- Terminal events ---
    /// Terminal created for command execution.
    TerminalCreated {
        terminal_id: String,
        command: String,
    },

    /// Terminal produced output.
    TerminalOutput { terminal_id: String, output: String },

    /// Terminal command exited.
    TerminalExited {
        terminal_id: String,
        exit_code: Option<u32>,
    },

    /// Terminal was killed.
    TerminalKilled { terminal_id: String },

    // --- Tool call events ---
    /// Agent initiated a tool call with full metadata.
    ToolCallStarted {
        session_id: String,
        call_id: String,
        title: String,
        kind: ToolKind,
        locations: Vec<ToolLocation>,
        raw_input: Option<String>,
    },

    /// Tool call received an incremental update.
    ToolCallUpdated {
        session_id: String,
        call_id: String,
        status: Option<ToolCallStatus>,
        title: Option<String>,
        diffs: Vec<ToolDiff>,
        locations: Vec<ToolLocation>,
        raw_output: Option<String>,
    },

    // --- Streaming events ---
    /// Agent message chunk received during prompt streaming.
    AgentMessageChunk { session_id: String, text: String },

    /// Agent thought/reasoning chunk received.
    AgentThoughtChunk { session_id: String, text: String },

    // --- Plan events ---
    /// Agent shared or updated its execution plan.
    PlanUpdated {
        session_id: String,
        entries: Vec<PlanEntry>,
    },

    // --- Spec events ---
    /// Spec was loaded or created.
    SpecLoaded { spec_id: SpecId },

    // --- Usage events ---
    /// Token usage reported by an agent at the end of a prompt turn.
    ///
    /// Aggregation, budgeting, and cost calculation are handled by
    /// `surge-persistence`. `estimated_cost_usd` is `None` until pricing
    /// data is integrated there.
    TokensConsumed {
        /// ACP session that generated the tokens.
        session_id: String,
        /// Agent that processed the turn.
        agent_name: String,
        /// Spec context if this turn was part of a spec execution.
        spec_id: Option<SpecId>,
        /// Subtask context if this turn was part of a subtask execution.
        subtask_id: Option<SubtaskId>,
        /// Total input tokens for this turn.
        input_tokens: u64,
        /// Total output/generation tokens for this turn.
        output_tokens: u64,
        /// Extended thinking/reasoning tokens (Anthropic models).
        thought_tokens: Option<u64>,
        /// Cache-read tokens — reduce billing on repeated prompts.
        cached_read_tokens: Option<u64>,
        /// Cache-write tokens.
        cached_write_tokens: Option<u64>,
        /// Estimated cost in USD, populated by `surge-persistence`.
        estimated_cost_usd: Option<f64>,
    },
}

/// Versioned wrapper for `SurgeEvent` — used by `surge-persistence` for durable
/// event logs. Adding new fields to `SurgeEvent` variants does not change `version`;
/// bump `version` only when old readers cannot safely ignore unknown fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedEvent {
    /// Schema version. Currently always `1`.
    pub version: u32,
    /// Unix timestamp in milliseconds when the event was emitted.
    pub timestamp_ms: u64,
    /// The event payload.
    pub event: SurgeEvent,
}

impl VersionedEvent {
    /// Wrap an event with the current schema version.
    /// Caller supplies the timestamp to keep this crate free of wall-clock I/O.
    pub fn new(event: SurgeEvent, timestamp_ms: u64) -> Self {
        Self {
            version: 1,
            timestamp_ms,
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::toml;

    // VersionedEvent wraps SurgeEvent which uses serde(tag)-free enum —
    // TOML requires a helper wrapper for enums; use a JSON-like table form via
    // the toml crate's Value round-trip for the test.

    fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(val: &T) -> T {
        let s = toml::to_string(val).unwrap();
        toml::from_str(&s).unwrap()
    }

    #[test]
    fn test_versioned_event_fields() {
        let event = SurgeEvent::AgentConnected {
            agent_name: "claude".to_string(),
        };
        let versioned = VersionedEvent::new(event, 1_700_000_000_000);
        assert_eq!(versioned.version, 1);
        assert_eq!(versioned.timestamp_ms, 1_700_000_000_000);
        assert!(matches!(versioned.event, SurgeEvent::AgentConnected { .. }));
    }

    #[test]
    fn test_versioned_event_roundtrip() {
        let versioned = VersionedEvent::new(
            SurgeEvent::AgentConnected {
                agent_name: "claude".to_string(),
            },
            42,
        );
        let rt = roundtrip(&versioned);
        assert_eq!(rt.version, 1);
        assert_eq!(rt.timestamp_ms, 42);
        assert!(matches!(rt.event, SurgeEvent::AgentConnected { .. }));
    }

    #[test]
    fn test_tokens_consumed_roundtrip() {
        let spec_id = SpecId::new();
        let subtask_id = SubtaskId::new();
        let versioned = VersionedEvent::new(
            SurgeEvent::TokensConsumed {
                session_id: "sess-1".to_string(),
                agent_name: "claude".to_string(),
                spec_id: Some(spec_id),
                subtask_id: Some(subtask_id),
                input_tokens: 1000,
                output_tokens: 500,
                thought_tokens: Some(200),
                cached_read_tokens: None,
                cached_write_tokens: None,
                estimated_cost_usd: Some(0.005),
            },
            0,
        );
        let rt = roundtrip(&versioned);
        if let SurgeEvent::TokensConsumed {
            input_tokens,
            output_tokens,
            spec_id: rt_spec_id,
            subtask_id: rt_subtask_id,
            ..
        } = rt.event
        {
            assert_eq!(input_tokens, 1000);
            assert_eq!(output_tokens, 500);
            assert_eq!(rt_spec_id, Some(spec_id));
            assert_eq!(rt_subtask_id, Some(subtask_id));
        } else {
            panic!("wrong variant");
        }
    }
}
