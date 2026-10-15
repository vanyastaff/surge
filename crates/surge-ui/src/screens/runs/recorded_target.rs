//! Exact journal destinations inside the existing run cockpit; no inferred identity.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordedTarget {
    Stage {
        entered: u64,
    },
    Session {
        entered: u64,
        opened: u64,
        connection: surge_core::SessionId,
    },
}
pub(super) fn unavailable(message: String) -> Div {
    div().v_flex().gap(px(8.0)).p(px(24.0)).child(
        div()
            .id("requested-run-evidence-unavailable")
            .test_support()
            .debug_selector(|| "requested-run-evidence-unavailable".into())
            .role(Role::Region)
            .aria_label("Requested evidence unavailable")
            .child(ui::meta(message)),
    )
}
pub(super) fn session_page(
    stream: &crate::run_stream::RunStreamState,
    opened: u64,
    connection: surge_core::SessionId,
) -> Option<usize> {
    if !stream.session_history_confirmed() {
        return None;
    }
    stream
        .sessions
        .iter()
        .rev()
        .position(|record| record.seq == opened && record.session == connection)
        .map(|index| index / ui::SESSIONS_PER_PAGE)
}
impl RunsScreen {
    pub(crate) fn select_recorded(
        &mut self,
        run: RunId,
        target: RecordedTarget,
        cx: &mut Context<Self>,
    ) {
        self.select_run(run, cx);
        self.recorded_target = Some((run, target));
        self.tab = match target {
            RecordedTarget::Stage { .. } => RunTab::Overview,
            RecordedTarget::Session { .. } => RunTab::Sessions,
        };
        cx.notify();
    }
    pub(super) fn recorded_session_known(
        &self,
        run: RunId,
        entered: u64,
        opened: u64,
        connection: surge_core::SessionId,
        cx: &Context<Self>,
    ) -> bool {
        self.state
            .read(cx)
            .project_history
            .runs
            .get(&run)
            .filter(|observed| {
                observed.source.availability == crate::project_lifecycle::Availability::Available
                    && observed.source.coverage == crate::project_lifecycle::Coverage::Complete
            })
            .and_then(|observed| observed.execution.as_ref())
            .filter(|view| view.run == run)
            .and_then(|view| view.stages.iter().find(|stage| stage.entered == entered))
            .is_some_and(|stage| {
                stage
                    .sessions
                    .iter()
                    .any(|session| session.opened == opened && session.connection == connection)
            })
    }
    pub(super) fn render_recorded_stage(&self, cx: &Context<Self>) -> Div {
        let Some((run, RecordedTarget::Stage { entered })) = self.recorded_target else {
            return unavailable("No recorded stage requested.".into());
        };
        let state = self.state.read(cx);
        let Some(observed) = state.project_history.runs.get(&run).filter(|run| {
            run.source.availability == crate::project_lifecycle::Availability::Available
                && run.source.coverage == crate::project_lifecycle::Coverage::Complete
        }) else {
            return unavailable(format!(
                "Stage entry #{entered} for {run} is unavailable or its journal prefix is incomplete."
            ));
        };
        let Some(execution) = observed.execution.as_ref().filter(|view| view.run == run) else {
            return unavailable("The recorded run projection is unavailable.".into());
        };
        let Some(stage) = execution
            .stages
            .iter()
            .find(|stage| stage.entered == entered)
        else {
            return unavailable(format!(
                "Stage entry #{entered} is absent from the loaded run history."
            ));
        };
        let mut body = div().v_flex().gap(px(8.0)).p(px(24.0)).child(
            div()
                .id("focused-recorded-stage")
                .test_support()
                .debug_selector(|| "focused-recorded-stage".into())
                .role(Role::Region)
                .aria_label(format!("Requested stage entry #{entered}"))
                .child(ui::meta(format!(
                    "Recorded stage {} · entry #{entered} · attempt {}",
                    stage.node, stage.attempt
                ))),
        );
        if let Some((_, result)) = &stage.result {
            body = body.child(ui::meta(format!("Recorded outcome: {result}")));
        }
        if let Some(error) = &stage.failure {
            body = body.child(ui::meta(format!("Recorded failure: {error}")));
        }
        if let Some(summary) = &stage.summary {
            body = body.child(ui::meta(summary.clone()));
        }
        if stage.continuity_unknown.is_some() {
            body = body.child(ui::meta("Continuation after a journal gap is unknown."));
        }
        for session in &stage.sessions {
            let target = RecordedTarget::Session {
                entered,
                opened: session.opened,
                connection: session.connection,
            };
            body = body
                .child(ui::meta(format!(
                    "{} via {} · opening #{} · {}",
                    session.profile,
                    session.runtime.as_deref().unwrap_or("unrecorded runtime"),
                    session.opened,
                    session.connection
                )))
                .child(
                    Button::new(SharedString::from(format!(
                        "stage-session-{}",
                        session.opened
                    )))
                    .label("Inspect this recorded session")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_recorded(run, target, cx)),
                    ),
                );
        }
        for caveat in &execution.caveats {
            body = body.child(ui::meta(caveat.to_string()));
        }
        body.child(
            Button::new("leave-recorded-stage-focus")
                .label("Show run overview")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.recorded_target = None;
                    cx.notify();
                })),
        )
    }
}
