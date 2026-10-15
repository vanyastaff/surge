use super::*;
use gpui_kit::{AppContext, TestAppContext};
use std::sync::atomic::{AtomicUsize, Ordering};
use surge_orchestrator::engine::{
    daemon_facade::DaemonEngineFacade,
    ipc::{DaemonRequest, read_request_frame},
};
fn pump(
    runtime: &tokio::runtime::Runtime,
    cx: &mut TestAppContext,
    condition: impl Fn(&mut TestAppContext) -> bool,
    seconds: u64,
) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while !condition(cx) && std::time::Instant::now() < end {
        cx.run_until_parked();
        runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(5)));
    }
    condition(cx)
}
fn state(
    cx: &mut TestAppContext,
    root: &std::path::Path,
    facade: Arc<DaemonEngineFacade>,
    history: bool,
) -> (Entity<AppState>, WorkItemId) {
    let item = WorkItemId::new();
    let state = cx.new(|_| {
        let mut state = AppState::new();
        let scope = crate::project::ProjectScope::resolve(root);
        state.project_path = Some(root.into());
        state.tasks.begin(scope.repository().unwrap().into());
        state.project_scope = Some(scope);
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade);
        if history {
            state.tasks.records.push(WorkItemRecord {
                id: item,
                project: surge_core::id::WorkItemProjectId::new(),
                title: "Held history".into(),
                accepted_revision: 1,
                version: 1,
                archived_at_ms: None,
                active_run: None,
                generation: 0,
                workspace: surge_core::work_item::WorkItemWorkspace {
                    repository: root.join(".git"),
                    checkout: root.into(),
                    path: root.into(),
                    ownership: "fixture".into(),
                    branch: "fixture".into(),
                    base_commit: "a".repeat(40),
                },
            });
            state.tasks.histories.insert(
                item,
                Arc::new(TaskHistory {
                    attempts: WorkItemPage {
                        entries: Vec::new(),
                        next_cursor: Some("1".into()),
                    },
                    discussion: WorkItemPage {
                        entries: Vec::new(),
                        next_cursor: None,
                    },
                    revisions: WorkItemPage {
                        entries: Vec::new(),
                        next_cursor: None,
                    },
                }),
            );
        }
        state.tasks.loading = false;
        state
    });
    (state, item)
}
fn held_read(cx: &mut TestAppContext, history: bool) {
    let home = tempfile::tempdir().unwrap();
    git2::Repository::init(home.path()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let count = Arc::new(AtomicUsize::new(0));
    let socket = home.path().join("held.sock");
    let (facade, server) = runtime.block_on(async {
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let observed = count.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, _write) = stream.into_split();
            let mut reader = tokio::io::BufReader::new(read);
            while let Some(DaemonRequest::WorkItem { .. }) =
                read_request_frame(&mut reader).await.unwrap()
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        });
        (
            Arc::new(DaemonEngineFacade::connect(socket).await.unwrap()),
            server,
        )
    });
    let facade_owner = Arc::downgrade(&facade);
    let (owner, item) = state(cx, home.path(), facade, history);
    cx.update(|cx| {
        if history {
            request_task_history(&owner, item, TaskHistoryKind::Attempts, cx);
        } else {
            refresh_project_tasks(&owner, false, cx);
        }
    });
    assert!(
        pump(&runtime, cx, |_| count.load(Ordering::SeqCst) == 1, 2),
        "peer received actual request"
    );
    let weak = owner.downgrade();
    owner.update(cx, |state, _| {
        state.tasks.disconnected();
        state.daemon_state = crate::daemon_link::ConnectionState::Disconnected;
    });
    drop(owner);
    cx.update(|_| {});
    let released = pump(&runtime, cx, |_| weak.upgrade().is_none(), 1);
    server.abort();
    runtime.block_on(async {
        let _ = server.await;
    });
    assert!(
        pump(&runtime, cx, |_| facade_owner.upgrade().is_none(), 2),
        "readonly request settled after peer close"
    );
    assert!(
        released,
        "silent daemon readonly request retained disconnected AppState"
    );
}
#[gpui_kit::test]
fn held_list_reply_does_not_retain_disconnected_project_owner(cx: &mut TestAppContext) {
    held_read(cx, false);
}
#[gpui_kit::test]
fn held_history_reply_does_not_retain_disconnected_project_owner(cx: &mut TestAppContext) {
    held_read(cx, true);
}

