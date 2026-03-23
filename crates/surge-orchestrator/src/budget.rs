//! Budget — token and cost limit enforcement for pipeline execution.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use surge_core::config::PipelineConfig;
use surge_core::event::SurgeEvent;
use tokio::sync::broadcast;
use tracing::warn;

/// Tracks cumulative token usage and cost, enforcing configured limits.
///
/// Thread-safe — can be shared across async tasks via `Arc`.
#[derive(Debug, Clone)]
pub struct BudgetTracker {
    total_tokens: Arc<AtomicU64>,
    /// Cost in micro-USD (1 USD = 1_000_000 micro-USD) to avoid floating point.
    total_cost_micro_usd: Arc<AtomicU64>,
    max_tokens: Option<u64>,
    /// Max cost in micro-USD.
    max_cost_micro_usd: Option<u64>,
}

/// Result of a budget check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetStatus {
    /// Within budget.
    Ok,
    /// Token limit exceeded.
    TokensExceeded {
        used: u64,
        limit: u64,
    },
    /// Cost limit exceeded.
    CostExceeded {
        used_micro_usd: u64,
        limit_micro_usd: u64,
    },
}

impl BudgetTracker {
    /// Create a new budget tracker from pipeline config.
    #[must_use]
    pub fn new(config: &PipelineConfig) -> Self {
        Self {
            total_tokens: Arc::new(AtomicU64::new(0)),
            total_cost_micro_usd: Arc::new(AtomicU64::new(0)),
            max_tokens: config.max_tokens,
            max_cost_micro_usd: config.max_cost_usd.map(|usd| (usd * 1_000_000.0) as u64),
        }
    }

    /// Create a tracker with no limits (for testing or unlimited mode).
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            total_tokens: Arc::new(AtomicU64::new(0)),
            total_cost_micro_usd: Arc::new(AtomicU64::new(0)),
            max_tokens: None,
            max_cost_micro_usd: None,
        }
    }

    /// Record token usage from a `TokensConsumed` event.
    pub fn record(&self, input_tokens: u64, output_tokens: u64, estimated_cost_usd: Option<f64>) {
        let total = input_tokens + output_tokens;
        self.total_tokens.fetch_add(total, Ordering::Relaxed);

        if let Some(cost) = estimated_cost_usd {
            let micro = (cost * 1_000_000.0) as u64;
            self.total_cost_micro_usd.fetch_add(micro, Ordering::Relaxed);
        }
    }

    /// Check whether the budget has been exceeded.
    #[must_use]
    pub fn check(&self) -> BudgetStatus {
        if let Some(limit) = self.max_tokens {
            let used = self.total_tokens.load(Ordering::Relaxed);
            if used > limit {
                return BudgetStatus::TokensExceeded { used, limit };
            }
        }

        if let Some(limit) = self.max_cost_micro_usd {
            let used = self.total_cost_micro_usd.load(Ordering::Relaxed);
            if used > limit {
                return BudgetStatus::CostExceeded {
                    used_micro_usd: used,
                    limit_micro_usd: limit,
                };
            }
        }

        BudgetStatus::Ok
    }

    /// Total tokens consumed so far.
    #[must_use]
    pub fn tokens_used(&self) -> u64 {
        self.total_tokens.load(Ordering::Relaxed)
    }

    /// Total cost in USD consumed so far.
    #[must_use]
    pub fn cost_usd(&self) -> f64 {
        self.total_cost_micro_usd.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    /// Returns `true` if any limits are configured.
    #[must_use]
    pub fn has_limits(&self) -> bool {
        self.max_tokens.is_some() || self.max_cost_micro_usd.is_some()
    }
}

/// Spawn a background task that listens for `TokensConsumed` events and
/// updates the budget tracker. Returns the join handle.
pub fn start_budget_listener(
    tracker: BudgetTracker,
    mut event_rx: broadcast::Receiver<SurgeEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            if let SurgeEvent::TokensConsumed {
                input_tokens,
                output_tokens,
                estimated_cost_usd,
                ..
            } = event
            {
                tracker.record(input_tokens, output_tokens, estimated_cost_usd);

                match tracker.check() {
                    BudgetStatus::TokensExceeded { used, limit } => {
                        warn!(used, limit, "token budget exceeded");
                    }
                    BudgetStatus::CostExceeded {
                        used_micro_usd,
                        limit_micro_usd,
                    } => {
                        warn!(
                            used_usd = used_micro_usd as f64 / 1_000_000.0,
                            limit_usd = limit_micro_usd as f64 / 1_000_000.0,
                            "cost budget exceeded"
                        );
                    }
                    BudgetStatus::Ok => {}
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_tracker_unlimited() {
        let tracker = BudgetTracker::unlimited();
        assert!(!tracker.has_limits());
        tracker.record(10_000, 5_000, Some(0.05));
        assert_eq!(tracker.check(), BudgetStatus::Ok);
        assert_eq!(tracker.tokens_used(), 15_000);
    }

    #[test]
    fn test_budget_tracker_token_limit() {
        let config = PipelineConfig {
            max_tokens: Some(10_000),
            max_cost_usd: None,
            ..PipelineConfig::default()
        };
        let tracker = BudgetTracker::new(&config);
        assert!(tracker.has_limits());

        tracker.record(6_000, 3_000, None);
        assert_eq!(tracker.check(), BudgetStatus::Ok);

        tracker.record(1_000, 1_000, None);
        assert!(matches!(
            tracker.check(),
            BudgetStatus::TokensExceeded { used: 11_000, limit: 10_000 }
        ));
    }

    #[test]
    fn test_budget_tracker_cost_limit() {
        let config = PipelineConfig {
            max_tokens: None,
            max_cost_usd: Some(1.0),
            ..PipelineConfig::default()
        };
        let tracker = BudgetTracker::new(&config);

        tracker.record(1_000, 500, Some(0.5));
        assert_eq!(tracker.check(), BudgetStatus::Ok);

        tracker.record(1_000, 500, Some(0.6));
        assert!(matches!(
            tracker.check(),
            BudgetStatus::CostExceeded { .. }
        ));
    }

    #[test]
    fn test_budget_tracker_cost_usd() {
        let tracker = BudgetTracker::unlimited();
        tracker.record(0, 0, Some(0.123));
        let cost = tracker.cost_usd();
        assert!((cost - 0.123).abs() < 0.001);
    }

    #[test]
    fn test_budget_tracker_no_cost_events() {
        let config = PipelineConfig {
            max_cost_usd: Some(1.0),
            ..PipelineConfig::default()
        };
        let tracker = BudgetTracker::new(&config);
        tracker.record(1_000, 500, None);
        assert_eq!(tracker.check(), BudgetStatus::Ok);
        assert!((tracker.cost_usd() - 0.0).abs() < f64::EPSILON);
    }
}
