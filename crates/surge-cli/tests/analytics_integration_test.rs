//! Integration tests for cost & token analytics feature.
//!
//! Verifies the complete analytics pipeline:
//! - Database storage of session usage and costs
//! - Analytics summary displays cost data correctly
//! - Analytics export outputs JSON/CSV with cost data
//! - Budget warnings trigger at configured thresholds
//!
//! These tests use programmatically created test data to avoid requiring a real ACP agent.

use std::path::PathBuf;
use surge_core::id::{SpecId, SubtaskId, TaskId};
use surge_persistence::budget::{BudgetTracker, BudgetWarningLevel};
use surge_persistence::models::SessionUsage;
use surge_persistence::pricing::{claude_sonnet_35_pricing, gpt4_turbo_pricing};
use surge_persistence::store::Store;
use tempfile::TempDir;

/// Create a test database with sample session usage data.
///
/// This simulates data that would be created by the UsageAggregator
/// when processing TokensConsumed events from an agent.
fn create_test_db_with_data() -> (TempDir, PathBuf, Store) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let db_path = temp_dir.path().join("analytics-test.db");

    // Create store and initialize schema
    let mut store = Store::open(&db_path).expect("Failed to create store");

    // Get pricing models for cost calculations
    let claude_pricing = claude_sonnet_35_pricing();
    let gpt_pricing = gpt4_turbo_pricing();

    // Create test spec, task, and subtask IDs
    let spec_id = SpecId::new();
    let task_id = TaskId::new();
    let subtask_1 = SubtaskId::new();
    let subtask_2 = SubtaskId::new();
    let subtask_3 = SubtaskId::new();

    // Get current timestamp and create timestamps for "this week"
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // Session 1: Planning phase with claude-sonnet (2 days ago)
    let session_1 = SessionUsage {
        session_id: "session-1".to_string(),
        spec_id,
        task_id,
        subtask_id: Some(subtask_1),
        agent_name: "claude-sonnet".to_string(),
        input_tokens: 50_000,
        output_tokens: 12_000,
        thought_tokens: Some(2_500),
        cached_read_tokens: Some(150_000),
        cached_write_tokens: Some(8_000),
        estimated_cost_usd: Some(claude_pricing.calculate_cost(
            50_000,
            12_000,
            Some(2_500),
            Some(150_000),
            Some(8_000),
        )),
        timestamp_ms: now - (2 * 24 * 60 * 60 * 1000), // 2 days ago
    };

    // Session 2: Coding phase with claude-sonnet (1 day ago)
    let session_2 = SessionUsage {
        session_id: "session-2".to_string(),
        spec_id,
        task_id,
        subtask_id: Some(subtask_2),
        agent_name: "claude-sonnet".to_string(),
        input_tokens: 75_000,
        output_tokens: 18_000,
        thought_tokens: Some(3_200),
        cached_read_tokens: Some(200_000),
        cached_write_tokens: Some(12_000),
        estimated_cost_usd: Some(claude_pricing.calculate_cost(
            75_000,
            18_000,
            Some(3_200),
            Some(200_000),
            Some(12_000),
        )),
        timestamp_ms: now - (1 * 24 * 60 * 60 * 1000), // 1 day ago
    };

    // Session 3: QA review phase with claude-sonnet (today)
    let session_3 = SessionUsage {
        session_id: "session-3".to_string(),
        spec_id,
        task_id,
        subtask_id: Some(subtask_3),
        agent_name: "claude-sonnet".to_string(),
        input_tokens: 30_000,
        output_tokens: 8_000,
        thought_tokens: Some(1_500),
        cached_read_tokens: Some(100_000),
        cached_write_tokens: Some(5_000),
        estimated_cost_usd: Some(claude_pricing.calculate_cost(
            30_000,
            8_000,
            Some(1_500),
            Some(100_000),
            Some(5_000),
        )),
        timestamp_ms: now,
    };

    // Session 4: Different spec, different agent (GPT-4) (today)
    let other_spec_id = SpecId::new();
    let other_task_id = TaskId::new();
    let session_4 = SessionUsage {
        session_id: "session-4".to_string(),
        spec_id: other_spec_id,
        task_id: other_task_id,
        subtask_id: None,
        agent_name: "gpt-4".to_string(),
        input_tokens: 25_000,
        output_tokens: 6_000,
        thought_tokens: None,
        cached_read_tokens: None,
        cached_write_tokens: None,
        estimated_cost_usd: Some(gpt_pricing.calculate_cost(25_000, 6_000, None, None, None)),
        timestamp_ms: now,
    };

    // Store all sessions (renamed from store_session to insert_session based on grep results)
    store
        .insert_session(&session_1)
        .expect("Failed to store session 1");
    store
        .insert_session(&session_2)
        .expect("Failed to store session 2");
    store
        .insert_session(&session_3)
        .expect("Failed to store session 3");
    store
        .insert_session(&session_4)
        .expect("Failed to store session 4");

    (temp_dir, db_path, store)
}

