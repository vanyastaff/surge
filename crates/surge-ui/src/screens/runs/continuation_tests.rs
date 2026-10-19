//! Actual Results controls for failed history and retained successor navigation.
use super::{AppState, Entity, RunId, RunStatus, RunsScreen};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
use surge_core::{
    bootstrap_attempt::{BootstrapAttempt, BootstrapAttemptState},
    bootstrap_continuation::{BootstrapFailure, BootstrapTerminal},
    bootstrap_operation::{BootstrapOperationStatus, BootstrapState},
};

fn failed_operation() -> BootstrapOperationStatus {
    let implementation_run = RunId::new();
    BootstrapOperationStatus {
        operation_id: RunId::new(),
        planning_run: RunId::new(),
        implementation_run,
        state: BootstrapState::Failed,
        result: Some(BootstrapTerminal::Failed {
            run_id: implementation_run,
            reason: BootstrapFailure::RunFailed,
        }),
        revision: 7,
        cancel_requested: false,
        queue_sequence: 1,
        payload_version: 1,
        continuations: Vec::new(),
    }
}

fn history_state(cx: &mut TestAppContext, status: BootstrapOperationStatus) -> Entity<AppState> {
    cx.new(|_| {
        let mut state = AppState::new();
        let mut rows = vec![
            (status.planning_run, RunStatus::Completed),
            (status.implementation_run, RunStatus::Failed),
        ];
        rows.extend(
            status
                .continuations
                .iter()
                .map(|attempt| (attempt.run, RunStatus::Active)),
        );
        state.runs = rows
            .into_iter()
            .map(|(run_id, status)| crate::app_state::UiRun {
                run_id,
                status,
                started_at: chrono::Utc::now(),
                ended_at: matches!(status, RunStatus::Completed | RunStatus::Failed)
                    .then(chrono::Utc::now),
                last_event_seq: None,
            })
            .collect();
        state
            .bootstrap_operations
            .insert(status.operation_id, status);
        state
    })
}

#[test]
fn failed_continuation_actual_button_dispatches_and_pending_click_cannot_repeat() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut cx = TestAppContext::single();
    cx.update(gpui_kit::init);
    crate::theme::init();
    let status = failed_operation();
    let source = status.implementation_run;
    let planning = status.planning_run;
    let state = history_state(&mut cx, status);
    let screen = cx.new(|cx| {
        let mut screen = RunsScreen::new(state, cx);
        screen.selected = Some(source);
        screen
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| window.draw(cx).clear(cx));
    let button = window
        .debug_bounds("continue-bootstrap")
        .expect("failed implementation exposes actual Continue control");
    window.simulate_click(button.center(), Modifiers::default());
    screen.update(window, |screen, cx| {
        assert_eq!(screen.selected, Some(source));
        assert_eq!(
            screen.action_note.as_deref(),
            Some("Daemon offline; continuation was not submitted")
        );
        assert!(screen.continuation_requests.is_empty());
        screen.continuation_pending = true;
        screen.action_note = Some("request still pending".into());
        cx.notify();
    });
    window.update(|window, cx| window.draw(cx).clear(cx));
    let disabled = window.debug_bounds("continue-bootstrap").unwrap();
    window.simulate_click(disabled.center(), Modifiers::default());
    screen.update(window, |screen, cx| {
        assert_eq!(screen.action_note.as_deref(), Some("request still pending"));
        assert!(screen.continuation_requests.is_empty());
        screen.select_run(planning, cx);
    });
    window.update(|window, cx| window.draw(cx).clear(cx));
    assert!(window.debug_bounds("continue-bootstrap").is_none());
}

#[test]
fn failed_continuation_fresh_screen_opens_retained_successor_and_keeps_failed_history() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut cx = TestAppContext::single();
    cx.update(gpui_kit::init);
    crate::theme::init();
    let mut status = failed_operation();
    let source = status.implementation_run;
    let successor = RunId::new();
    let operation = status.operation_id;
    status.continuations.push(BootstrapAttempt {
        continuation_id: RunId::new(),
        operation_id: operation,
        source_run: source,
        run: successor,
        boundary: 87,
        runtime: surge_core::ContentHash::compute(b"captured runtime"),
        state: BootstrapAttemptState::Running,
    });
    let state = history_state(&mut cx, status);
    let screen = cx.new(|cx| {
        let mut screen = RunsScreen::new(state.clone(), cx);
        screen.selected = Some(source);
        screen
    });
    let (_, window) =
        cx.add_window_view(|window, cx| gpui_kit::component::Root::new(screen.clone(), window, cx));
    window.update(|window, cx| window.draw(cx).clear(cx));
    assert!(window.debug_bounds("continue-bootstrap").is_none());
    let button = window
        .debug_bounds("open-bootstrap-continuation")
        .expect("fresh Results screen recovers recorded successor action");
    window.simulate_click(button.center(), Modifiers::default());
    window.update(|window, cx| {
        window.render_frame(cx);
        let facts = window.find("run-header-facts").label().unwrap().to_owned();
        assert!(facts.contains("Continuation: running"), "{facts}");
        assert!(
            facts.contains("original operation remains failed"),
            "{facts}"
        );
    });
    screen.update(window, |screen, cx| {
        assert_eq!(screen.selected, Some(successor));
        assert!(screen.continuation_requests.is_empty());
        let status = &state.read(cx).bootstrap_operations[&operation];
        assert_eq!(status.state, BootstrapState::Failed);
        assert_eq!(status.implementation_run, source);
        assert!(
            state
                .read(cx)
                .runs
                .iter()
                .any(|row| { row.run_id == source && row.status == RunStatus::Failed })
        );
    });
    assert!(window.debug_bounds("continue-bootstrap").is_none());
}
