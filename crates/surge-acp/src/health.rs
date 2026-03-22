//! Health monitoring and fallback routing for ACP agents.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn};

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
}

/// Monitors agent health and provides fallback routing.
#[derive(Debug, Default)]
pub struct HealthMonitor {
    agents: HashMap<String, AgentHealth>,
    fallback_map: HashMap<String, String>,
}

impl HealthMonitor {
    /// Creates a new empty `HealthMonitor`.
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

            // Clear rate limit if past reset time
            if health.rate_limited {
                if let Some(reset) = health.rate_limit_reset {
                    if Instant::now() >= reset {
                        info!(agent, "rate limit reset, clearing");
                        health.rate_limited = false;
                        health.rate_limit_reset = None;
                    }
                }
            }
        }
    }

    /// Records a failed request. Detects rate-limit errors by checking for
    /// "429", "rate limit", or "too many" in the error string (case-insensitive).
    pub fn record_failure(&mut self, agent: &str, error: &str) {
        if let Some(health) = self.agents.get_mut(agent) {
            health.total_requests += 1;
            health.total_failures += 1;
            health.last_error = Some(error.to_string());

            let lower = error.to_lowercase();
            if lower.contains("429") || lower.contains("rate limit") || lower.contains("too many")
            {
                warn!(agent, error, "rate limit detected");
                health.rate_limited = true;
                health.rate_limit_reset = Some(Instant::now() + Duration::from_secs(60));
            }
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
            if let Some(fallback_name) = self.fallback_map.get(preferred) {
                if let Some(fallback_health) = self.agents.get(fallback_name.as_str()) {
                    if fallback_health.is_healthy() {
                        warn!(
                            preferred,
                            fallback = fallback_name.as_str(),
                            "routing to fallback agent"
                        );
                        return fallback_name.as_str();
                    }
                }
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
        let mut monitor = HealthMonitor::new();
        monitor.register("claude");
        let health = monitor.get_health("claude").unwrap();
        assert!(health.is_healthy());
        assert_eq!(health.error_rate(), 0.0);
    }

    #[test]
    fn test_error_rate() {
        let mut monitor = HealthMonitor::new();
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
        let mut monitor = HealthMonitor::new();
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
        let mut monitor = HealthMonitor::new();
        monitor.register("claude");
        monitor.record_failure("claude", "429 Too Many Requests");
        let health = monitor.get_health("claude").unwrap();
        assert!(health.rate_limited);
        assert!(health.rate_limit_reset.is_some());
        assert!(!health.is_healthy());
    }

    #[test]
    fn test_fallback_routing() {
        let mut monitor = HealthMonitor::new();
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
        let mut monitor = HealthMonitor::new();
        monitor.register("claude");
        monitor.record_success("claude", Duration::from_millis(100));
        monitor.record_success("claude", Duration::from_millis(200));
        let health = monitor.get_health("claude").unwrap();
        assert_eq!(health.total_requests, 2);
        assert_eq!(health.total_failures, 0);
        assert_eq!(health.avg_latency_ms, 150);
    }
}
