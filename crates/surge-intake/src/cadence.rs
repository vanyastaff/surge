//! Tier-aware polling cadence controller.
//!
//! Computes the desired poll interval for a tracker source given:
//! - the most aggressive automation tier among the source's active
//!   tickets (L1 = 5min, L2 = 2min, L3 = 1min), and
//! - an exponential-backoff state driven by [`CadenceController::notify_rate_limited`].
//!
//! This module is pure algorithm — it does not own a clock, a task
//! source, or a polling loop. Callers query [`CadenceController::next_interval`]
//! and decide when to schedule the next fetch. This keeps the controller
//! deterministic and easy to test under a frozen `tokio::time` clock.
//!
//! ### Integration
//!
//! Each polling source owns one controller (keyed by its own id). The
//! GitHub source drives it from inside `watch_for_tasks`: it sleeps
//! [`CadenceController::next_interval`] (with [`jitter_seed`]) before each
//! poll, calls [`CadenceController::raise_tier_for`] from the tiers of the
//! issues it just fetched (only ever speeding up — the fetch is an
//! incremental delta, so a replace would wrongly downshift on a quiet poll),
//! [`CadenceController::notify_success`] on a clean fetch, and
//! [`CadenceController::notify_rate_limited`] on a rate-limited one. The
//! Linear source still polls at its fixed configured interval —
//! its fetch path does not yet surface per-issue labels, so tier-aware
//! cadence there is a follow-up.

use std::collections::HashMap;
use std::time::Duration;

use crate::policy::AutomationPolicy;

/// Default tier intervals, in line with ROADMAP `tracker-automation-tiers`
/// (lines 198 / 203). Exposed as `pub const` so the doctor / list CLI
/// can render the values without copy-pasting.
pub mod intervals {
    use super::Duration;

    /// Interval for L1 (`AutomationPolicy::Standard`).
    pub const L1: Duration = Duration::from_secs(5 * 60);
    /// Interval for L2 (`AutomationPolicy::Template { .. }`).
    pub const L2: Duration = Duration::from_secs(2 * 60);
    /// Interval for L3 (`AutomationPolicy::Auto { .. }`).
    pub const L3: Duration = Duration::from_secs(60);
    /// Interval used when no active tickets are tier-tagged. Equal to
    /// L1 so dormant sources stay polite without going silent.
    pub const IDLE: Duration = L1;
    /// Cap on backoff under repeated rate-limit hits.
    pub const BACKOFF_CEILING: Duration = Duration::from_secs(15 * 60);
}

/// Pick the tier interval for one policy. L0 collapses to the idle
/// interval — L0 tickets do not drive polling.
///
/// Same-crate code; when a new [`AutomationPolicy`] variant is added
/// the compiler forces an explicit branch here. External callers see
/// `AutomationPolicy::#[non_exhaustive]`, so they cannot extend this
/// table — that is by design.
#[must_use]
pub fn tier_interval(policy: &AutomationPolicy) -> Duration {
    match policy {
        AutomationPolicy::Standard => intervals::L1,
        AutomationPolicy::Template { .. } => intervals::L2,
        AutomationPolicy::Auto { .. } => intervals::L3,
        AutomationPolicy::Disabled => intervals::IDLE,
    }
}

/// Pick the most aggressive interval across a set of active tickets'
/// policies. Empty input ⇒ [`intervals::IDLE`].
#[must_use]
pub fn most_aggressive(policies: &[AutomationPolicy]) -> Duration {
    policies
        .iter()
        .map(tier_interval)
        .min()
        .unwrap_or(intervals::IDLE)
}

/// Per-source backoff and tier state used by [`CadenceController`].
#[derive(Debug, Clone)]
struct SourceState {
    /// Base tier interval — the most aggressive tier currently in
    /// flight for this source. Updated by [`CadenceController::set_tier_for`].
    base: Duration,
    /// Consecutive `Error::RateLimited` hits since the last successful
    /// fetch. Each hit doubles the wait, capped at
    /// [`intervals::BACKOFF_CEILING`]. Reset to 0 by
    /// [`CadenceController::notify_success`].
    backoff_exp: u32,
}

impl SourceState {
    fn fresh(base: Duration) -> Self {
        Self {
            base,
            backoff_exp: 0,
        }
    }
}

