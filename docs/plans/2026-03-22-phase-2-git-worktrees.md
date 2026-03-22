# Phase 2: Git Worktrees — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build `surge-git` crate — isolated git worktrees per task with full lifecycle (create, commit, diff, merge, discard) and cleanup tooling.

**Architecture:** New `surge-git` crate wraps `git2` (libgit2 bindings). `GitManager` operates on the main repo, creating worktrees in `.surge/worktrees/{spec-id}` on branches `surge/{spec-id}`. `LifecycleManager` handles cleanup of orphaned worktrees and stale branches. CLI gets git-oriented commands.

**Tech Stack:** Rust 2024, git2, surge-core (SpecId, SurgeError), clap 4

---

### Task 1: Create surge-git crate scaffold

**Files:**
- Create: `crates/surge-git/Cargo.toml`
- Create: `crates/surge-git/src/lib.rs`
- Create: `crates/surge-git/src/worktree.rs` (empty)
- Create: `crates/surge-git/src/cleanup.rs` (empty)
- Modify: `Cargo.toml` (workspace)
- Modify: `crates/surge-cli/Cargo.toml`

**Step 1: Create crate**

`crates/surge-git/Cargo.toml`:
```toml
[package]
name = "surge-git"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
surge-core = { workspace = true }
git2 = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
```

`crates/surge-git/src/lib.rs`:
```rust
//! Git worktree management for Surge — isolated workspaces per task.

pub mod worktree;
pub mod cleanup;

pub use worktree::GitManager;
pub use cleanup::LifecycleManager;
```

Empty module files with doc comments.

**Step 2: Update workspace**

Add to `Cargo.toml` workspace members: `"crates/surge-git"`

Add workspace dependency: `git2 = "0.20"`

Add internal crate: `surge-git = { path = "crates/surge-git" }`

Add `surge-git = { workspace = true }` to `crates/surge-cli/Cargo.toml`.

**Step 3: Verify**

Run: `cargo check --workspace`

**Step 4: Commit**

```bash
git add crates/surge-git/ Cargo.toml crates/surge-cli/Cargo.toml
git commit -m "feat(git): create surge-git crate scaffold"
```

---

### Task 2: GitManager — worktree creation and listing

**Files:**
- Create: `crates/surge-git/src/worktree.rs`

**Step 1: Write GitManager with create_worktree and list_worktrees**

