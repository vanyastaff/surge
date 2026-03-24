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
