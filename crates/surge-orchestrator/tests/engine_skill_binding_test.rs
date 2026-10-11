//! Integration test: a node's declared skills bind through the real engine
//! harness (`Engine::start_run` + `MockBridge`), not by calling
//! `engine::stage::skill_binding::bind_skills` directly.
//!
//! Covers Seam #2 ("existing engine harness") for:
//! - `default_skill_roots` actually resolving a pack from
//!   `<worktree>/.claude/skills` (not a fixture that bypasses root
//!   resolution).
//! - The skill-trust prompt going out as `HumanInputRequested` (the path
//!   `surge inbox` / the Telegram cockpit already render) and, when
//!   unanswered, denying the node before any ACP session opens — proven by
//!   `MockBridge` never recording an `OpenSession` call.
//! - The approved path reaching all the way to the agent's prompt: bound
//!   skill instructions show up in `MockBridge::last_prompt()`.

mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::AgentConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::skill::{SkillProvider, SkillRef};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

// Skills can scan the real provider corpus before subscribing. Keep that
// allowance local, and observe the run concurrently so a failed pump never
// leaves await_completion waiting until the runner kills the process.
async fn skill_pump(mock: Arc<fixtures::mock_bridge::MockBridge>, expected: usize) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while mock.subscribe_count() < expected {
            tokio::task::yield_now().await;
        }
        mock.pump_scripted_events().await;
    })
    .await
    .expect("skills fixture subscription readiness");
}

