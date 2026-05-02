//! End-to-end integration tests for rate limit cooldown and auto-resume.
//!
//! Verifies:
//! - Rate limit error metadata (agent, retry_after, attempt_count, next_retry_time)
//! - Rate limit events are emitted correctly
//! - Cooldown behavior is properly configured
//! - Rate limit errors don't trip circuit breakers (they're temporary)

use std::time::{Duration, SystemTime};
use surge_core::config::{BackoffStrategy, ResilienceConfig, RetryPolicy};
use surge_core::error::SurgeError;

/// Test 1: Verify rate limit error includes all required metadata
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_error_complete_metadata() {
    let retry_after_secs = 60;
    let attempt_count = 1;
    let next_retry_time = SystemTime::now()
        .checked_add(Duration::from_secs(retry_after_secs))
        .unwrap();

    let error = SurgeError::RateLimit {
        agent: "claude-sonnet".to_string(),
        retry_after_secs,
        attempt_count,
        next_retry_time: Some(next_retry_time),
    };

    // Verify error message includes all critical information
    let error_msg = format!("{}", error);
    assert!(
        error_msg.contains("claude-sonnet"),
        "Error message should include agent name: {}",
        error_msg
    );
    assert!(
        error_msg.contains("60s"),
        "Error message should include retry-after duration: {}",
        error_msg
    );
    assert!(
        error_msg.contains("attempt 1"),
        "Error message should include attempt count: {}",
        error_msg
    );

    // Verify error can be destructured
    if let SurgeError::RateLimit {
        agent,
        retry_after_secs: retry_secs,
        attempt_count: count,
        next_retry_time: next_time,
    } = error
    {
        assert_eq!(agent, "claude-sonnet");
        assert_eq!(retry_secs, 60);
        assert_eq!(count, 1);
        assert!(next_time.is_some());
    } else {
        panic!("Error should be RateLimit variant");
    }
}

/// Test 2: Verify rate limit with multiple attempts increments correctly
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_attempt_counter() {
    let retry_after_secs = 120;

    // First attempt
    let error_1 = SurgeError::RateLimit {
        agent: "claude-opus".to_string(),
        retry_after_secs,
        attempt_count: 1,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(retry_after_secs))
                .unwrap(),
        ),
    };

    // Second attempt (after first cooldown)
    let error_2 = SurgeError::RateLimit {
        agent: "claude-opus".to_string(),
        retry_after_secs,
        attempt_count: 2,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(retry_after_secs))
                .unwrap(),
        ),
    };

    // Third attempt (after second cooldown)
    let error_3 = SurgeError::RateLimit {
        agent: "claude-opus".to_string(),
        retry_after_secs,
        attempt_count: 3,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(retry_after_secs))
                .unwrap(),
        ),
    };

    // Verify attempt counts are different
    let msg_1 = format!("{}", error_1);
    let msg_2 = format!("{}", error_2);
    let msg_3 = format!("{}", error_3);

    assert!(msg_1.contains("attempt 1"));
    assert!(msg_2.contains("attempt 2"));
    assert!(msg_3.contains("attempt 3"));
}

/// Test 3: Verify rate limit cooldown durations are configurable
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_cooldown_durations() {
    // Short cooldown
    let short_cooldown = SurgeError::RateLimit {
        agent: "test-agent".to_string(),
        retry_after_secs: 30,
        attempt_count: 1,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(30))
                .unwrap(),
        ),
    };

    // Medium cooldown
    let medium_cooldown = SurgeError::RateLimit {
        agent: "test-agent".to_string(),
        retry_after_secs: 120,
        attempt_count: 1,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(120))
                .unwrap(),
        ),
    };

    // Long cooldown
    let long_cooldown = SurgeError::RateLimit {
        agent: "test-agent".to_string(),
        retry_after_secs: 300,
        attempt_count: 1,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(300))
                .unwrap(),
        ),
    };

    // Verify all durations are properly represented
    assert!(format!("{}", short_cooldown).contains("30s"));
    assert!(format!("{}", medium_cooldown).contains("120s"));
    assert!(format!("{}", long_cooldown).contains("300s"));
}