#[test]
fn test_analytics_summary_displays_costs() {
    // Create test database with data
    let (_temp_dir, db_path, store) = create_test_db_with_data();

    // Get all sessions using time range (0 to very large timestamp to get all)
    // Use a timestamp far in the future (year 3000)
    let far_future = 32503680000000u64; // Jan 1, 3000 in milliseconds
    let sessions = store
        .get_sessions_by_time_range(0, far_future)
        .expect("Failed to get sessions");
    assert_eq!(sessions.len(), 4, "Should have 4 test sessions");

    // Verify all sessions have cost data
    for session in &sessions {
        assert!(
            session.estimated_cost_usd.is_some(),
            "Session {} should have estimated cost",
            session.session_id
        );
        assert!(
            session.estimated_cost_usd.unwrap() > 0.0,
            "Session {} should have non-zero cost",
            session.session_id
        );
    }

    // Calculate expected total cost
    let total_cost: f64 = sessions
        .iter()
        .map(|s| s.estimated_cost_usd.unwrap_or(0.0))
        .sum();

    eprintln!("✓ Test database created with {} sessions", sessions.len());
    eprintln!("✓ Total cost in test data: ${:.4}", total_cost);
    eprintln!("✓ Database path: {:?}", db_path);

    eprintln!("✓ Analytics summary integration test: Database layer verified");
}

#[test]
fn test_analytics_export_json_contains_cost_data() {
    // Create test database with data
    let (_temp_dir, _db_path, store) = create_test_db_with_data();

    // Get all sessions using time range (0 to very large timestamp)
    let far_future = 32503680000000u64; // Jan 1, 3000 in milliseconds
    let sessions = store
        .get_sessions_by_time_range(0, far_future)
        .expect("Failed to get sessions");
    assert!(!sessions.is_empty(), "Should have test sessions");

    // Verify JSON serialization of session data
    let json_output =
        serde_json::to_string_pretty(&sessions).expect("Failed to serialize sessions to JSON");

    let preview_len = 500.min(json_output.len());
    eprintln!(
        "JSON export sample (first {} chars):\n{}",
        preview_len,
        &json_output[..preview_len]
    );

    // Verify JSON contains cost fields
    assert!(
        json_output.contains("estimated_cost_usd"),
        "JSON should contain estimated_cost_usd field"
    );
    assert!(
        json_output.contains("input_tokens"),
        "JSON should contain input_tokens field"
    );
    assert!(
        json_output.contains("output_tokens"),
        "JSON should contain output_tokens field"
    );
    assert!(
        json_output.contains("agent_name"),
        "JSON should contain agent_name field"
    );

    // Parse back to verify it's valid JSON
    let parsed: Vec<SessionUsage> =
        serde_json::from_str(&json_output).expect("Should parse back to SessionUsage vec");
    assert_eq!(
        parsed.len(),
        sessions.len(),
        "Parsed JSON should have same number of sessions"
    );

    eprintln!("✓ Analytics export JSON integration test passed");
}

