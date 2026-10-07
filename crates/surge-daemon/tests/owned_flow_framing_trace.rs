//! Actual malformed owner request diagnostics under the production global filter.
#[path = "../../surge-orchestrator/tests/fixtures/mock_bridge.rs"]
mod mock_bridge;

use interprocess::local_socket::tokio::prelude::*;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use surge_orchestrator::engine::{Engine, EngineConfig, facade::LocalEngineFacade};
use surge_persistence::runs::Storage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing_subscriber::prelude::*;

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut output = self.0.lock().unwrap();
        let remaining = 65536_usize.saturating_sub(output.len());
        output.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn malformed_private_owner_frame_is_opaque_and_has_no_effects() {
    let captured = Capture(Arc::new(Mutex::new(Vec::new())));
    let writer = captured.clone();
    // Same GLOBAL target veto AND EnvFilter ordering as the daemon entrypoint.
    tracing_subscriber::registry()
        .with(tracing_subscriber::filter::filter_fn(|metadata| {
            surge_mcp::diagnostics::permits_target(metadata.target())
        }))
        .with(tracing_subscriber::EnvFilter::new("trace"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone()),
        )
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let bridge = Arc::new(mock_bridge::MockBridge::new());
        let engine = Arc::new(Engine::new(bridge.clone(), storage.clone(), Arc::new(
            surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(project.path().into())
        ), EngineConfig::default()));
        let cancel = tokio_util::sync::CancellationToken::new();
        let socket = home.path().join("framing.sock");
        let mut server = tokio::spawn(surge_daemon::run_runs_only(
            surge_daemon::ServerConfig { socket_path: socket.clone(), max_active: 1, max_queue: 2 },
            Arc::new(LocalEngineFacade::new(engine.clone())),
            surge_daemon::tracked_run::TrackingContext::new(engine, storage.clone()),
            Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
            Arc::new(surge_daemon::admission::AdmissionController::new(1, 2)), cancel.clone(),
        ));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
            if server.is_finished() {
                panic!("daemon stopped before readiness: {:?}", (&mut server).await);
            }
            if surge_orchestrator::engine::daemon_facade::DaemonClient::connect(socket.clone())
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        }).await.unwrap();
        let sentinel = "PRIVATE-SOCKET-UNKNOWN-MCP-TRANSPORT";
        let request = serde_json::json!({"method":"owned_flow_start","request_id":1,"request":{
            "operation_id":surge_core::id::WorkItemOperationId::new(),"source_project":project.path(),
            "input":{"kind":"template","key":"single-task"},"workspace":{"kind":"managed"},
            "config":{"mcp":{"kind":"explicit","servers":[{"name":"public","transport":{"kind":sentinel}}]}}
        }});
        let stream = LocalSocketStream::connect(
            surge_orchestrator::engine::ipc::local_socket_name_from_path(&socket).unwrap()
        ).await.unwrap();
        let (read, mut write) = stream.split();
        let mut bytes = serde_json::to_vec(&request).unwrap(); bytes.push(b'\n');
        write.write_all(&bytes).await.unwrap();
        let mut reply = String::new();
        let count = tokio::time::timeout(Duration::from_secs(3), BufReader::new(read).read_line(&mut reply))
            .await.unwrap().unwrap();
        assert_eq!(count, 0, "malformed connection must close without owner acceptance");
        assert!(bridge.recorded_calls.lock().await.is_empty(), "malformed input opened a provider");
        let surge_core::work_item::WorkItemResult::Items(items) = storage.work_items().query(
            &surge_core::work_item::WorkItemCommand::List { after: None, limit: 10 }
        ).unwrap() else { panic!("expected item page"); };
        assert!(items.entries.is_empty(), "malformed input accepted a durable item");
        assert!(!home.path().join("work-items/private-flow-inputs").exists(), "malformed input captured private effects");
        assert!(!home.path().join("work-items/preparation-locks").exists(), "malformed input began an owned operation");
        cancel.cancel(); server.await.unwrap().unwrap();
        let output = captured.0.lock().unwrap();
        let public = String::from_utf8_lossy(&output);
        assert!(public.contains("read_request_frame failed; closing connection"), "real framing warning missing");
        assert!(public.contains("invalid JSON frame"), "typed safe framing metadata missing");
        assert!(!public.contains(sentinel), "private socket input escaped host TRACE diagnostics");
    });
}
