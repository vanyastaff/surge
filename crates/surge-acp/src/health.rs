//! Health monitoring and fallback routing for ACP agents.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
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
    /// Clears rate-limit status if the reset time has passed.
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
    }

    /// Records a failed request. Detects rate-limit errors by checking for
    /// "429", "rate limit", or "too many" in the error string (case-insensitive).
    /// Parses `Retry-After` header if present, otherwise defaults to 60 seconds.
    pub fn record_failure(&mut self, agent: &str, error: &str) {
        if let Some(health) = self.agents.get_mut(agent) {
            health.total_requests += 1;
            health.total_failures += 1;
            health.last_error = Some(error.to_string());

            let lower = error.to_lowercase();
            if lower.contains("429") || lower.contains("rate limit") || lower.contains("too many") {
                let retry_after_secs = parse_retry_after(error).unwrap_or(60);
                warn!(agent, error, retry_after_secs, "rate limit detected");
                health.rate_limited = true;
                health.rate_limit_reset =
                    Some(Instant::now() + Duration::from_secs(retry_after_secs));
            }
        }
    }

    /// Records a successful heartbeat from an agent.
    /// Clears consecutive heartbeat failure count.
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
    fn test_rate_limit_detection() {
        let mut monitor = HealthTracker::new();
        monitor.register("claude");
        monitor.record_failure("claude", "429 Too Many Requests");
        let health = monitor.get_health("claude").unwrap();
        assert!(health.rate_limited);
        assert!(health.rate_limit_reset.is_some());
        assert!(!health.is_healthy());
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