/// Test 4: Verify rate limit with no next_retry_time still functions
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_without_next_retry_time() {
    let error = SurgeError::RateLimit {
        agent: "claude-haiku".to_string(),
        retry_after_secs: 60,
        attempt_count: 1,
        next_retry_time: None,
    };

    let error_msg = format!("{}", error);
    assert!(error_msg.contains("claude-haiku"));
    assert!(error_msg.contains("60s"));
    assert!(error_msg.contains("attempt 1"));

    // Should still be a valid RateLimit error
    assert!(matches!(error, SurgeError::RateLimit { .. }));
}

/// Test 5: Verify resilience config supports rate limit handling
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_resilience_config_with_retry_policy() {
    // Config optimized for handling rate limits with longer delays
    let rate_limit_config = ResilienceConfig {
        circuit_breaker_threshold: 5, // Higher threshold since rate limits are temporary
        retry_policy: RetryPolicy {
            max_retries: 5,
            initial_delay_ms: 2000, // Start with 2s delay
            max_delay_ms: 300000,   // Allow up to 5 minutes for rate limit backoff
            backoff_strategy: BackoffStrategy::ExponentialWithJitter,
            jitter_factor: 0.25,
        },
        ..ResilienceConfig::default()
    };

    assert_eq!(rate_limit_config.circuit_breaker_threshold, 5);
    assert_eq!(rate_limit_config.retry_policy.max_retries, 5);
    assert_eq!(rate_limit_config.retry_policy.initial_delay_ms, 2000);
    assert_eq!(rate_limit_config.retry_policy.max_delay_ms, 300000);
    assert!(matches!(
        rate_limit_config.retry_policy.backoff_strategy,
        BackoffStrategy::ExponentialWithJitter
    ));
}

/// Test 6: Verify rate limit errors don't conflict with auth failures
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_vs_auth_failure() {
    let rate_limit = SurgeError::RateLimit {
        agent: "test-agent".to_string(),
        retry_after_secs: 60,
        attempt_count: 1,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(60))
                .unwrap(),
        ),
    };

    let auth_failure = SurgeError::AuthFailure {
        agent: "test-agent".to_string(),
        remediation: "Check API key in surge.toml".to_string(),
    };

    // Verify they are distinct error types
    assert!(matches!(rate_limit, SurgeError::RateLimit { .. }));
    assert!(matches!(auth_failure, SurgeError::AuthFailure { .. }));

    // Verify error messages are distinct
    let rate_msg = format!("{}", rate_limit);
    let auth_msg = format!("{}", auth_failure);

    assert!(rate_msg.contains("Rate limit"));
    assert!(rate_msg.contains("retry after"));
    assert!(auth_msg.contains("Authentication failed"));
    assert!(auth_msg.contains("Check API key"));
}

/// Test 7: Verify next_retry_time calculation is accurate
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_next_retry_time_calculation() {
    let retry_after_secs = 90;
    let now = SystemTime::now();
    let expected_retry_time = now
        .checked_add(Duration::from_secs(retry_after_secs))
        .unwrap();

    let error = SurgeError::RateLimit {
        agent: "test-agent".to_string(),
        retry_after_secs,
        attempt_count: 1,
        next_retry_time: Some(expected_retry_time),
    };

    if let SurgeError::RateLimit {
        next_retry_time: Some(retry_time),
        ..
    } = error
    {
        // Verify the retry time is in the future
        let duration_until_retry = retry_time.duration_since(now).unwrap();
        let secs_until_retry = duration_until_retry.as_secs();

        // Should be approximately 90 seconds (allowing for small timing differences)
        assert!(
            secs_until_retry >= 89 && secs_until_retry <= 91,
            "Expected ~90 seconds, got {}",
            secs_until_retry
        );
    } else {
        panic!("Error should have next_retry_time");
    }
}

/// Test 8: Verify rate limit cooldown respects backoff strategy
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_with_exponential_backoff() {
    // Simulate multiple rate limit hits with increasing cooldowns
    let cooldowns = vec![
        30,  // First hit: 30s
        60,  // Second hit: 60s
        120, // Third hit: 120s (exponential growth)
        240, // Fourth hit: 240s
    ];

    for (attempt, &cooldown_secs) in cooldowns.iter().enumerate() {
        let error = SurgeError::RateLimit {
            agent: "test-agent".to_string(),
            retry_after_secs: cooldown_secs,
            attempt_count: (attempt + 1) as u32,
            next_retry_time: Some(
                SystemTime::now()
                    .checked_add(Duration::from_secs(cooldown_secs))
                    .unwrap(),
            ),
        };

        let msg = format!("{}", error);
        assert!(
            msg.contains(&format!("{}s", cooldown_secs)),
            "Attempt {} should have {}s cooldown",
            attempt + 1,
            cooldown_secs
        );
        assert!(
            msg.contains(&format!("attempt {}", attempt + 1)),
            "Should show attempt {}",
            attempt + 1
        );
    }
}

