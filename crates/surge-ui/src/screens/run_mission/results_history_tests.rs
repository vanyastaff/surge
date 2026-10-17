use super::{
    BootstrapStage, ImplementationHistory, LoadingState, MissionPanel, MissionRequest, PlanState,
    RunId,
};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use surge_core::bootstrap_operation::{
    BootstrapCapture, BootstrapCaptureFields, BootstrapContentRef, BootstrapIntent,
};
use surge_core::{ContentHash, EventPayload, VersionedEventPayload};
use surge_persistence::runs::{Storage, bootstrap_operations::BootstrapOperationRecord};

struct Fixture {
    runtime: tokio::runtime::Runtime,
    active_loads: std::rc::Rc<std::cell::Cell<usize>>,
    home: crate::runtime_home_fixture::FixtureHome,
    storage: std::sync::Arc<Storage>,
    record: BootstrapOperationRecord,
    graph: surge_core::graph::Graph,
}

#[derive(Clone, Copy)]
enum ProjectIdentity {
    Raw,
    Canonical,
}
#[derive(Clone, Copy)]
enum ScreenJourney {
    Baseline,
    GeneratedGraphs,
    CanonicalAlias,
}

impl Fixture {
    fn new() -> Self {
        Self::with_project_identity(ProjectIdentity::Raw)
    }

    fn with_project_identity(identity: ProjectIdentity) -> Self {
        let fixture = Self::prepare(identity);
        fixture.write_run(false);
        fixture
    }

    fn prepare(identity: ProjectIdentity) -> Self {
        Self::prepare_with_graph(
            identity,
            toml::from_str(include_str!("../../../testdata/nested_loops_flow.toml")).unwrap(),
        )
    }

    fn prepare_with_graph(identity: ProjectIdentity, graph: surge_core::graph::Graph) -> Self {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let repository = match identity {
            ProjectIdentity::Raw => home.path().to_path_buf(),
            ProjectIdentity::Canonical => home.path().canonicalize().unwrap(),
        };
        let reference = BootstrapContentRef {
            name: "bootstrap".into(),
            digest: graph.identity().unwrap(),
        };
        let capture: BootstrapCapture = BootstrapCaptureFields {
            version: 1,
            planning_run: RunId::new(),
            implementation_run: RunId::new(),
            repository: repository.clone(),
            git_common_dir: repository.join(".git"),
            base_commit: "a".repeat(40),
            planning_worktree: repository.join("planning"),
            implementation_worktree: repository.join("implementation"),
            planning_branch: "surge/planning".into(),
            implementation_branch: "surge/implementation".into(),
            graph: reference,
            project_context: None,
            bootstrap_edit_loop_cap: None,
            runtime_identity: ContentHash::compute(b"runtime"),
            configuration: Vec::new(),
            credential_refs: Vec::new(),
        }
        .try_into()
        .unwrap();
        let storage = runtime.block_on(Storage::open(home.path())).unwrap();
        let store = storage.bootstrap_operation_store();
        let record = store
            .insert_if_absent(
                RunId::new(),
                &BootstrapIntent::new(repository, "Build timer".into(), Default::default())
                    .unwrap(),
                &capture,
            )
            .unwrap();
        let record = store
            .claim(record.status.operation_id, record.status.revision)
            .unwrap();
        let record = store
            .mark_started(record.status.operation_id, record.status.revision)
            .unwrap();
        Self {
            runtime,
            active_loads: Default::default(),
            home,
            storage,
            record,
            graph,
        }
    }

    fn write_run(&self, child: bool) {
        self.write_events(child, self.initial_events(child));
    }

    fn initial_events(&self, child: bool) -> Vec<EventPayload> {
        use surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload;
        let BootstrapStoredPayload::V1 { capture, .. } = &self.record.payload else {
            panic!("supported fixture");
        };
        let path = if child {
            &capture.fields().implementation_worktree
        } else {
            &capture.fields().planning_worktree
        };
        let mut events = vec![
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: path.clone(),
                initial_prompt: "Build timer".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    bootstrap_edit_loop_cap: None,
                },
            },
            EventPayload::PipelineMaterialized {
                graph: Box::new(self.graph.clone()),
                graph_hash: self.graph.identity().unwrap(),
            },
        ];
        if child {
            events.push(EventPayload::LoopIterationStarted {
                loop_id: "milestone_loop".parse().unwrap(),
                item: toml::from_str("id='m1'\ntitle='Timer'\ntasks=[]").unwrap(),
                index: 0,
            });
        } else {
            events.push(EventPayload::BootstrapStageStarted {
                stage: BootstrapStage::Description,
            });
            events.push(EventPayload::BootstrapApprovalRequested {
                stage: BootstrapStage::Description,
                channel: surge_core::approvals::ApprovalChannel::Email {
                    to_ref: "reviewer".into(),
                },
            });
        }
        events
    }

    fn write_events(&self, child: bool, events: Vec<EventPayload>) {
        use surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload;
        let BootstrapStoredPayload::V1 { capture, .. } = &self.record.payload else {
            panic!("supported fixture");
        };
        let (run, path) = if child {
            (
                self.record.status.implementation_run,
                &capture.fields().implementation_worktree,
            )
        } else {
            (
                self.record.status.planning_run,
                &capture.fields().planning_worktree,
            )
        };
        self.runtime.block_on(async {
            let writer = self.storage.create_run(run, path, None).await.unwrap();
            writer
                .append_events(events.into_iter().map(VersionedEventPayload::new).collect())
                .await
                .unwrap();
            writer.close().await.unwrap();
        });
    }

    fn commit_child(&mut self) {
        let child = surge_core::bootstrap_continuation::BootstrapContinuation::new(
            self.record.status.planning_run,
            BootstrapContentRef {
                name: "implementation".into(),
                digest: self.graph.identity().unwrap(),
            },
            ["description", "roadmap", "flow"]
                .into_iter()
                .map(|name| surge_core::run_state::ArtifactRef {
                    hash: ContentHash::compute(name.as_bytes()),
                    path: std::path::PathBuf::from(name),
                    name: name.into(),
                    produced_by: "author".parse().unwrap(),
                    produced_at_seq: 4,
                })
                .collect(),
            Default::default(),
        )
        .unwrap();
        self.record = self
            .storage
            .bootstrap_operation_store()
            .continue_planning(
                self.record.status.operation_id,
                self.record.status.revision,
                &child,
            )
            .unwrap();
    }

    fn panel(&self, cx: &mut TestAppContext) -> Entity<MissionPanel> {
        // GPUI supports parking for real external I/O; SQLite runs on Tokio blocking threads.
        cx.background_executor.allow_parking();
        cx.update(gpui_kit::init);
        cx.new(|cx| {
            let mut panel = MissionPanel {
                controlled_loads: None,
                active_loads: self.active_loads.clone(),
                home: Some(self.home.path().into()),
                request: MissionRequest {
                    selected: self.record.status.planning_run,
                    project: Some(self.home.path().into()),
                },
                mission: None,
                implementation: None,
                operation: None,
                loading: LoadingState::Reading,
                error: None,
                generation: 0,
                loaded_refresh: None,
                scope: Vec::new(),
                requested_task: None,
                expanded_steps: Default::default(),
            };
            panel.reload(cx);
            panel
        })
    }

    fn settle(&self, panel: &Entity<MissionPanel>, cx: &mut gpui_kit::VisualTestContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while cx.update(|_, cx| {
            let panel = panel.read(cx);
            panel.loading == LoadingState::Reading || panel.active_loads.get() != 0
        }) {
            assert!(
                std::time::Instant::now() < deadline,
                "actual async Results load did not settle"
            );
            cx.run_until_parked();
            self.runtime.block_on(tokio::task::yield_now());
        }
        cx.run_until_parked();
    }

    fn settle_screen(
        &self,
        screen: &Entity<super::super::RunsScreen>,
        window: &mut gpui_kit::VisualTestContext,
        expected: ImplementationHistory,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let ready = window.update(|window, cx| {
                // Render the actual screen: its ordinary Overview path constructs
                // and refreshes the panel. The test never invokes panel.sync/reload.
                window.render_frame(cx);
                screen.read(cx).mission.as_ref().is_some_and(|panel| {
                    let panel = panel.read(cx);
                    assert!(
                        panel.error.is_none(),
                        "actual RunsScreen history read failed: {:?}",
                        panel.error
                    );
                    panel.loading == LoadingState::Ready
                        && self.active_loads.get() == 0
                        && panel.operation.as_ref() == Some(&self.record.status)
                        && panel.implementation.as_ref() == Some(&expected)
                })
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "actual RunsScreen Overview did not reflect persisted operation revision {} and {expected:?}",
                self.record.status.revision
            );
            window.run_until_parked();
            self.runtime.block_on(tokio::task::yield_now());
        }
        window.run_until_parked();
    }

    fn close(self) {
        assert_eq!(
            self.active_loads.get(),
            0,
            "superseded or replaced panel reads must settle before cleanup"
        );
        drop(self.storage);
        self.home.close().unwrap();
    }

    fn read(&self) -> super::history::MissionSnapshot {
        let store = self.storage.bootstrap_operation_store();
        let before = store.get(self.record.status.operation_id).unwrap();
        let child = self
            .home
            .path()
            .join("runs")
            .join(self.record.status.implementation_run.to_string());
        let child_present = std::fs::symlink_metadata(&child).is_ok();
        let result = self
            .runtime
            .block_on(super::history::load(
                Some(self.home.path().into()),
                MissionRequest {
                    selected: self.record.status.planning_run,
                    project: Some(self.home.path().into()),
                },
            ))
            .unwrap();
        let super::history::LoadOutcome::Loaded(snapshot) = result else {
            panic!("stable fixture");
        };
        assert_eq!(
            store.get(self.record.status.operation_id).unwrap(),
            before,
            "Results must not repair or migrate operation records"
        );
        if !child_present {
            assert!(
                std::fs::symlink_metadata(child).is_err(),
                "Results must not create the reserved child directory"
            );
        }
        *snapshot
    }

    fn append(&self, child: bool, events: Vec<EventPayload>) {
        let run = if child {
            self.record.status.implementation_run
        } else {
            self.record.status.planning_run
        };
        self.runtime.block_on(async {
            let writer = self.storage.open_run_writer(run).await.unwrap();
            writer
                .append_events(events.into_iter().map(VersionedEventPayload::new).collect())
                .await
                .unwrap();
            writer.close().await.unwrap();
        });
    }
}

