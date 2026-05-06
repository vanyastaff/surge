//! Test helpers for surge-orchestrator E2E tests.
//!
//! Provides utilities for:
//! - Binary path detection
//! - Temp directory/database creation
//! - Agent detection and configuration
//! - Spec loading

use std::fs;
use std::path::{Path, PathBuf};
use surge_acp::Registry;
use surge_acp::discovery::AgentDiscovery;
use surge_core::config::{AgentConfig, PipelineConfig, SurgeConfig, Transport};
use surge_spec::SpecFile;

// ── Binary detection ────────────────────────────────────────────────

/// Get the path to the surge binary.
///
/// For integration tests, cargo compiles the binary in target/debug or target/release.
/// This function navigates from the test binary location to find the main surge executable.
#[must_use]
pub fn surge_bin() -> PathBuf {
    let mut path = std::env::current_exe().expect("Failed to get current executable path");

    // The test binary is in target/debug/deps, so we go up to target/debug
    path.pop(); // Remove test binary name
    if path.ends_with("deps") {
        path.pop(); // Remove deps
    }

    // Add surge binary
    path.push("surge");
    if cfg!(windows) {
        path.set_extension("exe");
    }

    path
}

// ── Temp path helpers ───────────────────────────────────────────────

/// Create a unique temp database file path for a test.
///
/// Uses the test name and process ID to ensure uniqueness across parallel test runs.
#[must_use]
pub fn temp_db_path(test_name: &str) -> PathBuf {
    let temp_dir = std::env::temp_dir();
    temp_dir.join(format!("surge-e2e-{}-{}.db", test_name, std::process::id()))
}

/// Create a unique temp directory for a test.
///
/// The directory is created if it doesn't exist. Returns the path to the directory.
pub fn temp_test_dir(test_name: &str) -> PathBuf {
    let temp_dir =
        std::env::temp_dir().join(format!("surge-e2e-{}-{}", test_name, std::process::id()));

    // Clean up any previous test run
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Failed to create temp test directory");

    temp_dir
}

// ── Config helpers ──────────────────────────────────────────────────

/// Create a minimal surge.toml config for testing.
///
/// Returns a string containing a valid TOML configuration with the specified default agent.
#[must_use]
pub fn minimal_surge_config(default_agent: &str, agent_command: &str) -> String {
    format!(
        r#"default_agent = "{default_agent}"

[agents.{default_agent}]
command = "{agent_command}"
args = []
transport = "stdio"

[pipeline]
max_qa_iterations = 3
max_parallel = 2

[pipeline.gates]
after_spec = true
after_plan = false
after_each_subtask = false
after_qa = false
"#
    )
}

/// Create a test surge.toml file in the specified directory.
///
/// Returns the path to the created config file.
pub fn create_test_config(dir: &Path, default_agent: &str, agent_command: &str) -> PathBuf {
    let config_content = minimal_surge_config(default_agent, agent_command);
    let config_path = dir.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");
    config_path
}

/// Create a `SurgeConfig` struct for testing with a single agent.
#[must_use]
pub fn test_surge_config(agent_name: &str, command: &str) -> SurgeConfig {
    let mut agents = std::collections::HashMap::new();
    agents.insert(
        agent_name.to_string(),
        AgentConfig {
            command: command.to_string(),
            args: vec![],
            transport: Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
        },
    );

    SurgeConfig {
        default_agent: agent_name.to_string(),
        agents,
        pipeline: PipelineConfig::default(),
        routing: surge_core::config::RoutingConfig::default(),
        cleanup: surge_core::config::CleanupPolicy::default(),
        ide: surge_core::config::IdeConfig::default(),
        resilience: surge_core::config::ResilienceConfig::default(),
        log: surge_core::config::LogConfig::default(),
        analytics: surge_core::config::AnalyticsConfig::default(),
        task_sources: vec![],
        telegram: None,
        inbox: surge_core::config::InboxConfig::default(),
    }
}

// ── Agent discovery helpers ─────────────────────────────────────────

/// Create an `AgentDiscovery` instance.
///
/// Returns a new discovery instance for the current platform.
#[must_use]
pub fn discover_agents() -> AgentDiscovery {
    AgentDiscovery::new()
}

/// Check if any ACP-compatible agent is available on the system.
///
/// Returns true if at least one agent (Claude, Copilot, Codex, or Gemini) is found.
#[must_use]
#[allow(dead_code)]
pub fn has_any_agent() -> bool {
    let mut discovery = discover_agents();
    let registry = Registry::builtin();
    let agents = discovery.discover_all(registry.list());
    !agents.is_empty()
}

