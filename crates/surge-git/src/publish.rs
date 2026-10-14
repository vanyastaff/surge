//! Publishing a run's branch (#127): commit what the agent left, read the
//! branch head, and push it to the project's remote.

use std::path::Path;

use git2::{Cred, PushOptions, RemoteCallbacks, Repository};

use crate::worktree::GitError;

/// The checked-out branch of `checkout` and the commit it points at.
///
/// # Errors
/// A detached `HEAD` is reported as [`GitError::BranchNotFound`].
pub fn branch_head(checkout: &Path) -> Result<(String, git2::Oid), GitError> {
    let repo = Repository::open(checkout)?;
    let head = repo.head()?;
    if !head.is_branch() {
        return Err(GitError::BranchNotFound("HEAD is detached".into()));
    }
    let branch = head
        .shorthand()
        .map_err(|_| GitError::BranchNotFound("HEAD has no readable name".into()))?
        .to_owned();
    Ok((branch, head.peel_to_commit()?.id()))
}

/// The commit `checkout`'s `HEAD` points at (branch or detached).
///
/// # Errors
/// Git failures, including an unborn `HEAD`.
pub fn head_commit(checkout: &Path) -> Result<git2::Oid, GitError> {
    Ok(Repository::open(checkout)?.head()?.peel_to_commit()?.id())
}

/// Paths Surge itself writes into a run worktree; never part of the work.
pub const SURGE_OWNED_PATHS: &[&str] = &[".surge", "pr.md"];

/// Commit everything the run changed in `checkout` except `excluded` paths
/// (and their contents), on the checked-out branch. `None` when there is
/// nothing to commit.
///
/// # Errors
/// Git failures.
pub fn commit_changes(
    checkout: &Path,
    message: &str,
    excluded: &[&str],
) -> Result<Option<git2::Oid>, GitError> {
    let repo = Repository::open(checkout)?;
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    // Deletions too: a file the agent removed leaves the index.
    index.update_all(["*"].iter(), None)?;
    for path in excluded {
        index.remove_all([*path].iter(), None)?;
    }
    index.write()?;
    let head = repo.head()?.peel_to_commit()?;
    let diff = repo.diff_tree_to_index(Some(&head.tree()?), Some(&index), None)?;
    if diff.stats()?.files_changed() == 0 {
        return Ok(None);
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("Surge", "surge@localhost"))?;
    let oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &[&head],
    )?;
    Ok(Some(oid))
}

