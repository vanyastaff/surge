//! Outer acceptance anchor: a real failed Bootstrap continues its original work.
//! The controlled provider proves protocol mechanics, not native Sol delivery.
use super::*;
use surge_core::graph::Graph;
use surge_core::node::NodeConfig;

const ORIGINAL_FLOW: &str = include_str!("bootstrap-continuation/flow.toml");
const ORIGINAL_ROADMAP: &str = include_str!("bootstrap-continuation/roadmap.toml");
const TASKS: [&str; 8] = [
    "m1-t1", "m1-t2", "m1-t3", "m1-t4", "m2-t1", "m2-t2", "m2-t3", "m2-t4",
];

pub(super) fn python() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("python")
    } else {
        PathBuf::from("/usr/bin/python3")
    }
}

fn fixture_graph() -> Graph {
    let mut graph: Graph = toml::from_str(ORIGINAL_FLOW).unwrap();
    // Only the provider/model selection changes for this synthetic ACP fixture.
    // Preserve every node, loop, binding, artifact contract, edge and retry limit.
    for node in graph.nodes.values_mut().chain(
        graph
            .subgraphs
            .values_mut()
            .flat_map(|body| body.nodes.values_mut()),
    ) {
        if let NodeConfig::Agent(agent) = &mut node.config {
            *agent = agent.with_runtime_override("wire-fixture", Some("sonnet"));
        }
    }
    graph
}

fn milestone_spec() -> String {
    let roadmap: toml::Value = toml::from_str(ORIGINAL_ROADMAP).unwrap();
    let milestone = &roadmap["milestones"][0];
    let subtasks: Vec<_> = milestone["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| {
            json!({"id":task["id"].as_str().unwrap(), "title":task["title"].as_str().unwrap(),
                "description":"Controlled acceptance fixture for the unchanged roadmap task.",
                "complexity":"small", "files":["tracked-sentinel.txt"],
                "acceptance_criteria":task["acceptance_criteria"].as_array().unwrap().iter()
                    .map(|criterion|json!({"description":criterion.as_str().unwrap()})).collect::<Vec<_>>()})
        })
        .collect();
    toml::to_string(&json!({"schema_version":1,"spec":{
        "id":"m1", "title":milestone["title"].as_str().unwrap(),
        "description":"Original first milestone, including all four tasks.",
        "complexity":"medium", "subtasks":subtasks}}))
    .unwrap()
}

fn milestone_spec_md() -> String {
    let roadmap: toml::Value = toml::from_str(ORIGINAL_ROADMAP).unwrap();
    let milestone = &roadmap["milestones"][0];
    let mut text = format!(
        "# Spec\n\n## Goal\n{}\n\n## Subtasks\n",
        milestone["title"].as_str().unwrap()
    );
    for task in milestone["tasks"].as_array().unwrap() {
        text.push_str(&format!(
            "- {}: {}\n",
            task["id"].as_str().unwrap(),
            task["title"].as_str().unwrap()
        ));
    }
    text.push_str("\n## Acceptance Criteria\n");
    for task in milestone["tasks"].as_array().unwrap() {
        for criterion in task["acceptance_criteria"].as_array().unwrap() {
            text.push_str(&format!("- [ ] {}\n", criterion.as_str().unwrap()));
        }
    }
    text.push_str("\n## Constraints\nPreserve original history and all eight roadmap tasks.\n");
    text
}

