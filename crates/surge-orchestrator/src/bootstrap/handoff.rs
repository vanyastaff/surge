//! The bootstrap handoff (#98): what a follow-up run inherits from its
//! completed bootstrap (planning) run, validated once, for every launcher.
//!
//! A bootstrap run (Description → Roadmap → Flow, each behind a human gate)
//! ends with an approved description, roadmap and generated flow. Given the
//! parent run, [`prepare`] returns a [`BootstrapHandoff`] or a typed
//! [`HandoffRejection`]. The CLI, the daemon continuation and
//! `Engine::start_run` with `bootstrap_parent` all use it, so every launch
//! gives the child the same inputs and refuses the same bad plans with the
//! same reasons.
//!
//! # Rules
//! - The parent must be confirmed completed: its log's terminal verdict is a
//!   single `RunCompleted`.
//! - The follow-up graph is the latest `PipelineMaterialized` after the
//!   first. The first is the bootstrap graph itself (replay keeps selecting
//!   that one; the handoff selects the generated one).
//! - Each inherited artifact is the latest of its kind, resolved through the
//!   core name authority ([`surge_core::artifact_contract::select_canonical`]);
//!   competing spellings are refused, never guessed.
//! - Bytes are hash-verified (`ArtifactStore::open_ref`), UTF-8, and valid
//!   under their artifact contract. The flow text must parse and equal the
//!   materialized graph, which must pass registry and M6 validation.
//! - The operator's flow-gate edits (`NodeOverrides` from the last
//!   `flow_gate` answer) are applied, re-validated, and the inherited `flow`
//!   bytes regenerated from the edited graph.
//! - Seed paths come from the artifact contract.
//! - **Read-only:** nothing is appended to the parent log.
//!
//! [`classify`] maps a bootstrap run's outcome to a [`BootstrapStop`], the
//! one exhaustive "why is there no handoff yet" answer.

use std::path::PathBuf;
use std::sync::Arc;

use surge_core::artifact_contract::{
    ArtifactKind, canonical_seed_path, select_canonical, validate_artifact_text,
};
use surge_core::content_hash::ContentHash;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::run_event::EventPayload;
use surge_core::run_state::ArtifactRef;
use surge_core::run_verdict::{TerminalOutcome, TerminalVerdict};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::{EventSeq, ReadEvent, Storage};

use crate::engine::config::RunSeedArtifact;
use crate::engine::handle::RunOutcome;
use crate::profile_loader::ProfileRegistry;

/// Node id of the bundled bootstrap flow's approval gate for the flow stage.
const FLOW_GATE_NODE: &str = "flow_gate";

/// Synthetic producer recorded on inherited artifacts in the child.
pub const BOOTSTRAP_PARENT_NODE: &str = "bootstrap_parent";

/// The artifacts a follow-up run inherits, in order: the canonical logical
/// name and the kind it denotes.
const INHERITED: [(&str, ArtifactKind); 3] = [
    ("description", ArtifactKind::Description),
    ("roadmap", ArtifactKind::Roadmap),
    ("flow", ArtifactKind::Flow),
];

/// What the child will execute with, for validating the plan.
#[derive(Clone, Copy, Default)]
pub struct HandoffContext<'a> {
    /// The profile registry the child resolves profiles through. `None`
    /// validates graph structure only.
    pub profiles: Option<&'a ProfileRegistry>,
    /// The agent catalog the child launches with. When given (with
    /// `profiles`), every agent node's runtime must be in it.
    pub agents: Option<&'a surge_acp::Registry>,
}

/// One inherited artifact, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct InheritedArtifact {
    /// Canonical logical name in the child (`description`, `roadmap`, `flow`).
    pub name: &'static str,
    /// The artifact kind.
    pub kind: ArtifactKind,
    /// Canonical worktree-relative seed path, from the artifact contract.
    pub seed_path: PathBuf,
    /// Hash of [`Self::content`].
    pub hash: ContentHash,
    /// The validated UTF-8 content (for `flow`, regenerated after edits).
    pub content: String,
    /// The parent's artifact this came from.
    pub source: ArtifactRef,
}

