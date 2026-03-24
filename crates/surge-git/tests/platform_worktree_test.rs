//! Platform-specific integration tests for git worktree path handling.
//!
//! Verifies that worktree operations work correctly across Windows, macOS,
//! and Linux, with special attention to:
//! - Path separator normalization (forward/back slashes)
//! - Special characters in spec IDs and paths
//! - Path comparison and existence checks
//! - Git operations with platform-specific paths

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use surge_git::{GitError, GitManager};

/// Create a temporary git repo with a single initial commit.
/// Returns `(TempDir, repo_path)` — keep `TempDir` alive for the test duration.
fn init_test_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();

    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(&path)
            .output()
            .unwrap();
    }

    fs::write(path.join("README.md"), "# Test repo\n").unwrap();

    Command::new("git")
        .args(["add", "."])
        .current_dir(&path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(&path)
        .output()
        .unwrap();

    (dir, path)
}

#[test]
fn test_worktree_path_uses_platform_separators() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let spec_id = "test-spec-123";

    let wt_path = gm.worktree_path(spec_id);

    // Path should end with .surge/worktrees/test-spec-123
    assert!(wt_path.ends_with(".surge/worktrees/test-spec-123")
        || wt_path.ends_with(r".surge\worktrees\test-spec-123"));

    // Path should be a valid PathBuf that can be used in operations
    assert!(wt_path.parent().is_some());
}

#[test]
fn test_worktree_creation_with_complex_paths() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();

    // Test spec ID with dashes and numbers (common pattern)
    let spec_id = "feature-123-my-spec";
    let info = gm.create_worktree(spec_id, None).unwrap();

    assert_eq!(info.spec_id, spec_id);
    assert!(info.exists_on_disk);
    assert!(info.path.exists());

    // Verify the worktree directory structure
    assert!(info.path.is_dir());
    assert!(info.path.join("README.md").exists());

    gm.discard(spec_id).unwrap();
}

#[test]
fn test_worktree_path_exists_check() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let spec_id = "exists-check-spec";

    // Before creation, path should not exist
    let wt_path = gm.worktree_path(spec_id);
    assert!(!wt_path.exists());

    // After creation, path should exist
    let info = gm.create_worktree(spec_id, None).unwrap();
    assert!(info.path.exists());
    assert_eq!(info.path, wt_path);

    // After discard, path should not exist
    gm.discard(spec_id).unwrap();
    assert!(!wt_path.exists());
}

#[test]
fn test_list_worktrees_returns_valid_paths() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();

    gm.create_worktree("spec-a", None).unwrap();
    gm.create_worktree("spec-b", None).unwrap();

    let list = gm.list_worktrees().unwrap();
    assert_eq!(list.len(), 2);

    for info in &list {
        // All returned paths should exist on disk
        assert!(info.exists_on_disk);
        assert!(info.path.exists());
        assert!(info.path.is_absolute());

        // Path should be properly formed
        assert!(info.path.to_str().is_some());
    }

    // Cleanup
    gm.discard("spec-a").unwrap();
    gm.discard("spec-b").unwrap();
}

#[test]
fn test_worktree_operations_with_file_paths() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let spec_id = "file-ops-spec";

    let info = gm.create_worktree(spec_id, None).unwrap();

    // Create a file with a platform-agnostic path join
    let test_file = info.path.join("test_file.txt");
    fs::write(&test_file, "test content\n").unwrap();
    assert!(test_file.exists());

    // Create a nested directory structure
    let nested_dir = info.path.join("src").join("module");
    fs::create_dir_all(&nested_dir).unwrap();
    let nested_file = nested_dir.join("code.rs");
    fs::write(&nested_file, "// code\n").unwrap();
    assert!(nested_file.exists());

    // Verify has_changes detects the new files
    assert!(gm.has_changes(spec_id).unwrap());

    // Commit and verify
    let oid = gm.commit(spec_id, "add test files").unwrap();
    assert!(!oid.is_zero());

    gm.discard(spec_id).unwrap();
}

#[test]
fn test_worktree_path_canonicalization() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let spec_id = "canonical-spec";

    let info = gm.create_worktree(spec_id, None).unwrap();

    // The path should be usable for file operations
    let canonical = info.path.canonicalize().unwrap();
    assert!(canonical.exists());
    assert!(canonical.is_absolute());

    // Should be able to compare paths
    let wt_path = gm.worktree_path(spec_id);
    let canonical_wt = wt_path.canonicalize().unwrap();
    assert_eq!(canonical, canonical_wt);

    gm.discard(spec_id).unwrap();
}

#[test]
fn test_repo_path_is_absolute() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();

    let repo_path = gm.repo_path();
    assert!(repo_path.is_absolute());
    assert!(repo_path.exists());
}

#[test]
fn test_worktree_not_found_error() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();

    // Attempting operations on non-existent worktree should error
    let result = gm.has_changes("nonexistent-spec");
    assert!(matches!(result, Err(GitError::WorktreeNotFound(_))));
}

#[cfg(target_os = "windows")]
#[test]
fn test_windows_path_handling() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let spec_id = "windows-spec";

    let info = gm.create_worktree(spec_id, None).unwrap();

    // On Windows, paths should handle backslashes
    let path_str = info.path.to_str().unwrap();
    // Path should be valid and contain drive letter if absolute
    assert!(path_str.len() > 0);

    // Should be able to create files with standard path operations
    let test_file = info.path.join("windows_test.txt");
    fs::write(&test_file, "windows content\n").unwrap();
    assert!(test_file.exists());

    gm.discard(spec_id).unwrap();
}

#[cfg(target_family = "unix")]
#[test]
fn test_unix_path_handling() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();
    let spec_id = "unix-spec";

    let info = gm.create_worktree(spec_id, None).unwrap();

    // On Unix, paths should use forward slashes
    let path_str = info.path.to_str().unwrap();
    assert!(path_str.contains('/'));

    // Should be able to create files with standard path operations
    let test_file = info.path.join("unix_test.txt");
    fs::write(&test_file, "unix content\n").unwrap();
    assert!(test_file.exists());

    gm.discard(spec_id).unwrap();
}

#[test]
fn test_worktree_with_underscores_and_numbers() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();

    let spec_ids = [
        "spec_with_underscores",
        "spec-123-456",
        "spec_123_test",
        "my-feature-2024",
    ];

    for spec_id in spec_ids {
        let info = gm.create_worktree(spec_id, None).unwrap();
        assert_eq!(info.spec_id, spec_id);
        assert!(info.path.exists());
        gm.discard(spec_id).unwrap();
    }
}

#[test]
fn test_worktree_parent_directory_creation() {
    let (_dir, path) = init_test_repo();
    let gm = GitManager::new(path.clone()).unwrap();

    // Remove .surge directory if it exists
    let surge_dir = path.join(".surge");
    if surge_dir.exists() {
        fs::remove_dir_all(&surge_dir).unwrap();
    }

    // Creating a worktree should create parent directories
    let spec_id = "parent-test";
    let info = gm.create_worktree(spec_id, None).unwrap();

    assert!(surge_dir.exists());
    assert!(surge_dir.join("worktrees").exists());
    assert!(info.path.exists());

    gm.discard(spec_id).unwrap();
}
