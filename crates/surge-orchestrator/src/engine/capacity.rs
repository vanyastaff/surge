//! Ports the run-task loop calls to make Task 12's capacity-aware dispatch
//! decision (`surge_core::capacity::CapacityPolicy::decide`, R37/R37.1/
//! R38/R38.1) without `surge-core` taking on any I/O.
//!
//! `surge-core` defines the pure policy and data model; this module (the
//! consumer, per the plan's "consumer owns the port" split) defines the
//! trait objects `RunTaskParams` carries and the concrete implementations
//! wrapping `surge-persistence` (the durable ledger, Task 12 M2) and a
//! per-run event-log reader (the work estimator).
//!
//! # `CanonicalRuntimeId` — Task 12 M3 acceptance criterion A
//!
//! `surge-persistence::runs::capacity::observe`/`status` hold their
//! normalization contract in prose alone: "the caller is expected to have
//! already normalized before it gets here" (see that module's doc). Prose
//! is not enough — the M2 review named this the exact failure direction
//! Task 12's own risk table rules out (an under-park sends work into an
//! exhausted runtime; over-parking is the safe side). [`CanonicalRuntimeId`]
//! closes it in the type system instead: the only way to construct one is
//! [`CanonicalRuntimeId::resolve`], which performs the one normalization
//! this crate already trusts (`surge_acp::Registry::normalize_agent_id`,
//! with the same raw-id fallback `StageError::RateLimited.runtime` and
//! `SessionOpened.agent_id` already use — see
//! `engine::stage::agent::canonical_runtime_id_for`, which every one of
//! those three call sites now shares).
//!
//! **Precisely, not absolutely**: this closes the ledger *port*
//! ([`CapacityLedger::observe`] and [`CapacityLedger::status`] both take
//! `&CanonicalRuntimeId`, never a bare `&str`), which is what every real
//! caller in this crate goes through. It does not — and cannot — reach
//! into `surge-persistence`: `Storage::capacity_status`/`observe_capacity`
//! are themselves `pub`, raw-`&str`-keyed, and in scope for anything in
//! `surge-orchestrator` (including this crate's own `run_task.rs`, which
//! already holds a `Storage` handle for other reasons). Nothing in this
//! delivery calls them directly instead of through this port — but a
//! future caller *could*, and the type system does not stop it. The
//! guarantee is: every call this crate actually makes goes through
//! [`CanonicalRuntimeId::resolve`]; it is not that a raw string can never
//! reach the table by any path through this crate.

use std::sync::Arc;

use async_trait::async_trait;
use surge_core::capacity::{CapacityStatus, CapacityWindow, WorkEstimate};
use surge_core::keys::NodeKey;
use surge_persistence::runs::{RunReader, Storage, StorageError};

/// A canonical agent-runtime registry id — the result of
/// [`surge_acp::Registry::normalize_agent_id`], falling back to the raw
/// `agent_id` when normalization fails despite a profile resolving (the
/// bundled `mock` profile is a real, shipped case of that fallback, not a
/// defensive one — see `engine::stage::agent::canonical_runtime_id_for`'s
/// doc). Constructible only through [`Self::resolve`] — see the module
/// doc's "acceptance criterion A" section for why that is the whole point
/// of this type existing rather than a plain `String`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalRuntimeId(String);

impl CanonicalRuntimeId {
    /// Resolve `raw_agent_id` through `registry`, falling back to the raw
    /// id when the registry does not recognize it. The one function that
    /// performs this normalization for every engine caller — see the
    /// module doc.
    #[must_use]
    pub fn resolve(registry: &surge_acp::Registry, raw_agent_id: &str) -> Self {
        Self(
            registry
                .normalize_agent_id(raw_agent_id)
                .unwrap_or_else(|| raw_agent_id.to_string()),
        )
    }

