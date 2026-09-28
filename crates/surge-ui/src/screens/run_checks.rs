//! Saved verifier reports from immutable, hash-checked run artifacts.

use gpui_kit::component::StyledExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::*;
use surge_core::artifact_contract::{ArtifactKind, validate_artifact_text};
use surge_core::{
    ContentHash, EventPayload, RunId, VerificationReportArtifact, VerificationReportOutcome,
};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::ReadEvent;

use crate::theme;

const MAX_REPORT_BYTES: usize = 1024 * 1024;

struct ReportSource {
    hash: ContentHash,
    node: String,
    seq: u64,
    timestamp_ms: i64,
}

fn latest_report(events: &[ReadEvent]) -> Option<ReportSource> {
    events
        .iter()
        .filter_map(|event| {
            let EventPayload::ArtifactProduced {
                node,
                artifact,
                name,
                source_path,
                ..
            } = &event.payload.payload
            else {
                return None;
            };
            let is_report = name.parse::<ArtifactKind>() == Ok(ArtifactKind::VerificationReport)
                || name == "verification-report.toml"
                || source_path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .is_some_and(|name| name == "verification-report.toml");
            is_report.then(|| ReportSource {
                hash: *artifact,
                node: node.to_string(),
                seq: event.seq.0,
                timestamp_ms: event.timestamp_ms,
            })
        })
        .max_by_key(|source| source.seq)
}

fn parse_report(bytes: &[u8]) -> Result<VerificationReportArtifact, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| format!("Report is not UTF-8: {error}"))?;
    let validation = validate_artifact_text(ArtifactKind::VerificationReport, text);
    if !validation.is_valid() {
        let details = validation
            .diagnostics
            .iter()
            .take(5)
            .map(|issue| issue.message.chars().take(300).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "Saved report does not satisfy the verification-report contract:\n{details}"
        ));
    }
    toml::from_str(text).map_err(|error| format!("Cannot parse saved report: {error}"))
}

fn report_text(report: &VerificationReportArtifact) -> String {
    let outcome = match report.outcome {
        VerificationReportOutcome::Passed => "passed",
        VerificationReportOutcome::Failed => "failed",
    };
    let mut text = format!(
        "Reported outcome: {outcome}\nTask: {}\n\n{}\n\nSaved report snapshot. Current worktree files may have changed.\nRecorded commands are not rerun by this UI.\nCommand execution and exit codes are not independently confirmed by this report.\n",
        report.task_id, report.summary
    );
    if report.outcome == VerificationReportOutcome::Passed
        && report
            .checks
            .iter()
            .any(|check| check.result.trim().eq_ignore_ascii_case("failed"))
    {
        text.push_str(
            "\nContradiction: the reported outcome is passed, but a check is reported failed.\n",
        );
    }
    if report.checks.is_empty() {
        text.push_str("\nNo commands recorded; checks remain unverified.\n");
    }
    for (index, check) in report.checks.iter().enumerate() {
        text.push_str(&format!(
            "\nCheck {}\nCommand: {}\nResult reported: {}\n",
            index + 1,
            check.command,
            check.result
        ));
        if let Some(note) = &check.note {
            text.push_str(&format!("Note: {note}\n"));
        }
    }
    if !report.evidence.is_empty() {
        text.push_str(
            "\nEvidence references reported by producer (not opened or independently validated):\n",
        );
        for evidence in &report.evidence {
            text.push_str(&format!("- {evidence}\n"));
        }
    }
    text
}

fn report_identity(run_id: RunId, source: &ReportSource, events: &[ReadEvent]) -> String {
    let timestamp = chrono::DateTime::from_timestamp_millis(source.timestamp_ms).map_or_else(
        || format!("{} ms", source.timestamp_ms),
        |time| time.to_rfc3339(),
    );
    let session = events
        .iter()
        .filter_map(|event| {
            if event.seq.0 >= source.seq {
                return None;
            }
            let EventPayload::SessionOpened {
                node,
                agent,
                agent_id,
                ..
            } = &event.payload.payload
            else {
                return None;
            };
            (node.as_str() == source.node).then_some((event.seq.0, agent, agent_id))
        })
        .max_by_key(|(seq, _, _)| *seq);
    let actor = session.map_or_else(
        || "Producer session identity not recorded".into(),
        |(_, profile, runtime)| {
            format!(
                "Producer profile: {profile}\nProducer runtime: {}",
                runtime.as_deref().unwrap_or("not recorded")
            )
        },
    );
    format!(
        "Source run: {run_id}\nNode: {}\nEvent: {}\nRecorded: {timestamp}\nContent hash: {}\n{actor}",
        source.node, source.seq, source.hash
    )
}

