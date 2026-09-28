//! Per-run worktree management — extension of `GitManager` for the new
//! run-based workflow alongside the legacy spec-based methods.

use std::path::PathBuf;

use surge_core::RunId;

use crate::{GitError, GitManager};
use git2::{Oid, Repository};
use std::path::{Component, Path};

/// Immutable repository identity captured before an operation is accepted.
/// Rehydration validates syntax only; preparation checks the actual repository.
#[derive(Debug, Clone)]
pub struct PinnedRunBase {
    repository: PathBuf,
    git_common_dir: PathBuf,
    commit: Oid,
}

impl PinnedRunBase {
    pub fn new(
        repository: PathBuf,
        git_common_dir: PathBuf,
        commit: Oid,
    ) -> Result<Self, GitError> {
        validate_absolute_path(&repository)?;
        validate_absolute_path(&git_common_dir)?;
        Ok(Self {
            repository,
            git_common_dir,
            commit,
        })
    }

    pub fn repository(&self) -> &Path {
        &self.repository
    }
    pub fn git_common_dir(&self) -> &Path {
        &self.git_common_dir
    }
    pub fn commit(&self) -> Oid {
        self.commit
    }
}

/// Pinned checkout intent. Persist these fields before invoking preparation.
#[derive(Debug, Clone)]
pub struct RunWorktreeSpec {
    run_id: RunId,
    base: PinnedRunBase,
    path: PathBuf,
}

impl RunWorktreeSpec {
    pub fn new(run_id: RunId, base: PinnedRunBase, path: PathBuf) -> Result<Self, GitError> {
        validate_absolute_path(&path)?;
        if path.starts_with(&base.repository)
            || base.repository.starts_with(&path)
            || path.starts_with(&base.git_common_dir)
            || base.git_common_dir.starts_with(&path)
        {
            return Err(conflict(&path, WorktreeConflict::InvalidPath));
        }
        Ok(Self { run_id, base, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn base(&self) -> &PinnedRunBase {
        &self.base
    }
    pub fn run_id(&self) -> RunId {
        self.run_id
    }
    pub fn branch(&self) -> String {
        run_branch_name(&self.run_id)
    }

    fn creation_message(&self) -> Result<String, GitError> {
        let bytes = serde_json::to_vec(&(
            1_u32,
            self.run_id.to_string(),
            &self.base.repository,
            &self.base.git_common_dir,
            self.base.commit.to_string(),
            &self.path,
            self.branch(),
        ))?;
        Ok(format!(
            "surge-worktree-v1 {} {}",
            self.run_id,
            surge_core::ContentHash::compute(&bytes)
        ))
    }
}

/// The caller obtains this phase from its durable execution journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcilePhase {
    /// A run has not executed: checkout must still be pristine at the pinned base.
    BeforeExecution,
    /// Execution may have edited/committed files: verify identity, never reset.
    AfterExecution,
}

/// Conflicts are not permission to reset, prune, delete, or adopt foreign content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeConflict {
    InvalidPath,
    SymbolicLink,
    RepositoryMismatch,
    OccupiedPath,
    MissingAfterExecution,
    RegistrationMismatch,
    BranchMismatch,
    MissingCreationEvidence,
    CreationLoggingDisabled,
    CreationLoggingUnsupported,
    ModifiedBeforeExecution,
}

impl std::fmt::Display for WorktreeConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreationLoggingDisabled | Self::CreationLoggingUnsupported => formatter.write_str(
                "recoverable branch creation requires core.logAllRefUpdates=true; the current setting is disabled or unsupported",
            ),
            _ => write!(formatter, "{self:?}"),
        }
    }
}

fn conflict(path: &Path, reason: WorktreeConflict) -> GitError {
    GitError::WorktreeConflict {
        path: path.into(),
        reason,
    }
}

fn validate_absolute_path(path: &Path) -> Result<(), GitError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(conflict(path, WorktreeConflict::InvalidPath));
    }
    Ok(())
}

fn reject_symlinks(path: &Path) -> Result<(), GitError> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(conflict(ancestor, WorktreeConflict::SymbolicLink));
            },
            Ok(_) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn is_dirty(repo: &Repository) -> Result<bool, GitError> {
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    Ok(!repo.statuses(Some(&mut options))?.is_empty())
}

