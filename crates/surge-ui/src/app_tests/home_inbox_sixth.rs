//! Observable operation/run separation at the actual application and Inbox boundary.
use super::*;
use surge_core::{
    EventPayload as E,
    bootstrap_continuation::{BootstrapFailure as F, BootstrapTerminal as T},
    bootstrap_operation::{
        BootstrapAttentionReason as A, BootstrapOperationStatus as Status, BootstrapPhase as P,
        BootstrapState as S,
    },
};
use surge_orchestrator::engine::handle::{EngineRunEvent, RunStatus};
use surge_persistence::runs::{ReadEvent, seq::EventSeq};

fn start(path: &std::path::Path) -> E {
    E::RunStarted {
        pipeline_template: None,
        project_path: path.into(),
        initial_prompt: "build timer".into(),
        config: surge_core::RunConfig {
            budget: Default::default(),
            sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
            approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: Vec::new(),
            bootstrap_edit_loop_cap: None,
        },
    }
}
fn rows(payloads: Vec<E>) -> Vec<ReadEvent> {
    payloads
        .into_iter()
        .enumerate()
        .map(|(index, payload)| ReadEvent {
            seq: EventSeq(index as u64 + 1),
            timestamp_ms: 1_700_000_000_000 + index as i64 * 1000,
            kind: String::new(),
            payload: surge_core::VersionedEventPayload::new(payload),
        })
        .collect()
}
fn completed() -> E {
    E::RunCompleted {
        terminal_node: "bootstrap_complete".parse().unwrap(),
    }
}
fn failed() -> E {
    E::RunFailed {
        error: "actual implementation failure".into(),
    }
}
fn status(
    operation: RunId,
    planning: RunId,
    implementation: RunId,
    state: S,
    result: Option<T>,
    revision: u64,
) -> Status {
    Status {
        operation_id: operation,
        planning_run: planning,
        implementation_run: implementation,
        state,
        result,
        revision,
        cancel_requested: false,
        queue_sequence: 1,
        payload_version: 1,
        continuations: Vec::new(),
    }
}
struct Harness {
    app: Entity<SurgeApp>,
    state: Entity<AppState>,
    root: tempfile::TempDir,
    cx: TestAppContext,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.app.update(&mut self.cx, |app, _| {
            app.inbox = None;
        });
        self.cx.update(|_| {});
    }
}
impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut cx = TestAppContext::single();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init();
            crate::theme::sync_component_theme(cx);
        });
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.runtime_home = Some(root.path().into());
            state.project_path = Some(PROJECT.into());
            state.dismissed_runs = Default::default();
            state
        });
        let app = cx.new(|cx| {
            SurgeApp::new_shell_at(
                state.clone(),
                Some(root.path().join("desktop/index.json")),
                cx,
            )
        });
        Self {
            cx,
            app,
            state,
            root,
        }
    }
    fn run(&mut self, id: RunId, payloads: Vec<E>, coarse: RunStatus) {
        let events = rows(payloads);
        self.state.update(&mut self.cx, |state, _| {
            let stream = crate::run_stream::RunStreamState::replay_candidate(id, &events);
            state.run_streams.insert(id, stream);
            state.runs.push(crate::app_state::UiRun {
                run_id: id,
                status: coarse,
                started_at: chrono::DateTime::from_timestamp_millis(events[0].timestamp_ms)
                    .unwrap(),
                ended_at: None,
                last_event_seq: events.last().map(|event| event.seq.as_u64()),
            });
        });
    }
    fn update(&mut self, snapshot: Status) {
        self.app.update(&mut self.cx, |app, cx| {
            app.update_bootstrap_view(
                snapshot.operation_id,
                snapshot,
                std::path::Path::new(PROJECT),
                cx,
            )
        });
    }
    fn queue(&mut self) -> Vec<(String, &'static str)> {
        self.app.update(&mut self.cx, |app, cx| {
            app.navigate(Screen::Inbox, cx);
            let _ = app.render_screen_content(cx);
        });
        let inbox = self.cx.read(|cx| self.app.read(cx).inbox.clone().unwrap());
        let (_, _window) = self.cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(inbox.clone(), window, cx)
        });
        inbox.update(&mut self.cx, |screen, cx| screen.queue_for_test(cx))
    }
}
#[test]
fn sixth_operation_only_attention_budget_and_cancel_do_not_create_reserved_runs() {
    for state in [
        S::NeedsAttention {
            phase: P::QueuedPlanning,
            run_id: None,
            reason: A::StorageUnconfirmed,
        },
        S::Failed,
        S::Cancelled,
    ] {
        let mut h = Harness::new();
        let operation = RunId::new();
        let planning = RunId::new();
        let child = RunId::new();
        let result = matches!(state, S::Failed).then_some(T::Failed {
            run_id: planning,
            reason: F::BudgetExhausted,
        });
        h.update(status(operation, planning, child, state.clone(), result, 1));
        let queue = h.queue();
        let count = usize::from(!matches!(state, S::Cancelled));
        assert_eq!(queue.len(), count);
        if count == 1 {
            assert_eq!(queue[0], (format!("operation-{operation}"), "operation"));
        }
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert!(state.runs.is_empty());
            assert!(state.run_streams.is_empty());
            assert_eq!(state.needs_you_count(), count);
        });
    }
}
#[test]
fn sixth_exact_terminal_failure_dedup_ignores_coarse_lifecycle_and_preserves_dismissals() {
    for (which, reason, terminal) in [
        (false, F::RunFailed, failed()),
        (true, F::RunFailed, failed()),
        (
            false,
            F::RunAborted,
            E::RunAborted {
                reason: "stop".into(),
            },
        ),
    ] {
        let mut h = Harness::new();
        let operation = RunId::new();
        let planning = RunId::new();
        let child = RunId::new();
        let failure = if which { planning } else { child };
        h.run(
            if which { child } else { planning },
            vec![start(std::path::Path::new(PROJECT)), completed()],
            RunStatus::Failed,
        );
        h.run(
            failure,
            vec![start(std::path::Path::new(PROJECT)), terminal],
            RunStatus::Active,
        );
        h.update(status(
            operation,
            planning,
            child,
            S::Failed,
            Some(T::Failed {
                run_id: failure,
                reason,
            }),
            3,
        ));
        let queue = h.queue();
        assert_eq!(
            queue,
            vec![(
                format!("failed-{failure}"),
                if h.cx.read(|cx| h.state.read(cx).failure_terminal(failure))
                    == Some(surge_core::TerminalReason::Failed)
                {
                    "failed"
                } else {
                    "aborted"
                }
            )]
        );
        h.state
            .update(&mut h.cx, |state, _| state.dismissed_runs.insert(failure));
        assert!(h.queue().is_empty());
    }
}
#[test]
fn sixth_attention_on_nonterminal_run_keeps_pending_decision_and_actual_live_flag() {
    let mut h = Harness::new();
    let operation = RunId::new();
    let planning = RunId::new();
    let child = RunId::new();
    h.run(
        planning,
        vec![start(std::path::Path::new(PROJECT))],
        RunStatus::Active,
    );
    h.state.update(&mut h.cx, |state, _| {
        let stream = state.run_streams.get_mut(&planning).unwrap();
        stream.live = true;
        stream
            .pending
            .push(crate::run_stream::PendingDecision::requested(
                2,
                "review",
                None,
                "Keep decision",
                None,
            ));
    });
    h.update(status(
        operation,
        planning,
        child,
        S::NeedsAttention {
            phase: P::Planning,
            run_id: Some(planning),
            reason: A::RecoveryRequired,
        },
        None,
        2,
    ));
    assert_eq!(h.queue().len(), 2);
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        assert!(state.run_streams[&planning].live);
        assert_eq!(state.run_streams[&planning].pending.len(), 1);
        assert!(state.unacknowledged_failures().is_empty());
        assert_eq!(state.needs_you_count(), 2);
    });
}
#[test]
fn sixth_stale_revision_identity_and_associated_ids_cannot_redirect_or_regress() {
    let mut h = Harness::new();
    let operation = RunId::new();
    let parent = RunId::new();
    let child = RunId::new();
    let original = status(
        operation,
        parent,
        child,
        S::Pending {
            phase: P::Implementing,
        },
        None,
        4,
    );
    h.update(original.clone());
    for changed in [
        status(operation, parent, child, S::Failed, None, 3),
        status(operation, RunId::new(), child, S::Failed, None, 5),
        status(operation, parent, RunId::new(), S::Failed, None, 5),
        status(operation, parent, child, S::Failed, None, 4),
    ] {
        h.update(changed);
    }
    h.app.update(&mut h.cx, |app, cx| {
        let mut changed = original.clone();
        changed.operation_id = RunId::new();
        app.update_bootstrap_view(operation, changed, std::path::Path::new(PROJECT), cx);
    });
    h.cx.read(|cx| {
        assert_eq!(h.state.read(cx).bootstrap_operations[&operation], original);
        assert_eq!(h.app.read(cx).bootstrap_index.len(), 1);
    });
    assert!(h.root.path().join("desktop/index.json").is_file());
}
#[test]
fn sixth_missing_gapped_origin_and_conflicting_terminal_are_inspection_never_failure() {
    for events in [
        vec![],
        vec![failed()],
        vec![start(std::path::Path::new(PROJECT)), completed(), failed()],
    ] {
        let mut h = Harness::new();
        let run = RunId::new();
        let events = rows(events);
        h.run_stream_only(run, events);
        let gapped = RunId::new();
        let mut gap = rows(vec![start(std::path::Path::new(PROJECT)), failed()]);
        gap[1].seq = EventSeq(3);
        h.run_stream_only(gapped, gap);
        assert!(
            h.queue()
                .iter()
                .all(|(_, badge)| *badge != "failed" && *badge != "aborted")
        );
        h.cx.read(|cx| assert!(h.state.read(cx).unacknowledged_failures().is_empty()));
    }
}
impl Harness {
    fn run_stream_only(&mut self, run: RunId, events: Vec<ReadEvent>) {
        self.state.update(&mut self.cx, |state, _| {
            state.run_streams.insert(
                run,
                crate::run_stream::RunStreamState::replay_candidate(run, &events),
            );
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: RunStatus::Failed,
                started_at: chrono::DateTime::from_timestamp_millis(1_700_000_000_000).unwrap(),
                ended_at: None,
                last_event_seq: None,
            });
        });
    }
}
#[test]
fn sixth_candidate_rejects_older_prefix_before_live_terminal_or_decision_and_conflict_retires_actions()
 {
    let mut h = Harness::new();
    let run = RunId::new();
    let history = rows(vec![start(std::path::Path::new(PROJECT))]);
    let token = h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, run, true).unwrap();
        let read = state.begin_history_read(&owner).unwrap();
        assert!(state.publish_history(&read, &history));
        state.observations.finish_read(&read, false);
        state.begin_history_read(&owner).unwrap()
    });
    h.state.update(&mut h.cx, |state, _| {
        let stream = state.run_streams.get_mut(&run).unwrap();
        stream.live = true;
        stream.apply_recorded(
            &EngineRunEvent::Persisted {
                seq: 2,
                payload: Box::new(failed()),
            },
            1_700_000_001_000,
        );
        assert!(
            !state
                .complete_history_read(&token, Ok(Some(history.clone())))
                .confirmed()
        );
        assert_eq!(
            state.failure_terminal(run),
            Some(surge_core::TerminalReason::Failed)
        );
        assert!(state.run_streams[&run].live);
    });
    let next = h.state.update(&mut h.cx, |state, _| {
        state.begin_history_read(&token).unwrap()
    });
    h.state.update(&mut h.cx, |state, _| {
        let conflicting = rows(vec![start(std::path::Path::new("/foreign")), failed()]);
        assert!(
            !state
                .complete_history_read(&next, Ok(Some(conflicting)))
                .confirmed()
        );
        assert_eq!(
            state.run_streams[&run].display(),
            surge_core::run_display::RunDisplayState::Unknown
        );
        assert!(state.run_streams[&run].pending.is_empty());
    });
}
#[test]
fn sixth_old_success_error_finalizer_cannot_mutate_replacement_or_a_b_a_scope() {
    let mut h = Harness::new();
    let run = RunId::new();
    let old = h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, run, false).unwrap();
        state.begin_history_read(&owner).unwrap()
    });
    let current = h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, run, true).unwrap();
        state.begin_history_read(&owner).unwrap()
    });
    h.state.update(&mut h.cx, |state, _| {
        assert!(
            state
                .complete_history_read(
                    &current,
                    Ok(Some(rows(vec![
                        start(std::path::Path::new(PROJECT)),
                        failed()
                    ])))
                )
                .confirmed()
        );
        assert!(
            !state
                .complete_history_read(&old, Err("obsolete error".into()))
                .confirmed()
        );
        assert!(
            !state
                .complete_history_read(
                    &old,
                    Ok(Some(rows(vec![start(std::path::Path::new(PROJECT))])))
                )
                .confirmed()
        );
        assert!(!state.observations.retire(&old));
        assert!(state.observations.accepts(&current));
        assert_eq!(
            state.failure_terminal(run),
            Some(surge_core::TerminalReason::Failed)
        );
        assert!(state.run_streams[&run].observation_error.is_none());
        state.observations.clear();
        state.project_path = Some("/other".into());
        let _ = state.observation_scope();
        state.project_path = Some(PROJECT.into());
        let scope = state.observation_scope().unwrap();
        assert_ne!(scope, old.scope);
        assert!(!state.accepts_observation(&current, false));
    });
}

fn drive(cx: &mut TestAppContext, ready: impl Fn(&TestAppContext) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready(cx) {
        assert!(
            std::time::Instant::now() < deadline,
            "controlled read did not settle"
        );
        cx.run_until_parked();
        std::thread::yield_now();
    }
    cx.run_until_parked();
}
#[test]
fn sixth_delayed_actual_hydration_success_error_and_finalizer_cannot_clear_replacement() {
    for (fail_read, replace_owner) in [(false, false), (false, true), (true, true)] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = runtime
            .block_on(surge_persistence::runs::Storage::open(home.path()))
            .unwrap();
        let mut h = Harness::new();
        let run = RunId::new();
        if !fail_read {
            runtime.block_on(async {
                let writer = storage
                    .create_run(run, std::path::Path::new(PROJECT), None)
                    .await
                    .unwrap();
                writer
                    .append_events(vec![surge_core::VersionedEventPayload::new(start(
                        std::path::Path::new(PROJECT),
                    ))])
                    .await
                    .unwrap();
                writer.close().await.unwrap();
            });
        }
        h.cx.background_executor.allow_parking();
        let owner = h.state.update(&mut h.cx, |state, _| {
            state.runtime_home = Some(home.path().into());
            let scope = state.observation_scope().unwrap();
            let owner = state
                .observations
                .claim(&scope, run, !replace_owner)
                .unwrap();
            if !replace_owner {
                let read = state.begin_history_read(&owner).unwrap();
                assert!(
                    state
                        .complete_history_read(
                            &read,
                            Ok(Some(rows(vec![start(std::path::Path::new(PROJECT))])))
                        )
                        .confirmed()
                );
            }
            owner
        });
        let (loaded_tx, mut loaded_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let done = std::rc::Rc::new(std::cell::Cell::new(false));
        let finished = done.clone();
        let weak = h.state.downgrade();
        let delayed = owner.clone();
        h.app.update(&mut h.cx, |_, cx| {
            cx.spawn(async move |_: gpui_kit::WeakEntity<SurgeApp>, cx| {
                let result =
                    super::super::hydrate_run_history_observed(&weak, &delayed, cx, async move {
                        let _ = loaded_tx.send(());
                        let _ = release_rx.await;
                    })
                    .await;
                assert!(!result.confirmed(), "obsolete hydration must not publish");
                finished.set(true);
            })
            .detach();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            h.cx.run_until_parked();
            if loaded_rx.try_recv().is_ok() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let replacement = h.state.update(&mut h.cx, |state, _| {
            if !replace_owner {
                let stream = state.run_streams.get_mut(&run).unwrap();
                stream.live = true;
                stream.apply_recorded(
                    &EngineRunEvent::Persisted {
                        seq: 2,
                        payload: Box::new(failed()),
                    },
                    1_700_000_001_000,
                );
                return owner.clone();
            }
            let scope = state.observation_scope().unwrap();
            let owner = state.observations.claim(&scope, run, true).unwrap();
            let read = state.begin_history_read(&owner).unwrap();
            assert!(
                state
                    .complete_history_read(
                        &read,
                        Ok(Some(rows(vec![
                            start(std::path::Path::new(PROJECT)),
                            failed()
                        ])))
                    )
                    .confirmed()
            );
            state.run_streams.get_mut(&run).unwrap().live = true;
            owner
        });
        h.app.update(&mut h.cx, |app, _| {
            app.stream_subscribed.insert(run);
        });
        release_tx.send(()).unwrap();
        drive(&mut h.cx, |_| done.get());
        h.app.update(&mut h.cx, |app, cx| {
            if replace_owner {
                app.finish_run_pump(&owner, cx);
            }
            assert!(app.stream_subscribed.contains(&run));
        });
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert!(state.observations.accepts(&replacement));
            assert!(state.run_streams[&run].live);
            assert!(state.run_streams[&run].observation_error.is_none());
            assert_eq!(
                state.failure_terminal(run),
                Some(surge_core::TerminalReason::Failed)
            );
        });
        drop(h);
        drop(storage);
        home.close().unwrap();
    }
}

