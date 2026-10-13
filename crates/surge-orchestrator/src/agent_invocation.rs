//! Standalone agent invocation (#99): run one agent profile outside a flow
//! graph.
//!
//! In plain terms: **run profile X with bindings Y in working directory Z,
//! under deadline D and cancellation C, and get back the reported outcome plus
//! the produced artifacts, or a typed failure.** Triage, project context and
//! the feature planner are thin callers: each renders its own inputs, calls
//! [`invoke`], and interprets the outcome in its own domain terms. Retry
//! policy stays with the caller that wants it ([`InvocationFailure::is_retryable`]).
//!
//! # What the module owns
//! - Profile resolution through the profile registry, and provider resolution
//!   through the unified agent registry (ADR-0019): user `[agents.*]` entries
//!   over builtins, launched by the entry's shape with its env and settings
//!   files. No per-vendor branch.
//! - The session, through the [`BridgeFacade`] seam, with the sandbox
//!   delegated to the agent (`AlwaysAllowSandbox`).
//! - Outcome validation against the profile's declared outcomes, and
//!   produced-artifact contract validation (the #94 outcome-acceptance rule),
//!   rejected and retried **inside the same session** under a bounded budget.
//!
//! # Invariants
//! - The event subscription precedes `open_session`: no event of the new
//!   session can be missed.
//! - Only events of the opened session are considered.
//! - A broadcast lag is a retryable [`InvocationFailure::EventsLost`], never a
//!   silent skip.
//! - Once `open_session` succeeds, `close_session` is called **exactly once**
//!   on every exit path. A close error is logged and never masks the result.
//! - The deadline covers the whole invocation, from the first send to the
//!   accepted outcome.
//! - Cancellation wins unless an outcome was already accepted.
//! - Artifacts are read only after the outcome is accepted, and only inside
//!   the working directory.
//!
//! # Not run here
//! Tool dispatch, human input, MCP servers, skills and hooks. A profile that
//! declares hooks, MCP servers or skills is refused with
//! [`InvocationFailure::Unsupported`]: never run with them silently skipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::{BridgeEvent, SessionEndReason};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig};
use surge_core::agent_config::TemplateVar;
use surge_core::content_hash::ContentHash;
use surge_core::id::SessionId;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::profile::keyref::parse_key_ref;
use surge_core::profile::registry::ResolvedProfile;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::run_writer::RunWriter;
use tokio_util::sync::CancellationToken;

use crate::engine::stage::agent::{AgentLaunchError, resolve_agent_launch};
use crate::engine::stage::outcome_acceptance::{
    validate_profile_artifact_contracts, validation_retry_prompt,
};
use crate::profile_loader::ProfileRegistry;
use crate::prompt::PromptRenderer;

/// Default number of contract rejections one invocation absorbs before it
/// fails: the agent gets this many corrected turns in the same session.
pub const DEFAULT_MAX_CONTRACT_REJECTIONS: u32 = 2;

/// Where an invocation records its evidence: a caller-owned run log and the
/// node its events are attributed to.
#[derive(Clone, Copy)]
pub struct InvocationRecorder<'a> {
    /// The caller's run writer.
    pub writer: &'a RunWriter,
    /// The node the events belong to.
    pub node: &'a NodeKey,
}

/// One standalone invocation.
pub struct InvocationRequest<'a> {
    /// Profile key reference, e.g. `feature-planner@1.0`.
    pub profile: &'a str,
    /// Template bindings: variable name → value.
    pub bindings: Vec<(String, String)>,
    /// The agent's working directory; produced artifacts must stay inside.
    pub working_dir: &'a Path,
    /// Covers the whole invocation, from the first send to the outcome.
    pub deadline: Duration,
    /// Stops the invocation and closes the session.
    pub cancel: &'a CancellationToken,
    /// Optional run-log evidence (outcome and produced artifacts).
    pub recorder: Option<InvocationRecorder<'a>>,
    /// The profile registry to resolve `profile` through.
    pub profiles: &'a ProfileRegistry,
    /// The operator's merged agent registry; `None` uses the builtins.
    pub agents: Option<&'a surge_acp::Registry>,
    /// How many contract rejections the agent may correct in-session.
    pub max_contract_rejections: u32,
}

/// One produced artifact, read after the outcome was accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducedArtifact {
    /// Path relative to the working directory.
    pub path: PathBuf,
    /// Hash of [`Self::bytes`].
    pub hash: ContentHash,
    /// The artifact's bytes.
    pub bytes: Vec<u8>,
}

/// An accepted invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationOutcome {
    /// One of the profile's declared outcomes.
    pub outcome: OutcomeKey,
    /// The agent's summary.
    pub summary: String,
    /// The produced artifacts, each valid against its declared contract.
    pub artifacts: Vec<ProducedArtifact>,
}