/// A validated handoff from a completed bootstrap run.
#[derive(Debug, Clone, PartialEq)]
pub struct BootstrapHandoff {
    /// The completed bootstrap run.
    pub parent: RunId,
    /// The parent's worktree, from its `RunStarted`.
    pub parent_worktree: PathBuf,
    /// The follow-up graph, with the operator's flow-gate edits applied.
    pub graph: Graph,
    /// [`Graph::identity`] of [`Self::graph`].
    pub identity: ContentHash,
    /// Inherited artifacts, in [`INHERITED`] order, each exactly once.
    pub artifacts: Vec<InheritedArtifact>,
    /// How many plan steps the operator's flow-gate edits changed.
    pub edits_applied: usize,
}

impl BootstrapHandoff {
    /// The inherited artifacts as run seeds: the child records each one
    /// exactly once, at its canonical path, produced by `bootstrap_parent`.
    ///
    /// # Errors
    /// [`HandoffRejection::Internal`] if a seed cannot be built.
    pub fn seed_artifacts(&self) -> Result<Vec<RunSeedArtifact>, HandoffRejection> {
        self.artifacts
            .iter()
            .map(|artifact| {
                RunSeedArtifact::new(
                    artifact.name,
                    artifact.seed_path.clone(),
                    &artifact.content,
                    BOOTSTRAP_PARENT_NODE,
                )
                .map_err(|error| HandoffRejection::Internal(error.to_string()))
            })
            .collect()
    }

    /// The parent artifact refs the handoff was built from, under their
    /// canonical names.
    #[must_use]
    pub fn source_refs(&self) -> Vec<ArtifactRef> {
        self.artifacts
            .iter()
            .map(|artifact| ArtifactRef {
                name: artifact.name.to_owned(),
                ..artifact.source.clone()
            })
            .collect()
    }
}

/// Why there is no valid handoff.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandoffRejection {
    /// The parent's event log could not be read.
    #[error("bootstrap parent log could not be read: {0}")]
    ParentUnreadable(String),
    /// The parent's log does not confirm a single successful completion.
    #[error("bootstrap parent completion is not confirmed")]
    ParentNotCompleted,
    /// The parent never materialized a follow-up graph.
    #[error("bootstrap did not materialize a follow-up graph")]
    FollowUpGraphMissing,
    /// A required artifact was never produced.
    #[error("bootstrap artifact missing: {0}")]
    ArtifactMissing(&'static str),
    /// Competing spellings claim one artifact.
    #[error("bootstrap artifact {name} has competing names {names:?}")]
    ArtifactAmbiguous {
        /// The inherited artifact.
        name: &'static str,
        /// The competing logical names.
        names: Vec<String>,
    },
    /// The artifact's bytes could not be read or failed hash verification.
    #[error("bootstrap artifact {name} is unreadable or corrupt: {reason}")]
    ArtifactCorrupt {
        /// The inherited artifact.
        name: &'static str,
        /// What failed.
        reason: String,
    },
    /// The artifact is not UTF-8 text.
    #[error("bootstrap artifact {0} is not UTF-8")]
    ArtifactNotUtf8(&'static str),
    /// The artifact violates its contract.
    #[error("bootstrap artifact {name} violates its contract: {issues:?}")]
    ArtifactContract {
        /// The inherited artifact.
        name: &'static str,
        /// The contract errors.
        issues: Vec<String>,
    },
    /// The flow text does not parse as a graph.
    #[error("bootstrap flow does not parse: {0}")]
    FlowUnparseable(String),
    /// The flow text and the materialized graph disagree.
    #[error("bootstrap flow text disagrees with the materialized graph")]
    FlowMismatch,
    /// The follow-up graph fails validation for the child's registries.
    #[error("follow-up graph is invalid: {0}")]
    GraphInvalid(String),
    /// The operator's flow-gate edits are malformed or invalid.
    #[error("flow-gate plan edits are invalid: {0}")]
    EditsInvalid(String),
    /// A reload produced a graph other than the pinned one.
    #[error("follow-up graph identity {actual} differs from the pinned {expected}")]
    IdentityMismatch {
        /// The pinned identity.
        expected: ContentHash,
        /// The reloaded graph's identity.
        actual: ContentHash,
    },
    /// An internal failure building the handoff.
    #[error("bootstrap handoff failed: {0}")]
    Internal(String),
}

impl HandoffRejection {
    /// Whether retrying may succeed: the parent could not be read or its
    /// completion is not confirmed yet. Every other rejection is an invalid
    /// plan.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::ParentUnreadable(_) | Self::ParentNotCompleted)
    }
}