struct SkillRunWatch<'a> {
    engine: &'a Engine,
    storage: &'a Arc<Storage>,
    run: RunId,
    completion: tokio::task::JoinHandle<RunOutcome>,
    pumps: tokio::task::JoinSet<()>,
    outcome: Option<RunOutcome>,
    completion_done: bool,
    deadline: tokio::time::Instant,
}
impl<'a> SkillRunWatch<'a> {
    fn new(
        engine: &'a Engine,
        storage: &'a Arc<Storage>,
        handle: surge_orchestrator::engine::RunHandle,
        pump: impl std::future::Future<Output = ()> + Send + 'static,
        deadline: tokio::time::Instant,
    ) -> Self {
        let mut pumps = tokio::task::JoinSet::new();
        pumps.spawn(pump);
        Self {
            engine,
            storage,
            run: handle.run_id,
            completion: handle.completion,
            pumps,
            outcome: None,
            completion_done: false,
            deadline,
        }
    }
    fn add_pump(&mut self, pump: impl std::future::Future<Output = ()> + Send + 'static) {
        self.pumps.spawn(pump);
    }
    async fn fail(&mut self, reason: String) -> ! {
        self.pumps.abort_all();
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while self.pumps.join_next().await.is_some() {}
        })
        .await;
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            self.engine
                .stop_run(self.run, "skills fixture cleanup".into()),
        )
        .await;
        if !self.completion_done
            && tokio::time::timeout(Duration::from_secs(5), &mut self.completion)
                .await
                .is_err()
        {
            self.completion.abort();
            let _ = tokio::time::timeout(Duration::from_secs(1), &mut self.completion).await;
        }
        let prefix = tokio::time::timeout(Duration::from_secs(2), async {
            let reader = self.storage.open_run_reader(self.run).await.ok()?;
            let events = reader
                .read_events(
                    surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(128),
                )
                .await
                .ok()?;
            Some(
                events
                    .iter()
                    .map(|row| (row.seq.as_u64(), row.kind.clone()))
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .ok()
        .flatten();
        panic!("{reason}; durable seq/kind prefix: {prefix:?}");
    }
    async fn gate(&mut self, index: usize) -> (NodeKey, surge_core::id::GateRequestId) {
        let until = self
            .deadline
            .min(tokio::time::Instant::now() + Duration::from_secs(30));
        loop {
            if let Some(outcome) = &self.outcome {
                self.fail(format!(
                    "run terminated before skill gate {index}: {outcome:?}"
                ))
                .await;
            }
            let storage = self.storage;
            let run = self.run;
            let query = async move {
                let reader = storage
                    .open_run_reader(run)
                    .await
                    .map_err(|error| error.to_string())?;
                let events = reader
                    .read_events(
                        surge_persistence::runs::EventSeq(0)
                            ..surge_persistence::runs::EventSeq(128),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>(
                    events
                        .iter()
                        .filter_map(|event| match event.payload.payload() {
                            surge_core::EventPayload::HumanInputRequested {
                                node,
                                call_id: Some(id),
                                ..
                            } => surge_core::id::GateRequestId::from_event_call_id(id)
                                .map(|request| (node.clone(), request)),
                            _ => None,
                        })
                        .nth(index),
                )
            };
            tokio::select! {
                result = &mut self.completion => {
                    self.completion_done = true;
                    match result {
                    Ok(outcome) => self.outcome = Some(outcome),
                    Err(error) => self.fail(format!("skills run join failed: {error}")).await,
                    }
                },
                result = self.pumps.join_next(), if !self.pumps.is_empty() => {
                    if let Some(Err(error)) = result {
                        self.fail(format!("skills pump failed: {error}")).await;
                    }
                },
                () = tokio::time::sleep_until(until) => self.fail("skill gate deadline".into()).await,
                found = query => {
                    match found {
                        Ok(Some(found)) => return found,
                        Ok(None) => {},
                        Err(error) => self.fail(format!("skill gate inspection failed: {error}")).await,
                    }
                    tokio::task::yield_now().await;
                },
            }
        }
    }
    async fn approve(&mut self, node: NodeKey, request: surge_core::id::GateRequestId) {
        let until = self
            .deadline
            .min(tokio::time::Instant::now() + Duration::from_secs(30));
        let engine = self.engine;
        let action = engine.resolve_gate_input(
            self.run,
            node,
            request,
            serde_json::json!({"outcome": "approve"}),
        );
        tokio::pin!(action);
        loop {
            tokio::select! {
                result = &mut self.completion, if !self.completion_done => {
                    self.completion_done = true;
                    match result {
                        Ok(outcome) => self.outcome = Some(outcome),
                        Err(error) => self.fail(format!("skills run join failed: {error}")).await,
                    }
                },
                result = self.pumps.join_next(), if !self.pumps.is_empty() => {
                    if let Some(Err(error)) = result {
                        self.fail(format!("skills pump failed: {error}")).await;
                    }
                },
                result = &mut action => match result {
                    Ok(_) => return,
                    Err(error) => self.fail(format!("skill approval failed: {error}")).await,
                },
                () = tokio::time::sleep_until(until) => self.fail("skill approval deadline".into()).await,
            }
        }
    }
    async fn finish(mut self) -> RunOutcome {
        loop {
            if self.outcome.is_some() && self.pumps.is_empty() {
                return self.outcome.take().unwrap();
            }
            tokio::select! {
                result = &mut self.completion, if !self.completion_done => {
                    self.completion_done = true;
                    match result {
                    Ok(outcome) => self.outcome = Some(outcome),
                    Err(error) => self.fail(format!("skills run join failed: {error}")).await,
                    }
                },
                result = self.pumps.join_next(), if !self.pumps.is_empty() => {
                    if let Some(Err(error)) = result {
                        self.fail(format!("skills pump failed: {error}")).await;
                    }
                },
                () = tokio::time::sleep_until(self.deadline) => self.fail("whole skills fixture deadline".into()).await,
            }
        }
    }
}

impl Drop for SkillRunWatch<'_> {
    fn drop(&mut self) {
        self.pumps.abort_all();
        if !self.completion_done {
            self.completion.abort();
        }
    }
}

/// Writes a single Agent Skills pack (`SKILL.md` only) under
/// `<worktree>/.claude/skills/<dir_name>/SKILL.md` — the real
/// `default_skill_roots` layout, not a fixture path the production roots
/// never look at.
fn write_project_skill(worktree: &std::path::Path, dir_name: &str, name: &str, body: &str) {
    let pack_dir = worktree.join(".claude/skills").join(dir_name);
    std::fs::create_dir_all(&pack_dir).unwrap();
    std::fs::write(
        pack_dir.join("SKILL.md"),
        format!("---\nname: {name}\n---\n\n{body}\n"),
    )
    .unwrap();
}

/// Writes an Agent Plugins package under
/// `<worktree>/.claude/plugins/<package_dir>/.claude-plugin/plugin.json` +
/// `.../skills/<skill_name>/SKILL.md` — the real corpus layout measured on
/// `~/.claude/plugins` (all 352 `SKILL.md` files and all 47 plugin
/// manifests live under `plugins/`, not `skills/`). A physically different
/// on-disk shape from `write_project_skill` above, so a test using this one
/// actually exercises the `.claude/plugins` root end to end.
fn write_project_plugin_skill(
    worktree: &std::path::Path,
    package_dir: &str,
    skill_name: &str,
    body: &str,
) {
    let package = worktree.join(".claude/plugins").join(package_dir);
    std::fs::create_dir_all(package.join(".claude-plugin")).unwrap();
    std::fs::write(
        package.join(".claude-plugin/plugin.json"),
        r#"{"version":"1.0.0"}"#,
    )
    .unwrap();
    let skill_dir = package.join("skills").join(skill_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {skill_name}\n---\n\n{body}\n"),
    )
    .unwrap();
}

/// A single-node graph: one Agent node declaring `skills`, wired to a
/// Terminal success node on its `"done"` outcome. Equivalent to
/// `graph_with_declared_skill_and_approval(declared, true)` — the gate
/// active, matching a node with no `approvals_override` at all.
fn graph_with_declared_skill(declared: Vec<SkillRef>) -> Graph {
    graph_with_declared_skill_and_approval(declared, true)
}

/// Like [`graph_with_declared_skill`], but with the trust gate explicitly
/// enabled or disabled via a node-level `approvals_override`.
fn graph_with_declared_skill_and_approval(declared: Vec<SkillRef>, gate_enabled: bool) -> Graph {
    let agent_key = NodeKey::try_from("implement").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();

    let mut custom_fields = BTreeMap::new();
    custom_fields.insert(
        "skills".to_string(),
        toml::Value::try_from(&declared).unwrap(),
    );

    let approvals_override = if gate_enabled {
        None
    } else {
        Some(surge_core::approvals::ApprovalConfig {
            skill_approval: false,
            ..surge_core::approvals::ApprovalConfig::default()
        })
    };

    let mut nodes = BTreeMap::new();
    nodes.insert(
        agent_key.clone(),
        Node {
            id: agent_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "stage completed".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("implementer@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks: vec![],
                custom_fields,
            }),
        },
    );
    nodes.insert(
        end_key.clone(),
        Node {
            id: end_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("skill-binding-harness", chrono::Utc::now()),
        start: agent_key.clone(),
        nodes,
        edges: vec![Edge {
            id: EdgeKey::try_from("e_done").unwrap(),
            from: PortRef {
                node: agent_key,
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: end_key,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }],
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpinned_skill_unanswered_rejects_before_any_session_opens() {
    let dir = FixtureHome::new().unwrap();
    let memory_dir = FixtureHome::new().unwrap();
    {
        write_project_skill(dir.path(), "reviewer", "code-reviewer", "Review carefully.");
        // This test drives the run to a genuine `RunOutcome::Failed` (skill
        // approval rejected), which trips
        // `engine::hooks::memory_writeback::record_node_failure` — route it at
        // a throwaway store instead of the developer's real `~/.surge/memory.db`.

        let store_path = memory_dir.path().join("memory.db");

        let storage = Storage::open(dir.path()).await.unwrap();
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;

        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None, // unpinned — always requires approval
        }];

        let run_id = RunId::new();
        let run_config = EngineRunConfig {
            human_input_timeout: Duration::from_millis(50),
            memory_store_path: Some(store_path),
            ..EngineRunConfig::default()
        };
        let handle = engine
            .start_run(
                run_id,
                graph_with_declared_skill(declared),
                dir.path().to_path_buf(),
                run_config,
            )
            .await
            .expect("start_run");

        // Nobody ever calls `engine.resolve_human_input` — the operator prompt
        // times out and the node must never start.
        let outcome = handle.await_completion().await.unwrap();
        match outcome {
            RunOutcome::Failed { error } => {
                assert!(
                    error.contains("skill approval"),
                    "failure reason should name the skill-approval rejection: {error}"
                );
            },
            other => panic!("expected Failed, got {other:?}"),
        }

        assert!(
            mock.recorded_calls.lock().await.is_empty(),
            "an unapproved skill must reject before any bridge call — the node never started"
        );

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        let kinds: Vec<&str> = events
            .iter()
            .map(|re| re.payload.payload.discriminant_str())
            .collect();
        assert!(
            kinds.contains(&"HumanInputRequested"),
            "the skill-trust prompt must go out on the delivered path, got {kinds:?}"
        );
        assert!(
            kinds.contains(&"HumanInputTimedOut"),
            "an unanswered prompt must time out, got {kinds:?}"
        );
        assert!(
            !kinds.contains(&"SkillBound"),
            "a rejected skill must never be bound, got {kinds:?}"
        );
        assert!(
            !kinds.contains(&"SessionOpened"),
            "a rejected skill must never open an agent session, got {kinds:?}"
        );
    }
    memory_dir.close().unwrap();
    dir.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_skill_binds_and_instructions_reach_the_agent_prompt() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let dir = FixtureHome::new().unwrap();
    {
        write_project_skill(
            dir.path(),
            "reviewer",
            "code-reviewer",
            "Only ever review; never edit files directly.",
        );

        // Discover the real hash so the declared pin matches — no approval
        // round trip expected on this path.
        let catalog = surge_core::skill::SkillCatalog::discover(&[surge_core::skill::SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: dir.path().join(".claude/skills"),
        }]);
        let (_, real) = catalog
            .resolve(&SkillRef {
                name: "code-reviewer".into(),
                provider: SkillProvider::ProjectDir,
                version: None,
                hash: None,
            })
            .unwrap();

        let storage = Storage::open(dir.path()).await.unwrap();
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;

        // Auto-complete the agent stage once it opens a session, exactly like
        // `engine_agent_stage_unit.rs` does — this test cares about what the
        // prompt carried, not about driving a real ACP subprocess.
        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::try_from("done").unwrap(),
            summary: "ok".into(),
            artifacts_produced: vec![],

            verification_report: None,
        })
        .await;
        let pump = skill_pump(mock.clone(), 1);

        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: Some(real.hash),
        }];

        let run_id = RunId::new();
        let handle = engine
            .start_run(
                run_id,
                graph_with_declared_skill(declared),
                dir.path().to_path_buf(),
                EngineRunConfig::default(),
            )
            .await
            .expect("start_run");

        let outcome = SkillRunWatch::new(&engine, &storage, handle, pump, deadline)
            .finish()
            .await;
        match outcome {
            RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
            other => panic!("expected Completed, got {other:?}"),
        }

        let prompt = mock
            .last_prompt()
            .await
            .expect("agent stage must have sent a prompt");
        assert!(
            prompt.contains("Only ever review; never edit files directly."),
            "the bound skill's instructions must reach the agent's prompt, got: {prompt}"
        );

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        let kinds: Vec<&str> = events
            .iter()
            .map(|re| re.payload.payload.discriminant_str())
            .collect();
        assert!(
            kinds.contains(&"SkillBound"),
            "the pinned skill must bind, got {kinds:?}"
        );
        assert!(
            !kinds.contains(&"HumanInputRequested"),
            "a matching pin must never prompt, got {kinds:?}"
        );
    }
    dir.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_packaged_skill_binds_via_dot_claude_plugins_root() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    // Both other tests in this file place their pack under
    // `.claude/skills` — this one places it under the Agent Plugins
    // layout (`.claude/plugins/<pkg>/.claude-plugin/plugin.json` +
    // `skills/<name>/SKILL.md`) so the `.claude/plugins` root is actually
    // exercised end to end through the real engine harness, not just
    // asserted to be present in `default_skill_roots`'s return value.
    let dir = FixtureHome::new().unwrap();
    {
        write_project_plugin_skill(
            dir.path(),
            "my-plugin",
            "code-reviewer",
            "Plugin-packaged review instructions.",
        );

        let storage = Storage::open(dir.path()).await.unwrap();
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;

        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::try_from("done").unwrap(),
            summary: "ok".into(),
            artifacts_produced: vec![],

            verification_report: None,
        })
        .await;
        let pump = skill_pump(mock.clone(), 1);

        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

        // Unpinned but declared on a node with the gate explicitly disabled —
        // this test is about proving the `.claude/plugins` root resolves at
        // all, not re-covering the approval path already covered above.
        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let run_id = RunId::new();
        let handle = engine
            .start_run(
                run_id,
                graph_with_declared_skill_and_approval(declared, false),
                dir.path().to_path_buf(),
                EngineRunConfig::default(),
            )
            .await
            .expect("start_run");

        let outcome = SkillRunWatch::new(&engine, &storage, handle, pump, deadline)
            .finish()
            .await;
        match outcome {
            RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
            other => panic!("expected Completed, got {other:?}"),
        }

        let prompt = mock
            .last_prompt()
            .await
            .expect("agent stage must have sent a prompt");
        assert!(
            prompt.contains("Plugin-packaged review instructions."),
            "a skill packaged under .claude/plugins must still reach the agent's \
         prompt, got: {prompt}"
        );
    }
    dir.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approved_skill_completion_does_not_leave_a_human_gate_recovery_record() {
    approved_skill_completion_fixture(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approved_skill_gate_does_not_leak_into_legitimate_stage_revisit() {
    approved_skill_completion_fixture(true).await;
}

async fn approved_skill_completion_fixture(revisit: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let dir = FixtureHome::new().unwrap();
    {
        write_project_skill(
            dir.path(),
            "reviewer",
            "code-reviewer",
            "Only ever review; never edit files directly.",
        );

        let storage = Storage::open(dir.path()).await.unwrap();
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;

        // Auto-complete the agent stage once it opens a session, exactly like
        // `engine_agent_stage_unit.rs` does — this test cares about what the
        // prompt carried, not about driving a real ACP subprocess.
        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::try_from(if revisit { "again" } else { "done" }).unwrap(),
            summary: "ok".into(),
            artifacts_produced: vec![],

            verification_report: None,
        })
        .await;
        let pump = skill_pump(mock.clone(), 1);

        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let mut graph = graph_with_declared_skill(declared);
        if revisit {
            let node = NodeKey::try_from("implement").unwrap();
            graph
                .nodes
                .get_mut(&node)
                .unwrap()
                .declared_outcomes
                .push(OutcomeDecl {
                    id: "again".parse().unwrap(),
                    description: "revisit once".into(),
                    edge_kind_hint: EdgeKind::Backtrack,
                    is_terminal: false,
                    ledger_effect: Default::default(),
                });
            graph.edges.push(Edge {
                id: "e_revisit".parse().unwrap(),
                from: PortRef {
                    node: node.clone(),
                    outcome: "again".parse().unwrap(),
                },
                to: node,
                kind: EdgeKind::Backtrack,
                policy: EdgePolicy::default(),
            });
        }
        let run_id = RunId::new();
        let handle = engine
            .start_run(
                run_id,
                graph,
                dir.path().to_path_buf(),
                EngineRunConfig::default(),
            )
            .await
            .expect("start_run");

        let mut watch = SkillRunWatch::new(&engine, &storage, handle, pump, deadline);
        let request = watch.gate(0).await;
        watch.approve(request.0.clone(), request.1).await;
        if revisit {
            let second = watch.gate(1).await;
            assert_ne!(
                request.1, second.1,
                "a legitimate revisit owns a new decision"
            );
            watch.approve(request.0.clone(), request.1).await;
            let second_session = SessionId::new();
            mock.pin_next_session_id(second_session).await;
            mock.enqueue_event(BridgeEvent::OutcomeReported {
                session: second_session,
                outcome: "done".parse().unwrap(),
                summary: "second stage".into(),
                artifacts_produced: vec![],
                verification_report: None,
            })
            .await;
            watch.add_pump(skill_pump(mock.clone(), 2));
            watch.approve(second.0, second.1).await;
        }
        let outcome = watch.finish().await;
        match outcome {
            RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
            other => panic!("expected Completed, got {other:?}"),
        }

        let prompt = mock
            .last_prompt()
            .await
            .expect("agent stage must have sent a prompt");
        assert!(
            prompt.contains("Only ever review; never edit files directly."),
            "the bound skill's instructions must reach the agent's prompt, got: {prompt}"
        );

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        let kinds: Vec<&str> = events
            .iter()
            .map(|re| re.payload.payload.discriminant_str())
            .collect();
        assert!(
            kinds.contains(&"SkillBound"),
            "the pinned skill must bind, got {kinds:?}"
        );
        assert!(kinds.contains(&"HumanInputRequested"));
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == "HumanInputRequested")
                .count(),
            if revisit { 2 } else { 1 }
        );
        assert_eq!(events.iter().filter(|event|matches!(event.payload.payload(), surge_core::EventPayload::StageEntered { node, .. } if node.as_str()=="implement")).count(), if revisit {2} else {1});
        let mut memory = surge_core::run_state::RunMemory::default();
        for event in events {
            memory.apply_event(&surge_core::RunEvent {
                run_id,
                seq: event.seq.as_u64(),
                timestamp: chrono::DateTime::from_timestamp_millis(event.timestamp_ms).unwrap(),
                payload: event.payload.payload().clone(),
            });
        }
        assert!(
            memory.gate_decisions.is_empty(),
            "completed skill approval is not an outstanding HumanGate route: {:?}",
            memory.gate_decisions
        );
    }
    dir.close().unwrap();
}
