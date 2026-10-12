//! The run's quota/capacity cycle: what happens before an agent stage
//! dispatches and after a provider reports exhaustion (#93).
//!
//! Two cycles share this module:
//!
//! - **Ordinary capacity** ([`RunCapacity`]): the durable ledger precheck,
//!   fallback-agent rotation (`StageRuntimeRotated`), reactive parking after a
//!   429, the one-shot precheck bypass on a resumed parked run, and clearing
//!   the ledger once a dispatch proves recovery.
//! - **Task-owned quota** ([`task`]): a persistent task's frozen candidate
//!   plan, advanced inside the agent stage under persistence-issued one-shot
//!   opening authority.
//!
//! [`StageOwnership`] states, once, which cycle owns a stage. The run task
//! asks [`RunCapacity::before_dispatch`] and [`RunCapacity::after_rate_limit`]
//! and acts on the answer; it never reads the ledger or the policy itself.
mod task;

pub(crate) use task::{AfterExhaustion, TaskQuotaCycle, TaskQuotaOwner};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use surge_core::capacity::{CapacityPolicy, CapacityStatus, Decision, RotationPolicy, WakeBasis};
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::ReadEvent;
use surge_persistence::runs::run_writer::RunWriter;
use surge_persistence::work_items::recovery_cycles::FrozenQuotaStage;

use crate::engine::capacity::{
    CanonicalRuntimeId, CapacityLedger, WorkEstimator, exhausted_reason,
    run_agent_registry_or_builtin,
};

/// Which cycle owns a stage's capacity decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StageOwnership {
    /// No frozen quota plan: the ordinary cycle prechecks, rotates and parks.
    Ordinary,
    /// A frozen runtime-only plan: the ordinary precheck still applies, but
    /// the stage keeps its planned runtime and a 429 belongs to the task cycle.
    FrozenPlan,
    /// A frozen plan with configured routes under a launch claim: the host
    /// planned the opening, so the ordinary precheck does not apply either.
    HostPlannedRoutes,
}

impl StageOwnership {
    /// The single ownership rule. `frozen` is the run's frozen quota stage for
    /// the node, `claimed` whether the run holds a task launch claim.
    #[must_use]
    pub(crate) fn of(frozen: Option<&FrozenQuotaStage>, claimed: bool) -> Self {
        match frozen {
            None => Self::Ordinary,
            Some(stage)
                if claimed
                    && stage
                        .candidates()
                        .iter()
                        .any(|candidate| candidate.configured_route().is_some()) =>
            {
                Self::HostPlannedRoutes
            },
            Some(_) => Self::FrozenPlan,
        }
    }

    /// Whether a stage may move to a fallback agent or a retry agent.
    #[must_use]
    pub(crate) fn allows_runtime_overrides(self) -> bool {
        self == Self::Ordinary
    }
}

/// Everything the ordinary cycle needs to know about one dispatch attempt.
pub(crate) struct StageCapacity<'a> {
    /// The stage's node.
    pub node: &'a NodeKey,
    /// Canonical runtime this attempt is configured to use (after retry and
    /// rotation overrides). `None` on the legacy no-profile-registry path.
    pub runtime: Option<CanonicalRuntimeId>,
    /// The node's own runtime id, the first rotation candidate.
    pub own_runtime: Option<String>,
    /// Fallback moves already made on this node during this visit.
    pub rotations_this_visit: u32,
    /// Runtimes of the stage's verifier/implementer partners.
    pub partners: &'a [String],
    /// Which cycle owns the stage.
    pub ownership: StageOwnership,
}

/// Where and why a run parks instead of dispatching.
#[derive(Debug, Clone)]
pub(crate) struct ParkPlan {
    pub wake_at: chrono::DateTime<chrono::Utc>,
    pub basis: WakeBasis,
    /// Canonical runtime the exhausted window belongs to.
    pub runtime: Option<String>,
    /// Raw provider text when parking reacts to a 429; `None` at precheck.
    pub details: Option<String>,
}

