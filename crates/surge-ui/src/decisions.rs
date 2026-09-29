//! Decisions you already made, read back from the run logs.
//!
//! The Inbox is a queue of what waits on you; this is its memory — what you
//! approved, sent back, rejected or answered, when, and with what comment.
//! Bootstrap gates record both a generic `HumanInputResolved` and a typed
//! `BootstrapApprovalDecided`; the typed one is used and the generic one
//! skipped so a decision is never listed twice.

use surge_core::{BootstrapDecision, BootstrapStage, EventPayload, RunId};

use crate::theme::Semantic;

/// One recorded decision.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub run: RunId,
    pub at_ms: i64,
    /// "Approved the roadmap", "Sent the description back", …
    pub what: String,
    pub role: Semantic,
    pub comment: Option<String>,
}

fn stage_name(stage: BootstrapStage) -> &'static str {
    match stage {
        BootstrapStage::Description => "the description",
        BootstrapStage::Roadmap => "the roadmap",
        BootstrapStage::Flow => "the flow",
    }
}

fn non_empty(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Decisions recorded in one run's events (`(timestamp_ms, payload)`).
pub fn decisions_in(run: RunId, events: &[(i64, EventPayload)]) -> Vec<Decision> {
    let bootstrap = events
        .iter()
        .any(|(_, e)| matches!(e, EventPayload::BootstrapApprovalDecided { .. }));
    let mut out = Vec::new();
    for (at_ms, event) in events {
        match event {
            EventPayload::BootstrapApprovalDecided {
                stage,
                decision,
                comment,
            } => {
                let (verb, role) = match decision {
                    BootstrapDecision::Approve => ("Approved", Semantic::Verified),
                    BootstrapDecision::Edit => ("Sent back", Semantic::You),
                    BootstrapDecision::Reject => ("Rejected", Semantic::Failure),
                };
                out.push(Decision {
                    run,
                    at_ms: *at_ms,
                    what: format!("{verb} {}", stage_name(*stage)),
                    role,
                    comment: non_empty(comment.as_deref()),
                });
            },
            EventPayload::HumanInputResolved { node, response, .. } if !bootstrap => {
                let outcome = response.get("outcome").and_then(|o| o.as_str());
                let comment = non_empty(response.get("comment").and_then(|c| c.as_str()))
                    .or_else(|| non_empty(response.as_str()));
                let step = node.as_str().replace('_', " ");
                let (what, role) = match outcome {
                    Some(o) if o.contains("approve") || o == "pass" => {
                        (format!("Approved {step}"), Semantic::Verified)
                    },
                    Some(o) if o.contains("reject") || o.contains("abort") => {
                        (format!("Rejected {step}"), Semantic::Failure)
                    },
                    Some(o) => (format!("Chose “{o}” at {step}"), Semantic::You),
                    None => (format!("Answered {step}"), Semantic::You),
                };
                out.push(Decision {
                    run,
                    at_ms: *at_ms,
                    what,
                    role,
                    comment,
                });
            },
            _ => {},
        }
    }
    out
}

/// The newest `limit` decisions across `runs`, newest first.
pub async fn recent(runs: Vec<RunId>, limit: usize) -> Vec<Decision> {
    let Some(home) = surge_core::home::surge_home_dir() else {
        return Vec::new();
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return Vec::new();
    }
    let root = home.join("runs");
    let mut all = Vec::new();
    for run in runs {
        let Ok(events) =
            surge_persistence::runs::Storage::inspect_existing_run_events(root.clone(), run).await
        else {
            continue;
        };
        let events: Vec<(i64, EventPayload)> = events
            .into_iter()
            .map(|e| (e.timestamp_ms, e.payload.payload))
            .collect();
        all.extend(decisions_in(run, &events));
    }
    all.sort_by_key(|d| std::cmp::Reverse(d.at_ms));
    all.truncate(limit);
    all
}

#[cfg(test)]
mod tests {
    use super::decisions_in;
    use crate::theme::Semantic;
    use surge_core::{BootstrapDecision, BootstrapStage, EventPayload, RunId};

    #[test]
    fn bootstrap_decisions_are_listed_once_with_their_comment() {
        let run = RunId::new();
        let events = vec![
            (
                1,
                EventPayload::HumanInputResolved {
                    node: "roadmap_gate".try_into().unwrap(),
                    call_id: None,
                    response: serde_json::json!({ "outcome": "edit", "comment": "split m1" }),
                },
            ),
            (
                2,
                EventPayload::BootstrapApprovalDecided {
                    stage: BootstrapStage::Roadmap,
                    decision: BootstrapDecision::Edit,
                    comment: Some("split m1".into()),
                },
            ),
            (
                3,
                EventPayload::BootstrapApprovalDecided {
                    stage: BootstrapStage::Roadmap,
                    decision: BootstrapDecision::Approve,
                    comment: Some("  ".into()),
                },
            ),
        ];
        let decisions = decisions_in(run, &events);
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].what, "Sent back the roadmap");
        assert_eq!(decisions[0].comment.as_deref(), Some("split m1"));
        assert_eq!(decisions[1].what, "Approved the roadmap");
        assert_eq!(decisions[1].role, Semantic::Verified);
        assert_eq!(decisions[1].comment, None);
    }

    #[test]
    fn plain_gate_answers_keep_their_outcome() {
        let run = RunId::new();
        let events = vec![(
            5,
            EventPayload::HumanInputResolved {
                node: "merge_gate".try_into().unwrap(),
                call_id: None,
                response: serde_json::json!({ "outcome": "defer" }),
            },
        )];
        let d = &decisions_in(run, &events)[0];
        assert_eq!(d.what, "Chose “defer” at merge gate");
        assert_eq!(d.role, Semantic::You);
    }
}
