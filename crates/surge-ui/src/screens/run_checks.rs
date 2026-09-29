//! Saved verifier reports from immutable, hash-checked run artifacts.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::artifact_contract::{ArtifactKind, validate_artifact_text};
use surge_core::{
    ContentHash, EventPayload, RunId, VerificationReportArtifact, VerificationReportOutcome,
};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::ReadEvent;

use crate::theme::{self, Semantic};
use crate::ui;

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

/// How a single reported check reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Passed,
    Failed,
    /// Anything else the producer wrote — kept, never rounded up to a pass.
    Unverified,
}

fn verdict(result: &str) -> Verdict {
    match result.trim().to_ascii_lowercase().as_str() {
        "passed" | "pass" | "ok" => Verdict::Passed,
        "failed" | "fail" => Verdict::Failed,
        _ => Verdict::Unverified,
    }
}

/// The report says "passed" while one of its own checks says "failed".
fn contradiction(report: &VerificationReportArtifact) -> bool {
    report.outcome == VerificationReportOutcome::Passed
        && report.checks.iter().any(|check| verdict(&check.result) == Verdict::Failed)
}

/// Where a saved report came from.
#[derive(Clone, Debug)]
struct Provenance {
    node: String,
    seq: u64,
    recorded: String,
    hash: ContentHash,
    /// (profile, runtime) of the session that produced the report.
    producer: Option<(String, Option<String>)>,
}

#[derive(Clone, Debug)]
struct SavedChecks {
    report: VerificationReportArtifact,
    provenance: Provenance,
}

fn provenance(source: &ReportSource, events: &[ReadEvent]) -> Provenance {
    let recorded = chrono::DateTime::from_timestamp_millis(source.timestamp_ms).map_or_else(
        || format!("{} ms", source.timestamp_ms),
        |time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        },
    );
    let producer = events
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
        .max_by_key(|(seq, _, _)| *seq)
        .map(|(_, profile, runtime)| (profile.to_string(), runtime.clone()));
    Provenance {
        node: source.node.clone(),
        seq: source.seq,
        recorded,
        hash: source.hash,
        producer,
    }
}