    /// Borrow the canonical id as a string (e.g. to key
    /// `surge-persistence`'s `runtime_capacity` table, or to log).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume `self`, returning the canonical id — used at the boundary
    /// where an already-typed field (`StageError::RateLimited.runtime`,
    /// `SessionOpened.agent_id`) still stores a plain `String` (Task 12
    /// M0/M1 shape, not reopened here; see `engine::stage::agent`).
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for CanonicalRuntimeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Durable observation of, and read of, a runtime's rate-limit capacity —
/// the seam `surge-orchestrator` owns so `surge_core::capacity::
/// CapacityPolicy` stays I/O-free (Task 12 M3). The concrete
/// [`PersistentCapacityLedger`] wraps `surge-persistence::runs::Storage`
/// (Task 12 M2's durable table).
#[async_trait]
pub trait CapacityLedger: Send + Sync {
    /// Persist a freshly observed capacity window for `runtime`. A write
    /// failure is reported (`Err`) but is not fatal to the caller's own
    /// immediate decision — the caller already holds the same `window` in
    /// memory and can `decide` from it directly; this call is for *other*
    /// runs and *future* dispatches to benefit from the observation, not a
    /// precondition for the current one.
    ///
    /// Takes `runtime: &CanonicalRuntimeId` in addition to `window` (whose
    /// own `CapacityWindow::runtime` is a plain `String` — a `surge-core`
    /// M1 shape this delivery does not reopen). The implementation is
    /// required to key the durable write on `runtime.as_str()` and never on
    /// `window.runtime()`, so the *write* half of this port carries the
    /// same compile-time normalization guarantee as the *read* half
    /// ([`Self::status`]) — see the module doc's "precisely, not
    /// absolutely" note, and [`PersistentCapacityLedger::observe`]'s doc
    /// for how it discharges that requirement.
    async fn observe(
        &self,
        runtime: &CanonicalRuntimeId,
        window: &CapacityWindow,
    ) -> Result<(), StorageError>;

    /// Read the current capacity status for `runtime`. Never writes.
    async fn status(&self, runtime: &CanonicalRuntimeId) -> Result<CapacityStatus, StorageError>;

