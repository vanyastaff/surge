//! Immutable working-tree checkpoints without changing HEAD or the user's index.

use std::path::Path;

use git2::{Index, IndexAddOption, Oid, Repository, Signature};
use surge_core::RunId;

use crate::GitError;

/// Durable locator for a checkpoint in the repository's shared object store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct WorkspaceCheckpoint {
    pub git_common_dir: std::path::PathBuf,
    pub commit: String,
}

/// Prepare an isolated run worktree at the recorded commit.
/// Existing or conflicting destinations are never overwritten.
pub fn restore(
    checkpoint: &WorkspaceCheckpoint,
    run: RunId,
    destination: std::path::PathBuf,
) -> Result<crate::RunWorktreeInfo, GitError> {
    use crate::run_worktree::{PinnedRunBase, ReconcilePhase, RunWorktreeSpec};
    let repo = Repository::open(&checkpoint.git_common_dir)?;
    let root = repo
        .workdir()
        .ok_or_else(|| git2::Error::from_str("checkpoint repository has no working directory"))?
        .canonicalize()?;
    let base = PinnedRunBase::new(
        root.clone(),
        checkpoint.git_common_dir.clone(),
        Oid::from_str(&checkpoint.commit)?,
    )?;
    let spec = RunWorktreeSpec::new(run, base, destination)?;
    crate::GitManager::new(root)?.prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution)
}

/// Capture a checkpoint with repository identity for later reconstruction.
pub fn capture_record(
    worktree: &Path,
    run: RunId,
    seq: u64,
) -> Result<Option<WorkspaceCheckpoint>, GitError> {
    let Some(commit) = capture(worktree, run, seq)? else {
        return Ok(None);
    };
    let repo = Repository::open(worktree)?;
    Ok(Some(WorkspaceCheckpoint {
        git_common_dir: repo.commondir().canonicalize()?,
        commit: commit.to_string(),
    }))
}

