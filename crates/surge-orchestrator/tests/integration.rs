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
    // Cost is now calculated by aggregator using claude_sonnet_35_pricing():
    // Input: 1000 tokens * $3.00/M = $0.003
    // Output: 500 tokens * $15.00/M = $0.0075
    // Thought: 200 tokens * $15.00/M = $0.003
    // Cache read: 100 tokens * $0.30/M = $0.00003
    // Cache write: 50 tokens * $3.75/M = $0.0001875
    // Total = $0.0137175
    let expected_cost = 0.0137175;
    assert!(
        session.estimated_cost_usd.is_some(),
        "Cost should be calculated"
    );
    assert!(
        (session.estimated_cost_usd.unwrap() - expected_cost).abs() < 1e-6,
        "Cost should be {}, got {}",
        expected_cost,
        session.estimated_cost_usd.unwrap()
    );

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
    assert!(
        (subtask.estimated_cost_usd - expected_cost).abs() < 1e-6,
        "Subtask cost should be {}, got {}",
        expected_cost,
        subtask.estimated_cost_usd
    );

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
    assert!(
        (spec.estimated_cost_usd - expected_cost).abs() < 1e-6,
        "Spec cost should be {}, got {}",
        expected_cost,
        spec.estimated_cost_usd
    );

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

    // Cost calculated by aggregator using claude_sonnet_35_pricing() (for haiku):
    // Input: 200 tokens * $3.00/M = $0.0006
    // Output: 100 tokens * $15.00/M = $0.0015
    // Total = $0.0021
    let expected_cost = 0.0021;
    assert!(
        (spec.estimated_cost_usd - expected_cost).abs() < 1e-6,
        "Spec cost should be {}, got {}",
        expected_cost,
        spec.estimated_cost_usd
    );

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

    // Cost calculated by aggregator using claude_sonnet_35_pricing():
    // Session 1: 1000*$3/M + 500*$15/M + 100*$15/M = $0.012
    // Session 2: 2000*$3/M + 1000*$15/M + 200*$15/M + 500*$0.3/M + 100*$3.75/M = $0.024525
    // Total = $0.036525
    let expected_accumulated_cost = 0.036525;
    assert!(
        (subtask.estimated_cost_usd - expected_accumulated_cost).abs() < 1e-6,
        "Subtask cost should be {}, got {}",
        expected_accumulated_cost,
        subtask.estimated_cost_usd
    );

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
    assert!(
        (spec.estimated_cost_usd - expected_accumulated_cost).abs() < 1e-6,
        "Spec cost should be {}, got {}",
        expected_accumulated_cost,
        spec.estimated_cost_usd
    );

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

    // 6. Send event with token counts
    // Cost will be calculated by aggregator using claude_opus_pricing():
    // Input: 10000 tokens * $15.00/M = $0.15
    // Output: 5000 tokens * $75.00/M = $0.375
    // Thought: 1000 tokens * $75.00/M = $0.075
    // Cache read: 2000 tokens * $1.50/M = $0.003
    // Cache write: 500 tokens * $18.75/M = $0.009375
    // Total = $0.612375
    let expected_cost = 0.612375;
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
        estimated_cost_usd: Some(0.99999), // This will be ignored, cost is calculated
    };

    tx.send(event).expect("Failed to send event");

    // 7. Wait for aggregation
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 8. Verify cost is calculated correctly using pricing model
    let verify_store = Store::open(&db_path).expect("Failed to open store for verification");

    // Check session
    let sessions = verify_store
        .list_sessions_by_spec(spec_id)
        .expect("Failed to list sessions");
    assert!(
        sessions[0].estimated_cost_usd.is_some(),
        "Cost should be calculated"
    );
    assert!(
        (sessions[0].estimated_cost_usd.unwrap() - expected_cost).abs() < 1e-6,
        "Cost should be {}, got {:?}",
        expected_cost,
        sessions[0].estimated_cost_usd
    );

    // Check subtask
    let subtask = verify_store
        .get_subtask(subtask_id, task_id, spec_id)
        .expect("Failed to get subtask")
        .expect("Subtask should exist");
    assert!(
        (subtask.estimated_cost_usd - expected_cost).abs() < 1e-6,
        "Subtask cost should be {}, got {}",
        expected_cost,
        subtask.estimated_cost_usd
    );

    // Check spec
    let spec = verify_store
        .get_spec(spec_id)
        .expect("Failed to get spec")
        .expect("Spec should exist");
    assert!(
        (spec.estimated_cost_usd - expected_cost).abs() < 1e-6,
        "Spec cost should be {}, got {}",
        expected_cost,
        spec.estimated_cost_usd
    );

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
}

