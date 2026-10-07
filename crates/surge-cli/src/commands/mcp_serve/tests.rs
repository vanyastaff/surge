//! In-process tests for `surge mcp serve`: the real server handler is driven
//! over an in-memory duplex stream by a real rmcp client, against a temp
//! surge home (passed explicitly — no process-global `SURGE_HOME` mutation,
//! and no daemon, so a developer's real daemon can never be reached).

use crate::runtime_home_fixture::FixtureHome;
use std::collections::BTreeMap;
use std::path::Path;

use interprocess::local_socket::tokio::prelude::*;
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

use super::{
    Answer, AttentionGroup, PendingState, ResolveRequest, ServeOptions, SurgeMcpServer, TOOLS,
    ToolError, authorize_resolution, serve,
};
use surge_orchestrator::operator::{GateOption, PendingInput, PendingKind};

const ALL_TOOLS: [&str; 12] = [
    "surge_task_read",
    "surge_task_control",
    "surge_inbox",
    "surge_run_status",
    "surge_ready_tasks",
    "surge_ledger",
    "surge_run_report",
    "surge_run_trace",
    "surge_steer",
    "surge_resolve",
    "surge_bootstrap_start",
    "surge_memory_search",
];

type Client = RunningService<RoleClient, ClientInfo>;

/// Start a server over a duplex pipe and connect an rmcp client to it.
async fn connect(
    home: &Path,
    allow_write: bool,
) -> (Client, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let options = ServeOptions {
        home: home.to_path_buf(),
        project_root: home.to_path_buf(),
        allow_write,
    };
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let server_task = tokio::spawn(async move { serve(options, server_io).await });
    let client = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("surge-test-client", "0"),
    )
    .serve(client_io)
    .await
    .expect("client handshake");
    (client, server_task)
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

fn server(home: &Path, allow_write: bool) -> SurgeMcpServer {
    SurgeMcpServer::new(ServeOptions {
        home: home.to_path_buf(),
        project_root: home.to_path_buf(),
        allow_write,
    })
}

/// Assert `result` is an MCP tool error of `kind` and return its message.
fn expect_error(result: &CallToolResult, kind: &str) -> String {
    assert_eq!(result.is_error, Some(true), "expected a tool error");
    assert_eq!(structured(result)["error"]["kind"], kind, "{result:?}");
    text(result)
}

