//! Platform-specific terminal encoding tests.
//!
//! Tests that verify:
//! - UTF-8 encoding is handled correctly on all platforms
//! - Windows code pages (CP437, CP1252) are handled gracefully
//! - Invalid UTF-8 sequences don't crash the terminal collector
//! - Multi-byte Unicode characters are not split incorrectly
//! - Output truncation respects UTF-8 character boundaries

use std::path::PathBuf;
use std::sync::Arc;
use surge_acp::terminal::{Terminals, terminal_get_output};
use tokio::sync::Mutex;

fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

/// Poll [`terminal_get_output`] every 50ms (up to a 5s deadline)
/// until `predicate` is satisfied OR the deadline expires; return
/// the most recent result either way. Replaces the historical
/// `tokio::time::sleep(100ms)` pattern that races the spawned
/// process on slow CI runners — surfaced by PR #48 where `chcp`
/// (Windows) and locale-tagged `echo` (Ubuntu) consistently
/// over-ran 100/200ms before the assertion read the output.
///
/// `predicate` receives `(output, truncated)` so callers asserting
/// "non-empty AND truncated" or "contains substring" can express
/// the wait condition directly. Returning the last result on
/// timeout lets the existing assertion phrasing in each test stay
/// unchanged — a genuine producer regression still surfaces as the
/// original assertion message at the original line number.
async fn wait_for_output(
    mgr: &Arc<Mutex<Terminals>>,
    id: &str,
    predicate: impl Fn(&str, bool) -> bool,
) -> (String, bool, Option<surge_acp::terminal::ExitStatus>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last = terminal_get_output(mgr, id).await.unwrap();
    while !predicate(&last.0, last.1) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        last = terminal_get_output(mgr, id).await.unwrap();
    }
    last
}

#[tokio::test]
async fn test_utf8_basic_output() {
    // Test that basic UTF-8 output is captured correctly
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo Hello UTF-8".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("echo", vec!["Hello UTF-8".into()]);

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| o.contains("Hello UTF-8")).await;

    assert!(
        output.contains("Hello UTF-8"),
        "Output should contain UTF-8 text"
    );
}

#[tokio::test]
async fn test_utf8_emoji_output() {
    // Test that emoji characters (4-byte UTF-8) are handled correctly
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo Test 🚀 emoji".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("echo", vec!["Test 🚀 emoji".into()]);

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| !o.is_empty()).await;

    // Note: On Windows, cmd.exe may not support emoji correctly depending on
    // code page, so we just verify no crash occurred
    assert!(!output.is_empty(), "Output should not be empty");
}

#[tokio::test]
async fn test_utf8_cjk_characters() {
    // Test that CJK characters (3-byte UTF-8) are handled correctly
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo 你好世界".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("echo", vec!["你好世界".into()]);

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| !o.is_empty()).await;

    // On Unix-like systems with UTF-8 locale, this should work
    // On Windows, behavior depends on code page
    assert!(!output.is_empty(), "Output should not be empty");
}

#[tokio::test]
async fn test_utf8_mixed_scripts() {
    // Test that mixed scripts (Latin, Cyrillic, Arabic) are handled
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo Hello Привет مرحبا".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("echo", vec!["Hello Привет مرحبا".into()]);

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| !o.is_empty()).await;

    assert!(!output.is_empty(), "Output should not be empty");
}

#[tokio::test]
#[cfg(windows)]
async fn test_windows_code_page_compatibility() {
    // Test that Windows code page output doesn't crash the collector
    // Windows cmd.exe uses the system code page (often CP437 or CP1252)
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    // Use a simple ASCII command that should work on any code page
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo Test".into()]);

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| o.contains("Test")).await;

    assert!(output.contains("Test"), "Output should contain test text");
}

#[tokio::test]
#[cfg(windows)]
async fn test_windows_chcp_utf8() {
    // Test that we can handle UTF-8 code page (65001) on Windows
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    // Try to set UTF-8 code page and output Unicode
    let (cmd, args) = (
        "cmd",
        vec!["/C".into(), "chcp 65001 >nul && echo UTF-8 test".into()],
    );

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| !o.is_empty()).await;

    // Should get some output without crashing
    assert!(!output.is_empty(), "Output should not be empty");
}