/// The answer to "may this stage dispatch now?".
#[derive(Debug)]
pub(crate) enum BeforeDispatch {
    Dispatch,
    /// `StageRuntimeRotated` is durable. The caller refolds memory and
    /// re-enters the stage, or parks with `park` if it cannot refold.
    Rotated {
        park: ParkPlan,
    },
    Park(ParkPlan),
    /// The policy produced a decision this cycle cannot act on; fail the
    /// stage loudly rather than dispatch or park on a guess.
    Refuse(String),
}

/// The answer to "this stage was rate limited: what next?".
#[derive(Debug)]
pub(crate) enum AfterRateLimit {
    /// See [`BeforeDispatch::Rotated`].
    Rotated {
        park: ParkPlan,
    },
    Park(ParkPlan),
    /// Hand the original error to the run's ordinary failure routing: the
    /// operator removed `blind_backoff`, or the task cycle owns this 429.
    Surface,
}

/// Per-run ordinary capacity cycle.
pub(crate) struct RunCapacity {
    ledger: Arc<dyn CapacityLedger>,
    estimator: Arc<dyn WorkEstimator>,
    /// The configured policy with rotation disabled: candidate rotation
    /// needs a persisted quota handoff, which only the task cycle has.
    policy: CapacityPolicy,
    fallback_agents: Vec<String>,
    agent_registry: Option<Arc<surge_acp::Registry>>,
    /// ADR-0016 M3: a run resumed from a park gets exactly one agent dispatch
    /// past the precheck, so a runtime whose reset was never learned cannot
    /// re-park on the same stale row forever. Consumed on first read.
    bypass_once: AtomicBool,
}

impl RunCapacity {
    /// Build the cycle for one run. `resumed_from_park` arms the one-shot
    /// precheck bypass.
    #[must_use]
    pub(crate) fn new(
        ledger: Arc<dyn CapacityLedger>,
        estimator: Arc<dyn WorkEstimator>,
        mut policy: CapacityPolicy,
        fallback_agents: Vec<String>,
        agent_registry: Option<Arc<surge_acp::Registry>>,
        resumed_from_park: bool,
    ) -> Self {
        policy.rotation = RotationPolicy::Disabled;
        Self {
            ledger,
            estimator,
            policy,
            fallback_agents,
            agent_registry,
            bypass_once: AtomicBool::new(resumed_from_park),
        }
    }

    /// Canonical ledger key for `raw` under the run's agent registry.
    #[must_use]
    pub(crate) fn canonical(&self, raw: &str) -> CanonicalRuntimeId {
        CanonicalRuntimeId::resolve_for_run(self.agent_registry.as_deref(), raw)
    }

    /// The jittered wake the run actually parks until (ADR-0016 §14).
    #[must_use]
    pub(crate) fn jittered_wake(
        &self,
        wake_at: chrono::DateTime<chrono::Utc>,
        run_id: surge_core::id::RunId,
    ) -> chrono::DateTime<chrono::Utc> {
        self.policy.apply_park_jitter(wake_at, run_id)
    }

    /// Decide whether the stage dispatches, moves to a fallback agent or
    /// parks. Consumes the one-shot resume bypass on every call.
    pub(crate) async fn before_dispatch(
        &self,
        stage: &StageCapacity<'_>,
        writer: &RunWriter,
    ) -> BeforeDispatch {
        let bypass = self.bypass_once.swap(false, Ordering::SeqCst);
        let Some(runtime) = stage.runtime.as_ref() else {
            return BeforeDispatch::Dispatch;
        };
        if bypass || stage.ownership == StageOwnership::HostPlannedRoutes {
            return BeforeDispatch::Dispatch;
        }
        match self.precheck_decision(stage.node, runtime).await {
            Decision::Park { wake_at, basis } => {
                let park = ParkPlan {
                    wake_at,
                    basis,
                    runtime: Some(runtime.as_str().to_owned()),
                    details: None,
                };
                let reason = format!("{runtime} usage limit exhausted until {wake_at}");
                if self.rotate(stage, writer, runtime, &reason).await {
                    BeforeDispatch::Rotated { park }
                } else {
                    BeforeDispatch::Park(park)
                }
            },
            Decision::Dispatch { degraded } => {
                if let Some(reason) = degraded {
                    tracing::warn!(target: "engine::capacity", node = %stage.node,
                        reason = ?reason, "dispatching despite a degraded capacity signal");
                }
                BeforeDispatch::Dispatch
            },
            Decision::Rotate { .. } => BeforeDispatch::Refuse(
                "capacity policy selected rotation although the ordinary cycle disables it".into(),
            ),
        }
    }