impl InvocationOutcome {
    /// The produced artifact at `path` (relative to the working directory).
    #[must_use]
    pub fn artifact(&self, path: &str) -> Option<&ProducedArtifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.path == Path::new(path))
    }
}

/// Why an invocation produced no accepted outcome.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvocationFailure {
    /// The profile reference does not resolve.
    #[error("profile {profile:?} does not resolve: {reason}")]
    ProfileUnresolved {
        /// The profile reference.
        profile: String,
        /// Why.
        reason: String,
    },
    /// The profile's agent id is not in the agent registry.
    #[error("agent {agent_id:?} is not configured; known ids and aliases: {known}")]
    ProviderUnknown {
        /// The unknown agent id.
        agent_id: String,
        /// The known ids and aliases.
        known: String,
    },
    /// The provider's environment cannot be resolved.
    #[error("agent {agent_id:?} cannot resolve its environment: {reason}")]
    ProviderEnvironment {
        /// The agent id.
        agent_id: String,
        /// The missing variable, by name only.
        reason: String,
    },
    /// The profile declares capabilities a standalone invocation never runs.
    #[error("profile {profile:?} declares {what}, which a standalone invocation does not run")]
    Unsupported {
        /// The profile reference.
        profile: String,
        /// What it declares (hooks, MCP servers, skills).
        what: &'static str,
    },
    /// The prompt template could not be rendered with the bindings.
    #[error("prompt for {profile:?} does not render: {reason}")]
    PromptRender {
        /// The profile reference.
        profile: String,
        /// Why.
        reason: String,
    },
    /// The session could not be opened.
    #[error("session open failed: {0}")]
    SessionOpen(String),
    /// A message could not be sent.
    #[error("send failed: {0}")]
    Send(String),
    /// The deadline passed before an outcome was accepted.
    #[error("timeout: no outcome within {0:?}")]
    DeadlineElapsed(Duration),
    /// The caller cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The agent crashed.
    #[error("agent crashed: {0}")]
    AgentCrashed(String),
    /// The session ended before an outcome.
    #[error("session ended without an outcome: {0}")]
    SessionEnded(String),
    /// The bridge's event queue dropped events for this session.
    #[error("bridge events lost ({0} skipped)")]
    EventsLost(u64),
    /// The bridge's event stream closed.
    #[error("bridge event stream closed")]
    StreamClosed,
    /// The agent reported an outcome its profile does not declare.
    #[error("agent reported undeclared outcome {0:?}")]
    UndeclaredOutcome(String),
    /// A reported artifact does not exist.
    #[error("produced artifact {0} is missing")]
    ArtifactMissing(String),
    /// A reported artifact resolves outside the working directory.
    #[error("produced artifact {0} is outside the working directory")]
    ArtifactOutsideWorkingDir(String),
    /// Produced artifacts stayed invalid after the in-session retries.
    #[error("produced artifacts are invalid: {0}")]
    ArtifactContract(String),
    /// Recording evidence in the caller's run log failed.
    #[error("recording invocation evidence failed: {0}")]
    Record(String),
}

impl InvocationFailure {
    /// Whether another attempt may succeed: deadline, crash, early end, lost
    /// events, invalid or missing artifacts, and outcome mistakes are
    /// retryable; configuration, a dead bridge, cancellation and recording
    /// failures are terminal.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::DeadlineElapsed(_)
                | Self::AgentCrashed(_)
                | Self::SessionEnded(_)
                | Self::EventsLost(_)
                | Self::UndeclaredOutcome(_)
                | Self::ArtifactMissing(_)
                | Self::ArtifactOutsideWorkingDir(_)
                | Self::ArtifactContract(_)
        )
    }
}

impl From<AgentLaunchError> for InvocationFailure {
    fn from(error: AgentLaunchError) -> Self {
        match error {
            AgentLaunchError::EmptyAgentId { profile } => Self::ProfileUnresolved {
                profile,
                reason: "empty runtime.agent_id".into(),
            },
            AgentLaunchError::UnknownAgent {
                agent_id, known, ..
            } => Self::ProviderUnknown { agent_id, known },
            AgentLaunchError::Environment {
                agent_id, source, ..
            } => Self::ProviderEnvironment {
                agent_id,
                reason: source.to_string(),
            },
        }
    }
}