#[gpui_kit::test]
fn silent_readonly_timeouts_preserve_cursors_and_allow_same_project_retry(cx: &mut TestAppContext) {
    for history in [false, true] {
        let home = tempfile::tempdir().unwrap();
        git2::Repository::init(home.path()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let count = Arc::new(AtomicUsize::new(0));
        let socket = home.path().join("timeout.sock");
        let (facade, server) = runtime.block_on(async {
            let listener = tokio::net::UnixListener::bind(&socket).unwrap();
            let observed = count.clone();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let (read, _write) = stream.into_split();
                let mut reader = tokio::io::BufReader::new(read);
                while let Some(DaemonRequest::WorkItem { .. }) =
                    read_request_frame(&mut reader).await.unwrap()
                {
                    observed.fetch_add(1, Ordering::SeqCst);
                }
            });
            (
                Arc::new(DaemonEngineFacade::connect(socket).await.unwrap()),
                server,
            )
        });
        let facade_owner = Arc::downgrade(&facade);
        let (owner, item) = state(cx, home.path(), facade, history);
        let request = |cx: &mut gpui_kit::App| {
            if history {
                request_task_history(&owner, item, TaskHistoryKind::Attempts, cx);
            } else {
                refresh_project_tasks(&owner, false, cx);
            }
        };
        cx.update(request);
        assert!(pump(&runtime, cx, |_| count.load(Ordering::SeqCst) == 1, 2));
        let timed_out = pump(
            &runtime,
            cx,
            |cx| cx.read(|cx| owner.read(cx).tasks.error.is_some()),
            10,
        );
        let state_valid = cx.read(|cx| {
            let tasks = &owner.read(cx).tasks;
            if history {
                tasks.histories[&item].attempts.next_cursor.as_deref() == Some("1")
            } else {
                !tasks.loading && !tasks.fresh
            }
        });
        cx.update(request);
        let retried = pump(&runtime, cx, |_| count.load(Ordering::SeqCst) == 2, 2);
        owner.update(cx, |state, _| state.tasks.disconnected());
        drop(owner);
        cx.update(|_| {});
        server.abort();
        runtime.block_on(async {
            let _ = server.await;
        });
        assert!(pump(&runtime, cx, |_| facade_owner.upgrade().is_none(), 2));
        cx.run_until_parked();
        assert!(
            timed_out && state_valid && retried,
            "bounded readonly failure must preserve history and permit explicit retry"
        );
    }
}

#[gpui_kit::test]
fn old_held_history_timeout_cannot_overwrite_newer_success(cx: &mut TestAppContext) {
    use surge_core::work_item::WorkItemResult;
    use surge_orchestrator::engine::ipc::{DaemonResponse, write_frame};
    let home = tempfile::tempdir().unwrap();
    git2::Repository::init(home.path()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let count = Arc::new(AtomicUsize::new(0));
    let socket = home.path().join("out-of-order.sock");
    let (facade,server)=runtime.block_on(async {
        let listener=tokio::net::UnixListener::bind(&socket).unwrap();let observed=count.clone();
        let server=tokio::spawn(async move {
            let(stream,_)=listener.accept().await.unwrap();let(read,mut write)=stream.into_split();let mut reader=tokio::io::BufReader::new(read);
            while let Some(DaemonRequest::WorkItem{request_id,command})=read_request_frame(&mut reader).await.unwrap() {
                assert!(matches!(*command,WorkItemCommand::Attempts{ref after,..} if after.as_deref()==Some("1")));
                if observed.fetch_add(1,Ordering::SeqCst)==1 {
                    write_frame(&mut write,&DaemonResponse::WorkItemOk{request_id,result:Box::new(WorkItemResult::Attempts(WorkItemPage{entries:Vec::new(),next_cursor:Some("2".into())}))}).await.unwrap();
                }
            }
        });(Arc::new(DaemonEngineFacade::connect(socket).await.unwrap()),server)
    });
    let transport = Arc::downgrade(&facade);
    let (owner, item) = state(cx, home.path(), facade, true);
    cx.update(|cx| request_task_history(&owner, item, TaskHistoryKind::Attempts, cx));
    assert!(pump(&runtime, cx, |_| count.load(Ordering::SeqCst) == 1, 2));
    cx.update(|cx| request_task_history(&owner, item, TaskHistoryKind::Attempts, cx));
    assert!(
        pump(
            &runtime,
            cx,
            |cx| cx.read(|cx| owner.read(cx).tasks.histories[&item]
                .attempts
                .next_cursor
                .as_deref()
                == Some("2")),
            2
        ),
        "newer request B succeeded while A is held"
    );
    let end = std::time::Instant::now() + std::time::Duration::from_secs(9);
    while std::time::Instant::now() < end {
        cx.run_until_parked();
        runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(5)));
    }
    let error = cx.read(|cx| owner.read(cx).tasks.error.clone());
    let cursor = cx.read(|cx| {
        owner.read(cx).tasks.histories[&item]
            .attempts
            .next_cursor
            .clone()
    });
    owner.update(cx, |state, _| state.tasks.disconnected());
    drop(owner);
    cx.update(|_| {});
    server.abort();
    runtime.block_on(async {
        let _ = server.await;
    });
    assert!(pump(&runtime, cx, |_| transport.upgrade().is_none(), 2));
    assert_eq!(cursor.as_deref(), Some("2"));
    assert!(
        error.is_none(),
        "old request A timeout overwrote newer B success: {error:?}"
    );
}