#[test]
fn sixth_cold_index_recovers_both_real_journals_and_operation_reasons_without_daemon() {
    use surge_core::ContentHash;
    use surge_core::bootstrap_operation::{
        BootstrapCapture, BootstrapCaptureFields, BootstrapContentRef, BootstrapIntent,
    };
    for case in [
        "child-failed",
        "budget",
        "attention-no-run",
        "unsupported",
        "corrupt-record",
        "foreign",
    ] {
        let uninspectable = matches!(case, "unsupported" | "corrupt-record" | "foreign");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let project = home.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let graph: surge_core::Graph =
            toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml")).unwrap();
        let digest = graph.identity().unwrap();
        let mut generated = graph.clone();
        generated.metadata.name = "generated child".into();
        let mut regenerated = graph.clone();
        regenerated.metadata.name = "regenerated child".into();
        let pins: BootstrapCapture = BootstrapCaptureFields {
            version: 1,
            planning_run: RunId::new(),
            implementation_run: RunId::new(),
            repository: project.clone(),
            git_common_dir: project.join(".git"),
            base_commit: "a".repeat(40),
            planning_worktree: project.join("planning"),
            implementation_worktree: project.join("implementation"),
            planning_branch: "surge/planning".into(),
            implementation_branch: "surge/implementation".into(),
            graph: BootstrapContentRef {
                name: "bootstrap".into(),
                digest,
            },
            project_context: None,
            bootstrap_edit_loop_cap: None,
            runtime_identity: ContentHash::compute(b"runtime"),
            configuration: vec![],
            credential_refs: vec![],
        }
        .try_into()
        .unwrap();
        let storage = runtime
            .block_on(surge_persistence::runs::Storage::open(home.path()))
            .unwrap();
        let store = storage.bootstrap_operation_store();
        let mut record = store
            .insert_if_absent(
                RunId::new(),
                &BootstrapIntent::new(project.clone(), "Build timer".into(), Default::default())
                    .unwrap(),
                &pins,
            )
            .unwrap();
        if case == "attention-no-run" {
            record = store
                .block(
                    record.status.operation_id,
                    record.status.revision,
                    A::StorageUnconfirmed,
                )
                .unwrap();
        } else if matches!(case, "child-failed" | "budget") {
            record = store
                .claim(record.status.operation_id, record.status.revision)
                .unwrap();
            record = store
                .mark_started(record.status.operation_id, record.status.revision)
                .unwrap();
            runtime.block_on(async {
                let writer = storage
                    .create_run(
                        record.status.planning_run,
                        &pins.fields().planning_worktree,
                        None,
                    )
                    .await
                    .unwrap();
                writer
                    .append_events(
                        vec![
                            start(&pins.fields().planning_worktree),
                            E::PipelineMaterialized {
                                graph: Box::new(graph.clone()),
                                graph_hash: digest,
                            },
                            E::PipelineMaterialized {
                                graph: Box::new(generated.clone()),
                                graph_hash: generated.identity().unwrap(),
                            },
                            E::PipelineMaterialized {
                                graph: Box::new(regenerated.clone()),
                                graph_hash: regenerated.identity().unwrap(),
                            },
                            E::RunCompleted {
                                terminal_node: "end".parse().unwrap(),
                            },
                        ]
                        .into_iter()
                        .map(surge_core::VersionedEventPayload::new)
                        .collect(),
                    )
                    .await
                    .unwrap();
                writer.close().await.unwrap();
            });
            if case == "child-failed" {
                let child = surge_core::bootstrap_continuation::BootstrapContinuation::new(
                    record.status.planning_run,
                    BootstrapContentRef {
                        name: "implementation".into(),
                        digest,
                    },
                    ["description", "roadmap", "flow"]
                        .into_iter()
                        .map(|name| surge_core::run_state::ArtifactRef {
                            hash: ContentHash::compute(name.as_bytes()),
                            path: name.into(),
                            name: name.into(),
                            produced_by: "author".parse().unwrap(),
                            produced_at_seq: 2,
                        })
                        .collect(),
                    Default::default(),
                )
                .unwrap();
                record = store
                    .continue_planning(record.status.operation_id, record.status.revision, &child)
                    .unwrap();
                runtime.block_on(async {
                    let writer = storage
                        .create_run(
                            record.status.implementation_run,
                            &pins.fields().implementation_worktree,
                            None,
                        )
                        .await
                        .unwrap();
                    writer
                        .append_events(
                            vec![
                                start(&pins.fields().implementation_worktree),
                                E::PipelineMaterialized {
                                    graph: Box::new(graph.clone()),
                                    graph_hash: digest,
                                },
                                failed(),
                            ]
                            .into_iter()
                            .map(surge_core::VersionedEventPayload::new)
                            .collect(),
                        )
                        .await
                        .unwrap();
                    writer.close().await.unwrap();
                });
                record = store
                    .settle(
                        record.status.operation_id,
                        record.status.revision,
                        &T::Failed {
                            run_id: record.status.implementation_run,
                            reason: F::RunFailed,
                        },
                    )
                    .unwrap();
            } else {
                record = store
                    .settle(
                        record.status.operation_id,
                        record.status.revision,
                        &T::Failed {
                            run_id: record.status.planning_run,
                            reason: F::BudgetExhausted,
                        },
                    )
                    .unwrap();
            }
        }
        if matches!(case, "child-failed" | "budget") {
            let valid = runtime
                .block_on(
                    surge_persistence::runs::Storage::inspect_existing_run_events(
                        home.path().join("runs"),
                        record.status.planning_run,
                    ),
                )
                .unwrap();
            assert!(
                super::super::observation::bound_related(
                    &record,
                    record.status.planning_run,
                    &valid
                )
                .is_ok()
            );
            let mut corrupt = valid.clone();
            if let E::PipelineMaterialized { graph_hash, .. } = &mut corrupt[3].payload.payload {
                *graph_hash = ContentHash::compute(b"wrong regenerated content");
            }
            assert!(
                super::super::observation::bound_related(
                    &record,
                    record.status.planning_run,
                    &corrupt
                )
                .is_err()
            );
            let mut foreign = valid.clone();
            foreign[1].payload.payload = E::PipelineMaterialized {
                graph: Box::new(generated.clone()),
                graph_hash: generated.identity().unwrap(),
            };
            assert!(
                super::super::observation::bound_related(
                    &record,
                    record.status.planning_run,
                    &foreign
                )
                .is_err()
            );
        }
        if matches!(case, "unsupported" | "corrupt-record") {
            let pool = surge_persistence::runs::registry::open_registry_pool(
                home.path(),
                &surge_persistence::runs::MockClock::new(1),
            )
            .unwrap();
            pool.get().unwrap().execute(if case=="unsupported" {"UPDATE bootstrap_operations SET payload_version=999"} else {"UPDATE bootstrap_operations SET intent_fingerprint='0000000000000000000000000000000000000000000000000000000000000000'"},[]).unwrap();
        }
        let opened_project = if case == "foreign" {
            let path = project.join("foreign");
            std::fs::create_dir(&path).unwrap();
            path
        } else {
            project.clone()
        };
        let index = home.path().join("desktop/index.json");
        SurgeApp::save_bootstrap_index_at(
            index.clone(),
            &[super::super::BootstrapIndexEntry {
                operation_id: record.status.operation_id,
                project_path: opened_project.clone(),
            }],
        )
        .unwrap();
        let mut cx = TestAppContext::single();
        cx.background_executor.allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init();
            crate::theme::sync_component_theme(cx);
        });
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.runtime_home = Some(home.path().into());
            state.project_path = Some(opened_project.clone());
            state
        });
        let app = cx.new(|cx| {
            let mut app = SurgeApp::new_shell_at(state.clone(), Some(index), cx);
            app.restore_indexed_operations(cx);
            app
        });
        drive(&mut cx, |cx| {
            cx.read(|cx| app.read(cx).offline_operations.is_empty())
        });
        cx.read(|cx| {
            let state = state.read(cx);
            assert!(state.daemon_state.facade().is_none());
            if uninspectable {
                assert!(state.bootstrap_operations.is_empty());
                assert!(
                    state
                        .operation_inspections
                        .contains_key(&record.status.operation_id)
                );
            } else {
                assert_eq!(
                    state.bootstrap_operations[&record.status.operation_id],
                    record.status
                );
            }
            assert_eq!(state.needs_you_count(), 1);
            if case == "attention-no-run" || uninspectable {
                assert!(state.runs.is_empty());
                assert!(state.run_streams.is_empty());
            } else {
                assert_eq!(
                    state.run_streams[&record.status.planning_run].display(),
                    surge_core::run_display::RunDisplayState::Done(
                        surge_core::TerminalReason::Completed
                    )
                );
                assert_eq!(
                    state.unacknowledged_failures().len(),
                    usize::from(case == "child-failed")
                );
            }
        });
        app.update(&mut cx, |app, cx| {
            app.navigate(Screen::Inbox, cx);
            let _ = app.render_screen_content(cx);
        });
        let inbox = cx.read(|cx| app.read(cx).inbox.clone().unwrap());
        let (_, _window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(inbox.clone(), window, cx)
        });
        let queue = inbox.update(&mut cx, |screen, cx| screen.queue_for_test(cx));
        assert_eq!(queue.len(), 1);
        if uninspectable {
            assert_eq!(queue[0].1, "inspect operation");
        }
        assert_eq!(
            queue[0].0,
            if case == "child-failed" {
                format!("failed-{}", record.status.implementation_run)
            } else {
                format!("operation-{}", record.status.operation_id)
            }
        );
        if case == "attention-no-run" {
            cx.background_executor
                .advance_clock(std::time::Duration::from_secs(100));
            cx.run_until_parked();
            cx.read(|cx| {
                let state = state.read(cx);
                assert_eq!(state.related_history_reads[&record.status.planning_run], 1);
                assert_eq!(
                    state.related_history_reads[&record.status.implementation_run],
                    1
                );
                assert!(state.runs.is_empty());
                assert!(state.run_streams.is_empty());
            });
        }
        drop(inbox);
        drop(app);
        drop(state);
        cx.update(|_| {});
        cx.run_until_parked();
        drop(cx);
        drop(store);
        drop(storage);
        home.close().unwrap();
    }
}

#[test]
fn sixth_terminal_stamp_uses_terminal_claim_before_lineage_and_list_refresh() {
    let mut h = Harness::new();
    let run = RunId::new();
    let events = rows(vec![
        start(std::path::Path::new(PROJECT)),
        completed(),
        E::ForkCreated {
            new_run: RunId::new(),
            fork_at_seq: 2,
        },
    ]);
    h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, run, false).unwrap();
        let read = state.begin_history_read(&owner).unwrap();
        assert!(
            state
                .complete_history_read(&read, Ok(Some(events)))
                .confirmed()
        );
        assert_eq!(
            state.runs[0].ended_at.unwrap().timestamp_millis(),
            1_700_000_001_000
        );
        assert!(
            state.observations.claim(&scope, run, false).is_none(),
            "unchanged terminal owner must not reread"
        );
        state.set_runs_from_summaries(&[]);
        assert_eq!(
            state.runs[0].ended_at.unwrap().timestamp_millis(),
            1_700_000_001_000
        );
    });
}

#[test]
fn sixth_late_actual_operation_absence_and_error_cannot_obscure_newer_status() {
    for corrupt in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let mut h = Harness::new();
        h.cx.background_executor.allow_parking();
        if corrupt {
            std::fs::create_dir(h.root.path().join("db")).unwrap();
            std::fs::create_dir(h.root.path().join("db/registry.sqlite")).unwrap();
        }
        let operation = RunId::new();
        let planning = RunId::new();
        let child = RunId::new();
        let scope = h
            .state
            .update(&mut h.cx, |state, _| state.observation_scope().unwrap());
        let (seen_tx, mut seen_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        h.app.update(&mut h.cx, |app, cx| {
            app.read_operation_observed(scope, operation, cx, async move {
                let _ = seen_tx.send(());
                let _ = release_rx.await;
            })
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            h.cx.run_until_parked();
            if seen_rx.try_recv().is_ok() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let current = status(
            operation,
            planning,
            child,
            S::Pending { phase: P::Planning },
            None,
            9,
        );
        let (current_tx, current_rx) = tokio::sync::oneshot::channel();
        let (current_release_tx, current_release_rx) = tokio::sync::oneshot::channel();
        h.app.update(&mut h.cx, |app, _| {
            app.controlled_operation_reads = Some(std::rc::Rc::new(std::cell::RefCell::new(
                std::collections::VecDeque::from([(current_tx, current_release_rx)]),
            )))
        });
        h.update(current.clone());
        release_tx.send(()).unwrap();
        let current_rx = std::cell::RefCell::new(current_rx);
        drive(&mut h.cx, |_| current_rx.borrow_mut().try_recv().is_ok());
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert_eq!(state.bootstrap_operations[&operation], current);
            assert!(
                state.operation_inspections.is_empty(),
                "late error/absence cannot obscure newer operation"
            );
            assert!(state.runs.is_empty());
        });
        current_release_tx.send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
        });
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert!(
                state.operation_inspection_reason(operation).is_some(),
                "fresh current unavailable registry must remain inspectable"
            );
            assert_eq!(state.bootstrap_operations[&operation], current);
            assert!(state.runs.is_empty());
            assert!(state.run_streams.is_empty());
        });
    }
}

#[test]
fn sixth_current_offline_absence_and_corruption_are_operation_inspection_without_run_links() {
    for corrupt in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let mut h = Harness::new();
        h.cx.background_executor.allow_parking();
        if corrupt {
            std::fs::create_dir(h.root.path().join("db")).unwrap();
            std::fs::write(
                h.root.path().join("db/registry.sqlite"),
                b"invalid database",
            )
            .unwrap();
        }
        let operation = RunId::new();
        h.app.update(&mut h.cx, |app, cx| {
            app.bootstrap_index.push(super::super::BootstrapIndexEntry {
                operation_id: operation,
                project_path: PROJECT.into(),
            });
            app.restore_indexed_operations(cx);
        });
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
        });
        assert_eq!(
            h.queue(),
            vec![(format!("operation-{operation}"), "inspect operation")]
        );
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert_eq!(state.needs_you_count(), 1);
            assert!(state.bootstrap_operations.is_empty());
            assert!(state.runs.is_empty());
            assert!(state.run_streams.is_empty());
        });
    }
}
#[test]
fn sixth_operation_recorded_links_emit_exact_navigation_and_no_reserved_buttons() {
    let mut h = Harness::new();
    let operation = RunId::new();
    let planning = RunId::new();
    let child = RunId::new();
    h.run(
        planning,
        vec![start(std::path::Path::new(PROJECT)), completed()],
        RunStatus::Completed,
    );
    h.update(status(
        operation,
        planning,
        child,
        S::Failed,
        Some(T::Failed {
            run_id: planning,
            reason: F::BudgetExhausted,
        }),
        3,
    ));
    let _ = h.queue();
    let inbox = h.cx.read(|cx| h.app.read(cx).inbox.clone().unwrap());
    let (_, window) = h
        .cx
        .add_window_view(|window, cx| gpui_kit::component::Root::new(inbox.clone(), window, cx));
    let observed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let copy = observed.clone();
    window.update(|_, cx| {
        cx.subscribe(
            &inbox,
            move |_, event: &crate::screens::inbox::InboxAction, _| {
                if let crate::screens::inbox::InboxAction::OpenRun(run) = event {
                    copy.borrow_mut().push(*run);
                }
            },
        )
        .detach();
    });
    let id = Box::leak(format!("inbox-open-{planning}").into_boxed_str());
    let button = window.debug_bounds(id).unwrap();
    window.simulate_click(button.center(), gpui_kit::Modifiers::default());
    assert_eq!(*observed.borrow(), vec![planning]);
    assert!(
        window
            .debug_bounds(Box::leak(format!("inbox-open-{child}").into_boxed_str()))
            .is_none()
    );
}

#[test]
fn sixth_older_prefix_preserves_actual_pending_session_and_watermark_atomically() {
    let mut h = Harness::new();
    let run = RunId::new();
    let initial = rows(vec![start(std::path::Path::new(PROJECT))]);
    h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, run, true).unwrap();
        let read = state.begin_history_read(&owner).unwrap();
        assert!(
            state
                .complete_history_read(&read, Ok(Some(initial.clone())))
                .confirmed()
        );
        let late = state.begin_history_read(&owner).unwrap();
        let stream = state.run_streams.get_mut(&run).unwrap();
        stream.live = true;
        stream.apply_recorded(
            &EngineRunEvent::Persisted {
                seq: 2,
                payload: Box::new(E::SessionOpened {
                    node: "work".parse().unwrap(),
                    session: surge_core::SessionId::new(),
                    agent: "test".into(),
                    agent_id: None,
                    opened: None,
                    handoff: None,
                }),
            },
            1_700_000_001_000,
        );
        stream.apply_recorded(
            &EngineRunEvent::Persisted {
                seq: 3,
                payload: Box::new(E::HumanInputRequested {
                    node: "work".parse().unwrap(),
                    session: None,
                    call_id: Some("exact-call".into()),
                    prompt: "Keep actual decision".into(),
                    schema: None,
                }),
            },
            1_700_000_002_000,
        );
        assert_eq!(stream.pending.len(), 1);
        assert_eq!(stream.sessions.len(), 1);
        assert!(
            !state
                .complete_history_read(&late, Ok(Some(initial)))
                .confirmed()
        );
        let stream = &state.run_streams[&run];
        assert_eq!(stream.pending[0].call_id().as_deref(), Some("exact-call"));
        assert_eq!(stream.pending[0].seq, 3);
        assert_eq!(stream.sessions.len(), 1);
        assert_eq!(stream.last_seq, 3);
        assert!(stream.live);
    });
}

#[test]
fn sixth_implementing_child_and_global_list_poll_orders_keep_distinct_run_truth() {
    use surge_orchestrator::engine::handle::RunOutcome;
    use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
    for operation_first in [false, true] {
        let mut h = Harness::new();
        let operation = RunId::new();
        let planning = RunId::new();
        let child = RunId::new();
        h.run(
            planning,
            vec![start(std::path::Path::new(PROJECT)), completed()],
            RunStatus::Completed,
        );
        h.run(
            child,
            vec![start(std::path::Path::new(PROJECT))],
            RunStatus::Active,
        );
        h.state.update(&mut h.cx, |state, _| {
            state.run_streams.get_mut(&child).unwrap().live = true
        });
        let implementing = status(
            operation,
            planning,
            child,
            S::Pending {
                phase: P::Implementing,
            },
            None,
            1,
        );
        h.update(implementing.clone());
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert_eq!(
                state.run_streams[&planning].display(),
                surge_core::run_display::RunDisplayState::Done(
                    surge_core::TerminalReason::Completed
                )
            );
            assert_eq!(
                state.run_streams[&child].display(),
                surge_core::run_display::RunDisplayState::Working
            );
        });
        assert!(h.queue().is_empty());
        let stopped = status(
            operation,
            planning,
            child,
            S::Failed,
            Some(T::Failed {
                run_id: child,
                reason: F::RunFailed,
            }),
            2,
        );
        if operation_first {
            h.update(stopped.clone());
        }
        h.state.update(&mut h.cx, |state, _| {
            state.apply_global_event(&GlobalDaemonEvent::RunAccepted { run_id: child });
            state.run_streams.get_mut(&child).unwrap().apply_recorded(
                &EngineRunEvent::Persisted {
                    seq: 2,
                    payload: Box::new(failed()),
                },
                1_700_000_001_000,
            );
            state.apply_global_event(&GlobalDaemonEvent::RunFinished {
                run_id: child,
                outcome: RunOutcome::Failed {
                    error: "actual implementation failure".into(),
                },
            });
        });
        h.update(stopped.clone());
        h.update(stopped);
        h.state
            .update(&mut h.cx, |state, _| state.set_runs_from_summaries(&[]));
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
        assert_eq!(h.queue(), vec![(format!("failed-{child}"), "failed")]);
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert!(state.run_streams.contains_key(&planning));
            assert!(state.run_streams.contains_key(&child));
            assert_eq!(state.runs.len(), 2);
            assert_eq!(state.needs_you_count(), 1);
        });
    }
}
#[test]
fn sixth_cancelled_operation_keeps_only_actual_aborted_run_in_triage() {
    let mut h = Harness::new();
    let operation = RunId::new();
    let planning = RunId::new();
    let child = RunId::new();
    h.run(
        planning,
        vec![
            start(std::path::Path::new(PROJECT)),
            E::RunAborted {
                reason: "actual stop".into(),
            },
        ],
        RunStatus::Active,
    );
    h.update(status(
        operation,
        planning,
        child,
        S::Cancelled,
        Some(T::Cancelled),
        2,
    ));
    assert_eq!(h.queue(), vec![(format!("failed-{planning}"), "aborted")]);
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        assert_eq!(state.runs.len(), 1);
        assert!(!state.run_streams.contains_key(&child));
    });
}
#[test]
fn sixth_home_change_and_return_revokes_delayed_run_callbacks() {
    let mut h = Harness::new();
    let run = RunId::new();
    h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, run, false).unwrap();
        let read = state.begin_history_read(&owner).unwrap();
        let home = state.runtime_home.clone();
        state.runtime_home = Some(h.root.path().join("replacement"));
        let _ = state.observation_scope();
        state.runtime_home = home;
        let current = state.observation_scope().unwrap();
        assert_ne!(scope, current);
        assert!(
            !state
                .complete_history_read(
                    &read,
                    Ok(Some(rows(vec![
                        start(std::path::Path::new(PROJECT)),
                        failed()
                    ])))
                )
                .confirmed()
        );
        assert!(
            !state
                .complete_history_read(&read, Err("obsolete home error".into()))
                .confirmed()
        );
        assert!(state.runs.is_empty());
        assert!(state.run_streams.is_empty());
    });
}