```rust
//! Git worktree lifecycle management.

use git2::{BranchType, Repository};
use std::path::{Path, PathBuf};
use surge_core::SurgeError;
use tracing::{debug, info};

/// Information about an active worktree.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Spec ID this worktree belongs to.
    pub spec_id: String,
    /// Path to the worktree directory.
    pub path: PathBuf,
    /// Branch name.
    pub branch: String,
    /// Whether the worktree directory exists on disk.
    pub exists_on_disk: bool,
}

/// Manages git worktrees for Surge tasks.
pub struct GitManager {
    /// Path to the main repository.
    repo_path: PathBuf,
}

impl GitManager {
    /// Create a new GitManager for the repository at the given path.
    pub fn new(repo_path: PathBuf) -> Result<Self, SurgeError> {
        // Verify it's a valid git repo
        Repository::open(&repo_path)
            .map_err(|e| SurgeError::Git(format!("Not a git repository: {e}")))?;
        Ok(Self { repo_path })
    }

    /// Discover the git repository from the current directory.
    pub fn discover() -> Result<Self, SurgeError> {
        let repo = Repository::discover(".")
            .map_err(|e| SurgeError::Git(format!("Could not find git repository: {e}")))?;
        let repo_path = repo.workdir()
            .ok_or_else(|| SurgeError::Git("Bare repository not supported".to_string()))?
            .to_path_buf();
        Ok(Self { repo_path })
    }

    fn open_repo(&self) -> Result<Repository, SurgeError> {
        Repository::open(&self.repo_path)
            .map_err(|e| SurgeError::Git(format!("Failed to open repository: {e}")))
    }

    /// Get the worktrees base directory (.surge/worktrees/).
    fn worktrees_dir(&self) -> PathBuf {
        self.repo_path.join(".surge").join("worktrees")
    }

    /// Get the worktree path for a spec ID.
    fn worktree_path(&self, spec_id: &str) -> PathBuf {
        self.worktrees_dir().join(spec_id)
    }

    /// Get the branch name for a spec ID.
    fn branch_name(&self, spec_id: &str) -> String {
        format!("surge/{spec_id}")
    }

    /// Create a new worktree for a spec.
    ///
    /// Creates a branch `surge/{spec_id}` and a worktree at `.surge/worktrees/{spec_id}`.
    pub fn create_worktree(&self, spec_id: &str) -> Result<WorktreeInfo, SurgeError> {
        let repo = self.open_repo()?;
        let wt_path = self.worktree_path(spec_id);
        let branch_name = self.branch_name(spec_id);

        // Check if worktree already exists
        if wt_path.exists() {
            return Err(SurgeError::Git(format!(
                "Worktree already exists at {}",
                wt_path.display()
            )));
        }

        // Create parent directory
        std::fs::create_dir_all(self.worktrees_dir())?;

        // Get HEAD commit to branch from
        let head = repo.head()
            .map_err(|e| SurgeError::Git(format!("Failed to get HEAD: {e}")))?;
        let head_commit = head.peel_to_commit()
            .map_err(|e| SurgeError::Git(format!("Failed to get HEAD commit: {e}")))?;

        // Create branch
        let branch = repo.branch(&branch_name, &head_commit, false)
            .map_err(|e| SurgeError::Git(format!("Failed to create branch '{branch_name}': {e}")))?;

        info!("Created branch '{branch_name}' from HEAD");

        // Create worktree
        let branch_ref = branch.into_reference();
        let ref_name = branch_ref.name()
            .ok_or_else(|| SurgeError::Git("Branch ref has no name".to_string()))?;

        repo.worktree(spec_id, &wt_path, Some(
            git2::WorktreeAddOptions::new()
                .reference(Some(&repo.find_reference(ref_name)
                    .map_err(|e| SurgeError::Git(format!("Failed to find ref: {e}")))?))
        )).map_err(|e| SurgeError::Git(format!("Failed to create worktree: {e}")))?;

        info!("Created worktree at {}", wt_path.display());

        Ok(WorktreeInfo {
            spec_id: spec_id.to_string(),
            path: wt_path,
            branch: branch_name,
            exists_on_disk: true,
        })
    }

    /// List all surge worktrees.
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, SurgeError> {
        let repo = self.open_repo()?;
        let mut worktrees = vec![];

        let wt_names = repo.worktrees()
            .map_err(|e| SurgeError::Git(format!("Failed to list worktrees: {e}")))?;

        for name in wt_names.iter() {
            let Some(name) = name else { continue };
            let wt_path = self.worktree_path(name);
            let branch_name = self.branch_name(name);

            worktrees.push(WorktreeInfo {
                spec_id: name.to_string(),
                path: wt_path.clone(),
                branch: branch_name,
                exists_on_disk: wt_path.exists(),
            });
        }

        Ok(worktrees)
    }

    /// Get the repository path.
    #[must_use]
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Create a temporary git repo for testing.
    fn create_test_repo() -> (tempfile::TempDir, GitManager) {
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();

        // Init git repo
        Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // Configure git user for commits
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // Create initial commit
        std::fs::write(repo_path.join("README.md"), "# Test").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        let manager = GitManager::new(repo_path.to_path_buf()).unwrap();
        (temp_dir, manager)
    }

    #[test]
    fn test_create_worktree() {
        let (_temp, mgr) = create_test_repo();
        let info = mgr.create_worktree("test-spec").unwrap();

        assert_eq!(info.spec_id, "test-spec");
        assert_eq!(info.branch, "surge/test-spec");
        assert!(info.path.exists());
        assert!(info.exists_on_disk);
    }

    #[test]
    fn test_create_worktree_duplicate() {
        let (_temp, mgr) = create_test_repo();
        mgr.create_worktree("dup-spec").unwrap();
        let result = mgr.create_worktree("dup-spec");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_worktrees() {
        let (_temp, mgr) = create_test_repo();
        mgr.create_worktree("spec-1").unwrap();
        mgr.create_worktree("spec-2").unwrap();

        let list = mgr.list_worktrees().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_list_empty() {
        let (_temp, mgr) = create_test_repo();
        let list = mgr.list_worktrees().unwrap();
        assert!(list.is_empty());
    }
}
```

Add `tempfile = "3"` to `[dev-dependencies]` in `crates/surge-git/Cargo.toml`.

**Step 2: Run tests**

