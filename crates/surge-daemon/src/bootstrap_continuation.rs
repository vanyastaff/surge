//! Validate durable planning output before committing or replaying child launch intent.
use crate::bootstrap_runtime::BootstrapRuntime;
use std::{collections::HashSet, sync::Arc};
use surge_core::{
    ContentHash, RunId,
    bootstrap_continuation::BootstrapContinuation,
    bootstrap_operation::{BootstrapContentRef, BootstrapIntent},
    budget::BudgetGuard,
    graph::Graph,
    run_event::EventPayload,
    run_state::ArtifactRef,
};
use surge_orchestrator::{
    bootstrap_driver::materialized_run_from_completed,
    engine::{Engine, RunOutcome, config::RunSeedArtifact},
};
use surge_persistence::{
    artifacts::ArtifactStore,
    runs::{ReadEvent, Storage, inspection::RunDatabaseInspection},
};

/// A continuation refusal; no child launch intent may be committed.
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum ContinuationError {
    /// Parent success or storage could not be confirmed.
    #[error("planning completion could not be confirmed")]
    Unconfirmed,
    /// Required approved output is missing, inconsistent, or invalid.
    #[error("planning artifacts or generated graph could not be validated")]
    Invalid,
    /// Recorded usage exhausted the operation allowance.
    #[error("planning exhausted the shared operation budget")]
    Exhausted,
    /// Usage evidence is insufficient to establish remaining allowance.
    #[error("remaining operation budget could not be confirmed")]
    BudgetUnconfirmed,
}

pub(crate) async fn prepare_child(
    engine: &Engine,
    storage: &Arc<Storage>,
    runtime: &BootstrapRuntime,
    parent: RunId,
    worktree: &std::path::Path,
    intent: &BootstrapIntent,
) -> Result<BootstrapContinuation, ContinuationError> {
    let RunDatabaseInspection::Present { events } = storage
        .inspect_run(parent)
        .await
        .map_err(|_| ContinuationError::Unconfirmed)?
        .database
    else {
        return Err(ContinuationError::Unconfirmed);
    };
    let terminal = events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::RunCompleted { terminal_node } => Some(terminal_node.clone()),
            _ => None,
        })
        .ok_or(ContinuationError::Unconfirmed)?;
    if !crate::tracked_run::confirms(&events, &RunOutcome::Completed { terminal }) {
        return Err(ContinuationError::Unconfirmed);
    }
    let materialized = materialized_run_from_completed(engine, parent)
        .await
        .map_err(|_| ContinuationError::Invalid)?;
    let mut graph =
        validate_artifacts(storage, runtime, parent, worktree, &materialized.artifacts).await?;
    if graph.graph != materialized.materialized_graph {
        return Err(ContinuationError::Invalid);
    }
    apply_operator_edits(storage, runtime, parent, &mut graph).await?;
    let budget = remaining_budget(intent.budget(), &events)?;
    BootstrapContinuation::new(
        parent,
        BootstrapContentRef {
            name: "materialized:flow".into(),
            digest: graph_hash(&graph.graph)?,
        },
        materialized.artifacts,
        budget,
    )
    .map_err(|_| ContinuationError::Invalid)
}

pub(crate) async fn load_child(
    storage: &Arc<Storage>,
    runtime: &BootstrapRuntime,
    worktree: &std::path::Path,
    child: &BootstrapContinuation,
) -> Result<ValidatedChild, ContinuationError> {
    let mut graph = validate_artifacts(
        storage,
        runtime,
        child.parent(),
        worktree,
        child.artifacts(),
    )
    .await?;
    // Same edits, same order as `prepare_child`: the pinned digest is of the
    // edited graph, so recovery reproduces exactly what was approved.
    apply_operator_edits(storage, runtime, child.parent(), &mut graph).await?;
    if graph_hash(&graph.graph)? != child.graph().digest {
        return Err(ContinuationError::Invalid);
    }
    Ok(graph)
}

/// Node id of the bootstrap flow approval gate (bundled `bootstrap` flow).
const FLOW_GATE_NODE: &str = "flow_gate";

/// Apply the plan edits the operator approved the flow with
/// ([`surge_core::node_overrides::NodeOverrides`] in the flow gate's
/// resolved response), re-validate, and make the `flow.toml` seed match.
async fn apply_operator_edits(
    storage: &Arc<Storage>,
    runtime: &BootstrapRuntime,
    parent: RunId,
    child: &mut ValidatedChild,
) -> Result<(), ContinuationError> {
    let RunDatabaseInspection::Present { events } = storage
        .inspect_run(parent)
        .await
        .map_err(|_| ContinuationError::Unconfirmed)?
        .database
    else {
        return Err(ContinuationError::Unconfirmed);
    };
    let applied = apply_edits_from_events(&events, runtime, child)?;
    if applied > 0 {
        tracing::info!(%parent, steps = applied, "applied operator plan edits to the approved flow");
    }
    Ok(())
}

