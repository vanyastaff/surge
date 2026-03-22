//! Cleanup utilities for orphaned worktrees and stale branches.
//!
//! Provides [`LifecycleManager`] which wraps a [`GitManager`] and offers
//! automated cleanup of orphaned worktrees and merged branches.

use git2::{BranchType, Repository};
use tracing::info;

use crate::worktree::{GitError, GitManager};

/// Report of cleanup operations performed.
#[derive(Debug, Clone, Default)]
pub struct CleanupReport {
    /// Worktrees that were removed (by spec id).
    pub removed_worktrees: Vec<String>,
    /// Branches that were removed.
    pub removed_branches: Vec<String>,
}

/// Manages lifecycle cleanup of Surge worktrees and branches.
pub struct LifecycleManager {
    git_manager: GitManager,
}

impl LifecycleManager {
    /// Create a new `LifecycleManager` wrapping the given [`GitManager`].
    pub fn new(git_manager: GitManager) -> Self {
        Self { git_manager }
    }

    /// Remove worktrees whose directories no longer exist on disk.
    ///
    /// For each orphaned worktree, calls [`GitManager::discard`] to prune
    /// the worktree metadata and delete the branch.
    pub fn cleanup_orphaned(&self) -> Result<CleanupReport, GitError> {
        let mut report = CleanupReport::default();
        let worktrees = self.git_manager.list_worktrees()?;

        for wt in &worktrees {
            if !wt.exists_on_disk {
                info!(spec_id = %wt.spec_id, "cleaning up orphaned worktree");
                self.git_manager.discard(&wt.spec_id)?;
                report.removed_worktrees.push(wt.spec_id.clone());
            }
        }

        Ok(report)
    }

    /// Remove `surge/*` branches that have been fully merged into HEAD.
    ///
    /// A branch is considered merged if it is an ancestor of the current HEAD commit.
    pub fn cleanup_merged_branches(&self) -> Result<CleanupReport, GitError> {
        let mut report = CleanupReport::default();
        let repo = Repository::open(self.git_manager.repo_path())?;

        let head = repo.head()?;
        let head_oid = head.peel_to_commit()?.id();

        let branches = repo.branches(Some(BranchType::Local))?;
        for branch_result in branches {
            let (branch, _) = branch_result?;
            let name = match branch.name()? {
                Some(n) => n.to_string(),
                None => continue,
            };

            if !name.starts_with("surge/") {
                continue;
            }

            let branch_oid = match branch.get().peel_to_commit() {
                Ok(c) => c.id(),
                Err(_) => continue,
            };

            // If HEAD is a descendant of (or equal to) the branch commit,
            // then the branch is fully merged.
            let is_merged = head_oid == branch_oid
                || repo.graph_descendant_of(head_oid, branch_oid)?;

            if is_merged {
                info!(branch = %name, "removing merged branch");
                // Need to re-find since we can't mutably borrow from the iterator
                let mut branch_to_delete = repo.find_branch(&name, BranchType::Local)?;
                branch_to_delete.delete()?;
                report.removed_branches.push(name);
            }
        }

        Ok(report)
    }

    /// Run full cleanup: orphaned worktrees first, then merged branches.
    pub fn full_cleanup(&self) -> Result<CleanupReport, GitError> {
        let mut report = self.cleanup_orphaned()?;
        let branch_report = self.cleanup_merged_branches()?;
        report.removed_branches = branch_report.removed_branches;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// Create a temporary git repo with an initial commit.
    fn init_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();

        Command::new("git")
            .args(["init"])
            .current_dir(&path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&path)
            .output()
            .unwrap();

        let file = path.join("README.md");
        fs::write(&file, "# Test repo\n").unwrap();

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
    fn test_cleanup_orphaned() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create a worktree then manually remove its directory
        let info = gm.create_worktree("orphan-spec").unwrap();
        assert!(info.path.exists());
        fs::remove_dir_all(&info.path).unwrap();

        let lm = LifecycleManager::new(GitManager::new(path).unwrap());
        let report = lm.cleanup_orphaned().unwrap();
        assert_eq!(report.removed_worktrees, vec!["orphan-spec"]);
    }

    #[test]
    fn test_cleanup_merged_branches() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create worktree, commit, merge, then discard worktree before cleanup
        let info = gm.create_worktree("merged-spec").unwrap();
        let new_file = info.path.join("merged.txt");
        fs::write(&new_file, "content\n").unwrap();
        gm.commit("merged-spec", "add file").unwrap();
        gm.merge("merged-spec", None).unwrap();

        // Discard the worktree (but not the branch — we'll recreate the branch
        // to simulate a leftover merged branch).
        // First remove worktree dir + prune metadata, keeping the branch.
        fs::remove_dir_all(&info.path).unwrap();
        {
            let repo = Repository::open(&path).unwrap();
            if let Ok(wt) = repo.find_worktree("merged-spec") {
                let mut prune_opts = git2::WorktreePruneOptions::new();
                prune_opts.valid(true).working_tree(true);
                let _ = wt.prune(Some(&mut prune_opts));
            }
        }

        // Now the surge/merged-spec branch should be an ancestor of HEAD
        let lm = LifecycleManager::new(GitManager::new(path).unwrap());
        let report = lm.cleanup_merged_branches().unwrap();
        assert!(report.removed_branches.contains(&"surge/merged-spec".to_string()));
    }

    #[test]
    fn test_full_cleanup_no_orphans() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // No worktrees at all — cleanup should succeed with empty report
        let lm = LifecycleManager::new(gm);
        let report = lm.full_cleanup().unwrap();
        assert!(report.removed_worktrees.is_empty());
        assert!(report.removed_branches.is_empty());
    }
}