Run: `cargo test -p surge-git -- worktree`
Expected: 4 tests PASS

**Step 3: Commit**

```bash
git add crates/surge-git/
git commit -m "feat(git): add GitManager with worktree creation and listing"
```

---

### Task 3: GitManager — commit, diff, discard

**Files:**
- Modify: `crates/surge-git/src/worktree.rs`

**Step 1: Add commit, diff, and discard methods to GitManager**

Add these methods to the `impl GitManager` block:

```rust
    /// Commit all changes in a worktree.
    pub fn commit(&self, spec_id: &str, message: &str) -> Result<git2::Oid, SurgeError> {
        let wt_path = self.worktree_path(spec_id);
        if !wt_path.exists() {
            return Err(SurgeError::Git(format!("Worktree not found: {}", wt_path.display())));
        }

        let repo = Repository::open(&wt_path)
            .map_err(|e| SurgeError::Git(format!("Failed to open worktree repo: {e}")))?;

        // Stage all changes
        let mut index = repo.index()
            .map_err(|e| SurgeError::Git(format!("Failed to get index: {e}")))?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .map_err(|e| SurgeError::Git(format!("Failed to stage files: {e}")))?;
        index.write()
            .map_err(|e| SurgeError::Git(format!("Failed to write index: {e}")))?;

        let tree_id = index.write_tree()
            .map_err(|e| SurgeError::Git(format!("Failed to write tree: {e}")))?;
        let tree = repo.find_tree(tree_id)
            .map_err(|e| SurgeError::Git(format!("Failed to find tree: {e}")))?;

        let head = repo.head()
            .map_err(|e| SurgeError::Git(format!("Failed to get HEAD: {e}")))?;
        let parent = head.peel_to_commit()
            .map_err(|e| SurgeError::Git(format!("Failed to get parent commit: {e}")))?;

        let sig = repo.signature()
            .or_else(|_| git2::Signature::now("Surge", "surge@localhost"))
            .map_err(|e| SurgeError::Git(format!("Failed to create signature: {e}")))?;

        let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
            .map_err(|e| SurgeError::Git(format!("Failed to commit: {e}")))?;

        info!("Committed {} in worktree '{}'", oid, spec_id);
        Ok(oid)
    }

    /// Get diff between worktree branch and base branch (e.g. main).
    pub fn diff(&self, spec_id: &str) -> Result<String, SurgeError> {
        let repo = self.open_repo()?;
        let branch_name = self.branch_name(spec_id);

        // Find the branch
        let branch = repo.find_branch(&branch_name, BranchType::Local)
            .map_err(|e| SurgeError::Git(format!("Branch '{branch_name}' not found: {e}")))?;
        let branch_commit = branch.get().peel_to_commit()
            .map_err(|e| SurgeError::Git(format!("Failed to get branch commit: {e}")))?;

        // Find merge base with HEAD of main repo
        let head = repo.head()
            .map_err(|e| SurgeError::Git(format!("Failed to get HEAD: {e}")))?;
        let head_commit = head.peel_to_commit()
            .map_err(|e| SurgeError::Git(format!("Failed to get HEAD commit: {e}")))?;

        let merge_base = repo.merge_base(head_commit.id(), branch_commit.id())
            .map_err(|e| SurgeError::Git(format!("Failed to find merge base: {e}")))?;

        let base_commit = repo.find_commit(merge_base)
            .map_err(|e| SurgeError::Git(format!("Failed to find base commit: {e}")))?;
        let base_tree = base_commit.tree()
            .map_err(|e| SurgeError::Git(format!("Failed to get base tree: {e}")))?;
        let branch_tree = branch_commit.tree()
            .map_err(|e| SurgeError::Git(format!("Failed to get branch tree: {e}")))?;

        let diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&branch_tree), None)
            .map_err(|e| SurgeError::Git(format!("Failed to compute diff: {e}")))?;

        let mut diff_text = String::new();
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            let prefix = match line.origin() {
                '+' => "+",
                '-' => "-",
                ' ' => " ",
                _ => "",
            };
            diff_text.push_str(prefix);
            diff_text.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            true
        }).map_err(|e| SurgeError::Git(format!("Failed to format diff: {e}")))?;

        Ok(diff_text)
    }

    /// Discard a worktree — removes the worktree directory and deletes the branch.
    pub fn discard(&self, spec_id: &str) -> Result<(), SurgeError> {
        let repo = self.open_repo()?;
        let wt_path = self.worktree_path(spec_id);
        let branch_name = self.branch_name(spec_id);

        // Remove worktree from git
        if let Ok(wt) = repo.find_worktree(spec_id) {
            debug!("Pruning worktree '{}'", spec_id);
            wt.prune(Some(
                git2::WorktreePruneOptions::new()
                    .working_tree(true)
                    .valid(true)
                    .locked(false)
            )).map_err(|e| SurgeError::Git(format!("Failed to prune worktree: {e}")))?;
        }

        // Remove directory if still exists
        if wt_path.exists() {
            std::fs::remove_dir_all(&wt_path)?;
            debug!("Removed worktree directory {}", wt_path.display());
        }

        // Delete branch
        if let Ok(mut branch) = repo.find_branch(&branch_name, BranchType::Local) {
            branch.delete()
                .map_err(|e| SurgeError::Git(format!("Failed to delete branch '{branch_name}': {e}")))?;
            info!("Deleted branch '{branch_name}'");
        }

        info!("Discarded worktree for '{spec_id}'");
        Ok(())
    }
```