/// Build the validated handoff from a completed bootstrap run.
///
/// # Errors
/// A typed [`HandoffRejection`].
pub async fn prepare(
    storage: &Arc<Storage>,
    parent: RunId,
    context: &HandoffContext<'_>,
) -> Result<BootstrapHandoff, HandoffRejection> {
    let events = read_parent(storage, parent).await?;
    let TerminalVerdict::Terminal(claim) = ReadEvent::terminal_verdict(&events) else {
        return Err(HandoffRejection::ParentNotCompleted);
    };
    if !matches!(claim.outcome, TerminalOutcome::Completed { .. }) {
        return Err(HandoffRejection::ParentNotCompleted);
    }
    let parent_worktree = events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::RunStarted { project_path, .. } => Some(project_path.clone()),
            _ => None,
        })
        .ok_or(HandoffRejection::ParentNotCompleted)?;
    let materialized = follow_up_graph(&events).ok_or(HandoffRejection::FollowUpGraphMissing)?;

    let refs: Vec<ArtifactRef> = events.iter().filter_map(artifact_ref).collect();
    let store = ArtifactStore::new(storage.home().join("runs"));
    let mut artifacts = Vec::with_capacity(INHERITED.len());
    for (name, kind) in INHERITED {
        let source = select_canonical(&refs, kind, None)
            .map_err(|ambiguous| HandoffRejection::ArtifactAmbiguous {
                name,
                names: ambiguous.names,
            })?
            .ok_or(HandoffRejection::ArtifactMissing(name))?
            .clone();
        let bytes = store
            .open_ref(parent, &source, &parent_worktree)
            .await
            .map_err(|error| HandoffRejection::ArtifactCorrupt {
                name,
                reason: error.to_string(),
            })?;
        let content =
            String::from_utf8(bytes).map_err(|_| HandoffRejection::ArtifactNotUtf8(name))?;
        let report = validate_artifact_text(kind, &content);
        if !report.is_valid() {
            return Err(HandoffRejection::ArtifactContract {
                name,
                issues: report
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.severity
                            == surge_core::artifact_contract::ArtifactDiagnosticSeverity::Error
                    })
                    .map(|diagnostic| diagnostic.message.clone())
                    .collect(),
            });
        }
        let seed_path = canonical_seed_path(kind, None).ok_or_else(|| {
            HandoffRejection::Internal(format!("artifact contract has no seed path for {name}"))
        })?;
        artifacts.push(InheritedArtifact {
            name,
            kind,
            seed_path,
            hash: ContentHash::compute(content.as_bytes()),
            content,
            source,
        });
    }

    let flow_text = &artifacts
        .iter()
        .find(|artifact| artifact.kind == ArtifactKind::Flow)
        .ok_or(HandoffRejection::ArtifactMissing("flow"))?
        .content;
    let flow: Graph = toml::from_str(flow_text)
        .map_err(|error| HandoffRejection::FlowUnparseable(error.to_string()))?;
    if flow != materialized {
        return Err(HandoffRejection::FlowMismatch);
    }
    validate(&materialized, context)?;

    let mut graph = materialized;
    let edits_applied = apply_flow_gate_edits(&events, &mut graph, context)?;
    if edits_applied > 0 {
        let text = toml::to_string(&graph)
            .map_err(|error| HandoffRejection::Internal(error.to_string()))?;
        if let Some(flow) = artifacts
            .iter_mut()
            .find(|artifact| artifact.kind == ArtifactKind::Flow)
        {
            flow.hash = ContentHash::compute(text.as_bytes());
            flow.content = text;
        }
    }
    let identity = graph
        .identity()
        .map_err(|error| HandoffRejection::Internal(error.to_string()))?;
    tracing::info!(
        target: "engine::bootstrap::handoff",
        %parent,
        identity = %identity,
        edits_applied,
        "bootstrap handoff prepared"
    );
    Ok(BootstrapHandoff {
        parent,
        parent_worktree,
        graph,
        identity,
        artifacts,
        edits_applied,
    })
}