#[cfg(unix)]
#[test]
fn sixth_actual_subscription_publishes_live_terminal_while_its_read_is_paused() {
    use surge_orchestrator::engine::ipc::{
        DaemonEvent, DaemonRequest, DaemonResponse, read_request_frame, write_frame,
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let storage = runtime
        .block_on(surge_persistence::runs::Storage::open_with(
            home.path(),
            std::sync::Arc::new(surge_persistence::runs::MockClock::new(1_700_000_000_000)),
        ))
        .unwrap();
    let run = RunId::new();
    runtime.block_on(async {
        let writer = storage
            .create_run(run, std::path::Path::new(PROJECT), None)
            .await
            .unwrap();
        writer
            .append_events(vec![surge_core::VersionedEventPayload::new(start(
                std::path::Path::new(PROJECT),
            ))])
            .await
            .unwrap();
        writer.close().await.unwrap();
    });
    let socket = home.path().join("paused-reader.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let (event_tx, event_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = runtime.spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut read = tokio::io::BufReader::new(read);
        let Some(DaemonRequest::Subscribe { request_id, run_id }) =
            read_request_frame(&mut read).await.unwrap()
        else {
            panic!("exact subscription expected")
        };
        assert_eq!(run_id, run);
        write_frame(&mut write, &DaemonResponse::SubscribeOk { request_id })
            .await
            .unwrap();
        let event = event_rx.await.unwrap();
        write_frame(
            &mut write,
            &DaemonEvent::PerRun {
                run_id: run,
                event: Box::new(event),
            },
        )
        .await
        .unwrap();
        let _ = stop_rx.await;
    });
    let facade = std::sync::Arc::new(
        runtime
            .block_on(
                surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket),
            )
            .unwrap(),
    );
    let mut h = Harness::new();
    h.cx.background_executor.allow_parking();
    h.run(
        run,
        vec![start(std::path::Path::new(PROJECT))],
        RunStatus::Active,
    );
    h.state.update(&mut h.cx, |state, _| {
        state.runtime_home = Some(home.path().into());
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade.clone());
    });
    let (loaded_tx, mut loaded_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, cx| {
        app.controlled_history_reads = Some(std::rc::Rc::new(std::cell::RefCell::new(
            std::collections::VecDeque::from([(loaded_tx, release_rx)]),
        )));
        app.sync_run_subscriptions(cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        h.cx.run_until_parked();
        if loaded_rx.try_recv().is_ok() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    runtime.block_on(async {
        let writer = storage.open_run_writer(run).await.unwrap();
        writer
            .append_events(vec![surge_core::VersionedEventPayload::new(failed())])
            .await
            .unwrap();
        writer.close().await.unwrap();
    });
    event_tx
        .send(EngineRunEvent::Persisted {
            seq: 2,
            payload: Box::new(failed()),
        })
        .unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).failure_terminal(run) == Some(surge_core::TerminalReason::Failed)
        })
    });
    assert!(release_tx.send(()).is_ok());
    h.cx.run_until_parked();
    stop_tx.send(()).unwrap();
    runtime.block_on(server).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| !h.state.read(cx).run_streams[&run].live)
    });
    h.cx.read(|cx| {
        assert_eq!(
            h.state.read(cx).failure_terminal(run),
            Some(surge_core::TerminalReason::Failed)
        )
    });
    drop(h);
    drop(facade);
    drop(storage);
    home.close().unwrap();
}

#[test]
fn sixth_changed_recorded_timestamp_conflicts_with_retained_prefix() {
    let mut h = Harness::new();
    let run = RunId::new();
    let history = rows(vec![start(std::path::Path::new(PROJECT)), completed()]);
    h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, run, false).unwrap();
        let first = state.begin_history_read(&owner).unwrap();
        assert!(
            state
                .complete_history_read(&first, Ok(Some(history.clone())))
                .confirmed()
        );
        let later = state.begin_history_read(&owner).unwrap();
        let mut changed = history;
        changed[1].timestamp_ms += 1000;
        assert!(
            !state
                .complete_history_read(&later, Ok(Some(changed)))
                .confirmed(),
            "equal payload and sequence cannot authenticate changed durable time"
        );
        assert_eq!(
            state.runs[0].ended_at.unwrap().timestamp_millis(),
            1_700_000_001_000
        );
        assert_eq!(
            state.run_streams[&run].display(),
            surge_core::run_display::RunDisplayState::Unknown
        );
    });
}

#[test]
fn sixth_equal_operation_snapshot_restores_current_generation_relation() {
    let mut h = Harness::new();
    let operation = RunId::new();
    let original = status(
        operation,
        RunId::new(),
        RunId::new(),
        S::Pending { phase: P::Planning },
        None,
        7,
    );
    h.update(original.clone());
    h.state.update(&mut h.cx, |state, _| {
        state.observations.clear();
        state.bootstrap_operations.clear();
        state.project_path = Some("/project-b".into());
        let _ = state.observation_scope();
        state.project_path = Some(PROJECT.into());
        let _ = state.observation_scope();
    });
    h.update(original.clone());
    h.cx.read(|cx| {
        assert_eq!(
            h.state.read(cx).bootstrap_operations.get(&operation),
            Some(&original),
            "equal snapshot must restore current-generation relation after project return"
        )
    });
}

#[test]
fn sixth_successful_same_owner_read_preserves_newer_live_error_and_flags() {
    for (success, error_before_read) in [(true, false), (false, false), (true, true), (false, true)]
    {
        let mut h = Harness::new();
        let run = RunId::new();
        let history = rows(vec![start(std::path::Path::new(PROJECT))]);
        h.state.update(&mut h.cx, |state, _| {
            let scope = state.observation_scope().unwrap();
            let owner = state.observations.claim(&scope, run, true).unwrap();
            let initial = state.begin_history_read(&owner).unwrap();
            assert!(state.complete_history_read(&initial, Ok(Some(history.clone()))).confirmed());
            state.run_streams.get_mut(&run).unwrap().history_unconfirmed("prior read failure");
            let repaired = state.begin_history_read(&owner).unwrap();
            assert!(state.complete_history_read(&repaired, Ok(Some(history.clone()))).confirmed());
            assert!(state.run_streams[&run].observation_error.is_none());
            let feed_error = |state: &mut AppState| {
                let stream = state.run_streams.get_mut(&run).unwrap();
                stream.live = true;
                stream.apply(&EngineRunEvent::StreamError {
                    message: "actual feed failure".into(),
                });
                assert!(!stream.live);
                assert!(stream.observation_error.is_some());
            };
            if error_before_read { feed_error(state); }
            let delayed = state.begin_history_read(&owner).unwrap();
            if !error_before_read { feed_error(state); }
            let result = if success {
                Ok(Some(history))
            } else {
                Err("older read failure".into())
            };
            assert_eq!(state.complete_history_read(&delayed, result).confirmed(), success);
            let stream = &state.run_streams[&run];
            assert!(!stream.live);
            assert!(
                stream.observation_error.as_deref()
                    .is_some_and(|error| error.contains("actual feed failure")),
                "history read must preserve stream-owned error (before={error_before_read}, success={success})"
            );
            assert_eq!(stream.display(), surge_core::run_display::RunDisplayState::Unknown);
        });
    }
}

struct SeventhFixture {
    home: crate::runtime_home_fixture::FixtureHome,
    storage: std::sync::Arc<surge_persistence::runs::Storage>,
    record: surge_persistence::runs::bootstrap_operations::BootstrapOperationRecord,
    project: std::path::PathBuf,
}
impl SeventhFixture {
    fn new(runtime: &tokio::runtime::Runtime) -> Self {
        Self::with_child_terminal(runtime, true)
    }
    fn with_child_terminal(runtime: &tokio::runtime::Runtime, child_terminal: bool) -> Self {
        Self::with_state(runtime, child_terminal, false)
    }
    fn with_state(
        runtime: &tokio::runtime::Runtime,
        child_terminal: bool,
        completed_child: bool,
    ) -> Self {
        Self::with_operation(runtime, child_terminal, completed_child, true)
    }
    fn with_operation(
        runtime: &tokio::runtime::Runtime,
        child_terminal: bool,
        completed_child: bool,
        settled: bool,
    ) -> Self {
        use surge_core::bootstrap_operation::{
            BootstrapCapture, BootstrapCaptureFields, BootstrapContentRef, BootstrapIntent,
        };
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let project = home.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let graph: surge_core::Graph =
            toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml")).unwrap();
        let digest = graph.identity().unwrap();
        let pins: BootstrapCapture = BootstrapCaptureFields {
            version: 1,
            planning_run: RunId::new(),
            implementation_run: RunId::new(),
            repository: project.clone(),
            git_common_dir: project.join(".git"),
            base_commit: "a".repeat(40),
            planning_worktree: project.join("planning"),
            implementation_worktree: project.join("implementation"),
            planning_branch: "surge/planning".into(),
            implementation_branch: "surge/implementation".into(),
            graph: BootstrapContentRef {
                name: "bootstrap".into(),
                digest,
            },
            project_context: None,
            bootstrap_edit_loop_cap: None,
            runtime_identity: surge_core::ContentHash::compute(b"runtime"),
            configuration: vec![],
            credential_refs: vec![],
        }
        .try_into()
        .unwrap();
        let storage = runtime
            .block_on(surge_persistence::runs::Storage::open_with(
                home.path(),
                std::sync::Arc::new(surge_persistence::runs::MockClock::new(1_700_000_000_000)),
            ))
            .unwrap();
        let store = storage.bootstrap_operation_store();
        let mut record = store
            .insert_if_absent(
                RunId::new(),
                &BootstrapIntent::new(project.clone(), "Build timer".into(), Default::default())
                    .unwrap(),
                &pins,
            )
            .unwrap();
        record = store
            .claim(record.status.operation_id, record.status.revision)
            .unwrap();
        record = store
            .mark_started(record.status.operation_id, record.status.revision)
            .unwrap();
        let child = surge_core::bootstrap_continuation::BootstrapContinuation::new(
            record.status.planning_run,
            BootstrapContentRef {
                name: "implementation".into(),
                digest,
            },
            ["description", "roadmap", "flow"]
                .into_iter()
                .map(|name| surge_core::run_state::ArtifactRef {
                    hash: surge_core::ContentHash::compute(name.as_bytes()),
                    path: name.into(),
                    name: name.into(),
                    produced_by: "author".parse().unwrap(),
                    produced_at_seq: 2,
                })
                .collect(),
            Default::default(),
        )
        .unwrap();
        record = store
            .continue_planning(record.status.operation_id, record.status.revision, &child)
            .unwrap();
        for (run, path, terminal) in [
            (
                record.status.planning_run,
                &pins.fields().planning_worktree,
                E::RunCompleted {
                    terminal_node: "end".parse().unwrap(),
                },
            ),
            (
                record.status.implementation_run,
                &pins.fields().implementation_worktree,
                if completed_child {
                    E::RunCompleted {
                        terminal_node: "end".parse().unwrap(),
                    }
                } else {
                    failed()
                },
            ),
        ] {
            runtime.block_on(async {
                let writer = storage.create_run(run, path, None).await.unwrap();
                let mut payloads = vec![
                    start(path),
                    E::PipelineMaterialized {
                        graph: Box::new(graph.clone()),
                        graph_hash: digest,
                    },
                ];
                if run == record.status.planning_run || child_terminal {
                    payloads.push(terminal);
                }
                writer
                    .append_events(
                        payloads
                            .into_iter()
                            .map(surge_core::VersionedEventPayload::new)
                            .collect(),
                    )
                    .await
                    .unwrap();
                writer.close().await.unwrap();
            });
        }
        if settled {
            record = store
                .settle(
                    record.status.operation_id,
                    record.status.revision,
                    &if completed_child {
                        T::Completed {
                            run_id: record.status.implementation_run,
                            terminal: "end".parse().unwrap(),
                        }
                    } else {
                        T::Failed {
                            run_id: record.status.implementation_run,
                            reason: F::RunFailed,
                        }
                    },
                )
                .unwrap();
        }
        drop(store);
        Self {
            home,
            storage,
            record,
            project,
        }
    }
    fn harness(&self) -> Harness {
        let mut h = Harness::new();
        h.cx.background_executor.allow_parking();
        h.state.update(&mut h.cx, |state, _| {
            state.runtime_home = Some(self.home.path().into());
            state.project_path = Some(self.project.clone());
        });
        h
    }
    fn obstruct(&self, run: RunId) -> std::path::PathBuf {
        let path = self
            .home
            .path()
            .join("runs")
            .join(run.to_string())
            .join("events.sqlite");
        std::fs::rename(&path, path.with_extension("held")).unwrap();
        std::fs::create_dir(&path).unwrap();
        path
    }
    fn restore(path: &std::path::Path) {
        std::fs::remove_dir(path).unwrap();
        std::fs::rename(path.with_extension("held"), path).unwrap();
    }
    fn close(self) {
        drop(self.storage);
        self.home.close().unwrap();
    }
}

#[test]
fn seventh_current_nested_history_error_recovers_without_daemon_or_status_change() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::new(&runtime);
    let child = fixture.record.status.implementation_run;
    let operation = fixture.record.status.operation_id;
    let path = fixture.obstruct(child);
    assert!(
        runtime
            .block_on(super::super::observation::read_history(
                fixture.home.path(),
                child
            ))
            .is_err()
    );
    let mut h = fixture.harness();
    h.app.update(&mut h.cx, |app, cx| {
        app.bootstrap_index.push(super::super::BootstrapIndexEntry {
            operation_id: operation,
            project_path: fixture.project.clone(),
        });
        app.restore_indexed_operations(cx);
    });
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        assert_eq!(
            state.bootstrap_operations[&operation],
            fixture.record.status
        );
        assert!(state.daemon_state.facade().is_none());
        assert!(state.operation_inspections.contains_key(&operation));
        assert_eq!(state.failure_terminal(child), None);
    });
    SeventhFixture::restore(&path);
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(1));
    // A bounded parked drain observes the scheduled production retry; no manual hydrate/status update.
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).failure_terminal(child) == Some(surge_core::TerminalReason::Failed)
        })
    });
    h.cx.read(|cx|{let state=h.state.read(cx);assert_eq!(state.bootstrap_operations[&operation],fixture.record.status);assert!(state.daemon_state.facade().is_none());assert_eq!(state.failure_terminal(child),Some(surge_core::TerminalReason::Failed),"current nested history failure must automatically retry without daemon or status update");assert!(!state.operation_inspections.contains_key(&operation));assert_eq!(state.needs_you_count(),1);});
    assert_eq!(h.queue(), vec![(format!("failed-{child}"), "failed")]);
    drop(h);
    fixture.close();
}

#[cfg(unix)]
#[test]
fn seventh_stale_nested_history_error_cannot_duplicate_replacement_live_failure() {
    use surge_orchestrator::engine::ipc::{
        DaemonRequest, DaemonResponse, read_request_frame, write_frame,
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::new(&runtime);
    let child = fixture.record.status.implementation_run;
    let operation = fixture.record.status.operation_id;
    let original = runtime
        .block_on(
            surge_persistence::runs::Storage::inspect_existing_run_events(
                fixture.home.path().join("runs"),
                child,
            ),
        )
        .unwrap();
    let path = fixture.obstruct(child);
    let mut h = fixture.harness();
    let scope = h
        .state
        .update(&mut h.cx, |state, _| state.observation_scope().unwrap());
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, cx| {
        app.read_operation_observed(scope, operation, cx, async move {
            let _ = seen_tx.send(());
            let _ = release_rx.await;
        })
    });
    let seen_rx = std::cell::RefCell::new(seen_rx);
    drive(&mut h.cx, |_| seen_rx.borrow_mut().try_recv().is_ok());
    SeventhFixture::restore(&path);
    // Publish only authentic child prefix to make it eligible for the actual subscription owner.
    h.state.update(&mut h.cx, |state, _| {
        state.run_streams.insert(
            child,
            crate::run_stream::RunStreamState::replay_candidate(child, &original[..2]),
        );
        state.runs.push(crate::app_state::UiRun {
            run_id: child,
            status: RunStatus::Active,
            started_at: chrono::Utc::now(),
            last_event_seq: Some(2),
            ended_at: None,
        });
    });
    let socket = fixture.home.path().join("seventh.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = runtime.spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut read = tokio::io::BufReader::new(read);
        let Some(DaemonRequest::Subscribe { request_id, run_id }) =
            read_request_frame(&mut read).await.unwrap()
        else {
            panic!("subscription expected")
        };
        assert_eq!(run_id, child);
        write_frame(&mut write, &DaemonResponse::SubscribeOk { request_id })
            .await
            .unwrap();
        let _ = stop_rx.await;
    });
    let facade = std::sync::Arc::new(
        runtime
            .block_on(
                surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket),
            )
            .unwrap(),
    );
    h.state.update(&mut h.cx, |state, _| {
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade.clone())
    });
    h.app
        .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).failure_terminal(child) == Some(surge_core::TerminalReason::Failed)
        })
    });
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        assert!(state.run_streams[&child].live);
        assert!(!state.operation_inspections.contains_key(&operation));
    });
    release_tx.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    let clean = h.cx.read(|cx| {
        let state = h.state.read(cx);
        state.operation_inspections.is_empty() && state.needs_you_count() == 1
    });
    let queue = h.queue();
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(100));
    h.cx.run_until_parked();
    h.cx.read(|cx| {
        assert!(h.app.read(cx).offline_retries.is_empty());
        assert_eq!(h.state.read(cx).related_history_reads[&child], 1);
    });
    stop_tx.send(()).unwrap();
    runtime.block_on(server).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| !h.state.read(cx).run_streams[&child].live)
    });
    drop(h);
    drop(facade);
    fixture.close();
    assert!(
        clean,
        "obsolete nested history error must not publish operation inspection before token validation"
    );
    assert_eq!(queue, vec![(format!("failed-{child}"), "failed")]);
}