/// Run one profile standalone. See the module doc for the invariants.
///
/// # Errors
/// A typed [`InvocationFailure`].
#[must_use = "an invocation's outcome must be interpreted by its caller"]
pub async fn invoke(
    bridge: &Arc<dyn BridgeFacade>,
    request: InvocationRequest<'_>,
) -> Result<InvocationOutcome, InvocationFailure> {
    let profile = resolve_profile(&request)?;
    let config = session_config(&request, &profile)?;
    let prompt = config.system_prompt.clone();

    // Subscribe before opening: no event of the new session can be missed.
    let mut events = bridge.subscribe();
    let session = bridge
        .open_session(config)
        .await
        .map_err(|error| InvocationFailure::SessionOpen(error.to_string()))?
        .session;
    tracing::debug!(target: "agent_invocation", profile = request.profile, %session, "session opened");

    let result = drive(bridge, &request, &profile, session, &mut events, prompt).await;
    // Exactly once, on every path after a successful open.
    if let Err(error) = bridge.close_session(session).await {
        tracing::warn!(target: "agent_invocation", %session, %error, "session close failed");
    }
    match &result {
        Ok(outcome) => tracing::info!(target: "agent_invocation", profile = request.profile,
            outcome = %outcome.outcome, artifacts = outcome.artifacts.len(), "invocation accepted"),
        Err(failure) => tracing::warn!(target: "agent_invocation", profile = request.profile,
            %failure, retryable = failure.is_retryable(), "invocation failed"),
    }
    result
}

fn resolve_profile(request: &InvocationRequest<'_>) -> Result<ResolvedProfile, InvocationFailure> {
    let unresolved = |reason: String| InvocationFailure::ProfileUnresolved {
        profile: request.profile.to_owned(),
        reason,
    };
    let key = parse_key_ref(request.profile).map_err(|error| unresolved(error.to_string()))?;
    let resolved = request
        .profiles
        .resolve(&key)
        .map_err(|error| unresolved(error.to_string()))?;
    let unsupported = |what| InvocationFailure::Unsupported {
        profile: request.profile.to_owned(),
        what,
    };
    if !resolved.profile.hooks.entries.is_empty() {
        return Err(unsupported("hooks"));
    }
    if !resolved.profile.tools.default_mcp.is_empty() {
        return Err(unsupported("MCP servers"));
    }
    if !resolved.profile.tools.default_skills.is_empty() {
        return Err(unsupported("skills"));
    }
    Ok(resolved)
}

fn session_config(
    request: &InvocationRequest<'_>,
    profile: &ResolvedProfile,
) -> Result<SessionConfig, InvocationFailure> {
    let launch = resolve_agent_launch(
        request.profile,
        &profile.profile.runtime.agent_id,
        request.agents,
    )?;
    if !launch.settings_files.is_empty() {
        surge_acp::settings_seed::seed_settings_files(&launch.settings_files, request.working_dir);
    }
    let runtime = crate::engine::capacity::run_agent_registry_or_builtin(request.agents)
        .normalize_agent_id(&profile.profile.runtime.agent_id)
        .unwrap_or_else(|| profile.profile.runtime.agent_id.clone());
    let values: Vec<(TemplateVar, String)> = request
        .bindings
        .iter()
        .map(|(name, value)| (TemplateVar(name.clone()), value.clone()))
        .collect();
    let prompt = PromptRenderer::strict()
        .render(&profile.profile.prompt.system, &values)
        .map_err(|error| InvocationFailure::PromptRender {
            profile: request.profile.to_owned(),
            reason: error.to_string(),
        })?;
    Ok(SessionConfig {
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime,
        opening: Default::default(),
        config_selections: Vec::new(),
        stage_mcp: None,
        agent_kind: launch.kind,
        working_dir: request.working_dir.to_path_buf(),
        system_prompt: prompt,
        declared_outcomes: profile
            .profile
            .outcomes
            .iter()
            .map(|outcome| outcome.id.clone())
            .collect(),
        allows_escalation: false,
        tools: Vec::new(),
        // Sandbox-delegated: the agent enforces its own sandbox; the
        // profile's mode is advisory to it.
        sandbox: Box::new(surge_acp::bridge::sandbox::AlwaysAllowSandbox),
        permission_policy: surge_acp::client::PermissionPolicy::default(),
        bindings: request.bindings.iter().cloned().collect::<BTreeMap<_, _>>(),
        env: launch.env,
    })
}