/// Tier-aware polling cadence controller.
///
/// One controller per daemon — it tracks every registered source's
/// current cadence target. Updates are pushed in:
/// - [`CadenceController::set_tier_for`] when the policy of an active
///   ticket changes (or a fresh tier becomes the most aggressive).
/// - [`CadenceController::notify_rate_limited`] when a fetch returns
///   `Error::RateLimited`.
/// - [`CadenceController::notify_success`] when a fetch completes
///   cleanly.
///
/// The controller does not own a clock; callers use the duration
/// returned by [`CadenceController::next_interval`] to schedule the
/// next fetch via `tokio::time::sleep` (or any other timer).
#[derive(Debug, Default, Clone)]
pub struct CadenceController {
    sources: HashMap<String, SourceState>,
}

impl CadenceController {
    /// Empty controller — no sources tracked yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the base interval for `source_id` from the most
    /// aggressive policy among its active tickets. Idempotent —
    /// repeated calls with the same value are no-ops. Replaces the base
    /// outright (can speed up *or* slow down); use this only when
    /// `policies` is the full active set.
    pub fn set_tier_for(&mut self, source_id: &str, policies: &[AutomationPolicy]) {
        let base = most_aggressive(policies);
        let entry = self
            .sources
            .entry(source_id.to_owned())
            .or_insert_with(|| SourceState::fresh(base));
        entry.base = base;
    }

    /// Raise (only ever speed up) the base cadence for `source_id` to the
    /// most aggressive tier among `policies`. Unlike [`set_tier_for`], this
    /// never slows the cadence down.
    ///
    /// This is the safe update when `policies` comes from an *incremental*
    /// poll delta (e.g. issues updated since a watermark): a quiet cycle
    /// yields an empty or low-tier delta, and replacing the base from it
    /// would wrongly downshift a source whose higher-tier issues are still
    /// open. Mirrors the most-aggressive-wins rule reviewers asked for.
    ///
    /// Trade-off: the cadence does not slow back down when a high-tier issue
    /// closes (a delta filtered on open state never reports the close).
    /// Recomputing the base from a full open-issue snapshot is a follow-up.
    pub fn raise_tier_for(&mut self, source_id: &str, policies: &[AutomationPolicy]) {
        let candidate = most_aggressive(policies);
        let entry = self
            .sources
            .entry(source_id.to_owned())
            .or_insert_with(|| SourceState::fresh(candidate));
        // Smaller Duration == faster poll. Only ever go faster.
        if candidate < entry.base {
            entry.base = candidate;
        }
    }

    /// Record a `RateLimited` fetch — bumps the backoff exponent.
    pub fn notify_rate_limited(&mut self, source_id: &str) {
        let entry = self
            .sources
            .entry(source_id.to_owned())
            .or_insert_with(|| SourceState::fresh(intervals::IDLE));
        entry.backoff_exp = entry.backoff_exp.saturating_add(1);
    }

    /// Record a successful fetch — clears any backoff.
    pub fn notify_success(&mut self, source_id: &str) {
        if let Some(entry) = self.sources.get_mut(source_id) {
            entry.backoff_exp = 0;
        }
    }

    /// Read-only view of the current backoff exponent for diagnostics.
    /// Returns `0` for unknown sources.
    #[must_use]
    pub fn backoff_exp(&self, source_id: &str) -> u32 {
        self.sources.get(source_id).map_or(0, |s| s.backoff_exp)
    }

    /// Compute the next poll interval for `source_id`.
    ///
    /// `jitter_unit_interval` is a deterministic seed in `[0.0, 1.0)`
    /// produced by the caller (e.g. a hash of `(source_id, attempt)`
    /// to keep the schedule reproducible in tests). Internally jitter
    /// adds ±10% to the computed interval so multiple sources do not
    /// align on the exact poll edge.
    ///
    /// Backoff curve: `base * 2 ^ backoff_exp`, capped at
    /// [`intervals::BACKOFF_CEILING`].
    #[must_use]
    pub fn next_interval(&self, source_id: &str, jitter_unit_interval: f64) -> Duration {
        let state = match self.sources.get(source_id) {
            Some(s) => s.clone(),
            None => SourceState::fresh(intervals::IDLE),
        };
        let base = state.base;
        let multiplier = 1u32.checked_shl(state.backoff_exp).unwrap_or(u32::MAX);
        let target_secs = base.as_secs().saturating_mul(u64::from(multiplier));
        let target = Duration::from_secs(target_secs).min(intervals::BACKOFF_CEILING);
        apply_jitter(target, jitter_unit_interval)
    }
}