fn seventh_observe(fixture: &SeventhFixture, h: &mut Harness) {
    h.app.update(&mut h.cx, |app, cx| {
        app.bootstrap_index.push(super::super::BootstrapIndexEntry {
            operation_id: fixture.record.status.operation_id,
            project_path: fixture.project.clone(),
        });
        app.restore_indexed_operations(cx);
    });
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
}
fn seventh_retry(h: &mut Harness, run: RunId, reads: usize) {
    h.cx.run_until_parked();
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(5));
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).related_history_reads.get(&run) == Some(&reads)
                && h.app.read(cx).offline_operations.is_empty()
        })
    });
}
#[test]
fn seventh_permanent_failure_has_four_actual_reads_and_sibling_success_never_resets_budget() {
    for mode in [0, 1, 2] {
        let both = mode != 0;
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::new(&runtime);
        let child = fixture.record.status.implementation_run;
        let parent = fixture.record.status.planning_run;
        let op = fixture.record.status.operation_id;
        let child_path = fixture.obstruct(child);
        let parent_path = both.then(|| fixture.obstruct(parent));
        let mut h = fixture.harness();
        seventh_observe(&fixture, &mut h);
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert_eq!(state.related_history_reads[&child], 1);
            assert!(state.operation_inspection_reason(op).is_some());
            assert!(state.runs.iter().all(|run| run.run_id != child));
        });
        if mode == 1 {
            SeventhFixture::restore(parent_path.as_ref().unwrap());
        }
        for count in 2..=4 {
            seventh_retry(&mut h, child, count);
            h.cx.read(|cx| assert!(h.state.read(cx).operation_inspection_reason(op).is_some()));
        }
        let before =
            h.cx.read(|cx| h.state.read(cx).related_history_reads.clone());
        h.cx.background_executor
            .advance_clock(std::time::Duration::from_secs(100));
        h.cx.run_until_parked();
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert_eq!(state.related_history_reads, before);
            assert_eq!(
                state.related_history_reads[&parent],
                if mode == 2 {
                    4
                } else if both {
                    2
                } else {
                    1
                }
            );
            assert!(
                state
                    .operation_inspection_reason(op)
                    .unwrap()
                    .contains("retries exhausted")
            );
            assert_eq!(state.failure_terminal(child), None);
            assert_eq!(state.needs_you_count(), 1);
        });
        assert_eq!(h.queue(), vec![(format!("operation-{op}"), "operation")]);
        if mode == 2 {
            assert_eq!(
                h.cx.read(|cx| h
                    .state
                    .read(cx)
                    .operation_inspection_reason(op)
                    .unwrap()
                    .matches("retries exhausted")
                    .count()),
                2
            );
            SeventhFixture::restore(parent_path.as_ref().unwrap());
        }
        SeventhFixture::restore(&child_path);
        drop(h);
        fixture.close();
    }
}
#[test]
fn seventh_retry_eligibility_is_per_failed_sibling_and_preserves_replacement_inflight() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::new(&runtime);
    let child = fixture.record.status.implementation_run;
    let parent = fixture.record.status.planning_run;
    let op = fixture.record.status.operation_id;
    let child_path = fixture.obstruct(child);
    let parent_path = fixture.obstruct(parent);
    let mut h = fixture.harness();
    seventh_observe(&fixture, &mut h);
    let replacement = h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, child, true).unwrap();
        state.begin_history_read(&owner).unwrap()
    });
    for reads in 2..=4 {
        seventh_retry(&mut h, parent, reads);
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert_eq!(state.related_history_reads[&child], 1);
            assert!(state.observations.accepts_read(&replacement));
            assert!(state.operation_inspection_reason(op).is_some());
        });
    }
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(100));
    h.cx.run_until_parked();
    h.state.update(&mut h.cx, |state, _| {
        assert!(state.observations.accepts_read(&replacement));
        assert_eq!(state.related_history_reads[&parent], 4);
        assert_eq!(state.related_history_reads[&child], 1);
        state.observations.finish_read(&replacement, false);
        state.observations.retire(&replacement);
    });
    SeventhFixture::restore(&child_path);
    SeventhFixture::restore(&parent_path);
    drop(h);
    fixture.close();
}
#[test]
fn seventh_new_read_generation_and_project_return_make_queued_retry_inert() {
    for return_project in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::new(&runtime);
        let child = fixture.record.status.implementation_run;
        let op = fixture.record.status.operation_id;
        let path = fixture.obstruct(child);
        let mut h = fixture.harness();
        seventh_observe(&fixture, &mut h);
        let token = h.cx.read(|cx| {
            h.app
                .read(cx)
                .offline_retries
                .values()
                .next()
                .unwrap()
                .eligible[0]
                .clone()
        });
        let replacement = h.state.update(&mut h.cx, |state, _| {
            if return_project {
                state.project_path = Some(fixture.project.join("other"));
                state.observation_scope();
                state.project_path = Some(fixture.project.clone());
                state.observation_scope();
                let scope = state.observation_scope().unwrap();
                let owner = state.observations.claim(&scope, child, false).unwrap();
                state.begin_history_read(&owner).unwrap()
            } else {
                state.begin_history_read(&token).unwrap()
            }
        });
        h.cx.background_executor
            .advance_clock(std::time::Duration::from_secs(100));
        h.cx.run_until_parked();
        h.state.update(&mut h.cx, |state, _| {
            assert!(state.observations.accepts_read(&replacement));
            assert_eq!(state.related_history_reads[&child], 1);
            assert!(state.operation_inspection_reason(op).is_none());
            state.observations.finish_read(&replacement, false);
            state.observations.retire(&replacement);
        });
        SeventhFixture::restore(&path);
        drop(h);
        fixture.close();
    }
}
#[test]
fn seventh_related_diagnostic_uses_last_read_and_post_error_revision_without_bypassing_live_error()
{
    for live_error in [None, Some(false), Some(true)] {
        let mut h = Harness::new();
        let run = RunId::new();
        let op = RunId::new();
        h.run(
            run,
            vec![start(std::path::Path::new(PROJECT))],
            RunStatus::Active,
        );
        h.state.update(&mut h.cx, |state, _| {
            let scope = state.observation_scope().unwrap();
            let owner = state.observations.claim(&scope, run, false).unwrap();
            let error = |state: &mut AppState| {
                state
                    .run_streams
                    .get_mut(&run)
                    .unwrap()
                    .apply(&EngineRunEvent::StreamError {
                        message: "actual live error".into(),
                    })
            };
            if live_error == Some(false) {
                error(state);
            }
            let read = state.begin_history_read(&owner).unwrap();
            if live_error == Some(true) {
                error(state);
            }
            let completion =
                state.complete_related_read(op, &read, Err("journal unavailable".into()), true);
            assert_eq!(
                completion.disposition,
                crate::app_state::ReadDisposition::Incomplete
            );
            assert!(state.observations.last_read_is_current(&read));
            if live_error.is_none() {
                assert!(
                    state
                        .operation_inspection_reason(op)
                        .unwrap()
                        .contains("journal unavailable")
                );
                state.set_operation_refusal(op, "genuine record integrity refusal".into());
                let next = state.begin_history_read(&owner).unwrap();
                assert!(
                    state
                        .complete_related_read(
                            op,
                            &next,
                            Ok(Some(rows(vec![start(std::path::Path::new(PROJECT))]))),
                            true
                        )
                        .confirmed()
                );
                assert_eq!(
                    state.operation_inspection_reason(op).as_deref(),
                    Some("genuine record integrity refusal")
                );
            } else {
                assert!(state.operation_inspection_reason(op).is_none());
                assert!(
                    state.run_streams[&run]
                        .observation_error
                        .as_deref()
                        .unwrap()
                        .contains("actual live error")
                );
            }
        });
    }
}

#[test]
fn seventh_current_older_prefix_retries_without_regressing_trusted_terminal() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::with_child_terminal(&runtime, false);
    let child = fixture.record.status.implementation_run;
    let op = fixture.record.status.operation_id;
    let mut h = fixture.harness();
    let scope = h
        .state
        .update(&mut h.cx, |state, _| state.observation_scope().unwrap());
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, cx| {
        app.read_operation_observed(scope, op, cx, async move {
            let _ = seen_tx.send(());
            let _ = release_rx.await;
        })
    });
    let seen_rx = std::cell::RefCell::new(seen_rx);
    drive(&mut h.cx, |_| seen_rx.borrow_mut().try_recv().is_ok());
    runtime.block_on(async {
        let writer = fixture.storage.open_run_writer(child).await.unwrap();
        writer
            .append_events(vec![surge_core::VersionedEventPayload::new(failed())])
            .await
            .unwrap();
        writer.close().await.unwrap();
    });
    let current = runtime
        .block_on(super::super::observation::read_history(
            fixture.home.path(),
            child,
        ))
        .unwrap()
        .unwrap();
    h.state.update(&mut h.cx, |state, _| {
        state.run_streams.insert(
            child,
            crate::run_stream::RunStreamState::replay_candidate(child, &current),
        );
    });
    release_tx.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        assert_eq!(state.run_streams[&child].last_seq, 3);
        assert_eq!(
            state.failure_terminal(child),
            Some(surge_core::TerminalReason::Failed)
        );
        assert!(state.operation_inspection_reason(op).is_some());
    });
    seventh_retry(&mut h, child, 2);
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        assert_eq!(state.run_streams[&child].last_seq, 3);
        assert_eq!(
            state.failure_terminal(child),
            Some(surge_core::TerminalReason::Failed)
        );
        assert!(state.operation_inspection_reason(op).is_none());
        assert_eq!(
            state.related_history_reads[&fixture.record.status.planning_run],
            1
        );
    });
    assert_eq!(h.queue(), vec![(format!("failed-{child}"), "failed")]);
    drop(h);
    fixture.close();
}
#[test]
fn seventh_late_project_return_batch_cannot_clear_current_batch_or_publish_old_error() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::new(&runtime);
    let op = fixture.record.status.operation_id;
    let child = fixture.record.status.implementation_run;
    let path = fixture.obstruct(child);
    let mut h = fixture.harness();
    let scope = h
        .state
        .update(&mut h.cx, |state, _| state.observation_scope().unwrap());
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, cx| {
        app.read_operation_observed(scope, op, cx, async move {
            let _ = seen_tx.send(());
            let _ = release_rx.await;
        })
    });
    let seen_rx = std::cell::RefCell::new(seen_rx);
    drive(&mut h.cx, |_| seen_rx.borrow_mut().try_recv().is_ok());
    let scope = h.state.update(&mut h.cx, |state, _| {
        state.project_path = Some(fixture.project.join("other"));
        state.observation_scope();
        state.project_path = Some(fixture.project.clone());
        state.observation_scope().unwrap()
    });
    let (new_tx, new_rx) = tokio::sync::oneshot::channel();
    let (new_release_tx, new_release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, cx| {
        app.read_operation_observed(scope.clone(), op, cx, async move {
            let _ = new_tx.send(());
            let _ = new_release_rx.await;
        })
    });
    let new_rx = std::cell::RefCell::new(new_rx);
    drive(&mut h.cx, |_| new_rx.borrow_mut().try_recv().is_ok());
    release_tx.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.len() == 1)
    });
    h.cx.read(|cx| {
        assert!(
            h.app
                .read(cx)
                .offline_operations
                .contains_key(&(scope.clone(), op))
        );
        assert!(h.state.read(cx).operation_inspections.is_empty());
        assert!(h.app.read(cx).offline_retries.is_empty());
    });
    new_release_tx.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    h.cx.read(|cx| {
        assert!(h.state.read(cx).operation_inspection_reason(op).is_some());
        assert_eq!(h.state.read(cx).related_history_reads[&child], 2);
    });
    h.state
        .update(&mut h.cx, |state, _| state.observations.clear());
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(100));
    h.cx.run_until_parked();
    h.cx.read(|cx| assert_eq!(h.state.read(cx).related_history_reads[&child], 2));
    SeventhFixture::restore(&path);
    drop(h);
    fixture.close();
}
#[test]
fn seventh_genuine_record_corruption_survives_current_successful_sibling_history() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::new(&runtime);
    let op = fixture.record.status.operation_id;
    let parent = fixture.record.status.planning_run;
    let mut h = fixture.harness();
    seventh_observe(&fixture, &mut h);
    assert_eq!(h.queue().len(), 1);
    let pool = surge_persistence::runs::registry::open_registry_pool(
        fixture.home.path(),
        &surge_persistence::runs::MockClock::new(1),
    )
    .unwrap();
    pool.get().unwrap().execute("UPDATE bootstrap_operations SET intent_fingerprint='0000000000000000000000000000000000000000000000000000000000000000'",[]).unwrap();
    h.app
        .update(&mut h.cx, |app, cx| app.restore_indexed_operations(cx));
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    let refusal =
        h.cx.read(|cx| h.state.read(cx).operation_inspection_reason(op).unwrap());
    let history = runtime
        .block_on(super::super::observation::read_history(
            fixture.home.path(),
            parent,
        ))
        .unwrap();
    h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, parent, true).unwrap();
        let read = state.begin_history_read(&owner).unwrap();
        assert!(
            state
                .complete_related_read(op, &read, Ok(history), true)
                .confirmed()
        );
        assert_eq!(
            state.operation_inspection_reason(op).as_deref(),
            Some(refusal.as_str())
        );
        state.observations.retire(&owner);
    });
    assert_eq!(h.queue().len(), 2);
    drop(h);
    drop(pool);
    fixture.close();
}

#[test]
fn seventh_expected_started_absent_failed_or_completed_journal_retries_without_inventing_run() {
    for completed_child in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::with_state(&runtime, true, completed_child);
        let child = fixture.record.status.implementation_run;
        let op = fixture.record.status.operation_id;
        let path = fixture.home.path().join("runs").join(child.to_string());
        let held = path.with_extension("held");
        std::fs::rename(&path, &held).unwrap();
        assert!(
            runtime
                .block_on(super::super::observation::read_history(
                    fixture.home.path(),
                    child
                ))
                .unwrap()
                .is_none()
        );
        let mut h = fixture.harness();
        seventh_observe(&fixture, &mut h);
        let inspection = h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert!(state.runs.iter().all(|run| run.run_id != child));
            assert!(!state.run_streams.contains_key(&child));
            state.operation_inspection_reason(op)
        });
        std::fs::rename(&held, &path).unwrap();
        if inspection.is_some() {
            seventh_retry(&mut h, child, 2);
            h.cx.read(|cx| {
                let state = h.state.read(cx);
                assert!(state.operation_inspection_reason(op).is_none());
                assert_eq!(
                    state.run_streams[&child].display(),
                    surge_core::run_display::RunDisplayState::Done(if completed_child {
                        surge_core::TerminalReason::Completed
                    } else {
                        surge_core::TerminalReason::Failed
                    })
                );
            });
        }
        drop(h);
        fixture.close();
        assert!(
            inspection.is_some(),
            "missing expected-started child must remain an inspectable retriable observation (completed={completed_child})"
        );
    }
}

#[test]
fn eighth_cold_registry_crashed_survives_actual_app_candidate_publication() {
    super::run_isolated_app_probe(
        "eighth-cold",
        "app::tests::home_inbox_sixth::eighth_cold_registry_child_probe",
    );
}
#[test]
fn eighth_cold_registry_child_probe() {
    if std::env::var("SURGE_TEST_PROJECT_HOME_ENTRY").as_deref() != Ok("eighth-cold") {
        return;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let home = std::path::PathBuf::from(std::env::var_os("SURGE_HOME").unwrap())
        .canonicalize()
        .unwrap();
    let storage = runtime
        .block_on(surge_persistence::runs::Storage::open(&home))
        .unwrap();
    for case in ["ordinary", "capacity", "terminal"] {
        let project = home.join(case);
        std::fs::create_dir(&project).unwrap();
        let run = RunId::new();
        let mut payloads = vec![start(&project)];
        if case == "capacity" {
            let graph: surge_core::Graph =
                toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml"))
                    .unwrap();
            let hash = graph.identity().unwrap();
            payloads.push(E::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash: hash,
            });
            payloads.push(E::RunParked {
                wake_at: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
                runtime: Some("fixture-runtime".into()),
                worktree: project.clone(),
                basis: surge_core::capacity::WakeBasis::ObservedReset,
                reason: "capacity".into(),
            });
        }
        if case == "terminal" {
            payloads.push(failed());
        }
        runtime.block_on(async {
            let writer = storage.create_run(run, &project, None).await.unwrap();
            writer
                .append_events(
                    payloads
                        .into_iter()
                        .map(surge_core::VersionedEventPayload::new)
                        .collect(),
                )
                .await
                .unwrap();
            writer.close().await.unwrap();
            storage
                .set_run_status(&run, surge_core::RunStatus::Crashed, None)
                .await
                .unwrap();
        });
        if case == "capacity" {
            let events = runtime
                .block_on(super::super::observation::read_history(&home, run))
                .unwrap()
                .unwrap();
            assert!(
                matches!(
                    crate::run_stream::RunStreamState::replay_candidate(run, &events).display(),
                    surge_core::run_display::RunDisplayState::Waiting(
                        surge_core::run_display::WaitingReason::Capacity { .. }
                    )
                ),
                "capacity journal must positively fold to waiting without independent Crashed hint"
            );
        }
        let summaries = runtime
            .block_on(
                surge_persistence::runs::Storage::inspect_existing_run_summaries(
                    home.clone(),
                    std::num::NonZeroU32::new(10).unwrap(),
                ),
            )
            .unwrap();
        assert_eq!(
            summaries
                .iter()
                .find(|summary| summary.id == run)
                .unwrap()
                .status,
            surge_core::RunStatus::Crashed
        );
        let mut h = Harness::new();
        h.cx.background_executor.allow_parking();
        h.state.update(&mut h.cx, |state, _| {
            state.runtime_home = Some(home.clone());
            state.project_path = Some(project);
        });
        h.app
            .update(&mut h.cx, |_, cx| SurgeApp::spawn_daemon_link(&h.state, cx));
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                matches!(
                    h.state.read(cx).daemon_state,
                    crate::daemon_link::ConnectionState::Failed(_)
                )
            })
        });
        h.cx.read(|cx|{let state=h.state.read(cx);assert_eq!(state.run_streams[&run].current_registry_hint(),Some(surge_core::RunStatus::Crashed));assert_eq!(state.run_streams[&run].display(),if case=="terminal" {surge_core::run_display::RunDisplayState::Done(surge_core::TerminalReason::Failed)} else {surge_core::run_display::RunDisplayState::Waiting(surge_core::run_display::WaitingReason::RecoveryRequired)},"current independently registered Crashed survives ordinary/capacity replay; terminal wins");});
        assert_eq!(
            h.queue(),
            vec![(
                format!(
                    "{}-{run}",
                    if case == "terminal" {
                        "failed"
                    } else {
                        "inspection"
                    }
                ),
                if case == "terminal" {
                    "failed"
                } else {
                    "inspect"
                }
            )]
        );
        if case != "terminal" {
            h.state.update(&mut h.cx, |state, _| {
                state.apply_global_event(
                    &surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted {
                        run_id: run,
                    },
                )
            });
            h.cx.read(|cx| {
                let stream = &h.state.read(cx).run_streams[&run];
                assert_eq!(
                    stream.current_registry_hint(),
                    Some(surge_core::RunStatus::Running)
                );
                assert_ne!(
                    stream.display(),
                    surge_core::run_display::RunDisplayState::Working
                );
                assert_ne!(
                    stream.display(),
                    surge_core::run_display::RunDisplayState::Waiting(
                        surge_core::run_display::WaitingReason::RecoveryRequired
                    )
                );
            });
        }
        drop(h);
    }
    drop(storage);
}

