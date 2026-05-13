//! Run event log entry — append-only event-sourced data model.

use crate::approvals::{ApprovalChannel, ApprovalChannelKind, ApprovalPolicy};
use crate::archetype::ArchetypeMetadata;
use crate::content_hash::ContentHash;
use crate::edge::EdgeKind;
use crate::graph::Graph;
use crate::hooks::HookFailureMode;
use crate::id::{RunId, SessionId};
use crate::keys::{EdgeKey, NodeKey, OutcomeKey, SubgraphKey, TemplateKey};
use crate::notify_config::NotifyChannelKind;
use crate::roadmap_patch::{
    ActivePickupPolicy, OperatorConflictChoice, RoadmapPatchApprovalDecision, RoadmapPatchId,
    RoadmapPatchTarget,
};
use crate::sandbox::SandboxMode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunEvent {
    pub run_id: RunId,
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub payload: EventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VersionedEventPayload {
    pub schema_version: u32,
    pub payload: EventPayload,
}

impl VersionedEventPayload {
    /// Wrap a payload with the current schema version
    /// ([`crate::migrations::MAX_SUPPORTED_VERSION`]).
    ///
    /// New writes always go out at the latest schema version. The migration
    /// chain handles older versions on read.
    #[must_use]
    pub fn new(payload: EventPayload) -> Self {
        Self {
            schema_version: crate::migrations::MAX_SUPPORTED_VERSION,
            payload,
        }
    }

    /// Returns the schema version of the payload.
    ///
    /// Provided as an explicit accessor so storage code can reason about the
    /// payload's encoding stability without taking ownership or borrowing the
    /// inner `EventPayload`.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns a reference to the inner payload.
    #[must_use]
    pub fn payload(&self) -> &EventPayload {
        &self.payload
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    // Lifecycle
    RunStarted {
        pipeline_template: Option<TemplateKey>,
        project_path: PathBuf,
        initial_prompt: String,
        config: RunConfig,
    },
    RunCompleted {
        terminal_node: NodeKey,
    },
    RunFailed {
        error: String,
    },
    RunAborted {
        reason: String,
    },

    // Bootstrap
    BootstrapStageStarted {
        stage: BootstrapStage,
    },
    BootstrapArtifactProduced {
        stage: BootstrapStage,
        artifact: ContentHash,
        name: String,
    },
    BootstrapApprovalRequested {
        stage: BootstrapStage,
        channel: ApprovalChannel,
    },
    BootstrapApprovalDecided {
        stage: BootstrapStage,
        decision: BootstrapDecision,
        comment: Option<String>,
    },
    BootstrapEditRequested {
        stage: BootstrapStage,
        feedback: String,
    },
    BootstrapTelemetry {
        stage_durations: BTreeMap<BootstrapStage, u64>,
        edit_counts: BTreeMap<BootstrapStage, u32>,
        archetype: Option<ArchetypeMetadata>,
    },

    // Pipeline construction
    /// Frozen graph for the run. The `graph` payload is the source of truth —
    /// it lets `fold` reconstruct `RunState::Pipeline` purely from the event
    /// log without an out-of-band channel. `graph_hash` is the hash of the
    /// canonical serialized form for integrity checks during replay.
    /// Boxed because `Graph` is large and would dominate the
    /// `EventPayload` enum size for every variant otherwise.
    PipelineMaterialized {
        graph: Box<Graph>,
        graph_hash: ContentHash,
    },

    // Roadmap amendment lifecycle
    RoadmapPatchDrafted {
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        patch_artifact: ContentHash,
        patch_path: PathBuf,
    },
    RoadmapPatchApprovalRequested {
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        channel: ApprovalChannel,
        summary_hash: ContentHash,
    },
    RoadmapPatchApprovalDecided {
        patch_id: RoadmapPatchId,
        decision: RoadmapPatchApprovalDecision,
        channel_used: ApprovalChannelKind,
        comment: Option<String>,
        conflict_choice: Option<OperatorConflictChoice>,
    },
    RoadmapPatchApplied {
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        amended_roadmap_artifact: ContentHash,
        amended_roadmap_path: PathBuf,
        amended_flow_artifact: Option<ContentHash>,
        amended_flow_path: Option<PathBuf>,
    },
    RoadmapUpdated {
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        roadmap_artifact: ContentHash,
        roadmap_path: PathBuf,
        flow_artifact: Option<ContentHash>,
        flow_path: Option<PathBuf>,
        active_pickup: ActivePickupPolicy,
    },
    /// Accepted graph revision for an active roadmap amendment. The full
    /// graph is embedded so replay can reconstruct the active topology
    /// without reading mutable artifact paths.
    GraphRevisionAccepted {
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        previous_graph_hash: ContentHash,
        graph: Box<Graph>,
        graph_hash: ContentHash,
        active_pickup: ActivePickupPolicy,
    },

    // Stage execution
    StageEntered {
        node: NodeKey,
        attempt: u32,
    },
    StageInputsResolved {
        node: NodeKey,
        bindings: BTreeMap<String, ContentHash>,
    },
    SessionOpened {
        node: NodeKey,
        session: SessionId,
        agent: String,
    },
    ToolCalled {
        session: SessionId,
        tool: String,
        args_redacted: ContentHash,
    },
    ToolResultReceived {
        session: SessionId,
        success: bool,
        result: ContentHash,
    },
    ArtifactProduced {
        node: NodeKey,
        artifact: ContentHash,
        path: PathBuf,
        name: String,
    },
    OutcomeReported {
        node: NodeKey,
        outcome: OutcomeKey,
        summary: String,
    },
    StageCompleted {
        node: NodeKey,
        outcome: OutcomeKey,
    },
    StageFailed {
        node: NodeKey,
        reason: String,
        retry_available: bool,
    },
    SessionClosed {
        session: SessionId,
        disposition: SessionDisposition,
    },

    // Routing
    EdgeTraversed {
        edge: EdgeKey,
        from: NodeKey,
        to: NodeKey,
        /// Edge kind selected by routing. `#[serde(default)]` keeps the
        /// pre-Task-27 on-disk shape (which omitted this field) decodable as
        /// `EdgeKind::Forward` so legacy event logs replay unchanged.
        /// Bootstrap-mode HumanGate edit loops emit `kind = Backtrack` so
        /// fold can drive `RunMemory.node_visits` deterministically.
        #[serde(default)]
        kind: EdgeKind,
    },
    LoopIterationStarted {
        loop_id: NodeKey,
        item: toml::Value,
        index: u32,
    },
    LoopIterationCompleted {
        loop_id: NodeKey,
        index: u32,
        outcome: OutcomeKey,
    },
    LoopCompleted {
        loop_id: NodeKey,
        completed_iterations: u32,
        final_outcome: OutcomeKey,
    },

    // Human/sandbox/hooks/telemetry/forking
    ApprovalRequested {
        gate: NodeKey,
        channel: ApprovalChannel,
        payload_hash: ContentHash,
    },
    ApprovalDecided {
        gate: NodeKey,
        decision: String,
        channel_used: ApprovalChannelKind,
        comment: Option<String>,
    },
    SandboxElevationRequested {
        node: NodeKey,
        capability: String,
    },
    SandboxElevationDecided {
        node: NodeKey,
        decision: ElevationDecision,
        remember: bool,
    },
    /// Operator failed to respond to an elevation within
    /// [`crate::approvals::ApprovalConfig::resolved_elevation_timeout`]. The
    /// engine implicitly denied the request and replied `Cancelled` to the
    /// agent. Always paired with a `SandboxElevationDecided` with
    /// `decision: Deny, remember: false` so `surge replay` keeps a single
    /// canonical decision shape.
    ///
    /// Schema v2.
    SandboxElevationTimedOut {
        node: NodeKey,
        capability: String,
        elapsed_seconds: u32,
    },
    /// The detected runtime binary version is below the declared
    /// [`crate::runtime::RuntimeVersionPolicy::min_version`]. Warn-only —
    /// surge proceeds with the run.
    ///
    /// Schema v2.
    RuntimeVersionWarning {
        runtime: crate::runtime::RuntimeKind,
        found_version: String,
        min_version: String,
    },
    HookExecuted {
        hook_id: String,
        exit_status: i32,
        on_failure: HookFailureMode,
    },
    OutcomeRejectedByHook {
        node: NodeKey,
        outcome: OutcomeKey,
        hook_id: String,
    },
    TokensConsumed {
        session: SessionId,
        prompt_tokens: u32,
        output_tokens: u32,
        cache_hits: u32,
        model: String,
        cost_usd: Option<f64>,
    },
    ForkCreated {
        new_run: RunId,
        fork_at_seq: u64,
    },

    // Human input — added in M5. Three variants to support both the
    // tool-driven `request_human_input` path and HumanGate-driven pauses.
    HumanInputRequested {
        node: NodeKey,
        session: Option<SessionId>,
        call_id: Option<String>,
        prompt: String,
        schema: Option<serde_json::Value>,
    },
    HumanInputResolved {
        node: NodeKey,
        call_id: Option<String>,
        response: serde_json::Value,
    },
    HumanInputTimedOut {
        node: NodeKey,
        call_id: Option<String>,
        elapsed_seconds: u32,
    },

    // M6: Subgraph and Notify lifecycle.
    /// Engine entered a `NodeKind::Subgraph` — pushed a `SubgraphFrame`
    /// onto the per-run frame stack and advanced the cursor to the
    /// inner subgraph's start.
    SubgraphEntered {
        /// `NodeKey` of the outer Subgraph node.
        outer: NodeKey,
        /// `SubgraphKey` of the inner subgraph being executed.
        inner: SubgraphKey,
    },
    /// Engine popped a `SubgraphFrame` after the inner subgraph reached
    /// a terminal node. `outcome` is the outer outcome projected from
    /// `SubgraphConfig::outputs`.
    SubgraphExited {
        /// `NodeKey` of the outer Subgraph node.
        outer: NodeKey,
        /// `SubgraphKey` of the inner subgraph that just finished.
        inner: SubgraphKey,
        /// Outer outcome the inner artifact projected to.
        outcome: OutcomeKey,
    },
    /// Notify stage attempted delivery of a notification.
    /// One per stage attempt; emitted before `OutcomeReported`.
    NotifyDelivered {
        /// `NodeKey` of the Notify node.
        node: NodeKey,
        /// Channel-kind tag (no secrets / no transport details).
        channel_kind: NotifyChannelKind,
        /// `true` if delivery succeeded.
        success: bool,
        /// Error message if delivery failed; `None` on success.
        error: Option<String>,
    },
    /// Operator-facing escalation request — emitted when an automated path
    /// hits a hard limit (e.g., the bootstrap edit-loop cap) and the engine
    /// gives up. Distinct from `NotifyDelivered` because the engine emits it
    /// directly (no Notify node) and the operator may not see a delivery
    /// confirmation at all. Carries enough context for an out-of-band
    /// dispatcher (telegram, email, dashboard) to surface a clear message
    /// without needing to replay the event log.
    EscalationRequested {
        /// Bootstrap stage that ran out of retries, when the escalation
        /// originates from the bootstrap flow. `None` for non-bootstrap
        /// escalations.
        #[serde(default)]
        stage: Option<BootstrapStage>,
        /// Free-form operator-readable explanation (e.g., the cap value
        /// and the failure mode).
        reason: String,
    },
}

impl EventPayload {
    /// Serialize for the event log.
    ///
    /// Internally uses JSON bytes rather than raw bincode because bincode 1.x
    /// does not support `serde(tag = "type")` internally-tagged enums
    /// (`DeserializeAnyNotSupported`). The method is named `to_bincode` /
    /// `from_bincode` to match the interface contract; callers treat the
    /// returned bytes as opaque.
    pub fn to_bincode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn from_bincode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Returns a stable, kind-string discriminant for this payload.
    ///
    /// Used as the `events.kind` column value by the storage layer so that
    /// indexed lookups and view-maintenance dispatch can avoid re-deserializing
    /// the JSON blob.
    #[must_use]
    pub fn discriminant_str(&self) -> &'static str {
        match self {
            Self::RunStarted { .. } => "RunStarted",
            Self::RunCompleted { .. } => "RunCompleted",
            Self::RunFailed { .. } => "RunFailed",
            Self::RunAborted { .. } => "RunAborted",
            Self::BootstrapStageStarted { .. } => "BootstrapStageStarted",
            Self::BootstrapArtifactProduced { .. } => "BootstrapArtifactProduced",
            Self::BootstrapApprovalRequested { .. } => "BootstrapApprovalRequested",
            Self::BootstrapApprovalDecided { .. } => "BootstrapApprovalDecided",
            Self::BootstrapEditRequested { .. } => "BootstrapEditRequested",
            Self::BootstrapTelemetry { .. } => "BootstrapTelemetry",
            Self::PipelineMaterialized { .. } => "PipelineMaterialized",
            Self::RoadmapPatchDrafted { .. } => "RoadmapPatchDrafted",
            Self::RoadmapPatchApprovalRequested { .. } => "RoadmapPatchApprovalRequested",
            Self::RoadmapPatchApprovalDecided { .. } => "RoadmapPatchApprovalDecided",
            Self::RoadmapPatchApplied { .. } => "RoadmapPatchApplied",
            Self::RoadmapUpdated { .. } => "RoadmapUpdated",
            Self::GraphRevisionAccepted { .. } => "GraphRevisionAccepted",
            Self::StageEntered { .. } => "StageEntered",
            Self::StageInputsResolved { .. } => "StageInputsResolved",
            Self::SessionOpened { .. } => "SessionOpened",
            Self::ToolCalled { .. } => "ToolCalled",
            Self::ToolResultReceived { .. } => "ToolResultReceived",
            Self::ArtifactProduced { .. } => "ArtifactProduced",
            Self::OutcomeReported { .. } => "OutcomeReported",
            Self::StageCompleted { .. } => "StageCompleted",
            Self::StageFailed { .. } => "StageFailed",
            Self::SessionClosed { .. } => "SessionClosed",
            Self::EdgeTraversed { .. } => "EdgeTraversed",
            Self::LoopIterationStarted { .. } => "LoopIterationStarted",
            Self::LoopIterationCompleted { .. } => "LoopIterationCompleted",
            Self::LoopCompleted { .. } => "LoopCompleted",
            Self::ApprovalRequested { .. } => "ApprovalRequested",
            Self::ApprovalDecided { .. } => "ApprovalDecided",
            Self::SandboxElevationRequested { .. } => "SandboxElevationRequested",
            Self::SandboxElevationDecided { .. } => "SandboxElevationDecided",
            Self::SandboxElevationTimedOut { .. } => "SandboxElevationTimedOut",
            Self::RuntimeVersionWarning { .. } => "RuntimeVersionWarning",
            Self::HookExecuted { .. } => "HookExecuted",
            Self::OutcomeRejectedByHook { .. } => "OutcomeRejectedByHook",
            Self::TokensConsumed { .. } => "TokensConsumed",
            Self::HumanInputRequested { .. } => "HumanInputRequested",
            Self::HumanInputResolved { .. } => "HumanInputResolved",
            Self::HumanInputTimedOut { .. } => "HumanInputTimedOut",
            Self::ForkCreated { .. } => "ForkCreated",
            Self::SubgraphEntered { .. } => "SubgraphEntered",
            Self::SubgraphExited { .. } => "SubgraphExited",
            Self::NotifyDelivered { .. } => "NotifyDelivered",
            Self::EscalationRequested { .. } => "EscalationRequested",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapStage {
    Description,
    Roadmap,
    Flow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapDecision {
    Approve,
    Edit,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDisposition {
    Normal,
    AgentCrashed,
    Timeout,
    ForcedClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationDecision {
    Allow,
    AllowAndRemember,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunConfig {
    pub sandbox_default: SandboxMode,
    pub approval_default: ApprovalPolicy,
    #[serde(default)]
    pub auto_pr: bool,
    /// Run-level registry of MCP servers available to agent stages.
    /// Per-stage `ToolOverride::mcp_add` references these by name.
    /// Empty by default — no MCP delegation.
    #[serde(default)]
    pub mcp_servers: Vec<crate::mcp_config::McpServerRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_graph_for_event(start: &str) -> Graph {
        use crate::graph::{GraphMetadata, SCHEMA_VERSION};
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};

        let start = NodeKey::try_from(start).unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            start.clone(),
            Node {
                id: start.clone(),
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
            metadata: GraphMetadata {
                name: "minimal".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    #[test]
    fn run_started_bincode_roundtrip() {
        let payload = EventPayload::RunStarted {
            pipeline_template: Some(TemplateKey::try_from("rust-crate-tdd@1.0").unwrap()),
            project_path: PathBuf::from("/work/proj"),
            initial_prompt: "build it".into(),
            config: RunConfig {
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: true,
                mcp_servers: Vec::new(),
            },
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn bootstrap_decision_roundtrip() {
        let payload = EventPayload::BootstrapApprovalDecided {
            stage: BootstrapStage::Description,
            decision: BootstrapDecision::Approve,
            comment: Some("LGTM".into()),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn bootstrap_telemetry_roundtrip() {
        let mut stage_durations = BTreeMap::new();
        stage_durations.insert(BootstrapStage::Description, 120);
        stage_durations.insert(BootstrapStage::Roadmap, 340);
        stage_durations.insert(BootstrapStage::Flow, 560);

        let mut edit_counts = BTreeMap::new();
        edit_counts.insert(BootstrapStage::Flow, 2);

        let payload = EventPayload::BootstrapTelemetry {
            stage_durations,
            edit_counts,
            archetype: Some(ArchetypeMetadata {
                name: crate::archetype::ArchetypeName::Linear3,
                milestones: Some(1),
                edit_loop_cap: Some(3),
            }),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
        assert_eq!(payload.discriminant_str(), "BootstrapTelemetry");
    }

    #[test]
    fn versioned_wrapper_roundtrip() {
        let v = VersionedEventPayload::new(EventPayload::RunCompleted {
            terminal_node: NodeKey::try_from("end").unwrap(),
        });
        let bytes = serde_json::to_vec(&v).unwrap();
        let parsed: VersionedEventPayload = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn versioned_payload_accessors() {
        let inner = EventPayload::RunCompleted {
            terminal_node: NodeKey::try_from("end").unwrap(),
        };
        let v = VersionedEventPayload::new(inner.clone());
        // New writes always go out at MAX_SUPPORTED_VERSION (bumped to 2 in
        // the schema v2 migration that introduced SandboxElevationTimedOut +
        // RuntimeVersionWarning).
        assert_eq!(v.schema_version(), crate::migrations::MAX_SUPPORTED_VERSION,);
        assert_eq!(v.payload(), &inner);
    }

    #[test]
    fn discriminant_str_is_pascal_case_kind() {
        let p = EventPayload::RunFailed {
            error: "boom".into(),
        };
        assert_eq!(p.discriminant_str(), "RunFailed");

        let p = EventPayload::TokensConsumed {
            session: SessionId::new(),
            prompt_tokens: 1,
            output_tokens: 2,
            cache_hits: 0,
            model: "claude-opus-4-7".into(),
            cost_usd: None,
        };
        assert_eq!(p.discriminant_str(), "TokensConsumed");
    }

    #[test]
    fn pipeline_materialized_carries_graph() {
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use crate::node::{Node, NodeConfig, Position};
        use crate::terminal_config::{TerminalConfig, TerminalKind};
        use std::collections::BTreeMap;

        let end = NodeKey::try_from("end").unwrap();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            end.clone(),
            Node {
                id: end.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "minimal".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: end,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        };
        let payload = EventPayload::PipelineMaterialized {
            graph: Box::new(graph),
            graph_hash: ContentHash::compute(b"placeholder"),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn roadmap_patch_lifecycle_events_roundtrip() {
        let patch_id = RoadmapPatchId::new("rpatch-demo").unwrap();
        let target = RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        };
        let patch_hash = ContentHash::compute(b"roadmap-patch");
        let roadmap_hash = ContentHash::compute(b"roadmap");
        let flow_hash = ContentHash::compute(b"flow");

        let events = vec![
            EventPayload::RoadmapPatchDrafted {
                patch_id: patch_id.clone(),
                target: target.clone(),
                patch_artifact: patch_hash,
                patch_path: PathBuf::from("roadmap-patch.toml"),
            },
            EventPayload::RoadmapPatchApprovalRequested {
                patch_id: patch_id.clone(),
                target: target.clone(),
                channel: ApprovalChannel::Desktop {
                    duration: crate::approvals::ApprovalDuration::Transient,
                },
                summary_hash: ContentHash::compute(b"summary"),
            },
            EventPayload::RoadmapPatchApprovalDecided {
                patch_id: patch_id.clone(),
                decision: RoadmapPatchApprovalDecision::Approve,
                channel_used: ApprovalChannelKind::Desktop,
                comment: Some("ship it".into()),
                conflict_choice: None,
            },
            EventPayload::RoadmapPatchApplied {
                patch_id: patch_id.clone(),
                target: target.clone(),
                amended_roadmap_artifact: roadmap_hash,
                amended_roadmap_path: PathBuf::from("roadmap.toml"),
                amended_flow_artifact: Some(flow_hash),
                amended_flow_path: Some(PathBuf::from("flow.toml")),
            },
            EventPayload::RoadmapUpdated {
                patch_id: patch_id.clone(),
                target: target.clone(),
                roadmap_artifact: roadmap_hash,
                roadmap_path: PathBuf::from("roadmap.toml"),
                flow_artifact: Some(flow_hash),
                flow_path: Some(PathBuf::from("flow.toml")),
                active_pickup: ActivePickupPolicy::Allowed,
            },
            EventPayload::GraphRevisionAccepted {
                patch_id,
                target,
                previous_graph_hash: ContentHash::compute(b"old-flow"),
                graph: Box::new(minimal_graph_for_event("end")),
                graph_hash: flow_hash,
                active_pickup: ActivePickupPolicy::Allowed,
            },
        ];

        for payload in events {
            let bytes = payload.to_bincode().unwrap();
            let parsed = EventPayload::from_bincode(&bytes).unwrap();
            assert_eq!(payload, parsed);
        }
    }

    #[test]
    fn roadmap_patch_discriminants_are_stable() {
        let patch_id = RoadmapPatchId::new("rpatch-demo").unwrap();
        let target = RoadmapPatchTarget::ProjectRoadmap {
            roadmap_path: ".ai-factory/ROADMAP.md".into(),
        };
        let payload = EventPayload::RoadmapPatchDrafted {
            patch_id,
            target,
            patch_artifact: ContentHash::compute(b"patch"),
            patch_path: PathBuf::from("roadmap-patch.toml"),
        };

        assert_eq!(payload.discriminant_str(), "RoadmapPatchDrafted");

        let payload = EventPayload::GraphRevisionAccepted {
            patch_id: RoadmapPatchId::new("rpatch-graph").unwrap(),
            target: RoadmapPatchTarget::ProjectRoadmap {
                roadmap_path: ".ai-factory/ROADMAP.md".into(),
            },
            previous_graph_hash: ContentHash::compute(b"old-flow"),
            graph: Box::new(minimal_graph_for_event("end")),
            graph_hash: ContentHash::compute(b"new-flow"),
            active_pickup: ActivePickupPolicy::Allowed,
        };
        assert_eq!(payload.discriminant_str(), "GraphRevisionAccepted");
    }

    // Stage execution + routing variants

    #[test]
    fn stage_entered_roundtrip() {
        let payload = EventPayload::StageEntered {
            node: NodeKey::try_from("impl_1").unwrap(),
            attempt: 2,
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn session_opened_and_closed_roundtrip() {
        let session = SessionId::new();
        let opened = EventPayload::SessionOpened {
            node: NodeKey::try_from("agent_1").unwrap(),
            session,
            agent: "claude-opus-4-7".into(),
        };
        let closed = EventPayload::SessionClosed {
            session,
            disposition: SessionDisposition::Normal,
        };
        for p in [opened, closed] {
            let bytes = p.to_bincode().unwrap();
            let parsed = EventPayload::from_bincode(&bytes).unwrap();
            assert_eq!(p, parsed);
        }
    }

    #[test]
    fn artifact_produced_roundtrip() {
        let payload = EventPayload::ArtifactProduced {
            node: NodeKey::try_from("spec_1").unwrap(),
            artifact: ContentHash::compute(b"content"),
            path: PathBuf::from("artifacts/spec.md"),
            name: "spec.md".into(),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn edge_traversed_roundtrip() {
        let payload = EventPayload::EdgeTraversed {
            edge: EdgeKey::try_from("e_done").unwrap(),
            from: NodeKey::try_from("a").unwrap(),
            to: NodeKey::try_from("b").unwrap(),
            kind: EdgeKind::Forward,
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn edge_traversed_backtrack_roundtrip() {
        // Bootstrap edit-loop traversals must round-trip with their
        // discriminant intact — fold relies on the kind field to bump
        // `RunMemory.node_visits` on the target node.
        let payload = EventPayload::EdgeTraversed {
            edge: EdgeKey::try_from("e_edit").unwrap(),
            from: NodeKey::try_from("gate1").unwrap(),
            to: NodeKey::try_from("desc_author").unwrap(),
            kind: EdgeKind::Backtrack,
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn edge_traversed_legacy_json_defaults_to_forward() {
        // Persistence migrates legacy JSON payloads that pre-date Task 27 and
        // therefore omit the `kind` field. `#[serde(default)]` must keep
        // those events decodable as `EdgeKind::Forward`. EventPayload is
        // internally tagged (`tag = "type"`, `rename_all = "snake_case"`),
        // so the legacy on-disk shape is a flat object.
        let legacy_json = r#"{"type":"edge_traversed","edge":"e_done","from":"a","to":"b"}"#;
        let parsed: EventPayload = serde_json::from_str(legacy_json).unwrap();
        match parsed {
            EventPayload::EdgeTraversed { kind, .. } => {
                assert_eq!(kind, EdgeKind::Forward);
            },
            other => panic!("expected EdgeTraversed, got {other:?}"),
        }
    }

    #[test]
    fn loop_lifecycle_variants_roundtrip() {
        let started = EventPayload::LoopIterationStarted {
            loop_id: NodeKey::try_from("loop1").unwrap(),
            item: toml::Value::String("milestone-1".into()),
            index: 0,
        };
        let completed = EventPayload::LoopIterationCompleted {
            loop_id: NodeKey::try_from("loop1").unwrap(),
            index: 0,
            outcome: OutcomeKey::try_from("done").unwrap(),
        };
        let final_ev = EventPayload::LoopCompleted {
            loop_id: NodeKey::try_from("loop1").unwrap(),
            completed_iterations: 5,
            final_outcome: OutcomeKey::try_from("done").unwrap(),
        };
        for p in [started, completed, final_ev] {
            let bytes = p.to_bincode().unwrap();
            let parsed = EventPayload::from_bincode(&bytes).unwrap();
            assert_eq!(p, parsed);
        }
    }

    // Human / sandbox / hooks / telemetry / forking

    #[test]
    fn approval_request_and_decision_roundtrip() {
        let req = EventPayload::ApprovalRequested {
            gate: NodeKey::try_from("gate_main").unwrap(),
            channel: ApprovalChannel::Telegram {
                chat_id_ref: "$DEFAULT".into(),
            },
            payload_hash: ContentHash::compute(b"summary"),
        };
        let dec = EventPayload::ApprovalDecided {
            gate: NodeKey::try_from("gate_main").unwrap(),
            decision: "approve".into(),
            channel_used: ApprovalChannelKind::Telegram,
            comment: None,
        };
        for p in [req, dec] {
            let bytes = p.to_bincode().unwrap();
            let parsed = EventPayload::from_bincode(&bytes).unwrap();
            assert_eq!(p, parsed);
        }
    }

    #[test]
    fn sandbox_elevation_roundtrip() {
        let req = EventPayload::SandboxElevationRequested {
            node: NodeKey::try_from("impl_1").unwrap(),
            capability: "network: api.example.com".into(),
        };
        let dec = EventPayload::SandboxElevationDecided {
            node: NodeKey::try_from("impl_1").unwrap(),
            decision: ElevationDecision::AllowAndRemember,
            remember: true,
        };
        for p in [req, dec] {
            let bytes = p.to_bincode().unwrap();
            let parsed = EventPayload::from_bincode(&bytes).unwrap();
            assert_eq!(p, parsed);
        }
    }

    #[test]
    fn hook_executed_and_rejection_roundtrip() {
        let hook_ev = EventPayload::HookExecuted {
            hook_id: "fmt-check".into(),
            exit_status: 0,
            on_failure: HookFailureMode::Warn,
        };
        let reject = EventPayload::OutcomeRejectedByHook {
            node: NodeKey::try_from("impl_1").unwrap(),
            outcome: OutcomeKey::try_from("done").unwrap(),
            hook_id: "test-runner".into(),
        };
        for p in [hook_ev, reject] {
            let bytes = p.to_bincode().unwrap();
            let parsed = EventPayload::from_bincode(&bytes).unwrap();
            assert_eq!(p, parsed);
        }
    }

    #[test]
    fn tokens_consumed_roundtrip() {
        let payload = EventPayload::TokensConsumed {
            session: SessionId::new(),
            prompt_tokens: 1500,
            output_tokens: 800,
            cache_hits: 200,
            model: "claude-opus-4-7".into(),
            cost_usd: Some(0.045),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn fork_created_roundtrip() {
        let payload = EventPayload::ForkCreated {
            new_run: RunId::new(),
            fork_at_seq: 412,
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn human_input_requested_roundtrip() {
        let payload = EventPayload::HumanInputRequested {
            node: NodeKey::try_from("plan_1").unwrap(),
            session: Some(SessionId::new()),
            call_id: Some("call-42".into()),
            prompt: "Approve the plan?".into(),
            schema: Some(serde_json::json!({"type":"string"})),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
        assert_eq!(payload.discriminant_str(), "HumanInputRequested");
    }

    #[test]
    fn human_input_resolved_roundtrip() {
        let payload = EventPayload::HumanInputResolved {
            node: NodeKey::try_from("plan_1").unwrap(),
            call_id: Some("call-42".into()),
            response: serde_json::json!({"decision":"approve"}),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
        assert_eq!(payload.discriminant_str(), "HumanInputResolved");
    }

    #[test]
    fn human_input_timed_out_roundtrip() {
        let payload = EventPayload::HumanInputTimedOut {
            node: NodeKey::try_from("plan_1").unwrap(),
            call_id: None,
            elapsed_seconds: 300,
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
        assert_eq!(payload.discriminant_str(), "HumanInputTimedOut");
    }

    #[test]
    fn subgraph_entered_roundtrips_via_bincode() {
        let payload = EventPayload::SubgraphEntered {
            outer: NodeKey::try_from("review_outer").unwrap(),
            inner: SubgraphKey::try_from("review_block").unwrap(),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn subgraph_exited_roundtrips_via_bincode() {
        let payload = EventPayload::SubgraphExited {
            outer: NodeKey::try_from("review_outer").unwrap(),
            inner: SubgraphKey::try_from("review_block").unwrap(),
            outcome: OutcomeKey::try_from("approved").unwrap(),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn notify_delivered_roundtrips_via_bincode() {
        let payload = EventPayload::NotifyDelivered {
            node: NodeKey::try_from("notify_done").unwrap(),
            channel_kind: NotifyChannelKind::Webhook,
            success: true,
            error: None,
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn discriminant_str_covers_new_variants() {
        let p1 = EventPayload::SubgraphEntered {
            outer: NodeKey::try_from("a").unwrap(),
            inner: SubgraphKey::try_from("b").unwrap(),
        };
        assert_eq!(p1.discriminant_str(), "SubgraphEntered");

        let p2 = EventPayload::NotifyDelivered {
            node: NodeKey::try_from("a").unwrap(),
            channel_kind: NotifyChannelKind::Desktop,
            success: false,
            error: Some("test".into()),
        };
        assert_eq!(p2.discriminant_str(), "NotifyDelivered");
    }

    #[test]
    fn run_config_with_mcp_servers_roundtrips() {
        use crate::mcp_config::{McpServerRef, McpTransportConfig};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::Duration;

        let cfg = RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![McpServerRef {
                name: "playwright".into(),
                transport: McpTransportConfig::Stdio {
                    command: PathBuf::from("mcp-playwright"),
                    args: vec![],
                    env: HashMap::new(),
                },
                allowed_tools: None,
                call_timeout: Duration::from_secs(60),
                restart_on_crash: true,
            }],
        };
        let payload = EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/work"),
            initial_prompt: "x".into(),
            config: cfg.clone(),
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn run_config_default_mcp_servers_empty() {
        let s = r#"
            sandbox_default = "workspace-write"
            approval_default = "on-request"
        "#;
        let cfg: RunConfig = toml::from_str(s).unwrap();
        assert!(cfg.mcp_servers.is_empty());
        assert!(!cfg.auto_pr);
    }
}