// The independent oracle is the existing External semantic color, and these
// outputs are exactly what step_chip consumes for border, background and cue.
fn repair4_unknown_step(waiting: bool) -> crate::mission::PlanningStep {
    let mut payloads = vec![EventPayload::StageEntered {
        node: "missing_configuration".parse().unwrap(),
        attempt: 1,
    }];
    if waiting {
        payloads.push(EventPayload::HumanInputRequested {
            node: "missing_configuration".parse().unwrap(),
            session: None,
            call_id: Some("request-unknown".into()),
            prompt: "recorded question".into(),
            schema: None,
        });
    }
    let planning = crate::mission::fixture_fold(&payloads, &[], None)
        .planning
        .unwrap();
    assert!(planning.recovery.is_complete());
    assert!(planning.steps[0].sessions.is_empty());
    planning.steps[0].clone()
}

#[test]
fn results_history_repair4_final_style_is_neutral_offline() {
    let external = crate::theme::Semantic::External.color();
    let agent = crate::theme::Semantic::Agent.color();
    let actual: Vec<_> = [false, true]
        .into_iter()
        .map(|waiting| {
            let occurrence = repair4_unknown_step(waiting);
            let style =
                super::step_chip_style(&occurrence.step, false, Some(occurrence.attribution));
            (style.border, style.background, style.activity)
        })
        .collect();
    let expected = (
        crate::theme::stroke(external).opacity(0.5),
        crate::theme::tint(external),
        None,
    );
    assert_ne!(expected.0, crate::theme::stroke(agent).opacity(0.5));
    assert_ne!(expected.1, crate::theme::tint(agent));
    assert_eq!(actual, vec![expected; 2]);
}

#[test]
fn results_history_repair4_final_style_is_neutral_live() {
    let external = crate::theme::Semantic::External.color();
    let agent = crate::theme::Semantic::Agent.color();
    let actual: Vec<_> = [false, true]
        .into_iter()
        .map(|waiting| {
            let occurrence = repair4_unknown_step(waiting);
            assert_eq!(
                occurrence.step.state,
                if waiting {
                    crate::mission::StepState::WaitingOnYou
                } else {
                    crate::mission::StepState::Working
                }
            );
            let style =
                super::step_chip_style(&occurrence.step, true, Some(occurrence.attribution));
            (style.border, style.background, style.activity)
        })
        .collect();
    let expected = (
        crate::theme::stroke(external),
        crate::theme::tint(external),
        Some(external),
    );
    assert_ne!(expected.0, crate::theme::stroke(agent));
    assert_ne!(expected.1, crate::theme::tint(agent));
    assert_ne!(expected.2, Some(agent));
    assert_eq!(actual, vec![expected; 2]);
}

#[test]
fn results_history_repair4_authenticated_and_implementation_styles_stay_unchanged() {
    use crate::mission::{PlanningAttribution, StepState};
    use crate::theme::{self, Semantic};
    let graph = surge_core::BundledFlows::by_name_latest("bootstrap")
        .unwrap()
        .graph;
    let gate = graph
        .nodes
        .values()
        .find(|node| matches!(node.config, surge_core::node::NodeConfig::HumanGate(_)))
        .unwrap()
        .id
        .clone();
    let terminal = graph
        .nodes
        .values()
        .find(|node| matches!(node.config, surge_core::node::NodeConfig::Terminal(_)))
        .unwrap()
        .id
        .clone();
    for (node, waiting, role) in [
        ("description_author".parse().unwrap(), false, Semantic::Plan),
        (gate, true, Semantic::You),
        (terminal, false, Semantic::External),
    ] {
        let mut events = vec![
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph.clone()),
                graph_hash: graph.identity().unwrap(),
            },
            EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            },
        ];
        if waiting {
            events.push(EventPayload::HumanInputRequested {
                node,
                session: None,
                call_id: Some("recorded-question".into()),
                prompt: "recorded question".into(),
                schema: None,
            });
        }
        let planning = crate::mission::fixture_fold(&events, &[], None)
            .planning
            .unwrap();
        let occurrence = &planning.steps[0];
        assert_eq!(occurrence.attribution, PlanningAttribution::Available);
        assert_eq!(
            occurrence.step.state,
            if waiting {
                StepState::WaitingOnYou
            } else {
                StepState::Working
            }
        );
        for available in [false, true] {
            let style =
                super::step_chip_style(&occurrence.step, available, Some(occurrence.attribution));
            let live_tone = if waiting {
                theme::warning()
            } else {
                theme::accent()
            };
            let (border, background, cue) = if available {
                (
                    theme::stroke(live_tone),
                    theme::tint(live_tone),
                    Some(live_tone),
                )
            } else {
                (
                    theme::stroke(role.color()).opacity(0.5),
                    theme::tint(role.color()),
                    None,
                )
            };
            assert_eq!(
                (style.border, style.background, style.activity),
                (border, background, cue)
            );
        }
    }
    let ordinary = crate::mission::fixture_fold(
        &[],
        &[EventPayload::StageEntered {
            node: "ordinary_unknown".parse().unwrap(),
            attempt: 1,
        }],
        None,
    );
    assert_eq!(ordinary.steps[0].lane, crate::flow_diagram::Lane::Build);
    for available in [false, true] {
        let style = super::step_chip_style(&ordinary.steps[0], available, None);
        let expected = if available {
            (
                theme::stroke(theme::accent()),
                theme::tint(theme::accent()),
                Some(theme::accent()),
            )
        } else {
            (
                theme::stroke(Semantic::Agent.color()).opacity(0.5),
                theme::tint(Semantic::Agent.color()),
                None,
            )
        };
        assert_eq!((style.border, style.background, style.activity), expected);
    }
}

#[gpui_kit::test]
fn results_history_repair4_complete_missing_node_retains_exact_live_activity(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::prepare_with_graph(
        ProjectIdentity::Raw,
        surge_core::BundledFlows::by_name_latest("bootstrap")
            .unwrap()
            .graph,
    );
    let mut events = fixture.initial_events(false);
    events.truncate(2);
    events.push(EventPayload::StageEntered {
        node: "missing_configuration".parse().unwrap(),
        attempt: 1,
    });
    fixture.write_events(false, events);
    let handle = fixture.runtime.handle().clone();
    let _guard = handle.enter();
    let (screen, state) = repair3_screen(&fixture, cx);
    let parent = fixture.record.status.planning_run;
    state.update(cx, |state, _| {
        state.run_streams.get_mut(&parent).unwrap().live = true
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click(format!("run-row-{parent}"), cx);
        window.render_frame(cx);
        window.click("run-tab-Overview", cx);
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::PlanningOnly);
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("planning-step-3").label(),
            Some("Missing configuration: working now")
        );
        let panel = screen.read(cx).mission.as_ref().unwrap().read(cx);
        let planning = panel.mission.as_ref().unwrap().planning.as_ref().unwrap();
        assert_eq!(planning.run, parent);
        assert_eq!(planning.through, Some(3));
        assert!(panel.observes_run(parent, &planning.recovery, false));
        assert!(planning.steps[0].sessions.is_empty());
        assert_eq!(
            planning.steps[0].step.state,
            crate::mission::StepState::Working
        );
        assert!(planning.caveats.iter().any(|c| {
            c.contains("Planning event 3 has no authenticated node configuration attribution")
        }));
        let style = super::step_chip_style(
            &planning.steps[0].step,
            true,
            Some(planning.steps[0].attribution),
        );
        let external = crate::theme::Semantic::External.color();
        assert_eq!(style.border, crate::theme::stroke(external));
        assert_eq!(style.background, crate::theme::tint(external));
        assert_eq!(style.activity, Some(external));
    });
    fixture.close();
}