#[test]
fn eighth_newer_terminal_operation_demand_survives_stale_success_or_error_batch() {
    for old_error in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::with_operation(&runtime, true, false, false);
        let op = fixture.record.status.operation_id;
        let child = fixture.record.status.implementation_run;
        let pool = surge_persistence::runs::registry::open_registry_pool(
            fixture.home.path(),
            &surge_persistence::runs::MockClock::new(1),
        )
        .unwrap();
        if old_error {
            pool.get().unwrap().execute("UPDATE bootstrap_operations SET intent_fingerprint='0000000000000000000000000000000000000000000000000000000000000000'",[]).unwrap();
        }
        let mut h = fixture.harness();
        let scope = h
            .state
            .update(&mut h.cx, |state, _| state.observation_scope().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        h.app.update(&mut h.cx, |app, cx| {
            app.read_operation_observed(scope, op, cx, async move {
                let _ = tx.send(());
                let _ = release_rx.await;
            })
        });
        let rx = std::cell::RefCell::new(rx);
        drive(&mut h.cx, |_| rx.borrow_mut().try_recv().is_ok());
        if old_error {
            pool.get()
                .unwrap()
                .execute(
                    "UPDATE bootstrap_operations SET intent_fingerprint=?1",
                    [fixture.record.intent_fingerprint.to_string()],
                )
                .unwrap();
        }
        let store = fixture.storage.bootstrap_operation_store();
        let middle = store
            .block(op, fixture.record.status.revision, A::StorageUnconfirmed)
            .unwrap();
        h.app.update(&mut h.cx, |app, cx| {
            app.update_bootstrap_view(op, middle.status.clone(), &fixture.project, cx)
        });
        let current = store
            .settle(
                op,
                middle.status.revision,
                &T::Failed {
                    run_id: child,
                    reason: F::RunFailed,
                },
            )
            .unwrap();
        assert!(current.status.revision > fixture.record.status.revision);
        h.app.update(&mut h.cx, |app, cx| {
            app.update_bootstrap_view(op, current.status.clone(), &fixture.project, cx)
        });
        release_tx.send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
        });
        // Only the pending demand can service this: no second poll/global/manual hydration.
        for _ in 0..50 {
            h.cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let actual = h.cx.read(|cx| {
            assert_eq!(h.state.read(cx).bootstrap_operations[&op], current.status);
            h.state.read(cx).failure_terminal(child)
        });
        drop(h);
        drop(store);
        drop(pool);
        fixture.close();
        assert_eq!(
            actual,
            Some(surge_core::TerminalReason::Failed),
            "new terminal request must drain after stale batch settlement (old_error={old_error})"
        );
    }
}
#[cfg(unix)]
#[test]
fn eighth_rejected_actual_subscription_recovers_finished_journal_without_old_owner_resurrection() {
    for combined in [false, true] {
        use surge_orchestrator::engine::ipc::{
            DaemonRequest, DaemonResponse, ErrorCode, read_request_frame, write_frame,
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = if combined {
            SeventhFixture::with_operation(&runtime, true, false, false)
        } else {
            SeventhFixture::new(&runtime)
        };
        let child = fixture.record.status.implementation_run;
        let op = fixture.record.status.operation_id;
        let mut h = fixture.harness();
        let original = runtime
            .block_on(super::super::observation::read_history(
                fixture.home.path(),
                child,
            ))
            .unwrap()
            .unwrap();
        h.state.update(&mut h.cx, |state, _| {
            state.run_streams.insert(
                child,
                crate::run_stream::RunStreamState::replay_candidate(child, &original[..2]),
            );
            state.runs.push(crate::app_state::UiRun {
                run_id: child,
                status: RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: Some(2),
                ended_at: None,
            });
        });
        let scope = h
            .state
            .update(&mut h.cx, |state, _| state.observation_scope().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        h.app.update(&mut h.cx, |app, cx| {
            app.read_operation_observed(scope, op, cx, async move {
                let _ = tx.send(());
                let _ = release_rx.await;
            })
        });
        let rx = std::cell::RefCell::new(rx);
        drive(&mut h.cx, |_| rx.borrow_mut().try_recv().is_ok());
        let store = fixture.storage.bootstrap_operation_store();
        if combined {
            let current = store
                .settle(
                    op,
                    fixture.record.status.revision,
                    &T::Failed {
                        run_id: child,
                        reason: F::RunFailed,
                    },
                )
                .unwrap();
            h.app.update(&mut h.cx, |app, cx| {
                app.update_bootstrap_view(op, current.status, &fixture.project, cx)
            });
        }
        let socket = fixture.home.path().join("reject.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (rejected_tx, rejected_rx) = tokio::sync::oneshot::channel();
        let server = runtime.spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut read = tokio::io::BufReader::new(read);
            let Some(DaemonRequest::Subscribe { request_id, run_id }) =
                read_request_frame(&mut read).await.unwrap()
            else {
                panic!("subscribe expected")
            };
            assert_eq!(run_id, child);
            write_frame(
                &mut write,
                &DaemonResponse::Error {
                    request_id,
                    code: ErrorCode::RunNotActive,
                    message: "run finished before attachment".into(),
                },
            )
            .await
            .unwrap();
            let _ = rejected_tx.send(());
            let _ = stop_rx.await;
        });
        let facade = std::sync::Arc::new(
            runtime
                .block_on(
                    surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket),
                )
                .unwrap(),
        );
        h.state.update(&mut h.cx, |state, _| {
            state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade.clone())
        });
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
        let rejected_rx = std::cell::RefCell::new(rejected_rx);
        drive(&mut h.cx, |_| rejected_rx.borrow_mut().try_recv().is_ok());
        drive(&mut h.cx, |cx| {
            cx.read(|cx| !h.app.read(cx).stream_subscribed.contains(&child))
        });
        release_tx.send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
        });
        for _ in 0..50 {
            h.cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let actual = h.cx.read(|cx| {
            assert!(!h.state.read(cx).run_streams[&child].live);
            h.state.read(cx).failure_terminal(child)
        });
        stop_tx.send(()).unwrap();
        runtime.block_on(server).unwrap();
        drop(h);
        drop(facade);
        drop(store);
        fixture.close();
        assert_eq!(
            actual,
            Some(surge_core::TerminalReason::Failed),
            "rejected current attachment must recover durable terminal under fresh current owner"
        );
    }
}

#[test]
fn eighth_inbox_merge_preserves_actual_error_and_unmergeable_exact_inspection() {
    let mut h = Harness::new();
    let run = RunId::new();
    let op = RunId::new();
    let reserved = RunId::new();
    h.run(
        run,
        vec![start(std::path::Path::new(PROJECT))],
        RunStatus::Active,
    );
    h.state.update(&mut h.cx, |state, _| {
        state
            .run_streams
            .get_mut(&run)
            .unwrap()
            .apply(&EngineRunEvent::StreamError {
                message: "actual current feed diagnostic".into(),
            })
    });
    h.update(status(
        op,
        run,
        reserved,
        S::NeedsAttention {
            phase: P::Planning,
            run_id: Some(run),
            reason: A::StorageUnconfirmed,
        },
        None,
        2,
    ));
    assert_eq!(h.queue().len(), 1);
    let inbox = h.cx.read(|cx| h.app.read(cx).inbox.clone().unwrap());
    let merged = inbox.update(&mut h.cx, |screen, cx| screen.evidence_for_test(cx));
    let has_error = merged
        .iter()
        .flat_map(|(_, evidence, _)| evidence)
        .any(|(_, text)| text.contains("actual current feed diagnostic"));
    h.state.update(&mut h.cx, |state, _| {
        let scope = state.observation_scope().unwrap();
        let owner = state.observations.claim(&scope, run, false).unwrap();
        let read = state.begin_history_read(&owner).unwrap();
        let mut conflict = rows(vec![start(std::path::Path::new(PROJECT))]);
        if let E::RunStarted { initial_prompt, .. } = &mut conflict[0].payload.payload {
            *initial_prompt = "contradictory origin".into();
        }
        assert!(
            !state
                .complete_history_read(&read, Ok(Some(conflict)))
                .confirmed()
        );
        assert_eq!(
            state.run_streams[&run].display(),
            surge_core::run_display::RunDisplayState::Unknown
        );
        assert!(!state.run_streams[&run].session_history_confirmed());
    });
    let queue = h.queue();
    let facts = inbox.update(&mut h.cx, |screen, cx| screen.evidence_for_test(cx));
    let standalone = facts
        .iter()
        .any(|(id, _, links)| id == &format!("inspection-{run}") && links == &vec![run]);
    drop(inbox);
    drop(h);
    assert!(
        has_error,
        "merged Inbox item must retain actual current StreamError reason"
    );
    assert!(
        standalone,
        "invalidated history needs canonical standalone exact inspection route"
    );
    assert_eq!(queue.len(), 2);
}

#[cfg(unix)]
struct EighthRejectPeer {
    facade: std::sync::Arc<surge_orchestrator::engine::daemon_facade::DaemonEngineFacade>,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[cfg(unix)]
impl EighthRejectPeer {
    fn new(runtime: &tokio::runtime::Runtime, home: &std::path::Path, run: RunId) -> Self {
        Self::with_response(runtime, home, run, false)
    }
    fn with_response(
        runtime: &tokio::runtime::Runtime,
        home: &std::path::Path,
        run: RunId,
        accept: bool,
    ) -> Self {
        use surge_orchestrator::engine::ipc::{
            DaemonRequest, DaemonResponse, ErrorCode, read_request_frame, write_frame,
        };
        let socket = home.join(format!("reject-{}.sock", run.short()));
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = requests.clone();
        let server=runtime.spawn(async move{let(stream,_)=listener.accept().await.unwrap();let(read,mut write)=stream.into_split();let mut read=tokio::io::BufReader::new(read);loop{let request=tokio::select!{result=read_request_frame(&mut read)=>result.unwrap(),_=&mut stopped=>return};let Some(DaemonRequest::Subscribe{request_id,run_id})=request else{return;};assert_eq!(run_id,run);counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst);write_frame(&mut write,&if accept {DaemonResponse::SubscribeOk{request_id}} else {DaemonResponse::Error{request_id,code:ErrorCode::RunNotActive,message:"fixture rejection".into()}}).await.unwrap();}});
        let facade = std::sync::Arc::new(
            runtime
                .block_on(
                    surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket),
                )
                .unwrap(),
        );
        Self {
            facade,
            stop,
            server,
            requests,
        }
    }
    fn close(self, runtime: &tokio::runtime::Runtime) {
        let _ = self.stop.send(());
        runtime.block_on(self.server).unwrap();
        drop(self.facade);
    }
}
#[cfg(unix)]
fn settled_rejected_read(
    h: &Harness,
    run: RunId,
    count: usize,
    original: Option<&crate::run_observation::Token>,
) -> crate::run_observation::Token {
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        assert_eq!(state.rejected_history_reads.get(&run), Some(&count));
        assert!(!state.observations.is_reading(run));
        if let Some(original) = original {
            assert!(
                state.observations.accepts(original),
                "same original publisher"
            );
        }
        h.app
            .read(cx)
            .current_read_settlements
            .iter()
            .rev()
            .find(|(token, kind)| {
                token.run == run
                    && *kind == crate::app_state::ReadDisposition::Incomplete
                    && state.observations.last_read_is_current(token)
            })
            .expect("actual current-owner incomplete read settled")
            .0
            .clone()
    })
}

#[cfg(unix)]
fn rejected_retry_clock_step(cx: &mut TestAppContext, completed: usize, maximum: usize) {
    // Independent fixed expectations: three retries, then retain the old exhaustion window.
    let delay = match (completed, maximum) {
        (count, max) if count == max => std::time::Duration::from_secs(5),
        (1, _) => std::time::Duration::from_millis(500),
        (2, _) => std::time::Duration::from_secs(1),
        (3, _) => std::time::Duration::from_secs(2),
        _ => panic!("unexpected fixture retry ordinal"),
    };
    cx.run_until_parked();
    cx.background_executor.advance_clock(delay);
    cx.run_until_parked();
}

#[cfg(unix)]
#[test]
fn eighth_ordinary_rejected_subscription_has_four_reads_no_operation_and_inert_replacement_timer() {
    for replacement in [0, 1, 2] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::new(&runtime);
        let run = RunId::new();
        runtime.block_on(async {
            let writer = fixture
                .storage
                .create_run(run, &fixture.project, None)
                .await
                .unwrap();
            writer
                .append_events(vec![surge_core::VersionedEventPayload::new(start(
                    &fixture.project,
                ))])
                .await
                .unwrap();
            writer.close().await.unwrap();
        });
        let path = fixture.obstruct(run);
        let mut h = fixture.harness();
        h.run(run, vec![start(&fixture.project)], RunStatus::Active);
        let peer = EighthRejectPeer::new(&runtime, fixture.home.path(), run);
        h.state.update(&mut h.cx, |state, _| {
            state.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
        });
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.state.read(cx).rejected_history_reads.get(&run) == Some(&1)
                    && !h.state.read(cx).observations.is_reading(run)
            })
        });
        let current = if replacement > 0 {
            Some(h.state.update(&mut h.cx, |state, _| {
                if replacement == 2 {
                    state.project_path = Some(fixture.project.join("other"));
                    state.observation_scope();
                    state.project_path = Some(fixture.project.clone());
                }
                let scope = state.observation_scope().unwrap();
                let owner = state.observations.claim(&scope, run, true).unwrap();
                state.begin_history_read(&owner).unwrap()
            }))
        } else {
            None
        };
        if replacement == 0 {
            let original = settled_rejected_read(&h, run, 1, None);
            for count in 2..=4 {
                rejected_retry_clock_step(&mut h.cx, count - 1, 4);
                drive(&mut h.cx, |cx| {
                    cx.read(|cx| {
                        h.state.read(cx).rejected_history_reads.get(&run) == Some(&count)
                            && !h.state.read(cx).observations.is_reading(run)
                    })
                });
                settled_rejected_read(&h, run, count, Some(&original));
            }
        }
        h.cx.background_executor
            .advance_clock(std::time::Duration::from_secs(100));
        h.cx.run_until_parked();
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
        h.cx.run_until_parked();
        h.state.update(&mut h.cx, |state, _| {
            assert!(state.bootstrap_operations.is_empty());
            assert!(state.operation_inspections.is_empty());
            assert_eq!(
                state.rejected_history_reads[&run],
                if replacement == 0 { 4 } else { 1 }
            );
            assert_eq!(state.failure_terminal(run), None);
            assert!(!state.run_streams[&run].live);
            if let Some(token) = &current {
                assert!(state.observations.accepts_read(token));
                state.observations.finish_read(token, false);
                state.observations.retire(token);
            } else {
                assert!(
                    state
                        .run_inspection_evidence(run)
                        .iter()
                        .any(|(_, reason)| reason.contains("retries exhausted"))
                );
                assert_eq!(state.needs_you_count(), 1);
            }
        });
        assert_eq!(peer.requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        SeventhFixture::restore(&path);
        peer.close(&runtime);
        drop(h);
        fixture.close();
    }
}
#[cfg(unix)]
#[test]
fn eighth_latest_operation_demand_waits_for_actual_current_read_settlement() {
    for close_before_read in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::with_operation(&runtime, true, false, false);
        let op = fixture.record.status.operation_id;
        let run = fixture.record.status.implementation_run;
        let mut h = fixture.harness();
        // Actual subscription read is paused after loading, before canonical publication.
        let prefix = runtime
            .block_on(super::super::observation::read_history(
                fixture.home.path(),
                run,
            ))
            .unwrap()
            .unwrap();
        h.state.update(&mut h.cx, |state, _| {
            state.run_streams.insert(
                run,
                crate::run_stream::RunStreamState::replay_candidate(run, &prefix[..2]),
            );
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: Some(2),
                ended_at: None,
            });
        });
        use surge_orchestrator::engine::ipc::{
            DaemonRequest, DaemonResponse, read_request_frame, write_frame,
        };
        let socket = fixture.home.path().join("pending-owned.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let server = runtime.spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut read = tokio::io::BufReader::new(read);
            let Some(DaemonRequest::Subscribe { request_id, .. }) =
                read_request_frame(&mut read).await.unwrap()
            else {
                panic!("subscribe expected")
            };
            write_frame(&mut write, &DaemonResponse::SubscribeOk { request_id })
                .await
                .unwrap();
            let _ = stop_rx.await;
        });
        let facade = std::sync::Arc::new(
            runtime
                .block_on(
                    surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket),
                )
                .unwrap(),
        );
        h.state.update(&mut h.cx, |state, _| {
            state.daemon_state = crate::daemon_link::ConnectionState::Connected(facade.clone())
        });
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        h.app.update(&mut h.cx, |app, cx| {
            app.controlled_history_reads = Some(std::rc::Rc::new(std::cell::RefCell::new(
                std::collections::VecDeque::from([(tx, release_rx)]),
            )));
            app.sync_run_subscriptions(cx);
        });
        let rx = std::cell::RefCell::new(rx);
        drive(&mut h.cx, |_| rx.borrow_mut().try_recv().is_ok());
        let store = fixture.storage.bootstrap_operation_store();
        let current = store
            .settle(
                op,
                fixture.record.status.revision,
                &T::Failed {
                    run_id: run,
                    reason: F::RunFailed,
                },
            )
            .unwrap();
        h.app.update(&mut h.cx, |app, cx| {
            app.update_bootstrap_view(op, current.status.clone(), &fixture.project, cx)
        });
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
        });
        h.cx.read(|cx| assert_eq!(h.app.read(cx).pending_operations.len(), 1));
        let mut peer_end = Some((stop_tx, server));
        if close_before_read {
            let (stop, server) = peer_end.take().unwrap();
            stop.send(()).unwrap();
            runtime.block_on(server).unwrap();
            h.cx.run_until_parked();
            h.cx.read(|cx| assert!(h.state.read(cx).observations.is_reading(run)));
        }
        release_tx.send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.state.read(cx).failure_terminal(run) == Some(surge_core::TerminalReason::Failed)
                    && h.app.read(cx).pending_operations.is_empty()
                    && h.app.read(cx).offline_operations.is_empty()
            })
        });
        assert_eq!(h.queue(), vec![(format!("failed-{run}"), "failed")]);
        if let Some((stop, server)) = peer_end.take() {
            stop.send(()).unwrap();
            runtime.block_on(server).unwrap();
        }
        drive(&mut h.cx, |cx| {
            cx.read(|cx| !h.state.read(cx).run_streams[&run].live)
        });
        drop(h);
        drop(facade);
        drop(store);
        fixture.close();
    }
}

