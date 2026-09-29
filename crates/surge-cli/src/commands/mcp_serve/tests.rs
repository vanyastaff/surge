//! In-process tests for `surge mcp serve`: the real server handler is driven
//! over an in-memory duplex stream by a real rmcp client, against a temp
//! surge home (passed explicitly — no process-global `SURGE_HOME` mutation,
//! and no daemon, so a developer's real daemon can never be reached).

use std::collections::BTreeMap;
use std::path::Path;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo};
use rmcp::model::{Implementation, RawContent};
use rmcp::service::{RoleClient, RunningService};
use serde_json::{Value, json};
use surge_core::approvals::{ApprovalChannel, ApprovalDuration, ApprovalPolicy};
use surge_core::content_hash::ContentHash;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::human_gate_config::{
    ApprovalOption, HumanGateConfig, HumanGateMode, OptionStyle, SummaryTemplate, TimeoutAction,
};
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::{BootstrapStage, EventPayload, RunConfig, VersionedEventPayload};
use surge_core::sandbox::SandboxMode;
use surge_persistence::runs::Storage;

use super::{ServeOptions, serve};

const ALL_TOOLS: [&str; 9] = [
    "surge_inbox",
    "surge_run_status",
    "surge_ready_tasks",
    "surge_ledger",
    "surge_run_report",
    "surge_steer",
    "surge_resolve",
    "surge_bootstrap_start",
    "surge_memory_search",
];

type Client = RunningService<RoleClient, ClientInfo>;

/// Start a server over a duplex pipe and connect an rmcp client to it.
async fn connect(home: &Path, allow_write: bool) -> Client {
    let options = ServeOptions {
        home: home.to_path_buf(),
        project_root: home.to_path_buf(),
        allow_write,
    };
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let _ = serve(options, server_io).await;
    });
    ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("surge-test-client", "0"),
    )
    .serve(client_io)
    .await
    .expect("client handshake")
}

async fn call(client: &Client, tool: &'static str, arguments: Value) -> CallToolResult {
    let arguments = arguments.as_object().cloned().expect("object arguments");
    client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .expect("tool call is not a protocol error")
}

fn structured(result: &CallToolResult) -> &Value {
    result
        .structured_content
        .as_ref()
        .expect("structured content")
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            RawContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect()
}

/// Assert `result` is an MCP tool error of `kind` and return its message.
fn expect_error(result: &CallToolResult, kind: &str) -> String {
    assert_eq!(result.is_error, Some(true), "expected a tool error");
    assert_eq!(structured(result)["error"]["kind"], kind, "{result:?}");
    text(result)
}

fn run_config() -> RunConfig {
    RunConfig {
        budget: Default::default(),
        sandbox_default: SandboxMode::WorkspaceWrite,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: vec![],
    }
}

fn run_started(home: &Path) -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::RunStarted {
        pipeline_template: None,
        project_path: home.to_path_buf(),
        initial_prompt: "seed".into(),
        config: run_config(),
    })
}

/// A run that has started and is simply working (no gate).
async fn seed_working_run(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home, None).await.unwrap();
    writer.append_events(vec![run_started(home)]).await.unwrap();
    run
}