/// Pure core of [`apply_operator_edits`]: apply the last flow-gate
/// approval's edits from `events`. Returns how many steps were edited.
fn apply_edits_from_events(
    events: &[ReadEvent],
    runtime: &BootstrapRuntime,
    child: &mut ValidatedChild,
) -> Result<usize, ContinuationError> {
    let response = events
        .iter()
        .rev()
        .find_map(|event| match &event.payload.payload {
            EventPayload::HumanInputResolved { node, response, .. }
                if node.as_str() == FLOW_GATE_NODE =>
            {
                Some(response.clone())
            },
            _ => None,
        });
    let Some(response) = response else {
        return Ok(0);
    };
    let edits = surge_core::node_overrides::NodeOverrides::from_gate_response(&response)
        .map_err(|_| ContinuationError::Invalid)?;
    if edits.is_empty() {
        return Ok(0);
    }
    edits
        .apply(&mut child.graph)
        .map_err(|_| ContinuationError::Invalid)?;
    runtime
        .validate_graph(&child.graph)
        .map_err(|_| ContinuationError::Invalid)?;
    surge_orchestrator::engine::validate::validate_for_m6(&child.graph)
        .map_err(|_| ContinuationError::Invalid)?;
    let text = toml::to_string(&child.graph).map_err(|_| ContinuationError::Invalid)?;
    let seed = RunSeedArtifact::new("flow", "flow.toml", &text, "bootstrap_parent")
        .map_err(|_| ContinuationError::Invalid)?;
    match child.seeds.iter_mut().find(|s| s.name == "flow") {
        Some(existing) => *existing = seed,
        None => child.seeds.push(seed),
    }
    Ok(edits.0.len())
}

async fn validate_artifacts(
    storage: &Arc<Storage>,
    runtime: &BootstrapRuntime,
    parent: RunId,
    worktree: &std::path::Path,
    artifacts: &[ArtifactRef],
) -> Result<ValidatedChild, ContinuationError> {
    use surge_core::artifact_contract::{ArtifactKind, validate_artifact_text};
    let store = ArtifactStore::new(storage.home().join("runs"));
    let mut graph = None;
    let mut seeds = Vec::new();
    for (name, kind) in [
        ("description", ArtifactKind::Description),
        ("roadmap", ArtifactKind::Roadmap),
        ("flow", ArtifactKind::Flow),
    ] {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.name == name)
            .ok_or(ContinuationError::Invalid)?;
        let bytes = store
            .open_ref(parent, artifact, worktree)
            .await
            .map_err(|_| ContinuationError::Invalid)?;
        if ContentHash::compute(&bytes) != artifact.hash {
            return Err(ContinuationError::Invalid);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| ContinuationError::Invalid)?;
        if !validate_artifact_text(kind, text).is_valid() {
            return Err(ContinuationError::Invalid);
        }
        if name == "flow" {
            graph = Some(toml::from_str::<Graph>(text).map_err(|_| ContinuationError::Invalid)?);
        }
        let path = if name == "flow" {
            "flow.toml".into()
        } else if name == "roadmap" && toml::from_str::<toml::Value>(text).is_ok() {
            "roadmap.toml".into()
        } else {
            format!("{name}.md")
        };
        seeds.push(
            RunSeedArtifact::new(name, path, text, "bootstrap_parent")
                .map_err(|_| ContinuationError::Invalid)?,
        );
    }
    let graph = graph.ok_or(ContinuationError::Invalid)?;
    runtime
        .validate_graph(&graph)
        .map_err(|_| ContinuationError::Invalid)?;
    surge_orchestrator::engine::validate::validate_for_m6(&graph)
        .map_err(|_| ContinuationError::Invalid)?;
    Ok(ValidatedChild { graph, seeds })
}

pub(crate) struct ValidatedChild {
    pub graph: Graph,
    pub seeds: Vec<RunSeedArtifact>,
}

pub(crate) fn graph_hash(graph: &Graph) -> Result<ContentHash, ContinuationError> {
    Ok(ContentHash::compute(
        &serde_json::to_vec(graph).map_err(|_| ContinuationError::Invalid)?,
    ))
}

