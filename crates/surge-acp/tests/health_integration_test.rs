//! Integration tests for agent health monitoring and heartbeat detection.
//!
//! Tests verify:
//! - Heartbeat monitoring detects agent connection failures
//! - AgentHeartbeatFailed events are emitted
//! - Health status transitions from Healthy → Degraded → Offline
//! - Health tracker correctly records consecutive heartbeat failures

use std::collections::HashMap;
use std::time::Duration;
use surge_acp::client::PermissionPolicy;
use surge_acp::pool::AgentPool;
use surge_core::config::{AgentConfig, ResilienceConfig, Transport};
use surge_core::SurgeEvent;
use tokio::time::timeout;

/// Helper to create a test ResilienceConfig with short heartbeat intervals for testing.
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
        // Use very short intervals for testing
        heartbeat_interval_active_secs: 1, // 1 second for fast testing
        heartbeat_interval_idle_secs: 2,    // 2 seconds for idle
        reconnect_max_attempts: 3,
        reconnect_initial_delay_ms: 100,
    }
}

/// Helper to create a basic stdio AgentConfig for testing.
fn test_agent_config(command: &str, args: Vec<&str>) -> AgentConfig {
    AgentConfig {
        command: command.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        transport: Transport::Stdio,
        mcp_servers: vec![],
    }
}