#[test]
fn eighth_pending_same_snapshot_demand_does_not_duplicate_or_reset_four_read_episode() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::new(&runtime);
    let op = fixture.record.status.operation_id;
    let run = fixture.record.status.implementation_run;
    let path = fixture.obstruct(run);
    let mut h = fixture.harness();
    let mut observed = Vec::new();
    let mut barriers = std::collections::VecDeque::new();
    for _ in 0..4 {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        observed.push((std::cell::RefCell::new(rx), release_tx));
        barriers.push_back((tx, release_rx));
    }
    h.app.update(&mut h.cx, |app, cx| {
        app.controlled_operation_reads = Some(std::rc::Rc::new(std::cell::RefCell::new(barriers)));
        app.bootstrap_index.push(super::super::BootstrapIndexEntry {
            operation_id: op,
            project_path: fixture.project.clone(),
        });
        app.restore_indexed_operations(cx);
    });
    for (index, (loaded, release)) in observed.into_iter().enumerate() {
        if index > 0 {
            h.cx.run_until_parked();
            h.cx.background_executor
                .advance_clock(std::time::Duration::from_secs(5));
        }
        drive(&mut h.cx, |_| loaded.borrow_mut().try_recv().is_ok());
        h.app.update(&mut h.cx, |app, cx| {
            app.update_bootstrap_view(op, fixture.record.status.clone(), &fixture.project, cx);
            app.hydrate_related(op, cx);
            app.hydrate_related(op, cx);
        });
        h.cx.read(|cx| assert_eq!(h.app.read(cx).pending_operations.len(), 1));
        release.send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
        });
        h.cx.read(|cx| {
            assert!(h.app.read(cx).pending_operations.is_empty());
            assert_eq!(h.state.read(cx).related_history_reads[&run], index + 1);
        });
    }
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(100));
    h.cx.run_until_parked();
    h.cx.read(|cx| {
        assert_eq!(h.state.read(cx).related_history_reads[&run], 4);
        assert!(
            h.state
                .read(cx)
                .operation_inspection_reason(op)
                .unwrap()
                .contains("retries exhausted")
        );
    });
    SeventhFixture::restore(&path);
    drop(h);
    fixture.close();
}
#[cfg(unix)]
#[test]
fn eighth_recovered_record_episode_demand_drains_after_live_reader_settles() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::new(&runtime);
    let op = fixture.record.status.operation_id;
    let run = fixture.record.status.implementation_run;
    let mut h = fixture.harness();
    seventh_observe(&fixture, &mut h);
    let prefix = runtime
        .block_on(super::super::observation::read_history(
            fixture.home.path(),
            run,
        ))
        .unwrap()
        .unwrap();
    h.state.update(&mut h.cx, |state, _| {
        state.run_streams.insert(
            run,
            crate::run_stream::RunStreamState::replay_candidate(run, &prefix[..2]),
        );
        state
            .runs
            .iter_mut()
            .find(|row| row.run_id == run)
            .unwrap()
            .status = RunStatus::Active;
    });
    let peer = EighthRejectPeer::with_response(&runtime, fixture.home.path(), run, true);
    h.state.update(&mut h.cx, |state, _| {
        state.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, cx| {
        app.controlled_history_reads = Some(std::rc::Rc::new(std::cell::RefCell::new(
            std::collections::VecDeque::from([(tx, release_rx)]),
        )));
        app.sync_run_subscriptions(cx);
    });
    let rx = std::cell::RefCell::new(rx);
    drive(&mut h.cx, |_| rx.borrow_mut().try_recv().is_ok());
    let pool = surge_persistence::runs::registry::open_registry_pool(
        fixture.home.path(),
        &surge_persistence::runs::MockClock::new(1),
    )
    .unwrap();
    pool.get().unwrap().execute("UPDATE bootstrap_operations SET intent_fingerprint='0000000000000000000000000000000000000000000000000000000000000000'",[]).unwrap();
    h.app
        .update(&mut h.cx, |app, cx| app.restore_indexed_operations(cx));
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    assert!(h.cx.read(|cx| h.state.read(cx).operation_inspection_reason(op).is_some()));
    pool.get()
        .unwrap()
        .execute(
            "UPDATE bootstrap_operations SET intent_fingerprint=?1",
            [fixture.record.intent_fingerprint.to_string()],
        )
        .unwrap();
    h.cx.run_until_parked();
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(5));
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            !h.app.read(cx).pending_operations.is_empty()
                && h.app.read(cx).offline_operations.is_empty()
        })
    });
    release_tx.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).failure_terminal(run) == Some(surge_core::TerminalReason::Failed)
                && !h.state.read(cx).observations.is_reading(run)
        })
    });
    let serviced = h.cx.read(|cx| {
        h.app.read(cx).pending_operations.is_empty()
            || !h.app.read(cx).offline_operations.is_empty()
    });
    if serviced {
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.app.read(cx).pending_operations.is_empty()
                    && h.app.read(cx).offline_operations.is_empty()
            })
        });
    }
    let drained = h.cx.read(|cx| {
        h.app.read(cx).pending_operations.is_empty() && h.app.read(cx).offline_operations.is_empty()
    });
    peer.close(&runtime);
    drive(&mut h.cx, |cx| {
        cx.read(|cx| !h.state.read(cx).run_streams[&run].live)
    });
    drop(h);
    drop(pool);
    fixture.close();
    assert!(
        drained,
        "record retry recovered; actual live reader settled, so current pending demand must drain without a lost wake"
    );
}

#[test]
fn eighth_inspection_merge_controls_preserve_host_loss_pending_unknown_attempt_and_exclude_foreign_reserved()
 {
    for case in ["confirmed", "pending", "foreign", "unknown", "reserved"] {
        let mut h = Harness::new();
        let project = h.root.path().join("project");
        let foreign = h.root.path().join("foreign");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(&foreign).unwrap();
        h.state.update(&mut h.cx, |state, _| {
            state.project_path = Some(project.clone());
            state.project_scope = Some(crate::project::ProjectScope::resolve(&project));
        });
        let run = RunId::new();
        let op = RunId::new();
        let reserved = RunId::new();
        if case == "unknown" {
            h.state.update(&mut h.cx, |state, _| {
                state.apply_global_event(
                    &surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted {
                        run_id: run,
                    },
                )
            });
        } else if case != "reserved" {
            let path = if case == "foreign" {
                &foreign
            } else {
                &project
            };
            let mut payloads = vec![start(path)];
            if case == "pending" {
                payloads.push(E::HumanInputRequested {
                    node: "impl_1".parse().unwrap(),
                    session: None,
                    call_id: Some("keep-question".into()),
                    prompt: "Keep this question".into(),
                    schema: None,
                });
            }
            h.run(run, payloads, RunStatus::Active);
        }
        if case == "confirmed" {
            h.state.update(&mut h.cx, |state, _| {
                state.run_streams.insert(
                    run,
                    crate::run_stream::RunStreamState::registry_hint(
                        run,
                        surge_core::RunStatus::Crashed,
                        project.clone(),
                    ),
                );
                let scope = state.observation_scope().unwrap();
                let owner = state.observations.claim(&scope, run, false).unwrap();
                let read = state.begin_history_read(&owner).unwrap();
                assert!(
                    state
                        .complete_history_read(&read, Ok(Some(rows(vec![start(&project)]))))
                        .confirmed()
                );
            });
        }
        if case != "reserved" {
            h.state.update(&mut h.cx, |state, _| {
                state
                    .run_streams
                    .get_mut(&run)
                    .unwrap()
                    .apply(&EngineRunEvent::StreamError {
                        message: "actual current inspection error".into(),
                    })
            });
        }
        let current = status(
            op,
            run,
            reserved,
            S::NeedsAttention {
                phase: P::Planning,
                run_id: Some(run),
                reason: A::StorageUnconfirmed,
            },
            None,
            2,
        );
        h.app.update(&mut h.cx, |app, cx| {
            app.update_bootstrap_view(op, current, &project, cx)
        });
        let queue = h.queue();
        let inbox = h.cx.read(|cx| h.app.read(cx).inbox.clone().unwrap());
        let facts = inbox.update(&mut h.cx, |screen, cx| screen.evidence_for_test(cx));
        match case {
            "reserved" | "foreign" => {
                assert_eq!(queue.len(), 1);
                assert!(facts.iter().all(|(_, _, links)| links.is_empty()));
                if case == "reserved" {
                    h.cx.read(|cx| {
                        assert!(h.state.read(cx).runs.is_empty());
                        assert!(h.state.read(cx).run_streams.is_empty());
                    });
                } else {
                    h.cx.read(|cx| assert!(!h.state.read(cx).run_in_project(&run)));
                }
            },
            "unknown" => {
                assert_eq!(queue.len(), 2);
                assert!(facts.iter().any(
                    |(id, _, links)| id == &format!("inspection-{run}") && links == &vec![run]
                ));
                assert!(
                    facts
                        .iter()
                        .find(|(id, _, _)| id == &format!("operation-{op}"))
                        .unwrap()
                        .2
                        .is_empty()
                );
                h.cx.read(|cx| {
                    let state = h.state.read(cx);
                    assert_eq!(
                        state
                            .project_scope
                            .as_ref()
                            .unwrap()
                            .owns(state.run_streams[&run].run_path.as_deref(), None),
                        None
                    );
                });
            },
            _ => {
                assert_eq!(queue.len(), if case == "pending" { 2 } else { 1 });
                let operation = facts
                    .iter()
                    .find(|(id, _, _)| id == &format!("operation-{op}"))
                    .unwrap();
                assert_eq!(operation.2, vec![run]);
                assert!(
                    operation
                        .1
                        .iter()
                        .any(|(_, reason)| reason.contains("actual current inspection error"))
                );
                if case == "confirmed" {
                    assert!(operation.1.iter().any(|(_,reason)|reason.contains("Recorded loss of daemon ownership")));
                } else {
                    h.cx.read(|cx| {
                        assert_eq!(h.state.read(cx).run_streams[&run].pending.len(), 1);
                        assert_eq!(
                            h.state.read(cx).run_streams[&run].pending[0]
                                .call_id()
                                .as_deref(),
                            Some("keep-question")
                        );
                    });
                }
            },
        }
        h.cx.read(|cx| assert_eq!(h.state.read(cx).needs_you_count(), queue.len()));
        drop(inbox);
        drop(h);
    }
}
#[test]
fn eighth_delayed_app_replay_preserves_latest_registry_or_genuine_start_hint() {
    for later_start in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
        let run = fixture.record.status.implementation_run;
        let op = fixture.record.status.operation_id;
        let mut h = fixture.harness();
        let scope = h
            .state
            .update(&mut h.cx, |state, _| state.observation_scope().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        h.app.update(&mut h.cx, |app, cx| {
            app.read_operation_observed(scope, op, cx, async move {
                let _ = tx.send(());
                let _ = release_rx.await;
            })
        });
        let rx = std::cell::RefCell::new(rx);
        drive(&mut h.cx, |_| rx.borrow_mut().try_recv().is_ok());
        runtime
            .block_on(
                fixture
                    .storage
                    .set_run_status(&run, surge_core::RunStatus::Crashed, None),
            )
            .unwrap();
        let summaries = runtime
            .block_on(
                surge_persistence::runs::Storage::inspect_existing_run_summaries(
                    fixture.home.path().into(),
                    std::num::NonZeroU32::new(10).unwrap(),
                ),
            )
            .unwrap();
        let summary = summaries.iter().find(|summary| summary.id == run).unwrap();
        assert_eq!(summary.status, surge_core::RunStatus::Crashed);
        h.state.update(&mut h.cx, |state, _| {
            state.run_streams.insert(
                run,
                crate::run_stream::RunStreamState::registry_hint(
                    run,
                    summary.status,
                    summary.project_path.clone(),
                ),
            );
            if later_start {
                state.apply_global_event(
                    &surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted {
                        run_id: run,
                    },
                );
            }
        });
        release_tx.send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
        });
        h.cx.read(|cx| {
            let stream = &h.state.read(cx).run_streams[&run];
            assert_eq!(
                stream.current_registry_hint(),
                Some(if later_start {
                    surge_core::RunStatus::Running
                } else {
                    surge_core::RunStatus::Crashed
                })
            );
            assert_ne!(
                stream.display(),
                surge_core::run_display::RunDisplayState::Working
            );
            if !later_start {
                assert_eq!(
                    stream.display(),
                    surge_core::run_display::RunDisplayState::Waiting(
                        surge_core::run_display::WaitingReason::RecoveryRequired
                    )
                );
            }
        });
        drop(h);
        fixture.close();
    }
}

#[cfg(unix)]
#[test]
fn ninth_actual_global_acceptance_renews_recovered_nonterminal_fallback() {
    use surge_orchestrator::engine::ipc::{
        DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent,
        read_request_frame, write_frame,
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
    let run = fixture.record.status.implementation_run;
    let mut h = fixture.harness();
    let journal = runtime
        .block_on(super::super::observation::read_history(
            fixture.home.path(),
            run,
        ))
        .unwrap()
        .unwrap();
    h.state.update(&mut h.cx, |s, _| {
        s.runs.push(crate::app_state::UiRun {
            run_id: run,
            status: RunStatus::Active,
            started_at: chrono::Utc::now(),
            last_event_seq: None,
            ended_at: None,
        });
    });
    let socket = fixture.home.path().join("renew.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let (signal, mut signals) = tokio::sync::mpsc::unbounded_channel::<()>();
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = requests.clone();
    let server=runtime.spawn(async move {let(stream,_)=listener.accept().await.unwrap();let(read,mut write)=stream.into_split();let mut read=tokio::io::BufReader::new(read);loop {tokio::select! {
      _=&mut stopped=>return,
      Some(())=signals.recv()=>{write_frame(&mut write,&DaemonEvent::Global(GlobalDaemonEvent::RunAccepted{run_id:run})).await.unwrap();},
      req=read_request_frame(&mut read)=>{match req.unwrap().unwrap(){
        DaemonRequest::SubscribeGlobal{request_id}=>write_frame(&mut write,&DaemonResponse::SubscribeGlobalOk{request_id}).await.unwrap(),
        DaemonRequest::Subscribe{request_id,run_id}=>{assert_eq!(run_id,run);let n=counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst);if n==0 {write_frame(&mut write,&DaemonResponse::Error{request_id,code:ErrorCode::RunNotActive,message:"initial admission ended".into()}).await.unwrap();} else {write_frame(&mut write,&DaemonResponse::SubscribeOk{request_id}).await.unwrap();write_frame(&mut write,&DaemonEvent::PerRun{run_id:run,event:Box::new(EngineRunEvent::StreamError{message:"renewed actual publisher".into()})}).await.unwrap();}},
        other=>panic!("unexpected {other:?}")
      }}
    }}});
    let facade = std::sync::Arc::new(
        runtime
            .block_on(
                surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket),
            )
            .unwrap(),
    );
    let mut global = runtime.block_on(facade.subscribe_global()).unwrap();
    h.state.update(&mut h.cx, |s, _| {
        s.daemon_state = crate::daemon_link::ConnectionState::Connected(facade.clone())
    });
    let scope = h.state.update(&mut h.cx, |s, _| s.observation_scope());
    h.app
        .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state
                .read(cx)
                .run_streams
                .get(&run)
                .is_some_and(|s| s.session_history_confirmed())
        })
    });
    h.cx.read(|cx| {
        let s = h.state.read(cx);
        assert!(s.bootstrap_operations.is_empty());
        assert!(!s.run_streams[&run].live);
        assert!(s.failure_terminal(run).is_none());
        assert!(s.run_streams[&run].session_history_confirmed());
    });
    for _ in 0..5 {
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    }
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    signal.send(()).unwrap();
    let event = runtime.block_on(global.recv()).unwrap();
    h.app.update(&mut h.cx, |app, cx| {
        app.receive_global_event(&facade, &scope, &event, cx)
    });
    for _ in 0..100 {
        h.cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let renewed = requests.load(std::sync::atomic::Ordering::SeqCst);
    let published = h.cx.read(|cx| {
        h.state.read(cx).run_streams[&run]
            .observation_error
            .as_ref()
            .is_some_and(|s| s.contains("renewed actual publisher"))
    });
    let _ = stop.send(());
    runtime.block_on(server).unwrap();
    drop(global);
    drop(facade);
    drop(h);
    assert!(journal.len() >= 2);
    fixture.close();
    assert_eq!(
        renewed, 2,
        "actual RunAccepted must service a second Subscribe after confirmed nonterminal fallback"
    );
    assert!(
        published,
        "renewed SubscribeOk must publish actual per-run events"
    );
}

