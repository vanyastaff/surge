//! Retry logic with exponential backoff and jitter.

use rand::Rng;
use std::time::Duration;
use surge_core::config::{BackoffStrategy, RetryPolicy};

/// Calculate the delay before the next retry attempt.
///
/// # Arguments
///
/// * `policy` - The retry policy configuration
/// * `attempt` - The current retry attempt number (0-indexed)
///
/// # Returns
///
/// The delay duration before the next retry, respecting `max_delay_ms`.
///
/// # Examples
///
/// ```
/// use surge_core::config::{BackoffStrategy, RetryPolicy};
/// use surge_orchestrator::retry::calculate_delay;
///
/// let policy = RetryPolicy {
///     max_retries: 3,
///     initial_delay_ms: 1000,
///     max_delay_ms: 60000,
///     backoff_strategy: BackoffStrategy::Exponential,
///     jitter_factor: 0.0,
/// };
///
/// let delay = calculate_delay(&policy, 0);
/// assert_eq!(delay.as_millis(), 1000);
/// ```
#[must_use]
pub fn calculate_delay(policy: &RetryPolicy, attempt: u32) -> Duration {
    let base_delay_ms = match policy.backoff_strategy {
        BackoffStrategy::Linear => {
            // Linear: delay = initial_delay * (attempt + 1)
            policy.initial_delay_ms.saturating_mul((attempt + 1) as u64)
        },
        BackoffStrategy::Exponential | BackoffStrategy::ExponentialWithJitter => {
            // Exponential: delay = initial_delay * 2^attempt
            policy
                .initial_delay_ms
                .saturating_mul(2_u64.saturating_pow(attempt))
        },
    };

    // Cap at max_delay_ms
    let capped_delay_ms = base_delay_ms.min(policy.max_delay_ms);

    // Apply jitter if using ExponentialWithJitter
    let final_delay_ms = if matches!(
        policy.backoff_strategy,
        BackoffStrategy::ExponentialWithJitter
    ) {
        apply_jitter(capped_delay_ms, policy.jitter_factor)
    } else {
        capped_delay_ms
    };

    Duration::from_millis(final_delay_ms)
}

