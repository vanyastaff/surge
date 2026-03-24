//! End-to-end integration tests for circuit breaker persistence across restarts.
//!
//! Verifies:
//! - Circuit breaker state persists to disk
//! - Circuit breaker state is restored on restart
//! - Tripped circuits remain tripped across restarts
//! - Failure counts are preserved across restarts
//! - Reset works correctly with persistence

use std::path::PathBuf;
use std::sync::Arc;
use surge_core::event::SurgeEvent;
use surge_core::id::{SubtaskId, TaskId};
use surge_orchestrator::circuit_breaker::CircuitBreaker;
use surge_persistence::models::CircuitBreakerState;
use surge_persistence::store::Store;
use tokio::sync::{Mutex, broadcast};

/// Helper to create a unique temp database file
fn temp_db_path(test_name: &str) -> PathBuf {
    let temp_dir = std::env::temp_dir();
    temp_dir.join(format!(
        "surge-cb-e2e-{}-{}.db",
        test_name,
        std::process::id()
    ))
}

/// Test 1: Circuit breaker state persists to disk and is restored on restart
#[tokio::test]
async fn test_circuit_breaker_persistence_basic() {
    let db_path = temp_db_path("basic_persistence");
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    // 1. Create store and circuit breaker, record some failures
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to create store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            3,
            Some(store.clone()),
            event_tx,
        )
        .await;

        // Record two failures (not yet tripped)
        cb.record_failure("error 1".to_string(), None).await;
        cb.record_failure("error 2".to_string(), None).await;

        assert_eq!(cb.consecutive_failures(), 2);
        assert!(!cb.is_tripped());
    }

    // 2. Simulate restart: create new circuit breaker with same IDs
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            3,
            Some(store),
            event_tx,
        )
        .await;

        // Verify state was restored
        assert_eq!(
            cb.consecutive_failures(),
            2,
            "Consecutive failures should be restored from persistence"
        );
        assert!(
            !cb.is_tripped(),
            "Circuit should not be tripped yet"
        );
        assert_eq!(
            cb.last_error(),
            Some("error 2"),
            "Last error message should be restored"
        );
    }

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test 2: Tripped circuit breaker remains tripped across restarts
#[tokio::test]
async fn test_circuit_breaker_tripped_persists() {
    let db_path = temp_db_path("tripped_persists");
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    // 1. Create circuit breaker and trip it
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to create store"),
        ));
        let (event_tx, mut rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            3,
            Some(store),
            event_tx,
        )
        .await;

        // Record failures to trip the circuit
        cb.record_failure("error 1".to_string(), None).await;
        cb.record_failure("error 2".to_string(), None).await;
        cb.record_failure("error 3".to_string(), Some(1_700_000_000_000))
            .await;

        assert_eq!(cb.consecutive_failures(), 3);
        assert!(cb.is_tripped(), "Circuit should be tripped");
        assert_eq!(cb.last_error(), Some("error 3"));
        assert_eq!(cb.next_retry_time(), Some(1_700_000_000_000));

        // Verify event was emitted
        let event = rx.try_recv().unwrap();
        assert!(
            matches!(event, SurgeEvent::CircuitBreakerOpened { .. }),
            "CircuitBreakerOpened event should be emitted"
        );
    }

    // 2. Simulate restart: create new circuit breaker with same IDs
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            3,
            Some(store),
            event_tx,
        )
        .await;

        // Verify tripped state was restored
        assert_eq!(
            cb.consecutive_failures(),
            3,
            "Consecutive failures should be restored"
        );
        assert!(
            cb.is_tripped(),
            "Circuit should remain tripped after restart"
        );
        assert_eq!(
            cb.last_error(),
            Some("error 3"),
            "Last error should be restored"
        );
        assert_eq!(
            cb.next_retry_time(),
            Some(1_700_000_000_000),
            "Next retry time should be restored"
        );
    }

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test 3: Reset clears persisted state
#[tokio::test]
async fn test_circuit_breaker_reset_persists() {
    let db_path = temp_db_path("reset_persists");
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    // 1. Create circuit breaker, record failures, then reset
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to create store"),
        ));
        let (event_tx, mut rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            3,
            Some(store),
            event_tx,
        )
        .await;

        // Record failures
        cb.record_failure("error 1".to_string(), None).await;
        cb.record_failure("error 2".to_string(), None).await;

        assert_eq!(cb.consecutive_failures(), 2);

        // Reset
        cb.reset().await;

        assert_eq!(cb.consecutive_failures(), 0);
        assert!(!cb.is_tripped());
        assert_eq!(cb.last_error(), None);

        // Verify reset event was emitted
        let event = rx.try_recv().unwrap();
        assert!(
            matches!(event, SurgeEvent::CircuitBreakerClosed { .. }),
            "CircuitBreakerClosed event should be emitted"
        );
    }

    // 2. Simulate restart: verify reset state persisted
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            3,
            Some(store),
            event_tx,
        )
        .await;

        // Verify reset state was restored
        assert_eq!(
            cb.consecutive_failures(),
            0,
            "Consecutive failures should be zero after reset"
        );
        assert!(
            !cb.is_tripped(),
            "Circuit should not be tripped after reset"
        );
        assert_eq!(
            cb.last_error(),
            None,
            "Last error should be cleared after reset"
        );
    }

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test 4: Multiple circuit breakers can persist independently
#[tokio::test]
async fn test_multiple_circuit_breakers_persist() {
    let db_path = temp_db_path("multiple_cbs");
    let task_id = TaskId::new();
    let subtask1 = SubtaskId::new();
    let subtask2 = SubtaskId::new();
    let subtask3 = SubtaskId::new();

    // 1. Create three circuit breakers with different states
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to create store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        // CB1: One failure
        let mut cb1 = CircuitBreaker::new(
            task_id,
            subtask1,
            3,
            Some(store.clone()),
            event_tx.clone(),
        )
        .await;
        cb1.record_failure("cb1 error".to_string(), None).await;

        // CB2: Tripped
        let mut cb2 = CircuitBreaker::new(
            task_id,
            subtask2,
            2,
            Some(store.clone()),
            event_tx.clone(),
        )
        .await;
        cb2.record_failure("cb2 error 1".to_string(), None).await;
        cb2.record_failure("cb2 error 2".to_string(), Some(1_800_000_000_000))
            .await;

        // CB3: Clean state (no failures)
        let _cb3 = CircuitBreaker::new(
            task_id,
            subtask3,
            3,
            Some(store),
            event_tx,
        )
        .await;

        assert_eq!(cb1.consecutive_failures(), 1);
        assert!(!cb1.is_tripped());

        assert_eq!(cb2.consecutive_failures(), 2);
        assert!(cb2.is_tripped());
    }

    // 2. Simulate restart: verify each circuit breaker restored correctly
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let cb1 = CircuitBreaker::new(
            task_id,
            subtask1,
            3,
            Some(store.clone()),
            event_tx.clone(),
        )
        .await;

        let cb2 = CircuitBreaker::new(
            task_id,
            subtask2,
            2,
            Some(store.clone()),
            event_tx.clone(),
        )
        .await;

        let cb3 = CircuitBreaker::new(
            task_id,
            subtask3,
            3,
            Some(store),
            event_tx,
        )
        .await;

        // Verify CB1 state
        assert_eq!(
            cb1.consecutive_failures(),
            1,
            "CB1 should have 1 failure"
        );
        assert!(!cb1.is_tripped(), "CB1 should not be tripped");
        assert_eq!(cb1.last_error(), Some("cb1 error"));

        // Verify CB2 state
        assert_eq!(
            cb2.consecutive_failures(),
            2,
            "CB2 should have 2 failures"
        );
        assert!(cb2.is_tripped(), "CB2 should be tripped");
        assert_eq!(cb2.last_error(), Some("cb2 error 2"));
        assert_eq!(cb2.next_retry_time(), Some(1_800_000_000_000));

        // Verify CB3 state
        assert_eq!(
            cb3.consecutive_failures(),
            0,
            "CB3 should have 0 failures"
        );
        assert!(!cb3.is_tripped(), "CB3 should not be tripped");
        assert_eq!(cb3.last_error(), None);
    }

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test 5: Circuit breaker state updates incrementally
#[tokio::test]
async fn test_circuit_breaker_incremental_updates() {
    let db_path = temp_db_path("incremental_updates");
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    // 1. Create circuit breaker and record failure
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to create store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            5,
            Some(store),
            event_tx,
        )
        .await;

        cb.record_failure("error 1".to_string(), None).await;
        assert_eq!(cb.consecutive_failures(), 1);
    }

    // 2. Restart and add another failure
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            5,
            Some(store),
            event_tx,
        )
        .await;

        assert_eq!(cb.consecutive_failures(), 1);

        cb.record_failure("error 2".to_string(), None).await;
        assert_eq!(cb.consecutive_failures(), 2);
    }

    // 3. Restart and add more failures until tripped
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            5,
            Some(store),
            event_tx,
        )
        .await;

        assert_eq!(cb.consecutive_failures(), 2);

        cb.record_failure("error 3".to_string(), None).await;
        cb.record_failure("error 4".to_string(), None).await;
        cb.record_failure("error 5".to_string(), None).await;

        assert_eq!(cb.consecutive_failures(), 5);
        assert!(cb.is_tripped());
    }

    // 4. Final restart: verify tripped state
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            5,
            Some(store),
            event_tx,
        )
        .await;

        assert_eq!(cb.consecutive_failures(), 5);
        assert!(cb.is_tripped());
        assert_eq!(cb.last_error(), Some("error 5"));
    }

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test 6: Circuit breaker without persistence store still works
#[tokio::test]
async fn test_circuit_breaker_without_persistence() {
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    // Create circuit breaker without store
    let (event_tx, _rx) = broadcast::channel(10);
    let mut cb = CircuitBreaker::new(task_id, subtask_id, 3, None, event_tx).await;

    // Record failures
    cb.record_failure("error 1".to_string(), None).await;
    cb.record_failure("error 2".to_string(), None).await;
    cb.record_failure("error 3".to_string(), None).await;

    // Circuit breaker should still work
    assert_eq!(cb.consecutive_failures(), 3);
    assert!(cb.is_tripped());
    assert_eq!(cb.last_error(), Some("error 3"));
}

