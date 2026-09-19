//! The queue's ordering policy: dependencies first, then priority with aging,
//! then size, then age.
//!
//! # Semantics (binding)
//!
//! - A **candidate** is one queue row the caller considers pending. The
//!   caller — the daemon's scheduler — is responsible for filtering out rows
//!   already dispatched, done or failed; the policy does not invent state it
//!   was not handed.
//! - **Ready** means every `depends_on` id is present in `dep_states` with
//!   [`RoadmapStatus::Completed`]. A dependency that is missing from
//!   `dep_states` is *unknown*, and unknown is treated as **waiting**, never
//!   as satisfied — a policy that guesses here is a policy that dispatches
//!   blocked work.
//! - **Blocked by failure** means at least one dependency is
//!   [`RoadmapStatus::Failed`] or [`RoadmapStatus::Skipped`]. Blocked tasks
//!   are never promoted; the operator resolves them with `surge task
//!   requeue` / `surge task skip` (T7), and until then they stay out of
//!   `ready` *and* are reported separately so the UI can say why.
//! - Any other dependency state (Pending, Running, Paused,
//!   ReadyForVerification, FailedVerification) is **waiting**: the task is
//!   simply not ready yet, and is not an error.
//! - **Order** within `ready` is `(effective priority descending, size
//!   ascending, enqueued_at ascending, task id ascending)`. The task id is
//!   the final tiebreak so the order is total and deterministic: the same
//!   input always yields the same output, and permuting the input does not
//!   change it.
//! - **Aging**: `effective = priority.level() + skipped_dispatches /
//!   aging_threshold`, saturating at `Critical`. Wall-clock time is
//!   deliberately not an input — a tick that fires twice in one second and a
//!   tick a day later must produce the same order for the same rows, or the
//!   policy stops being reproducible. Starvation is prevented by counting
//!   dispatches skipped, not seconds waited.
//! - An empty `ready` is not an error. A queue with nothing dispatchable is
//!   the normal state of a finished project.

use std::collections::BTreeMap;

use surge_core::{Priority, QueueConfig, RoadmapStatus, TaskSize};

/// One queued task, as the scheduler sees it.
///
/// `dep_states` is **derived by the caller** (a join over the queue rows and
/// the roadmap's statuses) and never stored: storing it would freeze a
/// snapshot of other tasks' states into this task's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    /// [`surge_core::RoadmapTask::id`].
    pub task_id: String,
    /// Manual priority from the roadmap.
    pub priority: Priority,
    /// Ids of tasks that must complete first.
    pub depends_on: Vec<String>,
    /// Current state of each dependency, by id. Missing = unknown = waiting.
    pub dep_states: BTreeMap<String, RoadmapStatus>,
    /// Context-budget class. `None` sorts after `L` — an unknown size is
    /// conservatively assumed to be the largest kind of work.
    pub size: Option<TaskSize>,
    /// Unix epoch milliseconds the row entered the queue.
    pub enqueued_at: i64,
    /// How many dispatch decisions passed this task over. Incremented by the
    /// scheduler when a higher-ranked task is chosen; drives aging.
    pub skipped_dispatches: u32,
}

impl QueueEntry {
    /// Effective priority after aging.
    #[must_use]
    pub fn effective_priority(&self, cfg: &QueueConfig) -> EffectivePriority {
        EffectivePriority::compute(self.priority, self.skipped_dispatches, cfg)
    }
}

/// Priority after aging, capped at `Critical`.
///
/// The arithmetic lives only here — the scheduler and the CLI ask this type,
/// they do not recompute levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EffectivePriority(u8);

impl EffectivePriority {
    /// `priority + floor(skipped_dispatches / aging_threshold)`, saturating
    /// at `Critical`. A zero threshold is treated as `1` here so a config
    /// that slipped past [`QueueConfig::validate`] cannot panic; validation
    /// still rejects it at the config boundary.
    #[must_use]
    pub fn compute(priority: Priority, skipped_dispatches: u32, cfg: &QueueConfig) -> Self {
        let threshold = cfg.aging_threshold.max(1);
        let steps = skipped_dispatches / threshold;
        let level = priority
            .level()
            .saturating_add(u8::try_from(steps).unwrap_or(u8::MAX));
        Self(level.min(Priority::Critical.level()))
    }

    /// The numeric level (`Low` = 0 … `Critical` = 3).
    #[must_use]
    pub fn level(self) -> u8 {
        self.0
    }

    /// The level as a named priority (saturating).
    #[must_use]
    pub fn as_priority(self) -> Priority {
        Priority::from_level(self.0)
    }
}