fn run_config() -> RunConfig {
    RunConfig {
        bootstrap_edit_loop_cap: None,
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
    writer.close().await.unwrap();
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
    writer.close().await.unwrap();
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_list_names_every_tool() {
    let home = FixtureHome::new().unwrap();
    {
        let (client, server_task) = connect(home.path(), false).await;

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
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_tools_work_on_an_empty_home_without_a_daemon() {
    let home = FixtureHome::new().unwrap();
    {
        let (client, server_task) = connect(home.path(), false).await;

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
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_run_is_a_tool_error_not_a_protocol_error() {
    let home = FixtureHome::new().unwrap();
    {
        let (client, server_task) = connect(home.path(), false).await;
        let missing = RunId::new().to_string();

        let status = call(&client, "surge_run_status", json!({"run_id": missing})).await;
        let message = expect_error(&status, "run_not_found");
        assert!(message.contains("no run"), "{message}");

        let report = call(&client, "surge_run_report", json!({"run_id": missing})).await;
        expect_error(&report, "run_not_found");

        let ready = call(&client, "surge_ready_tasks", json!({"run_id": missing})).await;
        expect_error(&ready, "run_not_found");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_and_unmatched_run_ids_are_caller_errors_not_faults() {
    let home = FixtureHome::new().unwrap();
    {
        seed_working_run(home.path()).await;
        let (client, server_task) = connect(home.path(), false).await;

        let too_short = call(&client, "surge_run_status", json!({"run_id": "abc"})).await;
        let message = expect_error(&too_short, "invalid_run_id");
        assert!(message.contains("too short"), "{message}");

        let unmatched = call(&client, "surge_run_status", json!({"run_id": "ZZZZZZZZ"})).await;
        expect_error(&unmatched, "run_not_found");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_server_refuses_every_mutating_tool() {
    let home = FixtureHome::new().unwrap();
    {
        let (client, server_task) = connect(home.path(), false).await;
        let run_id = RunId::new().to_string();

        let cases = [
            (
                "surge_task_control",
                json!({"action":"create","operation":surge_core::id::WorkItemOperationId::new(),"title":"Task","text":"Accepted requirements","criteria":["Works"]}),
            ),
            ("surge_steer", json!({"run_id": run_id, "message": "hi"})),
            (
                "surge_resolve",
                json!({"run_id": run_id, "decision": "approve", "expected_node": "plan_gate"}),
            ),
            ("surge_bootstrap_start", json!({"idea": "build a thing"})),
        ];
        // The e2e cases must cover exactly the tools the table marks mutating.
        let mut covered: Vec<&str> = cases.iter().map(|(tool, _)| *tool).collect();
        let mut mutating: Vec<&str> = TOOLS
            .iter()
            .filter(|t| t.mutating)
            .map(|t| t.name)
            .collect();
        covered.sort_unstable();
        mutating.sort_unstable();
        assert_eq!(covered, mutating);

        for (tool, args) in cases {
            let result = call(&client, tool, args).await;
            let message = expect_error(&result, "write_disabled");
            assert!(message.contains("--allow-write"), "{tool}: {message}");
        }
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutating_tools_report_daemon_not_running() {
    let home = FixtureHome::new().unwrap();
    {
        let run = seed_working_run(home.path()).await;
        let (client, server_task) = connect(home.path(), true).await;

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
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_run_is_surfaced_with_prompt_and_valid_decisions() {
    let home = FixtureHome::new().unwrap();
    {
        let run = seed_gate_run(home.path(), HumanGateMode::Generic).await;
        let (client, server_task) = connect(home.path(), false).await;

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
        assert_eq!(pending["prompt"], "Approve the plan?");
        assert!(structured(&status).get("note").is_none());
        let outcomes: Vec<&str> = pending["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["outcome"].as_str().unwrap())
            .collect();
        assert_eq!(outcomes, ["approve", "reject"]);
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_refuses_a_run_the_inbox_did_not_surface() {
    let home = FixtureHome::new().unwrap();
    {
        let run = seed_working_run(home.path()).await;
        let (client, server_task) = connect(home.path(), true).await;

        let result = call(
            &client,
            "surge_resolve",
            json!({"run_id": run.to_string(), "decision": "approve", "expected_node": "plan_gate"}),
        )
        .await;
        let message = expect_error(&result, "not_awaiting_input");
        assert!(message.contains("needs_input"), "{message}");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_validates_the_decision_before_touching_the_daemon() {
    let home = FixtureHome::new().unwrap();
    {
        let run = seed_gate_run(home.path(), HumanGateMode::Generic).await;
        let (client, server_task) = connect(home.path(), true).await;
        let run_id = run.to_string();

        let bogus = call(
            &client,
            "surge_resolve",
            json!({"run_id": run_id, "decision": "ship-it", "expected_node": "plan_gate"}),
        )
        .await;
        let message = expect_error(&bogus, "invalid_decision");
        assert!(message.contains("not valid"), "{message}");
        assert_eq!(
            structured(&bogus)["error"]["data"]["valid_decisions"],
            json!(["approve", "reject"])
        );

        let stale = call(
            &client,
            "surge_resolve",
            json!({"run_id": run_id, "decision": "approve", "expected_node": "other_gate"}),
        )
        .await;
        let message = expect_error(&stale, "stale_gate");
        assert!(message.contains("plan_gate"), "{message}");
        assert_eq!(
            structured(&stale)["error"]["data"],
            json!({"expected_node": "other_gate", "current_node": "plan_gate"})
        );

        // Every gate passed: only the (absent) daemon stands between the decision
        // and delivery, which proves the request reached the delivery step.
        let accepted = call(
        &client,
        "surge_resolve",
        json!({"run_id": run_id, "decision": "approve", "note": "lgtm", "expected_node": "plan_gate"}),
    )
    .await;
        expect_error(&accepted, "daemon_not_running");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_never_answers_a_bootstrap_approval_gate() {
    let home = FixtureHome::new().unwrap();
    {
        let run = seed_gate_run(
            home.path(),
            HumanGateMode::Bootstrap {
                stage: BootstrapStage::Roadmap,
            },
        )
        .await;
        let (client, server_task) = connect(home.path(), true).await;

        let result = call(
            &client,
            "surge_resolve",
            json!({"run_id": run.to_string(), "decision": "approve", "expected_node": "plan_gate"}),
        )
        .await;
        let message = expect_error(&result, "human_only_gate");
        assert!(message.contains("bootstrap approval"), "{message}");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_trace_exports_an_otlp_trace_for_a_seeded_run() {
    let home = FixtureHome::new().unwrap();
    {
        let run = seed_working_run(home.path()).await;
        let (client, server_task) = connect(home.path(), false).await;

        let trace = call(
            &client,
            "surge_run_trace",
            json!({"run_id": run.to_string()}),
        )
        .await;
        assert_eq!(trace.is_error, Some(false), "{trace:?}");
        let spans = structured(&trace)["trace"]["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .expect("OTLP spans");
        assert_eq!(spans[0]["name"], "surge.run");
        assert!(text(&trace).contains("span(s)"), "{}", text(&trace));

        let missing = call(
            &client,
            "surge_run_trace",
            json!({"run_id": "01ZZZZZZZZZZZZZZZZZZZZZZZZ"}),
        )
        .await;
        assert_eq!(missing.is_error, Some(true));
        assert_eq!(structured(&missing)["error"]["kind"], "run_not_found");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_report_compiles_for_a_seeded_run() {
    let home = FixtureHome::new().unwrap();
    {
        let run = seed_working_run(home.path()).await;
        let (client, server_task) = connect(home.path(), false).await;

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
        let one_line = text(&report);

        // `markdown` changes only the text; the structured report is the same.
        let markdown = call(
            &client,
            "surge_run_report",
            json!({"run_id": run.to_string(), "format": "markdown"}),
        )
        .await;
        assert_eq!(markdown.is_error, Some(false), "{markdown:?}");
        assert!(text(&markdown).len() > one_line.len());
        assert_eq!(structured(&markdown), structured(&report));

        // The old `json` / `md` spellings are gone.
        let old = client
            .call_tool(
                CallToolRequestParams::new("surge_run_report").with_arguments(
                    json!({"run_id": run.to_string(), "format": "md"})
                        .as_object()
                        .cloned()
                        .unwrap(),
                ),
            )
            .await;
        assert!(old.is_err(), "{old:?}");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_pending_input_is_null_when_not_blocked_and_tagged_when_blocked() {
    let home = FixtureHome::new().unwrap();
    {
        let working = seed_working_run(home.path()).await;
        let bootstrap_gate = seed_gate_run(
            home.path(),
            HumanGateMode::Bootstrap {
                stage: BootstrapStage::Flow,
            },
        )
        .await;
        let (client, server_task) = connect(home.path(), false).await;

        let status = call(
            &client,
            "surge_run_status",
            json!({"run_id": working.to_string()}),
        )
        .await;
        assert!(structured(&status)["pending_input"].is_null(), "{status:?}");

        let status = call(
            &client,
            "surge_run_status",
            json!({"run_id": bootstrap_gate.to_string()}),
        )
        .await;
        let pending = &structured(&status)["pending_input"];
        assert_eq!(pending["kind"], "bootstrap_approval");
        assert_eq!(pending["node"], "plan_gate");
        assert!(pending.get("options").is_none());
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_requires_expected_node() {
    let home = FixtureHome::new().unwrap();
    {
        let (client, server_task) = connect(home.path(), true).await;
        let arguments = json!({"run_id": RunId::new().to_string(), "decision": "approve"})
            .as_object()
            .cloned()
            .unwrap();
        let result = client
            .call_tool(CallToolRequestParams::new("surge_resolve").with_arguments(arguments))
            .await;
        assert!(result.is_err(), "{result:?}");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_range_limits_are_invalid_arguments_not_clamped() {
    let home = FixtureHome::new().unwrap();
    {
        let (client, server_task) = connect(home.path(), false).await;

        for (tool, args) in [
            ("surge_inbox", json!({"limit": 0})),
            ("surge_inbox", json!({"limit": 5001})),
            ("surge_ready_tasks", json!({"limit": -1})),
            ("surge_ledger", json!({"limit": 0})),
            ("surge_memory_search", json!({"query": "x", "limit": 51})),
        ] {
            let result = call(&client, tool, args.clone()).await;
            let message = expect_error(&result, "invalid_argument");
            assert!(message.contains("limit"), "{tool} {args}: {message}");
        }

        // The boundaries themselves are accepted.
        let inbox = call(&client, "surge_inbox", json!({"limit": 5000})).await;
        assert_eq!(inbox.is_error, Some(false), "{inbox:?}");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limit_schema_bounds_match_the_enforced_bounds() {
    let home = FixtureHome::new().unwrap();
    {
        let (client, server_task) = connect(home.path(), false).await;
        let tools = client.list_all_tools().await.unwrap();
        for (tool, max) in [
            ("surge_inbox", super::MAX_ROW_LIMIT),
            ("surge_ready_tasks", super::MAX_ROW_LIMIT),
            ("surge_ledger", super::MAX_ROW_LIMIT),
            ("surge_memory_search", super::MAX_MEMORY_LIMIT),
        ] {
            let schema = &tools.iter().find(|t| t.name == tool).unwrap().input_schema;
            let limit = &schema["properties"]["limit"];
            assert_eq!(limit["minimum"], 1, "{tool}: {limit}");
            assert_eq!(limit["maximum"], max, "{tool}: {limit}");
        }
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listings_report_total_and_truncation() {
    let home = FixtureHome::new().unwrap();
    {
        for _ in 0..3 {
            seed_working_run(home.path()).await;
        }
        let (client, server_task) = connect(home.path(), false).await;

        let inbox = call(&client, "surge_inbox", json!({})).await;
        let done = &structured(&inbox)["done"];
        assert_eq!(done["total"], 0);
        assert_eq!(done["truncated"], false);
        assert!(
            done.get("runs").is_none(),
            "runs omitted without include_done"
        );

        let inbox = call(&client, "surge_inbox", json!({"include_done": true})).await;
        assert_eq!(structured(&inbox)["done"]["runs"], json!([]));

        let ready = call(&client, "surge_ready_tasks", json!({})).await;
        let ready = structured(&ready);
        assert_eq!(ready["count"], 0);
        assert_eq!(ready["total"], 0);
        assert_eq!(ready["truncated"], false);

        let memory = call(&client, "surge_memory_search", json!({"query": "x"})).await;
        let memory = structured(&memory);
        assert_eq!(
            (memory["count"].clone(), memory["truncated"].clone()),
            (json!(0), json!(false))
        );
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_summary_counts_recovery_and_unconfirmed_runs() {
    let home = FixtureHome::new().unwrap();
    {
        let recovering = seed_working_run(home.path()).await;
        let storage = Storage::open(home.path()).await.unwrap();
        storage
            .set_run_status(&recovering, surge_core::RunStatus::Crashed, None)
            .await
            .unwrap();
        let unknown = RunId::new();
        storage
            .create_run(unknown, home.path(), None)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
        let (client, server_task) = connect(home.path(), false).await;
        let result = call(&client, "surge_inbox", json!({})).await;
        assert_eq!(structured(&result)["recovery"].as_array().unwrap().len(), 1);
        assert_eq!(structured(&result)["unknown"].as_array().unwrap().len(), 1);
        assert!(text(&result).contains("1 recovery, 1 unconfirmed"));
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blank_arguments_are_invalid_arguments() {
    let home = FixtureHome::new().unwrap();
    {
        let run = seed_working_run(home.path()).await;
        let (client, server_task) = connect(home.path(), true).await;

        let steer = call(
            &client,
            "surge_steer",
            json!({"run_id": run.to_string(), "message": "  "}),
        )
        .await;
        expect_error(&steer, "invalid_argument");

        let bootstrap = call(&client, "surge_bootstrap_start", json!({"idea": " "})).await;
        expect_error(&bootstrap, "invalid_argument");

        let memory = call(&client, "surge_memory_search", json!({"query": ""})).await;
        expect_error(&memory, "invalid_argument");
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_mutating_tool_is_refused_by_the_write_guard() {
    let home = FixtureHome::new().unwrap();
    {
        let read_only = server(home.path(), false);
        let writable = server(home.path(), true);
        for spec in TOOLS.iter().filter(|spec| spec.mutating) {
            let refused = read_only.begin_mutation(spec.name, "test").err();
            assert!(
                matches!(refused, Some(ToolError::WriteDisabled(tool)) if tool == spec.name),
                "{}: {refused:?}",
                spec.name
            );
            assert!(writable.begin_mutation(spec.name, "test").is_ok());
        }
    }
    home.close().unwrap();
}

#[test]
fn tool_table_matches_the_registered_router() {
    let home = FixtureHome::new().unwrap();
    {
        let registered = server(home.path(), false).tool_router.list_all();
        assert_eq!(registered.len(), TOOLS.len());
        for spec in TOOLS.iter() {
            let tool = registered
                .iter()
                .find(|tool| tool.name == spec.name)
                .unwrap_or_else(|| panic!("{} is not registered", spec.name));
            let read_only = tool
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint);
            assert_eq!(read_only, Some(!spec.mutating), "{}", spec.name);
        }
    }
    home.close().unwrap();
}

fn pending_input(is_tool_call: bool, is_bootstrap_gate: bool, options: &[&str]) -> PendingState {
    let kind = if is_bootstrap_gate {
        PendingKind::BootstrapGate
    } else if is_tool_call {
        PendingKind::ToolCall {
            call_id: "call-1".into(),
        }
    } else {
        PendingKind::Gate {
            call_id: None,
            options: options
                .iter()
                .map(|key| GateOption {
                    outcome: (*key).to_owned(),
                    label: key.to_uppercase(),
                })
                .collect(),
        }
    };
    PendingState::Input(PendingInput {
        node: NodeKey::try_from("plan_gate").unwrap(),
        prompt: "Approve?".into(),
        kind,
    })
}

fn request<'a>(
    expected_node: &'a str,
    decision: &'a str,
    note: Option<&'a str>,
) -> ResolveRequest<'a> {
    ResolveRequest {
        expected_node,
        decision,
        note,
    }
}

#[test]
fn authorize_requires_the_run_to_be_in_needs_input() {
    let pending = pending_input(false, false, &["approve"]);
    for attention in [
        AttentionGroup::Working,
        AttentionGroup::Waiting,
        AttentionGroup::Done,
    ] {
        let refused =
            authorize_resolution(attention, &pending, &request("plan_gate", "approve", None));
        assert!(
            matches!(refused, Err(ToolError::NotAwaitingInput(_))),
            "{attention:?}: {refused:?}"
        );
    }
}

#[test]
fn authorize_never_answers_a_bootstrap_approval() {
    for pending in [
        pending_input(false, true, &["approve"]),
        PendingState::BootstrapApproval,
    ] {
        let refused = authorize_resolution(
            AttentionGroup::NeedsInput,
            &pending,
            &request("plan_gate", "approve", None),
        );
        assert!(
            matches!(refused, Err(ToolError::HumanOnlyGate(_))),
            "{pending:?}: {refused:?}"
        );
    }
}

#[test]
fn authorize_refuses_a_stale_node_and_names_the_current_one() {
    let pending = pending_input(false, false, &["approve"]);
    let refused = authorize_resolution(
        AttentionGroup::NeedsInput,
        &pending,
        &request("older_gate", "approve", None),
    );
    match refused {
        Err(ToolError::StaleGate {
            expected_node,
            current_node,
        }) => assert_eq!(
            (expected_node.as_str(), current_node.as_str()),
            ("older_gate", "plan_gate")
        ),
        other => panic!("expected StaleGate, got {other:?}"),
    }
}

#[test]
fn authorize_only_accepts_a_declared_decision() {
    let pending = pending_input(false, false, &["approve", "reject"]);
    let refused = authorize_resolution(
        AttentionGroup::NeedsInput,
        &pending,
        &request("plan_gate", "ship-it", None),
    );
    match refused {
        Err(ToolError::InvalidDecision {
            valid_decisions, ..
        }) => assert_eq!(valid_decisions, ["approve", "reject"]),
        other => panic!("expected InvalidDecision, got {other:?}"),
    }

    let no_options = pending_input(false, false, &[]);
    let refused = authorize_resolution(
        AttentionGroup::NeedsInput,
        &no_options,
        &request("plan_gate", "approve", None),
    );
    assert!(
        matches!(refused, Err(ToolError::InvalidDecision { .. })),
        "{refused:?}"
    );

    let accepted = authorize_resolution(
        AttentionGroup::NeedsInput,
        &pending,
        &request("plan_gate", " reject ", Some("no")),
    )
    .unwrap();
    assert_eq!(accepted.answer, Answer::Outcome("reject".into()));
    assert_eq!(accepted.note, Some("no"));
}

#[test]
fn authorize_takes_free_form_answers_for_tool_calls_and_rejects_a_note() {
    let pending = pending_input(true, false, &[]);
    let accepted = authorize_resolution(
        AttentionGroup::NeedsInput,
        &pending,
        &request("plan_gate", "use postgres", None),
    )
    .unwrap();
    assert_eq!(accepted.answer, Answer::FreeForm("use postgres".into()));

    let with_note = authorize_resolution(
        AttentionGroup::NeedsInput,
        &pending,
        &request("plan_gate", "use postgres", Some("fyi")),
    );
    assert!(
        matches!(with_note, Err(ToolError::InvalidArgument(_))),
        "{with_note:?}"
    );

    let blank = authorize_resolution(
        AttentionGroup::NeedsInput,
        &pending,
        &request("plan_gate", "  ", None),
    );
    assert!(
        matches!(blank, Err(ToolError::InvalidArgument(_))),
        "{blank:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_task_mcp_writes_reach_daemon_and_amendments_remain_proposals() {
    use std::sync::Arc;
    use surge_orchestrator::engine::{
        Engine, EngineConfig, facade::LocalEngineFacade, tools::worktree::WorktreeToolDispatcher,
    };
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(project.path()).unwrap();
    let oid = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(oid).unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.com").unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
        .unwrap();
    drop(tree);
    drop(repo);
    let storage = Storage::open(home.path()).await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(surge_acp::bridge::acp_bridge::AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let socket = surge_daemon::pidfile::socket_path_in(home.path());
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let host = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 2,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        surge_daemon::tracked_run::TrackingContext::new(engine, storage.clone()),
        Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        Arc::new(surge_daemon::admission::AdmissionController::new(2, 2)),
        shutdown.clone(),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let name = surge_orchestrator::engine::ipc::local_socket_name_from_path(&socket)
                .expect("fixture IPC name");
            if LocalSocketStream::connect(name).await.is_ok() {
                break;
            }
            assert!(
                !host.is_finished(),
                "daemon exited before accepting connections"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("daemon accepts the actual IPC connection");
    let options = ServeOptions {
        home: home.path().into(),
        project_root: project.path().into(),
        allow_write: true,
    };
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let server_task = tokio::spawn(async move { serve(options, server_io).await });
    let client = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("task-test", "0"),
    )
    .serve(client_io)
    .await
    .unwrap();
    let created=call(&client,"surge_task_control",json!({"action":"create","operation":surge_core::id::WorkItemOperationId::new(),"title":"Persistent","text":"Accepted original","criteria":["Fixed criterion"]})).await;
    assert!(!created.is_error.unwrap_or(false), "{created:?}");
    let data = created.structured_content.unwrap();
    let item = data["value"]["item"]["id"].clone();
    let proposed=call(&client,"surge_task_control",json!({"action":"discuss","operation":surge_core::id::WorkItemOperationId::new(),"item":item,"version":1,"body":"Proposed change","proposal_text":"Unaccepted amendment","proposal_criteria":["Different criterion"]})).await;
    assert!(!proposed.is_error.unwrap_or(false), "{proposed:?}");
    let shown = call(
        &client,
        "surge_task_read",
        json!({"action":"show","item":item}),
    )
    .await;
    let data = shown.structured_content.unwrap();
    assert_eq!(
        data["value"]["revision"]["requirements"]["text"],
        "Accepted original"
    );
    let attempts = call(
        &client,
        "surge_task_read",
        json!({"action":"attempts","item":item,"after":null,"limit":2}),
    )
    .await;
    assert_eq!(
        attempts.structured_content.unwrap()["value"]["entries"],
        json!([])
    );
    let rejected = client
        .call_tool(
            CallToolRequestParams::new("surge_task_control").with_arguments(
                json!({"action":"edit","item":item,"requirements":{}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await;
    assert!(rejected.is_err());
    shutdown.cancel();
    host.await.unwrap().unwrap();
    client.cancel().await.unwrap();
    server_task.await.unwrap().unwrap();
    drop(storage);
    drop(project);
    home.close().unwrap();
}