/// [`prepare`] again and require the graph identity pinned earlier.
///
/// # Errors
/// Any [`prepare`] rejection, or [`HandoffRejection::IdentityMismatch`].
pub async fn reload(
    storage: &Arc<Storage>,
    parent: RunId,
    context: &HandoffContext<'_>,
    expected: ContentHash,
) -> Result<BootstrapHandoff, HandoffRejection> {
    let handoff = prepare(storage, parent, context).await?;
    if handoff.identity != expected {
        return Err(HandoffRejection::IdentityMismatch {
            expected,
            actual: handoff.identity,
        });
    }
    Ok(handoff)
}

/// Why a bootstrap run has (or has not yet) a handoff.
#[derive(Debug, Clone, PartialEq)]
pub enum BootstrapStop {
    /// Completed: a handoff can be prepared.
    Completed,
    /// Parked until a provider window resets; resumes on its own.
    Parked {
        /// When it is expected to resume.
        wake_at: chrono::DateTime<chrono::Utc>,
    },
    /// Suspended at a durable fence; resumable.
    Suspended,
    /// Restore refused; needs explicit recovery.
    RecoveryRequired {
        /// Actionable reason.
        diagnostic: String,
    },
    /// Failed.
    Failed {
        /// The failure.
        error: String,
    },
    /// Aborted.
    Aborted {
        /// Why.
        reason: String,
    },
    /// Execution produced an outcome but settlement could not be confirmed.
    SettlementFailed {
        /// What failed to settle.
        failures: String,
    },
}

/// Classify a bootstrap run's outcome. Exhaustive: a new [`RunOutcome`]
/// variant fails to compile here, in exactly one place.
#[must_use]
pub fn classify(outcome: &RunOutcome) -> BootstrapStop {
    match outcome {
        RunOutcome::Completed { .. } => BootstrapStop::Completed,
        RunOutcome::Parked { wake_at } => BootstrapStop::Parked { wake_at: *wake_at },
        RunOutcome::Suspended { .. } => BootstrapStop::Suspended,
        RunOutcome::RecoveryRequired { diagnostic, .. } => BootstrapStop::RecoveryRequired {
            diagnostic: diagnostic.clone(),
        },
        RunOutcome::Failed { error } => BootstrapStop::Failed {
            error: error.clone(),
        },
        RunOutcome::Aborted { reason } => BootstrapStop::Aborted {
            reason: reason.clone(),
        },
        RunOutcome::SettlementFailed { failures, .. } => BootstrapStop::SettlementFailed {
            failures: format!("{failures:?}"),
        },
    }
}