fn prepare_continuation(home: &Path, project: &Path) {
    let prerequisite = std::process::Command::new(python())
        .args(["--version"])
        .output()
        .expect("fixture Python prerequisite");
    assert!(prerequisite.status.success(), "fixture Python prerequisite");
    prepare(home, project);
    std::fs::write(
        project.join("tracked-sentinel.txt"),
        "original tracked bytes\n",
    )
    .unwrap();
    git(project, &["add", "tracked-sentinel.txt"]);
    git(project, &["commit", "-m", "tracked continuation sentinel"]);
    std::fs::write(
        home.join("artifact-proxy.py"),
        include_str!("bootstrap-continuation/artifact_proxy.py"),
    )
    .unwrap();
    for mut profile in surge_core::profile::bundled::BundledRegistry::all() {
        if ["spec-author", "implementer", "app-tester", "verifier"]
            .contains(&profile.role.id.as_str())
        {
            profile.runtime.agent_id = "wire-fixture".into();
            profile.runtime.recommended_model = "sonnet".into();
            // Mirror existing Bootstrap fixture hook discipline: the actual
            // in-process host artifact validator remains the acceptance owner.
            profile.hooks = Default::default();
            std::fs::write(
                home.join("profiles")
                    .join(format!("{}-{}.toml", profile.role.id, profile.role.version)),
                toml::to_string(&profile).unwrap(),
            )
            .unwrap();
        }
    }
    let script = json!([
        {"prompt_contains":"", "outcome":"drafted", "bootstrap_artifacts":{"description.md":include_str!("bootstrap-continuation/description.md")}},
        {"prompt_contains":"", "outcome":"drafted", "bootstrap_artifacts":{"roadmap.md":ROADMAP_MD,"roadmap.toml":ORIGINAL_ROADMAP}},
        {"prompt_contains":"", "outcome":"drafted", "bootstrap_artifacts":{"flow.toml":toml::to_string(&fixture_graph()).unwrap()}},
        {"prompt_contains":"", "outcome":"drafted", "fixture_files":{"spec.toml":milestone_spec(), "spec.md":milestone_spec_md()}, "fixture_artifacts":["spec.toml", "spec.md"]},
        {"prompt_contains":"m1-t1", "outcome":"ready_for_verification", "fixture_node":"impl_task", "fixture_task":"m1-t1", "fixture_files":{
            "tracked-sentinel.txt":"accepted Build dirty tracked bytes\n",
            "untracked-sentinel.txt":"accepted Build untracked bytes\n"}},
        // A genuine candidate reaches the real AppTester artifact contract and
        // is rejected. No report or terminal fact is forged by the fixture.
        {"prompt_contains":"m1-t1", "outcome":"passed"},
        {"prompt_contains":"verification-report.toml", "outcome":"passed"},
        {"prompt_contains":"verification-report.toml", "outcome":"passed"},
        {"prompt_contains":"verification-report.toml", "outcome":"passed"}
    ]);
    std::fs::write(
        home.join("bootstrap-script.json"),
        serde_json::to_vec(&script).unwrap(),
    )
    .unwrap();
    let config_path = project.join("surge.toml");
    let mut config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config["agents"]["wire-fixture"]["args"]
        .as_array_mut()
        .unwrap()
        .push(toml::Value::String("--config-options".into()));
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    git(project, &["add", "surge.toml"]);
    git(
        project,
        &[
            "commit",
            "-m",
            "freeze controlled provider recipe before admission",
        ],
    );
}

async fn raw_request(socket: &Path, frame: Value) -> Value {
    // Do not pre-decode a proposed request as the current production enum.
    let stream = LocalSocketStream::connect(local_socket_name_from_path(socket).unwrap())
        .await
        .unwrap();
    let (read, mut write) = stream.split();
    write
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        BufReader::new(read).read_line(&mut line),
    )
    .await
    .expect("continuation request must respond or observably close")
    .unwrap();
    if bytes == 0 {
        json!({"observation":"eof"})
    } else {
        serde_json::from_str(&line).unwrap()
    }
}

async fn failed_operation(host: &mut Host, operation: RunId) -> Value {
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            assert!(
                host.child.try_wait().unwrap().is_none(),
                "{}",
                host.diagnostic()
            );
            let response = rpc(
                &host.home.join("bootstrap.sock"),
                json!({"method":"bootstrap_status","request_id":3,"operation_id":operation}),
            )
            .await;
            assert_eq!(response["method"], "bootstrap_operation", "{response}");
            if response["status"]["state"]["state"] == "failed" {
                return response["status"].clone();
            }
            assert_ne!(
                response["status"]["state"]["state"],
                "needs_attention",
                "{response}: {}",
                host.diagnostic()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "failed implementation precondition: {error}: {}",
            host.diagnostic()
        )
    })
}

