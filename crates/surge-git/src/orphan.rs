//! Orphan resource detection for startup scanning.
//!
//! Provides [`OrphanScanner`] which scans for orphaned worktrees and branches
//! that remain from previous crashed sessions or incomplete cleanup.

use git2::{BranchType, Repository};
use tracing::{debug, info};

use crate::worktree::{GitError, GitManager};

/// Report of orphaned resources detected during scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrphanReport {
    /// Worktrees whose directories no longer exist on disk.
    pub orphaned_worktrees: Vec<OrphanedWorktree>,
    /// Surge branches without corresponding worktrees.
    pub orphaned_branches: Vec<String>,
}

impl OrphanReport {
    /// Returns `true` if no orphans were found.
    pub fn is_empty(&self) -> bool {
        self.orphaned_worktrees.is_empty() && self.orphaned_branches.is_empty()
    }

    /// Returns total count of orphaned resources.
    pub fn total_count(&self) -> usize {
        self.orphaned_worktrees.len() + self.orphaned_branches.len()
    }
}

/// Information about an orphaned worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanedWorktree {
    /// The spec id of the orphaned worktree.
    pub spec_id: String,
    /// Expected path where the worktree directory should exist.
    pub expected_path: std::path::PathBuf,
    /// Reason why it's considered orphaned.
    pub reason: String,
}

/// Scans for orphaned resources from previous crashed or incomplete sessions.
pub struct OrphanScanner {
    git_manager: GitManager,
}

impl OrphanScanner {
    /// Create a new `OrphanScanner` wrapping the given [`GitManager`].
    pub fn new(git_manager: GitManager) -> Self {
        Self { git_manager }
    }

