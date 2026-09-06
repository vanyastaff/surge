//! Engine-level loop hygiene for a node's agent stage.
//!
//! [`LoopGuard`] watches the tool calls one node's agent stage makes and
//! decides — the engine decides, never the agent under observation, per
//! `.autopilot/competitive-waves/spec.md` §15 ("an agent watching its own
//! looping is the same agent"). Two independent trips: the same tool call
//! repeated past a threshold ([`LoopGuard::observe`]), and the node running
//! past a wall-clock budget ([`LoopGuard::deadline`]). Both trips carry a
//! typed [`LoopGuardTrip`] rather than a free-form string, so a caller that
//! turns one into an operator-facing event or a durable trace does not have
//! to parse prose to know which kind of trip it was.
//!
//! Thresholds come from [`surge_core::loop_config::ToolCallLoopGuardConfig`]
//! (a `surge.toml` key); this module hides the repeat-window bookkeeping and
//! counters behind [`LoopGuard`]'s private fields.

use std::time::{Duration, Instant};

use surge_core::content_hash::ContentHash;
use surge_core::loop_config::ToolCallLoopGuardConfig;

use crate::engine::tools::ToolCall;

/// What a [`LoopGuard`] found when it decided to escalate.
///
/// Typed on purpose: a consumer (an event, a durable run-status trace) reads
/// the trip kind and its numbers directly instead of matching substrings out
/// of an operator-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopGuardTrip {
    /// The same tool call (name + arguments) repeated past
    /// `max_repeat_tool_calls`.
    RepeatedToolCall {
        /// Name of the tool being repeated.
        tool: String,
        /// How many consecutive times it was observed, including this call.
        repeat_count: u32,
        /// Configured threshold that was exceeded.
        threshold: u32,
    },
    /// The node's agent stage ran past `node_wall_clock_limit_secs`.
    NodeDeadlineExceeded {
        /// How long the node has been running.
        elapsed: Duration,
        /// Configured wall-clock budget that was exceeded.
        limit: Duration,
    },
}

impl LoopGuardTrip {
    /// Render an operator-readable explanation, suitable for the `reason`
    /// field of an escalation surface. Kept separate from `Display` so a
    /// caller that wants the typed value for a durable trace is not forced
    /// to parse this string back apart.
    #[must_use]
    pub fn operator_message(&self) -> String {
        match self {
            Self::RepeatedToolCall {
                tool,
                repeat_count,
                threshold,
            } => format!(
                "node loop guard: tool '{tool}' called {repeat_count} times in a row \
                 (threshold {threshold}); escalating instead of dispatching it again"
            ),
            Self::NodeDeadlineExceeded { elapsed, limit } => format!(
                "node loop guard: node has run for {elapsed_secs}s, past its \
                 {limit_secs}s wall-clock budget; escalating",
                elapsed_secs = elapsed.as_secs(),
                limit_secs = limit.as_secs(),
            ),
        }
    }
}

/// Verdict [`LoopGuard`] returns for each observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// No guard tripped; dispatch the call as usual.
    Continue,
    /// A guard tripped; the caller must not dispatch the call and should
    /// surface `LoopGuardTrip` as an escalation instead.
    Escalate(LoopGuardTrip),
}

/// Per-node loop guard. One instance covers one node's agent stage, mirroring
/// `RoutingToolDispatcher`'s own per-agent-stage lifecycle: `started_at` is
/// recorded at construction and stands in for "when this node started."
#[derive(Debug)]
pub struct LoopGuard {
    config: ToolCallLoopGuardConfig,
    started_at: Instant,
    last_call_fingerprint: Option<ContentHash>,
    repeat_streak: u32,
}

impl LoopGuard {
    /// Build a guard for one node, starting its wall-clock budget now.
    #[must_use]
    pub fn new(config: ToolCallLoopGuardConfig) -> Self {
        Self {
            config,
            started_at: Instant::now(),
            last_call_fingerprint: None,
            repeat_streak: 0,
        }
    }

    /// Observe one tool call about to be dispatched. Returns
    /// [`Verdict::Escalate`] once the same call (tool name + arguments) has
    /// been observed more than `max_repeat_tool_calls` times in a row.
    ///
    /// Fingerprints by content hash over `tool` and the JSON-serialized
    /// `arguments`; a semantically-identical call whose argument object key
    /// order differs would not match, which in practice does not happen — a
    /// single agent turn does not vary key order for the same call.
    pub fn observe(&mut self, call: &ToolCall) -> Verdict {
        let fingerprint =
            ContentHash::compute(format!("{}\u{0}{}", call.tool, call.arguments).as_bytes());

        if self.last_call_fingerprint == Some(fingerprint) {
            self.repeat_streak += 1;
        } else {
            self.last_call_fingerprint = Some(fingerprint);
            self.repeat_streak = 1;
        }

        if self.repeat_streak > self.config.max_repeat_tool_calls {
            return Verdict::Escalate(LoopGuardTrip::RepeatedToolCall {
                tool: call.tool.clone(),
                repeat_count: self.repeat_streak,
                threshold: self.config.max_repeat_tool_calls,
            });
        }
        Verdict::Continue
    }