/// Capture tracked and non-ignored files at an engine boundary.
///
/// The commit is retained by an immutable `refs/surge/checkpoints/` reference.
/// Ignored untracked files (build output, local credentials) are not captured.
/// Returns `None` for a directory that is not itself a Git worktree root.
/// Nested repositories and submodules require a separate ownership model and
/// are rejected instead of producing an incomplete checkpoint.
pub fn capture(worktree: &Path, run: RunId, seq: u64) -> Result<Option<Oid>, GitError> {
    let repo = match Repository::open(worktree) {
        Ok(repo) => repo,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Some(root) = repo.workdir() else {
        return Err(git2::Error::from_str("cannot checkpoint a bare repository").into());
    };
    if root.canonicalize()? != worktree.canonicalize()? {
        return Ok(None);
    }
    let original = repo.index()?;
    if original.has_conflicts() {
        return Err(git2::Error::from_str("cannot checkpoint unresolved index conflicts").into());
    }
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            None
        },
        Err(error) => return Err(error.into()),
    };
    let mut index = Index::new()?;
    repo.set_index(&mut index)?;
    if let Some(parent) = &parent {
        index.read_tree(&parent.tree()?)?;
    }
    for entry in original.iter() {
        index.add(&entry)?;
    }
    index.update_all(["*"], None)?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    if index.iter().any(|entry| entry.mode == 0o160_000) {
        return Err(
            git2::Error::from_str("cannot checkpoint nested repositories or submodules").into(),
        );
    }
    let tree_id = index.write_tree_to(&repo)?;
    let reference = format!("refs/surge/checkpoints/{run}/{seq}");
    match repo.find_reference(&reference) {
        Ok(existing) => {
            let commit = existing.peel_to_commit()?;
            if commit.tree_id() != tree_id {
                return Err(git2::Error::from_str(
                    "checkpoint sequence already owns different file contents",
                )
                .into());
            }
            return Ok(Some(commit.id()));
        },
        Err(error) if error.code() == git2::ErrorCode::NotFound => {},
        Err(error) => return Err(error.into()),
    }
    let tree = repo.find_tree(tree_id)?;
    let signature = Signature::now("Surge checkpoint", "checkpoint@surge.local")?;
    let parents: Vec<_> = parent.iter().collect();
    let commit = repo.commit(
        None,
        &signature,
        &signature,
        "Surge stage checkpoint",
        &tree,
        &parents,
    )?;
    repo.reference(&reference, commit, false, "retain Surge stage checkpoint")?;
    Ok(Some(commit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_files_and_deletions_without_mutating_head_or_index() {
        let (_dir, root) = crate::test_helpers::init_test_repo();
        let repo = Repository::open(&root).unwrap();
        let head = repo.head().unwrap().target().unwrap();
        std::fs::write(root.join("staged.txt"), "staged version").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged.txt")).unwrap();
        index.write().unwrap();
        let index_before = std::fs::read(repo.path().join("index")).unwrap();
        std::fs::write(root.join("staged.txt"), "working version").unwrap();
        std::fs::remove_file(root.join("README.md")).unwrap();
        std::fs::write(root.join("new.txt"), "new file").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(root.join("ignored.txt"), "private local file").unwrap();

        let run = RunId::new();
        let checkpoint = capture(&root, run, 4).unwrap().unwrap();
        let tree = repo.find_commit(checkpoint).unwrap().tree().unwrap();
        assert!(tree.get_path(Path::new("README.md")).is_err());
        assert!(tree.get_path(Path::new("ignored.txt")).is_err());
        for (name, content) in [
            ("staged.txt", b"working version".as_slice()),
            ("new.txt", b"new file"),
        ] {
            let entry = tree.get_path(Path::new(name)).unwrap();
            assert_eq!(repo.find_blob(entry.id()).unwrap().content(), content);
        }
        assert_eq!(repo.head().unwrap().target(), Some(head));
        assert_eq!(
            std::fs::read(repo.path().join("index")).unwrap(),
            index_before
        );
        assert_eq!(capture(&root, run, 4).unwrap(), Some(checkpoint));
        std::fs::write(root.join("new.txt"), "later edit").unwrap();
        assert!(capture(&root, run, 4).is_err());
        assert_eq!(
            repo.find_reference(&format!("refs/surge/checkpoints/{run}/4"))
                .unwrap()
                .target(),
            Some(checkpoint)
        );
    }

    #[cfg(unix)]
    #[test]
    fn checkpoint_preserves_executable_mode_and_symlink_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (_dir, root) = crate::test_helpers::init_test_repo();
        std::fs::write(root.join("run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        symlink("run.sh", root.join("run-link")).unwrap();
        let checkpoint = capture(&root, RunId::new(), 1).unwrap().unwrap();
        let repo = Repository::open(&root).unwrap();
        let tree = repo.find_commit(checkpoint).unwrap().tree().unwrap();
        assert_eq!(
            tree.get_path(Path::new("run.sh")).unwrap().filemode(),
            0o100_755
        );
        let link = tree.get_path(Path::new("run-link")).unwrap();
        assert_eq!(link.filemode(), 0o120_000);
        assert_eq!(repo.find_blob(link.id()).unwrap().content(), b"run.sh");
    }

    #[test]
    fn restored_checkpoint_is_independent_of_later_parent_edits() {
        let (_dir, root) = crate::test_helpers::init_test_repo();
        std::fs::write(root.join("app.txt"), "checkpoint version").unwrap();
        std::fs::remove_file(root.join("README.md")).unwrap();
        let checkpoint = capture_record(&root, RunId::new(), 10).unwrap().unwrap();
        std::fs::write(root.join("app.txt"), "later parent version").unwrap();
        std::fs::write(root.join("README.md"), "recreated later").unwrap();
        let children = tempfile::tempdir().unwrap();
        let destination = children.path().canonicalize().unwrap().join("child");
        let child = RunId::new();
        restore(&checkpoint, child, destination.clone()).unwrap();
        assert_eq!(
            std::fs::read(destination.join("app.txt")).unwrap(),
            b"checkpoint version"
        );
        assert!(!destination.join("README.md").exists());
        std::fs::write(destination.join("app.txt"), "child edit").unwrap();
        assert_eq!(
            std::fs::read(root.join("app.txt")).unwrap(),
            b"later parent version"
        );
        assert!(restore(&checkpoint, child, destination.clone()).is_err());
        assert_eq!(
            std::fs::read(destination.join("app.txt")).unwrap(),
            b"child edit"
        );
    }

    #[test]
    fn ordinary_directory_has_no_git_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(capture(dir.path(), RunId::new(), 1).unwrap(), None);
    }
}