    /// Record a typed 429 in the ledger and decide what follows it.
    /// `suspension_requested` is the task cycle's durable claim on this 429.
    pub(crate) async fn after_rate_limit(
        &self,
        stage: &StageCapacity<'_>,
        writer: &RunWriter,
        raw_runtime: &str,
        retry_after: Option<std::time::Duration>,
        details: &str,
        suspension_requested: bool,
    ) -> AfterRateLimit {
        let observed_at = chrono::Utc::now();
        let window = surge_core::capacity::CapacityWindow::observed_429(
            raw_runtime.to_owned(),
            retry_after,
            observed_at,
        );
        let runtime = self.canonical(raw_runtime);
        if let Err(error) = self.ledger.observe(&runtime, &window).await {
            tracing::warn!(target: "engine::capacity", node = %stage.node, %runtime, %error,
                "capacity ledger observe failed; this run still parks from the in-memory window, \
                 but other runs on this runtime will not see the observation");
        }
        if stage.ownership != StageOwnership::Ordinary || suspension_requested {
            return AfterRateLimit::Surface;
        }
        match self
            .policy
            .decide(None, &CapacityStatus::Known(window), observed_at)
        {
            Decision::Park { wake_at, basis } => {
                let park = ParkPlan {
                    wake_at,
                    basis,
                    runtime: Some(raw_runtime.to_owned()),
                    details: Some(details.to_owned()),
                };
                let reason = format!("{raw_runtime} rate limited: {details}");
                if self.rotate(stage, writer, &runtime, &reason).await {
                    AfterRateLimit::Rotated { park }
                } else {
                    AfterRateLimit::Park(park)
                }
            },
            // Rule 4's explicit operator opt-out (`blind_backoff` removed).
            // `Rotate` cannot occur (rotation is disabled here); surfacing the
            // original error is the conservative answer if it ever did.
            Decision::Dispatch { .. } | Decision::Rotate { .. } => AfterRateLimit::Surface,
        }
    }

    /// The runtime a successful dispatch proved recovered: the last
    /// `SessionOpened` for `node` among events strictly after the dispatch
    /// prefix, so earlier attempts cannot supply it.
    #[must_use]
    pub(crate) fn recovered_runtime(
        &self,
        node: &NodeKey,
        events_after_prefix: &[ReadEvent],
    ) -> Option<CanonicalRuntimeId> {
        let runtime = events_after_prefix
            .iter()
            .rev()
            .find_map(|event| match &event.payload.payload {
                EventPayload::SessionOpened {
                    node: opened_node,
                    agent_id,
                    ..
                } if opened_node == node => Some(agent_id.as_deref()),
                _ => None,
            })
            .flatten()?;
        Some(self.canonical(runtime))
    }

    /// Clear a runtime's exhaustion once a dispatch proved it recovered.
    /// Best-effort: a failure only retains a stale row until the next success.
    pub(crate) async fn clear_recovered(&self, node: &NodeKey, runtime: &CanonicalRuntimeId) {
        if let Err(error) = self.ledger.clear(runtime).await {
            tracing::warn!(target: "engine::capacity", %node, %runtime, %error,
                "capacity ledger clear failed; stale exhaustion may persist");
        }
    }