/// Test 7: Direct store operations for circuit breaker state
#[tokio::test]
async fn test_store_circuit_breaker_operations() {
    let db_path = temp_db_path("store_operations");
    let mut store = Store::open(&db_path).expect("Failed to create store");
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    // Create and save state
    let mut state = CircuitBreakerState::new(task_id, subtask_id);
    state.record_failure("test error".to_string(), Some(1_700_000_000_000));

    store
        .save_circuit_breaker_state(&state)
        .expect("Failed to save state");

    // Load state
    let loaded = store
        .load_circuit_breaker_state(task_id, subtask_id)
        .expect("Failed to load state")
        .expect("State should exist");

    assert_eq!(loaded.task_id, task_id);
    assert_eq!(loaded.subtask_id, subtask_id);
    assert_eq!(loaded.consecutive_failures, 1);
    assert_eq!(loaded.last_error, Some("test error".to_string()));
    assert_eq!(loaded.next_retry_time, Some(1_700_000_000_000));

    // Update state
    state.record_failure("second error".to_string(), None);
    state.trip(1_700_000_100_000);

    store
        .save_circuit_breaker_state(&state)
        .expect("Failed to update state");

    // Load updated state
    let updated = store
        .load_circuit_breaker_state(task_id, subtask_id)
        .expect("Failed to load updated state")
        .expect("State should exist");

    assert_eq!(updated.consecutive_failures, 2);
    assert_eq!(updated.last_error, Some("second error".to_string()));
    assert!(updated.is_tripped());
    assert_eq!(updated.tripped_at, Some(1_700_000_100_000));

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test 8: Circuit breaker with different thresholds persists correctly
#[tokio::test]
async fn test_circuit_breaker_different_thresholds() {
    let db_path = temp_db_path("different_thresholds");
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    // 1. Create circuit breaker with threshold 2
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to create store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            2, // Low threshold
            Some(store),
            event_tx,
        )
        .await;

        cb.record_failure("error 1".to_string(), None).await;
        assert!(!cb.is_tripped());
    }

    // 2. Restart with same threshold, trip the circuit
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            2,
            Some(store),
            event_tx,
        )
        .await;

        assert_eq!(cb.consecutive_failures(), 1);

        cb.record_failure("error 2".to_string(), None).await;
        assert!(cb.is_tripped(), "Should trip at threshold 2");
        assert_eq!(cb.consecutive_failures(), 2);
    }

    // 3. Restart with higher threshold - circuit should still be tripped
    //    (threshold is for new failures, not for checking existing state)
    {
        let store = Arc::new(Mutex::new(
            Store::open(&db_path).expect("Failed to reopen store"),
        ));
        let (event_tx, _rx) = broadcast::channel(10);

        let cb = CircuitBreaker::new(
            task_id,
            subtask_id,
            10, // Higher threshold
            Some(store),
            event_tx,
        )
        .await;

        assert_eq!(cb.consecutive_failures(), 2);
        assert!(
            cb.is_tripped(),
            "Circuit should remain tripped regardless of new threshold"
        );
    }

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}
