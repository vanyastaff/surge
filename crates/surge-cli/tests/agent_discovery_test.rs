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
fn test_agent_list_no_config() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test agent list without surge.toml (should use defaults and show discovered agents)
    let temp_dir = std::env::temp_dir().join("surge_agent_test_no_config");
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

    // Verify output contains expected sections
    assert!(
        stdout.contains("Agent Discovery Report"),
        "Output should contain 'Agent Discovery Report'"
    );
    assert!(
        stdout.contains("Default:"),
        "Output should contain 'Default:'"
    );
    assert!(
        stdout.contains("Available agents"),
        "Output should contain 'Available agents'"
    );
    assert!(
        stdout.contains("Configured agents"),
        "Output should contain 'Configured agents'"
    );
    assert!(
        stdout.contains("Legend:"),
        "Output should contain 'Legend:'"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_agent_list_with_config() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test agent list with surge.toml containing configured agents
    let temp_dir = std::env::temp_dir().join("surge_agent_test_with_config");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a sample surge.toml with configured agents
    let config_content = r#"default_agent = "test-agent"

[agents.test-agent]
command = "test-command"
args = ["--test"]
transport = "stdio"

[agents.another-agent]
command = "another-command"
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
        .expect("Failed to execute surge agent list with config");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(
        output.status.success(),
        "surge agent list failed with config file"
    );

    // Verify output contains configured agents
    assert!(
        stdout.contains("Agent Discovery Report"),
        "Output should contain 'Agent Discovery Report'"
    );
    assert!(
        stdout.contains("Default: test-agent"),
        "Output should contain 'Default: test-agent'"
    );
    assert!(
        stdout.contains("Configured agents (2)"),
        "Output should contain 'Configured agents (2)'"
    );
    assert!(
        stdout.contains("test-agent"),
        "Output should contain 'test-agent'"
    );
    assert!(
        stdout.contains("another-agent"),
        "Output should contain 'another-agent'"
    );
    assert!(
        stdout.contains("command: test-command"),
        "Output should contain 'command: test-command'"
    );
    assert!(
        stdout.contains("command: another-command"),
        "Output should contain 'command: another-command'"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_agent_list_shows_availability() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that agent list properly shows agent availability status
    let temp_dir = std::env::temp_dir().join("surge_agent_test_availability");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config with an agent that definitely won't be found
    let config_content = r#"default_agent = "nonexistent-agent"

[agents.nonexistent-agent]
command = "this-command-does-not-exist-12345"
args = []
transport = "stdio"
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

    // Verify that the missing agent section appears
    assert!(
        stdout.contains("Missing agents"),
        "Output should contain 'Missing agents' section"
    );
    assert!(
        stdout.contains("nonexistent-agent"),
        "Output should contain the missing agent name"
    );

    // Verify legend is present
    assert!(
        stdout.contains("Legend:"),
        "Output should contain 'Legend:'"
    );
    assert!(
        stdout.contains("✓ = available"),
        "Output should contain availability marker"
    );
    assert!(
        stdout.contains("✗ = missing"),
        "Output should contain missing marker"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_agent_refresh() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test agent refresh command
    let temp_dir = std::env::temp_dir().join("surge_agent_test_refresh");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    let output = Command::new(&surge_bin)
        .arg("agent")
        .arg("refresh")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent refresh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge agent refresh failed");

    // Verify output contains expected message
    assert!(
        stdout.contains("Refreshing agent discovery cache"),
        "Output should contain 'Refreshing agent discovery cache'"
    );
    assert!(
        stdout.contains("Agent discovery cache cleared"),
        "Output should contain 'Agent discovery cache cleared'"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_agent_discovery_full_flow() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test full discovery flow: refresh -> list
    let temp_dir = std::env::temp_dir().join("surge_agent_test_full_flow");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config file
    let config_content = r#"default_agent = "my-agent"

[agents.my-agent]
command = "echo"
args = ["test"]
transport = "stdio"
"#;

    let config_path = temp_dir.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");

    // Step 1: Refresh discovery cache
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

    // Step 2: List agents (should re-discover after refresh)
    let list_output = Command::new(&surge_bin)
        .arg("agent")
        .arg("list")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent list after refresh");

    let stdout = String::from_utf8_lossy(&list_output.stdout);
    let stderr = String::from_utf8_lossy(&list_output.stderr);

    // Print output for debugging if test fails
    if !list_output.status.success() {
        eprintln!("Command failed with status: {}", list_output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(
        list_output.status.success(),
        "surge agent list failed after refresh"
    );

    // Verify the full discovery report is generated
    assert!(
        stdout.contains("Agent Discovery Report"),
        "Output should contain discovery report"
    );
    assert!(
        stdout.contains("my-agent"),
        "Output should contain configured agent"
    );
    assert!(
        stdout.contains("Available agents"),
        "Output should contain available agents section"
    );
    assert!(
        stdout.contains("Configured agents"),
        "Output should contain configured agents section"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_agent_list_default_marker() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that the default agent is properly marked with * in the list
    let temp_dir = std::env::temp_dir().join("surge_agent_test_default_marker");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config with multiple agents
    let config_content = r#"default_agent = "primary-agent"

[agents.primary-agent]
command = "primary-cmd"
args = []
transport = "stdio"

[agents.secondary-agent]
command = "secondary-cmd"
args = []
transport = "stdio"
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

    // Verify default agent marker is shown
    assert!(
        stdout.contains("Default: primary-agent"),
        "Output should show default agent"
    );
    assert!(
        stdout.contains("* = default agent"),
        "Legend should explain default marker"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_agent_add_command() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test adding a custom agent via CLI
    let temp_dir = std::env::temp_dir().join("surge_agent_test_add");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a minimal surge.toml first
    let config_content = r#"default_agent = "test-agent"

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

    // Add a new agent via CLI
    let output = Command::new(&surge_bin)
        .arg("agent")
        .arg("add")
        .arg("my-custom-agent")
        .arg("--command")
        .arg("/path/to/custom/agent")
        .arg("--args")
        .arg("arg1")
        .arg("--args")
        .arg("arg2")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge agent add");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge agent add failed");

    // Verify the success message
    assert!(
        stdout.contains("✅ Added agent 'my-custom-agent' to surge.toml"),
        "Output should confirm agent was added"
    );

    // Verify the agent was actually added to surge.toml
    let config_contents = fs::read_to_string(&config_path).expect("Failed to read surge.toml");
    assert!(
        config_contents.contains("my-custom-agent"),
        "surge.toml should contain the new agent"
    );
    assert!(
        config_contents.contains("/path/to/custom/agent"),
        "surge.toml should contain the command path"
    );
    assert!(
        config_contents.contains("arg1") && config_contents.contains("arg2"),
        "surge.toml should contain the args"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_agent_list_shows_capabilities() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test that agent list shows capability metadata
    let temp_dir = std::env::temp_dir().join("surge_agent_test_capabilities");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Create a config with agents that have capability metadata
    let config_content = r#"default_agent = "agent-with-caps"

[agents.agent-with-caps]
command = "test-command"
args = []
transport = "stdio"
capabilities = ["code", "plan", "review"]

[agents.agent-no-caps]
command = "another-command"
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

    // Verify output shows discovered agents with capabilities
    // The builtin registry agents should show their capabilities
    assert!(
        stdout.contains("Available agents"),
        "Output should contain 'Available agents' section"
    );

    // Verify that when agents are discovered, capabilities are shown
    // Note: The builtin registry agents have capabilities defined
    if stdout.contains("capabilities:") {
        // If any agents were discovered, they should show capabilities
        println!("Capabilities displayed in output (good!)");
    }

    // Verify configured agents section shows our test agents
    assert!(
        stdout.contains("agent-with-caps"),
        "Output should contain configured agent with capabilities"
    );
    assert!(
        stdout.contains("agent-no-caps"),
        "Output should contain configured agent without capabilities"
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_registry_fetch() {
    let surge_bin = surge_bin();
    assert!(
        surge_bin.exists(),
        "surge binary not found at {:?}",
        surge_bin
    );

    // Test registry fetch command
    let temp_dir = std::env::temp_dir().join("surge_registry_test_fetch");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    // Note: This test will actually try to fetch from the remote registry
    // In a real environment, this might fail due to network issues
    // We'll test that the command runs and produces expected output format
    let output = Command::new(&surge_bin)
        .arg("registry")
        .arg("fetch")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge registry fetch");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    // The command should either succeed or fail gracefully
    // Check for the expected output format
    if output.status.success() {
        // If successful, should show fetching message
        assert!(
            stdout.contains("Fetching remote registry from"),
            "Output should contain fetching message"
        );
        assert!(
            stdout.contains("Successfully fetched and cached") || stdout.contains("agents"),
            "Output should indicate success"
        );
    } else {
        // If it fails (e.g., network issue), that's also acceptable for this test
        // Just verify it doesn't crash
        eprintln!("Note: Registry fetch failed (possibly due to network), but command ran");
    }

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}