**Step 2: Add tests**

Add to the `tests` module:

```rust
    #[test]
    fn test_commit_in_worktree() {
        let (_temp, mgr) = create_test_repo();
        let info = mgr.create_worktree("commit-test").unwrap();

        // Write a file in the worktree
        std::fs::write(info.path.join("new_file.txt"), "hello").unwrap();

        let oid = mgr.commit("commit-test", "Add new file").unwrap();
        assert!(!oid.is_zero());
    }

    #[test]
    fn test_diff() {
        let (_temp, mgr) = create_test_repo();
        let info = mgr.create_worktree("diff-test").unwrap();

        // Write a file and commit in worktree
        std::fs::write(info.path.join("diff_file.txt"), "diff content").unwrap();
        mgr.commit("diff-test", "Add diff file").unwrap();

        let diff = mgr.diff("diff-test").unwrap();
        assert!(diff.contains("diff_file.txt") || diff.contains("diff content"));
    }

    #[test]
    fn test_discard() {
        let (_temp, mgr) = create_test_repo();
        let info = mgr.create_worktree("discard-test").unwrap();
        assert!(info.path.exists());

        mgr.discard("discard-test").unwrap();
        assert!(!info.path.exists());

        // Should have no worktrees now
        let list = mgr.list_worktrees().unwrap();
        assert!(list.is_empty());
    }
```

**Step 3: Run tests**

Run: `cargo test -p surge-git`
Expected: 7 tests PASS

**Step 4: Commit**

```bash
git add crates/surge-git/src/worktree.rs
git commit -m "feat(git): add commit, diff, discard to GitManager"
```

---

### Task 4: GitManager — merge worktree into target branch

**Files:**
- Modify: `crates/surge-git/src/worktree.rs`

**Step 1: Add merge method**