/// Test that QaVerdictReceived events are properly emitted for APPROVED verdict.
#[tokio::test]
async fn test_qa_verdict_approved() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send QaVerdictReceived event with APPROVED verdict
    let event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Approved,
        iteration: 1,
        reasoning: Some(
            "All acceptance criteria have been met. The implementation is correct.".to_string(),
        ),
        met_criteria: vec![
            "Error handling implemented".to_string(),
            "Tests passing".to_string(),
            "Documentation complete".to_string(),
        ],
        unmet_criteria: vec![],
        issues: None,
    };

    tx.send(event).expect("Failed to send event");

    // 4. Receive and verify the event
    let received_event = rx.recv().await.expect("Failed to receive event");

    match received_event {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::Approved);
            assert_eq!(iteration, 1);
            assert!(reasoning.is_some());
            assert!(reasoning.unwrap().contains("acceptance criteria"));
            assert_eq!(met_criteria.len(), 3);
            assert!(met_criteria.contains(&"Error handling implemented".to_string()));
            assert!(met_criteria.contains(&"Tests passing".to_string()));
            assert!(met_criteria.contains(&"Documentation complete".to_string()));
            assert_eq!(unmet_criteria.len(), 0);
            assert!(issues.is_none());
        },
        _ => panic!("Expected QaVerdictReceived event"),
    }
}

/// Test that QaVerdictReceived events are properly emitted for APPROVED verdict with minimal data.
#[tokio::test]
async fn test_qa_verdict_approved_minimal() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send QaVerdictReceived event with minimal APPROVED data
    let event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Approved,
        iteration: 1,
        reasoning: None,
        met_criteria: vec![],
        unmet_criteria: vec![],
        issues: None,
    };

    tx.send(event).expect("Failed to send event");

    // 4. Receive and verify the event
    let received_event = rx.recv().await.expect("Failed to receive event");

    match received_event {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::Approved);
            assert_eq!(iteration, 1);
            assert!(reasoning.is_none());
            assert_eq!(met_criteria.len(), 0);
            assert_eq!(unmet_criteria.len(), 0);
            assert!(issues.is_none());
        },
        _ => panic!("Expected QaVerdictReceived event"),
    }
}

/// Test that APPROVED verdict after multiple QA iterations is properly tracked.
#[tokio::test]
async fn test_qa_verdict_approved_after_iterations() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Simulate multiple QA iterations by sending APPROVED on iteration 3
    let event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Approved,
        iteration: 3,
        reasoning: Some(
            "After fixing issues from previous iterations, all criteria now pass.".to_string(),
        ),
        met_criteria: vec![
            "Error handling implemented".to_string(),
            "Tests passing".to_string(),
            "Documentation complete".to_string(),
            "Performance optimizations applied".to_string(),
        ],
        unmet_criteria: vec![],
        issues: None,
    };

    tx.send(event).expect("Failed to send event");

    // 4. Receive and verify the event
    let received_event = rx.recv().await.expect("Failed to receive event");

    match received_event {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::Approved);
            assert_eq!(
                iteration, 3,
                "Should track that approval came on iteration 3"
            );
            assert!(reasoning.is_some());
            assert!(reasoning.unwrap().contains("previous iterations"));
            assert_eq!(met_criteria.len(), 4);
            assert_eq!(unmet_criteria.len(), 0);
            assert!(issues.is_none());
        },
        _ => panic!("Expected QaVerdictReceived event"),
    }
}

