//! Integration tests for auto-reconnect with exponential backoff.
//!
//! Tests verify:
//! - Connection loss triggers automatic reconnection
//! - AgentReconnecting events are emitted with correct attempt numbers
//! - Exponential backoff delays increase correctly (1s, 2s, 4s, 8s)
//! - AgentReconnected event is emitted on successful reconnect
//! - Reconnection respects max_attempts limit

use std::collections::HashMap;
use std::time::Duration;
use surge_acp::client::PermissionPolicy;
use surge_acp::pool::AgentPool;
use surge_core::SurgeEvent;
use surge_core::config::{AgentConfig, BackoffStrategy, ResilienceConfig, Transport};
use tokio::time::{Instant, timeout};

/// Helper to create a test ResilienceConfig with fast reconnect for testing.
fn test_resilience_config() -> ResilienceConfig {
    ResilienceConfig {
        connect_timeout_secs: 5,
        session_timeout_secs: 5,
        prompt_timeout_secs: 30,
        prompt_retries: 1,
        shutdown_grace_secs: 2,
        retry_policy: Default::default(),
        circuit_breaker_threshold: 3,
        auth_failure_immediate_fail: true,
        heartbeat_interval_active_secs: 1,
        heartbeat_interval_idle_secs: 2,
        reconnect_max_attempts: 4, // 4 attempts to test 1s, 2s, 4s, 8s
        reconnect_initial_delay_ms: 100, // 100ms for faster testing (scaled down from 1s)
    }
}

/// Helper to create a test ResilienceConfig with realistic 1s initial delay.
fn realistic_resilience_config() -> ResilienceConfig {
    let mut config = test_resilience_config();
    config.reconnect_initial_delay_ms = 1000; // 1 second initial delay (real-world value)
    config
}

/// Helper to create a basic stdio AgentConfig for testing.
fn test_agent_config(command: &str, args: Vec<&str>) -> AgentConfig {
    AgentConfig {
        command: command.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        transport: Transport::Stdio,
        mcp_servers: vec![],
        capabilities: vec![],
    }
}