impl GitManager {
    /// Capture a committed clean checkout, including untracked-file checks.
    /// Ignored files are neither rejected nor copied into a new checkout.
    pub fn capture_clean_base(&self) -> Result<PinnedRunBase, GitError> {
        let repo = Repository::open(self.repo_path())?;
        let path = repo
            .workdir()
            .ok_or_else(|| conflict(self.repo_path(), WorktreeConflict::RepositoryMismatch))?;
        let head = repo.head().map_err(|error| match error.code() {
            git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound => GitError::EmptyRepository,
            _ => error.into(),
        })?;
        let commit = head.peel_to_commit()?.id();
        if is_dirty(&repo)? {
            return Err(GitError::DirtyRepository);
        }
        PinnedRunBase::new(
            path.canonicalize()?,
            repo.commondir().canonicalize()?,
            commit,
        )
    }

    /// Reconcile a pinned checkout without modifying conflicting content.
    ///
    /// Branch creation records the full descriptor digest in the same reference
    /// operation's reflog entry. Missing/expired reflogs fail closed. This is
    /// Creation requires boolean `core.logAllRefUpdates=true` (or its non-bare
    /// default). Disabled or unsupported values, including Git's `always`, are
    /// refused before branch creation; set the option to `true` to enable this
    /// operation. The configuration is never changed here. This is
    /// local consistency evidence, not authentication against a Git-directory
    /// writer. The daemon must serialize calls for an operation. Concurrent
    /// external filesystem mutation is not made atomic by this synchronous API.
    pub fn prepare_run_worktree(
        &self,
        spec: &RunWorktreeSpec,
        phase: ReconcilePhase,
    ) -> Result<RunWorktreeInfo, GitError> {
        self.prepare_pinned_inner(spec, phase, || Ok(()))
    }

    fn prepare_pinned_inner(
        &self,
        spec: &RunWorktreeSpec,
        phase: ReconcilePhase,
        after_branch: impl FnOnce() -> Result<(), GitError>,
    ) -> Result<RunWorktreeInfo, GitError> {
        let repo = Repository::open(self.repo_path())?;
        verify_repository(&repo, spec)?;
        reject_symlinks(&spec.path)?;
        repo.find_commit(spec.base.commit)?;
        match repo.find_worktree(&spec.run_id.short()) {
            Ok(worktree) => {
                verify_checkout(&repo, &worktree, spec, phase)?;
                return Ok(prepared_info(spec));
            },
            Err(error) if error.code() == git2::ErrorCode::NotFound => {},
            Err(error) => return Err(error.into()),
        }
        if phase == ReconcilePhase::AfterExecution {
            return Err(conflict(
                &spec.path,
                WorktreeConflict::MissingAfterExecution,
            ));
        }
        match std::fs::symlink_metadata(&spec.path) {
            Ok(_) => return Err(conflict(&spec.path, WorktreeConflict::OccupiedPath)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        }
        let reference_name = format!("refs/heads/{}", spec.branch());
        let reference = match repo.find_reference(&reference_name) {
            Ok(reference) => {
                verify_creation(&repo, spec)?;
                if reference.target() != Some(spec.base.commit) {
                    return Err(conflict(&spec.path, WorktreeConflict::BranchMismatch));
                }
                reference
            },
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                // libgit2 does not append even an explicitly ensured log when
                // core.logAllRefUpdates=false. Refuse before creating a branch.
                match repo.config()?.get_bool("core.logallrefupdates") {
                    Ok(false) => {
                        return Err(conflict(
                            &spec.path,
                            WorktreeConflict::CreationLoggingDisabled,
                        ));
                    },
                    Ok(true) => {},
                    // libgit2 defaults to logging local branches in non-bare
                    // repositories; verify_repository already requires a workdir.
                    Err(error) if error.code() == git2::ErrorCode::NotFound => {},
                    Err(_) => {
                        return Err(conflict(
                            &spec.path,
                            WorktreeConflict::CreationLoggingUnsupported,
                        ));
                    },
                }
                // The marker is written by reference creation, never a later append.
                repo.reference_ensure_log(&reference_name)?;
                let reference = repo.reference(
                    &reference_name,
                    spec.base.commit,
                    false,
                    &spec.creation_message()?,
                )?;
                verify_creation(&repo, spec)?;
                reference
            },
            Err(error) => return Err(error.into()),
        };
        after_branch()?;
        let parent = spec
            .path
            .parent()
            .ok_or_else(|| conflict(&spec.path, WorktreeConflict::InvalidPath))?;
        std::fs::create_dir_all(parent)?;
        reject_symlinks(&spec.path)?;
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(&reference));
        let worktree = repo.worktree(&spec.run_id.short(), &spec.path, Some(&options))?;
        verify_checkout(&repo, &worktree, spec, phase)?;
        Ok(prepared_info(spec))
    }
}