#[cfg(unix)]
struct NinthPeer {
    facade: std::sync::Arc<surge_orchestrator::engine::daemon_facade::DaemonEngineFacade>,
    global: tokio::sync::broadcast::Receiver<surge_orchestrator::engine::ipc::GlobalDaemonEvent>,
    signal: tokio::sync::mpsc::UnboundedSender<RunId>,
    release: Option<tokio::sync::oneshot::Sender<()>>,
    publish: Option<tokio::sync::oneshot::Sender<()>>,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    sent: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[cfg(unix)]
impl NinthPeer {
    fn new(
        runtime: &tokio::runtime::Runtime,
        home: &std::path::Path,
        run: RunId,
        reject_renewal: bool,
    ) -> Self {
        use surge_orchestrator::engine::ipc::{
            DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent,
            read_request_frame, write_frame,
        };
        let socket = home.join(format!("ninth-{}.sock", RunId::new().short()));
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (signal, mut signals) = tokio::sync::mpsc::unbounded_channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let mut gate = Some(release_rx);
        let (publish, publish_rx) = tokio::sync::oneshot::channel();
        let mut publish_wait = Some(publish_rx);
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = requests.clone();
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sent_frames = sent.clone();
        let server=runtime.spawn(async move {let(stream,_)=listener.accept().await.unwrap();let(read,mut write)=stream.into_split();let mut read=tokio::io::BufReader::new(read);loop {tokio::select! {
          _=&mut stopped=>return,
          Some(run_id)=signals.recv()=>{write_frame(&mut write,&DaemonEvent::Global(GlobalDaemonEvent::RunAccepted{run_id})).await.unwrap();sent_frames.fetch_add(1,std::sync::atomic::Ordering::SeqCst);},
          req=read_request_frame(&mut read)=>{match req.unwrap().unwrap(){
            DaemonRequest::SubscribeGlobal{request_id}=>write_frame(&mut write,&DaemonResponse::SubscribeGlobalOk{request_id}).await.unwrap(),
            DaemonRequest::Subscribe{request_id,run_id}=>{assert_eq!(run_id,run);let n=counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
              if n==1 {for _ in 0..3 {write_frame(&mut write,&DaemonEvent::Global(GlobalDaemonEvent::RunAccepted{run_id})).await.unwrap();}let wait=gate.take().unwrap();tokio::select!{_ = wait=>{},_=&mut stopped=>return}}
              if n==0 || reject_renewal {write_frame(&mut write,&DaemonResponse::Error{request_id,code:ErrorCode::RunNotActive,message:"fixture admission not active".into()}).await.unwrap();} else {write_frame(&mut write,&DaemonResponse::SubscribeOk{request_id}).await.unwrap();let wait=publish_wait.take().unwrap();tokio::select!{_=wait=>{},_=&mut stopped=>return}write_frame(&mut write,&DaemonEvent::PerRun{run_id,event:Box::new(EngineRunEvent::StreamError{message:"ninth current publisher diagnostic".into()})}).await.unwrap();}},
            other=>panic!("unexpected {other:?}")
          }}
        }}});
        let facade = std::sync::Arc::new(
            runtime
                .block_on(
                    surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket),
                )
                .unwrap(),
        );
        let global = runtime.block_on(facade.subscribe_global()).unwrap();
        Self {
            facade,
            global,
            signal,
            release: Some(release),
            publish: Some(publish),
            stop,
            server,
            requests,
            sent,
        }
    }
    fn send(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        h: &mut Harness,
        run: RunId,
        scope: &Option<crate::run_observation::Scope>,
    ) {
        self.signal.send(run).unwrap();
        self.receive(runtime, h, scope);
    }
    fn receive(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        h: &mut Harness,
        scope: &Option<crate::run_observation::Scope>,
    ) {
        let event = runtime.block_on(self.global.recv()).unwrap();
        h.app.update(&mut h.cx, |app, cx| {
            app.receive_global_event(&self.facade, scope, &event, cx)
        });
    }
    fn count(&self) -> usize {
        self.requests.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn close(self, runtime: &tokio::runtime::Runtime) {
        let _ = self.stop.send(());
        runtime.block_on(self.server).unwrap();
        drop(self.facade);
    }
}
#[cfg(unix)]
fn ninth_active(fixture: &SeventhFixture, h: &mut Harness, run: RunId) {
    h.state.update(&mut h.cx, |s, _| {
        s.runs.push(crate::app_state::UiRun {
            run_id: run,
            status: RunStatus::Active,
            started_at: chrono::Utc::now(),
            last_event_seq: None,
            ended_at: None,
        });
        assert_eq!(s.project_path.as_ref(), Some(&fixture.project));
    });
}
#[cfg(unix)]
#[test]
fn ninth_paused_fallback_and_duplicate_actual_admissions_have_one_renewed_publisher() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
    let run = fixture.record.status.implementation_run;
    let mut h = fixture.harness();
    ninth_active(&fixture, &mut h, run);
    let (loaded, loaded_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let (live_loaded, live_loaded_rx) = tokio::sync::oneshot::channel();
    let (live_release, live_release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, _| {
        app.controlled_history_reads = Some(std::rc::Rc::new(std::cell::RefCell::new(
            std::collections::VecDeque::from([
                (loaded, release_rx),
                (live_loaded, live_release_rx),
            ]),
        )))
    });
    let mut peer = NinthPeer::new(&runtime, fixture.home.path(), run, false);
    h.state.update(&mut h.cx, |s, _| {
        s.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
    });
    let scope = h.state.update(&mut h.cx, |s, _| s.observation_scope());
    h.app
        .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    let loaded_rx = std::cell::RefCell::new(loaded_rx);
    drive(&mut h.cx, |_| loaded_rx.borrow_mut().try_recv().is_ok());
    for _ in 0..3 {
        peer.send(&runtime, &mut h, run, &scope);
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    }
    assert_eq!(peer.count(), 1);
    h.cx.read(|cx| assert!(h.state.read(cx).observations.is_reading(run)));
    release.send(()).unwrap();
    drive(&mut h.cx, |_| peer.count() == 2);
    for _ in 0..3 {
        peer.receive(&runtime, &mut h, &scope);
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    }
    assert_eq!(peer.count(), 2);
    peer.release.take().unwrap().send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.state.read(cx).run_streams[&run].live)
    });
    h.cx.read(|cx| {
        assert!(h.app.read(cx).stream_subscribed.contains(&run));
        assert!(!h.app.read(cx).fallback_owners.contains_key(&run));
    });
    assert_eq!(peer.count(), 2);
    peer.publish.take().unwrap().send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).run_streams[&run]
                .observation_error
                .as_ref()
                .is_some_and(|s| s.contains("ninth current publisher"))
        })
    });
    let live_loaded_rx = std::cell::RefCell::new(live_loaded_rx);
    drive(&mut h.cx, |_| {
        live_loaded_rx.borrow_mut().try_recv().is_ok()
    });
    h.cx.read(|cx| assert!(h.state.read(cx).observations.is_reading(run)));
    live_release.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| !h.state.read(cx).observations.is_reading(run))
    });
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(100));
    h.cx.run_until_parked();
    h.cx.read(|cx| {
        let s = h.state.read(cx);
        assert!(
            !s.run_streams[&run].live,
            "actual StreamError marks feed unconfirmed"
        );
        assert!(s.run_streams[&run].session_history_confirmed());
        assert!(
            s.run_streams[&run]
                .observation_error
                .as_ref()
                .unwrap()
                .contains("ninth current publisher")
        );
        assert_eq!(s.rejected_history_reads[&run], 1);
        assert!(s.failure_terminal(run).is_none());
        assert!(
            !h.app.read(cx).stream_subscribed.contains(&run),
            "current transport closure retires only its own subscription"
        );
        assert!(!h.app.read(cx).fallback_owners.contains_key(&run));
    });
    assert_eq!(peer.count(), 2);
    peer.close(&runtime);
    drop(h);
    fixture.close();
}
#[cfg(unix)]
#[test]
fn ninth_terminal_and_exhausted_fallback_require_genuine_current_admission() {
    for terminal in [true, false] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::with_operation(&runtime, terminal, false, false);
        let run = fixture.record.status.implementation_run;
        let obstruction = (!terminal).then(|| fixture.obstruct(run));
        let mut h = fixture.harness();
        ninth_active(&fixture, &mut h, run);
        let mut peer = NinthPeer::new(&runtime, fixture.home.path(), run, !terminal);
        h.state.update(&mut h.cx, |s, _| {
            s.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
        });
        let scope = h.state.update(&mut h.cx, |s, _| s.observation_scope());
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
        let max = if terminal { 1 } else { 4 };
        let mut first_read = None;
        for n in 1..=max {
            drive(&mut h.cx, |cx| {
                cx.read(|cx| {
                    h.state.read(cx).rejected_history_reads.get(&run) == Some(&n)
                        && !h.state.read(cx).observations.is_reading(run)
                })
            });
            if !terminal {
                let current = settled_rejected_read(&h, run, n, first_read.as_ref());
                first_read.get_or_insert(current);
            }
            rejected_retry_clock_step(&mut h.cx, n, max);
        }
        for _ in 0..5 {
            h.state.update(&mut h.cx, |s, _| {
                s.runs.iter_mut().find(|r| r.run_id == run).unwrap().status = RunStatus::Active
            });
            h.app
                .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
        }
        assert_eq!(peer.count(), 1);
        h.cx.read(|cx| assert_eq!(h.state.read(cx).rejected_history_reads[&run], max));
        if let Some(path) = &obstruction {
            assert!(h.cx.read(|cx| {
                h.state
                    .read(cx)
                    .run_inspection_evidence(run)
                    .iter()
                    .any(|(_, s)| s.contains("exhausted"))
            }));
            SeventhFixture::restore(path);
        }
        peer.send(&runtime, &mut h, run, &scope);
        if terminal {
            h.cx.run_until_parked();
            assert_eq!(peer.count(), 1);
            h.cx.read(|cx| {
                assert_eq!(
                    h.state.read(cx).failure_terminal(run),
                    Some(surge_core::TerminalReason::Failed)
                )
            });
        } else {
            drive(&mut h.cx, |_| peer.count() == 2);
            for _ in 0..3 {
                peer.receive(&runtime, &mut h, &scope);
            }
            peer.release.take().unwrap().send(()).unwrap();
            drive(&mut h.cx, |cx| {
                cx.read(|cx| {
                    h.state.read(cx).rejected_history_reads.get(&run) == Some(&5)
                        && !h.state.read(cx).observations.is_reading(run)
                })
            });
            for _ in 0..5 {
                h.app
                    .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
            }
            assert_eq!(peer.count(), 2);
            h.cx.read(|cx| {
                assert!(!h.state.read(cx).run_streams[&run].live);
                assert!(h.state.read(cx).run_streams[&run].session_history_confirmed());
            });
        }
        peer.close(&runtime);
        drop(h);
        fixture.close();
    }
}
#[cfg(unix)]
#[test]
fn ninth_old_connection_and_scope_callbacks_cannot_mutate_state_before_current_admission() {
    for scope_change in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
        let run = fixture.record.status.implementation_run;
        let mut h = fixture.harness();
        ninth_active(&fixture, &mut h, run);
        let mut old = NinthPeer::new(&runtime, fixture.home.path(), run, false);
        let mut current = NinthPeer::new(&runtime, fixture.home.path(), run, false);
        h.state.update(&mut h.cx, |s, _| {
            s.daemon_state = crate::daemon_link::ConnectionState::Connected(old.facade.clone())
        });
        let old_scope = h.state.update(&mut h.cx, |s, _| s.observation_scope());
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.state
                    .read(cx)
                    .run_streams
                    .get(&run)
                    .is_some_and(|s| s.session_history_confirmed())
            })
        });
        if scope_change {
            h.state.update(&mut h.cx, |s, _| {
                s.project_path = Some(fixture.project.join("other"));
                s.observation_scope();
                s.project_path = Some(fixture.project.clone());
                s.observation_scope();
            });
        } else {
            h.state.update(&mut h.cx, |s, _| {
                s.daemon_state =
                    crate::daemon_link::ConnectionState::Connected(current.facade.clone())
            });
        }
        let before =
            h.cx.read(|cx| h.state.read(cx).run_streams[&run].current_registry_hint());
        let absent = RunId::new();
        old.send(&runtime, &mut h, run, &old_scope);
        old.send(&runtime, &mut h, absent, &old_scope);
        h.cx.read(|cx| {
            let s = h.state.read(cx);
            assert_eq!(s.run_streams[&run].current_registry_hint(), before);
            assert!(!s.runs.iter().any(|r| r.run_id == absent));
            assert!(!s.run_streams.contains_key(&absent));
        });
        assert_eq!(old.count(), 1);
        let scope = h.state.update(&mut h.cx, |s, _| s.observation_scope());
        let obsolete_facade = old.facade.clone();
        let peer = if scope_change { &mut old } else { &mut current };
        peer.send(&runtime, &mut h, run, &scope);
        if !scope_change {
            drive(&mut h.cx, |cx| {
                cx.read(|cx| {
                    h.state.read(cx).rejected_history_reads.get(&run) == Some(&2)
                        && !h.state.read(cx).observations.is_reading(run)
                })
            });
            peer.send(&runtime, &mut h, run, &scope);
        }
        drive(&mut h.cx, |_| peer.count() == 2);
        for _ in 0..3 {
            peer.receive(&runtime, &mut h, &scope);
        }
        // An obsolete actual event must not steal the current pending Subscribe marker.
        let pending_facade = peer.facade.clone();
        let obsolete_event =
            surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted { run_id: absent };
        h.app.update(&mut h.cx, |app, cx| {
            app.receive_global_event(&obsolete_facade, &old_scope, &obsolete_event, cx)
        });
        h.cx.read(|cx| {
            assert!(h.app.read(cx).stream_subscribed.contains(&run));
            assert!(
                h.state
                    .read(cx)
                    .daemon_state
                    .facade()
                    .is_some_and(|f| std::sync::Arc::ptr_eq(&f, &pending_facade))
            );
        });
        peer.release.take().unwrap().send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.state.read(cx).run_streams[&run].live)
        });
        assert_eq!(peer.count(), 2);
        peer.publish.take().unwrap().send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.state.read(cx).run_streams[&run]
                    .observation_error
                    .as_ref()
                    .is_some_and(|s| s.contains("ninth current publisher"))
            })
        });
        drive(&mut h.cx, |cx| {
            cx.read(|cx| !h.app.read(cx).stream_subscribed.contains(&run))
        });
        old.send(&runtime, &mut h, absent, &old_scope);
        h.cx.background_executor
            .advance_clock(std::time::Duration::from_secs(100));
        h.cx.run_until_parked();
        h.cx.read(|cx| {
            let s = h.state.read(cx);
            assert!(
                !s.run_streams[&run].live,
                "actual StreamError marks feed unconfirmed"
            );
            assert!(
                s.run_streams[&run]
                    .observation_error
                    .as_ref()
                    .unwrap()
                    .contains("ninth current publisher")
            );
            assert!(!s.runs.iter().any(|r| r.run_id == absent));
            assert!(!h.app.read(cx).stream_subscribed.contains(&run));
        });
        old.close(&runtime);
        current.close(&runtime);
        drop(h);
        fixture.close();
    }
}

#[cfg(unix)]
#[test]
fn ninth_same_global_receiver_survives_project_selection_and_current_eof() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
    let run = fixture.record.status.implementation_run;
    let mut h = fixture.harness();
    h.state.update(&mut h.cx, |s, _| {
        s.project_path = None;
        s.observation_scope();
    });
    let mut peer = NinthPeer::new(&runtime, fixture.home.path(), run, false);
    h.state.update(&mut h.cx, |s, _| {
        s.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
    });
    let rx = std::mem::replace(&mut peer.global, tokio::sync::broadcast::channel(1).1);
    h.app.update(&mut h.cx, |app, cx| {
        app.watch_global_events(peer.facade.clone(), rx, false, cx)
            .detach()
    });
    h.cx.run_until_parked();
    h.state.update(&mut h.cx, |s, _| {
        s.project_path = Some(fixture.project.clone())
    });
    peer.signal.send(run).unwrap();
    for _ in 0..100 {
        h.cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let processed =
        h.cx.read(|cx| h.state.read(cx).runs.iter().any(|r| r.run_id == run));
    if processed {
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.state.read(cx).rejected_history_reads.get(&run) == Some(&1)
                    && !h.state.read(cx).observations.is_reading(run)
            })
        });
        peer.release.take().unwrap().send(()).unwrap();
        peer.publish.take().unwrap().send(()).unwrap();
        for project in [fixture.project.join("other"), fixture.project.clone()] {
            h.state.update(&mut h.cx, |state, _| {
                state.project_path = Some(project);
                state.observation_scope();
                state.run_streams.insert(
                    run,
                    crate::run_stream::RunStreamState::registry_hint(
                        run,
                        surge_core::RunStatus::Crashed,
                        fixture.project.clone(),
                    ),
                );
            });
            peer.signal.send(run).unwrap();
            drive(&mut h.cx, |cx| {
                cx.read(|cx| {
                    h.state.read(cx).run_streams[&run].current_registry_hint()
                        == Some(surge_core::RunStatus::Running)
                })
            });
        }
    }
    peer.close(&runtime);
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            matches!(
                h.state.read(cx).daemon_state,
                crate::daemon_link::ConnectionState::Disconnected
            )
        })
    });
    drop(h);
    fixture.close();
    assert!(
        processed,
        "same actual receiver connected before project selection must process current RunAccepted"
    );
}

#[cfg(unix)]
#[test]
fn ninth_replaced_actual_global_receiver_eof_cannot_disconnect_current_connection() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
    let run = fixture.record.status.implementation_run;
    let mut h = fixture.harness();
    let mut old = NinthPeer::new(&runtime, fixture.home.path(), run, false);
    let mut current = NinthPeer::new(&runtime, fixture.home.path(), run, false);
    let mut tasks = Vec::new();
    for peer in [&mut old, &mut current] {
        h.state.update(&mut h.cx, |s, _| {
            s.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
        });
        let rx = std::mem::replace(&mut peer.global, tokio::sync::broadcast::channel(1).1);
        let task = h.app.update(&mut h.cx, |app, cx| {
            app.watch_global_events(peer.facade.clone(), rx, false, cx)
        });
        let (done, finished) = tokio::sync::oneshot::channel();
        h.app.update(&mut h.cx, |_, cx| {
            cx.spawn(
                async move |_: gpui_kit::WeakEntity<SurgeApp>, _: &mut gpui_kit::AsyncApp| {
                    task.await;
                    let _ = done.send(());
                },
            )
            .detach()
        });
        tasks.push(std::cell::RefCell::new(finished));
        h.cx.run_until_parked();
    }
    let absent = RunId::new();
    old.signal.send(absent).unwrap();
    drive(&mut h.cx, |_| {
        old.sent.load(std::sync::atomic::Ordering::SeqCst) == 1
    });
    old.close(&runtime);
    drive(&mut h.cx, |_| tasks[0].borrow_mut().try_recv().is_ok());
    h.cx.read(|cx| {
        assert!(!h.state.read(cx).runs.iter().any(|r| r.run_id == absent));
        assert!(
            h.state
                .read(cx)
                .daemon_state
                .facade()
                .is_some_and(|facade| std::sync::Arc::ptr_eq(&facade, &current.facade))
        );
    });
    current.close(&runtime);
    drive(&mut h.cx, |_| tasks[1].borrow_mut().try_recv().is_ok());
    h.cx.read(|cx| {
        assert!(matches!(
            h.state.read(cx).daemon_state,
            crate::daemon_link::ConnectionState::Disconnected
        ))
    });
    drop(h);
    fixture.close();
}