    /// Clear any durable exhaustion record for `runtime` — called once a
    /// dispatch on it completes without a rate limit (Task 12 M3 review,
    /// BLOCKING #1: without this, a runtime whose reset time was never
    /// learned re-parks on `blind_backoff` forever, because nothing but a
    /// real dispatch attempt can ever refresh the row, and the row itself
    /// is what keeps a real attempt from happening). See
    /// `surge_persistence::runs::capacity::clear`'s doc for the full
    /// argument.
    async fn clear(&self, runtime: &CanonicalRuntimeId) -> Result<(), StorageError>;
}

/// [`CapacityLedger`] backed by the registry-level `runtime_capacity` table
/// (Task 12 M2, `surge_persistence::runs::capacity`), reached through
/// `Storage`'s M3 doors (`observe_capacity`/`capacity_status`).
pub struct PersistentCapacityLedger {
    storage: Arc<Storage>,
}

impl PersistentCapacityLedger {
    /// Wrap the engine's own registry-level `Storage` handle.
    #[must_use]
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl CapacityLedger for PersistentCapacityLedger {
    async fn observe(
        &self,
        runtime: &CanonicalRuntimeId,
        window: &CapacityWindow,
    ) -> Result<(), StorageError> {
        // Rekey under the typed id rather than trusting `window.runtime()`
        // to already agree with it: `CapacityWindow::from_parts` (a public
        // `surge-core` constructor, also used by `surge-persistence` itself
        // to rebuild a window off a row) reassembles every other field
        // as-is but substitutes `runtime.as_str()` for whatever string
        // `window` was built with. `window.runtime()` is never read below —
        // a caller that mismatched `runtime` and `window` still writes
        // (and reads back) under `runtime.as_str()`, not silently under a
        // second, wrong key. This is what makes the doc's "compile-time
        // normalization guarantee" true rather than a `debug_assert!` that
        // only detected the mismatch (and only in debug builds) instead of
        // preventing it.
        let keyed = CapacityWindow::from_parts(
            runtime.as_str(),
            window.window(),
            window.remaining(),
            window.resets_at(),
            window.source(),
        );
        self.storage.observe_capacity(&keyed).await
    }

    async fn status(&self, runtime: &CanonicalRuntimeId) -> Result<CapacityStatus, StorageError> {
        self.storage.capacity_status(runtime.as_str()).await
    }

    async fn clear(&self, runtime: &CanonicalRuntimeId) -> Result<(), StorageError> {
        self.storage.clear_capacity(runtime.as_str()).await
    }
}

/// Learn how long a node's dispatch is expected to take — the seam
/// `CapacityPolicy::decide`'s rule 5 (comparing an estimate against
/// remaining capacity) is prepared to consult, kept separate from
/// `surge-core` so the source of the estimate can change (this milestone's
/// per-run history vs. a future cross-run aggregate) without touching the
/// pure policy at all (Task 12 M3, plan §2).
#[async_trait]
pub trait WorkEstimator: Send + Sync {
    /// Estimate for the next dispatch of `node`, or `None` when there is
    /// no history to estimate from — the common case for a node's first
    /// attempt, and the case [`surge_core::capacity::CapacityPolicy::
    /// decide`]'s rule 4 (acceptance criterion D) depends on never causing
    /// a refusal by itself. A read failure degrades to `None` for the same
    /// reason: "could not learn anything" and "there is nothing to learn"
    /// must not be told apart by blocking dispatch — that would let a
    /// local storage fault masquerade as a provider capacity signal.
    async fn estimate(&self, node: &NodeKey) -> Option<WorkEstimate>;
}

/// [`WorkEstimator`] over completed runs of the same graph archetype.
/// Missing archetype metadata or history yields no estimate, preserving the
/// capacity policy's fail-open behavior for an unobserved workload.
pub struct RunHistoryWorkEstimator {
    reader: RunReader,
    history: Option<Arc<dyn ArchetypeHistory>>,
}

/// Read-only cross-run archetype analytics source.
#[async_trait]
pub trait ArchetypeHistory: Send + Sync {
    /// Completed duration and cost samples for an archetype.
    async fn samples(&self, archetype: surge_core::ArchetypeName) -> Vec<(u64, Option<u64>)>;
}

/// Registry-backed historical archetype samples; per-run read failures are
/// skipped so old or partially-written runs cannot block a new dispatch.
pub struct PersistentArchetypeHistory {
    storage: Arc<surge_persistence::runs::Storage>,
}

impl PersistentArchetypeHistory {
    /// Build from the engine's registry-level storage handle.
    #[must_use]
    pub fn new(storage: Arc<surge_persistence::runs::Storage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl ArchetypeHistory for PersistentArchetypeHistory {
    async fn samples(&self, archetype: surge_core::ArchetypeName) -> Vec<(u64, Option<u64>)> {
        let Ok(runs) = self
            .storage
            .list_runs(surge_persistence::runs::RunFilter {
                status: Some(surge_core::RunStatus::Completed),
                project_path: None,
                limit: Some(1_000),
            })
            .await
        else {
            return Vec::new();
        };
        let mut samples = Vec::new();
        for run in runs {
            let Ok(reader) = self.storage.open_run_reader(run.id).await else {
                continue;
            };
            let Ok(events) = reader.read_run_events().await else {
                continue;
            };
            let matches_archetype = events
                .iter()
                .find_map(|event| match &event.payload {
                    surge_core::run_event::EventPayload::PipelineMaterialized { graph, .. } => {
                        graph
                            .metadata
                            .archetype
                            .as_ref()
                            .map(|meta| meta.name == archetype)
                    },
                    _ => None,
                })
                .unwrap_or(false);
            if !matches_archetype {
                continue;
            }
            let Ok(rows) = reader.stage_executions().await else {
                continue;
            };
            for row in rows {
                if let Some(end) = row.ended_at_ms {
                    let Ok(duration) = u64::try_from(end.saturating_sub(row.started_at_ms)) else {
                        continue;
                    };
                    let cost = (!row.cost_unknown)
                        .then_some(row.known_cost_usd)
                        .flatten()
                        .and_then(usd_to_micros_usd);
                    samples.push((duration, cost));
                }
            }
        }
        samples
    }
}

fn usd_to_micros_usd(cost_usd: f64) -> Option<u64> {
    const MAX_MICROS_USD: f64 = 18_446_744_073_709_551_616.0;
    let micros = (cost_usd * 1_000_000.0).round();
    if !micros.is_finite() || micros < 0.0 || micros >= MAX_MICROS_USD {
        return None;
    }
    // The finite/range checks above make this rounded conversion safe.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "usage is rounded to integer micro-USD after finite nonnegative range checks"
    )]
    Some(micros as u64)
}

impl RunHistoryWorkEstimator {
    /// Build an estimator over `reader`'s own run. Callers typically pass
    /// `RunWriter::reader()` (a cheap clone) so the estimator keeps working
    /// after the writer that produced it is dropped.
    #[must_use]
    pub fn new(reader: RunReader) -> Self {
        Self {
            reader,
            history: None,
        }
    }

