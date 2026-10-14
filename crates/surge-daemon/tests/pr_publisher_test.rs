//! #127: the L3 publisher commits a ticket run's work on its branch, pushes
//! it to the project's `origin` and opens the PR that closes the ticket.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use surge_core::id::RunId;
use surge_daemon::pr_publisher::TicketPullRequestPublisher;
use surge_intake::TaskSource;
use surge_intake::testing::MockTaskSource;
use surge_intake::types::TaskId;
use surge_orchestrator::engine::publish::{PublishOutcome, PublishRequest, PullRequestPublisher};

struct Fixture {
    _dir: tempfile::TempDir,
    project: PathBuf,
    remote: PathBuf,
    worktree: PathBuf,
    run_id: RunId,
}

/// A committed project whose `origin` is a local bare repository, and a run
/// worktree of it on the run's branch.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let remote = root.join("origin.git");
    git2::Repository::init_bare(&remote).unwrap();
    let project = root.join("project");
    let repo = git2::Repository::init(&project).unwrap();
    std::fs::write(project.join("README.md"), "fixture\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
        .unwrap();
    repo.remote("origin", remote.to_str().unwrap()).unwrap();

    let run_id = RunId::new();
    let worktree = root.join("worktrees").join(run_id.to_string());
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    let git = surge_git::GitManager::new(project.clone()).unwrap();
    let base = git.capture_clean_base().unwrap();
    let spec =
        surge_git::run_worktree::RunWorktreeSpec::new(run_id, base, worktree.clone()).unwrap();
    git.prepare_run_worktree(
        &spec,
        surge_git::run_worktree::ReconcilePhase::BeforeExecution,
    )
    .unwrap();
    Fixture {
        _dir: dir,
        project,
        remote,
        worktree,
        run_id,
    }
}

fn publisher(fx: &Fixture, source: &Arc<MockTaskSource>) -> TicketPullRequestPublisher {
    TicketPullRequestPublisher::new(
        Arc::clone(source) as Arc<dyn TaskSource>,
        TaskId::try_new("github_issues:acme/app#42").unwrap(),
        "Fix the parser panic".into(),
        fx.project.clone(),
        None,
    )
}

fn request(fx: &Fixture) -> PublishRequest {
    PublishRequest {
        run_id: fx.run_id,
        worktree: fx.worktree.clone(),
    }
}

#[tokio::test]
async fn the_run_branch_is_committed_pushed_and_opened_as_a_pr() {
    let fx = fixture();
    let source = Arc::new(MockTaskSource::new("github_issues:acme/app", "github"));
    std::fs::write(fx.worktree.join("parser.rs"), "fn parse() {}\n").unwrap();
    std::fs::create_dir_all(fx.worktree.join(".surge")).unwrap();
    std::fs::write(fx.worktree.join(".surge/user_prompt.txt"), "prompt").unwrap();

    let outcome = publisher(&fx, &source)
        .publish(&request(&fx))
        .await
        .unwrap();
    let PublishOutcome::Published(published) = outcome else {
        panic!("expected a PR, got {outcome:?}")
    };
    let branch = surge_git::run_worktree::run_branch_name(&fx.run_id);
    assert_eq!(published.head_branch, branch);
    assert_eq!(published.pr.number, 100);
    assert_eq!(published.pr.repository, "acme/app");

    // The branch is on origin, at the committed head, without Surge's files.
    let remote = git2::Repository::open_bare(&fx.remote).unwrap();
    let pushed = remote
        .find_reference(&format!("refs/heads/{branch}"))
        .unwrap()
        .peel_to_commit()
        .unwrap();
    assert_eq!(pushed.id().to_string(), published.head_sha);
    let tree = pushed.tree().unwrap();
    assert!(tree.get_path(Path::new("parser.rs")).is_ok());
    assert!(tree.get_path(Path::new(".surge/user_prompt.txt")).is_err());
    assert!(pushed.message().unwrap().contains("Closes #42"));

    let drafts = source.opened_pull_requests().await;
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].head_branch, branch);
    assert_eq!(drafts[0].title, "Fix the parser panic");
    assert!(drafts[0].body.contains("Closes #42"));

    // Publishing again (a resume) adopts the same PR.
    let again = publisher(&fx, &source)
        .publish(&request(&fx))
        .await
        .unwrap();
    assert!(matches!(again, PublishOutcome::Published(ref p) if p.pr.number == 100));
    assert_eq!(source.opened_pull_requests().await.len(), 1);
}

#[tokio::test]
async fn a_run_that_changed_nothing_publishes_nothing() {
    let fx = fixture();
    let source = Arc::new(MockTaskSource::new("github_issues:acme/app", "github"));
    let outcome = publisher(&fx, &source)
        .publish(&request(&fx))
        .await
        .unwrap();
    assert_eq!(outcome, PublishOutcome::NothingToPublish);
    assert!(source.opened_pull_requests().await.is_empty());
    let remote = git2::Repository::open_bare(&fx.remote).unwrap();
    assert!(
        remote.references().unwrap().next().is_none(),
        "nothing pushed"
    );
}
