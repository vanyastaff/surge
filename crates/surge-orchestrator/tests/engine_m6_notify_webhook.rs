//! M6: Notify Webhook channel POSTs to local tiny_http server.
//! Verifies that captured request body contains the run_id.
//!
//! Graph layout:
//!   notify_node (Notify: Webhook to loopback) -- on "delivered" --> end
//!   end (Terminal::Success)

mod fixtures;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::notify_config::{
    NotifyChannel, NotifyConfig, NotifyFailureAction, NotifySeverity, NotifyTemplate,
};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_notify::{MultiplexingNotifier, WebhookDeliverer};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

fn build_notify_webhook_graph(webhook_url: String) -> Graph {
    let notify_key = NodeKey::try_from("notify_1").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();
    let delivered_outcome = OutcomeKey::try_from("delivered").unwrap();

    let notify_node = Node {
        id: notify_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Notify(NotifyConfig {
            channel: NotifyChannel::Webhook { url: webhook_url },
            template: NotifyTemplate {
                severity: NotifySeverity::Info,
                title: "Run progress".into(),
                body: "Run {{run_id}} executing notify stage".into(),
                artifacts: vec![],
            },
            on_failure: NotifyFailureAction::Continue,
        }),
    };

    let end_node = Node {
        id: end_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    };

    let edge_notify_to_end = Edge {
        id: EdgeKey::try_from("e_notify_done").unwrap(),
        from: PortRef {
            node: notify_key.clone(),
            outcome: delivered_outcome,
        },
        to: end_key.clone(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    };

    let mut nodes = BTreeMap::new();
    nodes.insert(notify_key.clone(), notify_node);
    nodes.insert(end_key, end_node);

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "notify_webhook".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: notify_key,
        nodes,
        edges: vec![edge_notify_to_end],
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notify_webhook_posts_body_containing_run_id() {
    // Start a tiny_http server on an ephemeral port.
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/hook", server.server_addr().to_ip().unwrap());
    let captured_bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured_bodies.clone();

    // Serve one request in a background thread.
    std::thread::spawn(move || {
        if let Ok(mut req) = server.recv() {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(&mut req.as_reader(), &mut body);
            captured_clone.lock().unwrap().push(body);
            let _ = req.respond(tiny_http::Response::empty(200));
        }
    });

    let run_id = RunId::new();
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()));
    let notifier =
        Arc::new(MultiplexingNotifier::new().with_webhook(Arc::new(WebhookDeliverer::new())));

    let engine = Engine::new_with_notifier(
        bridge,
        storage,
        dispatcher,
        notifier,
        EngineConfig::default(),
    );

    let handle = engine
        .start_run(
            run_id,
            build_notify_webhook_graph(url),
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let outcome = handle.await_completion().await.expect("await_completion");
    match outcome {
        RunOutcome::Completed { .. } => {},
        other => panic!("expected Completed, got {other:?}"),
    }

    // Give the background thread time to process the request.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let bodies = captured_bodies.lock().unwrap().clone();
    assert!(
        !bodies.is_empty(),
        "expected at least one captured POST body"
    );

    let run_id_str = run_id.to_string();
    assert!(
        bodies[0].contains(&run_id_str),
        "captured body should contain run_id '{run_id_str}', got: {}",
        bodies[0]
    );
}
