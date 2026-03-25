//! Git worktree lifecycle management.
//!
//! Provides [`GitManager`] for creating, listing, committing, diffing,
//! discarding, and merging git worktrees used by Surge tasks.

use std::fs;
use std::path::{Path, PathBuf};

use git2::{
    BranchType, DiffOptions, Index, MergeOptions, Repository, Signature, WorktreeAddOptions,
    WorktreePruneOptions,
};
use tracing::{debug, info, warn};

/// Information about a Surge-managed worktree.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// The spec id this worktree belongs to.
    pub spec_id: String,
    /// Filesystem path of the worktree.
    pub path: PathBuf,
    /// Branch name (e.g. `surge/my-spec`).
    pub branch: String,
    /// Whether the worktree directory still exists on disk.
    pub exists_on_disk: bool,
}

/// Error type for git operations.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git2 error: {0}")]
    Git2(#[from] git2::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("worktree already exists for spec: {0}")]
    WorktreeAlreadyExists(String),

    #[error("worktree not found for spec: {0}")]
    WorktreeNotFound(String),

    #[error("branch not found: {0}")]
    BranchNotFound(String),

    /// Merge produced conflicts. `conflicting_files` lists affected paths.
    #[error("merge conflict in {} file(s)", conflicting_files.len())]
    MergeConflict { conflicting_files: Vec<PathBuf> },

    #[error("repository has no commits")]
    EmptyRepository,

    #[error("nothing to commit: no changes staged in worktree for spec '{0}'")]
    NothingToCommit(String),

    #[error("merge source and target are the same branch: {0}")]
    SameBranch(String),
}

impl From<GitError> for surge_core::SurgeError {
    fn from(e: GitError) -> Self {
        match e {
            GitError::WorktreeNotFound(s) => {
                surge_core::SurgeError::NotFound(format!("worktree: {s}"))
            }
            GitError::BranchNotFound(s) => surge_core::SurgeError::NotFound(format!("branch: {s}")),
            GitError::Io(e) => surge_core::SurgeError::Io(e),
            GitError::Git2(e) => surge_core::SurgeError::git_source(e.message().to_string(), e),
            GitError::WorktreeAlreadyExists(s) => {
                surge_core::SurgeError::git(format!("worktree already exists: {s}"))
            }
            GitError::EmptyRepository => {
                surge_core::SurgeError::git("repository has no commits".to_string())
            }
            GitError::MergeConflict { conflicting_files } => {
                surge_core::SurgeError::git(format!(
                    "merge conflict in {} file(s)",
                    conflicting_files.len()
                ))
            }
            GitError::NothingToCommit(s) => {
                surge_core::SurgeError::git(format!("nothing to commit for spec '{s}'"))
            }
            GitError::SameBranch(s) => {
                surge_core::SurgeError::git(format!("source and target are the same branch: {s}"))
            }
        }
    }
}

/// Manages git worktrees for Surge tasks.
pub struct GitManager {
    repo_path: PathBuf,
}

impl GitManager {
    /// Open and verify a git repository at the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the path is not a valid git repository.
    #[must_use = "returns a new GitManager"]
    pub fn new(repo_path: PathBuf) -> Result<Self, GitError> {
        let _ = Repository::open(&repo_path)?;
        Ok(Self { repo_path })
    }

    /// Discover the git repository from the current working directory.
    pub fn discover() -> Result<Self, GitError> {
        let repo = Repository::discover(".")?;
        let repo_path = repo.workdir().unwrap_or_else(|| repo.path()).to_path_buf();
        Ok(Self { repo_path })
    }

    /// Returns the repository root path.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Returns the worktree directory for a given spec id.
    pub fn worktree_path(&self, spec_id: &str) -> PathBuf {
        self.repo_path
            .join(".surge")
            .join("worktrees")
            .join(spec_id)
    }

    fn open_repo(&self) -> Result<Repository, GitError> {
        Ok(Repository::open(&self.repo_path)?)
    }