fn assert_source_prefix(home: &Path, implementation: RunId, events: &[RunEvent]) -> Value {
    let builds: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::StageOutcomeCommitted { commit }
                if commit.context().node.as_str() == "impl_task" =>
            {
                Some(commit.invocation())
            },
            _ => None,
        })
        .collect();
    assert_eq!(builds.len(), 1, "one actual accepted Build");
    let route = events
        .iter()
        .find(|event| {
            matches!(&event.payload,
        EventPayload::StageRouteCommitted { invocation, .. } if *invocation==builds[0])
        })
        .unwrap();
    let connection = rusqlite::Connection::open_with_flags(
        home.join("runs")
            .join(implementation.to_string())
            .join("events.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let blob: Vec<u8> = connection
        .query_row(
            "SELECT snapshot FROM graph_snapshots WHERE at_seq=?",
            [route.seq],
            |row| row.get(0),
        )
        .unwrap();
    let snapshot =
        surge_orchestrator::engine::snapshot::EngineSnapshot::deserialize(&blob).unwrap();
    assert_eq!(snapshot.cursor.node, "app_task");
    assert!(snapshot.pending_stage.is_none());
    assert!(snapshot.workspace_checkpoint.is_some());
    let loops: Vec<_> = snapshot
        .frames
        .iter()
        .filter_map(|frame| match frame {
            surge_orchestrator::engine::snapshot::SerializableFrame::Loop(frame) => Some(frame),
            _ => None,
        })
        .collect();
    assert_eq!(loops.len(), 2);
    let all_tasks: Vec<_> = loops[0]
        .items_json
        .iter()
        .flat_map(|milestone| milestone["tasks"].as_array().unwrap())
        .map(|task| task["id"].as_str().unwrap())
        .collect();
    assert_eq!(all_tasks, TASKS, "all original task identities and order");
    let published_graph = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::PipelineMaterialized { graph, .. } => Some(graph.as_ref()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        published_graph,
        &fixture_graph(),
        "exact approved graph except explicit mock runtime selection"
    );
    assert_eq!(loops[0].current_index, 0);
    assert_eq!(loops[1].current_index, 0);
    assert_eq!(
        loops[0]
            .items_json
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["m1", "m2"]
    );
    assert_eq!(
        loops[1]
            .items_json
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        TASKS[..4]
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::RunFailed { .. }))
    );
    assert!(events.iter().any(
        |event| matches!(&event.payload, EventPayload::OutcomeRejectedByHook { node, hook_id, .. }
        if node.as_str()=="app_task" && hook_id=="profile-artifact-contract:verification-report")
    ));
    assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::RunFailed { error } if error.contains("verification-report"))));
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::TaskVerified { .. } | EventPayload::LoopIterationCompleted { .. }
    )));
    serde_json::to_value(snapshot).unwrap()
}

fn assert_writer_groups_empty(events: &[RunEvent]) -> Vec<String> {
    let containers: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::SessionOpened {
                opened: Some(opened),
                ..
            } => opened
                .execution_writer
                .as_ref()
                .and_then(|writer| writer.container()),
            _ => None,
        })
        .collect();
    assert!(!containers.is_empty(), "actual process containers required");
    containers
        .iter()
        .map(|container| {
            let state = surge_acp::process_evidence::group_state(container);
            assert_eq!(state, surge_acp::process_evidence::GroupState::Empty);
            format!("coverage={:?}; state={state:?}", container.coverage())
        })
        .collect()
}

fn git_output(project: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct RegisteredWorktree<'a> {
    path: &'a str,
    head: Option<&'a str>,
    branch: Option<&'a str>,
}

fn git_worktree_records(output: &str) -> Result<Vec<RegisteredWorktree<'_>>, &'static str> {
    let output = output
        .strip_suffix("\0\0")
        .ok_or("unterminated worktree records")?;
    let mut records = Vec::new();
    for record in output.split("\0\0") {
        let mut fields = record.split('\0');
        let path = fields
            .next()
            .and_then(|field| field.strip_prefix("worktree "))
            .filter(|path| Path::new(path).is_absolute())
            .ok_or("invalid worktree path")?;
        let mut parsed = RegisteredWorktree {
            path,
            head: None,
            branch: None,
        };
        let mut flags = std::collections::HashSet::new();
        for field in fields {
            if let Some(head) = field.strip_prefix("HEAD ") {
                if parsed.head.replace(head).is_some()
                    || !matches!(head.len(), 40 | 64)
                    || !head.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err("invalid or duplicate worktree HEAD");
                }
            } else if let Some(branch) = field.strip_prefix("branch ") {
                if parsed.branch.replace(branch).is_some()
                    || !branch.starts_with("refs/heads/")
                    || branch == "refs/heads/"
                {
                    return Err("invalid or duplicate worktree branch");
                }
            } else if field == "bare"
                || field == "detached"
                || field == "locked"
                || field.starts_with("locked ")
                || field == "prunable"
                || field.starts_with("prunable ")
            {
                let key = field.split(' ').next().ok_or("invalid worktree flag")?;
                if !flags.insert(key) {
                    return Err("duplicate worktree flag");
                }
            } else {
                return Err("unknown worktree field");
            }
        }
        if (flags.contains("bare")
            && (parsed.head.is_some() || parsed.branch.is_some() || flags.contains("detached")))
            || (!flags.contains("bare")
                && (parsed.head.is_none()
                    || (parsed.branch.is_some() == flags.contains("detached"))))
        {
            return Err("contradictory worktree record");
        }
        records.push(parsed);
    }
    Ok(records)
}