async fn load_saved_report(
    root: std::path::PathBuf,
    run_id: RunId,
    events: Vec<ReadEvent>,
) -> Result<String, String> {
    let source =
        latest_report(&events).ok_or("No saved verification report; checks are unverified")?;
    let identity = report_identity(run_id, &source, &events);
    let bytes = ArtifactStore::new(root)
        .open_bounded(run_id, source.hash, MAX_REPORT_BYTES)
        .await
        .map_err(|error| format!("Saved report unavailable or corrupt: {error}\n\n{identity}"))?;
    tokio::task::spawn_blocking(move || {
        let report = parse_report(&bytes).map_err(|error| format!("{error}\n\n{identity}"))?;
        Ok(format!("{}\n\n{identity}\n", report_text(&report)))
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn load_checks(run_id: RunId) -> Result<String, String> {
    let home = surge_core::home::surge_home_dir().ok_or("Surge home unavailable")?;
    let root = home.join("runs");
    let events =
        surge_persistence::runs::Storage::inspect_existing_run_events(root.clone(), run_id)
            .await
            .map_err(|error| error.to_string())?;
    load_saved_report(root, run_id, events).await
}

pub(super) struct ChecksView {
    run_id: RunId,
    generation: u64,
    text: String,
    dirty: bool,
    editor: Entity<EditorState>,
}

impl ChecksView {
    pub(super) fn new(run_id: RunId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| EditorState::new(window, cx).language("plaintext"));
        let mut view = Self {
            run_id,
            generation: 0,
            text: String::new(),
            dirty: true,
            editor,
        };
        view.refresh(cx);
        view
    }

    fn accept_result(&mut self, generation: u64, result: Result<String, String>) {
        if generation != self.generation {
            return;
        }
        self.text = result.unwrap_or_else(|error| format!("Checks unverified\n\n{error}"));
        self.dirty = true;
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let run_id = self.run_id;
        self.text = "Loading saved verification report…".into();
        self.dirty = true;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = load_checks(run_id).await;
            cx.update(|cx| {
                let _ = this.update(cx, |view, cx| {
                    view.accept_result(generation, result);
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

impl Render for ChecksView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dirty {
            self.editor
                .update(cx, |editor, cx| editor.set_value(&self.text, window, cx));
            self.dirty = false;
        }
        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap(px(8.0))
            .p(px(12.0))
            .child(
                div()
                    .h_flex()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(theme::text_primary())
                            .child("Saved verification report"),
                    )
                    .child(
                        Button::new("refresh-checks")
                            .ghost()
                            .label("Refresh checks")
                            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .debug_selector(|| "saved-checks-editor".into())
                    .child(
                        Editor::new(&self.editor)
                            .readonly(true)
                            .aria_label("Saved verification report and reported commands")
                            .h(relative(1.0)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    const FAILED: &str = "schema_version = 1\ntask_id = 'timer'\noutcome = 'failed'\nsummary = 'Broken check'\n[[checks]]\ncommand = 'npm test'\nresult = 'failed'\nnote = 'test failed'\n";

    fn artifact_event(
        seq: u64,
        hash: surge_core::ContentHash,
    ) -> surge_persistence::runs::ReadEvent {
        surge_persistence::runs::ReadEvent {
            seq: surge_persistence::runs::EventSeq(seq),
            timestamp_ms: 1234,
            kind: "ArtifactProduced".into(),
            payload: surge_core::VersionedEventPayload::new(
                surge_core::EventPayload::ArtifactProduced {
                    node: "verify_app".try_into().unwrap(),
                    artifact: hash,
                    path: "/mutable/verification-report.toml".into(),
                    name: "verification_report".into(),
                    source_path: None,
                },
            ),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn checks_use_latest_hash_and_never_fall_back_to_older_valid_report() {
        let root = tempfile::tempdir().unwrap();
        let store = surge_persistence::artifacts::ArtifactStore::new(root.path());
        let run = surge_core::RunId::new();
        let valid = store
            .put(run, "verification-report", FAILED.as_bytes())
            .await
            .unwrap();
        let events = vec![artifact_event(4, valid.hash)];
        let text = super::load_saved_report(root.path().into(), run, events)
            .await
            .unwrap();
        assert!(text.contains("Reported outcome: failed"));
        assert!(text.contains("Event: 4"));
        let bad = store
            .put(run, "verification-report", b"invalid schema")
            .await
            .unwrap();
        let events = vec![artifact_event(9, bad.hash), artifact_event(4, valid.hash)];
        assert!(
            super::load_saved_report(root.path().into(), run, events)
                .await
                .unwrap_err()
                .contains("contract")
        );
        let blob = root
            .path()
            .join(run.to_string())
            .join("artifacts")
            .join(valid.hash.to_hex());
        std::fs::write(blob, b"corrupt bytes").unwrap();
        assert!(
            super::load_saved_report(root.path().into(), run, vec![artifact_event(4, valid.hash)])
                .await
                .unwrap_err()
                .contains("corrupt")
        );
        let missing = surge_core::ContentHash::compute(b"absent artifact");
        assert!(
            super::load_saved_report(root.path().into(), run, vec![artifact_event(10, missing)])
                .await
                .unwrap_err()
                .contains("unavailable")
        );
        assert!(
            super::load_saved_report(root.path().into(), run, vec![])
                .await
                .unwrap_err()
                .contains("unverified")
        );
    }

    #[test]
    fn report_identity_uses_only_latest_prior_session_for_its_node() {
        let source = super::ReportSource {
            hash: surge_core::ContentHash::compute(b"report"),
            node: "verify_app".into(),
            seq: 10,
            timestamp_ms: 1234,
        };
        let sessions = [
            (2, "verify_app", "old"),
            (5, "verify_app", "codex"),
            (8, "other_node", "wrong"),
            (12, "verify_app", "future"),
        ]
        .map(|(seq, node, runtime)| surge_persistence::runs::ReadEvent {
            seq: surge_persistence::runs::EventSeq(seq),
            timestamp_ms: 1000,
            kind: "SessionOpened".into(),
            payload: surge_core::VersionedEventPayload::new(
                surge_core::EventPayload::SessionOpened {
                    node: node.try_into().unwrap(),
                    session: surge_core::SessionId::new(),
                    agent: "review@2".into(),
                    agent_id: Some(runtime.into()),
                },
            ),
        });
        let identity = super::report_identity(surge_core::RunId::new(), &source, &sessions);
        assert!(identity.contains("Producer runtime: codex"));
        assert!(identity.contains("Producer profile: review@2"));
        assert!(!identity.contains("wrong"));
        assert!(!identity.contains("future"));
    }

    #[test]
    fn native_checks_editor_is_readonly_and_ignores_stale_refresh() {
        use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        crate::theme::init();
        let (view, window) = cx.add_window_view(|window, cx| {
            let editor = cx.new(|cx| super::EditorState::new(window, cx).language("plaintext"));
            super::ChecksView {
                run_id: surge_core::RunId::new(),
                generation: 2,
                text: "Current report".into(),
                dirty: true,
                editor,
            }
        });
        view.update(window, |view, _| {
            view.accept_result(1, Ok("Stale report".into()));
            assert_eq!(view.text, "Current report");
            view.accept_result(2, Err("No report".into()));
            assert!(view.text.contains("Checks unverified"));
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = window.debug_bounds("saved-checks-editor").unwrap();
        window.simulate_click(bounds.center(), Modifiers::default());
        window.simulate_input("fabricated pass");
        view.update(window, |view, cx| {
            assert_eq!(view.editor.read(cx).value().as_ref(), view.text)
        });
    }

    #[test]
    fn checks_require_schema_and_show_reported_failure_without_execution_claim() {
        let report = super::parse_report(FAILED.as_bytes()).unwrap();
        let text = super::report_text(&report);
        assert!(text.contains("Reported outcome: failed"));
        assert!(text.contains("npm test"));
        assert!(text.contains("not independently confirmed"));
        assert!(
            super::parse_report(FAILED.replace("schema_version = 1\n", "").as_bytes()).is_err()
        );
        assert!(super::parse_report(b"bad report").is_err());
    }

    #[test]
    fn checks_preserve_unknown_results_and_flag_contradiction() {
        let report = super::parse_report(
            FAILED
                .replace("outcome = 'failed'", "outcome = 'passed'")
                .as_bytes(),
        )
        .unwrap();
        assert!(super::report_text(&report).contains("Contradiction"));
        let report = super::parse_report(
            FAILED
                .replace("result = 'failed'", "result = 'unverified'")
                .as_bytes(),
        )
        .unwrap();
        assert!(super::report_text(&report).contains("Result reported: unverified"));
    }
}