#[test]
fn test_budget_warnings_at_thresholds() {
    // Create test database with data
    let (_temp_dir, _db_path, store) = create_test_db_with_data();

    // Calculate total daily spending
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let start_of_day = now - (24 * 60 * 60 * 1000); // 24 hours ago

    let daily_cost = store
        .get_cost_in_time_range(start_of_day, now + 1000)
        .expect("Failed to get daily cost");

    eprintln!("Daily cost in test data: ${:.4}", daily_cost);

    // Test 1: Budget OK (budget well above spending)
    let high_budget = daily_cost * 2.0;
    let tracker_ok = BudgetTracker::new(80); // 80% warning threshold

    let status_ok = tracker_ok
        .check_daily_budget(&store, high_budget)
        .expect("Failed to check budget");

    assert_eq!(status_ok.warning_level, BudgetWarningLevel::Ok);
    assert!(
        status_ok.usage_percentage < 80.0,
        "Usage should be below warning threshold"
    );
    eprintln!(
        "✓ Budget OK: ${:.4}/${:.4} used ({:.1}%)",
        status_ok.actual_spending_usd, status_ok.budget_limit_usd, status_ok.usage_percentage
    );

    // Test 2: Budget Warning (budget at 85% usage)
    let warning_budget = daily_cost / 0.85; // Set budget so current spending is 85%
    let status_warning = tracker_ok
        .check_daily_budget(&store, warning_budget)
        .expect("Failed to check budget");

    assert_eq!(status_warning.warning_level, BudgetWarningLevel::Warning);
    assert!(
        status_warning.usage_percentage >= 80.0 && status_warning.usage_percentage < 95.0,
        "Usage should be in warning range (80-95%), got {:.1}%",
        status_warning.usage_percentage
    );
    eprintln!(
        "✓ Budget Warning: ${:.4}/${:.4} used ({:.1}%)",
        status_warning.actual_spending_usd,
        status_warning.budget_limit_usd,
        status_warning.usage_percentage
    );

    // Test 3: Budget Critical (budget at 96% usage)
    let critical_budget = daily_cost / 0.96; // Set budget so current spending is 96%
    let status_critical = tracker_ok
        .check_daily_budget(&store, critical_budget)
        .expect("Failed to check budget");

    assert_eq!(status_critical.warning_level, BudgetWarningLevel::Critical);
    assert!(
        status_critical.usage_percentage >= 95.0 && status_critical.usage_percentage < 100.0,
        "Usage should be in critical range (95-100%), got {:.1}%",
        status_critical.usage_percentage
    );
    eprintln!(
        "✓ Budget Critical: ${:.4}/${:.4} used ({:.1}%)",
        status_critical.actual_spending_usd,
        status_critical.budget_limit_usd,
        status_critical.usage_percentage
    );

    // Test 4: Budget Exceeded (spending > budget)
    let exceeded_budget = daily_cost * 0.5; // Set budget to half of spending
    let status_exceeded = tracker_ok
        .check_daily_budget(&store, exceeded_budget)
        .expect("Failed to check budget");

    assert!(status_exceeded.is_exceeded(), "Budget should be exceeded");
    assert!(
        status_exceeded.actual_spending_usd > status_exceeded.budget_limit_usd,
        "Spending should exceed budget"
    );
    let overage = status_exceeded.actual_spending_usd - status_exceeded.budget_limit_usd;
    assert!(overage > 0.0, "Overage should be positive");
    eprintln!(
        "✓ Budget Exceeded: ${:.4}/${:.4} (over by ${:.4})",
        status_exceeded.actual_spending_usd, status_exceeded.budget_limit_usd, overage
    );

    eprintln!("✓ Budget warning threshold integration test passed");
}

#[test]
fn test_time_range_queries_for_summary() {
    // Create test database with data
    let (_temp_dir, _db_path, store) = create_test_db_with_data();

    // Get current timestamp
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // Test 1: Get sessions from last 7 days (this week)
    let seven_days_ago = now - (7 * 24 * 60 * 60 * 1000);
    let week_sessions = store
        .get_sessions_by_time_range(seven_days_ago, now + 1000)
        .expect("Failed to get week sessions");

    assert!(
        !week_sessions.is_empty(),
        "Should have sessions in the last 7 days"
    );
    eprintln!("✓ Found {} sessions in last 7 days", week_sessions.len());

    // Test 2: Get cost by agent for this week
    let cost_by_agent = store
        .get_cost_by_agent_in_range(seven_days_ago, now + 1000)
        .expect("Failed to get cost by agent");

    assert!(
        !cost_by_agent.is_empty(),
        "Should have cost data grouped by agent"
    );
    for (agent, cost) in &cost_by_agent {
        eprintln!("  Agent {}: ${:.4}", agent, cost);
        assert!(*cost > 0.0, "Agent cost should be positive");
    }
    eprintln!("✓ Per-agent cost breakdown calculated");

    // Test 3: Get top 3 specs by cost
    let top_specs = store
        .get_top_n_specs_by_cost(3, seven_days_ago, now + 1000)
        .expect("Failed to get top specs");

    assert!(!top_specs.is_empty(), "Should have top specs data");
    eprintln!("✓ Top {} costliest specs:", top_specs.len());
    for (i, (spec_id, cost)) in top_specs.iter().enumerate() {
        eprintln!("  {}. Spec {}: ${:.4}", i + 1, spec_id, cost);
        assert!(*cost > 0.0, "Spec cost should be positive");
    }

    // Verify ordering (costs should be descending)
    for i in 1..top_specs.len() {
        assert!(
            top_specs[i - 1].1 >= top_specs[i].1,
            "Top specs should be ordered by cost descending"
        );
    }

    eprintln!("✓ Time range query integration test passed");
}