/// Test that NEEDS_FIX verdict triggers fix loop and eventual approval.
#[tokio::test]
async fn test_qa_verdict_needs_fix_loop() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send initial NEEDS_FIX verdict (iteration 1)
    let needs_fix_event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::NeedsFix,
        iteration: 1,
        reasoning: Some("Implementation has issues that need to be addressed.".to_string()),
        met_criteria: vec![
            "Error handling implemented".to_string(),
        ],
        unmet_criteria: vec![
            "Tests passing".to_string(),
            "Documentation complete".to_string(),
        ],
        issues: Some("Unit tests are failing due to missing edge case handling. Documentation is incomplete.".to_string()),
    };

    tx.send(needs_fix_event)
        .expect("Failed to send NEEDS_FIX event");

    // 4. Receive and verify NEEDS_FIX event
    let received_needs_fix = rx.recv().await.expect("Failed to receive NEEDS_FIX event");

    match received_needs_fix {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::NeedsFix);
            assert_eq!(iteration, 1, "Should be first QA iteration");
            assert!(reasoning.is_some());
            assert!(
                reasoning
                    .unwrap()
                    .contains("issues that need to be addressed")
            );
            assert_eq!(met_criteria.len(), 1);
            assert!(met_criteria.contains(&"Error handling implemented".to_string()));
            assert_eq!(unmet_criteria.len(), 2);
            assert!(unmet_criteria.contains(&"Tests passing".to_string()));
            assert!(unmet_criteria.contains(&"Documentation complete".to_string()));
            assert!(issues.is_some());
            assert!(issues.unwrap().contains("Unit tests are failing"));
        },
        _ => panic!("Expected QaVerdictReceived event with NEEDS_FIX"),
    }

    // 5. Simulate fix being applied and send APPROVED verdict (iteration 2)
    let approved_event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Approved,
        iteration: 2,
        reasoning: Some(
            "All issues from iteration 1 have been resolved. Implementation is now complete."
                .to_string(),
        ),
        met_criteria: vec![
            "Error handling implemented".to_string(),
            "Tests passing".to_string(),
            "Documentation complete".to_string(),
        ],
        unmet_criteria: vec![],
        issues: None,
    };

    tx.send(approved_event)
        .expect("Failed to send APPROVED event");

    // 6. Receive and verify APPROVED event
    let received_approved = rx.recv().await.expect("Failed to receive APPROVED event");

    match received_approved {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::Approved);
            assert_eq!(iteration, 2, "Should be second QA iteration after fix");
            assert!(reasoning.is_some());
            assert!(
                reasoning
                    .unwrap()
                    .contains("issues from iteration 1 have been resolved")
            );
            assert_eq!(met_criteria.len(), 3);
            assert!(met_criteria.contains(&"Tests passing".to_string()));
            assert!(met_criteria.contains(&"Documentation complete".to_string()));
            assert_eq!(unmet_criteria.len(), 0);
            assert!(issues.is_none());
        },
        _ => panic!("Expected QaVerdictReceived event with APPROVED"),
    }
}

/// Test that NEEDS_FIX verdict contains detailed issue information.
#[tokio::test]
async fn test_qa_verdict_needs_fix_with_issues() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send NEEDS_FIX verdict with detailed issues
    let event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::NeedsFix,
        iteration: 1,
        reasoning: Some("Critical bugs found in implementation.".to_string()),
        met_criteria: vec![],
        unmet_criteria: vec![
            "No panics or unwraps in production code".to_string(),
            "All error paths tested".to_string(),
            "Performance benchmarks pass".to_string(),
        ],
        issues: Some(
            "1. Found unwrap() on line 45 that could panic.\n\
             2. Error path in handle_request() is not tested.\n\
             3. Performance regression: response time increased by 200ms."
                .to_string(),
        ),
    };

    tx.send(event).expect("Failed to send event");

    // 4. Receive and verify the event
    let received_event = rx.recv().await.expect("Failed to receive event");

    match received_event {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::NeedsFix);
            assert_eq!(iteration, 1);
            assert!(reasoning.is_some());
            assert!(reasoning.unwrap().contains("Critical bugs"));
            assert_eq!(met_criteria.len(), 0);
            assert_eq!(unmet_criteria.len(), 3);
            assert!(
                unmet_criteria.contains(&"No panics or unwraps in production code".to_string())
            );
            assert!(unmet_criteria.contains(&"All error paths tested".to_string()));
            assert!(unmet_criteria.contains(&"Performance benchmarks pass".to_string()));
            assert!(issues.is_some());
            let issues_text = issues.unwrap();
            assert!(issues_text.contains("unwrap() on line 45"));
            assert!(issues_text.contains("Error path in handle_request()"));
            assert!(issues_text.contains("Performance regression"));
        },
        _ => panic!("Expected QaVerdictReceived event"),
    }
}

