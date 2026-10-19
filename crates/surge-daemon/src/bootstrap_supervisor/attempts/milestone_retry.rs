//! Positive owner proof for retrying a failed milestone audit after all tasks completed.
use surge_core::{EventPayload as E, keys::NodeKey, node::NodeConfig};
use surge_orchestrator::engine::{
    execution_state::ExecutionState,
    frames::{Frame, LoopFrame},
    snapshot::EngineSnapshot,
};
use surge_persistence::runs::ReadEvent;

type Proof = Result<(), ()>;

pub(super) fn prove(
    prefix: &ExecutionState,
    snapshot: &EngineSnapshot,
    latest: &ExecutionState,
    tail: &[ReadEvent],
    run: surge_core::RunId,
) -> Proof {
    if snapshot.at_seq != prefix.position
        || snapshot.stage_boundary_seq != prefix.position
        || snapshot.pending_stage.is_some()
        || snapshot.pending_human_input.is_some()
        || prefix.graph != latest.graph
        || !same_task_history(prefix, latest)
        || !prefix.pending_graph_revisions.is_empty()
        || !latest.pending_graph_revisions.is_empty()
    {
        return Err(());
    }
    let [Frame::Loop(outer), Frame::Loop(inner)] = prefix.frames.as_slice() else {
        return Err(());
    };
    check_frames(prefix, outer, inner)?;
    for name in ["description", "roadmap", "flow", "spec"] {
        if prefix
            .memory
            .artifacts
            .get(name)
            .map(|artifact| artifact.hash)
            != latest
                .memory
                .artifacts
                .get(name)
                .map(|artifact| artifact.hash)
        {
            return Err(());
        }
    }
    let body = prefix.graph.subgraphs.get(&outer.config.body).ok_or(())?;
    let audit = body.nodes.get(&inner.return_to).ok_or(())?;
    let NodeConfig::Agent(agent) = &audit.config else {
        return Err(());
    };
    if !agent.profile.as_str().starts_with("app-tester@") {
        return Err(());
    }
    let mut rest = tail;
    let (head, remaining) = rest.split_at_checked(3).ok_or(())?;
    if !matches!(&head[0].payload.payload, E::StageEntered { node, .. } if node == &prefix.cursor.node)
        || !matches!(&head[1].payload.payload, E::LoopIterationCompleted { loop_id,index,outcome } if loop_id == &inner.loop_node && *index == inner.current_index && outcome.as_str() == "completed")
        || !matches!(&head[2].payload.payload, E::LoopCompleted { loop_id,completed_iterations,final_outcome } if loop_id == &inner.loop_node && *completed_iterations as usize == inner.items.len() && final_outcome.as_str() == "completed")
    {
        return Err(());
    }
    rest = remaining;
    let milestone = outer
        .items
        .get(outer.current_index as usize)
        .and_then(|item| item.get("id"))
        .and_then(toml::Value::as_str)
        .ok_or(())?;
    let spec = Some(bound_spec(agent, &prefix.memory)?);
    if bound_spec(agent, &latest.memory)? != spec.ok_or(())? {
        return Err(());
    }
    let (rework, remaining) = failed_stage(
        rest,
        &audit.id,
        &body.edges,
        surge_core::edge::EdgeKind::Backtrack,
        milestone,
        spec,
        run,
    )?;
    let NodeConfig::Agent(rework_agent) = &body.nodes.get(&rework).ok_or(())?.config else {
        return Err(());
    };
    if !rework_agent.profile.as_str().starts_with("implementer@") {
        return Err(());
    }
    let (terminal, remaining) = failed_stage(
        remaining,
        &rework,
        &body.edges,
        surge_core::edge::EdgeKind::Forward,
        milestone,
        spec,
        run,
    )?;
    failed_loop_terminal(remaining, &terminal, outer, body)
}

fn failed_loop_terminal(
    remaining: &[ReadEvent],
    terminal: &NodeKey,
    outer: &LoopFrame,
    body: &surge_core::graph::Subgraph,
) -> Proof {
    if !matches!(
        &body.nodes.get(terminal).ok_or(())?.config,
        NodeConfig::Terminal(surge_core::terminal_config::TerminalConfig {
            kind: surge_core::terminal_config::TerminalKind::Failure { .. },
            ..
        })
    ) {
        return Err(());
    }
    let [entered, iteration, completed, failed] = remaining else {
        return Err(());
    };
    if !matches!(&entered.payload.payload, E::StageEntered { node,.. } if node == terminal)
        || !matches!(&iteration.payload.payload, E::LoopIterationCompleted { loop_id,index,outcome } if loop_id == &outer.loop_node && *index == outer.current_index && outcome.as_str() == "failed")
        || !matches!(&completed.payload.payload, E::LoopCompleted { loop_id,completed_iterations,final_outcome } if loop_id == &outer.loop_node && *completed_iterations == outer.current_index+1 && final_outcome.as_str() == "aborted")
        || !matches!(&failed.payload.payload, E::RunFailed { .. })
    {
        return Err(());
    }
    Ok(())
}