// Graph-driven planning: deliberately no legacy BootstrapStageStarted.
fn repair3_fixture(terminal: bool) -> (Fixture, surge_core::SessionId) {
    let mut fixture = Fixture::prepare_with_graph(
        ProjectIdentity::Raw,
        surge_core::BundledFlows::by_name_latest("bootstrap")
            .unwrap()
            .graph,
    );
    let session = surge_core::SessionId::new();
    let mut events = fixture.initial_events(false);
    events.truncate(2);
    events.extend([
        EventPayload::StageEntered {
            node: "description_author".parse().unwrap(),
            attempt: 1,
        },
        EventPayload::SessionOpened {
            opened: None,
            handoff: None,
            node: "description_author".parse().unwrap(),
            session: surge_core::SessionId::new(),
            agent: "description-author@1.0".into(),
            agent_id: Some("test-runtime".into()),
        },
        EventPayload::StageCompleted {
            node: "description_author".parse().unwrap(),
            outcome: "pass".parse().unwrap(),
        },
        EventPayload::BootstrapApprovalDecided {
            stage: BootstrapStage::Description,
            decision: surge_core::BootstrapDecision::Approve,
            comment: None,
        },
        EventPayload::StageEntered {
            node: "roadmap_planner".parse().unwrap(),
            attempt: 1,
        },
        EventPayload::SessionOpened {
            opened: None,
            handoff: None,
            node: "roadmap_planner".parse().unwrap(),
            session,
            agent: "roadmap-planner@1.0".into(),
            agent_id: Some("test-runtime".into()),
        },
    ]);
    if terminal {
        events.push(EventPayload::RunCompleted {
            terminal_node: "end".parse().unwrap(),
        });
    }
    fixture.write_events(false, events);
    if terminal {
        fixture.commit_child();
        let store = fixture.storage.bootstrap_operation_store();
        fixture.record = store
            .claim(
                fixture.record.status.operation_id,
                fixture.record.status.revision,
            )
            .unwrap();
        fixture.record = store
            .mark_started(
                fixture.record.status.operation_id,
                fixture.record.status.revision,
            )
            .unwrap();
        fixture.write_run(true);
    }
    (fixture, session)
}

fn repair3_screen(
    fixture: &Fixture,
    cx: &mut TestAppContext,
) -> (
    Entity<super::super::RunsScreen>,
    Entity<crate::app_state::AppState>,
) {
    cx.background_executor.allow_parking();
    cx.update(gpui_kit::init);
    let state = cx.new(|_| {
        let mut state = crate::app_state::AppState::new();
        state.project_path = Some(fixture.home.path().into());
        for (run, child) in [
            (fixture.record.status.planning_run, false),
            (fixture.record.status.implementation_run, true),
        ] {
            if child && fixture.record.child.is_none() {
                continue;
            }
            let rows = fixture
                .runtime
                .block_on(Storage::inspect_existing_run_events(
                    fixture.home.path().join("runs"),
                    run,
                ))
                .unwrap();
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: if rows
                    .iter()
                    .any(|row| matches!(row.payload.payload, EventPayload::RunCompleted { .. }))
                {
                    surge_orchestrator::engine::handle::RunStatus::Completed
                } else {
                    surge_orchestrator::engine::handle::RunStatus::Active
                },
                started_at: chrono::Utc::now(),
                last_event_seq: rows.last().map(|r| r.seq.as_u64()),
                ended_at: None,
            });
            let watermark = rows.last().unwrap().seq.as_u64();
            state
                .run_streams
                .entry(run)
                .or_default()
                .begin_display_history(run, None);
            for row in rows {
                state.run_streams.entry(run).or_default().apply_recorded(
                    &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                        seq: row.seq.as_u64(),
                        payload: Box::new(row.payload.payload),
                    },
                    row.timestamp_ms,
                );
            }
            state
                .run_streams
                .get_mut(&run)
                .unwrap()
                .finish_display_history(watermark);
        }
        state.bootstrap_operations.insert(
            fixture.record.status.operation_id,
            fixture.record.status.clone(),
        );
        state
    });
    let screen = cx.new(|cx| {
        let mut screen = super::super::RunsScreen::new(state.clone(), cx);
        screen.results_home = Some(fixture.home.path().into());
        screen.results_active_loads = fixture.active_loads.clone();
        screen
    });
    (screen, state)
}

#[gpui_kit::test]
fn results_history_repair3_graph_driven_planning_is_visible(cx: &mut TestAppContext) {
    let (fixture, _) = repair3_fixture(false);
    let handle = fixture.runtime.handle().clone();
    let _guard = handle.enter();
    let (screen, _) = repair3_screen(&fixture, cx);
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click(
            format!("run-row-{}", fixture.record.status.planning_run),
            cx,
        );
        window.render_frame(cx);
        window.click("run-tab-Overview", cx);
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::PlanningOnly);
    window.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("planning-step-3").is_some(),
            "actual description StageEntered must be visible"
        );
        assert_eq!(
            window.find("planning-step-7").label(),
            Some("Roadmap planner: last recorded progress")
        );
        assert_eq!(
            window.find("planning-review-1").label(),
            Some("Roadmap: No review recorded")
        );
    });
    fixture.close();
}

#[gpui_kit::test]
fn results_history_repair3_child_parent_navigation_preserves_exact_history(
    cx: &mut TestAppContext,
) {
    let (fixture, session) = repair3_fixture(true);
    let handle = fixture.runtime.handle().clone();
    let _guard = handle.enter();
    let (screen, _) = repair3_screen(&fixture, cx);
    let parent = fixture.record.status.planning_run;
    let child = fixture.record.status.implementation_run;
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click(format!("run-row-{child}"), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(format!("run-row-{parent}")).is_some(),
            "completed parent rail must survive selecting child"
        );
        assert_eq!(window.find("runs-count").label(), Some("Runs · 2"));
        window.click(format!("run-row-{parent}"), cx);
        window.render_frame(cx);
        window.click("run-tab-Log", cx);
        window.render_frame(cx);
        assert!(
            window
                .find("run-header-facts")
                .label()
                .unwrap()
                .contains(&parent.to_string())
        );
        assert!(
            window
                .find(("run-event", 1usize))
                .label()
                .unwrap()
                .contains("roadmap-planner@1.0 session @ roadmap_planner")
        );
        assert!(
            window
                .find(("run-event", 2usize))
                .label()
                .unwrap()
                .contains("STAGE → roadmap_planner")
        );
        window.click("run-tab-Sessions", cx);
        window.render_frame(cx);
        assert!(
            window
                .find("session-opening-8")
                .label()
                .unwrap()
                .contains(&session.to_string())
        );
        assert_eq!(window.find("runs-count").label(), Some("Runs · 2"));
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while fixture.active_loads.get() != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "superseded screen reads must settle"
        );
        window.run_until_parked();
        fixture.runtime.block_on(tokio::task::yield_now());
    }
    fixture.close();
}

#[gpui_kit::test]
fn results_history_repair3_parent_child_observations_are_independent(cx: &mut TestAppContext) {
    let (mut fixture, session) = repair3_fixture(false);
    fixture.commit_child();
    fixture.write_run(true);
    let handle = fixture.runtime.handle().clone();
    let _guard = handle.enter();
    let (screen, state) = repair3_screen(&fixture, cx);
    let parent = fixture.record.status.planning_run;
    let child = fixture.record.status.implementation_run;
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click(format!("run-row-{child}"), cx);
        window.render_frame(cx);
        window.click("run-tab-Overview", cx);
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::Recorded);
    for (parent_live, child_live, expected) in [
        (false, true, "Roadmap planner: last recorded progress"),
        (true, false, "Roadmap planner: working now"),
        (false, false, "Roadmap planner: last recorded progress"),
    ] {
        window.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.run_streams.get_mut(&parent).unwrap().live = parent_live;
                state.run_streams.get_mut(&child).unwrap().live = child_live;
                cx.notify();
            });
            window.render_frame(cx);
            assert_eq!(window.find("planning-step-7").label(), Some(expected));
            assert!(
                window
                    .find("planning-step-7-session-0")
                    .label()
                    .unwrap()
                    .contains(&session.to_string())
            );
        });
    }
    // A live parent observation cannot upgrade a prefix behind its own registry watermark.
    window.update(|window, cx| {
        state.update(cx, |state, cx| {
            state.run_streams.get_mut(&parent).unwrap().live = true;
            state
                .runs
                .iter_mut()
                .find(|run| run.run_id == parent)
                .unwrap()
                .last_event_seq = Some(9);
            cx.notify();
        });
        window.render_frame(cx);
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::Recorded);
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("planning-step-7").label(),
            Some("Roadmap planner: last recorded progress")
        );
    });
    let child_watermark = window.update(|_, cx| state.read(cx).run_streams[&child].last_seq);
    fixture.append(
        false,
        vec![EventPayload::StageCompleted {
            node: "roadmap_planner".parse().unwrap(),
            outcome: "pass".parse().unwrap(),
        }],
    );
    let rows = fixture
        .runtime
        .block_on(Storage::inspect_existing_run_events(
            fixture.home.path().join("runs"),
            parent,
        ))
        .unwrap();
    let last = rows.last().unwrap();
    window.update(|window, cx| {
        state.update(cx, |state, cx| {
            let stream = state.run_streams.get_mut(&parent).unwrap();
            stream.apply_recorded(
                &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                    seq: last.seq.as_u64(),
                    payload: Box::new(last.payload.payload.clone()),
                },
                last.timestamp_ms,
            );
            state
                .runs
                .iter_mut()
                .find(|run| run.run_id == parent)
                .unwrap()
                .last_event_seq = Some(last.seq.as_u64());
            cx.notify();
        });
        window.render_frame(cx);
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::Recorded);
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("planning-step-7").label(),
            Some("Roadmap planner: finished · pass")
        );
        assert_eq!(state.read(cx).run_streams[&child].last_seq, child_watermark);
    });
    fixture.close();
}

