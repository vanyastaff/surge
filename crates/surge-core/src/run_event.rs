//! Run event log entry — append-only event-sourced data model.

use crate::approvals::{ApprovalChannel, ApprovalChannelKind, ApprovalPolicy};
use crate::content_hash::ContentHash;
use crate::graph::Graph;
use crate::hooks::HookFailureMode;
use crate::id::{RunId, SessionId};
use crate::keys::{EdgeKey, NodeKey, OutcomeKey, TemplateKey};
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
    #[must_use]
    pub fn new(payload: EventPayload) -> Self {
        Self {
            schema_version: 1,
            payload,
        }
    }
}

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn versioned_wrapper_roundtrip() {
        let v = VersionedEventPayload::new(EventPayload::RunCompleted {
            terminal_node: NodeKey::try_from("end").unwrap(),
        });
        let bytes = serde_json::to_vec(&v).unwrap();
        let parsed: VersionedEventPayload = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v, parsed);
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
        };
        let bytes = payload.to_bincode().unwrap();
        let parsed = EventPayload::from_bincode(&bytes).unwrap();
        assert_eq!(payload, parsed);
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
}