fn bound_spec(
    agent: &surge_core::agent_config::AgentConfig,
    memory: &surge_core::run_state::RunMemory,
) -> Result<surge_core::ContentHash, ()> {
    use surge_core::agent_config::ArtifactSource;
    let binding = agent
        .bindings
        .iter()
        .find(|binding| binding.target.0 == "spec" && !binding.optional)
        .ok_or(())?;
    let reference = match &binding.source {
        ArtifactSource::RunArtifact { name } => memory.artifacts.get(name),
        ArtifactSource::NodeOutput { node, artifact } => {
            let entries = memory.artifacts_by_node.get(node).ok_or(())?;
            if let Some(name) = surge_core::artifact_contract::artifact_name(artifact) {
                surge_core::artifact_contract::select_canonical(entries, name.kind, name.format)
                    .map_err(|_| ())?
            } else {
                entries.iter().rev().find(|entry| &entry.name == artifact)
            }
        },
        _ => return Err(()),
    };
    reference.map(|reference| reference.hash).ok_or(())
}

fn same_task_history(before: &ExecutionState, after: &ExecutionState) -> bool {
    if before.memory.ledger.tasks.len() != after.memory.ledger.tasks.len() {
        return false;
    }
    before.memory.ledger.tasks.iter().all(|(id, original)| {
        let Some(current) = after.memory.ledger.tasks.get(id) else {
            return false;
        };
        if !original.verified && current.verified {
            return false;
        }
        let mut original = original.clone();
        let mut current = current.clone();
        original.verified = false;
        current.verified = false;
        original == current
    })
}

fn check_frames(state: &ExecutionState, outer: &LoopFrame, inner: &LoopFrame) -> Proof {
    let graph = &state.graph;
    if graph.nodes.get(&outer.loop_node).map(|node| &node.config)
        != Some(&NodeConfig::Loop(outer.config.clone()))
        || outer.current_index as usize >= outer.items.len()
        || inner.items.is_empty()
        || inner.current_index as usize + 1 != inner.items.len()
    {
        return Err(());
    }
    let outer_body = graph.subgraphs.get(&outer.config.body).ok_or(())?;
    if outer_body
        .nodes
        .get(&inner.loop_node)
        .map(|node| &node.config)
        != Some(&NodeConfig::Loop(inner.config.clone()))
    {
        return Err(());
    }
    let task_body = graph.subgraphs.get(&inner.config.body).ok_or(())?;
    if !matches!(
        task_body
            .nodes
            .get(&state.cursor.node)
            .map(|node| &node.config),
        Some(NodeConfig::Terminal(
            surge_core::terminal_config::TerminalConfig {
                kind: surge_core::terminal_config::TerminalKind::Success,
                ..
            }
        ))
    ) {
        return Err(());
    }
    let tasks = outer.items[outer.current_index as usize]
        .get("tasks")
        .and_then(toml::Value::as_array)
        .ok_or(())?;
    if tasks != &inner.items
        || !outer_body.edges.iter().any(|edge| {
            edge.from.node == inner.loop_node
                && edge.to == inner.return_to
                && edge.kind == surge_core::edge::EdgeKind::Forward
        })
        || !graph.edges.iter().any(|edge| {
            edge.from.node == outer.loop_node
                && edge.to == outer.return_to
                && edge.kind == surge_core::edge::EdgeKind::Forward
        })
    {
        return Err(());
    }
    for item in &inner.items {
        let id = item.get("id").and_then(toml::Value::as_str).ok_or(())?;
        let task = state
            .memory
            .ledger
            .tasks
            .get(&surge_core::roadmap::RoadmapTaskId::from(id))
            .ok_or(())?;
        if task.status != surge_core::roadmap::RoadmapStatus::Completed
            || task.last_authority_node.is_none()
            || task.accepted_by_human
            || task.requirement_revised
        {
            return Err(());
        }
    }
    last_verified_route(state, inner, task_body)
}