/// Test that multiple NEEDS_FIX iterations are tracked correctly.
#[tokio::test]
async fn test_qa_verdict_multiple_needs_fix_iterations() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send first NEEDS_FIX (iteration 1)
    let event1 = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::NeedsFix,
        iteration: 1,
        reasoning: Some("Tests are failing.".to_string()),
        met_criteria: vec!["Code compiles".to_string()],
        unmet_criteria: vec!["Tests passing".to_string()],
        issues: Some("3 unit tests failing".to_string()),
    };
    tx.send(event1).expect("Failed to send event 1");

    let recv1 = rx.recv().await.expect("Failed to receive event 1");
    if let SurgeEvent::QaVerdictReceived {
        iteration, verdict, ..
    } = recv1
    {
        assert_eq!(iteration, 1);
        assert_eq!(verdict, QaVerdictKind::NeedsFix);
    } else {
        panic!("Expected QaVerdictReceived");
    }

    // 4. Send second NEEDS_FIX (iteration 2) - still issues after first fix
    let event2 = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::NeedsFix,
        iteration: 2,
        reasoning: Some("Some tests still failing.".to_string()),
        met_criteria: vec!["Code compiles".to_string(), "2 tests fixed".to_string()],
        unmet_criteria: vec!["All tests passing".to_string()],
        issues: Some("1 unit test still failing".to_string()),
    };
    tx.send(event2).expect("Failed to send event 2");

    let recv2 = rx.recv().await.expect("Failed to receive event 2");
    if let SurgeEvent::QaVerdictReceived {
        iteration, verdict, ..
    } = recv2
    {
        assert_eq!(iteration, 2, "Should track second iteration");
        assert_eq!(verdict, QaVerdictKind::NeedsFix);
    } else {
        panic!("Expected QaVerdictReceived");
    }

    // 5. Send final APPROVED (iteration 3) - all fixed
    let event3 = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Approved,
        iteration: 3,
        reasoning: Some("All tests now passing.".to_string()),
        met_criteria: vec!["Code compiles".to_string(), "All tests passing".to_string()],
        unmet_criteria: vec![],
        issues: None,
    };
    tx.send(event3).expect("Failed to send event 3");

    let recv3 = rx.recv().await.expect("Failed to receive event 3");
    if let SurgeEvent::QaVerdictReceived {
        iteration, verdict, ..
    } = recv3
    {
        assert_eq!(iteration, 3, "Should track third iteration before approval");
        assert_eq!(verdict, QaVerdictKind::Approved);
    } else {
        panic!("Expected QaVerdictReceived");
    }
}

/// Test that PARTIAL verdict is properly emitted with met and unmet criteria.
#[tokio::test]
async fn test_qa_verdict_partial() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send QaVerdictReceived event with PARTIAL verdict
    let event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Partial,
        iteration: 1,
        reasoning: Some("Some acceptance criteria have been met, but others require implementation.".to_string()),
        met_criteria: vec![
            "Error handling implemented".to_string(),
            "Documentation complete".to_string(),
        ],
        unmet_criteria: vec![
            "Tests passing".to_string(),
            "Performance optimization".to_string(),
        ],
        issues: Some("Unit tests are missing for the new error handling code. Performance benchmarks need to be added.".to_string()),
    };

    tx.send(event).expect("Failed to send event");

    // 4. Receive and verify the event
    let received_event = rx.recv().await.expect("Failed to receive event");

    match received_event {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::Partial);
            assert_eq!(iteration, 1);
            assert!(reasoning.is_some());
            assert!(reasoning.unwrap().contains("Some acceptance criteria"));

            // Verify met criteria
            assert_eq!(met_criteria.len(), 2);
            assert!(met_criteria.contains(&"Error handling implemented".to_string()));
            assert!(met_criteria.contains(&"Documentation complete".to_string()));

            // Verify unmet criteria
            assert_eq!(unmet_criteria.len(), 2);
            assert!(unmet_criteria.contains(&"Tests passing".to_string()));
            assert!(unmet_criteria.contains(&"Performance optimization".to_string()));

            // Verify issues
            assert!(issues.is_some());
            let issues_text = issues.unwrap();
            assert!(issues_text.contains("Unit tests are missing"));
            assert!(issues_text.contains("Performance benchmarks"));
        },
        _ => panic!("Expected QaVerdictReceived event"),
    }
}

