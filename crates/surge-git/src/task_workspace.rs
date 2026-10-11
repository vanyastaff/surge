//! Persistent task workspace intent reuses pinned run-worktree ownership.
use crate::{
    GitError, GitManager,
    run_worktree::{PinnedRunBase, ReconcilePhase, RunWorktreeSpec},
};
use std::path::Path;
use surge_core::{RunId, work_item::WorkItemWorkspace};

/// Capture clean committed source identity before accepting workspace creation.
/// Dirty source checkouts fail without modifying their files or index.
pub fn plan(
    project: &Path,
    root: &Path,
    creation_run: RunId,
) -> Result<WorkItemWorkspace, GitError> {
    let manager = GitManager::new(project.to_path_buf())?;
    let base = manager.capture_clean_base()?;
    std::fs::create_dir_all(root)?;
    let path = root.canonicalize()?.join(creation_run.to_string());
    let spec = RunWorktreeSpec::new(creation_run, base, path)?;
    Ok(WorkItemWorkspace {
        repository: spec.base().git_common_dir().to_path_buf(),
        checkout: spec.base().repository().to_path_buf(),
        path: spec.path().to_path_buf(),
        ownership: creation_run.to_string(),
        branch: spec.branch(),
        base_commit: spec.base().commit().to_string(),
    })
}
/// Capture a clean original base for an explicit new workspace location.
/// The later preparation still refuses any foreign existing checkout.
pub fn plan_at_path(
    project: &Path,
    path: &Path,
    creation_run: RunId,
) -> Result<WorkItemWorkspace, GitError> {
    let manager = GitManager::new(project.to_path_buf())?;
    let base = manager.capture_clean_base()?;
    let parent = path
        .parent()
        .ok_or_else(|| git2::Error::from_str("workspace parent missing"))?;
    let name = path
        .file_name()
        .ok_or_else(|| git2::Error::from_str("workspace name missing"))?;
    let path = parent.canonicalize()?.join(name);
    let spec = RunWorktreeSpec::new(creation_run, base, path)?;
    Ok(WorkItemWorkspace {
        repository: spec.base().git_common_dir().to_path_buf(),
        checkout: spec.base().repository().to_path_buf(),
        path: spec.path().to_path_buf(),
        ownership: creation_run.to_string(),
        branch: spec.branch(),
        base_commit: spec.base().commit().to_string(),
    })
}
/// Prepare/reconcile the exact original registered owner; never reset, adopt or remove.
pub fn prepare(intent: &WorkItemWorkspace, phase: ReconcilePhase) -> Result<(), GitError> {
    let owner: RunId = intent
        .ownership
        .parse()
        .map_err(|_| git2::Error::from_str("invalid task workspace owner"))?;
    let commit = git2::Oid::from_str(&intent.base_commit)?;
    let base = PinnedRunBase::new(intent.checkout.clone(), intent.repository.clone(), commit)?;
    let spec = RunWorktreeSpec::new(owner, base, intent.path.clone())?;
    if spec.branch() != intent.branch {
        return Err(git2::Error::from_str(
            "task workspace branch differs from its original ownership",
        )
        .into());
    }
    GitManager::new(intent.checkout.clone())?.prepare_run_worktree(&spec, phase)?;
    Ok(())
}

/// Confirm the PR target against explicitly configured GitHub remotes of the pinned project.
/// Origin and upstream/fork remotes are equally eligible; unknown targets fail closed.
pub fn validate_pr_repository(
    intent: &WorkItemWorkspace,
    pr: &surge_core::work_item::WorkItemPr,
) -> Result<(), GitError> {
    pr.validate()
        .map_err(|_| git2::Error::from_str("invalid GitHub PR identity"))?;
    let repository = git2::Repository::open(&intent.checkout)?;
    if repository.commondir().canonicalize()? != intent.repository
        || repository
            .workdir()
            .ok_or_else(|| git2::Error::from_str("task project has no checkout"))?
            .canonicalize()?
            != intent.checkout
    {
        return Err(git2::Error::from_str(
            "PR association project differs from its pinned Git identity",
        )
        .into());
    }
    let remotes = repository.remotes()?;
    let mut known = false;
    for name in remotes.iter() {
        let Some(name) = name? else { continue };
        let remote = repository.find_remote(name)?;
        if let Some(identity) = github_remote_identity(remote.url()?) {
            known = true;
            if identity.eq_ignore_ascii_case(&pr.repository) {
                return Ok(());
            }
        }
    }
    Err(git2::Error::from_str(if known {
        "PR target is not mapped by any configured project GitHub remote"
    } else {
        "PR association requires an explicit configured project GitHub remote"
    })
    .into())
}

fn github_remote_identity(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let path = lower
        .strip_prefix("https://github.com/")
        .or_else(|| lower.strip_prefix("git@github.com:"))
        .or_else(|| lower.strip_prefix("ssh://git@github.com/"))
        .or_else(|| lower.strip_prefix("ssh://git@github.com:22/"))?;
    let path = path
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_end_matches('/'));
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
    {
        return None;
    }
    Some(path.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repository(path: &Path) -> git2::Repository {
        let repo = git2::Repository::init(path).unwrap();
        std::fs::write(path.join("tracked"), "base").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("tracked")).unwrap();
        index.write().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let signature = git2::Signature::now("Fixture", "fixture@example.com").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
            .unwrap();
        drop(tree);
        repo
    }
    #[test]
    fn persistent_workspace_keeps_original_owner_and_dirty_files_across_attempts() {
        let project = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let repo = repository(project.path());
        let head = repo.head().unwrap().target().unwrap();
        let index = std::fs::read(project.path().join(".git/index")).unwrap();
        let intent = plan(project.path(), root.path(), RunId::new()).unwrap();
        prepare(&intent, ReconcilePhase::BeforeExecution).unwrap();
        std::fs::write(intent.path.join("tracked"), "task changes").unwrap();
        std::fs::write(intent.path.join("untracked"), "keep").unwrap();
        prepare(&intent, ReconcilePhase::AfterExecution).unwrap();
        assert_eq!(
            std::fs::read_to_string(intent.path.join("tracked")).unwrap(),
            "task changes"
        );
        assert_eq!(
            std::fs::read_to_string(intent.path.join("untracked")).unwrap(),
            "keep"
        );
        assert_eq!(repo.head().unwrap().target().unwrap(), head);
        assert_eq!(
            std::fs::read(project.path().join(".git/index")).unwrap(),
            index
        );
    }
    #[test]
    fn dirty_source_and_missing_or_substituted_workspace_fail_without_cleanup() {
        let project = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        repository(project.path());
        std::fs::write(project.path().join("user-untracked"), "preserve").unwrap();
        assert!(plan(project.path(), root.path(), RunId::new()).is_err());
        assert_eq!(
            std::fs::read_to_string(project.path().join("user-untracked")).unwrap(),
            "preserve"
        );
        std::fs::remove_file(project.path().join("user-untracked")).unwrap();
        let intent = plan(project.path(), root.path(), RunId::new()).unwrap();
        assert!(prepare(&intent, ReconcilePhase::AfterExecution).is_err());
        repository(&intent.path);
        assert!(prepare(&intent, ReconcilePhase::BeforeExecution).is_err());
        assert!(intent.path.join("tracked").exists());
    }
}