/// Push `branch` of `checkout` to `remote` (same name on both sides).
///
/// `token`, when given, authenticates HTTPS remotes the way GitHub expects a
/// personal access token (`x-access-token` user). It never appears in errors.
///
/// # Errors
/// Git failures, and [`GitError::PushRejected`] when the remote refuses the
/// update (for example a non-fast-forward or a protected branch).
pub fn push_branch(
    checkout: &Path,
    remote: &str,
    branch: &str,
    token: Option<&str>,
) -> Result<(), GitError> {
    let repo = Repository::open(checkout)?;
    let mut remote = repo.find_remote(remote)?;
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let rejected = std::cell::RefCell::new(None::<(String, String)>);
    {
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(|_url, username, allowed| match token {
            Some(token) if allowed.is_user_pass_plaintext() => {
                Cred::userpass_plaintext("x-access-token", token)
            },
            _ => Cred::default().or_else(|_| Cred::username(username.unwrap_or("git"))),
        });
        callbacks.push_update_reference(|reference, status| {
            if let Some(reason) = status {
                *rejected.borrow_mut() = Some((reference.to_owned(), reason.to_owned()));
            }
            Ok(())
        });
        let mut options = PushOptions::new();
        options.remote_callbacks(callbacks);
        remote.push(&[refspec.as_str()], Some(&mut options))?;
    }
    if let Some((reference, reason)) = rejected.into_inner() {
        return Err(GitError::PushRejected { reference, reason });
    }
    tracing::info!(branch, "pushed run branch");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(repo: &Repository, file: &str, message: &str) -> git2::Oid {
        let workdir = repo.workdir().unwrap().to_path_buf();
        std::fs::write(workdir.join(file), message).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(file)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        let parents: Vec<git2::Commit<'_>> = repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok())
            .into_iter()
            .collect();
        let parents: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .unwrap()
    }

    #[test]
    fn the_run_work_is_committed_without_surge_owned_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let base = commit(&repo, "kept.txt", "base");
        commit(&repo, "removed.txt", "base-2");
        let checkout = repo.workdir().unwrap().to_path_buf();
        assert_eq!(
            commit_changes(&checkout, "noop", SURGE_OWNED_PATHS).unwrap(),
            None
        );

        std::fs::write(checkout.join("feature.rs"), "fn main() {}").unwrap();
        std::fs::remove_file(checkout.join("removed.txt")).unwrap();
        std::fs::create_dir_all(checkout.join(".surge")).unwrap();
        std::fs::write(checkout.join(".surge/user_prompt.txt"), "prompt").unwrap();
        std::fs::write(checkout.join("pr.md"), "title\n---\nbody").unwrap();
        let oid = commit_changes(&checkout, "work", SURGE_OWNED_PATHS)
            .unwrap()
            .expect("the run changed files");

        let tree = repo.find_commit(oid).unwrap().tree().unwrap();
        assert!(tree.get_path(Path::new("feature.rs")).is_ok());
        assert!(tree.get_path(Path::new("kept.txt")).is_ok());
        assert!(
            tree.get_path(Path::new("removed.txt")).is_err(),
            "deletions are committed"
        );
        assert!(tree.get_path(Path::new(".surge/user_prompt.txt")).is_err());
        assert!(tree.get_path(Path::new("pr.md")).is_err());
        assert_ne!(oid, base);
    }

    #[test]
    fn a_branch_is_pushed_to_the_remote_and_its_head_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let remote_path = dir.path().join("remote.git");
        Repository::init_bare(&remote_path).unwrap();
        let local = Repository::init(dir.path().join("local")).unwrap();
        commit(&local, "a.txt", "base");
        let base = local.head().unwrap().peel_to_commit().unwrap();
        local.branch("surge/run-abc", &base, false).unwrap();
        local.set_head("refs/heads/surge/run-abc").unwrap();
        let tip = commit(&local, "b.txt", "work");
        local
            .remote("origin", remote_path.to_str().unwrap())
            .unwrap();

        let checkout = local.workdir().unwrap();
        assert_eq!(
            branch_head(checkout).unwrap(),
            ("surge/run-abc".to_owned(), tip)
        );
        push_branch(checkout, "origin", "surge/run-abc", None).unwrap();

        let remote = Repository::open_bare(&remote_path).unwrap();
        assert_eq!(
            remote
                .find_reference("refs/heads/surge/run-abc")
                .unwrap()
                .target(),
            Some(tip)
        );
        // Pushing the same head again is a no-op, not a failure.
        push_branch(checkout, "origin", "surge/run-abc", None).unwrap();
    }

    #[test]
    fn a_non_fast_forward_push_is_reported_as_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let remote_path = dir.path().join("remote.git");
        Repository::init_bare(&remote_path).unwrap();
        let first = Repository::init(dir.path().join("first")).unwrap();
        commit(&first, "a.txt", "base");
        first
            .remote("origin", remote_path.to_str().unwrap())
            .unwrap();
        let branch = first.head().unwrap().shorthand().unwrap().to_owned();
        push_branch(first.workdir().unwrap(), "origin", &branch, None).unwrap();

        // An unrelated history on the same branch name cannot fast-forward.
        let second = Repository::init(dir.path().join("second")).unwrap();
        commit(&second, "z.txt", "unrelated");
        second
            .remote("origin", remote_path.to_str().unwrap())
            .unwrap();
        let error = push_branch(second.workdir().unwrap(), "origin", &branch, None).unwrap_err();
        assert!(
            matches!(error, GitError::PushRejected { .. } | GitError::Git2(_)),
            "{error}"
        );
    }
}
