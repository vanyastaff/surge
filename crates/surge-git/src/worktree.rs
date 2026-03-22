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

    #[error("merge conflict: cannot fast-forward or cleanly merge")]
    MergeConflict,

    #[error("repository has no commits")]
    EmptyRepository,
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
        // Verify the repo can be opened
        let _ = Repository::open(&repo_path)?;
        Ok(Self { repo_path })
    }

    /// Discover the git repository from the current working directory.
    ///
    /// # Errors
    ///
    /// Returns an error if no git repository is found.
    pub fn discover() -> Result<Self, GitError> {
        let repo = Repository::discover(".")?;
        let repo_path = repo
            .workdir()
            .unwrap_or_else(|| repo.path())
            .to_path_buf();
        Ok(Self { repo_path })
    }

    /// Returns the repository root path.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Open the repository. Internal helper.
    fn open_repo(&self) -> Result<Repository, GitError> {
        Ok(Repository::open(&self.repo_path)?)
    }

    /// Build a default signature, falling back to Surge defaults.
    fn signature(repo: &Repository) -> Signature<'_> {
        repo.signature()
            .unwrap_or_else(|_| Signature::now("Surge", "surge@localhost").unwrap())
    }

    /// The branch name for a given spec id.
    fn branch_name(spec_id: &str) -> String {
        format!("surge/{spec_id}")
    }

    /// The worktree directory for a given spec id.
    fn worktree_path(&self, spec_id: &str) -> PathBuf {
        self.repo_path
            .join(".surge")
            .join("worktrees")
            .join(spec_id)
    }

    /// Create a new worktree and branch for the given spec.
    ///
    /// Creates branch `surge/{spec_id}` and checks out the worktree at
    /// `.surge/worktrees/{spec_id}` relative to the repository root.
    ///
    /// # Errors
    ///
    /// Returns [`GitError::WorktreeAlreadyExists`] if a worktree for this spec
    /// already exists.
    pub fn create_worktree(&self, spec_id: &str) -> Result<WorktreeInfo, GitError> {
        let repo = self.open_repo()?;
        let branch_name = Self::branch_name(spec_id);
        let wt_path = self.worktree_path(spec_id);

        // Check if worktree already exists
        let worktrees = repo.worktrees()?;
        for name in worktrees.iter().flatten() {
            if name == spec_id {
                return Err(GitError::WorktreeAlreadyExists(spec_id.to_string()));
            }
        }

        // Get HEAD commit to base the branch on
        let head = repo.head().map_err(|_| GitError::EmptyRepository)?;
        let commit = head
            .peel_to_commit()
            .map_err(|_| GitError::EmptyRepository)?;

        // Create the branch
        let branch = repo.branch(&branch_name, &commit, false)?;

        // Create worktree directory parent
        if let Some(parent) = wt_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Create worktree with the branch reference
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

            // Try to look up worktree info
            let wt = match repo.find_worktree(name) {
                Ok(wt) => wt,
                Err(_) => continue,
            };

            let wt_path = wt.path().to_path_buf();
            let branch_name = Self::branch_name(name);
            let exists_on_disk = wt_path.exists();

            result.push(WorktreeInfo {
                spec_id: name.to_string(),
                path: wt_path,
                branch: branch_name,
                exists_on_disk,
            });
        }

        Ok(result)
    }

    /// Stage all changes and commit in the worktree for the given spec.
    ///
    /// # Errors
    ///
    /// Returns an error if the worktree does not exist or the commit fails.
    pub fn commit(&self, spec_id: &str, message: &str) -> Result<git2::Oid, GitError> {
        let wt_path = self.worktree_path(spec_id);
        if !wt_path.exists() {
            return Err(GitError::WorktreeNotFound(spec_id.to_string()));
        }

        let wt_repo = Repository::open(&wt_path)?;
        let sig = Self::signature(&wt_repo);

        // Stage all changes
        let mut index = wt_repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = wt_repo.find_tree(tree_oid)?;

        // Get parent commit
        let head = wt_repo.head()?;
        let parent = head.peel_to_commit()?;

        let oid = wt_repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;

        info!(spec_id, %oid, "committed in worktree");
        Ok(oid)
    }

    /// Get the diff between the surge branch and its merge base with HEAD.
    ///
    /// Returns the diff as a string.
    pub fn diff(&self, spec_id: &str) -> Result<String, GitError> {
        let repo = self.open_repo()?;
        let branch_name = Self::branch_name(spec_id);

        // Find the surge branch
        let branch = repo
            .find_branch(&branch_name, BranchType::Local)
            .map_err(|_| GitError::BranchNotFound(branch_name.clone()))?;
        let branch_commit = branch.get().peel_to_commit()?;

        // Find HEAD
        let head = repo.head()?;
        let head_commit = head.peel_to_commit()?;

        // Find merge base
        let merge_base = repo.merge_base(head_commit.id(), branch_commit.id())?;
        let base_commit = repo.find_commit(merge_base)?;
        let base_tree = base_commit.tree()?;
        let branch_tree = branch_commit.tree()?;

        let mut diff_opts = DiffOptions::new();
        let diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&branch_tree), Some(&mut diff_opts))?;

        let mut diff_text = Vec::new();
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            diff_text.extend_from_slice(line.content());
            true
        })?;

        Ok(String::from_utf8_lossy(&diff_text).into_owned())
    }

    /// Discard a worktree: prune it, remove the directory, and delete the branch.
    pub fn discard(&self, spec_id: &str) -> Result<(), GitError> {
        let repo = self.open_repo()?;
        let wt_path = self.worktree_path(spec_id);
        let branch_name = Self::branch_name(spec_id);

        // Try to prune the worktree via git2
        match repo.find_worktree(spec_id) {
            Ok(wt) => {
                let mut prune_opts = WorktreePruneOptions::new();
                prune_opts.valid(true);
                prune_opts.working_tree(true);
                // Prune may fail if already gone — that's OK
                if let Err(e) = wt.prune(Some(&mut prune_opts)) {
                    warn!(spec_id, %e, "worktree prune failed, continuing cleanup");
                }
            }
            Err(e) => {
                debug!(spec_id, %e, "worktree not found in git, continuing cleanup");
            }
        }

        // Remove the worktree directory if it still exists
        if wt_path.exists() {
            fs::remove_dir_all(&wt_path)?;
            debug!(spec_id, ?wt_path, "removed worktree directory");
        }

        // Delete the branch
        match repo.find_branch(&branch_name, BranchType::Local) {
            Ok(mut branch) => {
                branch.delete()?;
                info!(spec_id, branch_name, "deleted branch");
            }
            Err(e) => {
                debug!(spec_id, %e, "branch not found, skipping delete");
            }
        }

        Ok(())
    }

    /// Merge the surge branch into the target branch (default: current HEAD branch).
    ///
    /// Performs a fast-forward if possible, otherwise creates a merge commit.
    /// Returns an error on conflicts.
    pub fn merge(&self, spec_id: &str, target_branch: Option<&str>) -> Result<git2::Oid, GitError> {
        let repo = self.open_repo()?;
        let branch_name = Self::branch_name(spec_id);

        // Find the surge branch commit
        let surge_branch = repo
            .find_branch(&branch_name, BranchType::Local)
            .map_err(|_| GitError::BranchNotFound(branch_name.clone()))?;
        let surge_commit = surge_branch.get().peel_to_commit()?;

        // Resolve target branch
        let target_ref_name = if let Some(target) = target_branch {
            format!("refs/heads/{target}")
        } else {
            let head = repo.head()?;
            head.name()
                .ok_or_else(|| GitError::BranchNotFound("HEAD".to_string()))?
                .to_string()
        };

        let target_ref = repo.find_reference(&target_ref_name)?;
        let target_commit = target_ref.peel_to_commit()?;

        // Check if fast-forward is possible: surge commit is descendant of target
        let can_ff = repo.graph_descendant_of(surge_commit.id(), target_commit.id())?;

        if can_ff {
            // Fast-forward: just update the target ref to point to surge commit
            repo.reference(
                &target_ref_name,
                surge_commit.id(),
                true,
                &format!("surge: fast-forward merge of {branch_name}"),
            )?;

            // Update HEAD / working tree
            repo.set_head(&target_ref_name)?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;

            info!(spec_id, "fast-forward merge complete");
            Ok(surge_commit.id())
        } else {
            // Try a real merge
            let merge_opts = MergeOptions::new();
            let mut merged_index: Index =
                repo.merge_commits(&target_commit, &surge_commit, Some(&merge_opts))?;

            if merged_index.has_conflicts() {
                return Err(GitError::MergeConflict);
            }

            // Write the merged index to a tree
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

            // Update working tree
            repo.set_head(&target_ref_name)?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;

            info!(spec_id, %oid, "merge commit created");
            Ok(oid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Create a temporary git repo with an initial commit.
    fn init_test_repo() -> (tempfile::TempDir, PathBuf) {
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

        // Create initial commit
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
    fn test_create_worktree() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        let info = gm.create_worktree("test-spec").unwrap();
        assert_eq!(info.spec_id, "test-spec");
        assert_eq!(info.branch, "surge/test-spec");
        assert!(info.exists_on_disk);
        assert!(info.path.exists());
    }

    #[test]
    fn test_create_worktree_duplicate() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        gm.create_worktree("dup-spec").unwrap();
        let result = gm.create_worktree("dup-spec");
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), GitError::WorktreeAlreadyExists(ref s) if s == "dup-spec")
        );
    }

    #[test]
    fn test_list_worktrees() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        gm.create_worktree("spec-a").unwrap();
        gm.create_worktree("spec-b").unwrap();

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

        let list = gm.list_worktrees().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_commit_in_worktree() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        let info = gm.create_worktree("commit-spec").unwrap();

        // Write a file in the worktree
        let new_file = info.path.join("new_file.txt");
        fs::write(&new_file, "hello world\n").unwrap();

        let oid = gm.commit("commit-spec", "add new file").unwrap();
        assert!(!oid.is_zero());

        // Verify commit exists
        let wt_repo = Repository::open(&info.path).unwrap();
        let commit = wt_repo.find_commit(oid).unwrap();
        assert_eq!(commit.message(), Some("add new file"));
    }

    #[test]
    fn test_diff() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        let info = gm.create_worktree("diff-spec").unwrap();

        // Add a file and commit in the worktree
        let new_file = info.path.join("diff_test.txt");
        fs::write(&new_file, "diff content\n").unwrap();
        gm.commit("diff-spec", "add diff file").unwrap();

        let diff_text = gm.diff("diff-spec").unwrap();
        assert!(diff_text.contains("diff_test.txt"));
        assert!(diff_text.contains("diff content"));
    }

    #[test]
    fn test_discard() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        let info = gm.create_worktree("discard-spec").unwrap();
        assert!(info.path.exists());

        gm.discard("discard-spec").unwrap();

        // Worktree dir should be gone
        assert!(!info.path.exists());

        // Branch should be gone
        let repo = Repository::open(&path).unwrap();
        assert!(repo
            .find_branch("surge/discard-spec", BranchType::Local)
            .is_err());
    }

    #[test]
    fn test_merge_fast_forward() {
        let (_dir, path) = init_test_repo();
        let gm = GitManager::new(path.clone()).unwrap();

        let info = gm.create_worktree("merge-spec").unwrap();

        // Add a file and commit in the worktree
        let new_file = info.path.join("merge_file.txt");
        fs::write(&new_file, "merge content\n").unwrap();
        gm.commit("merge-spec", "add merge file").unwrap();

        // Merge back — should be fast-forward since main hasn't moved
        let oid = gm.merge("merge-spec", None).unwrap();
        assert!(!oid.is_zero());

        // Verify the file is now accessible from main
        let repo = Repository::open(&path).unwrap();
        let head = repo.head().unwrap();
        let commit = head.peel_to_commit().unwrap();
        assert_eq!(commit.message(), Some("add merge file"));
    }
}