async fn read_parent(
    storage: &Arc<Storage>,
    parent: RunId,
) -> Result<Vec<ReadEvent>, HandoffRejection> {
    let unreadable =
        |error: &dyn std::fmt::Display| HandoffRejection::ParentUnreadable(error.to_string());
    let reader = storage
        .open_run_reader(parent)
        .await
        .map_err(|error| unreadable(&error))?;
    let last = reader
        .current_seq()
        .await
        .map_err(|error| unreadable(&error))?;
    reader
        .read_events(EventSeq(1)..last.next())
        .await
        .map_err(|error| unreadable(&error))
}

/// The latest `PipelineMaterialized` graph after the first (the bootstrap
/// graph itself).
fn follow_up_graph(events: &[ReadEvent]) -> Option<Graph> {
    events
        .iter()
        .filter_map(|event| match &event.payload.payload {
            EventPayload::PipelineMaterialized { graph, .. } => Some(graph),
            _ => None,
        })
        .skip(1)
        .last()
        .map(|graph| (**graph).clone())
}

fn artifact_ref(event: &ReadEvent) -> Option<ArtifactRef> {
    let EventPayload::ArtifactProduced {
        node,
        artifact,
        path,
        name,
        ..
    } = &event.payload.payload
    else {
        return None;
    };
    Some(ArtifactRef {
        hash: *artifact,
        path: path.clone(),
        name: name.clone(),
        produced_by: node.clone(),
        produced_at_seq: event.seq.as_u64(),
    })
}

/// Registry, runtime and M6 validation of the follow-up graph.
fn validate(graph: &Graph, context: &HandoffContext<'_>) -> Result<(), HandoffRejection> {
    crate::engine::validate::validate_graph_against(graph, context.profiles)
        .map_err(|error| HandoffRejection::GraphInvalid(error.to_string()))?;
    let (Some(profiles), Some(agents)) = (context.profiles, context.agents) else {
        return Ok(());
    };
    let nodes = graph.nodes.values().chain(
        graph
            .subgraphs
            .values()
            .flat_map(|subgraph| subgraph.nodes.values()),
    );
    for node in nodes {
        let surge_core::node::NodeConfig::Agent(agent) = &node.config else {
            continue;
        };
        let key = surge_core::profile::keyref::parse_key_ref(agent.profile.as_str())
            .map_err(|error| HandoffRejection::GraphInvalid(error.to_string()))?;
        let resolved = profiles
            .resolve(&key)
            .map_err(|error| HandoffRejection::GraphInvalid(error.to_string()))?;
        let runtime = agent
            .runtime_override()
            .unwrap_or(&resolved.profile.runtime.agent_id);
        if agents.find_normalized(runtime).is_none() {
            return Err(HandoffRejection::GraphInvalid(format!(
                "node {} needs agent runtime {runtime}, which is not in the agent catalog",
                node.id
            )));
        }
    }
    Ok(())
}

/// Apply the last flow-gate answer's plan edits, re-validating the result.
/// Returns how many steps were edited.
fn apply_flow_gate_edits(
    events: &[ReadEvent],
    graph: &mut Graph,
    context: &HandoffContext<'_>,
) -> Result<usize, HandoffRejection> {
    let Some(response) = events
        .iter()
        .rev()
        .find_map(|event| match &event.payload.payload {
            EventPayload::HumanInputResolved { node, response, .. }
                if node.as_str() == FLOW_GATE_NODE =>
            {
                Some(response)
            },
            _ => None,
        })
    else {
        return Ok(0);
    };
    let edits = surge_core::node_overrides::NodeOverrides::from_gate_response(response)
        .map_err(|error| HandoffRejection::EditsInvalid(error.to_string()))?;
    if edits.is_empty() {
        return Ok(0);
    }
    edits
        .apply(graph)
        .map_err(|error| HandoffRejection::EditsInvalid(error.to_string()))?;
    validate(graph, context).map_err(|rejection| match rejection {
        HandoffRejection::GraphInvalid(reason) => HandoffRejection::EditsInvalid(reason),
        other => other,
    })?;
    Ok(edits.0.len())
}

#[cfg(test)]
mod tests;
