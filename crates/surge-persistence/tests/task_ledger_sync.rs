//! Integration test: task-ledger events maintain the per-run view, and
//! `Storage::sync_task_ledger_index` mirrors that view into the cross-run
//! registry index (`surge ready` / `surge ledger` read path — Phase 1 M5).

use std::path::PathBuf;

use surge_core::RoadmapStatus;
use surge_core::approvals::ApprovalPolicy;
use surge_core::content_hash::ContentHash;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
use surge_core::sandbox::SandboxMode;
use surge_persistence::runs::Storage;
use surge_persistence::task_ledger::TaskLedgerIndexFilter;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_mirrors_run_ledger_into_registry_index() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

    let events = vec![
        EventPayload::RunStarted {
            pipeline_template: None,
            project_path: project.clone(),
            initial_prompt: "ledger sync".into(),
            config: RunConfig {
                bootstrap_edit_loop_cap: None,
                budget: Default::default(),
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
            },
        },
        EventPayload::TaskDiscovered {
            task_id: "m1-t2".into(),
            discovered_from: "m1-t1".into(),
            title: "Handle empty input".into(),
        },
        EventPayload::TaskStatusChanged {
            task_id: "m1-t1".into(),
            from: RoadmapStatus::Pending,
            to: RoadmapStatus::ReadyForVerification,
            authority_node: NodeKey::try_from("impl_1").unwrap(),
        },
        EventPayload::TaskVerified {
            task_id: "m1-t1".into(),
            node: NodeKey::try_from("verify_1").unwrap(),
            evidence: ContentHash::compute(b"report"),

            report: None,
        },
    ];
    for payload in &events {
        writer
            .append_event(VersionedEventPayload::new(payload.clone()))
            .await
            .unwrap();
    }
    writer.flush().await.unwrap();

    let synced = storage
        .sync_task_ledger_index(run_id, &project)
        .await
        .expect("sync succeeds");
    assert_eq!(synced, 2, "two tasks in the ledger");

    let store = storage.task_ledger_store();
    let rows = store
        .list(&TaskLedgerIndexFilter {
            run_id: Some(run_id),
            ..Default::default()
        })
        .expect("list");
    assert_eq!(rows.len(), 2);

    let t1 = rows.iter().find(|r| r.task_id == "m1-t1").expect("t1");
    // An unbound, unauthorized historical claim cannot become current proof.
    assert_eq!(t1.status, RoadmapStatus::ReadyForVerification);
    assert!(!t1.verified);
    assert!(!t1.is_evidence_backed());
    assert_eq!(t1.last_authority_node.as_deref(), Some("impl_1"));
    assert_eq!(t1.project_path, PathBuf::from(&project));

    let t2 = rows.iter().find(|r| r.task_id == "m1-t2").expect("t2");
    assert_eq!(t2.status, RoadmapStatus::Pending);
    assert!(!t2.verified);
    assert_eq!(t2.discovered_from.as_deref(), Some("m1-t1"));

    // Idempotent: a second sync updates in place, no duplicate rows.
    let synced_again = storage
        .sync_task_ledger_index(run_id, &project)
        .await
        .expect("second sync");
    assert_eq!(synced_again, 2);
    let rows2 = store
        .list(&TaskLedgerIndexFilter {
            run_id: Some(run_id),
            ..Default::default()
        })
        .expect("list again");
    assert_eq!(rows2.len(), 2, "no duplicate index rows after re-sync");

    // `discovered_only` filter surfaces the mid-run discovery.
    let discovered = store
        .list(&TaskLedgerIndexFilter {
            discovered_only: true,
            ..Default::default()
        })
        .expect("discovered list");
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].task_id, "m1-t2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_overrides_reach_the_run_view_and_the_registry_index() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
    let verify = NodeKey::try_from("verify_1").unwrap();
    for payload in [
        EventPayload::RequirementRevised {
            node: verify.clone(),
            task: Some("login".into()),
            text: "Sessions last 24 hours".into(),
        },
        EventPayload::TaskAcceptedByHuman {
            node: verify.clone(),
            task: Some("login".into()),
            findings: Some(ContentHash::compute(b"report")),
            comment: None,
        },
        // A run-wide revision has no ledger row.
        EventPayload::RequirementRevised {
            node: verify.clone(),
            task: None,
            text: "Use UTC everywhere".into(),
        },
        EventPayload::TaskAcceptedByHuman {
            node: verify.clone(),
            task: Some("logout".into()),
            findings: None,
            comment: None,
        },
        // Sent back into work after acceptance: no longer accepted.
        EventPayload::TaskStatusChanged {
            task_id: "logout".into(),
            from: RoadmapStatus::Completed,
            to: RoadmapStatus::ReadyForVerification,
            authority_node: verify.clone(),
        },
    ] {
        writer
            .append_event(VersionedEventPayload::new(payload))
            .await
            .unwrap();
    }
    writer.flush().await.unwrap();

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let rows = reader.task_ledger().await.unwrap();
    let login = rows.iter().find(|row| row.task_id == "login").unwrap();
    assert_eq!(login.status, RoadmapStatus::Completed);
    assert!(!login.verified && login.accepted_by_human && login.requirement_revised);
    let logout = rows.iter().find(|row| row.task_id == "logout").unwrap();
    assert!(!logout.accepted_by_human);
    assert_eq!(rows.len(), 2);

    storage
        .sync_task_ledger_index(run_id, &project)
        .await
        .expect("sync succeeds");
    let index = storage
        .task_ledger_store()
        .get(run_id, "login")
        .unwrap()
        .unwrap();
    assert!(index.accepted_by_human && index.requirement_revised);
    assert!(
        !index.is_evidence_backed(),
        "a human acceptance is never evidence"
    );
}
