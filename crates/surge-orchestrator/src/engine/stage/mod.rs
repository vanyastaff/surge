//! Stage execution dispatch.

pub mod agent;
pub mod bindings;
pub mod branch;
pub mod human_gate;
pub mod loop_stage;
pub mod notify;
pub mod skill_binding;
pub mod subgraph_stage;
pub mod terminal;

use std::time::Duration;

use crate::engine::error::EngineError;
use surge_core::keys::OutcomeKey;
use thiserror::Error;

/// Outcome of one stage's execution. The cursor's next position is determined
/// by routing on this `OutcomeKey`.
pub type StageResult = Result<OutcomeKey, StageError>;

/// Errors that can occur during a single stage's execution.
///
/// `#[non_exhaustive]`: this crate alone decides this taxonomy and is
/// expected to keep growing it, so no external crate may match it
/// exhaustively — every addition here would otherwise be a breaking change
/// for such a caller. The trade-off is explicit: an external `match` is
/// forced to carry a wildcard arm from day one, so a genuinely new failure
/// mode lands quietly in that wildcard until the caller deliberately gives
/// it its own handling.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StageError {
    /// A `LoopGuard` trip ended this stage (`.autopilot/competitive-waves/spec.md`
    /// §15). Distinct from `AgentCrashed`: nothing crashed — the engine
    /// deliberately stops a node that stopped making progress ("escalating
    /// instead of burning budget", R39). Returned only after the session has
    /// already been closed and the matching `SessionClosed { disposition:
    /// ForcedClose }` event recorded. Carries the typed
    /// [`crate::guard::LoopGuardTrip`] so a caller does not have to parse
    /// this error's `Display` text apart to tell which guard tripped.
    #[error("{0}")]
    LoopGuardTripped(crate::guard::LoopGuardTrip),

    /// The ACP agent process crashed or the session ended abnormally.
    #[error("agent crashed: {0}")]
    AgentCrashed(String),

    /// The agent reported an outcome not declared in the node's spec.
    #[error("agent reported undeclared outcome: {0}")]
    UndeclaredOutcome(String),

    /// A `HumanGate` timed out or was explicitly rejected.
    #[error("human gate rejected (timeout or explicit)")]
    HumanGateRejected,

    /// `TimeoutAction::Continue` was set but no default outcome is configured.
    #[error("human gate has TimeoutAction::Continue but no default outcome configured")]
    HumanGateContinueWithoutDefault,

    /// A persistence write failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// An ACP bridge call failed.
    #[error("bridge error: {0}")]
    Bridge(String),

    /// The agent hit a provider-side rate limit or usage quota while this
    /// stage was mid-turn — matched directly off
    /// `surge_acp::bridge::error::SendMessageError::RateLimited`, never by
    /// re-parsing its rendered `Display` text. Kept as its own variant —
    /// rather than folded into [`Self::Bridge`] and stringified — so a
    /// capacity-aware caller can park the run on a known (or unknown) reset
    /// time instead of burning its retry budget against a wall that will
    /// not move (R37).
    #[error("agent rate limited (runtime = {runtime:?}, retry_after = {retry_after:?}): {details}")]
    RateLimited {
        /// The resolved profile's runtime registry id (e.g. `"claude-acp"`),
        /// normalized through `surge_acp::Registry::normalize_agent_id` so
        /// aliases of one entry collapse to the same string — or, when
        /// normalization fails despite a profile being resolved (e.g. the
        /// bundled `mock` profile, not itself a registry id or alias), the
        /// raw `agent_id`, so an unrecognized-but-real identity is never
        /// silently discarded. `None` **only** on the legacy
        /// no-profile-registry path, where no id is known at all — a
        /// placeholder string here would be fabricating an identity nothing
        /// observed.
        ///
        /// **Resolved (Task 12 revision 6, A1):** this key names the agent
        /// runtime, not a separately-tracked login — and today those two
        /// coincide for every installation Surge can express (see
        /// `surge_core::capacity`'s module doc, "Why the key is the
        /// runtime"). `builtin_registry.json` carries exactly one launch
        /// configuration per runtime, so there is no second axis a second
        /// login could be keyed on yet. Where a future installation *can*
        /// distinguish two logins sharing one runtime, this field's
        /// granularity errs toward the safe side: it collapses both into
        /// one capacity window, which over-parks (a false-positive refusal)
        /// rather than under-parks (dispatching into a wall) — see A2 in
        /// `docs/adr/0016-capacity-parking-and-wake.md` for the follow-up
        /// that would actually distinguish them.
        runtime: Option<String>,
        /// Provider-supplied retry delay, when the raw ACP error text
        /// carried one. Mirrors
        /// `surge_acp::bridge::error::SendMessageError::RateLimited::retry_after`.
        retry_after: Option<Duration>,
        /// Raw error text carried over from the bridge-level error, kept
        /// for debugging.
        details: String,
    },

    /// The run was cancelled while the stage was executing.
    #[error("cancelled")]
    Cancelled,

    /// An unexpected internal condition occurred within the stage executor.
    #[error("internal: {0}")]
    Internal(String),

    /// Loop body subgraph not found in `Graph::subgraphs`.
    #[error("loop body subgraph not found: {0}")]
    LoopBodyMissing(surge_core::keys::SubgraphKey),

    /// Loop iterable resolved to more items than `MAX_LOOP_ITEMS_RESOLVED`.
    #[error("loop iterable too large: {count}/{max}")]
    LoopItemsTooLarge {
        /// Actual number of items resolved.
        count: u32,
        /// Maximum permitted (`MAX_LOOP_ITEMS_RESOLVED`).
        max: u32,
    },

    /// Subgraph reference not found in `Graph::subgraphs`. (Reserved for Task 6.1.)
    #[error("subgraph reference not found: {0}")]
    SubgraphMissing(surge_core::keys::SubgraphKey),

    /// Notify channel delivery failed and `on_failure: Fail` is configured. (Reserved for Phase 8.)
    #[error("notify delivery error: {0}")]
    NotifyDelivery(String),

    /// Bootstrap edit-loop cap exceeded: the operator chose `edit` more
    /// times than `EngineRunConfig.bootstrap.edit_loop_cap` allows for the
    /// given stage. Engine surfaces this as a terminal failure plus an
    /// `EscalationRequested` notify event.
    #[error("bootstrap edit-loop cap exceeded for stage {stage:?} (cap = {cap})")]
    EditLoopCapExceeded {
        /// Bootstrap stage that ran out of retries.
        stage: surge_core::run_event::BootstrapStage,
        /// The configured cap value.
        cap: u32,
    },

    /// A node's declared skill failed to resolve against the discovered
    /// catalog — not found, malformed manifest, or ambiguous content across
    /// ProjectDir/UserDir/Registry candidates.
    #[error("skill resolution failed: {0}")]
    SkillResolutionFailed(#[from] surge_core::skill::SkillError),

    /// An unpinned, unhashed, or content-changed skill's trust prompt was
    /// denied or went unanswered within the timeout. The node does not
    /// start.
    #[error("skill approval rejected (timeout or explicit denial)")]
    SkillApprovalRejected,

    /// A node's `custom_fields["skills"]` declaration does not deserialize
    /// as a list of `surge_core::skill::SkillRef` — a graph-authoring
    /// mistake, not an internal engine condition. Kept as its own variant
    /// (rather than flattened into `Internal(String)`) so the typed
    /// `DeclaredSkillsError` — which names the malformed TOML and why —
    /// survives to whatever reports the failure, matching the file+reason
    /// standard `SkillError::MalformedFrontmatter` already holds for a
    /// broken `SKILL.md` (History 15 / R09.1).
    #[error("node {node} declares invalid skills: {source}")]
    InvalidSkillsDeclaration {
        /// The node whose declaration failed to parse.
        node: surge_core::keys::NodeKey,
        /// The typed parse failure.
        #[source]
        source: surge_core::agent_config::DeclaredSkillsError,
    },
}

