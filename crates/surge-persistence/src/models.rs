//! Data models for token usage tracking and task checkpoints.

use serde::{Deserialize, Serialize};
use surge_core::id::{SpecId, SubtaskId, TaskId};
use surge_core::spec::SubtaskState;

// ── Task Checkpoint Models ──────────────────────────────────────────

/// Checkpoint record for task execution state persistence.
///
/// Stores the current execution state and retry count for a subtask,
/// enabling recovery from failures and implementing retry policies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskCheckpoint {
    /// Spec this checkpoint belongs to.
    pub spec_id: SpecId,

    /// Subtask this checkpoint tracks (if any).
    pub subtask_id: Option<SubtaskId>,

    /// Current execution state of the subtask.
    pub state: SubtaskState,

    /// Number of retry attempts for this subtask.
    pub retry_count: u32,
}

// ── Circuit Breaker Models ──────────────────────────────────────────

/// Circuit breaker state persistence record.
///
/// Stores the current state of a circuit breaker for a subtask, enabling
/// recovery after crashes/restarts and preventing infinite retry loops.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CircuitBreakerState {
    /// Task this circuit breaker belongs to.
    pub task_id: TaskId,

    /// Subtask this circuit breaker tracks.
    pub subtask_id: SubtaskId,

    /// Number of consecutive failures recorded.
    pub consecutive_failures: u32,

    /// Last error message that triggered a failure.
    pub last_error: Option<String>,

    /// Unix timestamp in milliseconds when the circuit was tripped (if open).
    pub tripped_at: Option<u64>,

    /// Unix timestamp in milliseconds for the next retry attempt.
    pub next_retry_time: Option<u64>,
}

impl CircuitBreakerState {
    /// Create a new circuit breaker state for a subtask.
    #[must_use]
    pub fn new(task_id: TaskId, subtask_id: SubtaskId) -> Self {
        Self {
            task_id,
            subtask_id,
            consecutive_failures: 0,
            last_error: None,
            tripped_at: None,
            next_retry_time: None,
        }
    }

    /// Check if the circuit breaker is tripped (open).
    ///
    /// A circuit is considered tripped if it has a `tripped_at` timestamp.
    #[must_use]
    pub fn is_tripped(&self) -> bool {
        self.tripped_at.is_some()
    }

    /// Reset the circuit breaker state after a successful execution.
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.last_error = None;
        self.tripped_at = None;
        self.next_retry_time = None;
    }

    /// Record a failure and increment the consecutive failure count.
    pub fn record_failure(&mut self, error_msg: String, next_retry_time_ms: Option<u64>) {
        self.consecutive_failures += 1;
        self.last_error = Some(error_msg);
        self.next_retry_time = next_retry_time_ms;
    }

    /// Trip the circuit breaker (mark as open).
    pub fn trip(&mut self, timestamp_ms: u64) {
        self.tripped_at = Some(timestamp_ms);
    }
}

// ── Token Usage Models ──────────────────────────────────────────────

/// Token usage for a single ACP session.
///
/// Represents a single turn of agent interaction, capturing all token
/// consumption metrics reported by the agent via the `TokensConsumed` event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionUsage {
    /// ACP session identifier.
    pub session_id: String,

    /// Agent that processed this session.
    pub agent_name: String,

    /// Task this session belongs to.
    pub task_id: TaskId,

    /// Subtask this session belongs to (if any).
    pub subtask_id: Option<SubtaskId>,

    /// Spec this session is associated with.
    pub spec_id: SpecId,

    /// Unix timestamp in milliseconds when the session started.
    pub timestamp_ms: u64,

    /// Total input tokens for this session.
    pub input_tokens: u64,

    /// Total output/generation tokens for this session.
    pub output_tokens: u64,

    /// Extended thinking/reasoning tokens (Anthropic models).
    pub thought_tokens: Option<u64>,

    /// Cache-read tokens — reduce billing on repeated prompts.
    pub cached_read_tokens: Option<u64>,

    /// Cache-write tokens.
    pub cached_write_tokens: Option<u64>,

    /// Estimated cost in USD for this session.
    pub estimated_cost_usd: Option<f64>,
}

