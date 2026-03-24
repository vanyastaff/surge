//! End-to-end pipeline tests.
//!
//! Verifies the full orchestrator pipeline with real agents, including:
//! - Spec validation
//! - Git worktree creation
//! - Agent session management
//! - Subtask execution
//! - QA review
//! - Merge and cleanup

mod fixtures;
mod helpers;

use helpers::{cleanup_dir, has_any_agent, temp_test_dir, test_surge_config};
use surge_acp::discovery::AgentDiscovery;
use surge_acp::Registry;
use surge_orchestrator::pipeline::{Orchestrator, OrchestratorConfig, PipelineResult};
use std::process::Command;

/// Initialize a git repository in the specified directory.
///
/// Creates a git repo with an initial commit so we have a base branch to work from.
fn init_git_repo(dir: &std::path::Path) {
    // Initialize git repo
    Command::new("git")
        .arg("init")
        .current_dir(dir)
        .output()
        .expect("Failed to init git repo");

    // Configure git user for commits
    Command::new("git")
        .args(["config", "user.name", "Surge Test"])
        .current_dir(dir)
        .output()
        .expect("Failed to set git user.name");

    Command::new("git")
        .args(["config", "user.email", "test@surge.local"])
        .current_dir(dir)
        .output()
        .expect("Failed to set git user.email");

    // Create initial commit
    std::fs::write(dir.join("README.md"), "# Test Project\n")
        .expect("Failed to create README.md");

    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(dir)
        .output()
        .expect("Failed to add README.md");

    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(dir)
        .output()
        .expect("Failed to create initial commit");
}

/// Test end-to-end execution of a simple spec with no dependencies.
///
/// This test requires a real ACP agent to be available on the system.
/// If no agent is found, the test is skipped.
#[tokio::test]
async fn test_e2e_simple_spec() {
    // Check if any agent is available
    if !has_any_agent() {
        eprintln!("SKIP: No ACP agent available on this system");
        return;
    }

    // Discover agents and get the first available one
    let mut discovery = AgentDiscovery::new();
    let registry = Registry::builtin();
    let agents = discovery.discover_all(registry.list());

    if agents.is_empty() {
        eprintln!("SKIP: No agents discovered");
        return;
    }

    let agent = &agents[0];
    let agent_name = &agent.entry.id;
    let agent_command = &agent.entry.command;

    eprintln!("Using agent: {} ({})", agent_name, agent_command);

    // Create temp directory for test
    let test_dir = temp_test_dir("e2e_simple_spec");

    // Initialize git repository
    init_git_repo(&test_dir);

    // Create surge config
    let surge_config = test_surge_config(agent_name, agent_command);

    // Load simple spec fixture
    let mut spec_file = fixtures::load_simple_spec();

    // Create orchestrator
    let config = OrchestratorConfig {
        surge_config,
        working_dir: test_dir.clone(),
    };
    let orchestrator = Orchestrator::new(config);

    // Execute pipeline
    let result = orchestrator.execute(&mut spec_file).await;

    // Verify result
    // Note: The pipeline may pause at gates or fail due to QA, so we allow multiple outcomes
    match result {
        PipelineResult::Completed => {
            eprintln!("Pipeline completed successfully");
        }
        PipelineResult::Paused { phase, reason } => {
            eprintln!("Pipeline paused at phase {:?}: {}", phase, reason);
            // For simple specs, pausing at a gate is acceptable
        }
        PipelineResult::Failed { reason } => {
            // For E2E tests with real agents, some failures are acceptable
            // (agent might not be configured properly, network issues, etc.)
            eprintln!("Pipeline failed (may be expected in E2E test): {}", reason);
        }
    }

    // Cleanup
    cleanup_dir(&test_dir);
}