    /// Check this guard's node against its configured wall-clock budget.
    ///
    /// Takes no node identifier: a `LoopGuard` already covers exactly one
    /// node (see the struct doc), and `self.started_at` is the source of
    /// truth for elapsed time — there is nothing external to look up.
    #[must_use]
    pub fn deadline(&self) -> Verdict {
        let limit = Duration::from_secs(self.config.node_wall_clock_limit_secs);
        let elapsed = self.started_at.elapsed();
        if elapsed > limit {
            return Verdict::Escalate(LoopGuardTrip::NodeDeadlineExceeded { elapsed, limit });
        }
        Verdict::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            call_id: "c1".into(),
            tool: tool.into(),
            arguments: args,
        }
    }

    #[test]
    fn distinct_calls_never_escalate() {
        let mut guard = LoopGuard::new(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 3,
            node_wall_clock_limit_secs: 3600,
        });
        for i in 0..10 {
            let verdict = guard.observe(&call("shell_exec", serde_json::json!({ "n": i })));
            assert_eq!(verdict, Verdict::Continue);
        }
    }

    #[test]
    fn repeat_within_threshold_continues() {
        let mut guard = LoopGuard::new(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 3,
            node_wall_clock_limit_secs: 3600,
        });
        let repeated = call("read_file", serde_json::json!({ "path": "a.rs" }));
        for _ in 0..3 {
            assert_eq!(guard.observe(&repeated), Verdict::Continue);
        }
    }

    #[test]
    fn repeat_past_threshold_escalates() {
        let mut guard = LoopGuard::new(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 3,
            node_wall_clock_limit_secs: 3600,
        });
        let repeated = call("read_file", serde_json::json!({ "path": "a.rs" }));
        for _ in 0..3 {
            assert_eq!(guard.observe(&repeated), Verdict::Continue);
        }
        match guard.observe(&repeated) {
            Verdict::Escalate(LoopGuardTrip::RepeatedToolCall {
                tool,
                repeat_count,
                threshold,
            }) => {
                assert_eq!(tool, "read_file");
                assert_eq!(repeat_count, 4);
                assert_eq!(threshold, 3);
            },
            other => panic!("expected RepeatedToolCall escalation, got {other:?}"),
        }
    }

    #[test]
    fn a_different_call_resets_the_streak() {
        let mut guard = LoopGuard::new(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 2,
            node_wall_clock_limit_secs: 3600,
        });
        let a = call("read_file", serde_json::json!({ "path": "a.rs" }));
        let b = call("read_file", serde_json::json!({ "path": "b.rs" }));
        assert_eq!(guard.observe(&a), Verdict::Continue);
        assert_eq!(guard.observe(&a), Verdict::Continue);
        // Third `a` in a row would escalate (threshold 2) — but `b` breaks
        // the streak, so the count restarts from 1.
        assert_eq!(guard.observe(&b), Verdict::Continue);
        assert_eq!(guard.observe(&b), Verdict::Continue);
    }

    #[test]
    fn deadline_within_budget_continues() {
        let guard = LoopGuard::new(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 3,
            node_wall_clock_limit_secs: 3600,
        });
        assert_eq!(guard.deadline(), Verdict::Continue);
    }

    #[test]
    fn deadline_past_budget_escalates() {
        // A zero-second budget is already exceeded the instant the guard
        // is constructed — deterministic without sleeping in a unit test.
        let guard = LoopGuard::new(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 3,
            node_wall_clock_limit_secs: 0,
        });
        match guard.deadline() {
            Verdict::Escalate(LoopGuardTrip::NodeDeadlineExceeded { limit, .. }) => {
                assert_eq!(limit, Duration::from_secs(0));
            },
            other => panic!("expected NodeDeadlineExceeded escalation, got {other:?}"),
        }
    }

    #[test]
    fn operator_message_names_the_tool_and_counts() {
        let trip = LoopGuardTrip::RepeatedToolCall {
            tool: "shell_exec".into(),
            repeat_count: 4,
            threshold: 3,
        };
        let msg = trip.operator_message();
        assert!(msg.contains("shell_exec"));
        assert!(msg.contains('4'));
        assert!(msg.contains('3'));
    }
}