#[gpui_kit::test]
fn results_history_repair3_parent_header_is_separate_from_operation(cx: &mut TestAppContext) {
    let (fixture, _) = repair3_fixture(true);
    let handle = fixture.runtime.handle().clone();
    let _guard = handle.enter();
    let (screen, state) = repair3_screen(&fixture, cx);
    state.update(cx, |state, _| {
        state
            .run_streams
            .get_mut(&fixture.record.status.implementation_run)
            .unwrap()
            .live = true;
    });
    let child = state.update(cx, |state, _| state.runs.pop().unwrap());
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click(
            format!("run-row-{}", fixture.record.status.planning_run),
            cx,
        );
        window.render_frame(cx);
        state.update(cx, |state, cx| {
            state.runs.push(child);
            cx.notify();
        });
        window.render_frame(cx);
        assert_eq!(
            window.find("selected-run-status").label(),
            Some("Completed")
        );
        assert_eq!(
            window.find("selected-operation-status").label(),
            Some("Operation: building")
        );
    });
    // Settle any Overview read before releasing retained fixture resources.
    fixture.settle_screen(&screen, window, ImplementationHistory::Recorded);
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("planning-step-7").label(),
            Some("Roadmap planner: the run ended before this step reported")
        );
        assert_eq!(
            window.find("planning-terminal").label(),
            Some("Planning run: Completed")
        );
    });
    fixture.close();
}

#[gpui_kit::test]
fn results_history_actual_screen_overview_tracks_planning_to_implementation(
    cx: &mut TestAppContext,
) {
    actual_screen_journey(cx, ScreenJourney::Baseline);
}

#[gpui_kit::test]
fn results_history_repair2_generated_and_revised_flows_are_readable(cx: &mut TestAppContext) {
    actual_screen_journey(cx, ScreenJourney::GeneratedGraphs);
}

#[gpui_kit::test]
fn results_history_repair2_canonical_project_alias_is_readable(cx: &mut TestAppContext) {
    actual_screen_journey(cx, ScreenJourney::CanonicalAlias);
}

fn actual_screen_journey(cx: &mut TestAppContext, journey: ScreenJourney) {
    let mut fixture = if matches!(journey, ScreenJourney::GeneratedGraphs) {
        let fixture = Fixture::prepare_with_graph(
            ProjectIdentity::Raw,
            surge_core::BundledFlows::by_name_latest("bootstrap")
                .unwrap()
                .graph,
        );
        fixture.write_run(false);
        fixture
    } else {
        Fixture::with_project_identity(if matches!(journey, ScreenJourney::CanonicalAlias) {
            ProjectIdentity::Canonical
        } else {
            ProjectIdentity::Raw
        })
    };
    if matches!(journey, ScreenJourney::GeneratedGraphs) {
        let mut generated: surge_core::graph::Graph =
            toml::from_str(include_str!("../../../testdata/nested_loops_flow.toml")).unwrap();
        generated.metadata.name = "Generated implementation Flow".into();
        let mut revised = generated.clone();
        revised.metadata.description = Some("Revised implementation Flow".into());
        fixture.append(
            false,
            vec![
                EventPayload::PipelineMaterialized {
                    graph_hash: generated.identity().unwrap(),
                    graph: Box::new(generated),
                },
                EventPayload::PipelineMaterialized {
                    graph_hash: revised.identity().unwrap(),
                    graph: Box::new(revised.clone()),
                },
            ],
        );
        // The immutable parent capture keeps its first graph. Only the approved
        // continuation and actual implementation journal use the latest Flow.
        fixture.graph = revised;
    }
    let project = if matches!(journey, ScreenJourney::CanonicalAlias) {
        let root = fixture.home.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("planning")).unwrap();
        std::fs::create_dir(root.join("alias-hop")).unwrap();
        root.join("alias-hop").join("..")
    } else {
        fixture.home.path().to_path_buf()
    };
    let runtime_handle = fixture.runtime.handle().clone();
    let _guard = runtime_handle.enter();
    cx.background_executor.allow_parking();
    cx.update(gpui_kit::init);
    let planning = fixture.record.status.planning_run;
    let implementation = fixture.record.status.implementation_run;
    let planning_rows = fixture
        .runtime
        .block_on(Storage::inspect_existing_run_events(
            fixture.home.path().join("runs"),
            planning,
        ))
        .unwrap();
    let planning_watermark = planning_rows.last().unwrap().seq.as_u64();
    let state = cx.new(|_| {
        let mut state = crate::app_state::AppState::new();
        state.project_path = Some(project.clone());
        if matches!(journey, ScreenJourney::CanonicalAlias) {
            state.project_scope = Some(crate::project::ProjectScope::resolve(&project));
        }
        state.runs.push(crate::app_state::UiRun {
            run_id: planning,
            status: surge_orchestrator::engine::handle::RunStatus::Active,
            started_at: chrono::Utc::now(),
            last_event_seq: Some(planning_watermark),
            ended_at: None,
        });
        for row in planning_rows {
            state
                .run_streams
                .entry(planning)
                .or_default()
                .apply_recorded(
                    &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                        seq: row.seq.as_u64(),
                        payload: Box::new(row.payload.payload),
                    },
                    row.timestamp_ms,
                );
        }
        state
    });
    let screen = cx.new(|cx| {
        let mut screen = super::super::RunsScreen::new(state.clone(), cx);
        screen.results_home = Some(fixture.home.path().into());
        screen.results_active_loads = fixture.active_loads.clone();
        screen.tab = super::super::RunTab::Log;
        screen
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| {
        window.render_frame(cx);
        window.click(format!("run-row-{planning}"), cx);
        window.render_frame(cx);
        window.click("run-tab-Overview", cx);
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::PlanningOnly);
    let original_panel = window.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(format!("run-row-{planning}"))
                .label()
                .unwrap()
                .starts_with("Planning · ")
        );
        assert_eq!(
            window.find("selected-operation-status").label(),
            Some("Operation: planning")
        );
        assert_eq!(
            window.find("mission-implementation-history").label(),
            Some("No implementation launch is recorded.")
        );
        let screen = screen.read(cx);
        assert_eq!(screen.selected, Some(planning));
        assert!(matches!(screen.tab, super::super::RunTab::Overview));
        let panel = screen.mission.clone().unwrap();
        assert_eq!(panel.read(cx).request.project.as_ref(), Some(&project));
        assert!(matches!(
            panel.read(cx).mission.as_ref().unwrap().plan[0].state,
            PlanState::AwaitingYou { .. }
        ));
        panel
    });
    fixture.commit_child();
    window.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.bootstrap_operations.insert(
                fixture.record.status.operation_id,
                fixture.record.status.clone(),
            );
            cx.notify();
        })
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::AwaitingStartup);
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("mission-implementation-history").label(),
            Some("Implementation awaiting startup.")
        );
    });
    fixture.record = fixture
        .storage
        .bootstrap_operation_store()
        .claim(
            fixture.record.status.operation_id,
            fixture.record.status.revision,
        )
        .unwrap();
    window.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.bootstrap_operations.insert(
                fixture.record.status.operation_id,
                fixture.record.status.clone(),
            );
            cx.notify();
        })
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::AwaitingStartup);
    let planning_seq = window.update(|_, cx| state.read(cx).run_streams[&planning].last_seq);
    fixture.write_run(true);
    fixture.record = fixture
        .storage
        .bootstrap_operation_store()
        .mark_started(
            fixture.record.status.operation_id,
            fixture.record.status.revision,
        )
        .unwrap();
    let child_rows = fixture
        .runtime
        .block_on(Storage::inspect_existing_run_events(
            fixture.home.path().join("runs"),
            implementation,
        ))
        .unwrap();
    window.update(|_, cx| {
        state.update(cx, |state, cx| {
            // Only the actual child stream changes here. The cached operation remains
            // preparing and the selected planning row's watermark is unchanged.
            for row in child_rows {
                state
                    .run_streams
                    .entry(implementation)
                    .or_default()
                    .apply_recorded(
                        &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                            seq: row.seq.as_u64(),
                            payload: Box::new(row.payload.payload),
                        },
                        row.timestamp_ms,
                    );
            }
            assert_eq!(state.run_streams[&planning].last_seq, planning_seq);
            cx.notify();
        })
    });
    fixture.settle_screen(&screen, window, ImplementationHistory::Recorded);
    window.update(|window, cx| {
        window.render_frame(cx);
        let screen = screen.read(cx);
        assert_eq!(
            screen.selected,
            Some(planning),
            "the journey must not reselect a run"
        );
        assert_eq!(
            screen.mission.as_ref(),
            Some(&original_panel),
            "automatic refresh retains the original panel owner"
        );
        let panel = original_panel.read(cx);
        assert!(panel.error.is_none());
        assert_eq!(panel.mission.as_ref().unwrap().milestones[0].title, "Timer");
        assert!(panel.mission.as_ref().unwrap().end.is_none());
        assert_eq!(
            panel.active_loads.get(),
            0,
            "all superseded reads must settle before fixture cleanup"
        );
        assert!(window.try_find("mission-implementation-history").is_none());
    });
    let persisted_revision = fixture.record.status.revision;
    let cached_revision = window.update(|_, cx| {
        state.read(cx).bootstrap_operations[&fixture.record.status.operation_id].revision
    });
    for (events, expected_label) in [
        (
            vec![
                EventPayload::LoopIterationCompleted {
                    loop_id: "milestone_loop".parse().unwrap(),
                    index: 0,
                    outcome: "completed".parse().unwrap(),
                },
                EventPayload::StageEntered {
                    node: "final_spec".parse().unwrap(),
                    attempt: 1,
                },
            ],
            "Spec: last recorded progress",
        ),
        (
            vec![EventPayload::StageCompleted {
                node: "final_spec".parse().unwrap(),
                outcome: "drafted".parse().unwrap(),
            }],
            "Spec: finished · drafted",
        ),
    ] {
        let after = window.update(|_, cx| state.read(cx).run_streams[&implementation].last_seq);
        fixture.append(true, events);
        let rows = fixture
            .runtime
            .block_on(Storage::inspect_existing_events_after(
                fixture.home.path().join("runs"),
                implementation,
                surge_persistence::runs::seq::EventSeq(after),
                std::num::NonZeroU32::new(64).unwrap(),
            ))
            .unwrap()
            .unwrap();
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                for row in rows {
                    state
                        .run_streams
                        .get_mut(&implementation)
                        .unwrap()
                        .apply_recorded(
                            &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                                seq: row.seq.as_u64(),
                                payload: Box::new(row.payload.payload),
                            },
                            row.timestamp_ms,
                        );
                }
                assert_eq!(state.run_streams[&planning].last_seq, planning_seq);
                assert_eq!(
                    state.bootstrap_operations[&fixture.record.status.operation_id].revision,
                    cached_revision
                );
                cx.notify();
            })
        });
        assert_eq!(
            fixture
                .storage
                .bootstrap_operation_store()
                .get(fixture.record.status.operation_id)
                .unwrap()
                .unwrap()
                .status
                .revision,
            persisted_revision
        );
        fixture.settle_screen(&screen, window, ImplementationHistory::Recorded);
        window.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("run-0").label(), Some(expected_label));
            let screen = screen.read(cx);
            assert_eq!(screen.selected, Some(planning));
            assert_eq!(screen.mission.as_ref(), Some(&original_panel));
            assert_eq!(fixture.active_loads.get(), 0);
        });
    }
    if matches!(journey, ScreenJourney::CanonicalAlias) {
        let foreign = fixture.home.path().canonicalize().unwrap().join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        assert!(
            fixture
                .runtime
                .block_on(super::history::load(
                    Some(fixture.home.path().into()),
                    MissionRequest {
                        selected: planning,
                        project: Some(foreign)
                    }
                ))
                .is_err(),
            "a real nested foreign root must not gain authority through starts_with"
        );
    }
    drop(_guard);
    fixture.close();
}