// ── Spec loading helpers ────────────────────────────────────────────

/// Get the path to the fixtures directory.
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Load the simple spec fixture.
#[must_use]
pub fn load_simple_spec() -> SpecFile {
    let path = fixtures_dir().join("simple_spec.toml");
    SpecFile::load(&path).expect("Failed to load simple_spec.toml")
}

/// Load the dependency spec fixture.
#[must_use]
pub fn load_dependency_spec() -> SpecFile {
    let path = fixtures_dir().join("dependency_spec.toml");
    SpecFile::load(&path).expect("Failed to load dependency_spec.toml")
}

/// Get path to a fixture file by name.
#[must_use]
pub fn fixture_path(name: &str) -> PathBuf {
    fixtures_dir().join(name)
}

// ── Cleanup helpers ─────────────────────────────────────────────────

/// Clean up a temp database file.
///
/// Ignores errors if the file doesn't exist.
#[allow(dead_code)]
pub fn cleanup_db(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Clean up a temp directory.
///
/// Ignores errors if the directory doesn't exist.
pub fn cleanup_dir(path: &PathBuf) {
    let _ = fs::remove_dir_all(path);
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_surge_bin_path() {
        let bin = surge_bin();
        // Path should end with surge or surge.exe
        let file_name = bin.file_name().unwrap().to_str().unwrap();
        assert!(
            file_name == "surge" || file_name == "surge.exe",
            "Binary name should be 'surge' or 'surge.exe', got '{}'",
            file_name
        );
    }

    #[test]
    fn test_temp_db_path() {
        let path = temp_db_path("test");
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert!(file_name.starts_with("surge-e2e-test-"));
        assert!(file_name.ends_with(".db"));
    }

    #[test]
    fn test_temp_test_dir() {
        let dir = temp_test_dir("test_dir");
        assert!(dir.exists(), "Temp directory should be created");
        assert!(dir.is_dir(), "Path should be a directory");

        // Clean up
        cleanup_dir(&dir);
    }

    #[test]
    fn test_minimal_surge_config() {
        let config = minimal_surge_config("test-agent", "test-command");
        assert!(config.contains("default_agent = \"test-agent\""));
        assert!(config.contains("[agents.test-agent]"));
        assert!(config.contains("command = \"test-command\""));
        assert!(config.contains("transport = \"stdio\""));
    }

    #[test]
    fn test_create_test_config() {
        let dir = temp_test_dir("config_test");
        let config_path = create_test_config(&dir, "test-agent", "test-command");

        assert!(config_path.exists(), "Config file should exist");
        let content = fs::read_to_string(&config_path).expect("Failed to read config file");
        assert!(content.contains("default_agent = \"test-agent\""));

        // Clean up
        cleanup_dir(&dir);
    }

    #[test]
    fn test_test_surge_config() {
        let config = test_surge_config("test-agent", "test-command");
        assert_eq!(config.default_agent, "test-agent");
        assert!(config.agents.contains_key("test-agent"));
        assert_eq!(config.agents["test-agent"].command, "test-command");
    }

    #[test]
    fn test_discover_agents() {
        let discovery = discover_agents();
        // Just verify we can create a discovery instance
        // Actual agent detection depends on the system
        assert_eq!(
            discovery.platform(),
            surge_acp::discovery::Platform::current()
        );
    }

    #[test]
    fn test_fixtures_dir_exists() {
        let dir = fixtures_dir();
        assert!(dir.exists(), "Fixtures directory should exist");
        assert!(dir.is_dir(), "Fixtures path should be a directory");
    }

    #[test]
    fn test_fixture_path() {
        let path = fixture_path("simple_spec.toml");
        assert!(path.ends_with("fixtures/simple_spec.toml"));
    }

    #[test]
    fn test_load_simple_spec() {
        let spec_file = load_simple_spec();
        assert_eq!(spec_file.spec.title, "Simple test feature");
        assert_eq!(spec_file.spec.subtasks.len(), 1);
    }

    #[test]
    fn test_load_dependency_spec() {
        let spec_file = load_dependency_spec();
        assert_eq!(spec_file.spec.title, "Feature with dependencies");
        assert_eq!(spec_file.spec.subtasks.len(), 3);
    }
}
