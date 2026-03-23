//! Git worktree management for Surge — isolated workspaces per task.

pub mod cleanup;
pub mod worktree;

pub use cleanup::LifecycleManager;
pub use worktree::{GitError, GitManager, WorktreeInfo};

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    /// Create a temporary git repo with a single initial commit.
    /// Returns `(TempDir, repo_path)` — keep `TempDir` alive for the test duration.
    pub fn init_test_repo() -> (tempfile::TempDir, PathBuf) {
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

        Command::new("git").args(["add", "."]).current_dir(&path).output().unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(&path)
            .output()
            .unwrap();

        (dir, path)
    }
}