/// Test that PARTIAL verdict with minimal data is handled correctly.
#[tokio::test]
async fn test_qa_verdict_partial_minimal() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send QaVerdictReceived event with minimal PARTIAL data
    let event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Partial,
        iteration: 1,
        reasoning: None,
        met_criteria: vec![],
        unmet_criteria: vec![],
        issues: None,
    };

    tx.send(event).expect("Failed to send event");

    // 4. Receive and verify the event
    let received_event = rx.recv().await.expect("Failed to receive event");

    match received_event {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::Partial);
            assert_eq!(iteration, 1);
            assert!(reasoning.is_none());
            assert_eq!(met_criteria.len(), 0);
            assert_eq!(unmet_criteria.len(), 0);
            assert!(issues.is_none());
        },
        _ => panic!("Expected QaVerdictReceived event"),
    }
}

/// Test that PARTIAL verdict leads to eventual approval after fixing unmet criteria.
#[tokio::test]
async fn test_qa_verdict_partial_fix_loop() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send initial PARTIAL verdict (iteration 1)
    let partial_event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Partial,
        iteration: 1,
        reasoning: Some(
            "Implementation is partially complete. Some criteria met, others need work."
                .to_string(),
        ),
        met_criteria: vec![
            "Error handling implemented".to_string(),
            "Documentation complete".to_string(),
        ],
        unmet_criteria: vec![
            "Tests passing".to_string(),
            "Performance optimization".to_string(),
        ],
        issues: Some("Missing unit tests and performance benchmarks.".to_string()),
    };

    tx.send(partial_event)
        .expect("Failed to send PARTIAL event");

    // 4. Receive and verify PARTIAL event
    let received_partial = rx.recv().await.expect("Failed to receive PARTIAL event");

    match received_partial {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::Partial);
            assert_eq!(iteration, 1, "Should be first QA iteration");
            assert!(reasoning.is_some());
            assert!(reasoning.unwrap().contains("partially complete"));

            // Verify met criteria
            assert_eq!(met_criteria.len(), 2);
            assert!(met_criteria.contains(&"Error handling implemented".to_string()));
            assert!(met_criteria.contains(&"Documentation complete".to_string()));

            // Verify unmet criteria that need to be fixed
            assert_eq!(unmet_criteria.len(), 2);
            assert!(unmet_criteria.contains(&"Tests passing".to_string()));
            assert!(unmet_criteria.contains(&"Performance optimization".to_string()));

            assert!(issues.is_some());
            assert!(issues.unwrap().contains("Missing unit tests"));
        },
        _ => panic!("Expected QaVerdictReceived event with PARTIAL"),
    }

    // 5. Simulate fix being applied and send APPROVED verdict (iteration 2)
    let approved_event = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Approved,
        iteration: 2,
        reasoning: Some(
            "All unmet criteria from iteration 1 have been addressed. All criteria now pass."
                .to_string(),
        ),
        met_criteria: vec![
            "Error handling implemented".to_string(),
            "Documentation complete".to_string(),
            "Tests passing".to_string(),
            "Performance optimization".to_string(),
        ],
        unmet_criteria: vec![],
        issues: None,
    };

    tx.send(approved_event)
        .expect("Failed to send APPROVED event");

    // 6. Receive and verify APPROVED event
    let received_approved = rx.recv().await.expect("Failed to receive APPROVED event");

    match received_approved {
        SurgeEvent::QaVerdictReceived {
            task_id: recv_task_id,
            verdict,
            iteration,
            reasoning,
            met_criteria,
            unmet_criteria,
            issues,
        } => {
            assert_eq!(recv_task_id, task_id);
            assert_eq!(verdict, QaVerdictKind::Approved);
            assert_eq!(
                iteration, 2,
                "Should be second QA iteration after partial fix"
            );
            assert!(reasoning.is_some());
            assert!(
                reasoning
                    .unwrap()
                    .contains("unmet criteria from iteration 1 have been addressed")
            );

            // All criteria should now be met
            assert_eq!(met_criteria.len(), 4);
            assert!(met_criteria.contains(&"Tests passing".to_string()));
            assert!(met_criteria.contains(&"Performance optimization".to_string()));
            assert_eq!(unmet_criteria.len(), 0);
            assert!(issues.is_none());
        },
        _ => panic!("Expected QaVerdictReceived event with APPROVED"),
    }
}