#[gpui_kit::test]
fn results_history_planning_without_child_database(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let _guard = fixture.runtime.enter();
    let panel = fixture.panel(cx);
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel.clone(), window, cx));
    fixture.settle(&panel, window);
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("mission-implementation-history").label(),
            Some("No implementation launch is recorded.")
        );
        assert_eq!(
            window.find("mission-operation").label(),
            Some("Operation: planning")
        );
        let panel = panel.read(cx);
        assert!(
            panel.error.is_none(),
            "planning-only Overview must be readable: {:?}",
            panel.error
        );
        let mission = panel.mission.as_ref().unwrap();
        assert_eq!(
            panel.implementation,
            Some(ImplementationHistory::PlanningOnly)
        );
        assert!(matches!(
            mission.plan[0].state,
            PlanState::AwaitingYou { .. }
        ));
        assert!(
            mission.end.is_none(),
            "planning is not implementation completion"
        );
    });
    drop(_guard);
    fixture.close();
}

#[gpui_kit::test]
fn results_history_committed_queued_and_preparing_child_without_database(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new();
    fixture.commit_child();
    let _guard = fixture.runtime.enter();
    let panel = fixture.panel(cx);
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel.clone(), window, cx));
    fixture.settle(&panel, window);
    window.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("mission-implementation-history").label(),
            Some("Implementation awaiting startup.")
        );
        assert_eq!(
            panel.read(cx).implementation,
            Some(ImplementationHistory::AwaitingStartup)
        );
        assert!(
            panel.read(cx).error.is_none(),
            "approved queued child is not a missing historical run: {:?}",
            panel.read(cx).error
        );
    });
    fixture.record = fixture
        .storage
        .bootstrap_operation_store()
        .claim(
            fixture.record.status.operation_id,
            fixture.record.status.revision,
        )
        .unwrap();
    window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.mission = None;
            panel.error = None;
            panel.reload(cx);
        })
    });
    fixture.settle(&panel, window);
    assert!(
        window.update(|_, cx| panel.read(cx).error.is_none()),
        "preparing child absent storage is normal before start"
    );
    drop(_guard);
    fixture.close();
}

#[gpui_kit::test]
fn results_history_child_transition_without_planning_watermark_change(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new();
    fixture.commit_child();
    let _guard = fixture.runtime.enter();
    let panel = fixture.panel(cx);
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel.clone(), window, cx));
    fixture.settle(&panel, window);
    let mut state = crate::app_state::AppState::new();
    let mut planning_stream = crate::run_stream::RunStreamState::default();
    planning_stream.last_seq = 4;
    state
        .run_streams
        .insert(fixture.record.status.planning_run, planning_stream);
    window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            let refresh = panel.refresh_key(&state);
            panel.sync(refresh, cx);
        })
    });
    fixture.settle(&panel, window);
    let previous_key = window.update(|_, cx| panel.read(cx).loaded_refresh.clone());
    fixture.record = fixture
        .storage
        .bootstrap_operation_store()
        .claim(
            fixture.record.status.operation_id,
            fixture.record.status.revision,
        )
        .unwrap();
    fixture.write_run(true);
    fixture.record = fixture
        .storage
        .bootstrap_operation_store()
        .mark_started(
            fixture.record.status.operation_id,
            fixture.record.status.revision,
        )
        .unwrap();
    window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            let mut child_stream = crate::run_stream::RunStreamState::default();
            child_stream.last_seq = 3;
            state
                .run_streams
                .insert(fixture.record.status.implementation_run, child_stream);
            let refresh = panel.refresh_key(&state);
            assert_ne!(previous_key.as_ref(), Some(&refresh));
            assert_eq!(
                state.run_streams[&fixture.record.status.planning_run].last_seq,
                4
            );
            panel.sync(refresh, cx);
        })
    });
    fixture.settle(&panel, window);
    window.update(|window, cx| {
        window.render_frame(cx);
        let panel = panel.read(cx);
        assert!(
            panel.error.is_none(),
            "child stream must refresh without a planning event: {:?}",
            panel.error
        );
        assert_eq!(panel.implementation, Some(ImplementationHistory::Recorded));
        assert_eq!(panel.mission.as_ref().unwrap().milestones[0].title, "Timer");
    });
    drop(_guard);
    fixture.close();
}