/// Send the prompt and wait for an accepted outcome; never closes the session.
async fn drive(
    bridge: &Arc<dyn BridgeFacade>,
    request: &InvocationRequest<'_>,
    profile: &ResolvedProfile,
    session: SessionId,
    events: &mut tokio::sync::broadcast::Receiver<BridgeEvent>,
    prompt: String,
) -> Result<InvocationOutcome, InvocationFailure> {
    let deadline = tokio::time::sleep(request.deadline);
    tokio::pin!(deadline);
    let mut rejections = 0_u32;
    let mut message = prompt;
    loop {
        tokio::select! {
            biased;
            () = request.cancel.cancelled() => return Err(InvocationFailure::Cancelled),
            () = &mut deadline => return Err(InvocationFailure::DeadlineElapsed(request.deadline)),
            sent = bridge.send_message(session, MessageContent::Text(message.clone())) => {
                sent.map_err(|error| InvocationFailure::Send(error.to_string()))?;
            },
        }
        let (outcome, summary, produced) = loop {
            let event = tokio::select! {
                biased;
                () = request.cancel.cancelled() => return Err(InvocationFailure::Cancelled),
                () = &mut deadline => return Err(InvocationFailure::DeadlineElapsed(request.deadline)),
                event = events.recv() => match event {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        return Err(InvocationFailure::EventsLost(skipped));
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(InvocationFailure::StreamClosed);
                    },
                },
            };
            match event {
                BridgeEvent::OutcomeReported {
                    session: of,
                    outcome,
                    summary,
                    artifacts_produced,
                    ..
                } if of == session => break (outcome, summary, artifacts_produced),
                BridgeEvent::SessionEnded {
                    session: of,
                    reason,
                } if of == session => {
                    return Err(match reason {
                        SessionEndReason::AgentCrashed { .. } => {
                            InvocationFailure::AgentCrashed(format!("{reason:?}"))
                        },
                        other => InvocationFailure::SessionEnded(format!("{other:?}")),
                    });
                },
                _ => {},
            }
        };
        if !profile
            .profile
            .outcomes
            .iter()
            .any(|declared| declared.id == outcome)
        {
            return Err(InvocationFailure::UndeclaredOutcome(outcome.to_string()));
        }
        // The #94 contract rule, retried in this session under a budget.
        match validate_profile_artifact_contracts(
            Some(profile),
            request.profile,
            &outcome,
            &produced,
            request.working_dir,
            false,
        )
        .await
        .map_err(|error| InvocationFailure::ArtifactContract(error.to_string()))?
        {
            None => {
                let artifacts = read_artifacts(request.working_dir, &produced)?;
                record(request, &outcome, &summary, &artifacts).await?;
                return Ok(InvocationOutcome {
                    outcome,
                    summary,
                    artifacts,
                });
            },
            Some(rejection) => {
                let feedback = format!("{}: {}", rejection.hook_id, rejection.reason);
                rejections += 1;
                tracing::debug!(target: "agent_invocation", %feedback, rejections, "outcome rejected");
                if rejections > request.max_contract_rejections {
                    return Err(InvocationFailure::ArtifactContract(feedback));
                }
                message = validation_retry_prompt(&feedback);
            },
        }
    }
}

/// Read each produced artifact, refusing anything outside the working dir.
fn read_artifacts(
    working_dir: &Path,
    produced: &[String],
) -> Result<Vec<ProducedArtifact>, InvocationFailure> {
    let root = working_dir.canonicalize().map_err(|error| {
        InvocationFailure::ArtifactMissing(format!("{}: {error}", working_dir.display()))
    })?;
    produced
        .iter()
        .map(|reported| {
            let candidate = root.join(reported);
            let resolved = candidate
                .canonicalize()
                .map_err(|_| InvocationFailure::ArtifactMissing(reported.clone()))?;
            let relative = resolved
                .strip_prefix(&root)
                .map_err(|_| InvocationFailure::ArtifactOutsideWorkingDir(reported.clone()))?
                .to_path_buf();
            let bytes = std::fs::read(&resolved)
                .map_err(|_| InvocationFailure::ArtifactMissing(reported.clone()))?;
            Ok(ProducedArtifact {
                path: relative,
                hash: ContentHash::compute(&bytes),
                bytes,
            })
        })
        .collect()
}

async fn record(
    request: &InvocationRequest<'_>,
    outcome: &OutcomeKey,
    summary: &str,
    artifacts: &[ProducedArtifact],
) -> Result<(), InvocationFailure> {
    let Some(recorder) = request.recorder else {
        return Ok(());
    };
    let mut events: Vec<VersionedEventPayload> = artifacts
        .iter()
        .map(|artifact| {
            VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: recorder.node.clone(),
                artifact: artifact.hash,
                path: artifact.path.clone(),
                name: artifact
                    .path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_owned(),
                source_path: None,
            })
        })
        .collect();
    events.push(VersionedEventPayload::new(EventPayload::OutcomeReported {
        node: recorder.node.clone(),
        outcome: outcome.clone(),
        summary: summary.to_owned(),
    }));
    recorder
        .writer
        .append_events(events)
        .await
        .map(|_| ())
        .map_err(|error| InvocationFailure::Record(error.to_string()))
}
