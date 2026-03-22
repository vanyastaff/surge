use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Get the path to the surge binary
fn surge_bin() -> PathBuf {
    // For integration tests, cargo compiles the binary in target/debug or target/release
    let mut path = std::env::current_exe()
        .expect("Failed to get current executable path");

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
fn test_config_show() {
    let surge_bin = surge_bin();
    assert!(surge_bin.exists(), "surge binary not found at {:?}", surge_bin);

    // Test 1: Run config show without surge.toml (should use defaults)
    let temp_dir = std::env::temp_dir().join("surge_cli_test_no_config");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

    let output = Command::new(&surge_bin)
        .arg("config")
        .arg("show")
        .current_dir(&temp_dir)
        .output()
        .expect("Failed to execute surge config show");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge config show failed");

    // Verify output contains expected fields
    assert!(stdout.contains("Surge Configuration"), "Output should contain 'Surge Configuration'");
    assert!(stdout.contains("Default Agent:"), "Output should contain 'Default Agent:'");
    assert!(stdout.contains("Agents:"), "Output should contain 'Agents:'");
    assert!(stdout.contains("Pipeline:"), "Output should contain 'Pipeline:'");
    assert!(stdout.contains("max_qa_iterations:"), "Output should contain 'max_qa_iterations:'");
    assert!(stdout.contains("max_parallel:"), "Output should contain 'max_parallel:'");
    assert!(stdout.contains("Gates:"), "Output should contain 'Gates:'");

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);

    // Test 2: Run config show with surge.toml
    let temp_dir_with_config = std::env::temp_dir().join("surge_cli_test_with_config");
    let _ = fs::remove_dir_all(&temp_dir_with_config); // Clean up any previous test
    fs::create_dir_all(&temp_dir_with_config).expect("Failed to create temp dir");

    // Create a sample surge.toml
    let config_content = r#"default_agent = "test-agent"

[agents.test-agent]
command = "test-command"
args = ["--test"]
transport = "stdio"

[pipeline]
max_qa_iterations = 5
max_parallel = 2

[pipeline.gates]
after_spec = true
after_plan = false
after_each_subtask = true
after_qa = false
"#;

    let config_path = temp_dir_with_config.join("surge.toml");
    fs::write(&config_path, config_content).expect("Failed to write surge.toml");

    let output = Command::new(&surge_bin)
        .arg("config")
        .arg("show")
        .current_dir(&temp_dir_with_config)
        .output()
        .expect("Failed to execute surge config show with config file");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print output for debugging if test fails
    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
        eprintln!("STDOUT:\n{}", stdout);
        eprintln!("STDERR:\n{}", stderr);
    }

    assert!(output.status.success(), "surge config show failed with config file");

    // Verify output contains custom config values
    assert!(stdout.contains("test-agent"), "Output should contain 'test-agent'");
    assert!(stdout.contains("test-command"), "Output should contain 'test-command'");
    assert!(stdout.contains("max_qa_iterations: 5"), "Output should contain 'max_qa_iterations: 5'");
    assert!(stdout.contains("max_parallel: 2"), "Output should contain 'max_parallel: 2'");
    assert!(stdout.contains("after_spec: true"), "Output should contain 'after_spec: true'");
    assert!(stdout.contains("after_plan: false"), "Output should contain 'after_plan: false'");

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir_with_config);
}