fn last_verified_route(
    state: &ExecutionState,
    inner: &LoopFrame,
    task_body: &surge_core::graph::Subgraph,
) -> Proof {
    // The saved route must be the successful verifier that completed the final task.
    let last = inner
        .items
        .last()
        .and_then(|item| item.get("id"))
        .and_then(toml::Value::as_str)
        .ok_or(())?;
    let task = state
        .memory
        .ledger
        .tasks
        .get(&surge_core::roadmap::RoadmapTaskId::from(last))
        .ok_or(())?;
    if !state
        .memory
        .committed_stage_outcomes
        .values()
        .any(|record| {
            !record.conflicting
                && record.routed_seq == Some(state.position)
                && Some(&record.commit.context().node) == task.last_authority_node.as_ref()
                && task_body
                    .nodes
                    .get(&record.commit.context().node)
                    .is_some_and(|node| {
                        node.declared_outcomes.iter().any(|outcome| {
                            outcome.id == *record.commit.outcome()
                                && outcome.ledger_effect == surge_core::node::LedgerEffect::Verified
                        })
                    })
                && task_body.edges.iter().any(|edge| {
                    edge.from.node == record.commit.context().node
                        && edge.from.outcome == *record.commit.outcome()
                        && edge.to == state.cursor.node
                })
        })
    {
        return Err(());
    }
    Ok(())
}

struct StageClaim<'a> {
    commit: &'a surge_core::execution_recovery::commit::StageOutcomeCommit,
    commit_seq: u64,
    session: surge_core::SessionId,
    milestone: &'a str,
    spec: Option<surge_core::ContentHash>,
}

fn failed_stage<'a>(
    events: &'a [ReadEvent],
    node: &NodeKey,
    edges: &[surge_core::edge::Edge],
    kind: surge_core::edge::EdgeKind,
    milestone: &str,
    spec: Option<surge_core::ContentHash>,
    run: surge_core::RunId,
) -> Result<(NodeKey, &'a [ReadEvent]), ()> {
    if !matches!(events.first().map(|event| &event.payload.payload), Some(E::StageEntered { node: entered, .. }) if entered == node)
    {
        return Err(());
    }
    let route_index = events
        .iter()
        .position(|event| matches!(event.payload.payload, E::StageRouteCommitted { .. }))
        .ok_or(())?;
    let stage = &events[..=route_index];
    let (commit_seq, commit) = accepted_commit(stage, node, run)?;
    let session = observed_session(stage, commit)?;
    let target = failed_edge(stage, edges, kind, commit)?;
    let claim = StageClaim {
        commit,
        commit_seq,
        session,
        milestone,
        spec,
    };
    for (index, event) in stage.iter().enumerate().skip(1) {
        if !valid_stage_event(event, index, stage.len(), &claim) {
            return Err(());
        }
    }
    for kind in ["OutcomeReported", "SessionClosed", "StageCompleted"] {
        if stage
            .iter()
            .filter(|event| event.payload.payload.discriminant_str() == kind)
            .count()
            != 1
        {
            return Err(());
        }
    }
    Ok((target, &events[route_index + 1..]))
}

fn accepted_commit<'a>(
    stage: &'a [ReadEvent],
    node: &NodeKey,
    run: surge_core::RunId,
) -> Result<
    (
        u64,
        &'a surge_core::execution_recovery::commit::StageOutcomeCommit,
    ),
    (),
> {
    let commits: Vec<_> = stage
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            if let E::StageOutcomeCommitted { commit } = &event.payload.payload {
                Some((index, event.seq.as_u64(), commit))
            } else {
                None
            }
        })
        .collect();
    let [(index, seq, commit)] = commits.as_slice() else {
        return Err(());
    };
    if &commit.context().node != node || commit.context().run != run || *index + 5 != stage.len() {
        return Err(());
    }
    let effects = stage
        .get(
            index
                .checked_sub(commit.effects_count() as usize)
                .ok_or(())?..*index,
        )
        .ok_or(())?
        .iter()
        .map(|event| event.payload.clone())
        .collect::<Vec<_>>();
    if surge_core::ContentHash::compute(&serde_json::to_vec(&effects).map_err(|_| ())?)
        != *commit.effects_hash()
    {
        return Err(());
    }
    Ok((*seq, *commit))
}

fn observed_session(
    stage: &[ReadEvent],
    commit: &surge_core::execution_recovery::commit::StageOutcomeCommit,
) -> Result<surge_core::SessionId, ()> {
    let observations: Vec<_> = stage
        .iter()
        .filter_map(|event| {
            if let E::SessionOpened {
                opened: Some(opened),
                node,
                session,
                ..
            } = &event.payload.payload
            {
                Some((opened, node, session))
            } else {
                None
            }
        })
        .collect();
    let [(opened, node, session)] = observations.as_slice() else {
        return Err(());
    };
    if *node != &commit.context().node
        || opened.descriptor.invocation() != commit.invocation()
        || **session != commit.provider_connection()
        || opened.session != **session
    {
        return Err(());
    }
    Ok(**session)
}