    /// Scan for all orphaned resources (worktrees and branches).
    ///
    /// This performs:
    /// 1. Detection of worktrees whose directories no longer exist on disk
    /// 2. Detection of `surge/*` branches that don't have corresponding worktrees
    pub fn scan(&self) -> Result<OrphanReport, GitError> {
        let mut report = OrphanReport::default();

        // Scan for orphaned worktrees
        let worktree_infos = self.git_manager.list_worktrees()?;

        for wt in &worktree_infos {
            if !wt.exists_on_disk {
                info!(spec_id = %wt.spec_id, path = ?wt.path, "detected orphaned worktree");
                report.orphaned_worktrees.push(OrphanedWorktree {
                    spec_id: wt.spec_id.clone(),
                    expected_path: wt.path.clone(),
                    reason: "directory missing".to_string(),
                });
            }
        }

        // Scan for orphaned branches (surge/* branches without worktrees)
        let repo = Repository::open(self.git_manager.repo_path())?;
        let branches = repo.branches(Some(BranchType::Local))?;

        for branch_result in branches {
            let (branch, _) = branch_result?;
            let name = match branch.name()? {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Only check surge/* branches
            if !name.starts_with("surge/") {
                continue;
            }

            // Extract spec_id from branch name (surge/spec-id -> spec-id)
            let spec_id = name.strip_prefix("surge/").unwrap_or(&name);

            // Check if this branch has a corresponding worktree
            let has_worktree = worktree_infos.iter().any(|wt| wt.spec_id == spec_id);

            if !has_worktree {
                debug!(branch = %name, "detected orphaned branch");
                report.orphaned_branches.push(name);
            }
        }

        if !report.is_empty() {
            info!(
                orphaned_worktrees = report.orphaned_worktrees.len(),
                orphaned_branches = report.orphaned_branches.len(),
                "orphan scan complete"
            );
        } else {
            debug!("orphan scan complete: no orphans found");
        }

        Ok(report)
    }

    /// Scan only for orphaned worktrees.
    pub fn scan_worktrees(&self) -> Result<Vec<OrphanedWorktree>, GitError> {
        let worktree_infos = self.git_manager.list_worktrees()?;
        let mut orphaned = Vec::new();

        for wt in &worktree_infos {
            if !wt.exists_on_disk {
                orphaned.push(OrphanedWorktree {
                    spec_id: wt.spec_id.clone(),
                    expected_path: wt.path.clone(),
                    reason: "directory missing".to_string(),
                });
            }
        }

        Ok(orphaned)
    }

    /// Scan only for orphaned branches.
    pub fn scan_branches(&self) -> Result<Vec<String>, GitError> {
        let worktree_infos = self.git_manager.list_worktrees()?;
        let mut orphaned = Vec::new();

        let repo = Repository::open(self.git_manager.repo_path())?;
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

            let spec_id = name.strip_prefix("surge/").unwrap_or(&name);
            let has_worktree = worktree_infos.iter().any(|wt| wt.spec_id == spec_id);

            if !has_worktree {
                orphaned.push(name);
            }
        }

        Ok(orphaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::init_test_repo;
    use git2::Repository;
    use std::fs;

    #[test]
    fn test_scan_no_orphans() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path).unwrap();

        let scanner = OrphanScanner::new(gm);
        let report = scanner.scan().unwrap();

        assert!(report.is_empty());
        assert_eq!(report.total_count(), 0);
    }

    #[test]
    fn test_scan_orphaned_worktree() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create a worktree then remove its directory
        let info = gm.create_worktree("orphan-spec", None).unwrap();
        assert!(info.path.exists());
        fs::remove_dir_all(&info.path).unwrap();

        let scanner = OrphanScanner::new(GitManager::new(path).unwrap());
        let report = scanner.scan().unwrap();

        assert!(!report.is_empty());
        assert_eq!(report.orphaned_worktrees.len(), 1);
        assert_eq!(report.orphaned_worktrees[0].spec_id, "orphan-spec");
        assert_eq!(report.orphaned_worktrees[0].reason, "directory missing");
        // Branch still exists, so not orphaned yet (it has a worktree entry)
        assert_eq!(report.orphaned_branches.len(), 0);
    }

    #[test]
    fn test_scan_orphaned_branch() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create a worktree, then fully discard it, but manually recreate the branch
        let _info = gm.create_worktree("test-spec", None).unwrap();
        gm.discard("test-spec").unwrap();

        // Now manually create a surge/* branch without a worktree
        let repo = Repository::open(&path).unwrap();
        let head = repo.head().unwrap();
        let commit = head.peel_to_commit().unwrap();
        repo.branch("surge/orphan-branch", &commit, false).unwrap();

        let scanner = OrphanScanner::new(GitManager::new(path).unwrap());
        let report = scanner.scan().unwrap();

        assert!(!report.is_empty());
        assert_eq!(report.orphaned_worktrees.len(), 0);
        assert_eq!(report.orphaned_branches.len(), 1);
        assert_eq!(report.orphaned_branches[0], "surge/orphan-branch");
    }

    #[test]
    fn test_scan_both_types() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create orphaned worktree (directory removed)
        let info1 = gm.create_worktree("orphan-wt", None).unwrap();
        fs::remove_dir_all(&info1.path).unwrap();

        // Create orphaned branch (no worktree)
        let repo = Repository::open(&path).unwrap();
        let head = repo.head().unwrap();
        let commit = head.peel_to_commit().unwrap();
        repo.branch("surge/orphan-br", &commit, false).unwrap();

        let scanner = OrphanScanner::new(GitManager::new(path).unwrap());
        let report = scanner.scan().unwrap();

        assert_eq!(report.total_count(), 2);
        assert_eq!(report.orphaned_worktrees.len(), 1);
        assert_eq!(report.orphaned_branches.len(), 1);
        assert_eq!(report.orphaned_worktrees[0].spec_id, "orphan-wt");
        assert_eq!(report.orphaned_branches[0], "surge/orphan-br");
    }

    #[test]
    fn test_scan_worktrees_only() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        let info = gm.create_worktree("orphan-spec", None).unwrap();
        fs::remove_dir_all(&info.path).unwrap();

        let scanner = OrphanScanner::new(GitManager::new(path).unwrap());
        let orphaned = scanner.scan_worktrees().unwrap();

        assert_eq!(orphaned.len(), 1);
        assert_eq!(orphaned[0].spec_id, "orphan-spec");
    }

    #[test]
    fn test_scan_branches_only() {
        let (_dir, path) = init_test_repo();
        let repo = Repository::open(&path).unwrap();
        let head = repo.head().unwrap();
        let commit = head.peel_to_commit().unwrap();
        repo.branch("surge/orphan-branch", &commit, false).unwrap();

        let gm = GitManager::new(path).unwrap();
        let scanner = OrphanScanner::new(gm);
        let orphaned = scanner.scan_branches().unwrap();

        assert_eq!(orphaned.len(), 1);
        assert_eq!(orphaned[0], "surge/orphan-branch");
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "macOS /var/folders symlink confuses libgit2 worktree gitdir resolution; tracked separately"
    )]
    fn test_active_worktree_not_orphaned() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create an active worktree with directory intact
        let _info = gm.create_worktree("active-spec", None).unwrap();

        let scanner = OrphanScanner::new(GitManager::new(path).unwrap());
        let report = scanner.scan().unwrap();

        assert!(report.is_empty());
        assert_eq!(report.orphaned_worktrees.len(), 0);
        assert_eq!(report.orphaned_branches.len(), 0);
    }

    #[test]
    fn test_non_surge_branch_ignored() {
        let (_dir, path) = init_test_repo();
        let repo = Repository::open(&path).unwrap();
        let head = repo.head().unwrap();
        let commit = head.peel_to_commit().unwrap();

        // Create a non-surge branch
        repo.branch("feature/my-feature", &commit, false).unwrap();

        let gm = GitManager::new(path).unwrap();
        let scanner = OrphanScanner::new(gm);
        let report = scanner.scan().unwrap();

        // Non-surge branches should be ignored
        assert!(report.is_empty());
    }
}