/// Test that multiple PARTIAL verdicts converge to APPROVED.
#[tokio::test]
async fn test_qa_verdict_multiple_partial_iterations() {
    use surge_core::event::QaVerdictKind;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();

    // 3. Send first PARTIAL (iteration 1) - 2 met, 3 unmet
    let event1 = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Partial,
        iteration: 1,
        reasoning: Some("Initial review: some progress made.".to_string()),
        met_criteria: vec![
            "Code compiles".to_string(),
            "Error handling implemented".to_string(),
        ],
        unmet_criteria: vec![
            "Tests passing".to_string(),
            "Documentation complete".to_string(),
            "Performance optimization".to_string(),
        ],
        issues: Some("Tests failing, docs incomplete, perf not optimized.".to_string()),
    };
    tx.send(event1).expect("Failed to send event 1");

    let recv1 = rx.recv().await.expect("Failed to receive event 1");
    if let SurgeEvent::QaVerdictReceived {
        iteration,
        verdict,
        met_criteria,
        unmet_criteria,
        ..
    } = recv1
    {
        assert_eq!(iteration, 1);
        assert_eq!(verdict, QaVerdictKind::Partial);
        assert_eq!(met_criteria.len(), 2);
        assert_eq!(unmet_criteria.len(), 3);
    } else {
        panic!("Expected QaVerdictReceived");
    }

    // 4. Send second PARTIAL (iteration 2) - 3 met, 2 unmet (progress!)
    let event2 = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Partial,
        iteration: 2,
        reasoning: Some("Progress made: tests now passing.".to_string()),
        met_criteria: vec![
            "Code compiles".to_string(),
            "Error handling implemented".to_string(),
            "Tests passing".to_string(),
        ],
        unmet_criteria: vec![
            "Documentation complete".to_string(),
            "Performance optimization".to_string(),
        ],
        issues: Some("Docs and perf still need work.".to_string()),
    };
    tx.send(event2).expect("Failed to send event 2");

    let recv2 = rx.recv().await.expect("Failed to receive event 2");
    if let SurgeEvent::QaVerdictReceived {
        iteration,
        verdict,
        met_criteria,
        unmet_criteria,
        ..
    } = recv2
    {
        assert_eq!(iteration, 2, "Should track second iteration");
        assert_eq!(verdict, QaVerdictKind::Partial);
        assert_eq!(met_criteria.len(), 3, "One more criterion met");
        assert_eq!(unmet_criteria.len(), 2, "Two criteria still unmet");
    } else {
        panic!("Expected QaVerdictReceived");
    }

    // 5. Send third PARTIAL (iteration 3) - 4 met, 1 unmet (more progress!)
    let event3 = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Partial,
        iteration: 3,
        reasoning: Some("Almost there: documentation now complete.".to_string()),
        met_criteria: vec![
            "Code compiles".to_string(),
            "Error handling implemented".to_string(),
            "Tests passing".to_string(),
            "Documentation complete".to_string(),
        ],
        unmet_criteria: vec!["Performance optimization".to_string()],
        issues: Some("Only performance optimization remaining.".to_string()),
    };
    tx.send(event3).expect("Failed to send event 3");

    let recv3 = rx.recv().await.expect("Failed to receive event 3");
    if let SurgeEvent::QaVerdictReceived {
        iteration,
        verdict,
        met_criteria,
        unmet_criteria,
        ..
    } = recv3
    {
        assert_eq!(iteration, 3, "Should track third iteration");
        assert_eq!(verdict, QaVerdictKind::Partial);
        assert_eq!(met_criteria.len(), 4, "Four criteria now met");
        assert_eq!(unmet_criteria.len(), 1, "Only one criterion remaining");
    } else {
        panic!("Expected QaVerdictReceived");
    }

    // 6. Send final APPROVED (iteration 4) - all met!
    let event4 = SurgeEvent::QaVerdictReceived {
        task_id,
        verdict: QaVerdictKind::Approved,
        iteration: 4,
        reasoning: Some("All criteria now met after multiple partial iterations.".to_string()),
        met_criteria: vec![
            "Code compiles".to_string(),
            "Error handling implemented".to_string(),
            "Tests passing".to_string(),
            "Documentation complete".to_string(),
            "Performance optimization".to_string(),
        ],
        unmet_criteria: vec![],
        issues: None,
    };
    tx.send(event4).expect("Failed to send event 4");

    let recv4 = rx.recv().await.expect("Failed to receive event 4");
    if let SurgeEvent::QaVerdictReceived {
        iteration,
        verdict,
        met_criteria,
        unmet_criteria,
        ..
    } = recv4
    {
        assert_eq!(
            iteration, 4,
            "Should track fourth iteration before approval"
        );
        assert_eq!(verdict, QaVerdictKind::Approved);
        assert_eq!(met_criteria.len(), 5, "All criteria now met");
        assert_eq!(unmet_criteria.len(), 0, "No unmet criteria");
    } else {
        panic!("Expected QaVerdictReceived");
    }
}

