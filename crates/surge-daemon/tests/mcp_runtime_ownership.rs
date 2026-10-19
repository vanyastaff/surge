//! Actual child and original stable launch lease across runtime destruction.
#![cfg(unix)]
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use surge_core::{Graph, id::WorkItemOperationId, work_item::*};
use surge_persistence::{runs::Storage, work_items::OwnedFlowPreparationResult};

struct Observe {
    _claim: surge_persistence::work_items::WorkItemLaunchClaim,
    pid: Mutex<mpsc::Sender<u32>>,
}
#[async_trait::async_trait]
impl surge_mcp::writer_observer::HostWriterObserver for Observe {
    async fn before_child(
        &self,
        _server: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, surge_mcp::writer_observer::WriterObservationError>
    {
        Ok(surge_core::id::ExecutionWriterId::new())
    }
    async fn child_started(
        &self,
        _writer: surge_core::id::ExecutionWriterId,
        pid: Option<u32>,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        self.pid.lock().unwrap().send(pid.unwrap()).unwrap();
        std::future::pending().await
    }
}

#[test]
fn independent_claim_probe() {
    let Some(home) = std::env::var_os("SURGE_RUNTIME_LEASE_PROBE_HOME") else {
        return;
    };
    let run = std::env::var("SURGE_RUNTIME_LEASE_PROBE_RUN")
        .unwrap()
        .parse()
        .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let storage = runtime.block_on(Storage::open(&home)).unwrap();
    let result = match storage.work_items().claim(run) {
        Err(surge_persistence::work_items::WorkItemError::Busy) => "busy",
        Ok(_claim) => "claimed",
        Err(_) => "refused",
    };
    std::fs::write(std::path::Path::new(&home).join("probe-result"), result).unwrap();
}

#[test]
fn runtime_drop_cannot_release_original_launch_lease_while_child_survives() {
    let home = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let storage = runtime.block_on(Storage::open(home.path())).unwrap();
    let store = storage.work_items();
    let OwnedFlowPreparationResult::Preparing(mut prep) = store
        .begin_owned_flow(
            WorkItemOperationId::new(),
            b"runtime ownership oracle",
            false,
            1,
        )
        .unwrap()
    else {
        panic!("new preparation");
    };
    let graph: Graph =
        toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap();
    let snapshot = surge_persistence::work_items::OwnedFlowSourceSnapshot {
        contract: AcceptedFlowContract::new(Box::new(graph), String::new()).unwrap(),
        source_base: home.path().join("project"),
        workspace: WorkItemWorkspace {
            repository: home.path().join("project/.git"),
            checkout: home.path().join("project"),
            path: home.path().join("workspace"),
            ownership: prep.workspace_owner().to_string(),
            branch: "codex/owned".into(),
            base_commit: "a".repeat(40),
        },
    };
    prep.freeze_source(&snapshot, None).unwrap();
    prep.freeze_startup(
        r#"{"mcp_servers":[]}"#,
        OwnedFlowMcpSelection::Explicit,
        &[],
    )
    .unwrap();
    prep.retain_launch_ownership().unwrap();
    let (receipt, claim) = prep.finalize(2).unwrap().into_parts();
    let script = home.path().join("persistent-child.py");
    // Child survives closed stdin so transport Drop alone cannot fake settlement.
    std::fs::write(&script, "import time\nwhile True: time.sleep(0.1)\n").unwrap();
    let (send, receive) = mpsc::channel();
    let observer = Arc::new(Observe {
        _claim: claim,
        pid: Mutex::new(send),
    });
    let config = surge_core::mcp_config::McpServerRef::new(
        "runtime-oracle".into(),
        surge_core::mcp_config::McpTransportConfig::stdio(
            "/usr/bin/python3".into(),
            vec![script.to_string_lossy().into_owned()],
            Default::default(),
        ),
        None,
        Duration::from_secs(10),
        false,
    );
    let connection = Arc::new(surge_mcp::McpServerConnection::new_owned(
        config, None, observer,
    ));
    // Keep the actual postspawn future outside the runtime task set. Its Drop
    // must remain safe after the runtime and entered handle have disappeared.
    let mut opening = Box::pin(connection.list_tools());
    {
        let _entered = runtime.enter();
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(opening.as_mut(), &mut context).is_pending());
    }
    let pid = receive.recv_timeout(Duration::from_secs(5)).unwrap();
    let alive = |pid: u32| {
        std::process::Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .unwrap()
            .stdout
    };
    assert!(
        !alive(pid).is_empty(),
        "actual postspawn child missing before runtime drop"
    );
    drop(runtime);
    let cleanup_panicked =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(opening))).is_err();
    drop(connection);
    let probe = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "independent_claim_probe", "--nocapture"])
        .env("SURGE_RUNTIME_LEASE_PROBE_HOME", home.path())
        .env("SURGE_RUNTIME_LEASE_PROBE_RUN", receipt.run.to_string())
        .status()
        .unwrap();
    let observed = std::fs::read_to_string(home.path().join("probe-result"))
        .unwrap_or_else(|_| "probe-failed".into());
    let survived = !alive(pid).is_empty();
    // Terminal control is independent of runtime/transport lifetime and joins
    // every standard owner; it does not infer exit from the dropped runtime.
    surge_mcp::shutdown_children_and_join();
    let after_barrier = !alive(pid).is_empty();
    assert!(probe.success(), "independent lease probe setup failed");
    assert!(
        !cleanup_panicked,
        "actual postspawn cleanup panicked outside a Tokio runtime"
    );
    assert!(
        !survived || observed == "busy",
        "original launch lease was released while the actual child survived runtime destruction"
    );
    assert!(
        !after_barrier,
        "terminal child barrier returned before actual child reap"
    );
    let reacquired = store.claim(receipt.run).unwrap();
    drop(reacquired);
}