/// Apply random jitter to a delay value.
///
/// Jitter helps prevent thundering herd problems by randomizing retry times.
/// The jitter factor determines the randomization range:
/// - 0.0 = no jitter (delay unchanged)
/// - 0.25 = ±25% randomization
/// - 1.0 = full randomization (0 to 2x delay)
///
/// # Arguments
///
/// * `delay_ms` - The base delay in milliseconds
/// * `jitter_factor` - The jitter factor (0.0 to 1.0)
///
/// # Returns
///
/// The delay with jitter applied, in milliseconds.
///
/// # Examples
///
/// ```
/// use surge_orchestrator::retry::apply_jitter;
///
/// let delay = 1000;
/// let jittered = apply_jitter(delay, 0.25);
/// // Result will be between 750 and 1250
/// assert!(jittered >= 750 && jittered <= 1250);
/// ```
#[must_use]
pub fn apply_jitter(delay_ms: u64, jitter_factor: f64) -> u64 {
    if jitter_factor <= 0.0 {
        return delay_ms;
    }

    let jitter_factor = jitter_factor.clamp(0.0, 1.0);
    let delay_f64 = delay_ms as f64;

    // Calculate jitter range: delay * (1 ± jitter_factor)
    let jitter_range = delay_f64 * jitter_factor;
    let min_delay = delay_f64 - jitter_range;
    let max_delay = delay_f64 + jitter_range;

    // Generate random value in range
    let mut rng = rand::rng();
    let jittered = rng.random_range(min_delay..=max_delay);

    // Ensure result is at least 0
    jittered.max(0.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_backoff() {
        let policy = RetryPolicy {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 60000,
            backoff_strategy: BackoffStrategy::Linear,
            jitter_factor: 0.0,
        };

        // attempt 0: 1000 * 1 = 1000
        assert_eq!(calculate_delay(&policy, 0).as_millis(), 1000);
        // attempt 1: 1000 * 2 = 2000
        assert_eq!(calculate_delay(&policy, 1).as_millis(), 2000);
        // attempt 2: 1000 * 3 = 3000
        assert_eq!(calculate_delay(&policy, 2).as_millis(), 3000);
    }

    #[test]
    fn test_exponential_backoff() {
        let policy = RetryPolicy {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 60000,
            backoff_strategy: BackoffStrategy::Exponential,
            jitter_factor: 0.0,
        };

        // attempt 0: 1000 * 2^0 = 1000
        assert_eq!(calculate_delay(&policy, 0).as_millis(), 1000);
        // attempt 1: 1000 * 2^1 = 2000
        assert_eq!(calculate_delay(&policy, 1).as_millis(), 2000);
        // attempt 2: 1000 * 2^2 = 4000
        assert_eq!(calculate_delay(&policy, 2).as_millis(), 4000);
        // attempt 3: 1000 * 2^3 = 8000
        assert_eq!(calculate_delay(&policy, 3).as_millis(), 8000);
    }

    #[test]
    fn test_exponential_backoff_respects_max_delay() {
        let policy = RetryPolicy {
            max_retries: 10,
            initial_delay_ms: 1000,
            max_delay_ms: 5000,
            backoff_strategy: BackoffStrategy::Exponential,
            jitter_factor: 0.0,
        };

        // attempt 0: 1000
        assert_eq!(calculate_delay(&policy, 0).as_millis(), 1000);
        // attempt 1: 2000
        assert_eq!(calculate_delay(&policy, 1).as_millis(), 2000);
        // attempt 2: 4000
        assert_eq!(calculate_delay(&policy, 2).as_millis(), 4000);
        // attempt 3: 8000, capped to 5000
        assert_eq!(calculate_delay(&policy, 3).as_millis(), 5000);
        // attempt 4: 16000, capped to 5000
        assert_eq!(calculate_delay(&policy, 4).as_millis(), 5000);
    }

    #[test]
    fn test_linear_backoff_respects_max_delay() {
        let policy = RetryPolicy {
            max_retries: 10,
            initial_delay_ms: 1000,
            max_delay_ms: 2500,
            backoff_strategy: BackoffStrategy::Linear,
            jitter_factor: 0.0,
        };

        // attempt 0: 1000
        assert_eq!(calculate_delay(&policy, 0).as_millis(), 1000);
        // attempt 1: 2000
        assert_eq!(calculate_delay(&policy, 1).as_millis(), 2000);
        // attempt 2: 3000, capped to 2500
        assert_eq!(calculate_delay(&policy, 2).as_millis(), 2500);
        // attempt 3: 4000, capped to 2500
        assert_eq!(calculate_delay(&policy, 3).as_millis(), 2500);
    }

    #[test]
    fn test_exponential_with_jitter() {
        let policy = RetryPolicy {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 60000,
            backoff_strategy: BackoffStrategy::ExponentialWithJitter,
            jitter_factor: 0.25,
        };

        // Run multiple times to verify jitter range
        for _ in 0..10 {
            let delay = calculate_delay(&policy, 0).as_millis();
            // With 25% jitter, should be between 750 and 1250
            assert!(
                delay >= 750 && delay <= 1250,
                "delay {} out of range",
                delay
            );
        }

        for _ in 0..10 {
            let delay = calculate_delay(&policy, 1).as_millis();
            // attempt 1: 2000 ± 25% = 1500 to 2500
            assert!(
                delay >= 1500 && delay <= 2500,
                "delay {} out of range",
                delay
            );
        }
    }

    #[test]
    fn test_apply_jitter_no_jitter() {
        let delay = 1000;
        let jittered = apply_jitter(delay, 0.0);
        assert_eq!(jittered, 1000);
    }

    #[test]
    fn test_apply_jitter_25_percent() {
        let delay = 1000;
        // Run multiple times to verify range
        for _ in 0..20 {
            let jittered = apply_jitter(delay, 0.25);
            // Should be between 750 and 1250
            assert!(
                jittered >= 750 && jittered <= 1250,
                "jittered {} out of range",
                jittered
            );
        }
    }

    #[test]
    fn test_apply_jitter_full_jitter() {
        let delay = 1000;
        // Run multiple times to verify range
        for _ in 0..20 {
            let jittered = apply_jitter(delay, 1.0);
            // With full jitter, should be between 0 and 2000
            assert!(jittered <= 2000, "jittered {} out of range", jittered);
        }
    }

    #[test]
    fn test_apply_jitter_negative_becomes_zero() {
        // Jitter factor clamped to 0.0
        let delay = 1000;
        let jittered = apply_jitter(delay, -0.5);
        assert_eq!(jittered, 1000);
    }

    #[test]
    fn test_apply_jitter_above_one_clamped() {
        let delay = 1000;
        // Jitter factor > 1.0 should be clamped to 1.0
        let jittered = apply_jitter(delay, 2.0);
        // Should be same as jitter_factor = 1.0
        assert!(jittered <= 2000, "jittered {} out of range", jittered);
    }

    #[test]
    fn test_overflow_handling_exponential() {
        let policy = RetryPolicy {
            max_retries: 100,
            initial_delay_ms: 1000,
            max_delay_ms: u64::MAX,
            backoff_strategy: BackoffStrategy::Exponential,
            jitter_factor: 0.0,
        };

        // Attempt 64 would overflow u64, should saturate
        let delay = calculate_delay(&policy, 64);
        assert!(delay.as_millis() > 0);
    }

    #[test]
    fn test_overflow_handling_linear() {
        let policy = RetryPolicy {
            max_retries: 100,
            initial_delay_ms: u64::MAX,
            max_delay_ms: u64::MAX,
            backoff_strategy: BackoffStrategy::Linear,
            jitter_factor: 0.0,
        };

        // Should saturate, not panic
        let delay = calculate_delay(&policy, 10);
        assert_eq!(delay.as_millis(), u64::MAX as u128);
    }
}