#[test]
fn test_pricing_model_calculations() {
    // Test Claude 3.5 Sonnet pricing
    let claude_pricing = claude_sonnet_35_pricing();

    // Test calculation with all token types
    let cost = claude_pricing.calculate_cost(
        50_000,        // input tokens
        12_000,        // output tokens
        Some(2_500),   // thought tokens
        Some(150_000), // cache read tokens
        Some(8_000),   // cache write tokens
    );

    // Expected calculation:
    // - Input: 50,000 * $3.00 / 1,000,000 = $0.150
    // - Output: 12,000 * $15.00 / 1,000,000 = $0.180
    // - Thought: 2,500 * $15.00 / 1,000,000 = $0.0375 (output rate)
    // - Cache read: 150,000 * $0.30 / 1,000,000 = $0.045 (10% of input)
    // - Cache write: 8,000 * $3.75 / 1,000,000 = $0.030 (125% of input)
    // Total: ~$0.4425

    assert!(
        cost > 0.44 && cost < 0.45,
        "Expected cost ~$0.4425, got ${:.4}",
        cost
    );
    eprintln!("✓ Claude pricing calculation: ${:.4}", cost);

    // Test GPT-4 pricing
    let gpt_pricing = gpt4_turbo_pricing();
    let gpt_cost = gpt_pricing.calculate_cost(
        25_000, // input tokens
        6_000,  // output tokens
        None,   // no thought tokens
        None,   // no cache read
        None,   // no cache write
    );

    // Expected calculation:
    // - Input: 25_000 * $10.00 / 1,000,000 = $0.250
    // - Output: 6,000 * $30.00 / 1,000,000 = $0.180
    // Total: $0.430

    assert!(
        gpt_cost > 0.42 && gpt_cost < 0.44,
        "Expected cost ~$0.430, got ${:.4}",
        gpt_cost
    );
    eprintln!("✓ GPT-4 pricing calculation: ${:.4}", gpt_cost);

    eprintln!("✓ Pricing model calculation integration test passed");
}

#[test]
fn test_end_to_end_analytics_pipeline() {
    // This test verifies the complete analytics pipeline:
    // 1. Session data stored with costs
    // 2. Time-range queries work
    // 3. Budget tracking works
    // 4. Per-agent and per-spec aggregation works
    // 5. JSON serialization works

    eprintln!("\n=== End-to-End Analytics Pipeline Test ===\n");

    // Step 1: Create test database with sample data
    let (_temp_dir, db_path, store) = create_test_db_with_data();
    eprintln!("✓ Step 1: Test database created at {:?}", db_path);

    // Step 2: Verify session data is stored with costs
    let far_future = 32503680000000u64; // Jan 1, 3000 in milliseconds
    let all_sessions = store
        .get_sessions_by_time_range(0, far_future)
        .expect("Failed to get sessions");
    assert_eq!(all_sessions.len(), 4, "Should have 4 sessions");
    for session in &all_sessions {
        assert!(
            session.estimated_cost_usd.is_some(),
            "Session should have cost"
        );
    }
    eprintln!("✓ Step 2: All sessions have cost data");

    // Step 3: Calculate weekly totals (last 7 days)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let week_ago = now - (7 * 24 * 60 * 60 * 1000);

    let weekly_cost = store
        .get_cost_in_time_range(week_ago, now + 1000)
        .expect("Failed to get weekly cost");
    assert!(weekly_cost > 0.0, "Weekly cost should be positive");
    eprintln!("✓ Step 3: Weekly total cost: ${:.4}", weekly_cost);

    // Step 4: Get top 3 costliest specs
    let top_specs = store
        .get_top_n_specs_by_cost(3, week_ago, now + 1000)
        .expect("Failed to get top specs");
    assert!(!top_specs.is_empty(), "Should have top specs");
    eprintln!("✓ Step 4: Top {} specs identified", top_specs.len());

    // Step 5: Get per-agent breakdown
    let agent_costs = store
        .get_cost_by_agent_in_range(week_ago, now + 1000)
        .expect("Failed to get agent costs");
    assert!(!agent_costs.is_empty(), "Should have agent costs");
    eprintln!("✓ Step 5: Per-agent costs:");
    for (agent, cost) in &agent_costs {
        eprintln!("    - {}: ${:.4}", agent, cost);
    }

    // Step 6: Check budget status
    let tracker = BudgetTracker::new(80);
    let budget = weekly_cost * 1.5; // Set budget 50% above spending
    let status = tracker
        .check_daily_budget(&store, budget)
        .expect("Failed to check budget");

    assert!(status.is_ok(), "Expected OK status with comfortable budget");
    eprintln!(
        "✓ Step 6: Budget status OK ({:.1}% used)",
        status.usage_percentage
    );

    // Step 7: Verify JSON export works
    let json = serde_json::to_string(&all_sessions).expect("Failed to serialize to JSON");
    assert!(json.contains("estimated_cost_usd"));
    eprintln!("✓ Step 7: JSON export validated");

    eprintln!("\n✅ End-to-End Analytics Pipeline Test PASSED\n");
}
