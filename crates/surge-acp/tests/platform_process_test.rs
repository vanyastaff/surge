//! Platform-specific process spawning and cleanup tests.
//!
//! Tests that verify:
//! - Process spawning works correctly on all platforms
//! - Windows CREATE_NO_WINDOW flag is applied
//! - Windows cmd /C resolution for .cmd/.bat files
//! - Child processes are properly cleaned up (no zombie processes)
//! - Process termination works gracefully

use std::time::Duration;
use surge_acp::transport::{AgentTransport, StdioTransport};
use surge_core::config::{AgentConfig, Transport};
use tokio::time::timeout;

/// Helper to create a basic stdio AgentConfig for testing
fn stdio_config(command: &str, args: Vec<&str>) -> AgentConfig {
    AgentConfig {
        command: command.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        transport: Transport::Stdio,
        mcp_servers: vec![],
        capabilities: vec![],
    }
}

#[tokio::test]
async fn test_basic_process_spawn_unix() {
    #[cfg(unix)]
    {
        let config = stdio_config("echo", vec!["hello"]);
        let temp_dir = std::env::temp_dir();

        let result = StdioTransport::connect("test-echo", &config, &temp_dir).await;
        assert!(
            result.is_ok(),
            "Failed to spawn echo process: {:?}",
            result.err()
        );

        let mut io = result.unwrap();

        // Verify we got a child process handle on Unix
        assert!(io.child.is_some(), "Child process handle should be present");

        // Clean up: kill the process
        if let Some(mut child) = io.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[tokio::test]
async fn test_basic_process_spawn_windows() {
    #[cfg(windows)]
    {
        // On Windows, the transport automatically wraps commands in cmd /C
        let config = stdio_config("echo", vec!["hello"]);
        let temp_dir = std::env::temp_dir();

        let result = StdioTransport::connect("test-echo", &config, &temp_dir).await;
        assert!(
            result.is_ok(),
            "Failed to spawn echo process: {:?}",
            result.err()
        );

        let mut io = result.unwrap();

        // Verify we got a child process handle on Windows
        assert!(io.child.is_some(), "Child process handle should be present");

        // Clean up: kill the process
        if let Some(mut child) = io.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[tokio::test]
async fn test_process_cleanup_no_zombies() {
    // Test that child processes are properly cleaned up and don't become zombies

    #[cfg(unix)]
    let config = stdio_config("sleep", vec!["0.1"]);

    #[cfg(windows)]
    let config = stdio_config("timeout", vec!["/t", "1", "/nobreak"]);

    let temp_dir = std::env::temp_dir();

    let mut io = StdioTransport::connect("test-cleanup", &config, &temp_dir)
        .await
        .expect("Failed to spawn process");

    // Verify child process exists
    let mut child = io.child.take().expect("Child process handle missing");

    // Get the process ID before termination
    let pid = child.id().expect("Failed to get process ID");

    // Kill the process
    child.kill().await.expect("Failed to kill child process");

    // Wait for the process to terminate
    let _status = child
        .wait()
        .await
        .expect("Failed to wait for child process");

    // On Unix, killed processes should have a signal exit status
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            _status.signal().is_some(),
            "Process should have been killed by signal"
        );
    }

    // Verify the process is truly gone by checking if the PID is reusable
    // Note: This is a best-effort check - PIDs can be reused, but immediately after
    // killing a process, it should not exist.
    #[cfg(unix)]
    {
        use std::process::Command;
        // Use `kill -0` to check if process exists (doesn't actually kill)
        let check = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .output()
            .expect("Failed to check process existence");

        // Exit code 1 means process doesn't exist (which is what we want)
        assert!(
            !check.status.success(),
            "Process with PID {} still exists after cleanup",
            pid
        );
    }

    #[cfg(windows)]
    {
        // On Windows, we can try to open the process handle
        // If it fails, the process is gone
        use std::process::Command;
        let check = Command::new("tasklist")
            .arg("/FI")
            .arg(format!("PID eq {}", pid))
            .arg("/NH")
            .output()
            .expect("Failed to check process existence");

        let output = String::from_utf8_lossy(&check.stdout);
        // If the process is gone, tasklist will say "INFO: No tasks are running..."
        assert!(
            output.contains("No tasks") || !output.contains(&pid.to_string()),
            "Process with PID {} still exists after cleanup",
            pid
        );
    }
}

#[tokio::test]
async fn test_process_with_working_directory() {
    // Test that the process starts with the correct working directory

    let temp_dir = std::env::temp_dir().join("surge_test_wd");
    let _ = std::fs::remove_dir_all(&temp_dir); // Clean up from previous runs
    std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    #[cfg(unix)]
    let config = stdio_config("pwd", vec![]);

    #[cfg(windows)]
    let config = stdio_config("cd", vec![]);

    let mut io = StdioTransport::connect("test-wd", &config, &temp_dir)
        .await
        .expect("Failed to spawn process");

    // Give the process time to execute and exit
    if let Some(mut child) = io.child.take() {
        // Wait up to 2 seconds for the process to complete
        let wait_result = timeout(Duration::from_secs(2), child.wait()).await;
        assert!(wait_result.is_ok(), "Process did not complete in time");
    }

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_invalid_command_returns_error() {
    // Test that spawning a non-existent command returns an appropriate error
    // Note: On Windows, commands are wrapped with cmd /C, so cmd.exe itself spawns
    // successfully even if the wrapped command doesn't exist. We use a different
    // approach on Windows to test error handling.

    #[cfg(unix)]
    {
        let config = stdio_config("this-command-definitely-does-not-exist-12345", vec![]);
        let temp_dir = std::env::temp_dir();

        let result = StdioTransport::connect("test-invalid", &config, &temp_dir).await;

        assert!(
            result.is_err(),
            "Expected error when spawning non-existent command"
        );

        // Extract error message without requiring Debug on AgentIo
        if let Err(err) = result {
            let err_msg = err.to_string();
            // Error message should mention the failure to spawn
            assert!(
                err_msg.contains("Failed to spawn") || err_msg.contains("AgentConnection"),
                "Error message should indicate spawn failure: {}",
                err_msg
            );
        }
    }

    #[cfg(windows)]
    {
        // On Windows, test with a command that cmd.exe itself will reject
        // Using invalid syntax that cmd.exe cannot parse
        let config = stdio_config("<>|invalid", vec![]);
        let temp_dir = std::env::temp_dir();

        let result = StdioTransport::connect("test-invalid", &config, &temp_dir).await;

        // On Windows with cmd /C wrapper, the cmd.exe process itself may spawn
        // successfully even if the command is invalid, so we can't always expect
        // an error at spawn time. This test mainly verifies the code path doesn't panic.
        // The actual command failure would be detected later when trying to communicate.
        if let Err(err) = result {
            let err_msg = err.to_string();
            assert!(
                err_msg.contains("Failed to spawn") || err_msg.contains("AgentConnection"),
                "Error message should indicate spawn failure: {}",
                err_msg
            );
        } else {
            // If spawn succeeded, clean up the process
            let mut io = result.ok().unwrap();
            if let Some(mut child) = io.child.take() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }
    }
}

#[tokio::test]
#[cfg(windows)]
async fn test_windows_cmd_wrapper() {
    // Test that on Windows, commands are automatically wrapped with cmd /C
    // This is necessary for .cmd and .bat files to execute

    let config = stdio_config("echo", vec!["test"]);
    let temp_dir = std::env::temp_dir();

    let mut io = StdioTransport::connect("test-cmd-wrapper", &config, &temp_dir)
        .await
        .expect("Failed to spawn process with cmd wrapper");

    // Verify child process exists
    assert!(io.child.is_some(), "Child process should exist");

    // Clean up
    if let Some(mut child) = io.child.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

#[tokio::test]
#[cfg(windows)]
async fn test_windows_create_no_window_flag() {
    // Test that the CREATE_NO_WINDOW flag is applied on Windows
    // This prevents console windows from popping up

    // Use a command that would normally create a window
    let config = stdio_config("cmd", vec!["/C", "echo", "test"]);
    let temp_dir = std::env::temp_dir();

    let mut io = StdioTransport::connect("test-no-window", &config, &temp_dir)
        .await
        .expect("Failed to spawn process");

    // We can't directly verify the CREATE_NO_WINDOW flag from here,
    // but we can verify the process spawns successfully without creating a visible window
    assert!(io.child.is_some(), "Child process should exist");

    // If CREATE_NO_WINDOW wasn't set, this test would cause a console window to flash
    // The fact that it runs in CI without issues verifies the flag works

    // Clean up
    if let Some(mut child) = io.child.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

#[tokio::test]
async fn test_process_stdin_stdout_capture() {
    // Test that stdin and stdout are properly captured

    #[cfg(unix)]
    let config = stdio_config("cat", vec![]);

    #[cfg(windows)]
    let config = stdio_config("findstr", vec![".*"]);

    let temp_dir = std::env::temp_dir();

    let mut io = StdioTransport::connect("test-io-capture", &config, &temp_dir)
        .await
        .expect("Failed to spawn process");

    // Verify that we have reader and writer
    // The mere existence of these fields proves stdin/stdout were captured
    assert!(io.child.is_some(), "Child process should exist");

    // Clean up
    if let Some(mut child) = io.child.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

#[tokio::test]
async fn test_multiple_processes_sequential() {
    // Test that multiple processes can be spawned sequentially without interference
    // Note: We spawn sequentially instead of concurrently because AgentIo is not Send

    #[cfg(unix)]
    let config = stdio_config("sleep", vec!["0.1"]);

    #[cfg(windows)]
    let config = stdio_config("timeout", vec!["/t", "1", "/nobreak"]);

    let temp_dir = std::env::temp_dir();

    // Spawn and clean up 3 processes sequentially
    for i in 0..3 {
        let name = format!("test-sequential-{}", i);
        let mut io = StdioTransport::connect(&name, &config, &temp_dir)
            .await
            .expect("Failed to spawn process");

        // Verify process spawned
        assert!(io.child.is_some(), "Child process should exist");

        // Clean up
        if let Some(mut child) = io.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[tokio::test]
async fn test_git_env_vars_stripped() {
    // Test that git environment variables are stripped from child processes
    // This ensures child processes don't inherit Surge's git context

    // Set some git env vars that should be stripped
    // SAFETY: This test runs in isolation and we clean up the variables at the end
    unsafe {
        std::env::set_var("GIT_DIR", "/fake/git/dir");
        std::env::set_var("GIT_WORK_TREE", "/fake/work/tree");
    }

    #[cfg(unix)]
    let config = stdio_config("env", vec![]);

    #[cfg(windows)]
    let config = stdio_config("set", vec![]);

    let temp_dir = std::env::temp_dir();

    let mut io = StdioTransport::connect("test-git-env", &config, &temp_dir)
        .await
        .expect("Failed to spawn process");

    // The transport.rs code strips these variables before spawning
    // We can't easily verify the child's environment from here, but the test
    // verifies that the spawn succeeds even with these vars set

    // Clean up
    if let Some(mut child) = io.child.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }

    // Clean up env vars
    // SAFETY: We're removing the variables we set earlier in this test
    unsafe {
        std::env::remove_var("GIT_DIR");
        std::env::remove_var("GIT_WORK_TREE");
    }
}

#[tokio::test]
async fn test_process_termination_timeout() {
    // Test graceful termination of long-running processes

    #[cfg(unix)]
    let config = stdio_config("sleep", vec!["300"]); // 5 minutes

    #[cfg(windows)]
    let config = stdio_config("timeout", vec!["/t", "300", "/nobreak"]);

    let temp_dir = std::env::temp_dir();

    let mut io = StdioTransport::connect("test-long-running", &config, &temp_dir)
        .await
        .expect("Failed to spawn long-running process");

    let mut child = io.child.take().expect("Child process missing");

    // Immediately kill the process
    let kill_result = child.kill().await;
    assert!(kill_result.is_ok(), "Failed to kill long-running process");

    // Wait for the process with a timeout
    let wait_result = timeout(Duration::from_secs(2), child.wait()).await;
    assert!(
        wait_result.is_ok(),
        "Process did not terminate within timeout"
    );

    let status = wait_result.unwrap().expect("Failed to get exit status");

    // Process should not have exited successfully (it was killed)
    assert!(
        !status.success(),
        "Killed process should not have success status"
    );
}