fn registered_worktree(
    output: &str,
    expected: &Path,
    head: &str,
    branch: &str,
) -> Result<(), &'static str> {
    let expected = expected
        .canonicalize()
        .map_err(|_| "expected worktree unavailable")?;
    let mut matching = Vec::new();
    for record in git_worktree_records(output)? {
        let physical = Path::new(record.path)
            .canonicalize()
            .map_err(|_| "listed worktree unavailable")?;
        if physical == expected {
            matching.push(record);
        }
    }
    let [record] = matching.as_slice() else {
        return Err("worktree registration is not unique");
    };
    if record.head != Some(head) || record.branch != Some(branch) {
        return Err("worktree HEAD or branch differs");
    }
    Ok(())
}

fn registered_worktree_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String, String) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("repository");
    std::fs::create_dir(&project).unwrap();
    git(&project, &["init", "-q"]);
    git(
        &project,
        &["config", "user.email", "oracle@example.invalid"],
    );
    git(&project, &["config", "user.name", "Worktree oracle"]);
    std::fs::write(project.join("tracked.txt"), b"fixed registration fixture\n").unwrap();
    git(&project, &["add", "tracked.txt"]);
    git(&project, &["commit", "-q", "-m", "fixed registration"]);
    let workspace = root.path().join("work tree Юникод child");
    git(
        &project,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "registered-oracle",
            workspace.to_str().unwrap(),
        ],
    );
    let head = git_output(&workspace, &["rev-parse", "HEAD"]);
    let records = git_output(&project, &["worktree", "list", "--porcelain", "-z"]);
    (root, project, workspace, head.trim().into(), records)
}

#[test]
fn registered_worktree_uses_real_git_records_for_space_unicode_and_exact_identity() {
    let (root, _project, workspace, head, records) = registered_worktree_fixture();
    let branch = "refs/heads/registered-oracle";
    assert_eq!(
        registered_worktree(&records, &workspace, &head, branch),
        Ok(())
    );
    let sibling_prefix = root.path().join("work tree Юникод");
    std::fs::create_dir(&sibling_prefix).unwrap();
    assert_eq!(
        registered_worktree(&records, &sibling_prefix, &head, branch),
        Err("worktree registration is not unique")
    );
    assert_eq!(
        registered_worktree(&records, &workspace, &"0".repeat(head.len()), branch),
        Err("worktree HEAD or branch differs")
    );
    assert_eq!(
        registered_worktree(&records, &workspace, &head, "refs/heads/wrong-branch"),
        Err("worktree HEAD or branch differs")
    );
    assert_eq!(
        registered_worktree(&records, &root.path().join("missing"), &head, branch),
        Err("expected worktree unavailable")
    );
}

