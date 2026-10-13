//! Triage Author dispatcher: assembles inputs, runs the bundled
//! `triage-author@1.0` profile through the standalone agent invocation
//! (`crate::agent_invocation`, #99), and parses its `triage_decision.json`
//! into a typed [`TriageDecision`].
//!
//! The invocation resolves the agent through the operator's profile and agent
//! registries (ADR-0019), so triage runs on whichever provider the profile
//! names. This module keeps only the domain: the inputs, the attempt loop with
//! feedback, the decision parsing and the outcome/decision consistency check.
//!
//! Triage stays standalone. Running it as an engine graph node ("Layer 2")
//! is not planned; see `docs/ARCHITECTURE.md`.

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::facade::BridgeFacade;

use crate::agent_invocation::{
    DEFAULT_MAX_CONTRACT_REJECTIONS, InvocationFailure, InvocationRequest, invoke,
};
use crate::profile_loader::ProfileRegistry;
use surge_core::triage_decision::{TriageDecisionArtifact, TriagePriority, TriageVerdict};
use surge_intake::types::{Priority, TaskDetails, TaskId, TaskSummary, TriageDecision};

/// Full input bundle handed to Triage Author at the start of its session.
///
/// Serialised as the top-level JSON the agent receives. The agent reads
/// `task` (the new ticket), `candidates` (similar open tickets +
/// recent specs), and `active_runs` (current Surge runs).
#[derive(Debug, Clone, Serialize)]
pub struct TriageInput {
    pub task: TaskDetails,
    pub candidates: Vec<TaskSummary>,
    pub active_runs: Vec<ActiveRunSummary>,
}

/// Snapshot of an active Surge run for triage dedup context.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveRunSummary {
    pub run_id: String,
    pub task_id: Option<String>,
    pub status: String,
    pub started_at: String,
}

/// Turn `triage_decision.json` into a [`TriageDecision`]. The shape and the
/// per-decision required fields are [`TriageDecisionArtifact`]'s rule — the
/// same one the profile's artifact contract checks in-session (#119) — so
/// this only maps an accepted artifact, plus the task-id syntax of
/// `duplicate_of`, which the intake crate owns.
fn decision_from_artifact(raw: &[u8]) -> Result<TriageDecision, String> {
    let content = std::str::from_utf8(raw).map_err(|error| format!("not UTF-8: {error}"))?;
    let artifact = TriageDecisionArtifact::parse(content).map_err(|error| error.to_string())?;
    if let Some(issue) = artifact.validate().into_iter().next() {
        return Err(issue.to_string());
    }
    let priority = match artifact.priority() {
        TriagePriority::Urgent => Priority::Urgent,
        TriagePriority::High => Priority::High,
        TriagePriority::Medium => Priority::Medium,
        TriagePriority::Low => Priority::Low,
    };
    let reasoning = artifact.priority_reasoning.unwrap_or_default();
    Ok(match artifact.decision {
        TriageVerdict::Enqueued => TriageDecision::Enqueued {
            priority,
            reasoning,
            summary: artifact.summary.unwrap_or_default(),
        },
        TriageVerdict::Duplicate => TriageDecision::Duplicate {
            of: TaskId::try_new(artifact.duplicate_of.unwrap_or_default())
                .map_err(|error| format!("invalid duplicate_of: {error}"))?,
            reasoning,
        },
        TriageVerdict::OutOfScope => TriageDecision::OutOfScope { reasoning },
        TriageVerdict::Unclear => TriageDecision::Unclear {
            question: artifact.question.unwrap_or_default(),
        },
    })
}

/// The bundled Triage Author profile.
pub const TRIAGE_AUTHOR_PROFILE: &str = "triage-author@1.0";

/// Tunable parameters for [`dispatch_triage`].
#[derive(Clone)]
#[non_exhaustive]
pub struct TriageOptions {
    /// Per-attempt deadline. Default: 5 min (matches RFC-0010 §"Bootstrap stage failures").
    pub attempt_timeout: Duration,
    /// Maximum attempts before falling back to `Unclear`. Default: 3.
    pub max_attempts: u32,
    /// Root directory for per-call scratch dirs.
    pub scratch_root: PathBuf,
    /// Whether to keep scratch on Unclear / TriageError for post-mortem.
    pub keep_scratch_on_failure: bool,
    /// The profile registry the Triage Author resolves through.
    pub profiles: Arc<ProfileRegistry>,
    /// The operator's merged agent registry; `None` uses the builtins.
    pub agents: Option<Arc<surge_acp::Registry>>,
}