```rust
    /// Merge worktree branch into the target branch (default: current HEAD branch).
    ///
    /// Performs a fast-forward merge if possible, otherwise creates a merge commit.
    pub fn merge(&self, spec_id: &str, target_branch: Option<&str>) -> Result<(), SurgeError> {
        let repo = self.open_repo()?;
        let branch_name = self.branch_name(spec_id);

        // Get the surge branch commit
        let surge_branch = repo.find_branch(&branch_name, BranchType::Local)
            .map_err(|e| SurgeError::Git(format!("Branch '{branch_name}' not found: {e}")))?;
        let surge_commit = surge_branch.get().peel_to_commit()
            .map_err(|e| SurgeError::Git(format!("Failed to get surge branch commit: {e}")))?;

        // Get the target branch (default: HEAD)
        let target_ref_name = if let Some(target) = target_branch {
            format!("refs/heads/{target}")
        } else {
            let head = repo.head()
                .map_err(|e| SurgeError::Git(format!("Failed to get HEAD: {e}")))?;
            head.name()
                .ok_or_else(|| SurgeError::Git("HEAD has no name".to_string()))?
                .to_string()
        };

        let mut target_ref = repo.find_reference(&target_ref_name)
            .map_err(|e| SurgeError::Git(format!("Target branch not found: {e}")))?;
        let target_commit = target_ref.peel_to_commit()
            .map_err(|e| SurgeError::Git(format!("Failed to get target commit: {e}")))?;

        // Check if fast-forward is possible
        let merge_base = repo.merge_base(target_commit.id(), surge_commit.id())
            .map_err(|e| SurgeError::Git(format!("Failed to find merge base: {e}")))?;

        if merge_base == target_commit.id() {
            // Fast-forward possible
            target_ref.set_target(surge_commit.id(), &format!("surge: merge {spec_id}"))
                .map_err(|e| SurgeError::Git(format!("Failed to fast-forward: {e}")))?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
                .map_err(|e| SurgeError::Git(format!("Failed to checkout: {e}")))?;
            info!("Fast-forward merged '{branch_name}' into target");
        } else {
            // Need a merge commit
            let sig = repo.signature()
                .or_else(|_| git2::Signature::now("Surge", "surge@localhost"))
                .map_err(|e| SurgeError::Git(format!("Failed to create signature: {e}")))?;

            let mut merge_index = repo.merge_commits(&target_commit, &surge_commit, None)
                .map_err(|e| SurgeError::Git(format!("Merge conflict: {e}")))?;

            if merge_index.has_conflicts() {
                return Err(SurgeError::Git(format!(
                    "Merge conflicts detected when merging '{branch_name}'. Resolve manually."
                )));
            }

            let tree_oid = merge_index.write_tree_to(&repo)
                .map_err(|e| SurgeError::Git(format!("Failed to write merge tree: {e}")))?;
            let tree = repo.find_tree(tree_oid)
                .map_err(|e| SurgeError::Git(format!("Failed to find merge tree: {e}")))?;

            let message = format!("Merge surge/{spec_id}");
            repo.commit(
                Some(&target_ref_name),
                &sig, &sig, &message, &tree,
                &[&target_commit, &surge_commit],
            ).map_err(|e| SurgeError::Git(format!("Failed to create merge commit: {e}")))?;

            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
                .map_err(|e| SurgeError::Git(format!("Failed to checkout: {e}")))?;
            info!("Merge committed '{branch_name}' into target");
        }

        Ok(())
    }
```

**Step 2: Add test**

```rust
    #[test]
    fn test_merge_fast_forward() {
        let (_temp, mgr) = create_test_repo();
        let info = mgr.create_worktree("merge-test").unwrap();

        // Make a change and commit in worktree
        std::fs::write(info.path.join("merged_file.txt"), "merged").unwrap();
        mgr.commit("merge-test", "Add merged file").unwrap();

        // Merge back
        mgr.merge("merge-test", None).unwrap();

        // Verify file exists in main repo
        assert!(mgr.repo_path().join("merged_file.txt").exists());
    }
```

**Step 3: Run tests**

Run: `cargo test -p surge-git`
Expected: 8 tests PASS

**Step 4: Commit**

```bash
git add crates/surge-git/src/worktree.rs
git commit -m "feat(git): add merge support to GitManager"
```

---

### Task 5: LifecycleManager — cleanup orphaned worktrees and stale branches

**Files:**
- Create: `crates/surge-git/src/cleanup.rs`

**Step 1: Write LifecycleManager**