#[tokio::test]
async fn test_heartbeat_detects_nonexistent_agent() {
    // Test that heartbeat monitoring detects when an agent doesn't exist

    let temp_dir = std::env::temp_dir().join("surge_test_heartbeat_nonexistent");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    // Create a config with a command that doesn't exist
    let mut configs = HashMap::new();
    configs.insert(
        "nonexistent-agent".to_string(),
        test_agent_config("nonexistent-command-xyz-123", vec![]),
    );

    let resilience = test_resilience_config();
    let pool = AgentPool::new(
        configs,
        "nonexistent-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    // Subscribe to events
    let mut event_rx = pool.subscribe();

    // Wait for heartbeat failures to be detected
    // With 1 second interval, we should see failures within a few seconds
    let mut heartbeat_failed_count = 0;
    let mut health_degraded_seen = false;

    // Wait up to 10 seconds for events
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(2), event_rx.recv()).await {
            Ok(Ok(event)) => match event {
                SurgeEvent::AgentHeartbeatFailed {
                    agent_name,
                    consecutive_failures,
                } => {
                    assert_eq!(agent_name, "nonexistent-agent");
                    heartbeat_failed_count += 1;
                    // After 3 consecutive failures, we should see degraded event
                    if consecutive_failures >= 3 {
                        health_degraded_seen = true;
                    }
                }
                SurgeEvent::AgentHealthDegraded { agent_name, .. } => {
                    assert_eq!(agent_name, "nonexistent-agent");
                    health_degraded_seen = true;
                }
                _ => {} // Ignore other events
            },
            Ok(Err(_)) => break, // Channel closed
            Err(_) => {
                // Timeout waiting for event - check if we have enough
                if heartbeat_failed_count >= 3 && health_degraded_seen {
                    break;
                }
            }
        }
    }

    // Verify we saw heartbeat failures
    assert!(
        heartbeat_failed_count >= 3,
        "Expected at least 3 heartbeat failures, got {}",
        heartbeat_failed_count
    );

    // Verify we saw health degraded event
    assert!(
        health_degraded_seen,
        "Expected AgentHealthDegraded event to be emitted"
    );

    // Verify health status is Offline
    let health_tracker = pool.health().lock().await;
    let agent_health = health_tracker
        .get_health("nonexistent-agent")
        .expect("Agent should be registered");

    // Should have Offline status after 3+ consecutive failures
    assert_eq!(
        agent_health.status(),
        surge_acp::health::HealthStatus::Offline,
        "Agent should be Offline after consecutive heartbeat failures"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_heartbeat_detects_terminated_process() {
    // Test that heartbeat monitoring detects when a process terminates

    let temp_dir = std::env::temp_dir().join("surge_test_heartbeat_terminated");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    // Create a config with a command that exits immediately
    let mut configs = HashMap::new();

    #[cfg(unix)]
    let config = test_agent_config("sh", vec!["-c", "sleep 0.1 && exit 0"]);

    #[cfg(windows)]
    let config = test_agent_config("cmd", vec!["/C", "timeout /t 1 /nobreak >nul && exit 0"]);

    configs.insert("short-lived-agent".to_string(), config);

    let resilience = test_resilience_config();
    let pool = AgentPool::new(
        configs,
        "short-lived-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    // Subscribe to events
    let mut event_rx = pool.subscribe();

    // Try to connect to the agent (it will spawn and then exit)
    let connect_result = pool.get_or_connect("short-lived-agent").await;
    // Connection might succeed initially but process will exit quickly
    let _ = connect_result;

    // Wait for heartbeat failures to be detected after the process exits
    let mut heartbeat_failed_count = 0;
    let mut health_degraded_seen = false;

    // Wait up to 15 seconds for events (process needs time to exit and heartbeats to fail)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(2), event_rx.recv()).await {
            Ok(Ok(event)) => match event {
                SurgeEvent::AgentHeartbeatFailed {
                    agent_name,
                    consecutive_failures,
                } => {
                    assert_eq!(agent_name, "short-lived-agent");
                    heartbeat_failed_count += 1;
                    if consecutive_failures >= 3 {
                        health_degraded_seen = true;
                    }
                }
                SurgeEvent::AgentHealthDegraded { agent_name, .. } => {
                    assert_eq!(agent_name, "short-lived-agent");
                    health_degraded_seen = true;
                }
                _ => {} // Ignore other events
            },
            Ok(Err(_)) => break, // Channel closed
            Err(_) => {
                // Timeout - check if we have enough
                if heartbeat_failed_count >= 3 && health_degraded_seen {
                    break;
                }
            }
        }
    }

    // Verify we saw heartbeat failures
    assert!(
        heartbeat_failed_count >= 3,
        "Expected at least 3 heartbeat failures after process exit, got {}",
        heartbeat_failed_count
    );

    // Verify we saw health degraded event
    assert!(
        health_degraded_seen,
        "Expected AgentHealthDegraded event after process exit"
    );

    // Verify health status is Offline
    let health_tracker = pool.health().lock().await;
    let agent_health = health_tracker
        .get_health("short-lived-agent")
        .expect("Agent should be registered");

    assert_eq!(
        agent_health.status(),
        surge_acp::health::HealthStatus::Offline,
        "Agent should be Offline after process terminated"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_heartbeat_consecutive_failure_count() {
    // Test that consecutive heartbeat failures are counted correctly

    let temp_dir = std::env::temp_dir().join("surge_test_heartbeat_consecutive");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let mut configs = HashMap::new();
    configs.insert(
        "test-agent".to_string(),
        test_agent_config("nonexistent-xyz", vec![]),
    );

    let resilience = test_resilience_config();
    let pool = AgentPool::new(
        configs,
        "test-agent".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    // Subscribe to events
    let mut event_rx = pool.subscribe();

    // Track consecutive failures seen in events
    let mut max_consecutive_seen = 0u32;

    // Wait for several heartbeat cycles
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(500), event_rx.recv()).await {
            Ok(Ok(SurgeEvent::AgentHeartbeatFailed {
                consecutive_failures,
                ..
            })) => {
                max_consecutive_seen = max_consecutive_seen.max(consecutive_failures);
                // After we see 3+ consecutive failures, we can exit
                if consecutive_failures >= 3 {
                    break;
                }
            }
            Ok(Ok(_)) => {} // Ignore other events
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    // Verify consecutive failures increment correctly
    assert!(
        max_consecutive_seen >= 3,
        "Expected to see at least 3 consecutive failures, got {}",
        max_consecutive_seen
    );

    // Verify the health tracker shows correct consecutive failure count
    let health_tracker = pool.health().lock().await;
    let agent_health = health_tracker
        .get_health("test-agent")
        .expect("Agent should be registered");

    assert!(
        agent_health.consecutive_heartbeat_failures >= 3,
        "Health tracker should show at least 3 consecutive failures, got {}",
        agent_health.consecutive_heartbeat_failures
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_multiple_agents_heartbeat_monitoring() {
    // Test that heartbeat monitoring works correctly with multiple agents

    let temp_dir = std::env::temp_dir().join("surge_test_heartbeat_multiple");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let mut configs = HashMap::new();
    configs.insert(
        "agent-1".to_string(),
        test_agent_config("nonexistent-1", vec![]),
    );
    configs.insert(
        "agent-2".to_string(),
        test_agent_config("nonexistent-2", vec![]),
    );

    let resilience = test_resilience_config();
    let pool = AgentPool::new(
        configs,
        "agent-1".to_string(),
        temp_dir.clone(),
        PermissionPolicy::AutoApprove,
        resilience,
    )
    .expect("Failed to create pool");

    // Subscribe to events
    let mut event_rx = pool.subscribe();

    // Track which agents we've seen failures for
    let mut agent1_failed = false;
    let mut agent2_failed = false;

    // Wait for heartbeat failures from both agents
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(1), event_rx.recv()).await {
            Ok(Ok(SurgeEvent::AgentHeartbeatFailed { agent_name, .. })) => {
                if agent_name == "agent-1" {
                    agent1_failed = true;
                } else if agent_name == "agent-2" {
                    agent2_failed = true;
                }
                // If we've seen both, we can exit early
                if agent1_failed && agent2_failed {
                    break;
                }
            }
            Ok(Ok(_)) => {} // Ignore other events
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    // Verify both agents had heartbeat failures detected
    assert!(
        agent1_failed,
        "Expected heartbeat failures for agent-1"
    );
    assert!(
        agent2_failed,
        "Expected heartbeat failures for agent-2"
    );

    // Verify both agents are tracked in health tracker
    let health_tracker = pool.health().lock().await;
    assert!(
        health_tracker.get_health("agent-1").is_some(),
        "agent-1 should be registered in health tracker"
    );
    assert!(
        health_tracker.get_health("agent-2").is_some(),
        "agent-2 should be registered in health tracker"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}