#[test]
fn results_history_blocked_planning_preserves_progress_and_operation_attention() {
    use surge_core::bootstrap_operation::BootstrapAttentionReason;
    let mut fixture = Fixture::new();
    fixture.runtime.block_on(async {
        let writer = fixture
            .storage
            .open_run_writer(fixture.record.status.planning_run)
            .await
            .unwrap();
        writer
            .append_events(vec![VersionedEventPayload::new(
                EventPayload::RunRecoveryRequired {
                    control_generation: 1,
                    diagnostic: "human gate has contradictory durable occurrences".into(),
                },
            )])
            .await
            .unwrap();
        writer.close().await.unwrap();
    });
    fixture.record = fixture
        .storage
        .bootstrap_operation_store()
        .block(
            fixture.record.status.operation_id,
            fixture.record.status.revision,
            BootstrapAttentionReason::StorageUnconfirmed,
        )
        .unwrap();
    let snapshot = fixture.read();
    assert_eq!(snapshot.implementation, ImplementationHistory::PlanningOnly);
    assert!(matches!(
        snapshot.mission.plan[0].state,
        PlanState::AwaitingYou { .. }
    ));
    assert!(snapshot.mission.end.is_none());
    assert_eq!(snapshot.operation.unwrap(), fixture.record.status);
    fixture.close();
}

#[test]
fn results_history_cancelled_before_and_after_child_commit_are_distinct() {
    use surge_core::bootstrap_continuation::BootstrapTerminal;
    for child in [false, true] {
        let mut fixture = Fixture::new();
        if child {
            fixture.commit_child();
        }
        let store = fixture.storage.bootstrap_operation_store();
        fixture.record = store
            .request_cancel(fixture.record.status.operation_id)
            .unwrap();
        let cancelling = fixture.read();
        assert_eq!(
            cancelling.implementation,
            if child {
                ImplementationHistory::AwaitingStartup
            } else {
                ImplementationHistory::PlanningOnly
            }
        );
        fixture.record = store
            .settle(
                fixture.record.status.operation_id,
                fixture.record.status.revision,
                &BootstrapTerminal::Cancelled,
            )
            .unwrap();
        let cancelled = fixture.read();
        assert_eq!(
            cancelled.implementation,
            if child {
                ImplementationHistory::Unavailable(
                    super::history::HistoryIssue::MissingAfterStartup,
                )
            } else {
                ImplementationHistory::PlanningOnly
            }
        );
        assert!(cancelled.mission.end.is_none());
        assert!(matches!(
            cancelled.operation.unwrap().state,
            surge_core::bootstrap_operation::BootstrapState::Cancelled
        ));
        drop(store);
        fixture.close();
    }
}

#[test]
fn results_history_child_attention_does_not_turn_unknown_startup_into_absence() {
    use surge_core::bootstrap_operation::BootstrapAttentionReason as Reason;
    for (reason, awaiting) in [
        (Reason::MissingCredential, true),
        (Reason::ConfigurationChanged, true),
        (Reason::PartialStartup, false),
        (Reason::StorageUnconfirmed, false),
    ] {
        let mut fixture = Fixture::new();
        fixture.commit_child();
        fixture.record = fixture
            .storage
            .bootstrap_operation_store()
            .block(
                fixture.record.status.operation_id,
                fixture.record.status.revision,
                reason,
            )
            .unwrap();
        let snapshot = fixture.read();
        assert_eq!(
            snapshot.implementation == ImplementationHistory::AwaitingStartup,
            awaiting
        );
        assert_eq!(snapshot.operation.unwrap(), fixture.record.status);
        assert!(snapshot.mission.end.is_none());
        fixture.close();
    }
}

#[test]
fn results_history_attempt_directory_or_registry_prevents_awaiting_startup() {
    for registry in [false, true] {
        let mut fixture = Fixture::new();
        fixture.commit_child();
        let directory = fixture
            .home
            .path()
            .join("runs")
            .join(fixture.record.status.implementation_run.to_string());
        if registry {
            fixture.write_run(true);
            std::fs::remove_file(directory.join("events.sqlite")).unwrap();
        } else {
            std::fs::create_dir_all(directory).unwrap();
        }
        let snapshot = fixture.read();
        assert_eq!(
            snapshot.implementation,
            ImplementationHistory::Unavailable(super::history::HistoryIssue::ContradictoryStorage)
        );
        assert!(snapshot.mission.end.is_none());
        assert!(matches!(
            snapshot.mission.plan[0].state,
            PlanState::AwaitingYou { .. }
        ));
        fixture.close();
    }
}

#[test]
fn results_history_started_or_terminal_child_missing_is_unavailable() {
    use surge_core::bootstrap_continuation::{BootstrapFailure, BootstrapTerminal};
    for terminal in [false, true] {
        let mut fixture = Fixture::new();
        fixture.commit_child();
        let store = fixture.storage.bootstrap_operation_store();
        fixture.record = store
            .claim(
                fixture.record.status.operation_id,
                fixture.record.status.revision,
            )
            .unwrap();
        fixture.record = store
            .mark_started(
                fixture.record.status.operation_id,
                fixture.record.status.revision,
            )
            .unwrap();
        if terminal {
            fixture.record = store
                .settle(
                    fixture.record.status.operation_id,
                    fixture.record.status.revision,
                    &BootstrapTerminal::Failed {
                        run_id: fixture.record.status.implementation_run,
                        reason: BootstrapFailure::RunFailed,
                    },
                )
                .unwrap();
        }
        assert_eq!(
            fixture.read().implementation,
            ImplementationHistory::Unavailable(super::history::HistoryIssue::MissingAfterStartup)
        );
        drop(store);
        fixture.close();
    }
}

#[test]
fn results_history_present_empty_corrupt_and_wrong_graph_are_not_absent() {
    for case in ["empty", "corrupt", "wrong_graph", "no_child"] {
        let mut fixture = Fixture::new();
        if case != "no_child" {
            fixture.commit_child();
        }
        if case == "empty" {
            fixture.runtime.block_on(async {
                use surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload;
                let BootstrapStoredPayload::V1 { capture, .. } = &fixture.record.payload else {
                    panic!("supported");
                };
                fixture
                    .storage
                    .create_run(
                        fixture.record.status.implementation_run,
                        &capture.fields().implementation_worktree,
                        None,
                    )
                    .await
                    .unwrap()
                    .close()
                    .await
                    .unwrap();
            });
        } else {
            if case == "wrong_graph" {
                fixture.graph = toml::from_str(include_str!(
                    "../../../../../examples/flow_terminal_only.toml"
                ))
                .unwrap();
            }
            fixture.write_run(true);
            if case == "corrupt" {
                std::fs::write(
                    fixture
                        .home
                        .path()
                        .join("runs")
                        .join(fixture.record.status.implementation_run.to_string())
                        .join("events.sqlite"),
                    b"corrupt SQLite",
                )
                .unwrap();
            }
        }
        let snapshot = fixture.read();
        assert!(
            matches!(
                snapshot.implementation,
                ImplementationHistory::Unavailable(_)
            ),
            "{case} must retain unavailable evidence"
        );
        assert!(snapshot.mission.end.is_none());
        fixture.close();
    }
}

#[test]
fn results_history_cold_child_lookup_and_foreign_project_are_checked() {
    let mut fixture = Fixture::new();
    fixture.commit_child();
    let request = MissionRequest {
        selected: fixture.record.status.implementation_run,
        project: Some(fixture.home.path().into()),
    };
    let loaded = fixture
        .runtime
        .block_on(super::history::load(
            Some(fixture.home.path().into()),
            request.clone(),
        ))
        .unwrap();
    let super::history::LoadOutcome::Loaded(snapshot) = loaded else {
        panic!("stable");
    };
    assert_eq!(
        snapshot.implementation,
        ImplementationHistory::AwaitingStartup
    );
    let foreign = MissionRequest {
        project: Some(fixture.home.path().join("foreign")),
        ..request
    };
    assert!(
        fixture
            .runtime
            .block_on(super::history::load(
                Some(fixture.home.path().into()),
                foreign
            ))
            .is_err()
    );
    fixture.close();
}

#[test]
fn results_history_materialization_parent_and_child_pins_remain_immutable() {
    let mut fixture = Fixture::new();
    let parent = fixture.graph.identity().unwrap();
    for name in ["Generated Flow", "Revised Flow"] {
        fixture.graph.metadata.name = name.into();
        fixture.append(
            false,
            vec![EventPayload::PipelineMaterialized {
                graph: Box::new(fixture.graph.clone()),
                graph_hash: fixture.graph.identity().unwrap(),
            }],
        );
    }
    fixture.commit_child();
    let approved = fixture.graph.identity().unwrap();
    let immutable = fixture.record.clone();
    fixture.write_run(true);
    fixture.graph.metadata.description = Some("Later implementation amendment".into());
    fixture.append(
        true,
        vec![EventPayload::PipelineMaterialized {
            graph: Box::new(fixture.graph.clone()),
            graph_hash: fixture.graph.identity().unwrap(),
        }],
    );
    let snapshot = fixture.read();
    assert_eq!(snapshot.implementation, ImplementationHistory::Recorded);
    assert!(snapshot.mission.end.is_none());
    assert_eq!(fixture.record, immutable);
    let surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload::V1 {
        capture, ..
    } = &fixture.record.payload
    else {
        panic!("supported");
    };
    assert_eq!(capture.fields().graph.digest, parent);
    assert_eq!(
        fixture.record.child.as_ref().unwrap().graph().digest,
        approved
    );
    assert_ne!(parent, approved);
    fixture.close();
}