fn remaining_budget(
    mut budget: BudgetGuard,
    events: &[ReadEvent],
) -> Result<BudgetGuard, ContinuationError> {
    let Some(limit) = budget.limits.tokens else {
        return Ok(budget);
    };
    let mut sessions = HashSet::new();
    let mut observed = HashSet::new();
    let mut used = 0_u64;
    for event in events {
        match &event.payload.payload {
            EventPayload::SessionOpened { session, .. } => {
                sessions.insert(*session);
            },
            EventPayload::TokensConsumed {
                session,
                prompt_tokens,
                output_tokens,
                ..
            } => {
                observed.insert(*session);
                used = used
                    .checked_add(u64::from(*prompt_tokens) + u64::from(*output_tokens))
                    .ok_or(ContinuationError::BudgetUnconfirmed)?;
            },
            _ => {},
        }
    }
    if !sessions.is_subset(&observed) {
        return Err(ContinuationError::BudgetUnconfirmed);
    }
    let remaining = limit
        .checked_sub(used)
        .filter(|remaining| *remaining > 0)
        .ok_or(ContinuationError::Exhausted)?;
    budget.limits.tokens = Some(remaining);
    Ok(budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::{SessionId, VersionedEventPayload, keys::NodeKey};
    fn event(payload: EventPayload) -> ReadEvent {
        ReadEvent {
            seq: surge_persistence::runs::EventSeq(1),
            timestamp_ms: 0,
            kind: payload.discriminant_str().into(),
            payload: VersionedEventPayload::new(payload),
        }
    }
    fn runtime() -> (tempfile::TempDir, BootstrapRuntime) {
        let temp = tempfile::tempdir().unwrap();
        let config = Arc::new(surge_core::SurgeConfig::default());
        let profiles = Arc::new(surge_orchestrator::profile_loader::ProfileRegistry::new(
            surge_orchestrator::profile_loader::DiskProfileSet::empty(),
        ));
        let agents = Arc::new(surge_acp::Registry::builtin());
        let runtime = BootstrapRuntime::new(
            config,
            profiles,
            agents,
            temp.path().join("worktrees"),
            temp.path().join("profiles"),
        )
        .unwrap();
        (temp, runtime)
    }

    fn approved_child() -> ValidatedChild {
        let graph = surge_core::BundledFlows::by_name_latest("linear-3")
            .unwrap()
            .graph;
        let text = toml::to_string(&graph).unwrap();
        ValidatedChild {
            seeds: vec![
                RunSeedArtifact::new("flow", "flow.toml", &text, "bootstrap_parent").unwrap(),
            ],
            graph,
        }
    }

    fn gate_approval(response: serde_json::Value) -> ReadEvent {
        event(EventPayload::HumanInputResolved {
            node: NodeKey::try_new("flow_gate").unwrap(),
            call_id: None,
            response,
        })
    }

    #[test]
    fn approved_plan_edits_reach_the_child_graph_and_its_flow_seed() {
        let (_temp, runtime) = runtime();
        let mut child = approved_child();
        let step = child
            .graph
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.config, surge_core::node::NodeConfig::Agent(_)))
            .map(|(k, _)| k.as_str().to_string())
            .unwrap();
        let events = vec![gate_approval(serde_json::json!({
            "outcome": "approve",
            "node_overrides": { step.clone(): { "agent_id": "claude-acp" } }
        }))];
        let applied = apply_edits_from_events(&events, &runtime, &mut child).unwrap();
        assert_eq!(applied, 1);
        let seeded: surge_core::graph::Graph =
            toml::from_str(&child.seeds[0].content).expect("seed is the edited graph");
        assert_eq!(seeded, child.graph, "flow seed matches what will run");
        let surge_core::node::NodeConfig::Agent(agent) = &child
            .graph
            .nodes
            .iter()
            .find(|(k, _)| k.as_str() == step)
            .unwrap()
            .1
            .config
        else {
            unreachable!()
        };
        assert_eq!(agent.runtime_override(), Some("claude-acp"));
    }

    #[test]
    fn approval_without_edits_leaves_the_plan_untouched() {
        let (_temp, runtime) = runtime();
        let mut child = approved_child();
        let before = child.graph.clone();
        let events = vec![gate_approval(serde_json::json!({"outcome": "approve"}))];
        assert_eq!(
            apply_edits_from_events(&events, &runtime, &mut child).unwrap(),
            0
        );
        assert_eq!(child.graph, before);
        let bad = vec![gate_approval(serde_json::json!({
            "outcome": "approve",
            "node_overrides": { "no_such_step": { "agent_id": "claude-acp" } }
        }))];
        assert!(matches!(
            apply_edits_from_events(&bad, &runtime, &mut child),
            Err(ContinuationError::Invalid)
        ));
    }

    #[test]
    fn shared_token_budget_subtracts_planning_and_never_turns_zero_into_unlimited() {
        let session = SessionId::new();
        let mut budget = BudgetGuard::default();
        budget.limits.tokens = Some(100);
        let mut events = vec![event(EventPayload::SessionOpened {
            handoff: None,
            opened: None,
            node: NodeKey::try_new("planner").unwrap(),
            session,
            agent: "planner".into(),
            agent_id: None,
        })];
        assert!(matches!(
            remaining_budget(budget, &events),
            Err(ContinuationError::BudgetUnconfirmed)
        ));
        events.push(event(EventPayload::TokensConsumed {
            session,
            prompt_tokens: 20,
            output_tokens: 30,
            cache_hits: 0,
            model: "fixture".into(),
            cost_usd: None,
        }));
        assert_eq!(
            remaining_budget(budget, &events).unwrap().limits.tokens,
            Some(50)
        );
        events.push(event(EventPayload::TokensConsumed {
            session,
            prompt_tokens: 10,
            output_tokens: 40,
            cache_hits: 0,
            model: "fixture".into(),
            cost_usd: None,
        }));
        assert!(matches!(
            remaining_budget(budget, &events),
            Err(ContinuationError::Exhausted)
        ));
    }
}