impl From<StageError> for EngineError {
    fn from(e: StageError) -> Self {
        EngineError::Internal(format!("stage error: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::capacity::{CapacityWindow, RemainingShare};

    /// Pins the exact invariant `surge-cli`'s inbox capacity scan depends on.
    /// `resolve_stage_error` (`engine/run_task.rs`) persists
    /// `format!("stage error at {node}: {error}")` as `StageFailed.reason`,
    /// and `surge-cli`'s `scan_capacity_signal` (`commands/inbox.rs`) re-runs
    /// `CapacityWindow::from_observed_error` against that exact string —
    /// never against `StageError` itself. Today this only works because
    /// `RateLimited`'s `#[error(...)]` format splices `details` in verbatim,
    /// and its own `retry_after`/`runtime` debug formatting uses `retry_after`
    /// (underscore) rather than `retry-after`/`retry after`, so it never
    /// shadows the real marker inside `details`. Renaming `details`, dropping
    /// it from the format string, or introducing a literal "retry after"
    /// ahead of it would silently break the inbox's capacity signal without
    /// failing any test that looks at `StageError` in isolation — this one
    /// goes through the full persisted string on purpose. If it goes red,
    /// the fix belongs in the `#[error(...)]` format, not in this test.
    #[test]
    fn rate_limited_display_still_classifies_through_the_full_stage_failed_reason() {
        let node = surge_core::keys::NodeKey::try_from("plan_1").unwrap();
        let error = StageError::RateLimited {
            runtime: Some("claude-acp".to_string()),
            retry_after: Some(Duration::from_secs(30)),
            details: "429 Too Many Requests: Retry-After: 30".to_string(),
        };
        // Exact construction `resolve_stage_error` uses for `StageFailed.reason`.
        let raw_reason = format!("stage error at {node}: {error}");

        let observed_at = chrono::Utc::now();
        let window = CapacityWindow::from_observed_error("claude-acp", &raw_reason, observed_at)
            .expect(
                "the inbox's scan_capacity_signal must still classify a persisted \
                 RateLimited StageFailed.reason as a rate-limit signal",
            );
        assert_eq!(window.remaining(), Some(RemainingShare::EXHAUSTED));
        assert_eq!(
            window.resets_at(),
            Some(observed_at + chrono::Duration::seconds(30))
        );
    }
}