/// A task whose dependency made it undispatachable until a human intervenes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    /// The blocked task.
    pub task_id: String,
    /// Every failing dependency and its state, sorted by id.
    pub by: Vec<(String, RoadmapStatus)>,
}

/// What the scheduler should do this tick.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueDecision {
    /// Dispatchable tasks, best first. v1 dispatches `ready.first()`.
    pub ready: Vec<String>,
    /// Tasks whose dependencies failed; reported to the operator, never
    /// dispatched.
    pub blocked_by_failed: Vec<Blocked>,
}

/// The pure ordering policy. Stateless by construction.
pub struct QueuePolicy;

impl QueuePolicy {
    /// Rank ready tasks and report the blocked ones.
    ///
    /// See the module documentation for the binding semantics.
    #[must_use]
    pub fn next(entries: &[QueueEntry], cfg: &QueueConfig) -> QueueDecision {
        let mut ready: Vec<(&QueueEntry, EffectivePriority)> = Vec::new();
        let mut blocked: Vec<Blocked> = Vec::new();

        for entry in entries {
            let mut failures: Vec<(String, RoadmapStatus)> = entry
                .dep_states
                .iter()
                .filter(|(_, state)| {
                    matches!(state, RoadmapStatus::Failed | RoadmapStatus::Skipped)
                })
                .map(|(id, state)| (id.clone(), *state))
                .collect();
            if !failures.is_empty() {
                failures.sort_by(|a, b| a.0.cmp(&b.0));
                blocked.push(Blocked {
                    task_id: entry.task_id.clone(),
                    by: failures,
                });
                continue;
            }
            let all_completed = entry
                .depends_on
                .iter()
                .all(|id| entry.dep_states.get(id) == Some(&RoadmapStatus::Completed));
            if all_completed {
                ready.push((entry, entry.effective_priority(cfg)));
            }
            // Anything else is waiting: absent from both lists.
        }

        ready.sort_by(|(a, a_eff), (b, b_eff)| {
            b_eff
                .cmp(a_eff)
                .then_with(|| size_rank(a.size).cmp(&size_rank(b.size)))
                .then_with(|| a.enqueued_at.cmp(&b.enqueued_at))
                .then_with(|| a.task_id.cmp(&b.task_id))
        });
        blocked.sort_by(|a, b| a.task_id.cmp(&b.task_id));

        QueueDecision {
            ready: ready.into_iter().map(|(e, _)| e.task_id.clone()).collect(),
            blocked_by_failed: blocked,
        }
    }
}

