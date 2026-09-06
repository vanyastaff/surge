//! Health monitoring and fallback routing for ACP agents.
// pre-existing per M2 precedent; not in scope for M3
#![allow(clippy::manual_range_contains)]

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use chrono::Utc;
use surge_core::capacity::{CapacityStatus, CapacityWindow};
use tracing::{info, warn};

/// Health status of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Agent is operating normally.
    Healthy,
    /// Agent is experiencing issues (high error rate or rate-limited).
    Degraded,
    /// Agent is offline or unresponsive.
    Offline,
}

/// Parses a `Retry-After` header value from an error message.
/// Supports delay-seconds format (e.g., "Retry-After: 120").
/// Returns `None` if not found or parsing fails.
///
/// Deliberately its own local body, **not** delegated to
/// `surge_core::capacity::parse_retry_after_secs`. That function's return
/// value is not a pure display value here — it becomes
/// `rate_limit_reset`, the wake-up time `record_success` and
/// `resolve_agent` act on, i.e. a routing input, not an inert string. The
/// two parsers already disagree: this one requires a colon and recognizes
/// only `"retry-after"`; `capacity`'s makes the colon optional and also
/// accepts the prose form `"retry after 30s"`. Delegating would silently
/// change which messages move `rate_limit_reset` off its 60s default — the
/// exact class of change [`is_rate_limited_for_routing`]'s doc explains for
/// the boolean classifier next to this one. A review round asked twice not
/// to unify this parser; the request has not changed.
fn parse_retry_after(error: &str) -> Option<u64> {
    // Look for "retry-after" (case-insensitive) followed by colon and number
    let lower = error.to_lowercase();
    if let Some(pos) = lower.find("retry-after") {
        // Extract substring after "retry-after"
        let after = &error[pos + "retry-after".len()..];
        // Skip whitespace and colon
        let trimmed = after.trim_start().strip_prefix(':')?.trim_start();
        // Parse the first sequence of digits
        let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u64>().ok()
    } else {
        None
    }
}

/// `true` when `error` should set `rate_limited`/`rate_limit_reset` —
/// `resolve_agent`'s fallback-routing decision.
///
/// Deliberately its own narrow classifier, matching this tracker's
/// pre-existing behavior unchanged (only the bare `"too many"` — which also
/// matches capacity-unrelated failures like "too many open connections" —
/// narrowed to `"too many requests"`, a strict correction, not a widening).
/// **Not** shared with `surge_core::capacity::looks_like_rate_limit`, the
/// wider classifier [`HealthTracker::record_failure`] uses for the R34–R36
/// capacity observation a few lines below this same method. Relative to
/// this function's three patterns, that list additionally recognizes: a
/// bare `"rate_limit"` substring (so `rate_limit_error` — Anthropic's
/// actual error `type` — and `rate_limit_exceeded`), `"quota exceeded"`,
/// `"insufficient_quota"`, `"resource_exhausted"`, `"overloaded_error"`, and
/// "usage limit reached". Letting any of those six additionally flip
/// `rate_limited` would be a real, silent change to which agents
/// `resolve_agent` routes away from — exactly the kind of side effect a
/// prior round of review caught happening to `surge-acp::pool`'s own
/// routing classifier for the same reason. Fallback routing and the
/// capacity model are different questions with different costs for
/// over-inclusion (a routing decision vs. a displayed line) and must be
/// widened, if ever, on purpose and separately.
fn is_rate_limited_for_routing(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("429") || lower.contains("rate limit") || lower.contains("too many requests")
}

