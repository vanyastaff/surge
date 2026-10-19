//! Sanitized controlled-provider recording of the retained task-end/audit/rework shape.
//! Fixed roadmap and node identities come from checked-in independent fixture artifacts.
use super::*;
use serde::Deserialize;
use surge_core::{ContentHash, VersionedEventPayload};
use surge_orchestrator::engine::execution_state::CursorSource;

#[derive(Deserialize)]
struct Fixture {
    boundary: u64,
    snapshot: EngineSnapshot,
    events: Vec<Row>,
}
#[derive(Deserialize)]
struct Row {
    seq: u64,
    timestamp_ms: i64,
    kind: String,
    payload: VersionedEventPayload,
}
fn fixture() -> (
    ExecutionState,
    EngineSnapshot,
    ExecutionState,
    Vec<ReadEvent>,
) {
    let mut fixture: Fixture = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/bootstrap-continuation/failed-milestone-shape.json"
    ))
    .unwrap();
    restore_frozen_items(&mut fixture.snapshot);
    let raw_graph: surge_core::Graph = toml::from_str(include_str!(
        "../../../../tests/fixtures/bootstrap-continuation/synthetic-eight-task-flow.toml"
    ))
    .unwrap();
    let graph = surge_core::escalation::with_default_escalation_gates(&raw_graph);
    let run = fixture
        .events
        .iter()
        .find_map(|event| {
            if let E::StageOutcomeCommitted { commit } = &event.payload.payload {
                Some(commit.context().run)
            } else {
                None
            }
        })
        .unwrap();
    let mut rows: Vec<_> = fixture
        .events
        .drain(..)
        .map(|row| ReadEvent {
            seq: surge_persistence::runs::EventSeq(row.seq),
            timestamp_ms: row.timestamp_ms,
            kind: row.kind,
            payload: row.payload,
        })
        .collect();
    seal_sanitized_fixture(&mut rows, fixture.boundary);
    let mut memory = surge_core::run_state::RunMemory::default();
    memory.apply_event(&surge_core::RunEvent {
        run_id: run,
        seq: 1,
        timestamp: chrono::Utc::now(),
        payload: E::PipelineMaterialized {
            graph_hash: graph.identity().unwrap(),
            graph: Box::new(graph.clone()),
        },
    });
    for row in rows
        .iter()
        .filter(|row| row.seq.as_u64() <= fixture.boundary)
    {
        memory.apply_event(&row.to_run_event(run).unwrap());
    }
    let prefix = ExecutionState {
        position: fixture.boundary,
        cursor: fixture.snapshot.cursor.clone().into_cursor().unwrap(),
        cursor_source: CursorSource::Checkpoint {
            seq: fixture.boundary,
        },
        frames: fixture
            .snapshot
            .frames
            .clone()
            .into_iter()
            .map(|frame| frame.try_into().unwrap())
            .collect(),
        root_traversal_counts: std::collections::HashMap::new(),
        memory,
        memory_applied_seq: fixture.boundary,
        graph,
        applied_graph_revision_seq: 0,
        pending_graph_revisions: vec![],
        run_config: None,
        terminal_verdict: surge_core::run_verdict::TerminalVerdict::Open,
    };
    let tail: Vec<_> = rows
        .into_iter()
        .filter(|row| row.seq.as_u64() > fixture.boundary)
        .collect();
    let mut latest = prefix.clone();
    for row in &tail {
        latest.memory.apply_event(&row.to_run_event(run).unwrap());
    }
    latest.position = tail.last().unwrap().seq.as_u64();
    (prefix, fixture.snapshot, latest, tail)
}

fn restore_frozen_items(snapshot: &mut EngineSnapshot) {
    let roadmap: toml::Value = toml::from_str(include_str!(
        "../../../../tests/fixtures/bootstrap-continuation/roadmap.toml"
    ))
    .unwrap();
    let milestones = roadmap["milestones"].as_array().unwrap();
    if let surge_orchestrator::engine::snapshot::SerializableFrame::Loop(frame) =
        &mut snapshot.frames[0]
    {
        frame.items_json = milestones
            .iter()
            .map(|item| serde_json::to_value(item).unwrap())
            .collect();
    }
    if let surge_orchestrator::engine::snapshot::SerializableFrame::Loop(frame) =
        &mut snapshot.frames[1]
    {
        frame.items_json = milestones[0]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| serde_json::to_value(item).unwrap())
            .collect();
    }
}
fn seal_sanitized_fixture(rows: &mut [ReadEvent], boundary: u64) {
    for row in &mut *rows {
        if let E::TaskVerified {
            evidence,
            report: Some(report),
            ..
        } = &mut row.payload.payload
        {
            *evidence = ContentHash::compute(toml::to_string(report).unwrap().as_bytes());
        }
    }
    // Sanitization changes summaries; seal only this synthetic fixture's ordered effects.
    for index in 0..rows.len() {
        if rows[index].seq.as_u64() <= boundary {
            continue;
        }
        let E::StageOutcomeCommitted { commit } = &rows[index].payload.payload else {
            continue;
        };
        let effects = rows[index - commit.effects_count() as usize..index]
            .iter()
            .map(|row| row.payload.clone())
            .collect::<Vec<_>>();
        let mut encoded = serde_json::to_value(commit).unwrap();
        encoded["effects_hash"] =
            serde_json::to_value(ContentHash::compute(&serde_json::to_vec(&effects).unwrap()))
                .unwrap();
        rows[index].payload.payload = E::StageOutcomeCommitted {
            commit: serde_json::from_value(encoded).unwrap(),
        };
    }
}