fn verify_repository(repo: &Repository, spec: &RunWorktreeSpec) -> Result<(), GitError> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| conflict(&spec.path, WorktreeConflict::RepositoryMismatch))?;
    if workdir.canonicalize()? != spec.base.repository
        || repo.commondir().canonicalize()? != spec.base.git_common_dir
    {
        return Err(conflict(&spec.path, WorktreeConflict::RepositoryMismatch));
    }
    reject_symlinks(&spec.base.repository)?;
    reject_symlinks(&spec.base.git_common_dir)
}

fn verify_creation(repo: &Repository, spec: &RunWorktreeSpec) -> Result<(), GitError> {
    let message = spec.creation_message()?;
    let valid = repo
        .reflog(&format!("refs/heads/{}", spec.branch()))
        .ok()
        .is_some_and(|log| {
            log.len()
                .checked_sub(1)
                .and_then(|index| log.get(index))
                .is_some_and(|entry| {
                    entry.id_old().is_zero()
                        && entry.id_new() == spec.base.commit
                        && entry.message() == Some(message.as_str())
                })
        });
    if !valid {
        return Err(conflict(
            &spec.path,
            WorktreeConflict::MissingCreationEvidence,
        ));
    }
    Ok(())
}

fn verify_checkout(
    repo: &Repository,
    worktree: &git2::Worktree,
    spec: &RunWorktreeSpec,
    phase: ReconcilePhase,
) -> Result<(), GitError> {
    verify_creation(repo, spec)?;
    worktree.validate()?;
    if worktree.path() != spec.path || worktree.path().canonicalize()? != spec.path {
        return Err(conflict(&spec.path, WorktreeConflict::RegistrationMismatch));
    }
    let checkout = Repository::open(&spec.path)?;
    let linked = git2::Worktree::open_from_repository(&checkout)?;
    if checkout.commondir().canonicalize()? != spec.base.git_common_dir
        || checkout.workdir().map(Path::to_path_buf) != Some(spec.path.clone())
        || linked.name() != Some(spec.run_id.short().as_str())
        || linked.path() != spec.path
    {
        return Err(conflict(&spec.path, WorktreeConflict::RegistrationMismatch));
    }
    let head = checkout.find_reference("HEAD")?;
    if head.symbolic_target() != Some(format!("refs/heads/{}", spec.branch()).as_str()) {
        return Err(conflict(&spec.path, WorktreeConflict::BranchMismatch));
    }
    let commit = checkout.head()?.peel_to_commit()?.id();
    if phase == ReconcilePhase::BeforeExecution
        && (commit != spec.base.commit || is_dirty(&checkout)?)
    {
        return Err(conflict(
            &spec.path,
            WorktreeConflict::ModifiedBeforeExecution,
        ));
    }
    Ok(())
}

fn prepared_info(spec: &RunWorktreeSpec) -> RunWorktreeInfo {
    RunWorktreeInfo {
        run_id: spec.run_id,
        path: spec.path.clone(),
        branch: spec.branch(),
        exists_on_disk: true,
    }
}

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

#[cfg(test)]
mod pinned_tests {
    use super::*;
    use crate::GitManager;
    use git2::{Oid, Repository, Signature};