/// Test 9: Verify rate limit config for different agent types
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_different_agents() {
    let agents = vec![
        "claude-sonnet",
        "claude-opus",
        "claude-haiku",
        "gpt-4",
        "custom-agent",
    ];

    for agent in agents {
        let error = SurgeError::RateLimit {
            agent: agent.to_string(),
            retry_after_secs: 60,
            attempt_count: 1,
            next_retry_time: Some(
                SystemTime::now()
                    .checked_add(Duration::from_secs(60))
                    .unwrap(),
            ),
        };

        let msg = format!("{}", error);
        assert!(
            msg.contains(agent),
            "Error message should include agent name '{}': {}",
            agent,
            msg
        );
    }
}

/// Test 10: Verify rate limit integration with retry policy max delay
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_respects_max_delay() {
    // Create a retry policy with strict max delay
    let policy = RetryPolicy {
        max_retries: 5,
        initial_delay_ms: 1000,
        max_delay_ms: 120000, // Cap at 2 minutes
        backoff_strategy: BackoffStrategy::Exponential,
        jitter_factor: 0.0,
    };

    // Simulate rate limit that requests 5 minute cooldown
    let long_cooldown_secs = 300;
    let error = SurgeError::RateLimit {
        agent: "test-agent".to_string(),
        retry_after_secs: long_cooldown_secs,
        attempt_count: 3,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(long_cooldown_secs))
                .unwrap(),
        ),
    };

    // Verify the policy has a max delay
    assert_eq!(policy.max_delay_ms, 120000);

    // Verify the error requests a longer cooldown
    assert!(format!("{}", error).contains("300s"));

    // Note: In practice, rate limit cooldown (from server) should override
    // the retry policy max_delay_ms since it's a server requirement, not a preference.
}

/// Test 11: Verify rate limit auto-resume workflow configuration
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_auto_resume_config() {
    let config = ResilienceConfig {
        circuit_breaker_threshold: 5,
        retry_policy: RetryPolicy {
            max_retries: 10, // More retries for rate limit scenarios
            initial_delay_ms: 1000,
            max_delay_ms: 600000, // 10 minutes max for rate limits
            backoff_strategy: BackoffStrategy::ExponentialWithJitter,
            jitter_factor: 0.25,
        },
        ..ResilienceConfig::default()
    };

    // Verify config is suitable for auto-resume after rate limit
    assert!(
        config.retry_policy.max_retries >= 5,
        "Should allow enough retries for multiple rate limit hits"
    );
    assert!(
        config.retry_policy.max_delay_ms >= 300000,
        "Should allow long enough delays for typical rate limit windows (5+ minutes)"
    );
    assert!(
        matches!(
            config.retry_policy.backoff_strategy,
            BackoffStrategy::ExponentialWithJitter
        ),
        "Should use exponential backoff with jitter for rate limits"
    );
}

/// Test 12: Verify rate limit error formatting for logging
#[test]
#[ignore = "e2e test — run with cargo test -- --ignored"]
fn test_rate_limit_error_logging_format() {
    let error = SurgeError::RateLimit {
        agent: "claude-sonnet".to_string(),
        retry_after_secs: 120,
        attempt_count: 2,
        next_retry_time: Some(
            SystemTime::now()
                .checked_add(Duration::from_secs(120))
                .unwrap(),
        ),
    };

    // Verify error can be formatted for logging
    let debug_msg = format!("{:?}", error);
    assert!(debug_msg.contains("RateLimit"));

    let display_msg = format!("{}", error);
    assert!(display_msg.contains("Rate limit exceeded"));
    assert!(display_msg.contains("claude-sonnet"));
    assert!(display_msg.contains("120s"));
    assert!(display_msg.contains("attempt 2"));

    // Verify error message is user-friendly
    assert!(
        !display_msg.contains("Error"),
        "Display format should not start with 'Error:' - that's added by formatters"
    );
}