#[test]
fn results_history_initial_and_later_materialization_hash_refusals() {
    for child in [false, true] {
        for invalid in [
            "wrong_first_graph",
            "wrong_first_hash",
            "wrong_first_content",
            "corrupt_later_hash",
        ] {
            let mut fixture = Fixture::prepare(ProjectIdentity::Raw);
            if child {
                fixture.write_run(false);
                fixture.commit_child();
            }
            let original = fixture.graph.clone();
            let mut different = original.clone();
            different.metadata.name = "Different graph".into();
            let mut events = fixture.initial_events(child);
            let EventPayload::PipelineMaterialized { graph, graph_hash } = &mut events[1] else {
                panic!("first graph");
            };
            match invalid {
                "wrong_first_graph" => {
                    **graph = different.clone();
                    *graph_hash = different.identity().unwrap();
                },
                "wrong_first_hash" => *graph_hash = ContentHash::compute(b"wrong first hash"),
                "wrong_first_content" => **graph = different.clone(),
                "corrupt_later_hash" => {},
                _ => unreachable!(),
            }
            // A later matching graph cannot launder a wrong initial graph.
            events.push(EventPayload::PipelineMaterialized {
                graph: Box::new(original.clone()),
                graph_hash: original.identity().unwrap(),
            });
            if invalid == "corrupt_later_hash" {
                events.push(EventPayload::PipelineMaterialized {
                    graph: Box::new(different),
                    graph_hash: ContentHash::compute(b"corrupt later hash"),
                });
            }
            fixture.write_events(child, events);
            let loaded = fixture.runtime.block_on(super::history::load(
                Some(fixture.home.path().into()),
                MissionRequest {
                    selected: fixture.record.status.planning_run,
                    project: Some(fixture.home.path().into()),
                },
            ));
            if child {
                let super::history::LoadOutcome::Loaded(snapshot) = loaded.unwrap() else {
                    panic!("stable");
                };
                assert_eq!(
                    snapshot.implementation,
                    ImplementationHistory::Unavailable(super::history::HistoryIssue::Identity),
                    "{invalid}"
                );
                assert!(snapshot.mission.end.is_none());
            } else {
                assert!(loaded.is_err(), "{invalid}");
            }
            fixture.close();
        }
    }
}

#[test]
fn results_history_ordinary_missing_empty_and_corrupt_are_errors_without_creation() {
    let fixture = Fixture::new();
    for case in ["absent", "empty", "corrupt"] {
        let run = RunId::new();
        let directory = fixture.home.path().join("runs").join(run.to_string());
        if case != "absent" {
            fixture.runtime.block_on(async {
                fixture
                    .storage
                    .create_run(run, fixture.home.path(), None)
                    .await
                    .unwrap()
                    .close()
                    .await
                    .unwrap();
            });
            if case == "corrupt" {
                std::fs::write(directory.join("events.sqlite"), b"invalid database").unwrap();
            }
        }
        let loaded = fixture.runtime.block_on(super::history::load(
            Some(fixture.home.path().into()),
            MissionRequest {
                selected: run,
                project: None,
            },
        ));
        assert!(loaded.is_err(), "{case}");
        if case == "absent" {
            assert!(!directory.exists());
        }
    }
    fixture.close();
}

#[test]
fn results_history_planning_completion_never_completes_implementation() {
    let fixture = Fixture::new();
    let events = [
        BootstrapStage::Description,
        BootstrapStage::Roadmap,
        BootstrapStage::Flow,
    ]
    .into_iter()
    .flat_map(|stage| {
        [
            EventPayload::BootstrapStageStarted { stage },
            EventPayload::BootstrapApprovalDecided {
                stage,
                decision: surge_core::BootstrapDecision::Approve,
                comment: None,
            },
        ]
    })
    .chain([EventPayload::RunCompleted {
        terminal_node: "end".parse().unwrap(),
    }])
    .collect();
    fixture.append(false, events);
    let snapshot = fixture.read();
    assert_eq!(snapshot.implementation, ImplementationHistory::PlanningOnly);
    assert!(
        snapshot
            .mission
            .plan
            .iter()
            .all(|step| matches!(step.state, PlanState::Approved { .. }))
    );
    assert!(snapshot.mission.end.is_none());
    assert!(
        !fixture
            .home
            .path()
            .join("runs")
            .join(fixture.record.status.implementation_run.to_string())
            .exists()
    );
    let unchanged = fixture
        .storage
        .bootstrap_operation_store()
        .get(fixture.record.status.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(unchanged, fixture.record);
    fixture.close();
}

#[test]
fn results_history_failed_planning_retains_its_progress_without_implementation_end() {
    use surge_core::bootstrap_continuation::{BootstrapFailure, BootstrapTerminal};
    let mut fixture = Fixture::new();
    fixture.append(
        false,
        vec![EventPayload::RunFailed {
            error: "planning failed".into(),
        }],
    );
    fixture.record = fixture
        .storage
        .bootstrap_operation_store()
        .settle(
            fixture.record.status.operation_id,
            fixture.record.status.revision,
            &BootstrapTerminal::Failed {
                run_id: fixture.record.status.planning_run,
                reason: BootstrapFailure::RunFailed,
            },
        )
        .unwrap();
    let snapshot = fixture.read();
    assert_eq!(snapshot.implementation, ImplementationHistory::PlanningOnly);
    assert!(snapshot.mission.end.is_none());
    assert!(matches!(
        snapshot.operation.unwrap().state,
        surge_core::bootstrap_operation::BootstrapState::Failed
    ));
    fixture.close();
}

#[test]
fn results_history_recorded_terminal_is_separate_from_operation_cancellation() {
    let mut fixture = Fixture::new();
    fixture.commit_child();
    fixture.write_run(true);
    fixture.append(
        true,
        vec![EventPayload::RunAborted {
            reason: "settled implementation stop".into(),
        }],
    );
    let store = fixture.storage.bootstrap_operation_store();
    fixture.record = store
        .request_cancel(fixture.record.status.operation_id)
        .unwrap();
    fixture.record = store
        .settle(
            fixture.record.status.operation_id,
            fixture.record.status.revision,
            &surge_core::bootstrap_continuation::BootstrapTerminal::Cancelled,
        )
        .unwrap();
    let snapshot = fixture.read();
    assert_eq!(snapshot.implementation, ImplementationHistory::Recorded);
    assert_eq!(
        snapshot.mission.end,
        Some(super::RunEnd::Aborted("settled implementation stop".into()))
    );
    assert!(matches!(
        snapshot.operation.unwrap().state,
        surge_core::bootstrap_operation::BootstrapState::Cancelled
    ));
    drop(store);
    fixture.close();
}

#[test]
fn results_history_revision_change_discards_real_disk_observation() {
    let mut fixture = Fixture::new();
    let home = fixture.home.path().to_path_buf();
    let request = MissionRequest {
        selected: fixture.record.status.planning_run,
        project: Some(home.clone()),
    };
    let (observed, wait_observed) = tokio::sync::oneshot::channel();
    let (release, wait_release) = tokio::sync::oneshot::channel();
    let task = fixture.runtime.spawn(super::history::load_observed(
        Some(home),
        request,
        async move {
            observed.send(()).unwrap();
            wait_release.await.unwrap();
        },
    ));
    fixture.runtime.block_on(wait_observed).unwrap();
    fixture.commit_child();
    release.send(()).unwrap();
    assert!(matches!(
        fixture.runtime.block_on(task).unwrap().unwrap(),
        super::history::LoadOutcome::Stale
    ));
    assert_eq!(
        fixture.read().implementation,
        ImplementationHistory::AwaitingStartup
    );
    fixture.close();
}

#[gpui_kit::test]
fn results_history_generation_fence_ignores_older_async_completion(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let _guard = fixture.runtime.enter();
    let previous = fixture.read();
    fixture.append(
        false,
        vec![EventPayload::BootstrapApprovalDecided {
            stage: BootstrapStage::Description,
            decision: surge_core::BootstrapDecision::Approve,
            comment: None,
        }],
    );
    let current = fixture.read();
    let panel = fixture.panel(cx);
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel.clone(), window, cx));
    fixture.settle(&panel, window);
    let controlled = super::ControlledLoads::default();
    window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.controlled_loads = Some(controlled.clone());
            panel.reload(cx);
        })
    });
    window.run_until_parked();
    window.update(|_, cx| panel.update(cx, |panel, cx| panel.reload(cx)));
    window.run_until_parked();
    assert_eq!(controlled.borrow().len(), 2);
    let newer = controlled.borrow_mut().pop().unwrap();
    newer
        .send(Ok(super::history::LoadOutcome::Loaded(Box::new(current))))
        .ok()
        .unwrap();
    window.run_until_parked();
    let older = controlled.borrow_mut().pop().unwrap();
    older
        .send(Ok(super::history::LoadOutcome::Loaded(Box::new(previous))))
        .ok()
        .unwrap();
    window.run_until_parked();
    window.update(|window, cx| {
        window.render_frame(cx);
        assert!(matches!(
            panel.read(cx).mission.as_ref().unwrap().plan[0].state,
            PlanState::Approved { .. }
        ));
    });
    drop(_guard);
    fixture.close();
}

