//! Cleanup utilities for orphaned worktrees and stale branches.
//!
//! Provides [`LifecycleManager`] which wraps a [`GitManager`] and offers
//! automated cleanup of orphaned worktrees and merged branches.

use git2::{BranchType, Repository};
use tracing::info;

use crate::audit::CleanupAudit;
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
    audit: Option<CleanupAudit>,
}

impl LifecycleManager {
    /// Create a new `LifecycleManager` wrapping the given [`GitManager`].
    pub fn new(git_manager: GitManager) -> Self {
        Self {
            git_manager,
            audit: None,
        }
    }

    /// Create a new `LifecycleManager` with audit logging enabled.
    pub fn with_audit(git_manager: GitManager, audit: CleanupAudit) -> Self {
        Self {
            git_manager,
            audit: Some(audit),
        }
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

                // Log orphan detection
                if let Some(audit) = &self.audit {
                    let _ = audit
                        .log_orphan_detected(&wt.spec_id, Some("directory missing".to_string()));
                }

                self.git_manager.discard(&wt.spec_id)?;
                report.removed_worktrees.push(wt.spec_id.clone());

                // Log worktree removal
                if let Some(audit) = &self.audit {
                    let _ = audit
                        .log_worktree_removed(&wt.spec_id, Some("orphaned cleanup".to_string()));
                }
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
            let is_merged =
                head_oid == branch_oid || repo.graph_descendant_of(head_oid, branch_oid)?;

            if is_merged {
                info!(branch = %name, "removing merged branch");

                // Log merged branch detection
                if let Some(audit) = &self.audit {
                    let _ = audit.log_merged_branch_detected(
                        &name,
                        Some("fully merged into HEAD".to_string()),
                    );
                }

                // Need to re-find since we can't mutably borrow from the iterator
                let mut branch_to_delete = repo.find_branch(&name, BranchType::Local)?;
                branch_to_delete.delete()?;
                report.removed_branches.push(name.clone());

                // Log branch deletion
                if let Some(audit) = &self.audit {
                    let _ = audit.log_branch_deleted(&name, Some("merged cleanup".to_string()));
                }
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
    use crate::audit::{CleanupAudit, CleanupEventType};
    use crate::test_helpers::init_test_repo;
    use std::fs;

    #[test]
    fn test_cleanup_orphaned() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create a worktree then manually remove its directory
        let info = gm.create_worktree("orphan-spec", None).unwrap();
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
        let info = gm.create_worktree("merged-spec", None).unwrap();
        let new_file = info.path.join("merged.txt");
        fs::write(&new_file, "content\n").unwrap();
        gm.commit("merged-spec", "add file").unwrap();
        gm.merge("merged-spec", None, true).unwrap();

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
        assert!(
            report
                .removed_branches
                .contains(&"surge/merged-spec".to_string())
        );
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

    #[test]
    fn test_cleanup_orphaned_with_audit() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create a worktree then manually remove its directory
        let info = gm.create_worktree("orphan-spec-audit", None).unwrap();
        assert!(info.path.exists());
        fs::remove_dir_all(&info.path).unwrap();

        // Create audit logger
        let log_path = path.join(".surge").join("cleanup.log");
        let audit = CleanupAudit::new(&log_path).unwrap();

        let lm = LifecycleManager::with_audit(GitManager::new(path).unwrap(), audit);
        let report = lm.cleanup_orphaned().unwrap();
        assert_eq!(report.removed_worktrees, vec!["orphan-spec-audit"]);

        // Verify audit log entries
        let audit_verify = CleanupAudit::new(&log_path).unwrap();
        let events = audit_verify.read_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, CleanupEventType::OrphanDetected);
        assert_eq!(events[0].resource_id, "orphan-spec-audit");
        assert_eq!(events[1].event_type, CleanupEventType::WorktreeRemoved);
        assert_eq!(events[1].resource_id, "orphan-spec-audit");
    }

    #[test]
    fn test_cleanup_merged_branches_with_audit() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create worktree, commit, merge, then discard worktree before cleanup
        let info = gm.create_worktree("merged-spec-audit", None).unwrap();
        let new_file = info.path.join("merged.txt");
        fs::write(&new_file, "content\n").unwrap();
        gm.commit("merged-spec-audit", "add file").unwrap();
        gm.merge("merged-spec-audit", None, true).unwrap();

        // Discard the worktree but keep the branch
        fs::remove_dir_all(&info.path).unwrap();
        {
            let repo = Repository::open(&path).unwrap();
            if let Ok(wt) = repo.find_worktree("merged-spec-audit") {
                let mut prune_opts = git2::WorktreePruneOptions::new();
                prune_opts.valid(true).working_tree(true);
                let _ = wt.prune(Some(&mut prune_opts));
            }
        }

        // Create audit logger
        let log_path = path.join(".surge").join("cleanup.log");
        let audit = CleanupAudit::new(&log_path).unwrap();

        let lm = LifecycleManager::with_audit(GitManager::new(path).unwrap(), audit);
        let report = lm.cleanup_merged_branches().unwrap();
        assert!(
            report
                .removed_branches
                .contains(&"surge/merged-spec-audit".to_string())
        );

        // Verify audit log entries
        let audit_verify = CleanupAudit::new(&log_path).unwrap();
        let events = audit_verify.read_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, CleanupEventType::MergedBranchDetected);
        assert_eq!(events[0].resource_id, "surge/merged-spec-audit");
        assert_eq!(events[1].event_type, CleanupEventType::BranchDeleted);
        assert_eq!(events[1].resource_id, "surge/merged-spec-audit");
    }

    #[test]
    fn test_full_cleanup_with_audit() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create an orphaned worktree
        let info = gm.create_worktree("orphan-full", None).unwrap();
        fs::remove_dir_all(&info.path).unwrap();

        // Create a merged branch
        let info2 = gm.create_worktree("merged-full", None).unwrap();
        let new_file = info2.path.join("merged.txt");
        fs::write(&new_file, "content\n").unwrap();
        gm.commit("merged-full", "add file").unwrap();
        gm.merge("merged-full", None, true).unwrap();
        fs::remove_dir_all(&info2.path).unwrap();
        {
            let repo = Repository::open(&path).unwrap();
            if let Ok(wt) = repo.find_worktree("merged-full") {
                let mut prune_opts = git2::WorktreePruneOptions::new();
                prune_opts.valid(true).working_tree(true);
                let _ = wt.prune(Some(&mut prune_opts));
            }
        }

        // Create audit logger
        let log_path = path.join(".surge").join("cleanup.log");
        let audit = CleanupAudit::new(&log_path).unwrap();

        let lm = LifecycleManager::with_audit(GitManager::new(path).unwrap(), audit);
        let report = lm.full_cleanup().unwrap();
        assert_eq!(report.removed_worktrees.len(), 1);
        assert!(report.removed_branches.len() >= 1);

        // Verify audit log has entries for both operations
        let audit_verify = CleanupAudit::new(&log_path).unwrap();
        let events = audit_verify.read_events().unwrap();
        assert!(events.len() >= 4); // At least 2 for orphan + 2 for merged branch

        // Check we have the expected event types
        let event_types: Vec<_> = events.iter().map(|e| &e.event_type).collect();
        assert!(event_types.contains(&&CleanupEventType::OrphanDetected));
        assert!(event_types.contains(&&CleanupEventType::WorktreeRemoved));
        assert!(event_types.contains(&&CleanupEventType::MergedBranchDetected));
        assert!(event_types.contains(&&CleanupEventType::BranchDeleted));
    }
}