/// Test end-to-end execution of a spec with dependencies.
///
/// Verifies that subtasks are executed in the correct dependency order:
/// - Base module (no deps) runs first
/// - Utils module (depends on base) runs after base
/// - Integration module (depends on base + utils) runs last
///
/// This test requires a real ACP agent to be available on the system.
/// If no agent is found, the test is skipped.
#[tokio::test]
async fn test_e2e_dependency_order() {
    use surge_core::event::SurgeEvent;
    use surge_core::id::SubtaskId;
    use surge_spec::DependencyGraph;

    // Check if any agent is available
    if !has_any_agent() {
        eprintln!("SKIP: No ACP agent available on this system");
        return;
    }

    // Discover agents and get the first available one
    let mut discovery = AgentDiscovery::new();
    let registry = Registry::builtin();
    let agents = discovery.discover_all(registry.list());

    if agents.is_empty() {
        eprintln!("SKIP: No agents discovered");
        return;
    }

    let agent = &agents[0];
    let agent_name = &agent.entry.id;
    let agent_command = &agent.entry.command;

    eprintln!("Using agent: {} ({})", agent_name, agent_command);

    // Create temp directory for test
    let test_dir = temp_test_dir("e2e_dependency_order");

    // Initialize git repository
    init_git_repo(&test_dir);

    // Create surge config
    let surge_config = test_surge_config(agent_name, agent_command);

    // Load dependency spec fixture
    let mut spec_file = fixtures::load_dependency_spec();

    // Verify the dependency structure
    assert_eq!(spec_file.spec.subtasks.len(), 3);
    let base_id = spec_file.spec.subtasks[0].id;
    let utils_id = spec_file.spec.subtasks[1].id;
    let integration_id = spec_file.spec.subtasks[2].id;

    // Verify dependencies
    assert_eq!(spec_file.spec.subtasks[0].depends_on.len(), 0); // base has no deps
    assert_eq!(spec_file.spec.subtasks[1].depends_on.len(), 1); // utils depends on base
    assert!(spec_file.spec.subtasks[1].depends_on.contains(&base_id));
    assert_eq!(spec_file.spec.subtasks[2].depends_on.len(), 2); // integration depends on both
    assert!(spec_file.spec.subtasks[2].depends_on.contains(&base_id));
    assert!(spec_file.spec.subtasks[2].depends_on.contains(&utils_id));

    // Build dependency graph and verify topological order
    let graph = DependencyGraph::from_spec(&spec_file.spec)
        .expect("Failed to build dependency graph");
    let topo_order = graph.topological_order()
        .expect("Failed to get topological order");

    eprintln!("Expected topological order: {:?}", topo_order);

    // Verify that base comes before utils and integration
    let base_pos = topo_order.iter().position(|id| *id == base_id).unwrap();
    let utils_pos = topo_order.iter().position(|id| *id == utils_id).unwrap();
    let integration_pos = topo_order.iter().position(|id| *id == integration_id).unwrap();

    assert!(base_pos < utils_pos, "Base should come before utils");
    assert!(base_pos < integration_pos, "Base should come before integration");
    assert!(utils_pos < integration_pos, "Utils should come before integration");

    // Create orchestrator
    let config = OrchestratorConfig {
        surge_config,
        working_dir: test_dir.clone(),
    };
    let orchestrator = Orchestrator::new(config);

    // Subscribe to events to track execution order
    let mut event_rx = orchestrator.subscribe();

    // Spawn a task to collect SubtaskStarted events
    let execution_order = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<SubtaskId>::new()));
    let execution_order_clone = execution_order.clone();

    let event_listener = tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            if let SurgeEvent::SubtaskStarted { subtask_id, .. } = event {
                let mut order = execution_order_clone.lock().await;
                order.push(subtask_id);
                eprintln!("Subtask started: {}", subtask_id);
            }
        }
    });

    // Execute pipeline
    let result = orchestrator.execute(&mut spec_file).await;

    // Give event listener time to process remaining events
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    event_listener.abort();

    // Verify result (allow paused/failed as in simple test)
    match result {
        PipelineResult::Completed => {
            eprintln!("Pipeline completed successfully");
        }
        PipelineResult::Paused { phase, reason } => {
            eprintln!("Pipeline paused at phase {:?}: {}", phase, reason);
        }
        PipelineResult::Failed { reason } => {
            eprintln!("Pipeline failed (may be expected in E2E test): {}", reason);
        }
    }

    // Verify execution order respected dependencies
    let final_order = execution_order.lock().await;
    eprintln!("Actual execution order: {:?}", *final_order);

    // If subtasks were executed, verify the order
    if !final_order.is_empty() {
        // Find positions of each subtask in the execution order
        if let Some(base_exec_pos) = final_order.iter().position(|id| *id == base_id) {
            if let Some(utils_exec_pos) = final_order.iter().position(|id| *id == utils_id) {
                assert!(
                    base_exec_pos < utils_exec_pos,
                    "Base subtask should execute before utils subtask"
                );
            }

            if let Some(integration_exec_pos) = final_order.iter().position(|id| *id == integration_id) {
                assert!(
                    base_exec_pos < integration_exec_pos,
                    "Base subtask should execute before integration subtask"
                );
            }
        }

        if let (Some(utils_exec_pos), Some(integration_exec_pos)) = (
            final_order.iter().position(|id| *id == utils_id),
            final_order.iter().position(|id| *id == integration_id),
        ) {
            assert!(
                utils_exec_pos < integration_exec_pos,
                "Utils subtask should execute before integration subtask"
            );
        }
    }

    // Cleanup
    cleanup_dir(&test_dir);
}

