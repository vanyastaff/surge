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

/// [`WorkEstimator`] over the *current run's own* `stage_executions`
/// materialized view (`per_run/0001_initial.sql`) — zero new tables, zero
/// new writes on the hot path (plan §2). Averages the wall-clock duration
/// of every *completed* prior attempt of the same node in this run; `None`
/// when none exist yet (a fresh node, or one whose only attempts are still
/// running). A cross-run aggregate is a different `WorkEstimator`
/// implementation behind the same trait, not a change to this one or to
/// `decide` — deliberately not built here (plan §2: there is no
/// "archetype" table to source it from).
///
/// Three ways this diverges from the acceptance criterion it satisfies,
/// named plainly rather than left to be discovered by reading the body:
/// the criterion asked for the *median* of duration and *spend* by
/// *archetype*; this estimator computes the **mean** of **duration only**,
/// over **the same node in the same run**. Spend is not estimated at
/// all — `StageExecution::cost_usd` is read off every row [`Self::estimate`]
/// fetches and then dropped on the floor, because [`WorkEstimate`] itself
/// carries only a duration; no cost signal reaches
/// `CapacityPolicy::decide` through this estimator.
pub struct RunHistoryWorkEstimator {
    reader: RunReader,
}

impl RunHistoryWorkEstimator {
    /// Build an estimator over `reader`'s own run. Callers typically pass
    /// `RunWriter::reader()` (a cheap clone) so the estimator keeps working
    /// after the writer that produced it is dropped.
    #[must_use]
    pub fn new(reader: RunReader) -> Self {
        Self { reader }
    }
}

#[async_trait]
impl WorkEstimator for RunHistoryWorkEstimator {
    async fn estimate(&self, node: &NodeKey) -> Option<WorkEstimate> {
        let executions = match self.reader.stage_executions().await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(
                    target: "engine::capacity",
                    node = %node,
                    %error,
                    "stage_executions read failed; treating as no estimate"
                );
                return None;
            },
        };

        let durations_ms: Vec<u64> = executions
            .iter()
            .filter(|row| &row.node_id == node)
            .filter_map(|row| {
                let ended_at_ms = row.ended_at_ms?;
                u64::try_from(ended_at_ms.saturating_sub(row.started_at_ms)).ok()
            })
            .collect();

        if durations_ms.is_empty() {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a wall-clock duration average in milliseconds is nowhere near f64's \
                      53-bit exact-integer range for any run this crate could produce"
        )]
        let avg_ms = durations_ms.iter().sum::<u64>() as f64 / durations_ms.len() as f64;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "avg_ms is a mean of non-negative u64 values, always >= 0 and within u64::MAX"
        )]
        Some(WorkEstimate::new(std::time::Duration::from_millis(
            avg_ms as u64,
        )))
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
        let dir = tempfile::tempdir().unwrap();
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
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_history_estimator_has_no_estimate_for_a_node_with_no_prior_attempts() {
        // Acceptance criterion D's producer half: the common case (a
        // node's first dispatch ever) must read back `None`, not a
        // fabricated zero.
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let estimator = RunHistoryWorkEstimator::new(writer.reader());
        let node = NodeKey::try_from("impl_1").unwrap();
        assert_eq!(estimator.estimate(&node).await, None);
    }
}