    async fn precheck_decision(&self, node: &NodeKey, runtime: &CanonicalRuntimeId) -> Decision {
        let status = match self.ledger.status(runtime).await {
            Ok(status) => status,
            Err(error) => {
                // A local storage fault must not masquerade as a provider
                // capacity signal: dispatch as if never observed.
                tracing::warn!(target: "engine::capacity", %node, %runtime, %error,
                    "capacity ledger read failed; dispatching as if never observed");
                CapacityStatus::NeverObserved
            },
        };
        // Only a Known, not-yet-exhausted window can consult the estimate
        // (`CapacityPolicy::decide` rule 6); skip the history read otherwise.
        let estimate = if matches!(&status, CapacityStatus::Known(w) if !w.is_exhausted()) {
            self.estimator.estimate(node).await
        } else {
            None
        };
        self.policy
            .decide(estimate.as_ref(), &status, chrono::Utc::now())
    }

    /// Move the stage to a fallback agent (v1 task 1.4): choose among the
    /// node's own runtime and `fallback_agents`, at most once per fallback per
    /// visit, and record `StageRuntimeRotated` before any provider effect of
    /// the next attempt. `false` means nothing fits and the caller parks.
    async fn rotate(
        &self,
        stage: &StageCapacity<'_>,
        writer: &RunWriter,
        current: &CanonicalRuntimeId,
        reason: &str,
    ) -> bool {
        if self.fallback_agents.is_empty() || !stage.ownership.allows_runtime_overrides() {
            return false;
        }
        if stage.rotations_this_visit as usize > self.fallback_agents.len() {
            tracing::warn!(target: "engine::capacity", node = %stage.node,
                moves = stage.rotations_this_visit,
                "every fallback agent was tried this visit; parking");
            return false;
        }
        let registry = run_agent_registry_or_builtin(self.agent_registry.as_deref());
        let now = chrono::Utc::now();
        let mut candidates = Vec::new();
        for id in stage.own_runtime.iter().chain(self.fallback_agents.iter()) {
            let canonical = self.canonical(id);
            let runnable = registry
                .find_normalized(id)
                .is_some_and(|entry| surge_acp::agent_env::resolve(&entry.id, &entry.env).is_ok());
            let fresh = match self.ledger.status(&canonical).await {
                Ok(status) => exhausted_reason(&status, now).is_none(),
                Err(_) => false,
            };
            candidates.push(surge_core::agent_rotation::Candidate {
                id: id.clone(),
                canonical: canonical.into_string(),
                available: runnable && fresh,
            });
        }
        let Some(choice) =
            surge_core::agent_rotation::choose(&candidates, current.as_str(), stage.partners)
        else {
            tracing::info!(target: "engine::capacity", node = %stage.node, runtime = %current,
                "no fallback agent has capacity; parking");
            return false;
        };
        if choice.same_as_partner {
            tracing::warn!(target: "engine::capacity", node = %stage.node, to = %choice.to,
                "only available fallback agent matches the stage's verifier/implementer partner");
        }
        let event = EventPayload::StageRuntimeRotated {
            node: stage.node.clone(),
            from: current.as_str().to_owned(),
            to: choice.to.clone(),
            reason: reason.to_owned(),
            same_as_partner: choice.same_as_partner,
        };
        if let Err(error) = writer.append_event(VersionedEventPayload::new(event)).await {
            tracing::warn!(target: "engine::capacity", node = %stage.node, %error,
                "could not record the agent rotation; parking instead");
            return false;
        }
        tracing::info!(target: "engine::capacity", node = %stage.node, from = %current,
            to = %choice.to, "usage limit exhausted; moving the stage to a fallback agent");
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    use async_trait::async_trait;
    use surge_core::capacity::{CapacityWindow, WorkEstimate};
    use surge_persistence::runs::Storage;
    use surge_persistence::runs::seq::EventSeq;

    use crate::engine::capacity::PersistentCapacityLedger;

    struct NoEstimate;

    #[async_trait]
    impl WorkEstimator for NoEstimate {
        async fn estimate(&self, _node: &NodeKey) -> Option<WorkEstimate> {
            None
        }
    }

    /// A run registry of launchable custom agents (no required env).
    fn agents(ids: &[&str]) -> Arc<surge_acp::Registry> {
        Arc::new(surge_acp::Registry::from_config(
            ids.iter()
                .map(|id| {
                    let config = serde_json::from_value(
                        serde_json::json!({"command": format!("{id}-bin"), "args": []}),
                    )
                    .unwrap();
                    ((*id).to_string(), config)
                })
                .collect::<HashMap<_, _>>(),
        ))
    }

    fn policy(blind_backoff: Option<Duration>) -> CapacityPolicy {
        CapacityPolicy {
            blind_backoff,
            rotation: RotationPolicy::Disabled,
            jitter_max: Duration::ZERO,
        }
    }

    struct Fixture {
        _home: crate::runtime_home_fixture::FixtureHome,
        storage: Arc<Storage>,
        writer: RunWriter,
        node: NodeKey,
    }

    impl Fixture {
        async fn new() -> Self {
            let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
            let storage = Storage::open(home.path()).await.unwrap();
            let writer = storage
                .create_run(surge_core::RunId::new(), home.path(), None)
                .await
                .unwrap();
            Self {
                _home: home,
                storage,
                writer,
                node: NodeKey::try_from("impl").unwrap(),
            }
        }

        fn capacity(
            &self,
            blind_backoff: Option<Duration>,
            fallback: &[&str],
            registry: Arc<surge_acp::Registry>,
            resumed_from_park: bool,
        ) -> RunCapacity {
            RunCapacity::new(
                Arc::new(PersistentCapacityLedger::new(self.storage.clone())),
                Arc::new(NoEstimate),
                policy(blind_backoff),
                fallback.iter().map(|id| (*id).to_string()).collect(),
                Some(registry),
                resumed_from_park,
            )
        }

        fn stage<'a>(
            &'a self,
            runtime: &str,
            rotations_this_visit: u32,
            partners: &'a [String],
            ownership: StageOwnership,
        ) -> StageCapacity<'a> {
            StageCapacity {
                node: &self.node,
                runtime: Some(CanonicalRuntimeId::recorded(runtime)),
                own_runtime: Some(runtime.to_string()),
                rotations_this_visit,
                partners,
                ownership,
            }
        }