    fn commit(repo: &Repository, text: &str) -> Oid {
        std::fs::write(repo.workdir().unwrap().join("file"), text).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("file")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = Signature::now("test", "test@example.com").unwrap();
        let parent = repo.head().ok().map(|head| head.peel_to_commit().unwrap());
        let parents: Vec<_> = parent.iter().collect();
        repo.commit(Some("HEAD"), &signature, &signature, text, &tree, &parents)
            .unwrap()
    }

    fn fixture() -> (tempfile::TempDir, GitManager, Oid) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repo");
        let repo = Repository::init(&path).unwrap();
        repo.config().unwrap().set_str("user.name", "test").unwrap();
        repo.config()
            .unwrap()
            .set_str("user.email", "test@example.com")
            .unwrap();
        let oid = commit(&repo, "original");
        let manager = GitManager::new(path.canonicalize().unwrap()).unwrap();
        (root, manager, oid)
    }

    fn prepare_pinned(
        manager: &GitManager,
        id: RunId,
        base: Oid,
        path: PathBuf,
    ) -> RunWorktreeInfo {
        let repo = Repository::open(manager.repo_path()).unwrap();
        let pinned = PinnedRunBase::new(
            manager.repo_path().into(),
            repo.commondir().canonicalize().unwrap(),
            base,
        )
        .unwrap();
        let spec = RunWorktreeSpec::new(id, pinned, path).unwrap();
        manager
            .prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution)
            .unwrap()
    }

    #[test]
    fn preparation_uses_frozen_base_after_source_head_moves() {
        let (root, manager, base) = fixture();
        let repo = Repository::open(manager.repo_path()).unwrap();
        let moved = commit(&repo, "later source change");
        assert_ne!(base, moved);
        let id = RunId::new();
        let path = root.path().canonicalize().unwrap().join(id.short());
        let prepared = prepare_pinned(&manager, id, base, path);
        let checkout = Repository::open(prepared.path).unwrap();
        assert_eq!(checkout.head().unwrap().target(), Some(base));
    }

    fn spec(manager: &GitManager, root: &tempfile::TempDir) -> RunWorktreeSpec {
        RunWorktreeSpec::new(
            RunId::new(),
            manager.capture_clean_base().unwrap(),
            root.path().canonicalize().unwrap().join("exact-checkout"),
        )
        .unwrap()
    }

    fn assert_conflict(result: Result<RunWorktreeInfo, GitError>, reason: WorktreeConflict) {
        assert!(
            matches!(result, Err(GitError::WorktreeConflict { reason: actual, .. }) if actual == reason)
        );
    }

    #[test]
    fn preflight_requires_a_committed_clean_repository_including_untracked() {
        let (root, manager, base) = fixture();
        let pinned = manager.capture_clean_base().unwrap();
        assert_eq!(pinned.commit(), base);
        assert_eq!(pinned.repository(), manager.repo_path());
        std::fs::write(manager.repo_path().join("untracked"), "keep").unwrap();
        assert!(matches!(
            manager.capture_clean_base(),
            Err(GitError::DirtyRepository)
        ));
        std::fs::remove_file(manager.repo_path().join("untracked")).unwrap();
        std::fs::write(manager.repo_path().join("file"), "changed").unwrap();
        assert!(matches!(
            manager.capture_clean_base(),
            Err(GitError::DirtyRepository)
        ));
        let empty = root.path().join("empty");
        Repository::init(&empty).unwrap();
        assert!(matches!(
            GitManager::new(empty).unwrap().capture_clean_base(),
            Err(GitError::EmptyRepository)
        ));
    }

    #[test]
    fn exact_path_and_registration_are_reused_without_duplicates() {
        let (root, manager, base) = fixture();
        let spec = spec(&manager, &root);
        let first = manager
            .prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution)
            .unwrap();
        assert_eq!(first.path, spec.path());
        assert_eq!(
            std::fs::read_to_string(first.path.join("file")).unwrap(),
            "original"
        );
        let second = manager
            .prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution)
            .unwrap();
        assert_eq!(first.path, second.path);
        let repo = Repository::open(manager.repo_path()).unwrap();
        assert_eq!(repo.worktrees().unwrap().len(), 1);
        assert_eq!(repo.head().unwrap().target(), Some(base));
    }

    #[test]
    fn branch_creation_crash_recovers_from_creation_evidence() {
        let (root, manager, base) = fixture();
        let spec = spec(&manager, &root);
        let repo = Repository::open(manager.repo_path()).unwrap();
        let result = manager.prepare_pinned_inner(&spec, ReconcilePhase::BeforeExecution, || {
            Err(std::io::Error::other("simulated interruption after branch").into())
        });
        assert!(result.is_err());
        assert!(!spec.path().exists());
        assert_eq!(
            repo.find_branch(&spec.branch(), git2::BranchType::Local)
                .unwrap()
                .get()
                .target(),
            Some(base)
        );
        drop(repo);
        let reopened = GitManager::new(manager.repo_path().into()).unwrap();
        let rehydrated = RunWorktreeSpec::new(
            spec.run_id(),
            PinnedRunBase::new(
                spec.base().repository().into(),
                spec.base().git_common_dir().into(),
                spec.base().commit(),
            )
            .unwrap(),
            spec.path().into(),
        )
        .unwrap();
        reopened
            .prepare_run_worktree(&rehydrated, ReconcilePhase::BeforeExecution)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(spec.path().join("file")).unwrap(),
            "original"
        );
    }

    #[test]
    fn disabled_reflog_rejects_before_creating_an_unrecoverable_branch() {
        let (root, manager, _) = fixture();
        let spec = spec(&manager, &root);
        let repo = Repository::open(manager.repo_path()).unwrap();
        repo.config()
            .unwrap()
            .set_bool("core.logAllRefUpdates", false)
            .unwrap();
        assert_conflict(
            manager.prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution),
            WorktreeConflict::CreationLoggingDisabled,
        );
        assert!(!spec.path().exists());
        assert!(
            repo.find_branch(&spec.branch(), git2::BranchType::Local)
                .is_err()
        );
        assert_eq!(repo.worktrees().unwrap().len(), 0);
        assert!(
            !repo
                .config()
                .unwrap()
                .get_bool("core.logAllRefUpdates")
                .unwrap()
        );
    }

    #[test]
    fn unsupported_reflog_values_reject_before_branch_creation() {
        for value in ["always", "invalid"] {
            let (root, manager, _) = fixture();
            let spec = spec(&manager, &root);
            let repo = Repository::open(manager.repo_path()).unwrap();
            repo.config()
                .unwrap()
                .set_str("core.logAllRefUpdates", value)
                .unwrap();
            assert_conflict(
                manager.prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution),
                WorktreeConflict::CreationLoggingUnsupported,
            );
            assert!(!spec.path().exists());
            assert!(
                repo.find_branch(&spec.branch(), git2::BranchType::Local)
                    .is_err()
            );
            assert_eq!(repo.worktrees().unwrap().len(), 0);
            assert_eq!(
                repo.config()
                    .unwrap()
                    .get_string("core.logAllRefUpdates")
                    .unwrap(),
                value
            );
        }
    }

    #[test]
    fn absent_reflog_setting_uses_nonbare_default() {
        let (root, manager, _) = fixture();
        let spec = spec(&manager, &root);
        let repo = Repository::open(manager.repo_path()).unwrap();
        repo.config()
            .unwrap()
            .remove("core.logAllRefUpdates")
            .unwrap();
        manager
            .prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution)
            .unwrap();
        assert_eq!(
            Repository::open(spec.path())
                .unwrap()
                .head()
                .unwrap()
                .target(),
            Some(spec.base().commit())
        );
    }

    #[test]
    fn executed_checkout_preserves_edits_and_advanced_head() {
        let (root, manager, base) = fixture();
        let spec = spec(&manager, &root);
        manager
            .prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution)
            .unwrap();
        let checkout = Repository::open(spec.path()).unwrap();
        let advanced = commit(&checkout, "agent commit");
        assert_ne!(advanced, base);
        std::fs::write(spec.path().join("untracked"), "agent uncommitted output").unwrap();
        assert_conflict(
            manager.prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution),
            WorktreeConflict::ModifiedBeforeExecution,
        );
        manager
            .prepare_run_worktree(&spec, ReconcilePhase::AfterExecution)
            .unwrap();
        assert_eq!(checkout.head().unwrap().target(), Some(advanced));
        assert_eq!(
            std::fs::read_to_string(spec.path().join("untracked")).unwrap(),
            "agent uncommitted output"
        );
    }

    #[test]
    fn foreign_branch_at_the_same_oid_is_not_adopted() {
        let (root, manager, base) = fixture();
        let spec = spec(&manager, &root);
        let repo = Repository::open(manager.repo_path()).unwrap();
        repo.branch(&spec.branch(), &repo.find_commit(base).unwrap(), false)
            .unwrap();
        assert_conflict(
            manager.prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution),
            WorktreeConflict::MissingCreationEvidence,
        );
        assert!(!spec.path().exists());
        assert_eq!(
            repo.find_branch(&spec.branch(), git2::BranchType::Local)
                .unwrap()
                .get()
                .target(),
            Some(base)
        );
        assert_eq!(repo.worktrees().unwrap().len(), 0);
    }

    #[test]
    fn occupied_path_is_preserved_before_any_branch_creation() {
        let (root, manager, _) = fixture();
        let spec = spec(&manager, &root);
        std::fs::create_dir(spec.path()).unwrap();
        std::fs::write(spec.path().join("foreign"), "keep").unwrap();
        assert_conflict(
            manager.prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution),
            WorktreeConflict::OccupiedPath,
        );
        assert_eq!(
            std::fs::read_to_string(spec.path().join("foreign")).unwrap(),
            "keep"
        );
        let repo = Repository::open(manager.repo_path()).unwrap();
        assert!(
            repo.find_branch(&spec.branch(), git2::BranchType::Local)
                .is_err()
        );
    }

    #[test]
    fn expired_creation_evidence_and_missing_executed_checkout_fail_closed() {
        let (root, manager, _) = fixture();
        let spec = spec(&manager, &root);
        assert_conflict(
            manager.prepare_run_worktree(&spec, ReconcilePhase::AfterExecution),
            WorktreeConflict::MissingAfterExecution,
        );
        manager
            .prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution)
            .unwrap();
        let repo = Repository::open(manager.repo_path()).unwrap();
        repo.reflog_delete(&format!("refs/heads/{}", spec.branch()))
            .unwrap();
        assert_conflict(
            manager.prepare_run_worktree(&spec, ReconcilePhase::AfterExecution),
            WorktreeConflict::MissingCreationEvidence,
        );
        assert_eq!(
            std::fs::read_to_string(spec.path().join("file")).unwrap(),
            "original"
        );
    }

    #[test]
    fn changed_recorded_path_and_repository_cannot_adopt_an_existing_checkout() {
        let (root, manager, _) = fixture();
        let spec = spec(&manager, &root);
        manager
            .prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution)
            .unwrap();
        let changed = RunWorktreeSpec::new(
            spec.run_id(),
            spec.base().clone(),
            root.path().canonicalize().unwrap().join("different"),
        )
        .unwrap();
        assert!(
            manager
                .prepare_run_worktree(&changed, ReconcilePhase::AfterExecution)
                .is_err()
        );
        let (_other_root, other, _) = fixture();
        assert_conflict(
            other.prepare_run_worktree(&spec, ReconcilePhase::AfterExecution),
            WorktreeConflict::RepositoryMismatch,
        );
        assert!(!changed.path().exists());
        assert_eq!(
            std::fs::read_to_string(spec.path().join("file")).unwrap(),
            "original"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_is_never_adopted() {
        let (root, manager, _) = fixture();
        let spec = spec(&manager, &root);
        let foreign = root.path().join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        std::os::unix::fs::symlink(&foreign, spec.path()).unwrap();
        assert_conflict(
            manager.prepare_run_worktree(&spec, ReconcilePhase::BeforeExecution),
            WorktreeConflict::SymbolicLink,
        );
        assert!(
            std::fs::symlink_metadata(spec.path())
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_dir(&foreign).unwrap().count(), 0);
    }
}
