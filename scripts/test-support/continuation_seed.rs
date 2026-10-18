// Test-only pure event fixtures. Including module imports surge-core as `domain`.
// These metadata objects are never host read/launch capabilities.
use domain::execution_recovery::semantic_position::{ContinuationSeedId, SourceEventRef};
use domain::work_item::{ContinuationSeedFields, ContinuationSeedManifest, HmacSha256Tag,
    OwnedFlowObjectRef, WorkItemBinding, WorkItemContext, WorkItemRequirements};
use domain::{ContentHash, EventPayload, RunEvent, RunId};
pub fn graph() -> domain::Graph {
    let mut graph: domain::Graph = toml::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/flow_terminal_only.toml"))).unwrap();
    let node: domain::node::Node = toml::from_str("id = 'verify'\n[[declared_outcomes]]\nid='passed'\ndescription='passing checks'\nedge_kind_hint='forward'\nis_terminal=false\nledger_effect='verified'\n[position]\nx=0.0\ny=0.0\n[config]\nnode_kind='agent'\nprofile='verifier@2.0'\n").unwrap();
    graph.nodes.insert(node.id.clone(), node);
    graph.start = domain::NodeKey::try_from("verify").unwrap();
    graph.edges = vec![domain::edge::Edge {
        id: domain::EdgeKey::try_from("passed").unwrap(),
        from: domain::edge::PortRef { node: graph.start.clone(), outcome: domain::OutcomeKey::try_from("passed").unwrap() },
        to: domain::NodeKey::try_from("end").unwrap(), kind: domain::EdgeKind::Forward,
        policy: domain::edge::EdgePolicy::default(),
    }];
    graph
}
pub fn event(run: RunId, seq: u64, payload: EventPayload) -> RunEvent {
    RunEvent { run_id: run, seq, timestamp: chrono::DateTime::from_timestamp_millis(1_760_000_000_000 + i64::try_from(seq).unwrap()).unwrap(), payload }
}
pub fn startup(run: RunId) -> (Vec<RunEvent>, ContinuationSeedManifest) {
    let graph = graph();
    let requirements = WorkItemRequirements::new("Preserve the complete original mission.".into(), vec!["Required current behavior".into()]).unwrap();
    let binding = WorkItemBinding { item: domain::id::WorkItemId::new(), revision: 1, requirements_hash: requirements.hash().unwrap(), generation: 1 };
    let context = WorkItemContext::new(binding.clone(), requirements).unwrap();
    let source = RunId::new();
    let manifest = ContinuationSeedManifest::try_from(ContinuationSeedFields {
        schema_version: 1, operation: domain::id::WorkItemOperationId::new(), run, binding,
        workspace_owner: source, workspace_identity: ContentHash::compute(b"original workspace"),
        source_bootstrap: RunId::new(), source: SourceEventRef::new(source, 87).unwrap(),
        source_checkpoint: ContentHash::compute(b"source checkpoint"), graph_hash: graph.identity().unwrap(),
        seed: ContinuationSeedId::new(ContentHash::compute(b"fixed seed")),
        object_ref: OwnedFlowObjectRef::new("ab".repeat(32)).unwrap(),
        object_auth_tag: HmacSha256Tag::from_bytes([7;32]),
    }).unwrap();
    let events = vec![
        event(run,1,EventPayload::RunStarted { pipeline_template:None, project_path:std::env::temp_dir().join("semantic-fixture"), initial_prompt:context.prompt(), config:domain::RunConfig {
            bootstrap_edit_loop_cap:None, budget:domain::budget::BudgetGuard::default(), sandbox_default:domain::sandbox::SandboxMode::WorkspaceWrite,
            approval_default:domain::approvals::ApprovalPolicy::OnRequest, auto_pr:false, mcp_servers:vec![],
        }}),
        event(run,2,EventPayload::PipelineMaterialized { graph_hash:graph.identity().unwrap(), graph:Box::new(graph) }),
        event(run,3,EventPayload::WorkItemAttemptBound { context }),
    ];
    (events,manifest)
}
pub fn positive_tail(run: RunId, after: u64) -> Vec<RunEvent> {
    use domain::roadmap::{VerificationReportArtifact, VerificationReportOutcome, VerificationCheck, VerificationCheckResult};
    use domain::verification_evidence::{VerificationSubject, VerificationCriteria};
    let worktree=std::env::temp_dir().join("semantic-fixture");
    let subject=VerificationSubject { repository:worktree.join(".git"),worktree,tree:"a".repeat(40),checkpoint:"b".repeat(40) };
    let criteria=VerificationCriteria { hash:ContentHash::compute(b"required current behavior"),epoch:domain::id::StageGenerationId::new(),required:vec!["criterion:1".into()],definitions:vec![("criterion:1".into(),"Required current behavior".into())] };
    let report=VerificationReportArtifact { schema_version:1,task_id:"task".into(),outcome:VerificationReportOutcome::Passed,summary:"Controlled passing local observation".into(),checks:vec![VerificationCheck { command:"controlled acceptance".into(),result:VerificationCheckResult::Passed,covers:vec!["criterion:1".into()],note:None }],binding:Some(domain::verification_evidence::VerificationBinding { subject:subject.clone(),criteria:criteria.clone() }),..VerificationReportArtifact::default() };
    let evidence=ContentHash::compute(toml::to_string(&report).unwrap().as_bytes());
    let node=domain::NodeKey::try_from("verify").unwrap();
    let payloads=vec![
        EventPayload::StageEntered { node:node.clone(),attempt:1 },
        EventPayload::TaskStatusChanged { task_id:"task".into(),from:domain::RoadmapStatus::Pending,to:domain::RoadmapStatus::Completed,authority_node:node.clone() },
        EventPayload::VerificationSubjectObserved { subject:Some(subject) },
        EventPayload::VerificationCriteriaAccepted { task_id:"task".into(),criteria:Some(criteria) },
        EventPayload::TaskVerified { task_id:"task".into(),node,evidence,report:Some(report) },
        EventPayload::RunCompleted { terminal_node:domain::NodeKey::try_from("end").unwrap() },
    ];
    payloads.into_iter().enumerate().map(|(index,payload)|event(run,after+index as u64+1,payload)).collect()
}