/// Apply a deterministic ±10% jitter to `target` using a unit-interval
/// seed. `seed.clamp(0.0, 1.0)` is mapped to the symmetric range
/// `[-0.1, +0.1]` and applied multiplicatively.
fn apply_jitter(target: Duration, seed: f64) -> Duration {
    let s = seed.clamp(0.0, 1.0);
    let factor_raw = 0.9 + 0.2 * s;
    #[allow(clippy::cast_precision_loss)]
    let scaled = (target.as_secs_f64() * factor_raw).max(0.0);
    Duration::from_secs_f64(scaled)
}

/// Deterministic jitter seed in `[0.0, 1.0)` for poll attempt `n`.
///
/// Uses the golden-ratio low-discrepancy sequence so successive polls of one
/// source spread evenly across the jitter band instead of clustering, while
/// staying fully reproducible (no RNG — replay- and test-friendly). Feed the
/// result to [`CadenceController::next_interval`] as `jitter_unit_interval`.
#[must_use]
pub fn jitter_seed(attempt: u64) -> f64 {
    // Fractional part of n * (1/φ); the classic additive-recurrence sequence.
    const INV_PHI: f64 = 0.618_033_988_749_895;
    #[allow(clippy::cast_precision_loss)]
    let x = (attempt as f64) * INV_PHI;
    x.fract()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto() -> AutomationPolicy {
        AutomationPolicy::Auto {
            merge_when_clean: true,
        }
    }

    fn template(name: &str) -> AutomationPolicy {
        AutomationPolicy::Template {
            name: name.to_owned(),
        }
    }

    #[test]
    fn tier_interval_matches_table() {
        assert_eq!(tier_interval(&AutomationPolicy::Standard), intervals::L1);
        assert_eq!(tier_interval(&template("x")), intervals::L2);
        assert_eq!(tier_interval(&auto()), intervals::L3);
        assert_eq!(tier_interval(&AutomationPolicy::Disabled), intervals::IDLE);
    }

    #[test]
    fn most_aggressive_picks_l3_over_l1() {
        let policies = [AutomationPolicy::Standard, auto(), template("x")];
        assert_eq!(most_aggressive(&policies), intervals::L3);
    }

    #[test]
    fn most_aggressive_empty_is_idle() {
        assert_eq!(most_aggressive(&[]), intervals::IDLE);
    }

    #[test]
    fn next_interval_unknown_source_is_idle_with_no_jitter() {
        let c = CadenceController::new();
        let d = c.next_interval("nope", 0.5);
        // Within 10% of IDLE.
        let target = intervals::IDLE.as_secs_f64();
        let got = d.as_secs_f64();
        assert!(
            (got - target).abs() <= 0.11 * target,
            "{got} not within 10% of {target}"
        );
    }

    #[test]
    fn raise_tier_only_speeds_up_never_down() {
        let mut c = CadenceController::new();
        let l3 = intervals::L3.as_secs_f64();
        let within = |got: f64, target: f64| (got - target).abs() <= 0.11 * target;

        // First sighting of an L3 issue speeds the source to L3.
        c.raise_tier_for("src", &[auto()]);
        assert!(within(c.next_interval("src", 0.5).as_secs_f64(), l3));

        // A quiet poll (empty delta) must NOT downshift back to idle/L1.
        c.raise_tier_for("src", &[]);
        assert!(
            within(c.next_interval("src", 0.5).as_secs_f64(), l3),
            "empty delta downshifted the cadence"
        );

        // A low-tier delta (only L1 issues updated) must NOT downshift either.
        c.raise_tier_for("src", &[AutomationPolicy::Standard]);
        assert!(
            within(c.next_interval("src", 0.5).as_secs_f64(), l3),
            "L1 delta downshifted the cadence"
        );
    }

    #[test]
    fn set_tier_for_picks_most_aggressive() {
        let mut c = CadenceController::new();
        c.set_tier_for("src", &[AutomationPolicy::Standard, auto()]);
        // Backoff_exp=0 ⇒ interval ≈ L3 base ± 10%.
        let d = c.next_interval("src", 0.5);
        let target = intervals::L3.as_secs_f64();
        let got = d.as_secs_f64();
        assert!(
            (got - target).abs() <= 0.11 * target,
            "got {got}s expected ≈ {target}s"
        );
    }

    #[test]
    fn rate_limit_doubles_then_clears_on_success() {
        let mut c = CadenceController::new();
        c.set_tier_for("src", &[AutomationPolicy::Standard]); // L1 = 300s
        let base = c.next_interval("src", 0.5).as_secs_f64();

        c.notify_rate_limited("src");
        let after_one = c.next_interval("src", 0.5).as_secs_f64();
        assert!(after_one > base * 1.5, "first backoff should ≥ 2x base");

        c.notify_rate_limited("src");
        let after_two = c.next_interval("src", 0.5).as_secs_f64();
        assert!(after_two > after_one, "second backoff should grow further");

        c.notify_success("src");
        let after_reset = c.next_interval("src", 0.5).as_secs_f64();
        assert!(
            (after_reset - base).abs() <= 0.01,
            "reset should return to base"
        );
    }

    #[test]
    fn backoff_caps_at_ceiling() {
        let mut c = CadenceController::new();
        c.set_tier_for("src", &[AutomationPolicy::Standard]);
        for _ in 0..50 {
            c.notify_rate_limited("src");
        }
        let capped = c.next_interval("src", 0.5);
        // Within 10% of the ceiling.
        let target = intervals::BACKOFF_CEILING.as_secs_f64();
        let got = capped.as_secs_f64();
        assert!(
            got <= target * 1.11,
            "got {got}s should be ≤ ceiling {target}s + jitter"
        );
    }

    #[test]
    fn jitter_range_is_within_ten_percent() {
        let target = Duration::from_secs(300);
        for seed_i in 0..=10 {
            let seed = f64::from(seed_i) / 10.0;
            let jittered = apply_jitter(target, seed).as_secs_f64();
            let base = target.as_secs_f64();
            let ratio = jittered / base;
            assert!(
                (0.9..=1.1).contains(&ratio),
                "ratio {ratio} out of band for seed {seed}"
            );
        }
    }

    #[test]
    fn jitter_clamps_out_of_band_seed() {
        let target = Duration::from_secs(100);
        let low = apply_jitter(target, -5.0).as_secs_f64();
        let high = apply_jitter(target, 5.0).as_secs_f64();
        let base = target.as_secs_f64();
        assert!((low / base - 0.9).abs() < 1e-9);
        assert!((high / base - 1.1).abs() < 1e-9);
    }

    #[test]
    fn jitter_seed_is_in_unit_interval_and_varies() {
        let seeds: Vec<f64> = (0..16).map(jitter_seed).collect();
        for (n, s) in seeds.iter().enumerate() {
            assert!(
                (0.0..1.0).contains(s),
                "seed for attempt {n} = {s} out of [0,1)"
            );
        }
        // Consecutive attempts must differ (no clustering on one point).
        assert!(seeds[0] != seeds[1] && seeds[1] != seeds[2]);
        // attempt 0 maps to 0.0 (n * c with n=0).
        assert!((jitter_seed(0) - 0.0).abs() < 1e-12);
    }

    #[test]
    fn jitter_seed_feeds_next_interval_within_band() {
        let mut c = CadenceController::new();
        c.set_tier_for("src", &[AutomationPolicy::Standard]); // L1 = 300s
        let base = intervals::L1.as_secs_f64();
        for attempt in 0..32u64 {
            let d = c.next_interval("src", jitter_seed(attempt)).as_secs_f64();
            assert!(
                (0.9 * base..=1.1 * base).contains(&d),
                "attempt {attempt}: {d}s outside ±10% of {base}s"
            );
        }
    }

    #[test]
    fn backoff_exp_reflects_internal_state() {
        let mut c = CadenceController::new();
        assert_eq!(c.backoff_exp("absent"), 0);
        c.notify_rate_limited("src");
        c.notify_rate_limited("src");
        c.notify_rate_limited("src");
        assert_eq!(c.backoff_exp("src"), 3);
        c.notify_success("src");
        assert_eq!(c.backoff_exp("src"), 0);
    }
}
