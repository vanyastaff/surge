//! Git worktree management for Surge — isolated workspaces per task.

// Pre-existing legacy code; M5 does not modify this crate.
// These allows suppress pedantic lints that fire when clippy::pedantic is
// requested transitively by surge-orchestrator.
#![allow(clippy::doc_markdown)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::redundant_else)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::single_match_else)]
#![allow(clippy::if_not_else)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::needless_raw_string_hashes)]
#![allow(clippy::bool_to_int_with_if)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::excessive_nesting)]
#![allow(clippy::explicit_iter_loop)]
#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::unused_self)]

pub mod audit;
pub mod cleanup;
pub mod orphan;
pub mod run_worktree;
pub mod worktree;

pub use audit::{CleanupAudit, CleanupEvent, CleanupEventType};
pub use cleanup::LifecycleManager;
pub use orphan::{OrphanReport, OrphanScanner, OrphanedWorktree};
// Note: `run_worktree::OrphanedWorktree` is intentionally NOT re-exported at
// the crate root because `orphan::OrphanedWorktree` (legacy) already lives
// there. Access via `surge_git::run_worktree::OrphanedWorktree`.
pub use run_worktree::{RunWorktreeInfo, WorktreeLocation, resolve_path, run_branch_name};
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
}