fn failed_edge(
    stage: &[ReadEvent],
    edges: &[surge_core::edge::Edge],
    kind: surge_core::edge::EdgeKind,
    commit: &surge_core::execution_recovery::commit::StageOutcomeCommit,
) -> Result<NodeKey, ()> {
    let traversals: Vec<_> = stage
        .iter()
        .filter_map(|event| {
            if let E::EdgeTraversed {
                edge,
                from,
                to,
                kind,
            } = &event.payload.payload
            {
                Some((edge, from, to, kind))
            } else {
                None
            }
        })
        .collect();
    let [(id, from, target, actual_kind)] = traversals.as_slice() else {
        return Err(());
    };
    if *from != &commit.context().node
        || **actual_kind != kind
        || !edges.iter().any(|edge| {
            &edge.id == *id
                && &edge.from.node == *from
                && edge.from.outcome == *commit.outcome()
                && &edge.to == *target
                && edge.kind == kind
        })
    {
        return Err(());
    }
    Ok((*target).clone())
}

fn valid_stage_event(event: &ReadEvent, index: usize, len: usize, claim: &StageClaim<'_>) -> bool {
    let commit = claim.commit;
    let node = &commit.context().node;
    match &event.payload.payload {
        E::OutcomeReported {
            node: actual,
            outcome,
            ..
        } => actual == node && outcome == commit.outcome() && index + 6 == len,
        E::StageOutcomeCommitted { .. } => {
            event.seq.as_u64() == claim.commit_seq && index + 5 == len
        },
        E::SessionClosed { session, .. } => *session == claim.session && index + 4 == len,
        E::EdgeTraversed { .. } => index + 3 == len,
        E::StageCompleted {
            node: actual,
            outcome,
        } => actual == node && outcome == commit.outcome() && index + 2 == len,
        E::StageRouteCommitted {
            invocation,
            outcome_commit_seq,
        } => {
            *invocation == commit.invocation()
                && *outcome_commit_seq == claim.commit_seq
                && index + 1 == len
        },
        _ => index + 6 < len && stage_metadata_matches(&event.payload.payload, claim),
    }
}

fn stage_metadata_matches(payload: &E, claim: &StageClaim<'_>) -> bool {
    let commit = claim.commit;
    let node = &commit.context().node;
    match payload {
        E::StageInputsResolved {
            node: actual,
            bindings,
            ..
        } => actual == node && bindings.get("spec").copied() == claim.spec,
        E::OutcomeRejectedByHook { node: actual, .. } => actual == node,
        E::ArtifactProduced {
            node: actual,
            name,
            source_path,
            ..
        } => actual == node && !requirement_artifact(name, source_path.as_deref()),
        E::VerificationSubjectObserved { .. } => true,
        E::VerificationCriteriaAccepted { task_id, criteria } => {
            task_id.as_str() == claim.milestone && criteria.is_none()
        },
        E::ExecutionWriterIntent { intent } => {
            intent.invocation == commit.invocation()
                && matches!(
                    intent.kind,
                    surge_core::execution_recovery::process::ExecutionWriterKind::Provider
                )
        },
        E::SessionEstablishmentRequested {
            node: actual,
            invocation,
            authority: Some(authority),
            restore,
        } => {
            actual == node
                && *invocation == commit.invocation()
                && authority == commit.context()
                && !restore
        },
        E::SessionOpened {
            opened: Some(opened),
            node: actual,
            session,
            ..
        } => actual == node && opened.session == claim.session && *session == claim.session,
        E::ContextUsageObserved { session, .. }
        | E::ProviderUsageObserved { session, .. }
        | E::TokensConsumed { session, .. } => *session == claim.session,
        E::StageToolReceipt { receipt } => &receipt.context == commit.context(),
        _ => false,
    }
}

fn requirement_artifact(name: &str, path: Option<&std::path::Path>) -> bool {
    use surge_core::artifact_contract::{ArtifactKind, artifact_name};
    let named = |name: &str| {
        artifact_name(name).is_some_and(|name| {
            matches!(
                name.kind,
                ArtifactKind::Description
                    | ArtifactKind::Requirements
                    | ArtifactKind::Roadmap
                    | ArtifactKind::RoadmapPatch
                    | ArtifactKind::Spec
                    | ArtifactKind::Flow
            )
        })
    };
    named(name)
        || path
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .is_some_and(named)
}

#[cfg(test)]
mod tests;
