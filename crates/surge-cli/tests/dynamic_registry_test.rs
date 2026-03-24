use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Get the path to the surge binary
fn surge_bin() -> PathBuf {
    // For integration tests, cargo compiles the binary in target/debug or target/release
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

#[test]
fn test_dynamic_registry_builtin_agents() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that builtin registry agents are discovered
    let temp_dir = std::env::temp_dir().join("surge_dynamic_registry_builtin");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    let output = Command::new(&surge_bin)
        .arg("agent")
        .arg("list")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge agent list failed");

    // Verify builtin registry agents are shown
    assert!(
        stdout.contains("Available agents"),
        "Output should contain 'Available agents' section"
    );

    // The builtin registry should include well-known agents
    // At minimum, we should see some discovered agents or configured agents section
    assert!(
        stdout.contains("Configured agents") || stdout.contains("Available agents"),
        "Output should show some agents from builtin registry"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_dynamic_registry_config_override() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that config-based agents override builtin registry agents
    let temp_dir = std::env::temp_dir().join("surge_dynamic_registry_override");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config that overrides a builtin agent
    let config_content = r#"default_agent = "claude"

[agents.claude]
command = "/custom/path/to/claude"
args = ["--custom-flag"]
transport = "stdio"

[pipeline]
max_qa_iterations = 3
max_parallel = 2

[pipeline.gates]
after_spec = true
after_plan = false
after_each_subtask = false
after_qa = false
"#;

    let config_path = temp_dir.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");

    let output = Command::new(&surge_bin)
        .arg("agent")
        .arg("list")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge agent list failed");

    // Verify the configured agent appears
    assert!(
        stdout.contains("claude"),
        "Output should contain the configured agent 'claude'"
    );
    assert!(
        stdout.contains("/custom/path/to/claude"),
        "Output should show the custom command path"
    );

    // Verify configured agents section exists
    assert!(
        stdout.contains("Configured agents"),
        "Output should contain 'Configured agents' section"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_dynamic_registry_merge_behavior() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that config agents are merged with builtin registry
    let temp_dir = std::env::temp_dir().join("surge_dynamic_registry_merge");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config with a custom agent (not in builtin registry)
    let config_content = r#"default_agent = "my-custom-agent"

[agents.my-custom-agent]
command = "/path/to/custom/agent"
args = ["--mode", "custom"]
transport = "stdio"
capabilities = ["code", "test"]

[pipeline]
max_qa_iterations = 3
max_parallel = 2

[pipeline.gates]
after_spec = true
after_plan = false
after_each_subtask = false
after_qa = false
"#;

    let config_path = temp_dir.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");

    let output = Command::new(&surge_bin)
        .arg("agent")
        .arg("list")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge agent list failed");

    // Verify the custom agent appears in configured section
    assert!(
        stdout.contains("my-custom-agent"),
        "Output should contain the custom agent"
    );
    assert!(
        stdout.contains("Configured agents"),
        "Output should have configured agents section"
    );

    // Verify default is set correctly
    assert!(
        stdout.contains("Default: my-custom-agent"),
        "Output should show custom agent as default"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_dynamic_registry_capabilities_display() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that capabilities from both builtin and config are displayed
    let temp_dir = std::env::temp_dir().join("surge_dynamic_registry_capabilities");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config with agents that have explicit capabilities
    let config_content = r#"default_agent = "agent-with-capabilities"

[agents.agent-with-capabilities]
command = "test-agent"
args = []
transport = "stdio"
capabilities = ["code", "plan", "review", "test"]

[agents.agent-without-capabilities]
command = "simple-agent"
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
"#;

    let config_path = temp_dir.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");

    let output = Command::new(&surge_bin)
        .arg("agent")
        .arg("list")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge agent list failed");

    // Verify configured agents appear
    assert!(
        stdout.contains("agent-with-capabilities"),
        "Output should contain agent with capabilities"
    );
    assert!(
        stdout.contains("agent-without-capabilities"),
        "Output should contain agent without capabilities"
    );

    // Verify report structure
    assert!(
        stdout.contains("Agent Discovery Report"),
        "Output should contain discovery report header"
    );
    assert!(
        stdout.contains("Configured agents"),
        "Output should have configured agents section"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_dynamic_registry_empty_config() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that builtin registry works when config has no agents
    let temp_dir = std::env::temp_dir().join("surge_dynamic_registry_empty");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a minimal config without any custom agents
    // Uses a builtin agent as default
    let config_content = r#"default_agent = "claude"

[pipeline]
max_qa_iterations = 3
max_parallel = 2

[pipeline.gates]
after_spec = true
after_plan = false
after_each_subtask = false
after_qa = false
"#;

    let config_path = temp_dir.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");

    let output = Command::new(&surge_bin)
        .arg("agent")
        .arg("list")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge agent list should succeed");

    // Should still show discovery report with available agents from builtin registry
    assert!(
        stdout.contains("Agent Discovery Report"),
        "Output should contain discovery report"
    );
    assert!(
        stdout.contains("Available agents") || stdout.contains("Configured agents (0)"),
        "Output should show builtin agents or indicate no configured agents"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_dynamic_registry_multiple_agents() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test discovery with multiple configured agents
    let temp_dir = std::env::temp_dir().join("surge_dynamic_registry_multiple");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config with multiple agents
    let config_content = r#"default_agent = "agent-one"

[agents.agent-one]
command = "agent1"
args = ["--flag1"]
transport = "stdio"
capabilities = ["code"]

[agents.agent-two]
command = "agent2"
args = ["--flag2"]
transport = "stdio"
capabilities = ["plan"]

[agents.agent-three]
command = "agent3"
args = []
transport = "stdio"
capabilities = ["review", "test"]

[pipeline]
max_qa_iterations = 3
max_parallel = 2

[pipeline.gates]
after_spec = true
after_plan = false
after_each_subtask = false
after_qa = false
"#;

    let config_path = temp_dir.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");

    let output = Command::new(&surge_bin)
        .arg("agent")
        .arg("list")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge agent list failed");

    // Verify all agents are shown
    assert!(
        stdout.contains("Configured agents (3)"),
        "Output should show 3 configured agents"
    );
    assert!(
        stdout.contains("agent-one"),
        "Output should contain agent-one"
    );
    assert!(
        stdout.contains("agent-two"),
        "Output should contain agent-two"
    );
    assert!(
        stdout.contains("agent-three"),
        "Output should contain agent-three"
    );

    // Verify default is correctly set
    assert!(
        stdout.contains("Default: agent-one"),
        "Output should show agent-one as default"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_dynamic_registry_with_refresh() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that refresh works with dynamic registry
    let temp_dir = std::env::temp_dir().join("surge_dynamic_registry_refresh");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config
    let config_content = r#"default_agent = "test-agent"

[agents.test-agent]
command = "echo"
args = ["hello"]
transport = "stdio"

[pipeline]
max_qa_iterations = 3
max_parallel = 2

[pipeline.gates]
after_spec = true
after_plan = false
after_each_subtask = false
after_qa = false
"#;

    let config_path = temp_dir.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");

    // First, refresh the cache
    let refresh_output = Command::new(&surge_bin)
        .arg("agent")
        .arg("refresh")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent refresh");

    assert!(
        refresh_output.status.success(),
        "surge agent refresh failed"
    );

    // Then list agents to verify dynamic registry still works after refresh
    let list_output = Command::new(&surge_bin)
        .arg("agent")
        .arg("list")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent list");

    let stdout = String::from_utf8_lossy(&list_output.stdout);
    let stderr = String::from_utf8_lossy(&list_output.stderr);

    // Print output for debugging if test fails
    if !list_output.status.success() {
        eprintln!("Command failed with status: {}", list_output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(list_output.status.success(), "surge agent list failed");

    // Verify the registry is working properly after refresh
    assert!(
        stdout.contains("Agent Discovery Report"),
        "Output should contain discovery report"
    );
    assert!(
        stdout.contains("test-agent"),
        "Output should contain the configured agent"
    );
    assert!(
        stdout.contains("Configured agents"),
        "Output should have configured agents section"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}
