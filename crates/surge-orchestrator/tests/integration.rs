//! Integration tests for end-to-end token tracking.
//!
//! Verifies that TokensConsumed events are properly aggregated from the event
//! system into the persistence layer, with correct session → subtask → spec
//! aggregation and cost calculations.

use std::path::PathBuf;
use surge_core::event::SurgeEvent;
use surge_core::id::{SpecId, SubtaskId, TaskId};
use surge_persistence::aggregator::{SessionContext, UsageAggregator};
use surge_persistence::store::Store;
use tokio::sync::broadcast;

/// Helper to create a unique temp database file
fn temp_db_path(test_name: &str) -> PathBuf {
    let temp_dir = std::env::temp_dir();
    temp_dir.join(format!(
        "surge-test-{}-{}.db",
        test_name,
        std::process::id()
    ))
}

/// Test that TokensConsumed events are aggregated into the store end-to-end.
#[tokio::test]
async fn test_token_tracking_end_to_end() {
    let db_path = temp_db_path("end_to_end");

    // 1. Create store
    let store = Store::open(&db_path).expect("Failed to create store");

    // 2. Create aggregator
    let aggregator = UsageAggregator::new(store);

    // 3. Create broadcast channel
    let (tx, rx) = broadcast::channel(100);

    // 4. Start background listener
    let _handle = aggregator.start_listening(rx);

    // 5. Register session with context
    let session_id = "test-session-1".to_string();
    let spec_id = SpecId::new();
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    let context = SessionContext {
        task_id,
        subtask_id: Some(subtask_id),
        spec_id,
    };

    aggregator
        .register_session(session_id.clone(), context)
        .await;

    // 6. Send TokensConsumed event
    let event = SurgeEvent::TokensConsumed {
        session_id: session_id.clone(),
        agent_name: "claude-sonnet".to_string(),
        spec_id: Some(spec_id),
        subtask_id: Some(subtask_id),
        input_tokens: 1000,
        output_tokens: 500,
        thought_tokens: Some(200),
        cached_read_tokens: Some(100),
        cached_write_tokens: Some(50),
        estimated_cost_usd: Some(0.015),
    };

    tx.send(event).expect("Failed to send event");

    // 7. Wait for aggregation (small delay for async processing)
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 8. Verify data in store by opening a new connection to the same DB file
    let verify_store = Store::open(&db_path).expect("Failed to open store for verification");

    // Check session record
    let sessions = verify_store
        .list_sessions_by_spec(spec_id)
        .expect("Failed to list sessions");
    assert_eq!(sessions.len(), 1, "Expected 1 session record");

    let session = &sessions[0];
    assert_eq!(session.session_id, session_id);
    assert_eq!(session.agent_name, "claude-sonnet");
    assert_eq!(session.input_tokens, 1000);
    assert_eq!(session.output_tokens, 500);
    assert_eq!(session.thought_tokens, Some(200));
    assert_eq!(session.cached_read_tokens, Some(100));
    assert_eq!(session.cached_write_tokens, Some(50));
    assert_eq!(session.estimated_cost_usd, Some(0.015));

    // Check subtask record
    let subtask = verify_store
        .get_subtask(subtask_id, task_id, spec_id)
        .expect("Failed to get subtask")
        .expect("Subtask should exist");

    assert_eq!(subtask.session_count, 1);
    assert_eq!(subtask.input_tokens, 1000);
    assert_eq!(subtask.output_tokens, 500);
    assert_eq!(subtask.thought_tokens, 200);
    assert_eq!(subtask.cached_read_tokens, 100);
    assert_eq!(subtask.cached_write_tokens, 50);
    assert_eq!(subtask.estimated_cost_usd, 0.015);

    // Check spec record
    let spec = verify_store
        .get_spec(spec_id)
        .expect("Failed to get spec")
        .expect("Spec should exist");

    assert_eq!(spec.session_count, 1);
    assert_eq!(spec.input_tokens, 1000);
    assert_eq!(spec.output_tokens, 500);
    assert_eq!(spec.thought_tokens, 200);
    assert_eq!(spec.cached_read_tokens, 100);
    assert_eq!(spec.cached_write_tokens, 50);
    assert_eq!(spec.estimated_cost_usd, 0.015);

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test that sessions without subtask_id are handled correctly.
#[tokio::test]
async fn test_session_without_subtask() {
    let db_path = temp_db_path("no_subtask");

    // 1. Create store
    let store = Store::open(&db_path).expect("Failed to create store");

    // 2. Create aggregator
    let aggregator = UsageAggregator::new(store);

    // 3. Create broadcast channel
    let (tx, rx) = broadcast::channel(100);

    // 4. Start background listener
    let _handle = aggregator.start_listening(rx);

    // 5. Register session WITHOUT subtask_id
    let session_id = "test-session-no-subtask".to_string();
    let spec_id = SpecId::new();
    let task_id = TaskId::new();

    let context = SessionContext {
        task_id,
        subtask_id: None, // No subtask
        spec_id,
    };

    aggregator
        .register_session(session_id.clone(), context)
        .await;

    // 6. Send TokensConsumed event
    let event = SurgeEvent::TokensConsumed {
        session_id: session_id.clone(),
        agent_name: "claude-haiku".to_string(),
        spec_id: Some(spec_id),
        subtask_id: None,
        input_tokens: 200,
        output_tokens: 100,
        thought_tokens: None,
        cached_read_tokens: None,
        cached_write_tokens: None,
        estimated_cost_usd: Some(0.001),
    };

    tx.send(event).expect("Failed to send event");

    // 7. Wait for aggregation
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 8. Verify data in store
    let verify_store = Store::open(&db_path).expect("Failed to open store for verification");

    // Check session record
    let sessions = verify_store
        .list_sessions_by_spec(spec_id)
        .expect("Failed to list sessions");
    assert_eq!(sessions.len(), 1, "Expected 1 session record");

    let session = &sessions[0];
    assert_eq!(session.session_id, session_id);
    assert_eq!(session.subtask_id, None);
    assert_eq!(session.input_tokens, 200);
    assert_eq!(session.output_tokens, 100);

    // Check that NO subtask record was created
    let all_subtasks = verify_store
        .list_subtasks_by_spec(spec_id)
        .expect("Failed to list subtasks");
    assert_eq!(
        all_subtasks.len(),
        0,
        "No subtask record should exist for session without subtask_id"
    );

    // Check spec record (should still exist and aggregate the session)
    let spec = verify_store
        .get_spec(spec_id)
        .expect("Failed to get spec")
        .expect("Spec should exist");

    assert_eq!(spec.session_count, 1);
    assert_eq!(spec.input_tokens, 200);
    assert_eq!(spec.output_tokens, 100);
    assert_eq!(spec.thought_tokens, 0); // No thought tokens
    assert_eq!(spec.estimated_cost_usd, 0.001);

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test that multiple sessions accumulate costs correctly.
#[tokio::test]
async fn test_cost_accumulation() {
    let db_path = temp_db_path("accumulation");

    // 1. Create store
    let store = Store::open(&db_path).expect("Failed to create store");

    // 2. Create aggregator
    let aggregator = UsageAggregator::new(store);

    // 3. Create broadcast channel
    let (tx, rx) = broadcast::channel(100);

    // 4. Start background listener
    let _handle = aggregator.start_listening(rx);

    // 5. Same spec, same subtask, multiple sessions
    let spec_id = SpecId::new();
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    // Session 1
    let session_id_1 = "session-1".to_string();
    let context_1 = SessionContext {
        task_id,
        subtask_id: Some(subtask_id),
        spec_id,
    };
    aggregator
        .register_session(session_id_1.clone(), context_1)
        .await;

    let event_1 = SurgeEvent::TokensConsumed {
        session_id: session_id_1,
        agent_name: "claude-sonnet".to_string(),
        spec_id: Some(spec_id),
        subtask_id: Some(subtask_id),
        input_tokens: 1000,
        output_tokens: 500,
        thought_tokens: Some(100),
        cached_read_tokens: None,
        cached_write_tokens: None,
        estimated_cost_usd: Some(0.010),
    };
    tx.send(event_1).expect("Failed to send event 1");

    // Session 2
    let session_id_2 = "session-2".to_string();
    let context_2 = SessionContext {
        task_id,
        subtask_id: Some(subtask_id),
        spec_id,
    };
    aggregator
        .register_session(session_id_2.clone(), context_2)
        .await;

    let event_2 = SurgeEvent::TokensConsumed {
        session_id: session_id_2,
        agent_name: "claude-sonnet".to_string(),
        spec_id: Some(spec_id),
        subtask_id: Some(subtask_id),
        input_tokens: 2000,
        output_tokens: 1000,
        thought_tokens: Some(200),
        cached_read_tokens: Some(500),
        cached_write_tokens: Some(100),
        estimated_cost_usd: Some(0.020),
    };
    tx.send(event_2).expect("Failed to send event 2");

    // 6. Wait for aggregation
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 7. Verify accumulated data
    let verify_store = Store::open(&db_path).expect("Failed to open store for verification");

    // Check sessions (should have 2)
    let sessions = verify_store
        .list_sessions_by_spec(spec_id)
        .expect("Failed to list sessions");
    assert_eq!(sessions.len(), 2, "Expected 2 session records");

    // Check subtask (should accumulate both sessions)
    let subtask = verify_store
        .get_subtask(subtask_id, task_id, spec_id)
        .expect("Failed to get subtask")
        .expect("Subtask should exist");

    assert_eq!(subtask.session_count, 2);
    assert_eq!(subtask.input_tokens, 3000); // 1000 + 2000
    assert_eq!(subtask.output_tokens, 1500); // 500 + 1000
    assert_eq!(subtask.thought_tokens, 300); // 100 + 200
    assert_eq!(subtask.cached_read_tokens, 500); // 0 + 500
    assert_eq!(subtask.cached_write_tokens, 100); // 0 + 100
    assert_eq!(subtask.estimated_cost_usd, 0.030); // 0.010 + 0.020

    // Check spec (should accumulate both sessions)
    let spec = verify_store
        .get_spec(spec_id)
        .expect("Failed to get spec")
        .expect("Spec should exist");

    assert_eq!(spec.session_count, 2);
    assert_eq!(spec.input_tokens, 3000);
    assert_eq!(spec.output_tokens, 1500);
    assert_eq!(spec.thought_tokens, 300);
    assert_eq!(spec.cached_read_tokens, 500);
    assert_eq!(spec.cached_write_tokens, 100);
    assert_eq!(spec.estimated_cost_usd, 0.030);

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test that cost calculations are preserved through aggregation.
#[tokio::test]
async fn test_cost_calculation_preservation() {
    let db_path = temp_db_path("cost_calc");

    // 1. Create store
    let store = Store::open(&db_path).expect("Failed to create store");

    // 2. Create aggregator
    let aggregator = UsageAggregator::new(store);

    // 3. Create broadcast channel
    let (tx, rx) = broadcast::channel(100);

    // 4. Start background listener
    let _handle = aggregator.start_listening(rx);

    // 5. Register session with specific cost
    let session_id = "cost-test-session".to_string();
    let spec_id = SpecId::new();
    let task_id = TaskId::new();
    let subtask_id = SubtaskId::new();

    let context = SessionContext {
        task_id,
        subtask_id: Some(subtask_id),
        spec_id,
    };

    aggregator
        .register_session(session_id.clone(), context)
        .await;

    // 6. Send event with specific cost
    let expected_cost = 0.12345; // Precise cost value
    let event = SurgeEvent::TokensConsumed {
        session_id: session_id.clone(),
        agent_name: "claude-opus".to_string(),
        spec_id: Some(spec_id),
        subtask_id: Some(subtask_id),
        input_tokens: 10000,
        output_tokens: 5000,
        thought_tokens: Some(1000),
        cached_read_tokens: Some(2000),
        cached_write_tokens: Some(500),
        estimated_cost_usd: Some(expected_cost),
    };

    tx.send(event).expect("Failed to send event");

    // 7. Wait for aggregation
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 8. Verify cost is preserved exactly
    let verify_store = Store::open(&db_path).expect("Failed to open store for verification");

    // Check session
    let sessions = verify_store
        .list_sessions_by_spec(spec_id)
        .expect("Failed to list sessions");
    assert_eq!(sessions[0].estimated_cost_usd, Some(expected_cost));

    // Check subtask
    let subtask = verify_store
        .get_subtask(subtask_id, task_id, spec_id)
        .expect("Failed to get subtask")
        .expect("Subtask should exist");
    assert_eq!(subtask.estimated_cost_usd, expected_cost);

    // Check spec
    let spec = verify_store
        .get_spec(spec_id)
        .expect("Failed to get spec")
        .expect("Spec should exist");
    assert_eq!(spec.estimated_cost_usd, expected_cost);

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}