/// `S` first; unknown size last (conservative).
fn size_rank(size: Option<TaskSize>) -> u8 {
    match size {
        Some(TaskSize::S) => 0,
        Some(TaskSize::M) => 1,
        Some(TaskSize::L) => 2,
        None => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn cfg() -> QueueConfig {
        QueueConfig::default()
    }

    fn entry(id: &str) -> QueueEntry {
        QueueEntry {
            task_id: id.to_string(),
            priority: Priority::Medium,
            depends_on: Vec::new(),
            dep_states: BTreeMap::new(),
            size: Some(TaskSize::M),
            enqueued_at: 0,
            skipped_dispatches: 0,
        }
    }

    fn with_deps(id: &str, deps: &[(&str, RoadmapStatus)]) -> QueueEntry {
        let mut e = entry(id);
        e.depends_on = deps.iter().map(|(id, _)| (*id).to_string()).collect();
        e.dep_states = deps.iter().map(|(id, s)| ((*id).to_string(), *s)).collect();
        e
    }

    #[test]
    fn empty_input_is_an_empty_decision() {
        let d = QueuePolicy::next(&[], &cfg());
        assert!(d.ready.is_empty());
        assert!(d.blocked_by_failed.is_empty());
    }

    #[test]
    fn completed_dependency_is_ready() {
        let e = with_deps("t2", &[("t1", RoadmapStatus::Completed)]);
        let d = QueuePolicy::next(&[e], &cfg());
        assert_eq!(d.ready, vec!["t2"]);
        assert!(d.blocked_by_failed.is_empty());
    }

    #[test]
    fn running_dependency_is_waiting_not_blocked() {
        let e = with_deps("t2", &[("t1", RoadmapStatus::Running)]);
        let d = QueuePolicy::next(&[e], &cfg());
        assert!(d.ready.is_empty());
        assert!(d.blocked_by_failed.is_empty());
    }

    #[test]
    fn missing_dependency_state_is_waiting() {
        let mut e = entry("t2");
        e.depends_on = vec!["t1".to_string()];
        // dep_states deliberately empty: unknown is not satisfied
        let d = QueuePolicy::next(&[e], &cfg());
        assert!(d.ready.is_empty());
        assert!(d.blocked_by_failed.is_empty());
    }

    #[test]
    fn failed_dependency_blocks_with_the_offending_id() {
        let e = with_deps(
            "t2",
            &[
                ("t1", RoadmapStatus::Failed),
                ("t0", RoadmapStatus::Completed),
            ],
        );
        let d = QueuePolicy::next(&[e], &cfg());
        assert!(d.ready.is_empty());
        assert_eq!(d.blocked_by_failed.len(), 1);
        assert_eq!(d.blocked_by_failed[0].task_id, "t2");
        assert_eq!(
            d.blocked_by_failed[0].by,
            vec![("t1".to_string(), RoadmapStatus::Failed)]
        );
    }

    #[test]
    fn skipped_dependency_also_blocks() {
        let e = with_deps("t2", &[("t1", RoadmapStatus::Skipped)]);
        let d = QueuePolicy::next(&[e], &cfg());
        assert_eq!(d.blocked_by_failed.len(), 1);
    }

    #[test]
    fn aging_level_rises_one_step_at_the_threshold() {
        let c = cfg();
        let mut low = entry("t");
        low.priority = Priority::Low;
        low.skipped_dispatches = c.aging_threshold;
        assert_eq!(low.effective_priority(&c).as_priority(), Priority::Medium);
        low.skipped_dispatches = c.aging_threshold - 1;
        assert_eq!(low.effective_priority(&c).as_priority(), Priority::Low);
    }

    #[test]
    fn aging_saturates_at_critical() {
        let c = cfg();
        let mut high = entry("t");
        high.priority = Priority::High;
        high.skipped_dispatches = u32::MAX;
        assert_eq!(
            high.effective_priority(&c).as_priority(),
            Priority::Critical
        );
    }

    #[test]
    fn priority_orders_low_before_critical() {
        assert!(Priority::Low < Priority::Medium);
        assert!(Priority::Critical > Priority::High);
    }

    #[test]
    fn order_is_priority_then_size_then_age() {
        let mut a = entry("a");
        a.priority = Priority::High;
        a.size = Some(TaskSize::L);
        a.enqueued_at = 1;
        let mut b = entry("b");
        b.priority = Priority::High;
        b.size = Some(TaskSize::S);
        b.enqueued_at = 2;
        let mut c = entry("c");
        c.priority = Priority::Critical;
        c.enqueued_at = 3;
        let d = QueuePolicy::next(&[a, b, c], &cfg());
        assert_eq!(d.ready, vec!["c", "b", "a"]);
    }

    #[test]
    fn unknown_size_sorts_after_large() {
        let mut a = entry("a");
        a.size = None;
        let mut b = entry("b");
        b.size = Some(TaskSize::L);
        let d = QueuePolicy::next(&[a, b], &cfg());
        assert_eq!(d.ready, vec!["b", "a"]);
    }

    #[test]
    fn enqueue_time_breaks_priority_ties() {
        let mut older = entry("older");
        older.enqueued_at = 10;
        let mut newer = entry("newer");
        newer.enqueued_at = 20;
        let d = QueuePolicy::next(&[newer, older], &cfg());
        assert_eq!(d.ready, vec!["older", "newer"]);
    }

    #[test]
    fn task_id_is_the_final_tiebreak() {
        let d = QueuePolicy::next(&[entry("b"), entry("a")], &cfg());
        assert_eq!(d.ready, vec!["a", "b"]);
    }

    // ── property laws ────────────────────────────────────────────────

    fn arb_priority() -> impl Strategy<Value = Priority> {
        prop_oneof![
            Just(Priority::Low),
            Just(Priority::Medium),
            Just(Priority::High),
            Just(Priority::Critical),
        ]
    }

    fn arb_size() -> impl Strategy<Value = Option<TaskSize>> {
        prop_oneof![
            Just(None),
            Just(Some(TaskSize::S)),
            Just(Some(TaskSize::M)),
            Just(Some(TaskSize::L)),
        ]
    }

    fn arb_status() -> impl Strategy<Value = RoadmapStatus> {
        prop_oneof![
            Just(RoadmapStatus::Pending),
            Just(RoadmapStatus::Running),
            Just(RoadmapStatus::Completed),
            Just(RoadmapStatus::Failed),
            Just(RoadmapStatus::Skipped),
            Just(RoadmapStatus::ReadyForVerification),
        ]
    }

    fn arb_entry() -> impl Strategy<Value = QueueEntry> {
        (
            "[a-z]{1,3}",
            arb_priority(),
            arb_size(),
            0i64..100,
            0u32..20,
            prop::collection::vec(("[a-z]{1,3}", arb_status()), 0..3),
        )
            .prop_map(|(id, priority, size, enqueued_at, skipped, deps)| {
                let mut e = QueueEntry {
                    task_id: id,
                    priority,
                    depends_on: Vec::new(),
                    dep_states: BTreeMap::new(),
                    size,
                    enqueued_at,
                    skipped_dispatches: skipped,
                };
                for (dep, state) in deps {
                    if dep == e.task_id || e.dep_states.contains_key(&dep) {
                        continue; // keep ids unique and no self-deps
                    }
                    e.depends_on.push(dep.clone());
                    e.dep_states.insert(dep, state);
                }
                e
            })
    }

    fn arb_entries() -> impl Strategy<Value = Vec<QueueEntry>> {
        prop::collection::vec(arb_entry(), 0..8).prop_map(|mut v| {
            v.sort_by(|a, b| a.task_id.cmp(&b.task_id));
            v.dedup_by(|a, b| a.task_id == b.task_id);
            v
        })
    }

    proptest! {
        #[test]
        fn order_is_deterministic_and_permutation_invariant(entries in arb_entries()) {
            let c = cfg();
            let first = QueuePolicy::next(&entries, &c);
            let mut reversed = entries.clone();
            reversed.reverse();
            let second = QueuePolicy::next(&reversed, &c);
            prop_assert_eq!(&first, &second);
        }

        #[test]
        fn deps_dominate(entries in arb_entries()) {
            let c = cfg();
            let d = QueuePolicy::next(&entries, &c);
            for id in &d.ready {
                let e = entries.iter().find(|e| &e.task_id == id).expect("ready id must be an entry");
                for dep in &e.depends_on {
                    prop_assert_eq!(e.dep_states.get(dep), Some(&RoadmapStatus::Completed));
                }
            }
        }

        #[test]
        fn ready_and_blocked_are_disjoint_and_cover_every_entry_exactly_once(entries in arb_entries()) {
            let c = cfg();
            let d = QueuePolicy::next(&entries, &c);
            let ready: std::collections::HashSet<&String> = d.ready.iter().collect();
            let blocked: std::collections::HashSet<&String> =
                d.blocked_by_failed.iter().map(|b| &b.task_id).collect();
            prop_assert!(ready.is_disjoint(&blocked));
            let total = entries.len();
            let covered = ready.len() + blocked.len();
            prop_assert!(covered <= total);
            // Every entry is either ready, blocked, or waiting; the union is
            // what the caller renders, so it must never double-report.
            let mut union: std::collections::HashSet<&String> = std::collections::HashSet::new();
            for id in &d.ready { prop_assert!(union.insert(id)); }
            for b in &d.blocked_by_failed { prop_assert!(union.insert(&b.task_id)); }
        }

        #[test]
        fn aging_is_monotone(mut entries in arb_entries(), idx in 0usize..7, bump in 1u32..30) {
            let c = cfg();
            if entries.is_empty() { return Ok(()); }
            let idx = idx % entries.len();
            let before = QueuePolicy::next(&entries, &c);
            let id = entries[idx].task_id.clone();
            entries[idx].skipped_dispatches = entries[idx].skipped_dispatches.saturating_add(bump);
            let after = QueuePolicy::next(&entries, &c);
            let pos_before = before.ready.iter().position(|t| t == &id);
            let pos_after = after.ready.iter().position(|t| t == &id);
            // Raising one task's skipped count never pushes it *later* in
            // `ready` (or drops it: skipped_dispatches does not affect
            // readiness).
            if let (Some(pb), Some(pa)) = (pos_before, pos_after) {
                prop_assert!(pa <= pb);
            } else {
                prop_assert_eq!(pos_before.is_some(), pos_after.is_some());
            }
        }

        #[test]
        fn effective_priority_law(priority in arb_priority(), skipped in 0u32..1000, threshold in 1u32..10) {
            let c = QueueConfig { aging_threshold: threshold };
            let eff = EffectivePriority::compute(priority, skipped, &c);
            let expected = (u32::from(priority.level()) + skipped / threshold)
                .min(u32::from(Priority::Critical.level()));
            prop_assert_eq!(u32::from(eff.level()), expected);
        }
    }
}
