//! Per-run worktree management — extension of `GitManager` for the new
//! run-based workflow alongside the legacy spec-based methods.

use std::path::PathBuf;

use surge_core::RunId;

/// Where to place per-run worktrees on disk.
#[derive(Debug, Clone)]
pub enum WorktreeLocation {
    /// Default: `<repo_parent>/.surge-worktrees/<short_id>/`. Sibling-outside-repo.
    Sibling,
    /// `~/.surge/runs/<run_id>/worktree/`. Centralized.
    Central,
    /// Explicit absolute path; final dir is `<path>/<short_id>`.
    Custom(PathBuf),
}

impl Default for WorktreeLocation {
    fn default() -> Self {
        WorktreeLocation::Sibling
    }
}

/// Info returned by `GitManager::create_run_worktree`.
#[derive(Debug, Clone)]
pub struct RunWorktreeInfo {
    pub run_id: RunId,
    pub path: PathBuf,
    pub branch: String,
    pub exists_on_disk: bool,
}

/// A worktree whose recorded path no longer exists on disk.
#[derive(Debug, Clone)]
pub struct OrphanedWorktree {
    pub name: String,
    pub recorded_path: PathBuf,
}

/// Branch name format for run-based worktrees: `surge/run-<short_id>`.
#[must_use]
pub fn run_branch_name(run_id: &RunId) -> String {
    format!("surge/run-{}", run_id.short())
}

/// Resolve the worktree directory path for a given run + location strategy.
#[must_use]
pub fn resolve_path(
    repo_path: &std::path::Path,
    run_id: &RunId,
    location: &WorktreeLocation,
) -> PathBuf {
    let short = run_id.short();
    match location {
        WorktreeLocation::Sibling => {
            let parent = repo_path.parent().unwrap_or(repo_path);
            parent.join(".surge-worktrees").join(&short)
        },
        WorktreeLocation::Central => {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
            home.join(".surge")
                .join("runs")
                .join(run_id.to_string())
                .join("worktree")
        },
        WorktreeLocation::Custom(p) => p.join(&short),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn sibling_path_is_under_repo_parent() {
        let id = RunId::new();
        let p = resolve_path(
            Path::new("/projects/myrepo"),
            &id,
            &WorktreeLocation::Sibling,
        );
        // On Windows test runners the path uses backslashes; check substrings instead.
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.contains("/projects/.surge-worktrees/"));
        assert!(s.contains(&id.short()));
    }

    #[test]
    fn central_path_under_home() {
        let id = RunId::new();
        let p = resolve_path(
            Path::new("/projects/myrepo"),
            &id,
            &WorktreeLocation::Central,
        );
        let s = p.to_string_lossy();
        assert!(s.contains(".surge"));
        assert!(s.contains(&id.to_string()));
    }

    #[test]
    fn branch_name_format() {
        let id = RunId::new();
        let b = run_branch_name(&id);
        assert!(b.starts_with("surge/run-"));
        assert_eq!(b.len(), "surge/run-".len() + 12);
    }

    #[test]
    fn custom_path_appends_short_id() {
        let id = RunId::new();
        let custom = PathBuf::from("/some/abs/path");
        let p = resolve_path(
            Path::new("/anywhere"),
            &id,
            &WorktreeLocation::Custom(custom),
        );
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.starts_with("/some/abs/path/"));
        assert!(s.ends_with(&id.short()));
    }
}