impl SessionUsage {
    /// Calculate the total tokens consumed in this session.
    ///
    /// Includes input, output, and thought tokens. Cache tokens are tracked
    /// separately and not included in the total.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.thought_tokens.unwrap_or(0)
    }
}

/// Aggregated token usage for a subtask.
///
/// Accumulates all token consumption across multiple sessions that occurred
/// during the execution of a single subtask.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubtaskUsage {
    /// Subtask identifier.
    pub subtask_id: SubtaskId,

    /// Task this subtask belongs to.
    pub task_id: TaskId,

    /// Spec this subtask is associated with.
    pub spec_id: SpecId,

    /// Total input tokens across all sessions for this subtask.
    pub input_tokens: u64,

    /// Total output tokens across all sessions for this subtask.
    pub output_tokens: u64,

    /// Total thought tokens across all sessions for this subtask.
    pub thought_tokens: u64,

    /// Total cache-read tokens across all sessions for this subtask.
    pub cached_read_tokens: u64,

    /// Total cache-write tokens across all sessions for this subtask.
    pub cached_write_tokens: u64,

    /// Total estimated cost in USD for this subtask.
    pub estimated_cost_usd: f64,

    /// Number of sessions that contributed to this subtask.
    pub session_count: u32,
}

impl SubtaskUsage {
    /// Calculate the total tokens consumed by this subtask.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.thought_tokens
    }

    /// Create a new `SubtaskUsage` from a single `SessionUsage`.
    ///
    /// Returns `None` if the session has no `subtask_id` (e.g. top-level spec usage).
    #[must_use]
    pub fn from_session(session: &SessionUsage) -> Option<Self> {
        let subtask_id = session.subtask_id?;
        Some(Self {
            subtask_id,
            task_id: session.task_id,
            spec_id: session.spec_id,
            input_tokens: session.input_tokens,
            output_tokens: session.output_tokens,
            thought_tokens: session.thought_tokens.unwrap_or(0),
            cached_read_tokens: session.cached_read_tokens.unwrap_or(0),
            cached_write_tokens: session.cached_write_tokens.unwrap_or(0),
            estimated_cost_usd: session.estimated_cost_usd.unwrap_or(0.0),
            session_count: 1,
        })
    }

    /// Aggregate another session's usage into this subtask.
    pub fn add_session(&mut self, session: &SessionUsage) {
        self.input_tokens += session.input_tokens;
        self.output_tokens += session.output_tokens;
        self.thought_tokens += session.thought_tokens.unwrap_or(0);
        self.cached_read_tokens += session.cached_read_tokens.unwrap_or(0);
        self.cached_write_tokens += session.cached_write_tokens.unwrap_or(0);
        self.estimated_cost_usd += session.estimated_cost_usd.unwrap_or(0.0);
        self.session_count += 1;
    }
}

/// Aggregated token usage for an entire spec.
///
/// Accumulates all token consumption across all subtasks and sessions
/// that occurred during the execution of a spec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpecUsage {
    /// Spec identifier.
    pub spec_id: SpecId,

    /// Total input tokens across all subtasks for this spec.
    pub input_tokens: u64,

    /// Total output tokens across all subtasks for this spec.
    pub output_tokens: u64,

    /// Total thought tokens across all subtasks for this spec.
    pub thought_tokens: u64,

    /// Total cache-read tokens across all subtasks for this spec.
    pub cached_read_tokens: u64,

    /// Total cache-write tokens across all subtasks for this spec.
    pub cached_write_tokens: u64,

    /// Total estimated cost in USD for this spec.
    pub estimated_cost_usd: f64,

    /// Number of subtasks that contributed to this spec.
    pub subtask_count: u32,

    /// Number of sessions that contributed to this spec.
    pub session_count: u32,
}