async fn load_saved_report(
    root: std::path::PathBuf,
    run_id: RunId,
    events: Vec<ReadEvent>,
) -> Result<SavedChecks, String> {
    let source =
        latest_report(&events).ok_or("No saved verification report; checks are unverified")?;
    let provenance = provenance(&source, &events);
    let bytes = ArtifactStore::new(root)
        .open_bounded(run_id, source.hash, MAX_REPORT_BYTES)
        .await
        .map_err(|error| format!("Saved report unavailable or corrupt: {error}"))?;
    tokio::task::spawn_blocking(move || {
        let report = parse_report(&bytes)?;
        Ok(SavedChecks { report, provenance })
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn load_checks(run_id: RunId) -> Result<SavedChecks, String> {
    let home = surge_core::home::surge_home_dir().ok_or("Surge home unavailable")?;
    let root = home.join("runs");
    let events =
        surge_persistence::runs::Storage::inspect_existing_run_events(root.clone(), run_id)
            .await
            .map_err(|error| error.to_string())?;
    load_saved_report(root, run_id, events).await
}

enum Loaded {
    Loading,
    Ready(Box<SavedChecks>),
    Unavailable(String),
}

pub(super) struct ChecksView {
    run_id: RunId,
    generation: u64,
    loaded: Loaded,
    show_provenance: bool,
}

impl ChecksView {
    pub(super) fn new(run_id: RunId, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            run_id,
            generation: 0,
            loaded: Loaded::Loading,
            show_provenance: false,
        };
        view.refresh(cx);
        view
    }

    fn accept_result(&mut self, generation: u64, result: Result<SavedChecks, String>) {
        if generation != self.generation {
            return;
        }
        self.loaded = match result {
            Ok(saved) => Loaded::Ready(Box::new(saved)),
            Err(error) => Loaded::Unavailable(error),
        };
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let run_id = self.run_id;
        self.loaded = Loaded::Loading;
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

    fn render_report(&self, saved: &SavedChecks, cx: &mut Context<Self>) -> Div {
        let report = &saved.report;
        let passed = report.outcome == VerificationReportOutcome::Passed;
        let (label, role) = if passed {
            ("Verifier reported: passed", Semantic::Verified)
        } else {
            ("Verifier reported: failed", Semantic::Failure)
        };
        let color = role.color();
        let counts = |v: Verdict| report.checks.iter().filter(|c| verdict(&c.result) == v).count();

        let banner = ui::node_card(color)
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(14.0))
            .py(px(12.0))
            .child(
                Icon::new(if passed { Lucide::ShieldCheck } else { Lucide::ShieldAlert })
                    .size(px(18.0))
                    .text_color(color),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .v_flex()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::text_muted())
                            .child(format!(
                                "{} checks · {} passed · {} failed · {} unverified — saved report; commands are not re-run here",
                                report.checks.len(),
                                counts(Verdict::Passed),
                                counts(Verdict::Failed),
                                counts(Verdict::Unverified)
                            )),
                    ),
            );

        let mut body = div()
            .v_flex()
            .gap(px(12.0))
            .child(banner)
            .when(contradiction(report), |el| {
                el.child(
                    ui::node_card(theme::warning())
                        .px(px(14.0))
                        .py(px(10.0))
                        .text_size(px(11.5))
                        .text_color(theme::text_primary())
                        .child(
                            "Contradiction: the report says passed, but one of its own checks failed. \
                             Treat this run as unverified.",
                        ),
                )
            })
            .when(!report.summary.trim().is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(19.0))
                        .text_color(theme::text_primary())
                        .child(report.summary.clone()),
                )
            });

        if report.checks.is_empty() {
            body = body.child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme::text_muted())
                    .child("No commands recorded — the checks remain unverified."),
            );
        }
        for (index, check) in report.checks.iter().enumerate() {
            let v = verdict(&check.result);
            let (icon, tone) = match v {
                Verdict::Passed => (IconName::CircleCheck, theme::success()),
                Verdict::Failed => (IconName::CircleX, theme::error()),
                Verdict::Unverified => (IconName::CircleAlert, theme::warning()),
            };
            body = body.child(
                ui::panel()
                    .v_flex()
                    .gap(px(8.0))
                    .p(px(12.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(10.0))
                            .items_center()
                            .child(Icon::new(icon).size(px(14.0)).text_color(tone))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_dim())
                                    .child(format!("Check {}", index + 1)),
                            )
                            .child(div().flex_1())
                            .child(ui::pill(check.result.trim().to_string(), tone, theme::tint(tone))),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(ui::R_CONTROL))
                            .bg(theme::panel_deep())
                            .border_1()
                            .border_color(theme::hairline())
                            .text_size(px(11.5))
                            .text_color(theme::text_primary())
                            .child(check.command.clone()),
                    )
                    .children(check.note.clone().map(|note| {
                        div()
                            .text_size(px(11.5))
                            .line_height(px(17.0))
                            .text_color(theme::text_muted())
                            .child(note)
                    })),
            );
        }

        if !report.evidence.is_empty() {
            body = body.child(
                div()
                    .v_flex()
                    .gap(px(4.0))
                    .child(ui::section_label("Evidence the verifier points to (not opened here)"))
                    .children(report.evidence.iter().map(|e| {
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::text_muted())
                            .child(format!("· {e}"))
                    })),
            );
        }

        let p = &saved.provenance;
        let open = self.show_provenance;
        body.child(
            div()
                .v_flex()
                .gap(px(6.0))
                .child(
                    div()
                        .id("checks-provenance")
                        .role(Role::Button)
                        .h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(10.5))
                        .text_color(theme::text_dim())
                        .hover(|s: StyleRefinement| s.text_color(theme::text_muted()))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.show_provenance = !view.show_provenance;
                            cx.notify();
                        }))
                        .child(
                            Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                                .size(px(11.0)),
                        )
                        .child("Where this report came from"),
                )
                .when(open, |el| {
                    let producer = p.producer.as_ref().map_or_else(
                        || "producer session not recorded".to_string(),
                        |(profile, runtime)| {
                            format!("{profile} on {}", runtime.as_deref().unwrap_or("unrecorded runtime"))
                        },
                    );
                    el.child(
                        div()
                            .v_flex()
                            .gap(px(3.0))
                            .pl(px(17.0))
                            .text_size(px(10.5))
                            .text_color(theme::text_muted())
                            .child(format!("Task {}", report.task_id))
                            .child(format!("Node {} · event {} · {}", p.node, p.seq, p.recorded))
                            .child(format!("Producer {producer}"))
                            .child(format!("Content {}", p.hash)),
                    )
                }),
        )
    }
}