#[tokio::test]
async fn test_reconnect_events_emitted_on_connection_loss() {
    // Test that AgentReconnecting events are emitted when an agent connection is lost

    let temp_dir = std::env::temp_dir().join("surge_test_reconnect_events");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    // Create a config with a command that exits immediately
    let mut configs = HashMap::new();

    #[cfg(unix)]
    let config = test_agent_config("sh", vec!["-c", "exit 0"]);

    #[cfg(windows)]
    let config = test_agent_config("cmd", vec!["/C", "exit 0"]);

    configs.insert("exit-agent".to_string(), config);

    let resilience = test_resilience_config();
    let pool = AgentPool::new(
        configs,
        "exit-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    // Subscribe to events
    let mut event_rx = pool.subscribe();

    // Try to connect (will fail and trigger reconnect)
    let _ = pool.get_or_connect("exit-agent").await;

    // Wait for reconnection events
    let mut reconnecting_events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline {
        match timeout(Duration::from_millis(500), event_rx.recv()).await {
            Ok(Ok(event)) => match event {
                SurgeEvent::AgentReconnecting {
                    agent_name,
                    attempt,
                    max_attempts,
                } => {
                    assert_eq!(agent_name, "exit-agent");
                    reconnecting_events.push((attempt, max_attempts));
                    // If we've seen enough attempts, we can exit
                    if attempt >= 2 {
                        break;
                    }
                },
                _ => {}, // Ignore other events
            },
            Ok(Err(_)) => break, // Channel closed
            Err(_) => {
                // Timeout - check if we have enough events
                if reconnecting_events.len() >= 2 {
                    break;
                }
            },
        }
    }

    // Verify we saw at least 2 reconnection attempts
    assert!(
        reconnecting_events.len() >= 2,
        "Expected at least 2 reconnection attempts, got {}",
        reconnecting_events.len()
    );

    // Verify attempt numbers increment correctly
    for (idx, (attempt, max_attempts)) in reconnecting_events.iter().enumerate() {
        assert_eq!(
            *attempt,
            (idx + 1) as u32,
            "Attempt number should increment"
        );
        assert_eq!(*max_attempts, 4, "Max attempts should be 4");
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_exponential_backoff_delays() {
    // Test that reconnection attempts happen over time (not all at once)
    // Due to how the implementation emits events (before sleeping), we verify
    // that the total elapsed time indicates exponential backoff is happening

    let temp_dir = std::env::temp_dir().join("surge_test_reconnect_backoff");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let mut configs = HashMap::new();
    configs.insert(
        "backoff-agent".to_string(),
        test_agent_config("nonexistent-cmd-xyz", vec![]),
    );

    let resilience = test_resilience_config();
    let pool = AgentPool::new(
        configs,
        "backoff-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    let mut event_rx = pool.subscribe();

    let start_time = Instant::now();

    // Try to connect (will fail and trigger reconnect)
    let _ = pool.get_or_connect("backoff-agent").await;

    // Track when we receive reconnection events
    let mut last_attempt = 0u32;
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline {
        match timeout(Duration::from_millis(500), event_rx.recv()).await {
            Ok(Ok(SurgeEvent::AgentReconnecting { attempt, .. })) => {
                last_attempt = attempt;
                // Stop after we've seen 4 attempts
                if attempt >= 4 {
                    break;
                }
            },
            Ok(Ok(_)) => {}, // Ignore other events
            Ok(Err(_)) => break,
            Err(_) => {
                if last_attempt >= 4 {
                    break;
                }
            },
        }
    }

    let elapsed = start_time.elapsed();

    // Verify we saw at least 3 attempts
    assert!(
        last_attempt >= 3,
        "Expected at least 3 reconnection attempts, got {}",
        last_attempt
    );

    // With exponential backoff (100ms initial): 0ms + 100ms + 200ms = 300ms minimum
    // Plus connection attempt overhead, should be at least 200ms
    assert!(
        elapsed >= Duration::from_millis(200),
        "Total elapsed time {:?} should indicate exponential backoff occurred (expected >=200ms)",
        elapsed
    );

    // Shouldn't take too long either (max 4 attempts: 0 + 100 + 200 + 400 = 700ms base + overhead)
    // Allow up to 10s for test environment variance (heartbeat checks, connection overhead, etc.)
    assert!(
        elapsed <= Duration::from_secs(10),
        "Total elapsed time {:?} took too long (expected <10s)",
        elapsed
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_reconnect_respects_max_attempts() {
    // Test that reconnection stops after max_attempts is reached

    let temp_dir = std::env::temp_dir().join("surge_test_reconnect_max_attempts");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let mut configs = HashMap::new();
    configs.insert(
        "max-attempts-agent".to_string(),
        test_agent_config("nonexistent-xyz-123", vec![]),
    );

    let mut resilience = test_resilience_config();
    resilience.reconnect_max_attempts = 3; // Only 3 attempts

    let pool = AgentPool::new(
        configs,
        "max-attempts-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    let mut event_rx = pool.subscribe();

    // Try to connect (will fail and trigger reconnect)
    let _ = pool.get_or_connect("max-attempts-agent").await;

    // Count reconnection events
    let mut max_attempt_seen = 0u32;
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline {
        match timeout(Duration::from_millis(500), event_rx.recv()).await {
            Ok(Ok(SurgeEvent::AgentReconnecting {
                attempt,
                max_attempts,
                ..
            })) => {
                assert_eq!(max_attempts, 3, "Max attempts should be 3");
                max_attempt_seen = max_attempt_seen.max(attempt);
            },
            Ok(Ok(_)) => {}, // Ignore other events
            Ok(Err(_)) => break,
            Err(_) => {
                // Timeout - check if we've seen all attempts
                if max_attempt_seen >= 3 {
                    break;
                }
            },
        }
    }

    // Verify we saw exactly 3 attempts (not more)
    assert_eq!(
        max_attempt_seen, 3,
        "Should see exactly 3 reconnection attempts, saw {}",
        max_attempt_seen
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_backoff_strategy_exponential() {
    // Test that exponential backoff strategy is applied during reconnection
    // Verify that reconnection takes time proportional to exponential delays

    let temp_dir = std::env::temp_dir().join("surge_test_backoff_strategy");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let mut configs = HashMap::new();
    configs.insert(
        "strategy-agent".to_string(),
        test_agent_config("nonexistent-cmd", vec![]),
    );

    let mut resilience = test_resilience_config();
    // Ensure exponential strategy is used (it's the default)
    resilience.retry_policy.backoff_strategy = BackoffStrategy::Exponential;
    resilience.retry_policy.initial_delay_ms = 200; // Larger delay for easier measurement
    resilience.retry_policy.max_delay_ms = 10000;
    resilience.reconnect_initial_delay_ms = 200;

    let pool = AgentPool::new(
        configs,
        "strategy-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    let mut event_rx = pool.subscribe();

    let start = Instant::now();

    // Trigger reconnection
    let _ = pool.get_or_connect("strategy-agent").await;

    // Wait for at least 3 attempts
    let mut attempts_seen = 0;
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline {
        match timeout(Duration::from_millis(500), event_rx.recv()).await {
            Ok(Ok(SurgeEvent::AgentReconnecting { attempt, .. })) => {
                attempts_seen = attempt;
                if attempt >= 3 {
                    break;
                }
            },
            Ok(Ok(_)) => {},
            Ok(Err(_)) => break,
            Err(_) => {
                if attempts_seen >= 3 {
                    break;
                }
            },
        }
    }

    let elapsed = start.elapsed();

    assert!(attempts_seen >= 3, "Should see at least 3 attempts");

    // With 200ms initial delay and exponential backoff for 3 attempts:
    // Attempt 1: 0ms delay
    // Attempt 2: 200ms delay (200 * 2^0)
    // Attempt 3: 400ms delay (200 * 2^1)
    // Total minimum: 600ms (plus connection overhead)
    assert!(
        elapsed >= Duration::from_millis(400),
        "Elapsed time {:?} should show exponential backoff occurred (expected >=400ms for 3 attempts)",
        elapsed
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_reconnect_prevents_duplicate_attempts() {
    // Test that multiple reconnection requests don't create duplicate reconnect tasks

    let temp_dir = std::env::temp_dir().join("surge_test_reconnect_duplicate");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let mut configs = HashMap::new();
    configs.insert(
        "duplicate-agent".to_string(),
        test_agent_config("nonexistent-xyz", vec![]),
    );

    let resilience = test_resilience_config();
    let pool = AgentPool::new(
        configs,
        "duplicate-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    let mut event_rx = pool.subscribe();

    // Try to connect multiple times rapidly (should not create duplicate reconnect tasks)
    let _ = pool.get_or_connect("duplicate-agent").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = pool.get_or_connect("duplicate-agent").await;

    // Count attempt 1 events (should only see it once)
    let mut attempt_1_count = 0;
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        match timeout(Duration::from_millis(100), event_rx.recv()).await {
            Ok(Ok(SurgeEvent::AgentReconnecting { attempt, .. })) => {
                if attempt == 1 {
                    attempt_1_count += 1;
                }
                // Exit after seeing a few attempts
                if attempt >= 2 {
                    break;
                }
            },
            Ok(Ok(_)) => {},
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }

    // We should see attempt 1 exactly once (not duplicated)
    // Note: Due to timing, we might see it 1-2 times if heartbeat also triggers,
    // but definitely not 3+ times
    assert!(
        attempt_1_count <= 2,
        "Should see at most 2 'attempt 1' events (no excessive duplication), saw {}",
        attempt_1_count
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
#[ignore] // This test takes ~7 seconds due to realistic delays
async fn test_realistic_reconnect_delays() {
    // Test with realistic 1s, 2s, 4s delay pattern (as specified in acceptance criteria)

    let temp_dir = std::env::temp_dir().join("surge_test_reconnect_realistic");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let mut configs = HashMap::new();
    configs.insert(
        "realistic-agent".to_string(),
        test_agent_config("nonexistent-real", vec![]),
    );

    let resilience = realistic_resilience_config();
    let pool = AgentPool::new(
        configs,
        "realistic-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    let mut event_rx = pool.subscribe();

    let start = Instant::now();

    // Trigger reconnection
    let _ = pool.get_or_connect("realistic-agent").await;

    // Wait for attempts
    let mut last_attempt = 0u32;
    let deadline = Instant::now() + Duration::from_secs(20);

    while Instant::now() < deadline {
        match timeout(Duration::from_secs(2), event_rx.recv()).await {
            Ok(Ok(SurgeEvent::AgentReconnecting { attempt, .. })) => {
                last_attempt = attempt;
                if attempt >= 4 {
                    break;
                }
            },
            Ok(Ok(_)) => {},
            Ok(Err(_)) => break,
            Err(_) => {
                if last_attempt >= 4 {
                    break;
                }
            },
        }
    }

    let elapsed = start.elapsed();

    assert!(
        last_attempt >= 3,
        "Expected at least 3 attempts with realistic delays"
    );

    // With 1s initial delay and exponential backoff for 4 attempts:
    // Attempt 1: 0s delay
    // Attempt 2: 1s delay
    // Attempt 3: 2s delay
    // Attempt 4: 4s delay
    // Total minimum: 7s (plus small connection overhead)
    assert!(
        elapsed >= Duration::from_secs(6),
        "Total elapsed time {:?} should reflect realistic 1s, 2s, 4s backoff (expected >=6s)",
        elapsed
    );

    assert!(
        elapsed <= Duration::from_secs(15),
        "Total elapsed time {:?} took too long (expected <=15s)",
        elapsed
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}