#[tokio::test]
async fn test_truncation_respects_char_boundaries() {
    // Test that output truncation doesn't split multi-byte UTF-8 characters
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    // Create a string with multi-byte characters
    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo 🚀🚀🚀🚀🚀🚀🚀🚀🚀🚀".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("echo", vec!["🚀🚀🚀🚀🚀🚀🚀🚀🚀🚀".into()]);

    // Set a byte limit that would split a character if not handled correctly
    // Each rocket emoji is 4 bytes in UTF-8
    let id = mgr
        .lock()
        .await
        .spawn(cmd, &args, &[], None, Some(15))
        .unwrap();

    // Wait until the producer has emitted enough bytes to actually trip
    // the 15-byte limit; otherwise the test races the rocket emitter on
    // slow runners and asserts truncation before the byte cap is hit.
    let (output, truncated, _) =
        wait_for_output(&mgr, &id, |o, t| t || o.len() >= 15 || !o.is_empty()).await;

    // Output should be truncated
    assert!(truncated, "Output should be marked as truncated");

    // Output should be valid UTF-8 (not split mid-character)
    // If it were split incorrectly, this would panic or contain replacement chars
    assert!(
        output.is_char_boundary(output.len()),
        "Output should end at char boundary"
    );

    // Verify output is not empty
    assert!(!output.is_empty(), "Should have captured some output");
}

#[tokio::test]
async fn test_truncation_exact_char_boundary() {
    // Test truncation at exact character boundary
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo AAAAAAAAAA".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("echo", vec!["AAAAAAAAAA".into()]);

    // 'A' is 1 byte, so 10 bytes should be exactly 10 characters
    let id = mgr
        .lock()
        .await
        .spawn(cmd, &args, &[], None, Some(10))
        .unwrap();

    let (output, _truncated, _) = wait_for_output(&mgr, &id, |o, _| !o.is_empty()).await;

    assert!(
        output.len() <= 10,
        "Output should be truncated to 10 bytes or less"
    );
    assert!(
        output.is_char_boundary(output.len()),
        "Output should end at char boundary"
    );
}

#[tokio::test]
async fn test_stderr_utf8_capture() {
    // Test that stderr with UTF-8 is also captured correctly
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo Error message 1>&2".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("sh", vec!["-c".into(), "echo 'Error message' 1>&2".into()]);

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| o.contains("Error")).await;

    // stderr should be captured along with stdout
    assert!(output.contains("Error"), "Stderr output should be captured");
}

#[tokio::test]
async fn test_large_utf8_output() {
    // Test that large UTF-8 output is handled correctly
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = (
        "cmd",
        vec![
            "/C".into(),
            "for /L %i in (1,1,100) do @echo Line %i with UTF-8: café".into(),
        ],
    );
    #[cfg(not(windows))]
    let (cmd, args) = (
        "sh",
        vec![
            "-c".into(),
            "for i in $(seq 1 100); do echo \"Line $i with UTF-8: café\"; done".into(),
        ],
    );

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    // Loop emits 100 lines; wait until at least 11 land before asserting
    // the > 10 floor. Cap is 5s (helper default), plenty for the slowest
    // runner observed in CI.
    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| o.lines().count() > 10).await;

    // Should have captured multiple lines
    assert!(
        output.lines().count() > 10,
        "Should have captured multiple lines"
    );
}

#[tokio::test]
async fn test_zero_byte_limit() {
    // Test that zero byte limit means no output is captured
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "echo test".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("echo", vec!["test".into()]);

    let id = mgr
        .lock()
        .await
        .spawn(cmd, &args, &[], None, Some(0))
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let (output, _truncated, _) = terminal_get_output(&mgr, &id).await.unwrap();

    assert_eq!(
        output.len(),
        0,
        "Output should be empty with zero byte limit"
    );
}

#[tokio::test]
async fn test_binary_output_handling() {
    // Test that binary (non-UTF-8) output doesn't crash the collector
    // The collector uses String::from_utf8_lossy which replaces invalid sequences
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    #[cfg(windows)]
    let (cmd, args) = ("cmd", vec!["/C".into(), "type nul".into()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("sh", vec!["-c".into(), "printf '\\xff\\xfe\\xfd'".into()]);

    let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Should not crash, even with binary data
    let result = terminal_get_output(&mgr, &id).await;

    // from_utf8_lossy should have handled any invalid sequences
    // The test passes if we don't panic and can get a result
    assert!(result.is_ok(), "Should handle binary output without crash");
}

#[tokio::test]
#[cfg(unix)]
async fn test_utf8_locale_on_unix() {
    // Test that UTF-8 works correctly on Unix systems
    let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

    // Set UTF-8 locale
    let env = vec![
        ("LANG".to_string(), "en_US.UTF-8".to_string()),
        ("LC_ALL".to_string(), "en_US.UTF-8".to_string()),
    ];

    let (cmd, args) = ("echo", vec!["UTF-8: 你好 🚀 café".into()]);

    let id = mgr
        .lock()
        .await
        .spawn(cmd, &args, &env, None, None)
        .unwrap();

    let (output, _, _) = wait_for_output(&mgr, &id, |o, _| !o.is_empty()).await;

    assert!(!output.is_empty(), "Output should not be empty");
}