/// Health statistics for a single agent.
#[derive(Debug)]
pub struct AgentHealth {
    /// Agent name.
    pub name: String,
    /// Total number of requests sent to this agent.
    pub total_requests: u64,
    /// Total number of failed requests.
    pub total_failures: u64,
    /// Whether the agent is currently rate-limited.
    pub rate_limited: bool,
    /// When the rate limit resets (if rate-limited).
    pub rate_limit_reset: Option<Instant>,
    /// Average latency in milliseconds.
    pub avg_latency_ms: u64,
    /// Last error message, if any.
    pub last_error: Option<String>,
    /// Last 100 latency samples for percentile calculation.
    latency_samples: VecDeque<Duration>,
    /// When the agent was registered (used for uptime tracking).
    pub uptime_start: Instant,
    /// Total number of heartbeat failures.
    pub total_heartbeat_failures: u64,
    /// Number of consecutive heartbeat failures.
    pub consecutive_heartbeat_failures: u64,
    /// Count of every failure of any kind ever recorded for this agent —
    /// `record_failure`, `record_connect_failure`, *and*
    /// `record_heartbeat_failure` all increment this (R34–R36's
    /// `CapacityStatus::NeverObserved` vs `Unclassified` distinction needs
    /// "has anything at all failed", not just "has a prompt dispatch
    /// failed"; a connect()/spawn/handshake failure, or a heartbeat-only
    /// failure, must not read as `NeverObserved`). Deliberately separate
    /// from `total_failures`, which pairs with `total_requests` for
    /// `error_rate()` — connect attempts and heartbeats are not requests,
    /// and mixing them into that ratio would skew it.
    pub observed_failures: u64,
}

impl AgentHealth {
    fn new(name: String) -> Self {
        Self {
            name,
            total_requests: 0,
            total_failures: 0,
            rate_limited: false,
            rate_limit_reset: None,
            avg_latency_ms: 0,
            last_error: None,
            latency_samples: VecDeque::with_capacity(100),
            uptime_start: Instant::now(),
            total_heartbeat_failures: 0,
            consecutive_heartbeat_failures: 0,
            observed_failures: 0,
        }
    }