```rust
//! Cleanup utilities for orphaned worktrees and stale branches.

use git2::{BranchType, Repository};
use std::path::PathBuf;
use surge_core::SurgeError;
use tracing::{debug, info};

use crate::worktree::GitManager;

/// Result of a cleanup operation.
#[derive(Debug, Default)]
pub struct CleanupReport {
    /// Worktrees that were removed.
    pub removed_worktrees: Vec<String>,
    /// Branches that were deleted.
    pub removed_branches: Vec<String>,
}

/// Manages cleanup of orphaned worktrees and stale branches.
pub struct LifecycleManager {
    git_manager: GitManager,
}

impl LifecycleManager {
    /// Create a new LifecycleManager.
    pub fn new(git_manager: GitManager) -> Self {
        Self { git_manager }
    }

    /// Find and remove orphaned worktrees (worktrees whose directories don't exist).
    pub fn cleanup_orphaned(&self) -> Result<CleanupReport, SurgeError> {
        let mut report = CleanupReport::default();
        let worktrees = self.git_manager.list_worktrees()?;

        for wt in worktrees {
            if !wt.exists_on_disk {
                info!("Cleaning up orphaned worktree: {}", wt.spec_id);
                self.git_manager.discard(&wt.spec_id)?;
                report.removed_worktrees.push(wt.spec_id);
            }
        }

        Ok(report)
    }

    /// Find and remove merged surge/* branches.
    pub fn cleanup_merged_branches(&self) -> Result<CleanupReport, SurgeError> {
        let repo = Repository::open(self.git_manager.repo_path())
            .map_err(|e| SurgeError::Git(format!("Failed to open repo: {e}")))?;

        let mut report = CleanupReport::default();

        // Get HEAD commit
        let head = repo.head()
            .map_err(|e| SurgeError::Git(format!("Failed to get HEAD: {e}")))?;
        let head_commit = head.peel_to_commit()
            .map_err(|e| SurgeError::Git(format!("Failed to get HEAD commit: {e}")))?;

        // Iterate all local branches
        let branches = repo.branches(Some(BranchType::Local))
            .map_err(|e| SurgeError::Git(format!("Failed to list branches: {e}")))?;

        let mut to_delete = vec![];
        for branch_result in branches {
            let (branch, _) = branch_result
                .map_err(|e| SurgeError::Git(format!("Failed to iterate branches: {e}")))?;

            let Some(name) = branch.name().ok().flatten() else { continue };
            if !name.starts_with("surge/") { continue }

            let branch_commit = branch.get().peel_to_commit()
                .map_err(|e| SurgeError::Git(format!("Failed to get branch commit: {e}")))?;

            // Check if branch is an ancestor of HEAD (i.e., merged)
            if repo.graph_descendant_of(head_commit.id(), branch_commit.id())
                .unwrap_or(false)
            {
                to_delete.push(name.to_string());
            }
        }

        for name in to_delete {
            debug!("Deleting merged branch: {name}");
            if let Ok(mut branch) = repo.find_branch(&name, BranchType::Local) {
                branch.delete()
                    .map_err(|e| SurgeError::Git(format!("Failed to delete branch '{name}': {e}")))?;
                report.removed_branches.push(name);
            }
        }

        Ok(report)
    }

    /// Run full cleanup — orphaned worktrees + merged branches.
    pub fn full_cleanup(&self) -> Result<CleanupReport, SurgeError> {
        let mut report = self.cleanup_orphaned()?;
        let branch_report = self.cleanup_merged_branches()?;
        report.removed_branches = branch_report.removed_branches;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn create_test_repo() -> (tempfile::TempDir, GitManager) {
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();

        Command::new("git").args(["init"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test"]).current_dir(repo_path).output().unwrap();
        std::fs::write(repo_path.join("README.md"), "# Test").unwrap();
        Command::new("git").args(["add", "."]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "initial"]).current_dir(repo_path).output().unwrap();

        let manager = GitManager::new(repo_path.to_path_buf()).unwrap();
        (temp_dir, manager)
    }

    #[test]
    fn test_cleanup_orphaned() {
        let (_temp, mgr) = create_test_repo();
        let info = mgr.create_worktree("orphan-test").unwrap();

        // Manually remove the worktree directory to simulate orphan
        std::fs::remove_dir_all(&info.path).unwrap();

        let lifecycle = LifecycleManager::new(mgr);
        let report = lifecycle.cleanup_orphaned().unwrap();
        assert_eq!(report.removed_worktrees.len(), 1);
        assert_eq!(report.removed_worktrees[0], "orphan-test");
    }

    #[test]
    fn test_cleanup_merged_branches() {
        let (_temp, mgr) = create_test_repo();
        let info = mgr.create_worktree("merged-branch").unwrap();

        // Make changes and commit
        std::fs::write(info.path.join("file.txt"), "content").unwrap();
        mgr.commit("merged-branch", "Add file").unwrap();

        // Merge into main
        mgr.merge("merged-branch", None).unwrap();

        // Discard worktree but branch still exists if discard only removes worktree dir
        // Actually discard removes both — so let's test differently:
        // After merge, the branch is an ancestor of HEAD, so cleanup should find it
        let lifecycle = LifecycleManager::new(mgr);
        let report = lifecycle.cleanup_merged_branches().unwrap();
        assert!(report.removed_branches.iter().any(|b| b == "surge/merged-branch"));
    }

    #[test]
    fn test_full_cleanup_no_orphans() {
        let (_temp, mgr) = create_test_repo();
        let lifecycle = LifecycleManager::new(mgr);
        let report = lifecycle.full_cleanup().unwrap();
        assert!(report.removed_worktrees.is_empty());
        assert!(report.removed_branches.is_empty());
    }
}
```