fn gate_graph(gate: &NodeKey, mode: HumanGateMode) -> Graph {
    let node = Node {
        id: gate.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::HumanGate(HumanGateConfig {
            delivery_channels: vec![ApprovalChannel::Desktop {
                duration: ApprovalDuration::Persistent,
            }],
            timeout_seconds: None,
            on_timeout: TimeoutAction::default(),
            summary: SummaryTemplate {
                title: "Approve?".into(),
                body: "body".into(),
                show_artifacts: vec![],
            },
            options: ["approve", "reject"]
                .into_iter()
                .map(|key| ApprovalOption {
                    outcome: OutcomeKey::try_from(key).unwrap(),
                    label: key.to_uppercase(),
                    style: OptionStyle::Normal,
                })
                .collect(),
            allow_freetext: false,
            mode,
        }),
    };
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "mcp-serve-test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: gate.clone(),
        nodes: BTreeMap::from([(gate.clone(), node)]),
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

/// A pipeline run blocked at a HumanGate named `gate` with `approve`/`reject`.
async fn seed_gate_run(home: &Path, mode: HumanGateMode) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home, None).await.unwrap();
    let gate = NodeKey::try_from("plan_gate").unwrap();
    let graph = gate_graph(&gate, mode);
    let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());
    writer
        .append_events(vec![
            run_started(home),
            VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash,
            }),
            VersionedEventPayload::new(EventPayload::StageEntered {
                node: gate.clone(),
                attempt: 1,
            }),
            VersionedEventPayload::new(EventPayload::HumanInputRequested {
                node: gate,
                session: None,
                call_id: None,
                prompt: "Approve the plan?".into(),
                schema: None,
            }),
        ])
        .await
        .unwrap();
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_list_names_every_tool() {
    let home = tempfile::tempdir().unwrap();
    let client = connect(home.path(), false).await;

    let mut names: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();
    let mut expected: Vec<String> = ALL_TOOLS.iter().map(ToString::to_string).collect();
    expected.sort();
    assert_eq!(names, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_tools_work_on_an_empty_home_without_a_daemon() {
    let home = tempfile::tempdir().unwrap();
    let client = connect(home.path(), false).await;

    let inbox = call(&client, "surge_inbox", json!({})).await;
    assert_eq!(inbox.is_error, Some(false), "{inbox:?}");
    assert_eq!(structured(&inbox)["done"]["count"], 0);
    assert_eq!(structured(&inbox)["needs_input"], json!([]));

    let ready = call(&client, "surge_ready_tasks", json!({})).await;
    assert_eq!(structured(&ready)["count"], 0);

    let ledger = call(&client, "surge_ledger", json!({})).await;
    assert_eq!(structured(&ledger)["count"], 0);

    let memory = call(&client, "surge_memory_search", json!({"query": "anything"})).await;
    assert_eq!(memory.is_error, Some(false), "{memory:?}");
    assert_eq!(structured(&memory)["total"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_run_is_a_tool_error_not_a_protocol_error() {
    let home = tempfile::tempdir().unwrap();
    let client = connect(home.path(), false).await;
    let missing = RunId::new().to_string();

    let status = call(&client, "surge_run_status", json!({"run_id": missing})).await;
    let message = expect_error(&status, "rejected");
    assert!(message.contains("no run"), "{message}");

    let report = call(&client, "surge_run_report", json!({"run_id": missing})).await;
    assert_eq!(report.is_error, Some(true), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_server_refuses_every_mutating_tool() {
    let home = tempfile::tempdir().unwrap();
    let client = connect(home.path(), false).await;
    let run_id = RunId::new().to_string();

    for (tool, args) in [
        ("surge_steer", json!({"run_id": run_id, "message": "hi"})),
        (
            "surge_resolve",
            json!({"run_id": run_id, "decision": "approve"}),
        ),
        ("surge_bootstrap_start", json!({"idea": "build a thing"})),
    ] {
        let result = call(&client, tool, args).await;
        let message = expect_error(&result, "write_disabled");
        assert!(message.contains("--allow-write"), "{tool}: {message}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutating_tools_report_daemon_not_running() {
    let home = tempfile::tempdir().unwrap();
    let run = seed_working_run(home.path()).await;
    let client = connect(home.path(), true).await;

    let steer = call(
        &client,
        "surge_steer",
        json!({"run_id": run.to_string(), "message": "use the staging db"}),
    )
    .await;
    let message = expect_error(&steer, "daemon_not_running");
    assert!(message.contains("daemon not running"), "{message}");

    let bootstrap = call(
        &client,
        "surge_bootstrap_start",
        json!({"idea": "a todo app"}),
    )
    .await;
    expect_error(&bootstrap, "daemon_not_running");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_run_is_surfaced_with_prompt_and_valid_decisions() {
    let home = tempfile::tempdir().unwrap();
    let run = seed_gate_run(home.path(), HumanGateMode::Generic).await;
    let client = connect(home.path(), false).await;

    let inbox = call(&client, "surge_inbox", json!({})).await;
    let blocked = &structured(&inbox)["needs_input"];
    assert_eq!(blocked.as_array().map(Vec::len), Some(1), "{inbox:?}");
    assert_eq!(blocked[0]["run_id"], run.to_string());
    assert_eq!(blocked[0]["prompt"], "Approve the plan?");

    let status = call(
        &client,
        "surge_run_status",
        json!({"run_id": run.to_string()}),
    )
    .await;
    let pending = &structured(&status)["pending_input"];
    assert_eq!(pending["node"], "plan_gate");
    assert_eq!(pending["kind"], "gate");
    let outcomes: Vec<&str> = pending["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["approve", "reject"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_refuses_a_run_the_inbox_did_not_surface() {
    let home = tempfile::tempdir().unwrap();
    let run = seed_working_run(home.path()).await;
    let client = connect(home.path(), true).await;

    let result = call(
        &client,
        "surge_resolve",
        json!({"run_id": run.to_string(), "decision": "approve"}),
    )
    .await;
    let message = expect_error(&result, "rejected");
    assert!(message.contains("needs_input"), "{message}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_validates_the_decision_before_touching_the_daemon() {
    let home = tempfile::tempdir().unwrap();
    let run = seed_gate_run(home.path(), HumanGateMode::Generic).await;
    let client = connect(home.path(), true).await;
    let run_id = run.to_string();

    let bogus = call(
        &client,
        "surge_resolve",
        json!({"run_id": run_id, "decision": "ship-it"}),
    )
    .await;
    let message = expect_error(&bogus, "rejected");
    assert!(message.contains("not valid"), "{message}");

    let stale = call(
        &client,
        "surge_resolve",
        json!({"run_id": run_id, "decision": "approve", "expected_node": "other_gate"}),
    )
    .await;
    let message = expect_error(&stale, "rejected");
    assert!(message.contains("plan_gate"), "{message}");

    // Every gate passed: only the (absent) daemon stands between the decision
    // and delivery, which proves the request reached the delivery step.
    let accepted = call(
        &client,
        "surge_resolve",
        json!({"run_id": run_id, "decision": "approve", "note": "lgtm"}),
    )
    .await;
    expect_error(&accepted, "daemon_not_running");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_never_answers_a_bootstrap_approval_gate() {
    let home = tempfile::tempdir().unwrap();
    let run = seed_gate_run(
        home.path(),
        HumanGateMode::Bootstrap {
            stage: BootstrapStage::Roadmap,
        },
    )
    .await;
    let client = connect(home.path(), true).await;

    let result = call(
        &client,
        "surge_resolve",
        json!({"run_id": run.to_string(), "decision": "approve"}),
    )
    .await;
    let message = expect_error(&result, "rejected");
    assert!(message.contains("bootstrap approval"), "{message}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_report_compiles_for_a_seeded_run() {
    let home = tempfile::tempdir().unwrap();
    let run = seed_working_run(home.path()).await;
    let client = connect(home.path(), false).await;

    let report = call(
        &client,
        "surge_run_report",
        json!({"run_id": run.to_string()}),
    )
    .await;
    assert_eq!(report.is_error, Some(false), "{report:?}");
    // The serialized report carries the bare ULID; `Display` adds a `run-` prefix.
    let reported = structured(&report)["report"]["run_id"].as_str().unwrap();
    assert!(run.to_string().ends_with(reported), "{reported}");
    assert!(text(&report).contains("incomplete"), "{}", text(&report));
}