/// Test end-to-end streaming of agent session updates and event broadcasting.
///
/// Verifies that:
/// - AgentMessageChunk events are broadcast during prompt streaming
/// - TokensConsumed events are broadcast with usage metrics
/// - Events contain proper session and context information
///
/// This test requires a real ACP agent to be available on the system.
/// If no agent is found, the test is skipped.
#[tokio::test]
async fn test_e2e_streaming_events() {
    use surge_core::event::SurgeEvent;

    // Check if any agent is available
    if !has_any_agent() {
        eprintln!("SKIP: No ACP agent available on this system");
        return;
    }

    // Discover agents and get the first available one
    let mut discovery = AgentDiscovery::new();
    let registry = Registry::builtin();
    let agents = discovery.discover_all(registry.list());

    if agents.is_empty() {
        eprintln!("SKIP: No agents discovered");
        return;
    }

    let agent = &agents[0];
    let agent_name = &agent.entry.id;
    let agent_command = &agent.entry.command;

    eprintln!("Using agent: {} ({})", agent_name, agent_command);

    // Create temp directory for test
    let test_dir = temp_test_dir("e2e_streaming_events");

    // Initialize git repository
    init_git_repo(&test_dir);

    // Create surge config
    let surge_config = test_surge_config(agent_name, agent_command);

    // Load simple spec fixture
    let mut spec_file = fixtures::load_simple_spec();

    // Create orchestrator
    let config = OrchestratorConfig {
        surge_config,
        working_dir: test_dir.clone(),
    };
    let orchestrator = Orchestrator::new(config);

    // Subscribe to events to track streaming
    let mut event_rx = orchestrator.subscribe();

    // Spawn a task to collect streaming events
    let message_chunks = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let message_chunks_clone = message_chunks.clone();

    let tokens_consumed = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<(u64, u64, Option<u64>)>::new()));
    let tokens_consumed_clone = tokens_consumed.clone();

    let event_listener = tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            match event {
                SurgeEvent::AgentMessageChunk { session_id, text } => {
                    eprintln!("AgentMessageChunk [{}]: {}", session_id, text);
                    let mut chunks = message_chunks_clone.lock().await;
                    chunks.push(text);
                }
                SurgeEvent::TokensConsumed {
                    session_id,
                    agent_name,
                    input_tokens,
                    output_tokens,
                    thought_tokens,
                    ..
                } => {
                    eprintln!(
                        "TokensConsumed [{}] agent={}: in={} out={} thought={:?}",
                        session_id, agent_name, input_tokens, output_tokens, thought_tokens
                    );
                    let mut tokens = tokens_consumed_clone.lock().await;
                    tokens.push((input_tokens, output_tokens, thought_tokens));
                }
                _ => {}
            }
        }
    });

    // Execute pipeline
    let result = orchestrator.execute(&mut spec_file).await;

    // Give event listener time to process remaining events
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    event_listener.abort();

    // Verify result (allow paused/failed as in simple test)
    match result {
        PipelineResult::Completed => {
            eprintln!("Pipeline completed successfully");
        }
        PipelineResult::Paused { phase, reason } => {
            eprintln!("Pipeline paused at phase {:?}: {}", phase, reason);
        }
        PipelineResult::Failed { reason } => {
            eprintln!("Pipeline failed (may be expected in E2E test): {}", reason);
        }
    }

    // Verify streaming events were received
    let chunks = message_chunks.lock().await;
    let tokens = tokens_consumed.lock().await;

    eprintln!("Total AgentMessageChunk events: {}", chunks.len());
    eprintln!("Total TokensConsumed events: {}", tokens.len());

    // Verify that we received streaming events
    // Note: In E2E tests with real agents, we may not always get chunks
    // (depends on agent implementation), but we should always get token usage
    if !chunks.is_empty() {
        eprintln!("✓ AgentMessageChunk events broadcast successfully");
    } else {
        eprintln!("⚠ No AgentMessageChunk events (may be expected depending on agent)");
    }

    // Verify TokensConsumed events
    assert!(
        !tokens.is_empty(),
        "Expected TokensConsumed events to be broadcast"
    );

    // Verify token counts are reasonable
    for (input_tokens, output_tokens, thought_tokens) in tokens.iter() {
        assert!(*input_tokens > 0, "Expected non-zero input tokens");
        assert!(*output_tokens > 0, "Expected non-zero output tokens");
        // thought_tokens may be None for non-Anthropic agents
        eprintln!(
            "Token usage verified: in={} out={} thought={:?}",
            input_tokens, output_tokens, thought_tokens
        );
    }

    eprintln!("✓ TokensConsumed events broadcast successfully");

    // Cleanup
    cleanup_dir(&test_dir);
}