    /// Returns the error rate as a percentage (0-100).
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        (self.total_failures as f64 / self.total_requests as f64) * 100.0
    }

    /// Returns `true` if the agent is considered healthy:
    /// not rate-limited AND error rate below 50%.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        !self.rate_limited && self.error_rate() < 50.0
    }

    /// Returns the current health status of the agent.
    ///
    /// Status is determined by the following rules:
    /// - `Offline`: 3 or more consecutive heartbeat failures
    /// - `Degraded`: error rate >= 50% OR rate-limited (but not offline)
    /// - `Healthy`: otherwise
    #[must_use]
    pub fn status(&self) -> HealthStatus {
        // Offline: 3+ consecutive heartbeat failures
        if self.consecutive_heartbeat_failures >= 3 {
            return HealthStatus::Offline;
        }

        // Degraded: rate-limited or high error rate
        if self.rate_limited || self.error_rate() >= 50.0 {
            return HealthStatus::Degraded;
        }

        HealthStatus::Healthy
    }

    /// Returns the p50 (median) latency in milliseconds.
    /// Returns 0 if no latency samples are available.
    #[must_use]
    pub fn latency_p50_ms(&self) -> u64 {
        self.calculate_percentile(50.0)
    }

    /// Returns the p99 latency in milliseconds.
    /// Returns 0 if no latency samples are available.
    #[must_use]
    pub fn latency_p99_ms(&self) -> u64 {
        self.calculate_percentile(99.0)
    }

    /// Calculates a percentile from latency samples.
    /// Percentile should be between 0.0 and 100.0.
    fn calculate_percentile(&self, percentile: f64) -> u64 {
        if self.latency_samples.is_empty() {
            return 0;
        }

        let mut sorted: Vec<u64> = self
            .latency_samples
            .iter()
            .map(|d| d.as_millis() as u64)
            .collect();
        sorted.sort_unstable();

        let index = ((percentile / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[index]
    }

    /// Returns the uptime duration since agent registration.
    #[must_use]
    pub fn uptime(&self) -> Duration {
        Instant::now().duration_since(self.uptime_start)
    }
}

/// Monitors agent health and provides fallback routing.
#[derive(Debug, Default)]
pub struct HealthTracker {
    agents: HashMap<String, AgentHealth>,
    fallback_map: HashMap<String, String>,
    /// Learned capacity per agent account (R34–R36), populated from real
    /// observed 429s in [`Self::record_failure`]. Absent entry means "not
    /// yet observed" — see [`Self::capacity_status`].
    capacity: HashMap<String, CapacityWindow>,
}

impl HealthTracker {
    /// Creates a new empty `HealthTracker`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an agent for health monitoring.
    pub fn register(&mut self, name: &str) {
        info!(agent = name, "registering agent for health monitoring");
        self.agents
            .entry(name.to_string())
            .or_insert_with(|| AgentHealth::new(name.to_string()));
    }

    /// Configures a fallback agent for a primary agent.
    pub fn set_fallback(&mut self, primary: &str, fallback: &str) {
        info!(primary, fallback, "setting fallback agent");
        self.fallback_map
            .insert(primary.to_string(), fallback.to_string());
    }

    /// Records a successful request to an agent, updating latency stats.
    /// Clears rate-limit status if the reset time has passed, and expires a
    /// stale capacity window (see below).
    pub fn record_success(&mut self, agent: &str, latency: Duration) {
        if let Some(health) = self.agents.get_mut(agent) {
            health.total_requests += 1;
            let latency_ms = latency.as_millis() as u64;
            // Running average
            let prev_total = health.total_requests - 1;
            health.avg_latency_ms = if prev_total == 0 {
                latency_ms
            } else {
                (health.avg_latency_ms * prev_total + latency_ms) / health.total_requests
            };

            // Store latency sample for percentile calculation
            health.latency_samples.push_back(latency);
            if health.latency_samples.len() > 100 {
                health.latency_samples.pop_front();
            }

            // Clear rate limit if past reset time
            if health.rate_limited
                && health
                    .rate_limit_reset
                    .is_some_and(|reset| Instant::now() >= reset)
            {
                info!(agent, "rate limit reset, clearing");
                health.rate_limited = false;
                health.rate_limit_reset = None;
            }
        }

        // R34–R36: a stale `CapacityWindow` expires on its own terms —
        // **never** gated on `rate_limited` above. `rate_limited` is set by
        // `is_rate_limited_for_routing`'s narrow classifier;
        // `self.capacity` is set by `record_failure`'s wider one (see its
        // doc). A message matching the wide classifier but not the narrow
        // one (`overloaded_error`, `resource_exhausted`, ...) produces
        // `Known(_)` with `rate_limited` never becoming `true` at all — the
        // `if health.rate_limited` guard this clearing previously lived
        // inside would then never run, and that window would report
        // `is_exhausted() == true` forever, through any number of later
        // successes. Expiry has to read the window's own state:
        // - a known `resets_at` in the past is definitive;
        // - an unknown `resets_at` (no `Retry-After` was ever observed) has
        //   no time criterion at all — the alternative to treating a
        //   success as sufficient evidence there is a window that can
        //   *never* expire, which is the same permanent-park bug by
        //   another route.
        if let Some(window) = self.capacity.get(agent) {
            let expired = match window.resets_at() {
                Some(reset_at) => Utc::now() >= reset_at,
                None => true,
            };
            if expired {
                self.capacity.remove(agent);
            }
        }
    }

    /// Records a failed request.
    ///
    /// Two independent classifications happen here, deliberately on two
    /// different classifiers (see [`is_rate_limited_for_routing`]'s doc for
    /// why): `rate_limited`/`rate_limit_reset` (this method's pre-existing
    /// fields, feeding `resolve_agent`'s fallback-routing decision — a
    /// narrow, unchanged classifier, with `rate_limit_reset` defaulting to
    /// 60 seconds when the provider sent no `Retry-After`, because routing
    /// needs *some* concrete wake-up estimate) and `self.capacity` (R34–R36,
    /// feeding [`Self::capacity_status`] — a wider, observation-only
    /// classifier via `surge_core::capacity`, whose `resets_at` stays
    /// `None` without a real `Retry-After` rather than inheriting that same
    /// 60s routing default; see [`CapacityWindow::observed_429`]).
    pub fn record_failure(&mut self, agent: &str, error: &str) {
        if let Some(health) = self.agents.get_mut(agent) {
            health.total_requests += 1;
            health.total_failures += 1;
            health.observed_failures += 1;
            health.last_error = Some(error.to_string());

            if is_rate_limited_for_routing(error) {
                let retry_after_secs = parse_retry_after(error).unwrap_or(60);
                warn!(agent, error, retry_after_secs, "rate limit detected");
                health.rate_limited = true;
                health.rate_limit_reset =
                    Some(Instant::now() + Duration::from_secs(retry_after_secs));
            }

            // Capacity model (R34–R36): independent of the routing check
            // above — a provider shape the narrow routing classifier
            // doesn't recognize (`rate_limit_error` — Anthropic's actual
            // error `type`, `rate_limit_exceeded`, `quota exceeded`,
            // `insufficient_quota`, `resource_exhausted`, `overloaded_error`,
            // "usage limit reached") still populates this, since
            // over-inclusion here only changes what an operator is shown,
            // not a routing decision. Gated on registration like every
            // other field on `health`, above.
            if let Some(window) = CapacityWindow::from_observed_error(agent, error, Utc::now()) {
                self.capacity.insert(agent.to_string(), window);
            }
        }
    }

    /// Records a connection/spawn/handshake failure for `agent`, called
    /// from `AgentPool`'s prompt-dispatch retry loop when a *reconnect*
    /// (a fallback candidate, or a previously-connected agent whose
    /// connection dropped) fails — previously untracked here at all,
    /// which made such an agent misreport `CapacityStatus::NeverObserved`
    /// identical to one that had never been touched.
    ///
    /// **Not** reached on an agent's very first contact: `create_session`
    /// resolves its own initial `connect()` via `validate_agent_auth`,
    /// whose failure propagates with a bare `?` and never calls this
    /// method. An agent that fails on every attempt from a cold start
    /// still reports `NeverObserved` until it reaches the retry loop this
    /// method is actually wired into.
    ///
    /// Increments `observed_failures` only — not `total_requests`/
    /// `total_failures`, which pair for `error_rate()` and would be skewed
    /// by counting a connect attempt as a "request" alongside actual prompt
    /// dispatches. Also runs the same R34–R36 capacity classification
    /// `record_failure` does, in case the connection error names a real
    /// provider exhaustion shape.
    pub fn record_connect_failure(&mut self, agent: &str, error: &str) {
        if let Some(health) = self.agents.get_mut(agent) {
            health.observed_failures += 1;
            health.last_error = Some(error.to_string());

            if let Some(window) = CapacityWindow::from_observed_error(agent, error, Utc::now()) {
                self.capacity.insert(agent.to_string(), window);
            }
        }
    }

    /// Capacity status Surge has for `agent` (R34–R36). Distinguishes a
    /// fresh account nobody has ever failed against
    /// ([`CapacityStatus::NeverObserved`], the common case, R35.1) from one
    /// that has failed without ever matching a recognized rate-limit shape
    /// ([`CapacityStatus::Unclassified`]) — the two must not read the same
    /// to an operator, or a real capacity problem hides behind "no data".
    #[must_use]
    pub fn capacity_status(&self, agent: &str) -> CapacityStatus {
        if let Some(window) = self.capacity.get(agent) {
            return CapacityStatus::Known(window.clone());
        }
        match self.agents.get(agent) {
            // `observed_failures`, not `total_failures`: the latter only
            // counts prompt-dispatch failures, so a connect()/spawn/
            // handshake- or heartbeat-only failure history would never
            // move it and this agent would misreport `NeverObserved`
            // despite having failed repeatedly. `observed_failures` is
            // incremented by `record_failure`, `record_connect_failure`,
            // *and* `record_heartbeat_failure` — see its own doc.
            //
            // Known imprecision, not fixed here: `observed_failures` is a
            // strict superset of `total_failures`, so a failure pool.rs
            // has *already* classified unambiguously as something else
            // (401 auth, a connection reset, a timeout) still counts
            // toward `Unclassified` here — this predicate can say "at
            // least one failure happened, none matched a rate-limit
            // shape", not "this failure might be a rate limit". Narrowing
            // it would require importing pool.rs's own auth/connection
            // classifiers into this crate-internal count, which is a
            // separate change from R34–R36's scope.
            Some(health) if health.observed_failures > 0 => CapacityStatus::Unclassified,
            _ => CapacityStatus::NeverObserved,
        }
    }

    /// Records a successful heartbeat from an agent.
    /// Clears consecutive heartbeat failure count.
    ///
    /// Does **not** expire `self.capacity` — a live connection is not
    /// evidence a prompt would succeed. Only [`Self::record_success`] does
    /// that. An agent that recovers on heartbeats alone and never takes
    /// another prompt keeps whatever `CapacityWindow` it last had (e.g. one
    /// `record_connect_failure` set with `resets_at: None`) until its next
    /// real prompt outcome.
    pub fn record_heartbeat_success(&mut self, agent: &str) {
        if let Some(health) = self.agents.get_mut(agent)
            && health.consecutive_heartbeat_failures > 0
        {
            info!(
                agent,
                "heartbeat recovered after {} consecutive failures",
                health.consecutive_heartbeat_failures
            );
            health.consecutive_heartbeat_failures = 0;
        }
    }

    /// Records a failed heartbeat from an agent.
    /// Increments both total and consecutive heartbeat failure counts.
    pub fn record_heartbeat_failure(&mut self, agent: &str) {
        if let Some(health) = self.agents.get_mut(agent) {
            health.total_heartbeat_failures += 1;
            health.consecutive_heartbeat_failures += 1;
            // R34–R36: a heartbeat failure is still evidence *something*
            // failed, even though it carries no rate-limit text to
            // classify — see `capacity_status`.
            health.observed_failures += 1;
            warn!(
                agent,
                total = health.total_heartbeat_failures,
                consecutive = health.consecutive_heartbeat_failures,
                "heartbeat failure"
            );
        }
    }

    /// Resolves which agent to use. Returns the preferred agent if healthy,
    /// otherwise its fallback (if healthy), otherwise the preferred agent.
    #[must_use]
    pub fn resolve_agent<'a>(&'a self, preferred: &'a str) -> &'a str {
        if let Some(health) = self.agents.get(preferred) {
            if health.is_healthy() {
                return preferred;
            }
            // Try fallback
            if let Some(fallback_name) = self.fallback_map.get(preferred)
                && let Some(fallback_health) = self.agents.get(fallback_name.as_str())
                && fallback_health.is_healthy()
            {
                warn!(
                    preferred,
                    fallback = fallback_name.as_str(),
                    "routing to fallback agent"
                );
                return fallback_name.as_str();
            }
        }
        preferred
    }

    /// Returns health info for all monitored agents.
    #[must_use]
    pub fn all_health(&self) -> Vec<&AgentHealth> {
        self.agents.values().collect()
    }

    /// Returns health info for a specific agent.
    #[must_use]
    pub fn get_health(&self, agent: &str) -> Option<&AgentHealth> {
        self.agents.get(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_healthy_by_default() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        let health = monitor.get_health("claude").unwrap();
        assert!(health.is_healthy());
        assert_eq!(health.error_rate(), 0.0);
    }

    #[test]
    fn test_error_rate() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        // 10 requests, 3 failures → 30%
        for _ in 0..7 {
            monitor.record_success("claude", Duration::from_millis(100));
        }
        for _ in 0..3 {
            monitor.record_failure("claude", "internal error");
        }
        let health = monitor.get_health("claude").unwrap();
        assert!((health.error_rate() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_unhealthy_on_high_errors() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        // 60% error rate
        for _ in 0..4 {
            monitor.record_success("claude", Duration::from_millis(50));
        }
        for _ in 0..6 {
            monitor.record_failure("claude", "server error");
        }
        let health = monitor.get_health("claude").unwrap();
        assert!(!health.is_healthy());
    }

    #[test]
    fn test_is_rate_limited_for_routing_rejects_bare_too_many() {
        // This classifier's `"too many requests"` (not the bare `"too
        // many"` HEAD used) is a deliberate narrowing, not a byte-for-byte
        // port — pin it: a capacity-unrelated failure must not flip
        // `rate_limited` and reroute traffic away from a healthy agent.
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "too many open connections");
        assert!(!monitor.get_health("claude").unwrap().rate_limited);
    }

    #[test]
    fn test_record_connect_failure_is_observed_but_not_a_request() {
        // The reconnect-within-prompt-dispatch blindness: previously
        // nothing here moved at all for this failure mode (see this
        // method's doc for the cold-start case it does *not* cover).
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_connect_failure("claude", "spawn failed: binary not found");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(
            health.total_requests, 0,
            "a connect attempt is not a request"
        );
        assert_eq!(health.total_failures, 0);
        assert_eq!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Unclassified
        );
    }

    #[test]
    fn test_record_connect_failure_still_classifies_a_real_capacity_shape() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_connect_failure("claude", "overloaded_error");
        assert!(matches!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Known(_)
        ));
    }

    #[test]
    fn test_rate_limit_detection() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "429 Too Many Requests");
        let health = monitor.get_health("claude").unwrap();
        assert!(health.rate_limited);
        assert!(health.rate_limit_reset.is_some());
        assert!(!health.is_healthy());
    }

    /// Pins `record_failure`'s routing classifier apart from the wider
    /// R34–R36 capacity classifier it also runs (a few lines below in the
    /// same method): a real provider shape the wide classifier recognizes
    /// still must not flip `rate_limited` (`resolve_agent`'s
    /// fallback-routing input) — only `capacity_status` should see it. If
    /// these two classifiers are ever merged back into one, this test goes
    /// red first.
    #[test]
    fn test_record_failure_capacity_shape_does_not_set_rate_limited_for_routing() {
        // `rate_limit_error` is Anthropic's actual error `type` — the most
        // realistic wide-only shape, not a contrived one: the narrow
        // routing classifier (`is_rate_limited_for_routing`) knows only
        // "429" / "rate limit" / "too many requests" and does not match it.
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "rate_limit_error");

        let health = monitor.get_health("claude").unwrap();
        assert!(
            !health.rate_limited,
            "routing must stay on its own narrow classifier"
        );
        assert!(matches!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Known(_)
        ));

        // The state the previous (fixed) bug got stuck in forever: no
        // `Retry-After` was ever observed (`resets_at` is `None`), and
        // `rate_limited` was never `true` in the first place — so a fix
        // that only cleared `self.capacity` inside the `if
        // health.rate_limited` branch would never run for this exact case.
        // A later success must still expire it.
        monitor.record_success("claude", Duration::from_millis(10));
        assert_eq!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Unclassified,
            "a wide-only, rate_limited-never-true window must still expire on success"
        );
    }

    #[test]
    fn test_fallback_routing() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.register("copilot");
        monitor.set_fallback("claude", "copilot");

        // Make claude rate-limited
        monitor.record_failure("claude", "429 Too Many Requests");

        let resolved = monitor.resolve_agent("claude");
        assert_eq!(resolved, "copilot");
    }

    #[test]
    fn test_success_recording() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_success("claude", Duration::from_millis(100));
        monitor.record_success("claude", Duration::from_millis(200));
        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.total_requests, 2);
        assert_eq!(health.total_failures, 0);
        assert_eq!(health.avg_latency_ms, 150);
    }

    #[test]
    fn test_rate_limit_with_retry_after() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "429 Too Many Requests; Retry-After: 120");
        let health = monitor.get_health("claude").unwrap();
        assert!(health.rate_limited);
        assert!(health.rate_limit_reset.is_some());
        // Verify reset time is approximately 120 seconds from now
        let reset_duration = health
            .rate_limit_reset
            .unwrap()
            .duration_since(Instant::now());
        assert!(reset_duration.as_secs() >= 119 && reset_duration.as_secs() <= 121);
    }

    #[test]
    fn test_rate_limit_without_retry_after() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "429 Too Many Requests");
        let health = monitor.get_health("claude").unwrap();
        assert!(health.rate_limited);
        // Should default to 60 seconds
        let reset_duration = health
            .rate_limit_reset
            .unwrap()
            .duration_since(Instant::now());
        assert!(reset_duration.as_secs() >= 59 && reset_duration.as_secs() <= 61);
    }

    #[test]
    fn test_capacity_status_never_observed_before_any_failure() {
        // The primary case (R35.1): a fresh account has never been
        // observed, so there is nothing to report — not a fabricated
        // default.
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        assert_eq!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::NeverObserved
        );
    }

    #[test]
    fn test_capacity_status_known_via_record_failure_with_retry_after() {
        // Exercises the real production entry point (`record_failure` is
        // called from `AgentPool`'s prompt-dispatch failure handling) —
        // not a capacity-only helper called in isolation.
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "429 Too Many Requests; Retry-After: 120");

        let window = monitor
            .capacity_status("claude")
            .window()
            .cloned()
            .expect("429 with Retry-After must populate a capacity window");
        assert_eq!(window.account(), "claude");
        assert_eq!(
            window.source(),
            surge_core::capacity::CapacitySource::Observed429
        );
        assert!(window.is_exhausted());
        // resets_at is set (~120s out); exact arithmetic is covered in
        // surge-core's own capacity tests — here we only prove the wiring.
        assert!(window.resets_at().is_some());

        // Negative direction of the expiry fix, pinned: a `resets_at` still
        // in the future must *not* be treated as expired. Without the
        // `Utc::now() >= reset_at` comparison in `record_success` (e.g. if
        // it were replaced by an unconditional `true`), this would also
        // clear here and every other green test would stay green — this is
        // the one assertion that would catch it.
        monitor.record_success("claude", Duration::from_millis(10));
        assert!(matches!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Known(_)
        ));
    }

    #[test]
    fn test_capacity_status_resets_at_unknown_without_retry_after() {
        // No Retry-After on the provider's response: `resets_at` must stay
        // unknown, not fall back to the 60s routing default that
        // `rate_limit_reset` (a different, pre-existing field) uses.
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "429 Too Many Requests");

        let window = monitor
            .capacity_status("claude")
            .window()
            .cloned()
            .expect("429 observed");
        assert_eq!(window.resets_at(), None);
        assert!(window.is_exhausted());
    }

    #[test]
    fn test_capacity_status_unclassified_for_a_failure_that_is_not_a_rate_limit() {
        // A real failure happened, but it isn't a recognized rate-limit
        // shape — this must read as "saw something, couldn't classify it",
        // never the same as `NeverObserved` (see `CapacityStatus`'s doc).
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "internal server error");
        assert_eq!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Unclassified
        );
    }

    #[test]
    fn test_capacity_status_clears_on_recovery_not_stuck_exhausted_forever() {
        // `rate_limit_exceeded` (OpenAI) is wide-only — the narrow routing
        // classifier does not match it, so `rate_limited` never becomes
        // `true` here. This exercises the *other* branch of the expiry fix:
        // a known `resets_at` (from the embedded `Retry-After: 0`) that has
        // already passed, checked against the window's own state — not
        // against `health.rate_limited`, which this message never sets.
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "rate_limit_exceeded; Retry-After: 0");
        assert!(!monitor.get_health("claude").unwrap().rate_limited);
        assert!(matches!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Known(_)
        ));

        // `Retry-After: 0` means `resets_at` is already in the past by the
        // time this runs.
        monitor.record_success("claude", Duration::from_millis(10));
        // Not `Known` (the stale window is gone) and not `NeverObserved`
        // either — a 429 genuinely did happen at some point, so
        // `Unclassified` ("a failure was observed, no currently-known
        // window") is the accurate post-recovery state, not a fabricated
        // "nothing ever happened".
        assert_eq!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Unclassified
        );
    }

    #[test]
    fn test_capacity_status_unclassified_from_heartbeat_failures_alone() {
        // An agent whose every heartbeat fails (spawn/handshake broken)
        // never calls `record_failure` — `total_failures` never moves —
        // but it has very much failed, and must not report the same
        // `NeverObserved` a truly untouched account would.
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_heartbeat_failure("claude");
        assert_eq!(
            monitor.capacity_status("claude"),
            surge_core::capacity::CapacityStatus::Unclassified
        );
    }

    #[test]
    fn test_parse_retry_after_various_formats() {
        assert_eq!(parse_retry_after("Retry-After: 120"), Some(120));
        assert_eq!(parse_retry_after("retry-after: 30"), Some(30));
        assert_eq!(parse_retry_after("RETRY-AFTER:90"), Some(90));
        assert_eq!(parse_retry_after("Retry-After:  180  "), Some(180));
        assert_eq!(
            parse_retry_after("429 Too Many Requests; Retry-After: 60"),
            Some(60)
        );
        assert_eq!(parse_retry_after("no retry header here"), None);
        assert_eq!(parse_retry_after("Retry-After: invalid"), None);
    }

    #[test]
    fn test_latency_percentiles() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // Record latencies: 10ms, 20ms, 30ms, ..., 100ms (10 samples)
        for i in 1..=10 {
            monitor.record_success("claude", Duration::from_millis(i * 10));
        }

        let health = monitor.get_health("claude").unwrap();
        // p50: index = (0.5 * 9).round() = 4.5.round() = 5 → sorted[5] = 60
        assert_eq!(health.latency_p50_ms(), 60);
        // p99: index = (0.99 * 9).round() = 8.91.round() = 9 → sorted[9] = 100
        assert_eq!(health.latency_p99_ms(), 100);
    }

    #[test]
    fn test_latency_percentiles_with_no_data() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        let health = monitor.get_health("claude").unwrap();
        // Should return 0 when no samples
        assert_eq!(health.latency_p50_ms(), 0);
        assert_eq!(health.latency_p99_ms(), 0);
    }

    #[test]
    fn test_latency_samples_bounded_to_100() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // Record 150 latencies
        for i in 1..=150 {
            monitor.record_success("claude", Duration::from_millis(i));
        }

        let health = monitor.get_health("claude").unwrap();
        // Should only keep last 100 samples (51-150ms)
        // p50 of 51-150 should be around 100ms
        let p50 = health.latency_p50_ms();
        assert!(p50 >= 95 && p50 <= 105, "p50 was {}", p50);
    }

    #[test]
    fn test_uptime_tracking() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // Sleep briefly to ensure uptime is measurable
        std::thread::sleep(Duration::from_millis(10));

        let health = monitor.get_health("claude").unwrap();
        let uptime = health.uptime();

        // Uptime should be at least 10ms
        assert!(uptime.as_millis() >= 10);
    }

    #[test]
    fn test_heartbeat_failure_tracking() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // Record 3 heartbeat failures
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_failure("claude");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.total_heartbeat_failures, 3);
        assert_eq!(health.consecutive_heartbeat_failures, 3);
    }

    #[test]
    fn test_heartbeat_recovery() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // Record failures then success
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_success("claude");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.total_heartbeat_failures, 2);
        assert_eq!(health.consecutive_heartbeat_failures, 0);

        // Record more failures after recovery
        monitor.record_heartbeat_failure("claude");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.total_heartbeat_failures, 3);
        assert_eq!(health.consecutive_heartbeat_failures, 1);
    }

    #[test]
    fn test_health_status_healthy() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_success("claude", Duration::from_millis(100));

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.status(), HealthStatus::Healthy);
    }

    #[test]
    fn test_health_status_degraded_high_error_rate() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // 60% error rate
        for _ in 0..4 {
            monitor.record_success("claude", Duration::from_millis(50));
        }
        for _ in 0..6 {
            monitor.record_failure("claude", "server error");
        }

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.status(), HealthStatus::Degraded);
    }

    #[test]
    fn test_health_status_degraded_rate_limited() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "429 Too Many Requests");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.status(), HealthStatus::Degraded);
    }

    #[test]
    fn test_health_status_offline() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // 3 consecutive heartbeat failures
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_failure("claude");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.status(), HealthStatus::Offline);
    }

    #[test]
    fn test_health_status_recovery_from_offline() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // Go offline
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_failure("claude");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.status(), HealthStatus::Offline);

        // Recover
        monitor.record_heartbeat_success("claude");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.status(), HealthStatus::Healthy);
    }

    #[test]
    fn test_health_status_degraded_before_offline() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");

        // Rate-limited AND 2 heartbeat failures
        monitor.record_failure("claude", "429 Too Many Requests");
        monitor.record_heartbeat_failure("claude");
        monitor.record_heartbeat_failure("claude");

        let health = monitor.get_health("claude").unwrap();
        // Should be degraded (not offline yet, only 2 consecutive heartbeat failures)
        assert_eq!(health.status(), HealthStatus::Degraded);

        // Third heartbeat failure pushes to offline
        monitor.record_heartbeat_failure("claude");

        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.status(), HealthStatus::Offline);
    }
}