#[test]
fn registered_worktree_refuses_malformed_and_duplicate_real_git_records() {
    let (_root, _project, workspace, head, records) = registered_worktree_fixture();
    let branch = "refs/heads/registered-oracle";
    assert_eq!(
        registered_worktree(&records, &workspace, &head, branch),
        Ok(())
    );
    assert_eq!(
        registered_worktree(&records.repeat(2), &workspace, &head, branch),
        Err("worktree registration is not unique")
    );
    for (malformed, expected) in [
        (
            records.trim_end_matches('\0').to_owned(),
            "unterminated worktree records",
        ),
        (
            records.replacen("HEAD ", "HEAD invalid", 1),
            "invalid or duplicate worktree HEAD",
        ),
        (
            records.replacen("HEAD ", &format!("HEAD {head}\0HEAD "), 1),
            "invalid or duplicate worktree HEAD",
        ),
        (
            records.replacen("HEAD ", "unknown field\0HEAD ", 1),
            "unknown worktree field",
        ),
        (
            format!("worktree relative-path\0HEAD {head}\0branch {branch}\0\0"),
            "invalid worktree path",
        ),
    ] {
        assert_eq!(
            registered_worktree(&malformed, &workspace, &head, branch),
            Err(expected)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_bootstrap_continues_original_eight_tasks_without_rebuilding() {
    let home = FixtureHome::new().unwrap();
    let result = std::panic::AssertUnwindSafe(run_continuation(&home))
        .catch_unwind()
        .await;
    finish_home(home, result);
}

async fn run_continuation(home: &FixtureHome) {
    // Keep project inside retained fixture evidence when the intentional RED unwinds.
    let project = home.path().join("original-project");
    prepare_continuation(home.path(), &project);
    let project = project.canonicalize().unwrap();
    let mut host = Host::spawn_configured(home.path(), &project, false, None, true);
    ready(&mut host).await;
    let socket = home.path().join("bootstrap.sock");
    assert_eq!(
        rpc(&socket, json!({"method":"ping","request_id":1})).await["method"],
        "ping_ok"
    );
    let bootstrap = RunId::new();
    let intent = surge_core::bootstrap_operation::BootstrapIntent::new(
        project.clone(),
        include_str!("bootstrap-continuation/prompt.txt").into(),
        Default::default(),
    )
    .unwrap();
    let accepted = rpc(
        &socket,
        json!({"method":"start_bootstrap","request_id":2,
        "operation_id":bootstrap,"intent":intent}),
    )
    .await;
    assert_eq!(accepted["method"], "bootstrap_operation", "{accepted}");
    let planning: RunId =
        serde_json::from_value(accepted["status"]["planning_run"].clone()).unwrap();
    let implementation: RunId =
        serde_json::from_value(accepted["status"]["implementation_run"].clone()).unwrap();
    host.run = Some(planning);
    for node in ["description_gate", "roadmap_gate", "flow_gate"] {
        let (_, _, call) = gate(&mut host, node, 0).await;
        assert_eq!(
            answer(&host, node, &call, "approve").await["method"],
            "resolve_human_input_ok"
        );
    }
    let status = failed_operation(&mut host, bootstrap).await;
    let planning_before = read_events(home.path(), planning);
    let source_before = read_events(home.path(), implementation);
    assert!(
        planning_before
            .iter()
            .any(|event| matches!(event.payload, EventPayload::RunCompleted { .. }))
    );
    let snapshot = assert_source_prefix(home.path(), implementation, &source_before);
    let workspace = home
        .path()
        .join("worktrees")
        .join(implementation.to_string());
    let head = git_output(&workspace, &["rev-parse", "HEAD"]);
    let owner = git_output(&workspace, &["rev-parse", "--git-common-dir"]);
    assert_eq!(
        PathBuf::from(owner.trim()).canonicalize().unwrap(),
        project.join(".git").canonicalize().unwrap()
    );
    assert_eq!(
        git_output(&workspace, &["branch", "--show-current"]).trim(),
        surge_git::run_branch_name(&implementation)
    );
    assert_eq!(
        registered_worktree(
            &git_output(&project, &["worktree", "list", "--porcelain", "-z"]),
            &workspace,
            head.trim(),
            &format!("refs/heads/{}", surge_git::run_branch_name(&implementation)),
        ),
        Ok(()),
        "original linked worktree has one physical Git registration with its exact HEAD and branch"
    );
    let index = std::fs::read(workspace.join(".git")).unwrap();
    assert!(
        String::from_utf8_lossy(&index).contains("gitdir:"),
        "original registered linked worktree"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("tracked-sentinel.txt")).unwrap(),
        "accepted Build dirty tracked bytes\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("untracked-sentinel.txt")).unwrap(),
        "accepted Build untracked bytes\n"
    );
    let dirty = git_output(
        &workspace,
        &["status", "--porcelain", "--untracked-files=all"],
    );
    assert!(dirty.contains(" M tracked-sentinel.txt"), "{dirty}");
    assert!(dirty.contains("?? untracked-sentinel.txt"), "{dirty}");
    let mut groups = assert_writer_groups_empty(&planning_before);
    groups.extend(assert_writer_groups_empty(&source_before));
    let before_wire: Vec<Value> = std::fs::read_to_string(home.path().join("provider-wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let actual_prompts = before_wire
        .iter()
        .filter(|entry| entry["operation"] == "prompt")
        .count();
    assert_eq!(
        actual_prompts, 9,
        "three planners/spec/Build/four real AppTester turns"
    );
    let invocations: Vec<Value> = std::fs::read_to_string(home.path().join("artifact-proxy.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let builds: Vec<_> = invocations
        .iter()
        .filter(|row| row["node"] == "impl_task")
        .map(|row| row["task"].as_str().unwrap())
        .collect();
    assert_eq!(
        builds,
        ["m1-t1"],
        "actual first Build called once before failure"
    );
    let operation = surge_core::id::WorkItemOperationId::new();
    let body = json!({"method":"continue_bootstrap_implementation", "request_id":42,
        "operation":operation, "bootstrap":bootstrap, "expected_revision":status["revision"],
        "expected_failed_run":implementation});
    let response = raw_request(&socket, body.clone()).await;
    let accepted_continuation = response["method"] == "bootstrap_implementation_continued";
    if accepted_continuation {
        let successor: RunId = serde_json::from_value(response["receipt"]["run"].clone()).unwrap();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                assert!(
                    host.child.try_wait().unwrap().is_none(),
                    "{}",
                    host.diagnostic()
                );
                let events = read_events(home.path(), successor);
                if events.iter().any(|event| {
                    matches!(
                        event.payload,
                        EventPayload::RunCompleted { .. } | EventPayload::RunFailed { .. }
                    )
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("accepted continuation must finish its original graph");
    }
    assert_eq!(
        rpc(&socket, json!({"method":"ping","request_id":43})).await["method"],
        "ping_ok"
    );
    assert_eq!(
        rpc(
            &socket,
            json!({"method":"bootstrap_status","request_id":44,"operation_id":bootstrap})
        )
        .await["status"],
        status
    );
    assert_eq!(read_events(home.path(), planning), planning_before);
    assert_eq!(read_events(home.path(), implementation), source_before);
    assert_eq!(git_output(&workspace, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git_output(&workspace, &["rev-parse", "--git-common-dir"]),
        owner
    );
    if !accepted_continuation {
        assert_eq!(
            git_output(
                &workspace,
                &["status", "--porcelain", "--untracked-files=all"]
            ),
            dirty
        );
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join("tracked-sentinel.txt")).unwrap(),
        "accepted Build dirty tracked bytes\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("untracked-sentinel.txt")).unwrap(),
        "accepted Build untracked bytes\n"
    );
    std::fs::write(home.path().join("continuation-preconditions.json"), serde_json::to_vec_pretty(&json!({
        "planning":planning,"implementation":implementation,"status":status,"request":body,
        "actual_response":response,"source_route_snapshot":snapshot,"writer_groups":groups,
        "actual_prompt_count":actual_prompts,"head":head,"git_common_dir":owner,"git_status":dirty,
        "fixed_tasks":TASKS,"source_journals_unchanged_after_request":true,
        "all_workspace_status_unchanged_required":!accepted_continuation, "sentinel_bytes_preserved":true,"ping_after_request":true
    })).unwrap()).unwrap();
    // Cleanup is checked before the expected assertion, so teardown cannot count as RED.
    drop(host);
    let cleanup: Value = serde_json::from_slice(
        &std::fs::read(home.path().join("second-host.cleanup.log")).unwrap(),
    )
    .unwrap();
    assert_eq!(cleanup["forced"], false, "{cleanup}");
    assert_eq!(cleanup["joined_success"], true, "{cleanup}");
    assert_eq!(cleanup["groups_empty"], true, "{cleanup}");
    assert_eq!(cleanup["cleanup_errors"], json!([]), "{cleanup}");
    assert_eq!(
        response["method"], "bootstrap_implementation_continued",
        "MISSING CONTINUATION: real daemon returned {response}; all positive source and settlement preconditions passed"
    );
    // This fixed oracle remains behind the first missing action and will become
    // reachable through the same real host when the endpoint is implemented.
    assert_completed_successor(home.path(), &response, implementation, &source_before).await;
}

async fn assert_completed_successor(
    home: &Path,
    response: &Value,
    source: RunId,
    source_events: &[RunEvent],
) {
    let successor: RunId = serde_json::from_value(response["receipt"]["run"].clone()).unwrap();
    assert_ne!(successor, source);
    assert!(
        !response["receipt"]["item"].is_null(),
        "real WorkItem association"
    );
    let item: surge_core::id::WorkItemId =
        serde_json::from_value(response["receipt"]["item"].clone()).unwrap();
    let storage = Storage::open(home).await.unwrap();
    let detail = storage.work_items().show(item).unwrap();
    let attempt = storage
        .work_items()
        .for_run(successor)
        .unwrap()
        .expect("real associated attempt");
    assert_eq!(detail.item.id, item);
    assert_eq!(attempt.item, item);
    assert_eq!(attempt.run, successor);
    assert_eq!(
        attempt.ordinal, 1,
        "first real attempt, no fabricated legacy attempt zero"
    );
    assert_eq!(
        attempt.state,
        surge_core::work_item::WorkItemAttemptState::Completed
    );
    assert_eq!(
        detail.item.workspace.path,
        home.join("worktrees")
            .join(source.to_string())
            .canonicalize()
            .unwrap()
    );
    let events = read_events(home, successor);
    let bound = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::WorkItemAttemptBound { context } => Some(context),
            _ => None,
        })
        .expect("actual accepted startup binding");
    assert_eq!(bound.binding(), &attempt.binding);
    drop(storage);
    assert!(
        events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::RunCompleted { .. }))
    );
    assert_eq!(
        read_events(home, source),
        source_events,
        "original failed history remains inspectable"
    );
    assert_eq!(source_events.iter().chain(&events).filter(|event| matches!(&event.payload,
        EventPayload::StageOutcomeCommitted { commit } if commit.context().node.as_str()=="impl_task"
    )).count(), 8, "one Build for each original task; first Build inherited once");
    let invocations: Vec<Value> = std::fs::read_to_string(home.join("artifact-proxy.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let builds: Vec<_> = invocations
        .iter()
        .filter(|row| row["node"] == "impl_task")
        .map(|row| row["task"].as_str().unwrap())
        .collect();
    assert_eq!(
        builds, TASKS,
        "actual provider Build call order, first task invoked once"
    );
    let task_order: Vec<_> = source_events
        .iter()
        .chain(&events)
        .filter_map(|event| match &event.payload {
            EventPayload::LoopIterationStarted { loop_id, item, .. }
                if loop_id.as_str() == "task_loop" =>
            {
                Some(item["id"].as_str().unwrap())
            },
            _ => None,
        })
        .collect();
    assert_eq!(task_order, TASKS);
    let verified: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TaskVerified { task_id, node, .. } if node.as_str() == "verify_task" => {
                Some(task_id.as_str())
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        verified, TASKS,
        "all eight original tasks actually verified"
    );
    assert_eq!(events.iter().filter(|event| matches!(&event.payload,
        EventPayload::LoopIterationCompleted { loop_id, .. } if loop_id.as_str()=="milestone_loop"
    )).count(), 2);
    let old_sessions: Vec<_> = source_events
        .iter()
        .filter_map(|event| match event.payload {
            EventPayload::SessionOpened { session, .. } => Some(session),
            _ => None,
        })
        .collect();
    let new_app = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::SessionOpened { session, node, .. } if node.as_str() == "app_task" => {
                Some(*session)
            },
            _ => None,
        })
        .unwrap();
    assert!(!old_sessions.contains(&new_app));
    let source_epoch = source_events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::VerificationCriteriaAccepted {
                task_id,
                criteria: Some(criteria),
            } if task_id.as_str() == "m1-t1" => Some(criteria.epoch),
            _ => None,
        })
        .unwrap();
    let successor_epoch = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::VerificationCriteriaAccepted {
                task_id,
                criteria: Some(criteria),
            } if task_id.as_str() == "m1-t1" => Some(criteria.epoch),
            _ => None,
        })
        .unwrap();
    assert_ne!(source_epoch, successor_epoch, "fresh host criterion epoch");
}