impl TriageOptions {
    /// Build options with sensible defaults given a scratch root and the
    /// registries the Triage Author resolves through.
    #[must_use]
    pub fn with_scratch_root(
        scratch_root: PathBuf,
        profiles: Arc<ProfileRegistry>,
        agents: Option<Arc<surge_acp::Registry>>,
    ) -> Self {
        Self {
            attempt_timeout: Duration::from_secs(300),
            max_attempts: 3,
            scratch_root,
            keep_scratch_on_failure: true,
            profiles,
            agents,
        }
    }
}

/// Errors returned from [`dispatch_triage`] for invariant violations.
///
/// Retryable invocation failures (deadline, agent crash, lost events, bad or
/// missing artifacts) are NOT surfaced here — they retry up to
/// `opts.max_attempts` times and on exhaustion become
/// `Ok(TriageDecision::Unclear { question })`. A provider that is not
/// configured also becomes an `Unclear` with a configuration hint. `TriageError`
/// is reserved for failures that prevent any forward progress.
#[derive(Debug, thiserror::Error)]
pub enum TriageError {
    /// Could not create or write to the per-call scratch directory.
    #[error("scratch dir setup failed: {0}")]
    Scratch(#[from] std::io::Error),
    /// The invocation failed terminally (dead bridge, session open or send
    /// failure, closed stream).
    #[error("triage agent invocation failed: {0}")]
    Invocation(InvocationFailure),
}

/// The Triage Author's template bindings: the fenced input JSON and the
/// previous attempt's feedback (empty on the first attempt).
fn triage_bindings(input: &TriageInput, feedback: Option<&str>) -> Vec<(String, String)> {
    let json = serde_json::to_string_pretty(input).unwrap_or_else(|_| "{}".into());
    vec![
        (
            "triage_input".into(),
            surge_core::untrusted::fence(
                surge_core::untrusted::ContentSource::TrackerTriageInput,
                &json,
            ),
        ),
        ("feedback".into(), feedback.unwrap_or_default().to_owned()),
    ]
}

/// One attempt's result in triage terms.
enum Attempt {
    Decided(TriageDecision),
    /// Retry with this feedback.
    Retry(String),
    /// Stop: the decision to return.
    Stop(Result<TriageDecision, TriageError>),
}

async fn try_one_attempt(
    bridge: &Arc<dyn BridgeFacade>,
    input: &TriageInput,
    opts: &TriageOptions,
    scratch_dir: &std::path::Path,
    feedback: Option<&str>,
) -> Attempt {
    // Every attempt writes its own decision: never read one a previous,
    // rejected attempt left in the shared scratch directory.
    for stale in ["triage_decision.json", "inbox_summary.md"] {
        let _ = std::fs::remove_file(scratch_dir.join(stale));
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let invoked = invoke(
        bridge,
        InvocationRequest {
            profile: TRIAGE_AUTHOR_PROFILE,
            bindings: triage_bindings(input, feedback),
            working_dir: scratch_dir,
            deadline: opts.attempt_timeout,
            cancel: &cancel,
            recorder: None,
            profiles: &opts.profiles,
            agents: opts.agents.as_deref(),
            max_contract_rejections: DEFAULT_MAX_CONTRACT_REJECTIONS,
            seed_agent_settings: true,
        },
    )
    .await;
    let outcome = match invoked {
        Ok(outcome) => outcome,
        Err(failure) if failure.is_retryable() => return Attempt::Retry(failure.to_string()),
        Err(
            failure @ (InvocationFailure::ProfileUnresolved { .. }
            | InvocationFailure::ProviderUnknown { .. }
            | InvocationFailure::ProviderEnvironment { .. }
            | InvocationFailure::Unsupported { .. }
            | InvocationFailure::PromptRender { .. }
            | InvocationFailure::AgentNotStarted(_)),
        ) => {
            // Same operator signal as before: an `Unclear` card naming the
            // configuration problem, never a retry.
            return Attempt::Stop(Ok(TriageDecision::Unclear {
                question: format!(
                    "Triage agent is not configured ({failure}); configure the \
                     triage-author profile's agent to enable LLM-driven triage"
                ),
            }));
        },
        Err(failure) => return Attempt::Stop(Err(TriageError::Invocation(failure))),
    };

    let raw = match outcome.artifact("triage_decision.json") {
        Some(artifact) => artifact.bytes.clone(),
        None => match std::fs::read(scratch_dir.join("triage_decision.json")) {
            Ok(raw) => raw,
            Err(error) => return Attempt::Retry(format!("triage_decision.json missing: {error}")),
        },
    };
    let mut decision = match decision_from_artifact(&raw) {
        Ok(decision) => decision,
        Err(error) => return Attempt::Retry(format!("triage_decision.json rejected: {error}")),
    };

    // For Enqueued, splice in the inbox_summary.md if present.
    if let TriageDecision::Enqueued { summary, .. } = &mut decision
        && summary.is_empty()
        && let Ok(md) = std::fs::read_to_string(scratch_dir.join("inbox_summary.md"))
    {
        *summary = md;
    }

    // The agent's reported outcome must match the parsed decision.
    let decision_kind = match &decision {
        TriageDecision::Enqueued { .. } => "enqueued",
        TriageDecision::Duplicate { .. } => "duplicate",
        TriageDecision::OutOfScope { .. } => "out_of_scope",
        TriageDecision::Unclear { .. } => "unclear",
    };
    if outcome.outcome.as_str() != decision_kind {
        return Attempt::Retry(format!(
            "outcome ({}) mismatch with decision ({decision_kind})",
            outcome.outcome
        ));
    }
    Attempt::Decided(decision)
}

/// Dispatch a Triage Author session against the supplied bridge.
///
/// Returns `Ok(TriageDecision)` even on retry exhaustion (materialises
/// as `Unclear` with a diagnostic question), and for an agent that is not
/// configured (`Unclear` with a configuration hint). `Err(TriageError)` is
/// reserved for invariant violations that prevent any forward progress.
///
/// # Errors
/// - [`TriageError::Scratch`] if the per-call scratch directory
///   cannot be created.
/// - [`TriageError::Invocation`] for a dead bridge or a session that cannot
///   be opened or sent to.
pub async fn dispatch_triage(
    bridge: Arc<dyn BridgeFacade>,
    input: TriageInput,
    opts: TriageOptions,
) -> Result<TriageDecision, TriageError> {
    // Build a fresh scratch dir for this top-level call.
    let scratch_dir = opts.scratch_root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(&scratch_dir)?;

    let mut last_err: Option<String> = None;
    for attempt in 1..=opts.max_attempts {
        match try_one_attempt(&bridge, &input, &opts, &scratch_dir, last_err.as_deref()).await {
            Attempt::Decided(decision) => {
                // Cleanup policy:
                // - Decisive success (Enqueued / Duplicate / OutOfScope): always
                //   remove scratch (best-effort) so the per-call dir doesn't leak.
                // - Unclear (agent-reported): keep iff `keep_scratch_on_failure`,
                //   otherwise remove. Unclear is non-decisive — the operator may
                //   want the artifacts for post-mortem.
                let is_unclear = matches!(decision, TriageDecision::Unclear { .. });
                if !is_unclear || !opts.keep_scratch_on_failure {
                    let _ = std::fs::remove_dir_all(&scratch_dir);
                }
                return Ok(decision);
            },
            Attempt::Stop(result) => {
                if !opts.keep_scratch_on_failure {
                    let _ = std::fs::remove_dir_all(&scratch_dir);
                }
                return result;
            },
            Attempt::Retry(msg) => {
                tracing::warn!(attempt, error = %msg, "triage attempt failed; will retry");
                last_err = Some(msg);
            },
        }
    }

    // Retry exhaustion → synthetic Unclear. Same cleanup rule as agent-reported
    // Unclear: keep artifacts iff `keep_scratch_on_failure`.
    if !opts.keep_scratch_on_failure {
        let _ = std::fs::remove_dir_all(&scratch_dir);
    }
    let question = format!(
        "Triage failed after {} attempts: {}",
        opts.max_attempts,
        last_err.as_deref().unwrap_or("unknown error")
    );
    Ok(TriageDecision::Unclear { question })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide(raw: &str) -> Result<TriageDecision, String> {
        decision_from_artifact(raw.as_bytes())
    }

    #[test]
    fn parse_enqueued() {
        let dec = decide(r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#).unwrap();
        match dec {
            TriageDecision::Enqueued { priority, .. } => assert_eq!(priority, Priority::High),
            other => panic!("expected Enqueued, got {other:?}"),
        }
    }

    #[test]
    fn parse_duplicate_requires_duplicate_of() {
        assert!(
            decide(r#"{"decision":"duplicate"}"#)
                .unwrap_err()
                .contains("duplicate_of")
        );
    }

    #[test]
    fn parse_unknown_priority_errors() {
        let err = decide(r#"{"decision":"enqueued","priority":"super-urgent"}"#).unwrap_err();
        assert!(err.contains("super-urgent"), "{err}");
    }

    #[test]
    fn parse_out_of_scope() {
        let dec = decide(r#"{"decision":"out_of_scope","priority":"low","priority_reasoning":"not a coding task"}"#).unwrap();
        assert!(matches!(dec, TriageDecision::OutOfScope { .. }));
    }

    #[test]
    fn parse_unclear() {
        let dec = decide(
            r#"{"decision":"unclear","priority":"medium","question":"What does this mean?"}"#,
        )
        .unwrap();
        match dec {
            TriageDecision::Unclear { question } => assert_eq!(question, "What does this mean?"),
            other => panic!("expected Unclear, got {other:?}"),
        }
    }

    #[test]
    fn unclear_requires_a_question() {
        assert!(
            decide(r#"{"decision":"unclear"}"#)
                .unwrap_err()
                .contains("question")
        );
    }

    #[test]
    fn parse_unknown_decision_errors() {
        let err = decide(r#"{"decision":"meow","priority":"medium"}"#).unwrap_err();
        assert!(err.contains("meow"), "{err}");
    }

    /// Render the bundled profile's prompt with triage's bindings, exactly
    /// as the invocation does.
    fn render_prompt(input: &TriageInput, feedback: Option<&str>) -> String {
        let profile = surge_core::BundledRegistry::by_name_latest("triage-author").unwrap();
        let values: Vec<_> = super::triage_bindings(input, feedback)
            .into_iter()
            .map(|(name, value)| (surge_core::agent_config::TemplateVar(name), value))
            .collect();
        crate::prompt::PromptRenderer::strict()
            .render(&profile.prompt.system, &values)
            .unwrap()
    }

    #[test]
    fn the_bundled_profile_is_a_standard_registry_profile() {
        let profile = surge_core::BundledRegistry::by_name_latest("triage-author").unwrap();
        let outcomes: Vec<_> = profile.outcomes.iter().map(|o| o.id.to_string()).collect();
        assert_eq!(
            outcomes,
            ["enqueued", "duplicate", "out_of_scope", "unclear"]
        );
        assert_ne!(profile.runtime.agent_id, "");
        assert!(
            profile.hooks.entries.is_empty(),
            "standalone profiles run no hooks"
        );
    }

    #[test]
    fn render_prompt_snapshot() {
        use chrono::TimeZone;
        let task = TaskDetails {
            task_id: TaskId::try_new("github_issues:test/repo#42").unwrap(),
            source_id: "github_issues:test/repo".into(),
            title: "Add tracing to auth middleware".into(),
            description: "We have no observability into the auth flow.".into(),
            status: "open".into(),
            labels: vec!["surge:enabled".into(), "area/auth".into()],
            url: "https://github.com/test/repo/issues/42".into(),
            created_at: chrono::Utc.with_ymd_and_hms(2026, 5, 1, 10, 0, 0).unwrap(),
            updated_at: chrono::Utc.with_ymd_and_hms(2026, 5, 5, 12, 0, 0).unwrap(),
            assignee: None,
            raw_payload: serde_json::json!({}),
        };
        let candidates = vec![TaskSummary {
            task_id: TaskId::try_new("github_issues:test/repo#10").unwrap(),
            title: "Update README".into(),
            status: "open".into(),
            url: "https://github.com/test/repo/issues/10".into(),
            updated_at: chrono::Utc.with_ymd_and_hms(2026, 4, 28, 9, 0, 0).unwrap(),
        }];
        let active_runs = vec![ActiveRunSummary {
            run_id: "01HXX0000000000000000RUN1".into(),
            task_id: Some("github_issues:test/repo#9".into()),
            status: "Running".into(),
            started_at: "2026-05-06T10:00:00Z".into(),
        }];
        let input = TriageInput {
            task,
            candidates,
            active_runs,
        };
        let rendered = render_prompt(&input, None);
        insta::assert_snapshot!("triage_initial_prompt", rendered);
    }

    #[test]
    fn render_prompt_with_feedback_includes_feedback_block() {
        let task = TaskDetails {
            task_id: TaskId::try_new("github_issues:t/r#1").unwrap(),
            source_id: "github_issues:t/r".into(),
            title: "x".into(),
            description: "y".into(),
            status: "open".into(),
            labels: vec![],
            url: "https://x".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            assignee: None,
            raw_payload: serde_json::json!({}),
        };
        let input = TriageInput {
            task,
            candidates: vec![],
            active_runs: vec![],
        };
        let rendered = render_prompt(&input, Some("previous attempt malformed JSON"));
        assert!(rendered.contains("# Feedback from previous attempt"));
        assert!(rendered.contains("previous attempt malformed JSON"));
    }
}
