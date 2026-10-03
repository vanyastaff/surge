//! Host-only task ownership and immutable startup hydration.
use super::{EngineError, EngineRunConfig};
use surge_core::{ContentHash, EventPayload, work_item::WorkItemContext};
use surge_persistence::{runs::Storage, work_items::WorkItemLaunchClaim};

pub(super) fn pinned(
    storage: &Storage,
    claim: &WorkItemLaunchClaim,
) -> Result<
    (
        surge_core::work_item::WorkItemAttempt,
        surge_core::work_item::WorkItemWorkspace,
        WorkItemContext,
        EngineRunConfig,
    ),
    EngineError,
> {
    let store = storage.work_items();
    let attempt = store
        .validate_claim(claim)
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    let detail = store
        .show(attempt.item)
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    let accepted = store
        .requirements(&attempt)
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    let context = WorkItemContext::new(attempt.binding.clone(), accepted.requirements)
        .map_err(EngineError::Internal)?;
    let mut config: EngineRunConfig = serde_json::from_str(&attempt.config)
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    config.initial_prompt = context.prompt();
    Ok((attempt, detail.item.workspace, context, config))
}
pub(super) async fn validate_resume(
    storage: &std::sync::Arc<Storage>,
    claim: &WorkItemLaunchClaim,
) -> Result<std::path::PathBuf, EngineError> {
    let (attempt, workspace, context, config) = pinned(storage, claim)?;
    let inspected = storage
        .inspect_folded_run(attempt.run)
        .await
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    let Some(history) = inspected.database else {
        return Err(EngineError::Storage("task journal absent".into()));
    };
    let events = history.startup;
    if !events.first().is_some_and(|event| {
        event.seq.0 == 1 && matches!(event.payload.payload, EventPayload::RunStarted { .. })
    }) {
        return Err(EngineError::Storage(
            "task journal origin/discontinuity".into(),
        ));
    }
    let legacy_unbound_cap = events[0].payload.schema_version < 13
        && matches!(&events[0].payload.payload, EventPayload::RunStarted { config, .. } if config.bootstrap_edit_loop_cap.is_none());
    let expected_config = surge_core::run_event::RunConfig {
        bootstrap_edit_loop_cap: if legacy_unbound_cap {
            None
        } else {
            Some(config.bootstrap.edit_loop_cap)
        },
        sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
        approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: config.mcp_servers.clone(),
        budget: config.budget,
    };
    if !matches!(&events[0].payload.payload,EventPayload::RunStarted{project_path,initial_prompt,config,..} if project_path==&workspace.path && initial_prompt==&context.prompt() && config==&expected_config)
    {
        return Err(EngineError::Storage(
            "task startup frozen input mismatch".into(),
        ));
    }
    if events
        .iter()
        .filter(|event| {
            matches!(
                event.payload.payload,
                EventPayload::WorkItemAttemptBound { .. }
            )
        })
        .count()
        != 1
    {
        return Err(EngineError::Storage(
            "task startup binding is not unique".into(),
        ));
    }
    let bound = events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::WorkItemAttemptBound { context } => Some(context),
            _ => None,
        });
    if bound != Some(&context) {
        return Err(EngineError::Storage("task journal binding mismatch".into()));
    }
    validate_graph(&events, &attempt.graph)?;
    let path = events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::ArtifactProduced {
                artifact,
                path,
                name,
                ..
            } if name == "accepted_requirements"
                && *artifact == context.binding().requirements_hash =>
            {
                Some(path)
            },
            _ => None,
        })
        .ok_or_else(|| EngineError::Storage("task requirements artifact missing".into()))?;
    let bytes = std::fs::read(path).map_err(|error| EngineError::Storage(error.to_string()))?;
    if ContentHash::compute(&bytes) != context.binding().requirements_hash {
        return Err(EngineError::Storage(
            "task accepted requirement bytes mismatch".into(),
        ));
    }
    validate_prompt(&events, &workspace.path, &context)?;
    Ok(workspace.path)
}

fn validate_prompt(
    events: &[surge_persistence::runs::ReadEvent],
    workspace: &std::path::Path,
    context: &WorkItemContext,
) -> Result<(), EngineError> {
    let prompt_hash = ContentHash::compute(context.prompt().as_bytes());
    let prompt_path = events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::ArtifactProduced {
                artifact,
                path,
                name,
                ..
            } if name == "user_prompt"
                && *artifact == prompt_hash
                && path == std::path::Path::new(".surge/user_prompt.txt") =>
            {
                Some(path)
            },
            _ => None,
        })
        .ok_or_else(|| {
            EngineError::Storage("task startup user_prompt artifact missing or mismatched".into())
        })?;
    let prompt_bytes = std::fs::read(workspace.join(prompt_path))
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    if ContentHash::compute(&prompt_bytes) != prompt_hash {
        return Err(EngineError::Storage(
            "task startup user_prompt bytes mismatch".into(),
        ));
    }
    Ok(())
}

fn validate_graph(
    events: &[surge_persistence::runs::ReadEvent],
    frozen: &surge_core::Graph,
) -> Result<(), EngineError> {
    let expected = ContentHash::compute(
        &serde_json::to_vec(frozen).map_err(|error| EngineError::Internal(error.to_string()))?,
    );
    let actual = events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::PipelineMaterialized { graph, graph_hash } => Some((graph, graph_hash)),
            _ => None,
        })
        .ok_or_else(|| EngineError::Storage("task frozen graph missing from startup".into()))?;
    let payload_hash = ContentHash::compute(
        &serde_json::to_vec(actual.0).map_err(|error| EngineError::Internal(error.to_string()))?,
    );
    if payload_hash != *actual.1 || payload_hash != expected {
        return Err(EngineError::Storage(
            "task frozen graph payload/hash mismatch".into(),
        ));
    }
    Ok(())
}