fn owner_run() -> surge_core::RunId {
    "01M59GJAK0RT117XS783WP2H3M".parse().unwrap()
}

#[test]
fn positive_shape_preserves_completed_history_with_current_freshness_unknown() {
    let (prefix, snapshot, latest, tail) = fixture();
    assert_eq!(prefix.frames.len(), 2);
    assert_eq!(
        prefix
            .memory
            .ledger
            .tasks
            .values()
            .filter(|task| task.status == surge_core::roadmap::RoadmapStatus::Completed)
            .count(),
        4
    );
    prove(&prefix, &snapshot, &latest, &tail, owner_run()).unwrap();
    // A later owner can retain a verifier commit from an earlier fork without rewriting it.
    let mut inherited = prefix.clone();
    let mut inherited_latest = latest.clone();
    for memory in [&mut inherited.memory, &mut inherited_latest.memory] {
        let record = memory
            .committed_stage_outcomes
            .values_mut()
            .find(|record| record.routed_seq == Some(prefix.position))
            .unwrap();
        let mut encoded = serde_json::to_value(&record.commit).unwrap();
        encoded["context"]["run"] = serde_json::to_value(surge_core::RunId::new()).unwrap();
        record.commit = serde_json::from_value(encoded).unwrap();
    }
    prove(&inherited, &snapshot, &inherited_latest, &tail, owner_run()).unwrap();
    assert!(
        prove(
            &inherited,
            &snapshot,
            &inherited_latest,
            &tail,
            surge_core::RunId::new()
        )
        .is_err()
    );
}

#[test]
fn one_event_tail_mutations_refuse_at_every_position() {
    let (prefix, snapshot, latest, tail) = fixture();
    let node = NodeKey::try_from("verify_task").unwrap();
    let forbidden = [
        E::TaskSplit {
            loop_id: NodeKey::try_from("task_loop").unwrap(),
            index: 3,
            task: Some("m1-t4".into()),
            into: vec![],
        },
        E::TaskVerified {
            task_id: surge_core::roadmap::RoadmapTaskId::from("m2-t1"),
            node: node.clone(),
            evidence: ContentHash::compute(b"foreign"),
            report: None,
        },
        E::TaskDiscovered {
            task_id: surge_core::roadmap::RoadmapTaskId::from("foreign"),
            discovered_from: surge_core::roadmap::RoadmapTaskId::from("m1-t4"),
            title: "foreign".into(),
        },
        E::StageCompleted {
            node: NodeKey::try_from("app_milestone").unwrap(),
            outcome: surge_core::OutcomeKey::try_from("passed").unwrap(),
        },
        E::VerificationCriteriaAccepted {
            task_id: surge_core::roadmap::RoadmapTaskId::from("m1-t4"),
            criteria: None,
        },
    ];
    for payload in forbidden {
        for index in 0..=tail.len() {
            let mut modified = tail.clone();
            modified.insert(
                index,
                ReadEvent {
                    seq: surge_persistence::runs::EventSeq(999),
                    timestamp_ms: 0,
                    kind: payload.discriminant_str().into(),
                    payload: VersionedEventPayload::new(payload.clone()),
                },
            );
            assert!(
                prove(&prefix, &snapshot, &latest, &modified, owner_run()).is_err(),
                "{} at {index}",
                payload.discriminant_str()
            );
        }
    }
    for index in 0..tail.len() {
        let mut modified = tail.clone();
        if let E::StageRouteCommitted { invocation, .. } = &mut modified[index].payload.payload {
            *invocation = surge_core::id::StageInvocationId::new();
            assert!(prove(&prefix, &snapshot, &latest, &modified, owner_run()).is_err());
        }
    }
}

#[test]
fn frame_requirement_and_checkpoint_drift_refuse() {
    let (prefix, snapshot, latest, tail) = fixture();
    let mut wrong = prefix.clone();
    let Frame::Loop(inner) = &mut wrong.frames[1] else {
        panic!("task frame")
    };
    inner.current_index -= 1;
    assert!(prove(&wrong, &snapshot, &latest, &tail, owner_run()).is_err());
    let mut wrong = prefix.clone();
    let Frame::Loop(inner) = &mut wrong.frames[1] else {
        panic!("task frame")
    };
    inner.return_to = NodeKey::try_from("impl_task").unwrap();
    assert!(prove(&wrong, &snapshot, &latest, &tail, owner_run()).is_err());
    let mut wrong = prefix.clone();
    let Frame::Loop(outer) = &mut wrong.frames[0] else {
        panic!("milestone frame")
    };
    outer.items[0].as_table_mut().unwrap().remove("tasks");
    assert!(prove(&wrong, &snapshot, &latest, &tail, owner_run()).is_err());
    let mut wrong = snapshot.clone();
    wrong.stage_boundary_seq -= 1;
    assert!(prove(&prefix, &wrong, &latest, &tail, owner_run()).is_err());
    let mut changed = latest;
    changed
        .memory
        .ledger
        .tasks
        .get_mut(&surge_core::roadmap::RoadmapTaskId::from("m1-t4"))
        .unwrap()
        .requirement_revised = true;
    assert!(prove(&prefix, &snapshot, &changed, &tail, owner_run()).is_err());
}