#[gpui_kit::test]
fn results_history_stale_reload_is_bounded(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let _guard = fixture.runtime.enter();
    let panel = fixture.panel(cx);
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel.clone(), window, cx));
    fixture.settle(&panel, window);
    let controlled = super::ControlledLoads::default();
    window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.controlled_loads = Some(controlled.clone());
            panel.reload(cx);
        })
    });
    window.run_until_parked();
    for _ in 0..2 {
        assert_eq!(controlled.borrow().len(), 1);
        controlled
            .borrow_mut()
            .pop()
            .unwrap()
            .send(Ok(super::history::LoadOutcome::Stale))
            .ok()
            .unwrap();
        window.run_until_parked();
    }
    assert!(
        controlled.borrow().is_empty(),
        "churn must not create an unbounded reload loop"
    );
    window.update(|_, cx| {
        assert_eq!(panel.read(cx).loading, LoadingState::Changed);
        assert!(panel.read(cx).error.is_none());
    });
    drop(_guard);
    fixture.close();
}

#[test]
fn results_history_registry_identity_and_terminal_claims_must_match() {
    for mismatch in [
        "project",
        "terminal_summary",
        "operation_terminal",
        "matching_terminal",
    ] {
        let mut fixture = Fixture::new();
        fixture.commit_child();
        fixture.write_run(true);
        let run = fixture.record.status.implementation_run;
        if mismatch == "project" {
            let pool = surge_persistence::runs::registry::open_registry_pool(
                fixture.home.path(),
                &surge_persistence::runs::MockClock::new(1),
            )
            .unwrap();
            pool.get()
                .unwrap()
                .execute(
                    "UPDATE runs SET project_path=?1 WHERE id=?2",
                    [
                        fixture
                            .home
                            .path()
                            .join("foreign")
                            .to_string_lossy()
                            .to_string(),
                        run.to_string(),
                    ],
                )
                .unwrap();
        } else if mismatch == "terminal_summary" {
            fixture
                .runtime
                .block_on(fixture.storage.set_run_status(
                    &run,
                    surge_core::RunStatus::Completed,
                    Some(1),
                ))
                .unwrap();
        } else {
            let store = fixture.storage.bootstrap_operation_store();
            fixture.record = store
                .claim(
                    fixture.record.status.operation_id,
                    fixture.record.status.revision,
                )
                .unwrap();
            fixture.record = store
                .mark_started(
                    fixture.record.status.operation_id,
                    fixture.record.status.revision,
                )
                .unwrap();
            fixture.record = store
                .settle(
                    fixture.record.status.operation_id,
                    fixture.record.status.revision,
                    &surge_core::bootstrap_continuation::BootstrapTerminal::Completed {
                        run_id: run,
                        terminal: "end".parse().unwrap(),
                    },
                )
                .unwrap();
        }
        if mismatch == "matching_terminal" {
            fixture.append(
                true,
                vec![EventPayload::RunCompleted {
                    terminal_node: "end".parse().unwrap(),
                }],
            );
            fixture
                .runtime
                .block_on(fixture.storage.set_run_status(
                    &run,
                    surge_core::RunStatus::Completed,
                    Some(1),
                ))
                .unwrap();
        }
        let snapshot = fixture.read();
        if mismatch == "matching_terminal" {
            assert_eq!(snapshot.implementation, ImplementationHistory::Recorded);
            assert_eq!(snapshot.mission.end, Some(super::RunEnd::Completed));
            fixture.close();
            continue;
        }
        assert!(
            matches!(
                snapshot.implementation,
                ImplementationHistory::Unavailable(_)
            ),
            "{mismatch}"
        );
        assert!(snapshot.mission.end.is_none());
        fixture.close();
    }
}

#[test]
fn results_history_unsupported_or_corrupt_operation_never_falls_back_to_ordinary_run() {
    for update in [
        "payload_version=99",
        "intent_json='invalid JSON'",
        "state_json='{\"state\":\"pending\",\"phase\":\"implementing\"}'",
    ] {
        let fixture = Fixture::new();
        let pool = surge_persistence::runs::registry::open_registry_pool(
            fixture.home.path(),
            &surge_persistence::runs::MockClock::new(1),
        )
        .unwrap();
        pool.get()
            .unwrap()
            .execute(
                &format!("UPDATE bootstrap_operations SET {update} WHERE operation_id=?1"),
                [fixture.record.status.operation_id.to_string()],
            )
            .unwrap();
        assert!(
            fixture
                .runtime
                .block_on(super::history::load(
                    Some(fixture.home.path().into()),
                    MissionRequest {
                        selected: fixture.record.status.planning_run,
                        project: Some(fixture.home.path().into())
                    }
                ))
                .is_err()
        );
        drop(pool);
        fixture.close();
    }
}

#[test]
fn results_history_child_parent_and_reserved_id_corruption_are_refused() {
    for parent in [true, false] {
        let mut fixture = Fixture::new();
        fixture.commit_child();
        let pool = surge_persistence::runs::registry::open_registry_pool(
            fixture.home.path(),
            &surge_persistence::runs::MockClock::new(1),
        )
        .unwrap();
        if parent {
            let mut child = serde_json::to_value(fixture.record.child.as_ref().unwrap()).unwrap();
            child["parent"] = serde_json::to_value(RunId::new()).unwrap();
            pool.get()
                .unwrap()
                .execute(
                    "UPDATE bootstrap_operations SET child_json=?1 WHERE operation_id=?2",
                    [
                        serde_json::to_string(&child).unwrap(),
                        fixture.record.status.operation_id.to_string(),
                    ],
                )
                .unwrap();
        } else {
            pool.get()
                .unwrap()
                .execute(
                    "UPDATE bootstrap_operations SET implementation_run=?1 WHERE operation_id=?2",
                    [
                        RunId::new().to_string(),
                        fixture.record.status.operation_id.to_string(),
                    ],
                )
                .unwrap();
        }
        assert!(
            fixture
                .runtime
                .block_on(super::history::load(
                    Some(fixture.home.path().into()),
                    MissionRequest {
                        selected: fixture.record.status.planning_run,
                        project: Some(fixture.home.path().into())
                    }
                ))
                .is_err()
        );
        drop(pool);
        fixture.close();
    }
}

#[gpui_kit::test]
fn results_history_screen_request_identity_includes_selected_run_and_project(
    cx: &mut TestAppContext,
) {
    let state = cx.new(|_| crate::app_state::AppState::new());
    let screen = cx.new(|cx| super::super::RunsScreen::new(state.clone(), cx));
    let run = RunId::new();
    let row = |run| {
        super::super::RunRow::from_run(
            &crate::app_state::UiRun {
                run_id: run,
                status: surge_orchestrator::engine::handle::RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: Some(4),
                ended_at: None,
            },
            None,
        )
    };
    state.update(cx, |state, _| {
        state.project_path = Some("/project-a".into())
    });
    let original = screen.update(cx, |screen, cx| {
        screen.mission_request(&row(run), cx).unwrap()
    });
    let other_run = screen.update(cx, |screen, cx| {
        screen.mission_request(&row(RunId::new()), cx).unwrap()
    });
    assert_ne!(
        original, other_run,
        "a different run must replace the async panel owner"
    );
    state.update(cx, |state, _| {
        state.project_path = Some("/project-b".into())
    });
    let other_project = screen.update(cx, |screen, cx| {
        screen.mission_request(&row(run), cx).unwrap()
    });
    assert_ne!(
        original, other_project,
        "a project switch must replace the async panel owner even for the same run id"
    );
}

#[cfg(unix)]
#[test]
fn results_history_child_symlink_and_permission_failure_are_unavailable() {
    use std::os::unix::fs::PermissionsExt;
    for symlink in [true, false] {
        let mut fixture = Fixture::new();
        fixture.commit_child();
        let directory = fixture
            .home
            .path()
            .join("runs")
            .join(fixture.record.status.implementation_run.to_string());
        if symlink {
            std::os::unix::fs::symlink(fixture.home.path().join("absent-target"), &directory)
                .unwrap();
        } else {
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o0)).unwrap();
        }
        let snapshot = fixture.read();
        if !symlink {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(matches!(
            snapshot.implementation,
            ImplementationHistory::Unavailable(super::history::HistoryIssue::Read(_))
        ));
        fixture.close();
    }
}