impl Render for ChecksView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyElement = match &self.loaded {
            Loaded::Loading => div()
                .text_size(px(12.0))
                .text_color(theme::text_muted())
                .child("Loading the saved verification report…")
                .into_any_element(),
            Loaded::Unavailable(reason) => ui::empty_state("◌", "Checks are unverified", reason.clone())
                .into_any_element(),
            Loaded::Ready(saved) => {
                let saved = (**saved).clone();
                self.render_report(&saved, cx).into_any_element()
            },
        };
        div()
            .id("run-checks")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .v_flex()
            .gap(px(12.0))
            .px(px(20.0))
            .py(px(14.0))
            .child(
                div()
                    .h_flex()
                    .child(div().flex_1().child(ui::section_label("Verification")))
                    .child(
                        Button::new("refresh-checks")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label("Refresh")
                            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                    ),
            )
            .child(div().max_w(px(900.0)).child(content))
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
        let saved = super::load_saved_report(root.path().into(), run, events)
            .await
            .unwrap();
        assert_eq!(
            saved.report.outcome,
            surge_core::VerificationReportOutcome::Failed
        );
        assert_eq!(saved.provenance.seq, 4);
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
    fn provenance_uses_only_latest_prior_session_for_its_node() {
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
        let provenance = super::provenance(&source, &sessions);
        assert_eq!(
            provenance.producer,
            Some(("review@2".to_string(), Some("codex".to_string())))
        );
    }

    #[test]
    fn stale_refresh_never_replaces_the_current_result() {
        use gpui_kit::TestAppContext;
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        crate::theme::init();
        let (view, window) = cx.add_window_view(|_window, _cx| super::ChecksView {
            run_id: surge_core::RunId::new(),
            generation: 2,
            loaded: super::Loaded::Loading,
            show_provenance: false,
        });
        view.update(window, |view, _| {
            view.accept_result(1, Err("stale".into()));
            assert!(matches!(view.loaded, super::Loaded::Loading));
            view.accept_result(2, Err("No report".into()));
            assert!(matches!(&view.loaded, super::Loaded::Unavailable(r) if r == "No report"));
        });
    }

    #[test]
    fn checks_require_schema_and_show_reported_failure_without_execution_claim() {
        let report = super::parse_report(FAILED.as_bytes()).unwrap();
        assert_eq!(report.checks[0].command, "npm test");
        assert_eq!(super::verdict(&report.checks[0].result), super::Verdict::Failed);
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
        assert!(super::contradiction(&report));
        let report = super::parse_report(
            FAILED
                .replace("result = 'failed'", "result = 'unverified'")
                .as_bytes(),
        )
        .unwrap();
        // An unknown result is kept as unverified, never rounded to a pass.
        assert_eq!(report.checks[0].result, "unverified");
        assert_eq!(super::verdict(&report.checks[0].result), super::Verdict::Unverified);
        assert!(!super::contradiction(&report));
    }
}