#[cfg(unix)]
#[test]
fn tenth_actual_related_read_settlement_services_pending_live_renewal_without_poll() {
    tenth_related_settlement(0);
}
#[cfg(unix)]
fn tenth_related_settlement(mode: u8) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
    let run = fixture.record.status.implementation_run;
    let op = fixture.record.status.operation_id;
    let obstruction = fixture.obstruct(run);
    let mut h = fixture.harness();
    ninth_active(&fixture, &mut h, run);
    let mut peer = NinthPeer::new(&runtime, fixture.home.path(), run, false);
    h.state.update(&mut h.cx, |s, _| {
        s.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
    });
    let rx = std::mem::replace(&mut peer.global, tokio::sync::broadcast::channel(1).1);
    h.app.update(&mut h.cx, |app, cx| {
        app.watch_global_events(peer.facade.clone(), rx, false, cx)
            .detach()
    });
    h.app
        .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).rejected_history_reads.get(&run) == Some(&1)
                && !h.state.read(cx).observations.is_reading(run)
        })
    });
    assert_eq!(peer.count(), 1);
    SeventhFixture::restore(&obstruction);
    let mut missing = false;
    let mut child_obstruction = None;
    let mut parent_obstruction = None;
    if matches!(mode, 1 | 2 | 3 | 9) {
        let path = fixture.obstruct(run);
        if mode != 1 && mode != 9 {
            std::fs::remove_dir(&path).unwrap();
            missing = true;
        }
        child_obstruction = Some(path);
    }
    if mode == 1 || mode == 9 {
        parent_obstruction = Some(fixture.obstruct(fixture.record.status.planning_run));
    }
    if mode == 7 {
        runtime.block_on(async {
            let writer = fixture.storage.open_run_writer(run).await.unwrap();
            writer
                .append_events(vec![surge_core::VersionedEventPayload::new(failed())])
                .await
                .unwrap();
            writer.close().await.unwrap();
        });
    }
    let store = fixture.storage.bootstrap_operation_store();
    let record = if matches!(mode, 4 | 5) {
        store
            .block(op, fixture.record.status.revision, A::StorageUnconfirmed)
            .unwrap()
    } else {
        store
            .settle(
                op,
                fixture.record.status.revision,
                &T::Failed {
                    run_id: run,
                    reason: if mode == 3 || mode == 7 {
                        F::RunFailed
                    } else {
                        F::BudgetExhausted
                    },
                },
            )
            .unwrap()
    };
    let (loaded, loaded_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, cx| {
        let reads = Some(std::rc::Rc::new(std::cell::RefCell::new(
            std::collections::VecDeque::from([(loaded, release_rx)]),
        )));
        if mode == 5 {
            app.controlled_operation_rechecks = reads;
        } else {
            app.controlled_operation_reads = reads;
        }
        app.update_bootstrap_view(op, record.status.clone(), &fixture.project, cx);
    });
    let loaded_rx = std::cell::RefCell::new(loaded_rx);
    drive(&mut h.cx, |_| loaded_rx.borrow_mut().try_recv().is_ok());
    h.cx.read(|cx| {
        assert!(h.state.read(cx).observations.is_reading(run));
        assert_eq!(h.state.read(cx).related_history_reads[&run], 1);
    });
    if mode != 8 && mode != 9 {
        peer.signal.send(run).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.app
                    .read(cx)
                    .fallback_owners
                    .get(&run)
                    .is_some_and(|owner| owner.renewed)
            })
        });
    }
    assert_eq!(peer.count(), 1);
    if matches!(mode, 4 | 5) {
        let newer = if mode == 4 {
            store.request_cancel(op).unwrap()
        } else {
            store
                .settle(
                    op,
                    record.status.revision,
                    &T::Failed {
                        run_id: run,
                        reason: F::BudgetExhausted,
                    },
                )
                .unwrap()
        };
        if mode == 4 {
            h.app.update(&mut h.cx, |app, cx| {
                app.update_bootstrap_view(op, newer.status, &fixture.project, cx)
            });
        }
    } else if mode == 0 {
        // Equal pending demand must not take this owner before renewal settlement.
        h.app.update(&mut h.cx, |app, cx| {
            app.update_bootstrap_view(op, record.status.clone(), &fixture.project, cx)
        });
    }
    release.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    for _ in 0..100 {
        h.cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let renewed = peer.count();
    h.cx.read(|cx| {
        let expected = if matches!(mode, 4 | 5) {
            crate::app_state::ReadDisposition::Retired
        } else if matches!(mode, 1 | 3 | 9) {
            crate::app_state::ReadDisposition::Incomplete
        } else if mode == 2 {
            crate::app_state::ReadDisposition::Absent
        } else {
            crate::app_state::ReadDisposition::Confirmed
        };
        assert!(
            h.app
                .read(cx)
                .current_read_settlements
                .iter()
                .any(|(token, kind)| token.run == run && *kind == expected)
        );
    });
    if let Some(path) = child_obstruction.take() {
        if missing {
            std::fs::create_dir(&path).unwrap();
        }
        SeventhFixture::restore(&path);
    }
    if mode == 1 || mode == 9 {
        h.cx.read(|cx| {
            let state = h.state.read(cx);
            assert!(state.operation_inspection_reason(op).is_some());
            let app = h.app.read(cx);
            assert!(app.offline_retries.values().any(|episode| {
                episode
                    .eligible
                    .iter()
                    .any(|token| token.run == fixture.record.status.planning_run)
                    && episode.eligible.iter().any(|token| token.run == run) == (mode == 9)
            }));
        });
    }
    if mode == 5 {
        h.cx.read(|cx| {
            assert!(h.state.read(cx).operation_inspection_reason(op).is_some());
            assert!(!h.state.read(cx).run_streams[&run].session_history_confirmed());
        });
    }
    if renewed == 2 {
        peer.release.take().unwrap().send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.state.read(cx).run_streams[&run].live)
        });
        peer.publish.take().unwrap().send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| !h.app.read(cx).stream_subscribed.contains(&run))
        });
    }
    if let Some(path) = parent_obstruction.take() {
        SeventhFixture::restore(&path);
    }
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(100));
    h.cx.run_until_parked();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.app.read(cx).offline_operations.is_empty()
                && !h.state.read(cx).observations.is_reading(run)
                && !h
                    .state
                    .read(cx)
                    .observations
                    .is_reading(fixture.record.status.planning_run)
        })
    });
    h.cx.read(|cx| {
        let s = h.state.read(cx);
        assert_eq!(s.rejected_history_reads[&run], 1);
        assert!(s.run_streams[&run].session_history_confirmed());
        assert_eq!(
            s.failure_terminal(run),
            if mode == 7 {
                Some(surge_core::TerminalReason::Failed)
            } else {
                None
            }
        );
        assert!(!s.observations.is_reading(run));
    });
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.app.read(cx).offline_operations.is_empty()
                && !h
                    .state
                    .read(cx)
                    .observations
                    .is_reading(fixture.record.status.planning_run)
        })
    });
    peer.close(&runtime);
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            matches!(
                h.state.read(cx).daemon_state,
                crate::daemon_link::ConnectionState::Disconnected
            )
        })
    });
    drop(store);
    drop(h);
    fixture.close();
    assert_eq!(
        renewed,
        if matches!(mode, 7..=9) { 1 } else { 2 },
        "actual related read completion must service already pending renewal without another sync/poll/global event"
    );
}

#[cfg(unix)]
#[test]
fn tenth_related_error_absence_retirements_and_terminal_share_settlement_ordering() {
    for mode in [1, 2, 3, 4, 5, 7, 8, 9] {
        tenth_related_settlement(mode);
    }
}

#[cfg(unix)]
#[test]
fn tenth_cold_and_live_helpers_share_current_settlement_and_reject_obsolete_completion() {
    for obsolete in [false, true] {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
        let run = RunId::new();
        let recorded_project = fixture.project.canonicalize().unwrap();
        runtime.block_on(async {
            let writer = fixture
                .storage
                .create_run(run, &recorded_project, None)
                .await
                .unwrap();
            writer
                .append_events(vec![surge_core::VersionedEventPayload::new(start(
                    &recorded_project,
                ))])
                .await
                .unwrap();
            writer.close().await.unwrap();
        });
        let obstruction = fixture.obstruct(run);
        let mut h = fixture.harness();
        ninth_active(&fixture, &mut h, run);
        let mut peer = NinthPeer::new(&runtime, fixture.home.path(), run, false);
        h.state.update(&mut h.cx, |s, _| {
            s.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
        });
        let rx = std::mem::replace(&mut peer.global, tokio::sync::broadcast::channel(1).1);
        h.app.update(&mut h.cx, |app, cx| {
            app.watch_global_events(peer.facade.clone(), rx, false, cx)
                .detach()
        });
        h.app
            .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.state.read(cx).rejected_history_reads.get(&run) == Some(&1)
                    && !h.state.read(cx).observations.is_reading(run)
            })
        });
        SeventhFixture::restore(&obstruction);
        let summary = runtime
            .block_on(
                surge_persistence::runs::Storage::inspect_existing_run_summaries(
                    fixture.home.path().into(),
                    std::num::NonZeroU32::new(10).unwrap(),
                ),
            )
            .unwrap()
            .into_iter()
            .find(|summary| summary.id == run)
            .unwrap();
        let actual_scope = h
            .state
            .update(&mut h.cx, |state, _| state.observation_scope().unwrap());
        assert_eq!(
            summary.project_path, actual_scope.project,
            "cold fixture must meet real production project scope guard"
        );
        let (loaded, loaded_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let mut release = Some(release);
        let (done, done_rx) = tokio::sync::oneshot::channel();
        let state = h.state.downgrade();
        h.app.update(&mut h.cx, |_, cx| {
            cx.spawn(
                async move |app: gpui_kit::WeakEntity<SurgeApp>, cx: &mut gpui_kit::AsyncApp| {
                    super::super::hydrate_cold_summary(&app, &state, summary, cx, async move {
                        let _ = loaded.send(());
                        let _ = release_rx.await;
                    })
                    .await;
                    let _ = done.send(());
                },
            )
            .detach()
        });
        let loaded_rx = std::cell::RefCell::new(loaded_rx);
        eprintln!("cold obsolete={obsolete} await actual cold read loaded");
        drive(&mut h.cx, |_| loaded_rx.borrow_mut().try_recv().is_ok());
        h.cx.read(|cx| assert!(h.state.read(cx).observations.is_reading(run)));
        let (live_loaded, live_loaded_rx) = tokio::sync::oneshot::channel();
        let (live_release, live_release_rx) = tokio::sync::oneshot::channel();
        h.app.update(&mut h.cx, |app, _| {
            app.controlled_history_reads = Some(std::rc::Rc::new(std::cell::RefCell::new(
                std::collections::VecDeque::from([(live_loaded, live_release_rx)]),
            )))
        });
        peer.signal.send(run).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.app
                    .read(cx)
                    .fallback_owners
                    .get(&run)
                    .is_some_and(|owner| owner.renewed)
            })
        });
        assert_eq!(peer.count(), 1);
        if obsolete {
            h.state.update(&mut h.cx, |s, _| {
                s.project_path = Some(fixture.project.join("other"));
                s.observation_scope();
                s.project_path = Some(fixture.project.clone());
                s.observation_scope();
            });
            peer.signal.send(run).unwrap();
        } else {
            release.take().unwrap().send(()).unwrap();
        }
        eprintln!("cold obsolete={obsolete} await renewed Subscribe");
        drive(&mut h.cx, |_| peer.count() == 2);
        peer.release.take().unwrap().send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| h.state.read(cx).run_streams[&run].live)
        });
        assert_eq!(peer.count(), 2);
        peer.publish.take().unwrap().send(()).unwrap();
        let live_loaded_rx = std::cell::RefCell::new(live_loaded_rx);
        drive(&mut h.cx, |_| {
            live_loaded_rx.borrow_mut().try_recv().is_ok()
        });
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                h.state.read(cx).run_streams[&run]
                    .observation_error
                    .as_ref()
                    .is_some_and(|error| error.contains("ninth current publisher"))
            })
        });
        if obsolete {
            release.take().unwrap().send(()).unwrap();
        }
        let done_rx = std::cell::RefCell::new(done_rx);
        eprintln!("cold obsolete={obsolete} await cold producer settled");
        drive(&mut h.cx, |_| done_rx.borrow_mut().try_recv().is_ok());
        h.cx.read(|cx| {
            let s = h.state.read(cx);
            assert!(s.observations.is_reading(run));
            assert!(h.app.read(cx).stream_subscribed.contains(&run));
            assert!(
                s.run_streams[&run]
                    .observation_error
                    .as_ref()
                    .unwrap()
                    .contains("ninth current publisher")
            );
            let confirmed = h
                .app
                .read(cx)
                .current_read_settlements
                .iter()
                .filter(|(token, kind)| {
                    token.run == run && *kind == crate::app_state::ReadDisposition::Confirmed
                })
                .count();
            assert_eq!(confirmed, usize::from(!obsolete));
        });
        live_release.send(()).unwrap();
        drive(&mut h.cx, |cx| {
            cx.read(|cx| !h.app.read(cx).stream_subscribed.contains(&run))
        });
        h.cx.background_executor
            .advance_clock(std::time::Duration::from_secs(100));
        h.cx.run_until_parked();
        h.cx.read(|cx| {
            let s = h.state.read(cx);
            assert!(s.run_streams[&run].session_history_confirmed());
            assert!(!s.run_streams[&run].live);
            assert!(
                s.run_streams[&run]
                    .observation_error
                    .as_ref()
                    .unwrap()
                    .contains("ninth current publisher")
            );
            assert_eq!(s.rejected_history_reads[&run], 1);
            let count = h
                .app
                .read(cx)
                .current_read_settlements
                .iter()
                .filter(|(token, kind)| {
                    token.run == run && *kind == crate::app_state::ReadDisposition::Confirmed
                })
                .count();
            assert_eq!(count, if obsolete { 1 } else { 2 });
        });
        peer.close(&runtime);
        drive(&mut h.cx, |cx| {
            cx.read(|cx| {
                matches!(
                    h.state.read(cx).daemon_state,
                    crate::daemon_link::ConnectionState::Disconnected
                )
            })
        });
        drop(h);
        fixture.close();
    }
}

#[cfg(unix)]
#[test]
fn tenth_obsolete_checked_retirements_cannot_touch_new_live_read_error_or_marker() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let fixture = SeventhFixture::with_operation(&runtime, false, false, false);
    let run = fixture.record.status.implementation_run;
    let op = fixture.record.status.operation_id;
    let obstruction = fixture.obstruct(run);
    let mut h = fixture.harness();
    ninth_active(&fixture, &mut h, run);
    let mut peer = NinthPeer::new(&runtime, fixture.home.path(), run, false);
    h.state.update(&mut h.cx, |s, _| {
        s.daemon_state = crate::daemon_link::ConnectionState::Connected(peer.facade.clone())
    });
    let rx = std::mem::replace(&mut peer.global, tokio::sync::broadcast::channel(1).1);
    h.app.update(&mut h.cx, |app, cx| {
        app.watch_global_events(peer.facade.clone(), rx, false, cx)
            .detach()
    });
    h.app
        .update(&mut h.cx, |app, cx| app.sync_run_subscriptions(cx));
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).rejected_history_reads.get(&run) == Some(&1)
                && !h.state.read(cx).observations.is_reading(run)
        })
    });
    SeventhFixture::restore(&obstruction);
    let store = fixture.storage.bootstrap_operation_store();
    let record = store
        .block(op, fixture.record.status.revision, A::StorageUnconfirmed)
        .unwrap();
    let (loaded, loaded_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, cx| {
        app.controlled_operation_rechecks = Some(std::rc::Rc::new(std::cell::RefCell::new(
            std::collections::VecDeque::from([(loaded, release_rx)]),
        )));
        app.update_bootstrap_view(op, record.status.clone(), &fixture.project, cx);
    });
    let loaded_rx = std::cell::RefCell::new(loaded_rx);
    drive(&mut h.cx, |_| loaded_rx.borrow_mut().try_recv().is_ok());
    h.cx.read(|cx| assert!(h.state.read(cx).observations.is_reading(run)));
    peer.signal.send(run).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.app
                .read(cx)
                .fallback_owners
                .get(&run)
                .is_some_and(|owner| owner.renewed)
        })
    });
    assert_eq!(peer.count(), 1);
    let (live_loaded, live_loaded_rx) = tokio::sync::oneshot::channel();
    let (live_release, live_release_rx) = tokio::sync::oneshot::channel();
    h.app.update(&mut h.cx, |app, _| {
        app.controlled_history_reads = Some(std::rc::Rc::new(std::cell::RefCell::new(
            std::collections::VecDeque::from([(live_loaded, live_release_rx)]),
        )))
    });
    h.state.update(&mut h.cx, |s, _| {
        s.project_path = Some(fixture.project.join("other"));
        s.observation_scope();
        s.project_path = Some(fixture.project.clone());
        s.observation_scope();
    });
    peer.signal.send(run).unwrap();
    drive(&mut h.cx, |_| peer.count() == 2);
    peer.release.take().unwrap().send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.state.read(cx).run_streams[&run].live)
    });
    peer.publish.take().unwrap().send(()).unwrap();
    let live_loaded_rx = std::cell::RefCell::new(live_loaded_rx);
    drive(&mut h.cx, |_| {
        live_loaded_rx.borrow_mut().try_recv().is_ok()
    });
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            h.state.read(cx).run_streams[&run]
                .observation_error
                .as_ref()
                .is_some_and(|reason| reason.contains("ninth current publisher"))
        })
    });
    store
        .settle(
            op,
            record.status.revision,
            &T::Failed {
                run_id: run,
                reason: F::BudgetExhausted,
            },
        )
        .unwrap();
    release.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| h.app.read(cx).offline_operations.is_empty())
    });
    h.cx.read(|cx| {
        let s = h.state.read(cx);
        assert!(s.observations.is_reading(run));
        assert!(h.app.read(cx).stream_subscribed.contains(&run));
        assert!(
            s.run_streams[&run]
                .observation_error
                .as_ref()
                .unwrap()
                .contains("ninth current publisher")
        );
        assert!(s.operation_inspection_reason(op).is_none());
        assert!(
            h.app
                .read(cx)
                .current_read_settlements
                .iter()
                .all(|(_, kind)| *kind != crate::app_state::ReadDisposition::Retired)
        );
        assert_eq!(
            h.app.read(cx).bootstrap_operations[&op].1.revision,
            record.status.revision
        );
    });
    live_release.send(()).unwrap();
    drive(&mut h.cx, |cx| {
        cx.read(|cx| !h.app.read(cx).stream_subscribed.contains(&run))
    });
    h.cx.background_executor
        .advance_clock(std::time::Duration::from_secs(100));
    h.cx.run_until_parked();
    h.cx.read(|cx| {
        let s = h.state.read(cx);
        assert_eq!(s.rejected_history_reads[&run], 1);
        assert!(s.run_streams[&run].session_history_confirmed());
        assert!(
            s.run_streams[&run]
                .observation_error
                .as_ref()
                .unwrap()
                .contains("ninth current publisher")
        );
        assert!(s.failure_terminal(run).is_none());
    });
    assert_eq!(peer.count(), 2);
    peer.close(&runtime);
    drive(&mut h.cx, |cx| {
        cx.read(|cx| {
            matches!(
                h.state.read(cx).daemon_state,
                crate::daemon_link::ConnectionState::Disconnected
            )
        })
    });
    drop(store);
    drop(h);
    fixture.close();
}