    /// Get a commit signature from git config, falling back to a hardcoded
    /// default when the user has no `user.name` / `user.email` configured.
    fn signature(repo: &Repository) -> Signature<'_> {
        repo.signature().unwrap_or_else(|_| {
            // Signature::now only fails if the name/email contain interior NUL
            // bytes, which cannot happen with these literals.
            Signature::now("Surge", "surge@localhost")
                .expect("hardcoded signature literals are valid")
        })
    }

    fn branch_name(spec_id: &str) -> String {
        format!("surge/{spec_id}")
    }

    // ── Preflight checks ─────────────────────────────────────────────────

    /// Returns `true` if the **main repository** has staged or unstaged changes.
    ///
    /// Use this as a preflight check before starting a new spec execution.
    /// Untracked files are not considered dirty.
    pub fn has_uncommitted_changes(&self) -> Result<bool, GitError> {
        let repo = self.open_repo()?;
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(false).include_ignored(false);
        let statuses = repo.statuses(Some(&mut opts))?;
        Ok(!statuses.is_empty())
    }

    /// Returns paths of files with uncommitted changes in the main repository.
    pub fn uncommitted_files(&self) -> Result<Vec<PathBuf>, GitError> {
        let repo = self.open_repo()?;
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(false).include_ignored(false);
        let statuses = repo.statuses(Some(&mut opts))?;
        Ok(statuses
            .iter()
            .filter_map(|s| s.path().map(PathBuf::from))
            .collect())
    }

    // ── Worktree lifecycle ────────────────────────────────────────────────

    /// Create a new worktree and branch for the given spec.
    ///
    /// The worktree branches from `base_branch` if given, otherwise from the
    /// current HEAD. Creates branch `surge/{spec_id}` and places the worktree
    /// at `.surge/worktrees/{spec_id}`.
    ///
    /// # Errors
    ///
    /// Returns [`GitError::WorktreeAlreadyExists`] if a worktree already exists.
    pub fn create_worktree(
        &self,
        spec_id: &str,
        base_branch: Option<&str>,
    ) -> Result<WorktreeInfo, GitError> {
        let repo = self.open_repo()?;
        let branch_name = Self::branch_name(spec_id);
        let wt_path = self.worktree_path(spec_id);

        // Check for duplicate
        let worktrees = repo.worktrees()?;
        for name in worktrees.iter().flatten() {
            if name == spec_id {
                return Err(GitError::WorktreeAlreadyExists(spec_id.to_string()));
            }
        }

        // Resolve base commit
        let commit = if let Some(base) = base_branch {
            let b = repo
                .find_branch(base, BranchType::Local)
                .map_err(|_| GitError::BranchNotFound(base.to_string()))?;
            b.get().peel_to_commit()?
        } else {
            let head = repo.head().map_err(|_| GitError::EmptyRepository)?;
            head.peel_to_commit()
                .map_err(|_| GitError::EmptyRepository)?
        };

        let branch = repo.branch(&branch_name, &commit, false)?;

        if let Some(parent) = wt_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let reference = branch.into_reference();
        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(&reference));
        let _wt = repo.worktree(spec_id, &wt_path, Some(&opts))?;

        info!(spec_id, ?wt_path, "created worktree");

        Ok(WorktreeInfo {
            spec_id: spec_id.to_string(),
            path: wt_path,
            branch: branch_name,
            exists_on_disk: true,
        })
    }

    /// List all Surge-managed worktrees.
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, GitError> {
        let repo = self.open_repo()?;
        let worktrees = repo.worktrees()?;
        let mut result = Vec::new();

        for name in worktrees.iter() {
            let Some(name) = name else { continue };
            let wt = match repo.find_worktree(name) {
                Ok(wt) => wt,
                Err(_) => continue,
            };
            let wt_path = wt.path().to_path_buf();
            result.push(WorktreeInfo {
                spec_id: name.to_string(),
                path: wt_path.clone(),
                branch: Self::branch_name(name),
                exists_on_disk: wt_path.exists(),
            });
        }

        Ok(result)
    }

    /// Returns `true` if the worktree for the given spec has any changes:
    /// staged, unstaged, or untracked files.
    pub fn has_changes(&self, spec_id: &str) -> Result<bool, GitError> {
        let wt_path = self.worktree_path(spec_id);
        if !wt_path.exists() {
            return Err(GitError::WorktreeNotFound(spec_id.to_string()));
        }
        let wt_repo = Repository::open(&wt_path)?;
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true).include_ignored(false);
        let statuses = wt_repo.statuses(Some(&mut opts))?;
        Ok(!statuses.is_empty())
    }

    /// Stage all changes and commit in the worktree for the given spec.
    ///
    /// Returns [`GitError::NothingToCommit`] if there are no changes to stage.
    pub fn commit(&self, spec_id: &str, message: &str) -> Result<git2::Oid, GitError> {
        let wt_path = self.worktree_path(spec_id);
        if !wt_path.exists() {
            return Err(GitError::WorktreeNotFound(spec_id.to_string()));
        }

        let wt_repo = Repository::open(&wt_path)?;
        let sig = Self::signature(&wt_repo);

        let mut index = wt_repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;

        // Guard: nothing to commit
        let head = wt_repo.head()?;
        let head_commit = head.peel_to_commit()?;
        let head_tree = head_commit.tree()?;
        let diff = wt_repo.diff_tree_to_index(Some(&head_tree), Some(&index), None)?;
        if diff.stats()?.files_changed() == 0 {
            return Err(GitError::NothingToCommit(spec_id.to_string()));
        }

        let tree_oid = index.write_tree()?;
        let tree = wt_repo.find_tree(tree_oid)?;
        let oid = wt_repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&head_commit])?;

        info!(spec_id, %oid, "committed in worktree");
        Ok(oid)
    }

    /// Get the diff of committed changes on the surge branch vs its merge base.
    ///
    /// Only shows **committed** changes. Uncommitted changes in the worktree
    /// working directory are not included.
    pub fn diff(&self, spec_id: &str) -> Result<String, GitError> {
        let repo = self.open_repo()?;
        let branch_name = Self::branch_name(spec_id);

        let branch = repo
            .find_branch(&branch_name, BranchType::Local)
            .map_err(|_| GitError::BranchNotFound(branch_name.clone()))?;
        let branch_commit = branch.get().peel_to_commit()?;

        let head = repo.head()?;
        let head_commit = head.peel_to_commit()?;

        let merge_base = repo.merge_base(head_commit.id(), branch_commit.id())?;
        let base_tree = repo.find_commit(merge_base)?.tree()?;
        let branch_tree = branch_commit.tree()?;

        let mut diff_opts = DiffOptions::new();
        let diff =
            repo.diff_tree_to_tree(Some(&base_tree), Some(&branch_tree), Some(&mut diff_opts))?;

        let mut diff_text = Vec::new();
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            diff_text.extend_from_slice(line.content());
            true
        })?;

        Ok(String::from_utf8_lossy(&diff_text).into_owned())
    }

    /// Discard a worktree: prune it, remove the directory, delete the branch.
    pub fn discard(&self, spec_id: &str) -> Result<(), GitError> {
        let repo = self.open_repo()?;
        let wt_path = self.worktree_path(spec_id);
        let branch_name = Self::branch_name(spec_id);

        match repo.find_worktree(spec_id) {
            Ok(wt) => {
                let mut prune_opts = WorktreePruneOptions::new();
                prune_opts.valid(true).working_tree(true);
                if let Err(e) = wt.prune(Some(&mut prune_opts)) {
                    warn!(spec_id, %e, "worktree prune failed, continuing cleanup");
                }
            }
            Err(e) => debug!(spec_id, %e, "worktree not found in git, continuing cleanup"),
        }

        if wt_path.exists() {
            fs::remove_dir_all(&wt_path)?;
            debug!(spec_id, ?wt_path, "removed worktree directory");
        }

        match repo.find_branch(&branch_name, BranchType::Local) {
            Ok(mut branch) => {
                branch.delete()?;
                info!(spec_id, branch_name, "deleted branch");
            }
            Err(e) => debug!(spec_id, %e, "branch not found, skipping delete"),
        }

        Ok(())
    }

    /// Merge the surge branch into the target branch (default: current HEAD branch).
    ///
    /// Performs a fast-forward if possible, otherwise creates a merge commit.
    /// When `checkout` is `false` the working tree is not updated — useful for
    /// background merges into a project branch that is not currently checked out.
    ///
    /// # Errors
    ///
    /// Returns [`GitError::MergeConflict`] with the list of conflicting files,
    /// [`GitError::SameBranch`] if source and target are the same reference.
    pub fn merge(
        &self,
        spec_id: &str,
        target_branch: Option<&str>,
        checkout: bool,
    ) -> Result<git2::Oid, GitError> {
        let repo = self.open_repo()?;
        let branch_name = Self::branch_name(spec_id);

        let target_ref_name = if let Some(target) = target_branch {
            format!("refs/heads/{target}")
        } else {
            let head = repo.head()?;
            head.name()
                .ok_or_else(|| GitError::BranchNotFound("HEAD".to_string()))?
                .to_string()
        };

        // Guard: source ≠ target
        let source_ref_name = format!("refs/heads/{branch_name}");
        if source_ref_name == target_ref_name {
            return Err(GitError::SameBranch(branch_name));
        }

        let surge_branch = repo
            .find_branch(&branch_name, BranchType::Local)
            .map_err(|_| GitError::BranchNotFound(branch_name.clone()))?;
        let surge_commit = surge_branch.get().peel_to_commit()?;

        let target_ref = repo.find_reference(&target_ref_name)?;
        let target_commit = target_ref.peel_to_commit()?;

        let can_ff = repo.graph_descendant_of(surge_commit.id(), target_commit.id())?;

        if can_ff {
            repo.reference(
                &target_ref_name,
                surge_commit.id(),
                true,
                &format!("surge: fast-forward merge of {branch_name}"),
            )?;
            if checkout {
                repo.set_head(&target_ref_name)?;
                repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
            }
            info!(spec_id, "fast-forward merge complete");
            Ok(surge_commit.id())
        } else {
            let merge_opts = MergeOptions::new();
            let mut merged_index: Index =
                repo.merge_commits(&target_commit, &surge_commit, Some(&merge_opts))?;

            if merged_index.has_conflicts() {
                let mut files = Vec::new();
                if let Ok(mut conflicts) = merged_index.conflicts() {
                    while let Some(Ok(conflict)) = conflicts.next() {
                        let entry = conflict.our.or(conflict.their).or(conflict.ancestor);
                        if let Some(e) = entry {
                            let path = PathBuf::from(String::from_utf8_lossy(&e.path).as_ref());
                            files.push(path);
                        }
                    }
                }
                return Err(GitError::MergeConflict {
                    conflicting_files: files,
                });
            }

            let tree_oid = merged_index.write_tree_to(&repo)?;
            let tree = repo.find_tree(tree_oid)?;
            let sig = Self::signature(&repo);
            let merge_msg = format!("Merge branch '{branch_name}'");
            let oid = repo.commit(
                Some(&target_ref_name),
                &sig,
                &sig,
                &merge_msg,
                &tree,
                &[&target_commit, &surge_commit],
            )?;

            if checkout {
                repo.set_head(&target_ref_name)?;
                repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
            }

            info!(spec_id, %oid, "merge commit created");
            Ok(oid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::init_test_repo;

    #[test]
    fn test_create_worktree() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        let info = gm.create_worktree("test-spec", None).unwrap();
        assert_eq!(info.spec_id, "test-spec");
        assert_eq!(info.branch, "surge/test-spec");
        assert!(info.exists_on_disk);
        assert!(info.path.exists());
    }

    #[test]
    fn test_create_worktree_from_base_branch() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create a separate branch to use as base
        {
            let repo = Repository::open(&path).unwrap();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.branch("feature-base", &head, false).unwrap();
        }

        let info = gm
            .create_worktree("based-spec", Some("feature-base"))
            .unwrap();
        assert!(info.exists_on_disk);

        // Non-existent base branch should error
        let result = gm.create_worktree("bad-base", Some("nonexistent"));
        assert!(matches!(result, Err(GitError::BranchNotFound(_))));
    }

    #[test]
    fn test_create_worktree_duplicate() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        gm.create_worktree("dup-spec", None).unwrap();
        let result = gm.create_worktree("dup-spec", None);
        assert!(matches!(result, Err(GitError::WorktreeAlreadyExists(_))));
    }

    #[test]
    fn test_list_worktrees() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        gm.create_worktree("spec-a", None).unwrap();
        gm.create_worktree("spec-b", None).unwrap();
        let list = gm.list_worktrees().unwrap();
        assert_eq!(list.len(), 2);
        let ids: Vec<&str> = list.iter().map(|w| w.spec_id.as_str()).collect();
        assert!(ids.contains(&"spec-a"));
        assert!(ids.contains(&"spec-b"));
    }

    #[test]
    fn test_list_empty() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        assert!(gm.list_worktrees().unwrap().is_empty());
    }

    #[test]
    fn test_has_uncommitted_changes_clean() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        assert!(!gm.has_uncommitted_changes().unwrap());
    }

    #[test]
    fn test_has_uncommitted_changes_dirty() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        fs::write(path.join("README.md"), "changed\n").unwrap();
        assert!(gm.has_uncommitted_changes().unwrap());
    }

    #[test]
    fn test_uncommitted_files() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        fs::write(path.join("README.md"), "changed\n").unwrap();
        let files = gm.uncommitted_files().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], PathBuf::from("README.md"));
    }

    #[test]
    fn test_has_changes_no_changes() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        gm.create_worktree("clean-spec", None).unwrap();
        assert!(!gm.has_changes("clean-spec").unwrap());
    }

    #[test]
    fn test_has_changes_with_changes() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        let info = gm.create_worktree("dirty-spec", None).unwrap();
        fs::write(info.path.join("new.txt"), "hello\n").unwrap();
        assert!(gm.has_changes("dirty-spec").unwrap());
    }

    #[test]
    fn test_commit_in_worktree() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        let info = gm.create_worktree("commit-spec", None).unwrap();
        fs::write(info.path.join("new_file.txt"), "hello world\n").unwrap();
        let oid = gm.commit("commit-spec", "add new file").unwrap();
        assert!(!oid.is_zero());
        let wt_repo = Repository::open(&info.path).unwrap();
        assert_eq!(
            wt_repo.find_commit(oid).unwrap().message(),
            Some("add new file")
        );
    }

    #[test]
    fn test_commit_nothing_to_commit() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        gm.create_worktree("empty-commit-spec", None).unwrap();
        assert!(matches!(
            gm.commit("empty-commit-spec", "should fail"),
            Err(GitError::NothingToCommit(_))
        ));
    }

    #[test]
    fn test_diff() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        let info = gm.create_worktree("diff-spec", None).unwrap();
        fs::write(info.path.join("diff_test.txt"), "diff content\n").unwrap();
        gm.commit("diff-spec", "add diff file").unwrap();
        let diff_text = gm.diff("diff-spec").unwrap();
        assert!(diff_text.contains("diff_test.txt"));
        assert!(diff_text.contains("diff content"));
    }

    #[test]
    fn test_discard() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        let info = gm.create_worktree("discard-spec", None).unwrap();
        assert!(info.path.exists());
        gm.discard("discard-spec").unwrap();
        assert!(!info.path.exists());
        let repo = Repository::open(&path).unwrap();
        assert!(
            repo.find_branch("surge/discard-spec", BranchType::Local)
                .is_err()
        );
    }

    #[test]
    fn test_merge_fast_forward() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        let info = gm.create_worktree("merge-spec", None).unwrap();
        fs::write(info.path.join("merge_file.txt"), "merge content\n").unwrap();
        gm.commit("merge-spec", "add merge file").unwrap();
        let oid = gm.merge("merge-spec", None, true).unwrap();
        assert!(!oid.is_zero());
        let repo = Repository::open(&path).unwrap();
        assert_eq!(
            repo.head().unwrap().peel_to_commit().unwrap().message(),
            Some("add merge file")
        );
    }

    #[test]
    fn test_merge_no_checkout() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        // Create a target branch to merge into (not current HEAD)
        let target_branch = "project-branch";
        {
            let repo = Repository::open(&path).unwrap();
            let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
            repo.branch(target_branch, &head_commit, false).unwrap();
        }

        let info = gm.create_worktree("bg-merge-spec", None).unwrap();
        fs::write(info.path.join("bg.txt"), "background\n").unwrap();
        gm.commit("bg-merge-spec", "background commit").unwrap();

        // Merge into project-branch without checkout — HEAD should stay on original branch
        let repo_before_head = {
            let repo = Repository::open(&path).unwrap();
            repo.head().unwrap().name().unwrap().to_string()
        };

        gm.merge("bg-merge-spec", Some(target_branch), false)
            .unwrap();

        let repo_after_head = {
            let repo = Repository::open(&path).unwrap();
            repo.head().unwrap().name().unwrap().to_string()
        };

        // HEAD should not have changed
        assert_eq!(repo_before_head, repo_after_head);

        // The target branch should have the new commit
        let repo = Repository::open(&path).unwrap();
        let tb = repo.find_branch(target_branch, BranchType::Local).unwrap();
        let tb_commit = tb.get().peel_to_commit().unwrap();
        assert_eq!(tb_commit.message(), Some("background commit"));
    }

    #[test]
    fn test_merge_same_branch_error() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        gm.create_worktree("same-spec", None).unwrap();
        let result = gm.merge("same-spec", Some("surge/same-spec"), true);
        assert!(matches!(result, Err(GitError::SameBranch(_))));
    }

    #[test]
    fn test_from_git_error_for_surge_error() {
        use surge_core::SurgeError;
        let e: SurgeError = GitError::WorktreeNotFound("abc".into()).into();
        assert!(matches!(e, SurgeError::NotFound(_)));
        let e: SurgeError = GitError::BranchNotFound("main".into()).into();
        assert!(matches!(e, SurgeError::NotFound(_)));
        let e: SurgeError = GitError::MergeConflict {
            conflicting_files: vec![],
        }
        .into();
        assert!(matches!(e, SurgeError::Git { .. }));
    }

    #[test]
    fn test_worktree_path_is_public() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();
        let wt_path = gm.worktree_path("my-spec");
        assert!(wt_path.ends_with(".surge/worktrees/my-spec"));
    }
}