        async fn exhaust(&self, runtime: &str) {
            self.storage
                .observe_capacity(&CapacityWindow::observed_429(
                    runtime,
                    Some(Duration::from_secs(3600)),
                    chrono::Utc::now(),
                ))
                .await
                .unwrap();
        }

        async fn rotations(&self) -> Vec<(String, String)> {
            let last = self.writer.current_seq().await.unwrap();
            self.writer
                .read_events(EventSeq(0)..last.next())
                .await
                .unwrap()
                .into_iter()
                .filter_map(|event| match event.payload.payload {
                    EventPayload::StageRuntimeRotated { from, to, .. } => Some((from, to)),
                    _ => None,
                })
                .collect()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_429_without_a_fallback_parks_on_the_configured_backoff_and_records_it() {
        let fx = Fixture::new().await;
        let capacity = fx.capacity(
            Some(Duration::from_secs(77)),
            &[],
            agents(&["alpha"]),
            false,
        );
        let partners = Vec::new();
        let stage = fx.stage("alpha", 0, &partners, StageOwnership::Ordinary);
        let before = chrono::Utc::now();

        let AfterRateLimit::Park(park) = capacity
            .after_rate_limit(&stage, &fx.writer, "alpha", None, "usage limit", false)
            .await
        else {
            panic!("expected Park");
        };
        assert_eq!(park.basis, WakeBasis::PolicyBackoff);
        assert!(park.wake_at >= before + chrono::Duration::seconds(77));
        assert_eq!(park.runtime.as_deref(), Some("alpha"));
        assert_eq!(park.details.as_deref(), Some("usage limit"));
        assert!(
            matches!(fx.storage.capacity_status("alpha").await.unwrap(),
                CapacityStatus::Known(window) if window.is_exhausted()),
            "the 429 is observed into the ledger before the decision"
        );
        assert_eq!(fx.rotations().await, Vec::<(String, String)>::new());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_429_with_retry_after_parks_until_the_observed_reset() {
        let fx = Fixture::new().await;
        let capacity = fx.capacity(
            Some(Duration::from_secs(77)),
            &[],
            agents(&["alpha"]),
            false,
        );
        let partners = Vec::new();
        let stage = fx.stage("alpha", 0, &partners, StageOwnership::Ordinary);

        let outcome = capacity
            .after_rate_limit(
                &stage,
                &fx.writer,
                "alpha",
                Some(Duration::from_secs(600)),
                "slow down",
                false,
            )
            .await;
        assert!(
            matches!(outcome, AfterRateLimit::Park(ref park) if park.basis == WakeBasis::ObservedReset),
            "got {outcome:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn removing_blind_backoff_surfaces_the_429_unchanged() {
        let fx = Fixture::new().await;
        let capacity = fx.capacity(None, &[], agents(&["alpha"]), false);
        let partners = Vec::new();
        let stage = fx.stage("alpha", 0, &partners, StageOwnership::Ordinary);

        let outcome = capacity
            .after_rate_limit(&stage, &fx.writer, "alpha", None, "usage limit", false)
            .await;
        assert!(
            matches!(outcome, AfterRateLimit::Surface),
            "got {outcome:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_429_moves_the_stage_to_a_fallback_with_capacity_exactly_once() {
        let fx = Fixture::new().await;
        let capacity = fx.capacity(
            Some(Duration::from_secs(77)),
            &["beta"],
            agents(&["alpha", "beta"]),
            false,
        );
        let partners = Vec::new();
        let stage = fx.stage("alpha", 0, &partners, StageOwnership::Ordinary);

        let outcome = capacity
            .after_rate_limit(&stage, &fx.writer, "alpha", None, "usage limit", false)
            .await;
        assert!(
            matches!(outcome, AfterRateLimit::Rotated { .. }),
            "got {outcome:?}"
        );
        assert_eq!(
            fx.rotations().await,
            vec![("alpha".to_string(), "beta".to_string())]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stage_that_tried_every_fallback_this_visit_parks() {
        let fx = Fixture::new().await;
        let capacity = fx.capacity(
            Some(Duration::from_secs(77)),
            &["beta"],
            agents(&["alpha", "beta"]),
            false,
        );
        let partners = Vec::new();
        let stage = fx.stage("alpha", 2, &partners, StageOwnership::Ordinary);

        let outcome = capacity
            .after_rate_limit(&stage, &fx.writer, "alpha", None, "usage limit", false)
            .await;
        assert!(
            matches!(outcome, AfterRateLimit::Park(_)),
            "got {outcome:?}"
        );
        assert_eq!(fx.rotations().await, Vec::<(String, String)>::new());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exhausted_or_unlaunchable_fallbacks_are_never_chosen() {
        let fx = Fixture::new().await;
        fx.exhaust("beta").await;
        let capacity = fx.capacity(
            Some(Duration::from_secs(77)),
            &["beta", "not-in-registry"],
            agents(&["alpha", "beta"]),
            false,
        );
        let partners = Vec::new();
        let stage = fx.stage("alpha", 0, &partners, StageOwnership::Ordinary);

        let outcome = capacity
            .after_rate_limit(&stage, &fx.writer, "alpha", None, "usage limit", false)
            .await;
        assert!(
            matches!(outcome, AfterRateLimit::Park(_)),
            "got {outcome:?}"
        );
        assert_eq!(fx.rotations().await, Vec::<(String, String)>::new());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_task_owned_stage_never_rotates_and_leaves_the_429_to_the_task_cycle() {
        let fx = Fixture::new().await;
        let capacity = fx.capacity(
            Some(Duration::from_secs(77)),
            &["beta"],
            agents(&["alpha", "beta"]),
            false,
        );
        let partners = Vec::new();
        for ownership in [
            StageOwnership::FrozenPlan,
            StageOwnership::HostPlannedRoutes,
        ] {
            let stage = fx.stage("alpha", 0, &partners, ownership);
            let outcome = capacity
                .after_rate_limit(&stage, &fx.writer, "alpha", None, "usage limit", false)
                .await;
            assert!(
                matches!(outcome, AfterRateLimit::Surface),
                "{ownership:?}: {outcome:?}"
            );
        }
        let stage = fx.stage("alpha", 0, &partners, StageOwnership::Ordinary);
        let outcome = capacity
            .after_rate_limit(&stage, &fx.writer, "alpha", None, "usage limit", true)
            .await;
        assert!(
            matches!(outcome, AfterRateLimit::Surface),
            "a requested task suspension owns the 429: {outcome:?}"
        );
        assert_eq!(fx.rotations().await, Vec::<(String, String)>::new());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_planned_routes_skip_the_precheck_but_a_frozen_runtime_plan_does_not() {
        let fx = Fixture::new().await;
        fx.exhaust("alpha").await;
        let capacity = fx.capacity(
            Some(Duration::from_secs(77)),
            &[],
            agents(&["alpha"]),
            false,
        );
        let partners = Vec::new();

        let planned = fx.stage("alpha", 0, &partners, StageOwnership::HostPlannedRoutes);
        assert!(matches!(
            capacity.before_dispatch(&planned, &fx.writer).await,
            BeforeDispatch::Dispatch
        ));
        let frozen = fx.stage("alpha", 0, &partners, StageOwnership::FrozenPlan);
        assert!(matches!(
            capacity.before_dispatch(&frozen, &fx.writer).await,
            BeforeDispatch::Park(_)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_resumed_parked_run_dispatches_once_past_the_precheck() {
        let fx = Fixture::new().await;
        fx.exhaust("alpha").await;
        let capacity = fx.capacity(Some(Duration::from_secs(77)), &[], agents(&["alpha"]), true);
        let partners = Vec::new();
        let stage = fx.stage("alpha", 0, &partners, StageOwnership::Ordinary);

        assert!(matches!(
            capacity.before_dispatch(&stage, &fx.writer).await,
            BeforeDispatch::Dispatch
        ));
        assert!(
            matches!(
                capacity.before_dispatch(&stage, &fx.writer).await,
                BeforeDispatch::Park(_)
            ),
            "the bypass fires once"
        );
    }

    #[test]
    fn ownership_follows_the_frozen_plan() {
        use surge_core::ContentHash;
        use surge_persistence::work_items::recovery_cycles::{
            AccountEvidence, FrozenQuotaCandidate, QuotaRoutingMode, RecoveryCandidate,
        };
        let stage = FrozenQuotaStage::new(
            NodeKey::try_from("impl").unwrap(),
            QuotaRoutingMode::Disabled,
            vec![
                FrozenQuotaCandidate::new(
                    RecoveryCandidate::new("alpha".into(), AccountEvidence::Unknown).unwrap(),
                    None,
                    ContentHash::compute(b"alpha"),
                )
                .unwrap(),
            ],
            1000,
            1000,
        )
        .unwrap();
        assert_eq!(StageOwnership::of(None, true), StageOwnership::Ordinary);
        assert_eq!(
            StageOwnership::of(Some(&stage), true),
            StageOwnership::FrozenPlan
        );
        assert_eq!(
            StageOwnership::of(Some(&stage), false),
            StageOwnership::FrozenPlan
        );
        assert!(StageOwnership::Ordinary.allows_runtime_overrides());
        assert!(!StageOwnership::FrozenPlan.allows_runtime_overrides());
    }
}