    /// Enable cross-run estimates over completed graphs of the same archetype.
    #[must_use]
    pub fn with_history(mut self, history: Arc<dyn ArchetypeHistory>) -> Self {
        self.history = Some(history);
        self
    }
}

#[async_trait]
impl WorkEstimator for RunHistoryWorkEstimator {
    async fn estimate(&self, _node: &NodeKey) -> Option<WorkEstimate> {
        let history = self.history.as_ref()?;
        let events = self.reader.read_run_events().await.ok()?;
        let archetype = events.iter().find_map(|event| match &event.payload {
            surge_core::run_event::EventPayload::PipelineMaterialized { graph, .. } => graph
                .metadata
                .archetype
                .as_ref()
                .map(|metadata| metadata.name),
            _ => None,
        })?;
        let samples = history.samples(archetype).await;
        if samples.is_empty() {
            return None;
        }
        let durations: Vec<u64> = samples.iter().map(|sample| sample.0).collect();
        let median = median_u64(&durations);
        let costs: Vec<u64> = samples.iter().filter_map(|sample| sample.1).collect();
        Some(
            WorkEstimate::new(std::time::Duration::from_millis(median))
                .with_cost_micros_usd((!costs.is_empty()).then(|| median_u64(&costs))),
        )
    }
}

fn median_u64(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        u64::midpoint(sorted[middle - 1], sorted[middle])
    } else {
        sorted[middle]
    }
}

/// Why a runtime cannot take work now (shared by the plan catalog and the
/// desktop provider picker), when its last observed capacity
/// window is exhausted and has not reset yet. `None` = usable.
#[must_use]
pub fn exhausted_reason(
    status: &surge_core::capacity::CapacityStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let surge_core::capacity::CapacityStatus::Known(window) = status else {
        return None;
    };
    let empty = window.remaining().is_none_or(|share| share.get() <= 0.0);
    match window.resets_at() {
        Some(reset) if reset > now && empty => Some(format!(
            "provider usage limit, resets {}",
            reset.format("%Y-%m-%d %H:%M UTC")
        )),
        None if empty => Some("provider usage limit reached".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_runtime_id_collapses_registry_aliases() {
        // Task 12 M2 review, "Decisions" #1: `claude`, `claude-code`, and
        // `claude-acp` must resolve to the identical key through the one
        // engine-side constructor, not three keys that happen to look
        // similar. `unnormalized_aliases_are_not_collapsed_by_this_store_
        // alone` (surge-persistence) proves the *store* does not collapse
        // them; this proves the *engine's* only way to produce a key does.
        let registry = surge_acp::Registry::builtin();
        let a = CanonicalRuntimeId::resolve(&registry, "claude");
        let b = CanonicalRuntimeId::resolve(&registry, "claude-code");
        let c = CanonicalRuntimeId::resolve(&registry, "claude-acp");
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert_eq!(c.as_str(), "claude-acp");
    }

    #[test]
    fn canonical_runtime_id_falls_back_to_the_raw_id_when_unrecognized() {
        // The bundled `mock` profile's `agent_id = "mock"` is a real,
        // shipped case (see `engine::stage::agent`'s doc on this exact
        // fallback) — not discarded into some placeholder identity.
        let registry = surge_acp::Registry::builtin();
        let id = CanonicalRuntimeId::resolve(&registry, "mock");
        assert_eq!(id.as_str(), "mock");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[expect(
        clippy::too_many_lines,
        reason = "one integration fixture exercises graph, session, usage, and completion projections"
    )]
    async fn persistent_archetype_history_aggregates_only_matching_completed_runs() {
        use std::collections::BTreeMap;
        use surge_core::archetype::{ArchetypeMetadata, ArchetypeName};
        use surge_core::graph::{GraphMetadata, SCHEMA_VERSION};
        use surge_core::run_event::{EventPayload, VersionedEventPayload};
        use surge_core::{NodeKey, RunStatus};

        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        for (name, archetype, cost) in [
            ("hist_a", ArchetypeName::Spike, 0.001),
            ("hist_b", ArchetypeName::Spike, 0.009),
            ("hist_c", ArchetypeName::Spike, 0.003),
            ("hist_unknown_cost", ArchetypeName::Spike, f64::NAN),
            ("hist_other", ArchetypeName::BugFix, 0.9),
        ] {
            let id = surge_core::RunId::new();
            let writer = storage.create_run(id, dir.path(), None).await.unwrap();
            let mut graph = surge_core::Graph {
                schema_version: SCHEMA_VERSION,
                metadata: GraphMetadata::new(name, chrono::Utc::now()),
                start: NodeKey::try_from("worker").unwrap(),
                nodes: BTreeMap::new(),
                edges: Vec::new(),
                subgraphs: BTreeMap::new(),
            };
            graph.metadata.archetype = Some(ArchetypeMetadata {
                name: archetype,
                milestones: None,
                edit_loop_cap: None,
                node_capacity_estimate: None,
            });
            writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::PipelineMaterialized {
                        graph: Box::new(graph),
                        graph_hash: surge_core::ContentHash::compute(name.as_bytes()),
                    },
                ))
                .await
                .unwrap();
            writer
                .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
                    node: NodeKey::try_from("worker").unwrap(),
                    attempt: 1,
                }))
                .await
                .unwrap();
            let session = surge_core::SessionId::new();
            writer
                .append_event(VersionedEventPayload::new(EventPayload::SessionOpened {
                    handoff: None,
                    opened: None,
                    node: NodeKey::try_from("worker").unwrap(),
                    session,
                    agent: "test-profile".to_string(),
                    agent_id: Some("test-runtime".to_string()),
                }))
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            writer
                .append_event(VersionedEventPayload::new(EventPayload::TokensConsumed {
                    session,
                    prompt_tokens: 1,
                    output_tokens: 1,
                    cache_hits: 0,
                    model: "test-model".to_string(),
                    cost_usd: cost.is_finite().then_some(cost),
                }))
                .await
                .unwrap();
            writer
                .append_event(VersionedEventPayload::new(EventPayload::StageCompleted {
                    node: NodeKey::try_from("worker").unwrap(),
                    outcome: surge_core::OutcomeKey::try_from("done").unwrap(),
                }))
                .await
                .unwrap();
            writer
                .append_event(VersionedEventPayload::new(EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_from("worker").unwrap(),
                }))
                .await
                .unwrap();
            writer.close().await.unwrap();
            storage
                .set_run_status(&id, RunStatus::Completed, None)
                .await
                .unwrap();
        }
        let history = PersistentArchetypeHistory::new(storage);
        let samples = history.samples(ArchetypeName::Spike).await;
        assert_eq!(samples.len(), 4);
        assert!(samples.iter().all(|sample| sample.0 >= 10));
        assert_eq!(
            samples.iter().filter(|sample| sample.1.is_some()).count(),
            3
        );
        assert_eq!(
            median_u64(&samples.iter().filter_map(|s| s.1).collect::<Vec<_>>()),
            3000
        );
        drop(history);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn observe_writes_under_the_typed_runtime_even_when_window_runtime_disagrees() {
        // Guards `CapacityLedger::observe`'s doc promise directly: a caller
        // that passes a correctly resolved `CanonicalRuntimeId` alongside a
        // `CapacityWindow` built under some *other* string (a bug this
        // trait's own type signature does nothing to stop, since
        // `CapacityWindow::runtime` stays a plain `String`) must still land
        // its row under `runtime.as_str()` — never under `window.runtime()`.
        // Before this delivery that mismatch was only a `debug_assert!`
        // (compiled out in release, and would have aborted this very test
        // in debug); now it is the write path's actual behavior.
        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let ledger = PersistentCapacityLedger::new(storage.clone());

        let registry = surge_acp::Registry::builtin();
        let runtime = CanonicalRuntimeId::resolve(&registry, "claude");
        assert_eq!(runtime.as_str(), "claude-acp");

        // Deliberately mismatched: the window's own `runtime` names a
        // different, made-up string.
        let window = CapacityWindow::observed_429("not-the-canonical-id", None, chrono::Utc::now());

        ledger.observe(&runtime, &window).await.unwrap();

        // Read back under the typed key: must be `Known`, not
        // `NeverObserved` (which is what a wrong-key write would produce).
        let status = ledger.status(&runtime).await.unwrap();
        assert!(
            matches!(status, CapacityStatus::Known(_)),
            "expected Known under the canonical key, got {status:?}"
        );

        // And nothing must have leaked into a second row under the
        // window's own (wrong) string.
        let stray = storage
            .capacity_status("not-the-canonical-id")
            .await
            .unwrap();
        assert_eq!(
            stray,
            CapacityStatus::NeverObserved,
            "window.runtime() must never reach the durable key"
        );
        drop(ledger);
        drop(storage);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_history_estimator_has_no_estimate_for_a_node_with_no_prior_attempts() {
        // Acceptance criterion D's producer half: the common case (a
        // node's first dispatch ever) must read back `None`, not a
        // fabricated zero.
        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let estimator = RunHistoryWorkEstimator::new(writer.reader());
        let node = NodeKey::try_from("impl_1").unwrap();
        assert_eq!(estimator.estimate(&node).await, None);
        writer.close().await.unwrap();
        drop(estimator);
        drop(storage);
        dir.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_history_estimator_does_not_substitute_node_history_for_archetype_history() {
        use surge_core::keys::OutcomeKey;
        use surge_core::run_event::{EventPayload, VersionedEventPayload};
        use surge_persistence::runs::clock::MockClock;

        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let clock = Arc::new(MockClock::new(1_000));
        let storage = Storage::open_with(dir.path(), clock.clone()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = NodeKey::try_from("impl_1").unwrap();
        for (advance_ms, payload) in [
            (
                0,
                EventPayload::StageEntered {
                    node: node.clone(),
                    attempt: 1,
                },
            ),
            (
                10,
                EventPayload::StageCompleted {
                    node: node.clone(),
                    outcome: OutcomeKey::try_from("done").unwrap(),
                },
            ),
            (
                990,
                EventPayload::StageEntered {
                    node: node.clone(),
                    attempt: 2,
                },
            ),
            (
                90,
                EventPayload::StageCompleted {
                    node: node.clone(),
                    outcome: OutcomeKey::try_from("done").unwrap(),
                },
            ),
            (
                910,
                EventPayload::StageEntered {
                    node: node.clone(),
                    attempt: 3,
                },
            ),
            (
                30,
                EventPayload::StageCompleted {
                    node: node.clone(),
                    outcome: OutcomeKey::try_from("done").unwrap(),
                },
            ),
            (
                970,
                EventPayload::StageEntered {
                    node: node.clone(),
                    attempt: 4,
                },
            ),
            (
                200,
                EventPayload::StageCompleted {
                    node: node.clone(),
                    outcome: OutcomeKey::try_from("done").unwrap(),
                },
            ),
        ] {
            clock.advance(advance_ms);
            writer
                .append_event(VersionedEventPayload::new(payload))
                .await
                .unwrap();
        }

        let estimator = RunHistoryWorkEstimator::new(writer.reader());
        assert_eq!(
            estimator.estimate(&node).await,
            None,
            "same-run node history must not masquerade as an archetype estimate"
        );
        writer.close().await.unwrap();
        drop(estimator);
        drop(storage);
        dir.close().unwrap();
    }
}