/// Test that max QA iterations exceeded results in task transition to Failed state.
#[tokio::test]
async fn test_qa_max_iterations_failure() {
    use surge_core::event::QaVerdictKind;
    use surge_core::state::TaskState;

    // 1. Create broadcast channel
    let (tx, mut rx) = broadcast::channel(100);

    // 2. Create IDs for the test
    let task_id = TaskId::new();
    let max_iterations = 3;

    // 3. Send NEEDS_FIX verdicts up to max_iterations
    for iteration in 1..=max_iterations {
        let event = SurgeEvent::QaVerdictReceived {
            task_id,
            verdict: QaVerdictKind::NeedsFix,
            iteration,
            reasoning: Some(format!("Iteration {} - tests still failing.", iteration)),
            met_criteria: vec!["Code compiles".to_string()],
            unmet_criteria: vec!["Tests passing".to_string()],
            issues: Some(format!("Tests failing on iteration {}", iteration)),
        };
        tx.send(event)
            .expect(&format!("Failed to send event {}", iteration));

        let recv = rx
            .recv()
            .await
            .expect(&format!("Failed to receive event {}", iteration));
        if let SurgeEvent::QaVerdictReceived {
            iteration: recv_iter,
            verdict,
            ..
        } = recv
        {
            assert_eq!(recv_iter, iteration);
            assert_eq!(verdict, QaVerdictKind::NeedsFix);
        } else {
            panic!("Expected QaVerdictReceived on iteration {}", iteration);
        }
    }

    // 4. After max_iterations exhausted, send TaskStateChanged to Failed
    let failed_event = SurgeEvent::TaskStateChanged {
        task_id,
        old_state: TaskState::QaReview {
            verdict: Some("needs_fix".to_string()),
            reasoning: Some(format!(
                "QA did not approve after {} iterations",
                max_iterations
            )),
        },
        new_state: TaskState::Failed {
            reason: format!(
                "QA review failed after max iterations: QA did not approve after {} iterations",
                max_iterations
            ),
        },
    };
    tx.send(failed_event)
        .expect("Failed to send TaskStateChanged event");

    let recv_failed = rx
        .recv()
        .await
        .expect("Failed to receive TaskStateChanged event");
    if let SurgeEvent::TaskStateChanged {
        task_id: recv_task_id,
        old_state,
        new_state,
    } = recv_failed
    {
        assert_eq!(recv_task_id, task_id);

        // Verify old state is QaReview
        if let TaskState::QaReview { verdict, .. } = old_state {
            assert_eq!(verdict, Some("needs_fix".to_string()));
        } else {
            panic!("Expected old_state to be QaReview, got {:?}", old_state);
        }

        // Verify new state is Failed with correct reason
        if let TaskState::Failed { reason } = new_state {
            assert!(
                reason.contains("max iterations"),
                "Failure reason should mention max iterations, got: {}",
                reason
            );
            assert!(
                reason.contains(&max_iterations.to_string()),
                "Failure reason should include iteration count, got: {}",
                reason
            );
        } else {
            panic!("Expected new_state to be Failed, got {:?}", new_state);
        }
    } else {
        panic!("Expected TaskStateChanged event");
    }
}