**Step 2: Update lib.rs exports**

Already done in scaffold — `pub use cleanup::LifecycleManager;`

**Step 3: Run tests**

Run: `cargo test -p surge-git`
Expected: 11 tests PASS

**Step 4: Commit**

```bash
git add crates/surge-git/src/cleanup.rs
git commit -m "feat(git): add LifecycleManager — cleanup orphaned worktrees and merged branches"
```

---

### Task 6: CLI git commands — diff, merge, discard, clean

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Add git-related commands to CLI**

Add top-level commands to the `Commands` enum:

```rust
    /// Show diff for a spec's worktree
    Diff {
        /// Spec ID
        spec_id: String,
    },

    /// Merge a spec's worktree into the current branch
    Merge {
        /// Spec ID
        spec_id: String,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Discard a spec's worktree and branch
    Discard {
        /// Spec ID
        spec_id: String,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Clean up orphaned worktrees and merged branches
    Clean {
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// List active worktrees
    Worktrees,
```

Add handlers:

```rust
        Commands::Diff { spec_id } => {
            let mgr = surge_git::GitManager::discover()?;
            let diff = mgr.diff(&spec_id)?;
            if diff.is_empty() {
                println!("No changes in worktree for '{spec_id}'");
            } else {
                println!("{diff}");
            }
        }
        Commands::Merge { spec_id, yes } => {
            if !yes {
                println!("⚡ Merge worktree for '{spec_id}' into current branch?");
                println!("   Run with -y to skip confirmation.");
                return Ok(());
            }

            let mgr = surge_git::GitManager::discover()?;
            mgr.merge(&spec_id, None)?;
            println!("✅ Merged '{spec_id}' into current branch");
        }
        Commands::Discard { spec_id, yes } => {
            if !yes {
                println!("⚡ Discard worktree and branch for '{spec_id}'?");
                println!("   This is irreversible. Run with -y to confirm.");
                return Ok(());
            }

            let mgr = surge_git::GitManager::discover()?;
            mgr.discard(&spec_id)?;
            println!("✅ Discarded worktree for '{spec_id}'");
        }
        Commands::Clean { yes } => {
            let mgr = surge_git::GitManager::discover()?;
            let lifecycle = surge_git::LifecycleManager::new(mgr);

            if !yes {
                // Show what would be cleaned
                println!("⚡ Cleanup preview (run with -y to execute):\n");
                // Just run the full report
                println!("   Run with -y to execute cleanup.");
                return Ok(());
            }

            let report = lifecycle.full_cleanup()?;

            if report.removed_worktrees.is_empty() && report.removed_branches.is_empty() {
                println!("✅ Nothing to clean up");
            } else {
                for wt in &report.removed_worktrees {
                    println!("  Removed worktree: {wt}");
                }
                for br in &report.removed_branches {
                    println!("  Deleted branch: {br}");
                }
                println!("\n✅ Cleanup complete");
            }
        }
        Commands::Worktrees => {
            let mgr = surge_git::GitManager::discover()?;
            let worktrees = mgr.list_worktrees()?;

            if worktrees.is_empty() {
                println!("No active worktrees.");
            } else {
                println!("⚡ Active worktrees:\n");
                for wt in &worktrees {
                    let status = if wt.exists_on_disk { "✅" } else { "❌ (missing)" };
                    println!("  {status} {} — {}", wt.spec_id, wt.branch);
                    println!("       {}", wt.path.display());
                }
            }
        }
```

**Step 2: Verify compilation**

Run: `cargo check -p surge-cli`

**Step 3: Commit**

```bash
git add crates/surge-cli/src/main.rs
git commit -m "feat(cli): add git commands — diff, merge, discard, clean, worktrees"
```

---

### Task 7: Final verification

**Step 1: Run full test suite**

Run: `cargo test --workspace`

**Step 2: Run clippy**

Run: `cargo clippy --workspace`

**Step 3: Commit if fixes needed**

```bash
git add -A
git commit -m "test: Phase 2 final verification"
```

---

## Dependency Graph

```
Task 1 (scaffold) → Task 2 (create/list) → Task 3 (commit/diff/discard) → Task 4 (merge) → Task 5 (cleanup) → Task 6 (CLI) → Task 7 (verify)
```

Linear chain — each builds on the previous.
