//! Git worktree management for Surge — isolated workspaces per task.

pub mod cleanup;
pub mod worktree;

pub use cleanup::LifecycleManager;
pub use worktree::{GitError, GitManager, WorktreeInfo};