impl SpecUsage {
    /// Calculate the total tokens consumed by this spec.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.thought_tokens
    }

    /// Create a new `SpecUsage` from a single `SessionUsage`.
    #[must_use]
    pub fn from_session(session: &SessionUsage) -> Self {
        Self {
            spec_id: session.spec_id,
            input_tokens: session.input_tokens,
            output_tokens: session.output_tokens,
            thought_tokens: session.thought_tokens.unwrap_or(0),
            cached_read_tokens: session.cached_read_tokens.unwrap_or(0),
            cached_write_tokens: session.cached_write_tokens.unwrap_or(0),
            estimated_cost_usd: session.estimated_cost_usd.unwrap_or(0.0),
            subtask_count: if session.subtask_id.is_some() { 1 } else { 0 },
            session_count: 1,
        }
    }

    /// Create a new `SpecUsage` from a single `SubtaskUsage`.
    #[must_use]
    pub fn from_subtask(subtask: &SubtaskUsage) -> Self {
        Self {
            spec_id: subtask.spec_id,
            input_tokens: subtask.input_tokens,
            output_tokens: subtask.output_tokens,
            thought_tokens: subtask.thought_tokens,
            cached_read_tokens: subtask.cached_read_tokens,
            cached_write_tokens: subtask.cached_write_tokens,
            estimated_cost_usd: subtask.estimated_cost_usd,
            subtask_count: 1,
            session_count: subtask.session_count,
        }
    }

    /// Aggregate another session's usage into this spec.
    pub fn add_session(&mut self, session: &SessionUsage) {
        self.input_tokens += session.input_tokens;
        self.output_tokens += session.output_tokens;
        self.thought_tokens += session.thought_tokens.unwrap_or(0);
        self.cached_read_tokens += session.cached_read_tokens.unwrap_or(0);
        self.cached_write_tokens += session.cached_write_tokens.unwrap_or(0);
        self.estimated_cost_usd += session.estimated_cost_usd.unwrap_or(0.0);
        self.session_count += 1;
    }

    /// Aggregate another subtask's usage into this spec.
    pub fn add_subtask(&mut self, subtask: &SubtaskUsage) {
        self.input_tokens += subtask.input_tokens;
        self.output_tokens += subtask.output_tokens;
        self.thought_tokens += subtask.thought_tokens;
        self.cached_read_tokens += subtask.cached_read_tokens;
        self.cached_write_tokens += subtask.cached_write_tokens;
        self.estimated_cost_usd += subtask.estimated_cost_usd;
        self.subtask_count += 1;
        self.session_count += subtask.session_count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session() -> SessionUsage {
        SessionUsage {
            session_id: "sess-1".to_string(),
            agent_name: "claude".to_string(),
            task_id: TaskId::new(),
            subtask_id: Some(SubtaskId::new()),
            spec_id: SpecId::new(),
            timestamp_ms: 1_700_000_000_000,
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: Some(200),
            cached_read_tokens: Some(100),
            cached_write_tokens: Some(50),
            estimated_cost_usd: Some(0.005),
        }
    }

    #[test]
    fn test_session_usage_total_tokens() {
        let session = sample_session();
        assert_eq!(session.total_tokens(), 1700); // 1000 + 500 + 200
    }

    #[test]
    fn test_session_usage_total_tokens_without_thought() {
        let mut session = sample_session();
        session.thought_tokens = None;
        assert_eq!(session.total_tokens(), 1500); // 1000 + 500
    }

    #[test]
    fn test_subtask_usage_from_session() {
        let session = sample_session();
        let subtask = SubtaskUsage::from_session(&session).unwrap();

        assert_eq!(subtask.subtask_id, session.subtask_id.unwrap());
        assert_eq!(subtask.task_id, session.task_id);
        assert_eq!(subtask.spec_id, session.spec_id);
        assert_eq!(subtask.input_tokens, 1000);
        assert_eq!(subtask.output_tokens, 500);
        assert_eq!(subtask.thought_tokens, 200);
        assert_eq!(subtask.cached_read_tokens, 100);
        assert_eq!(subtask.cached_write_tokens, 50);
        assert_eq!(subtask.estimated_cost_usd, 0.005);
        assert_eq!(subtask.session_count, 1);
    }

    #[test]
    fn test_subtask_usage_add_session() {
        let session1 = sample_session();
        let mut subtask = SubtaskUsage::from_session(&session1).unwrap();

        let mut session2 = sample_session();
        session2.session_id = "sess-2".to_string();
        session2.input_tokens = 500;
        session2.output_tokens = 300;

        subtask.add_session(&session2);

        assert_eq!(subtask.input_tokens, 1500); // 1000 + 500
        assert_eq!(subtask.output_tokens, 800); // 500 + 300
        assert_eq!(subtask.session_count, 2);
    }

    #[test]
    fn test_subtask_usage_total_tokens() {
        let session = sample_session();
        let subtask = SubtaskUsage::from_session(&session).unwrap();
        assert_eq!(subtask.total_tokens(), 1700); // 1000 + 500 + 200
    }

    #[test]
    fn test_spec_usage_from_session() {
        let session = sample_session();
        let spec = SpecUsage::from_session(&session);

        assert_eq!(spec.spec_id, session.spec_id);
        assert_eq!(spec.input_tokens, 1000);
        assert_eq!(spec.output_tokens, 500);
        assert_eq!(spec.thought_tokens, 200);
        assert_eq!(spec.estimated_cost_usd, 0.005);
        assert_eq!(spec.subtask_count, 1);
        assert_eq!(spec.session_count, 1);
    }

    #[test]
    fn test_spec_usage_from_subtask() {
        let session = sample_session();
        let subtask = SubtaskUsage::from_session(&session).unwrap();
        let spec = SpecUsage::from_subtask(&subtask);

        assert_eq!(spec.spec_id, subtask.spec_id);
        assert_eq!(spec.input_tokens, 1000);
        assert_eq!(spec.output_tokens, 500);
        assert_eq!(spec.subtask_count, 1);
        assert_eq!(spec.session_count, 1);
    }

    #[test]
    fn test_spec_usage_add_session() {
        let session1 = sample_session();
        let mut spec = SpecUsage::from_session(&session1);

        let mut session2 = sample_session();
        session2.session_id = "sess-2".to_string();
        session2.input_tokens = 800;
        session2.output_tokens = 400;

        spec.add_session(&session2);

        assert_eq!(spec.input_tokens, 1800); // 1000 + 800
        assert_eq!(spec.output_tokens, 900); // 500 + 400
        assert_eq!(spec.session_count, 2);
    }

    #[test]
    fn test_spec_usage_add_subtask() {
        let session1 = sample_session();
        let subtask1 = SubtaskUsage::from_session(&session1).unwrap();
        let mut spec = SpecUsage::from_subtask(&subtask1);

        let mut session2 = sample_session();
        session2.subtask_id = Some(SubtaskId::new()); // Different subtask
        session2.input_tokens = 600;
        session2.output_tokens = 300;
        let subtask2 = SubtaskUsage::from_session(&session2).unwrap();

        spec.add_subtask(&subtask2);

        assert_eq!(spec.input_tokens, 1600); // 1000 + 600
        assert_eq!(spec.output_tokens, 800); // 500 + 300
        assert_eq!(spec.subtask_count, 2);
        assert_eq!(spec.session_count, 2);
    }

    #[test]
    fn test_spec_usage_total_tokens() {
        let session = sample_session();
        let spec = SpecUsage::from_session(&session);
        assert_eq!(spec.total_tokens(), 1700); // 1000 + 500 + 200
    }

    #[test]
    fn test_models_are_serializable() {
        let session = sample_session();
        let json = serde_json::to_string(&session).unwrap();
        let deserialized: SessionUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(session, deserialized);

        let subtask = SubtaskUsage::from_session(&session).unwrap();
        let json = serde_json::to_string(&subtask).unwrap();
        let deserialized: SubtaskUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(subtask, deserialized);

        let spec = SpecUsage::from_session(&session);
        let json = serde_json::to_string(&spec).unwrap();
        let deserialized: SpecUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deserialized);
    }
}
