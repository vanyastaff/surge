//! Ordinary Flow owner endpoint. All external preparation stays under one worker owner.
use crate::{
    admission::AdmissionController, broadcast::BroadcastRegistry, tracked_run::TrackingContext,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use surge_core::{
    Graph,
    work_item::{AcceptedFlowContract, OwnedFlowMcpSelection, OwnedFlowReceipt},
};
use surge_orchestrator::engine::owned_flow::{
    FlowInput, McpSelection, OwnedFlowStart, WorkspaceRequest,
};
use surge_persistence::work_items::{
    OwnedFlowCapturedSource, OwnedFlowPreparation, OwnedFlowPreparationResult,
    OwnedFlowSourceSnapshot, WorkItemError,
};

pub(crate) async fn execute(
    request: &OwnedFlowStart,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
) -> Result<OwnedFlowReceipt, WorkItemError> {
    let request = request
        .clone()
        .normalize()
        .map_err(|_| WorkItemError::Invalid("invalid owned Flow request".into()))?;
    let body = request
        .canonical_bytes()
        .map_err(|_| WorkItemError::Invalid("invalid owned Flow request encoding".into()))?;
    let explicit_private =
        matches!(&request.config.mcp,McpSelection::Explicit(servers) if !servers.is_empty());
    let (engine, storage) = tracking
        .task_sources()
        .ok_or_else(|| WorkItemError::Invalid("owned Flow host unavailable".into()))?;
    let store = storage.work_items();
    if let Some(receipt) = store.replay_owned_flow(request.operation_id, &body, explicit_private)? {
        return Ok(receipt);
    }
    let runtime = tokio::runtime::Handle::current();
    let preparing_engine = engine.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        let preparation = store.begin_owned_flow(
            request.operation_id,
            &body,
            explicit_private,
            chrono::Utc::now().timestamp_millis(),
        )?;
        let OwnedFlowPreparationResult::Preparing(mut preparation) = preparation else {
            return Ok(preparation);
        };
        prepare_source(&request, &mut preparation, storage.home())?;
        let source = preparation
            .source_snapshot()?
            .ok_or_else(|| WorkItemError::Invalid("owned Flow source not frozen".into()))?;
        let startup = preparation.startup_snapshot()?;
        let phase = if startup.is_some() {
            surge_git::run_worktree::ReconcilePhase::AfterExecution
        } else {
            surge_git::run_worktree::ReconcilePhase::BeforeExecution
        };
        surge_git::task_workspace::prepare(&source.workspace, phase).map_err(|_| {
            WorkItemError::Invalid("owned Flow workspace preparation refused".into())
        })?;
        if startup.is_none() {
            let host = surge_core::SurgeConfig::discover_from(&source.workspace.checkout).map_err(
                |_| WorkItemError::Invalid("invalid owned Flow host configuration".into()),
            )?;
            let config =
                request
                    .config
                    .materialize(&source.workspace.path, &host, host.mcp_servers.clone());
            let mut frozen = runtime
                .block_on(preparing_engine.freeze_work_item_config(
                    source.contract.graph(),
                    config,
                    &source.workspace.path,
                ))
                .map_err(|_| {
                    WorkItemError::Invalid("owned Flow graph or host configuration refused".into())
                })?;
            let servers = std::mem::take(&mut frozen.mcp_servers);
            let selection = match request.config.mcp {
                McpSelection::HostDefault => OwnedFlowMcpSelection::HostDefault,
                McpSelection::Explicit(_) => OwnedFlowMcpSelection::Explicit,
            };
            preparation.freeze_startup(&serde_json::to_string(&frozen)?, selection, &servers)?;
        }
        preparation.retain_launch_ownership()?;
        Ok::<_, WorkItemError>(OwnedFlowPreparationResult::Preparing(preparation))
    })
    .await
    .map_err(|_| WorkItemError::Invalid("owned Flow preparation worker failed".into()))??;
    let OwnedFlowPreparationResult::Preparing(preparation) = prepared else {
        let OwnedFlowPreparationResult::Replay(receipt) = prepared else {
            return Err(WorkItemError::Invalid(
                "owned Flow preparation result invalid".into(),
            ));
        };
        return Ok(receipt);
    };
    // Acceptance happens only in the awaiting owner after the blocking worker
    // returned with both guards. Cancellation before this point abandons, never accepts.
    let (receipt, claim) = preparation
        .finalize(chrono::Utc::now().timestamp_millis())?
        .into_parts();
    let tracking = tracking.clone();
    let admission = admission.clone();
    let broadcasts = broadcasts.clone();
    tokio::spawn(async move {
        if let Err(error) =
            crate::work_items::launch_owned_flow(claim, &tracking, &admission, &broadcasts).await
        {
            tracing::warn!(%error,"owned Flow launch remains subject to durable reconciliation");
        }
    });
    Ok(receipt)
}
fn prepare_source(
    request: &OwnedFlowStart,
    preparation: &mut OwnedFlowPreparation,
    home: &Path,
) -> Result<(), WorkItemError> {
    if preparation.source_snapshot()?.is_some() {
        return Ok(());
    }
    let (graph, captured, base) = resolve_graph(request, preparation, home)?;
    let contract =
        AcceptedFlowContract::new(Box::new(graph), request.config.initial_prompt.clone())
            .map_err(|_| WorkItemError::Invalid("invalid accepted Flow graph or prompt".into()))?;
    let workspace = match &request.workspace {
        WorkspaceRequest::Managed => surge_git::task_workspace::plan(
            &request.source_project,
            &home.join("work-items/workspaces"),
            preparation.workspace_owner(),
        ),
        WorkspaceRequest::Explicit { path } => surge_git::task_workspace::plan_at_path(
            &request.source_project,
            path,
            preparation.workspace_owner(),
        ),
    }
    .map_err(|_| WorkItemError::Invalid("owned Flow source or workspace intent refused".into()))?;
    let source_base = base.unwrap_or_else(|| workspace.checkout.clone());
    preparation.freeze_source(
        &OwnedFlowSourceSnapshot {
            contract,
            source_base,
            workspace,
        },
        captured.as_ref(),
    )
}
fn resolve_graph(
    request: &OwnedFlowStart,
    preparation: &OwnedFlowPreparation,
    home: &Path,
) -> Result<(Graph, Option<OwnedFlowCapturedSource>, Option<PathBuf>), WorkItemError> {
    match &request.input {
        FlowInput::Inline { graph } => Ok((*graph.clone(), None, None)),
        FlowInput::ProjectFile { locator } => {
            let captured = preparation.capture_source(&request.source_project, locator)?;
            Ok((captured.graph().clone(), Some(captured), None))
        },
        FlowInput::Template { key } => resolve_template(key, preparation, &home.join("templates")),
    }
}
fn resolve_template(
    key: &str,
    preparation: &OwnedFlowPreparation,
    root: &Path,
) -> Result<(Graph, Option<OwnedFlowCapturedSource>, Option<PathBuf>), WorkItemError> {
    let canonical = surge_core::BundledFlows::canonical_name(key);
    let mut paths = Vec::new();
    match std::fs::read_dir(root) {
        Ok(entries) => {
            for (index, entry) in entries.take(2049).enumerate() {
                let entry = entry.map_err(|_| {
                    WorkItemError::Invalid("owned Flow template directory refused".into())
                })?;
                if index >= 2048 {
                    return Err(WorkItemError::Invalid(
                        "owned Flow template directory oversized".into(),
                    ));
                }
                if entry.path().extension().is_some_and(|ext| ext == "toml") {
                    paths.push(entry.file_name());
                }
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => {
            return Err(WorkItemError::Invalid(
                "owned Flow template directory refused".into(),
            ));
        },
    }
    paths.sort();
    let mut alias = None;
    for path in paths {
        let Ok(captured) = preparation.capture_source(root, Path::new(&path)) else {
            continue;
        };
        let stem = Path::new(&path)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let graph = captured.graph();
        if graph.metadata.name == key || stem == key {
            let base = captured.base_path().to_path_buf();
            return Ok((graph.clone(), Some(captured), Some(base)));
        }
        if alias.is_none() && (graph.metadata.name == canonical || stem == canonical) {
            alias = Some(captured);
        }
    }
    if let Some(captured) = alias {
        let base = captured.base_path().to_path_buf();
        return Ok((captured.graph().clone(), Some(captured), Some(base)));
    }
    if let Some(template) = surge_core::BundledFlows::by_name_latest(key) {
        return Ok((template.graph, None, None));
    }
    Err(WorkItemError::Invalid(
        "owned Flow template unavailable".into(),
    ))
}

pub(crate) async fn resume(
    request_id: u64,
    attempt: &surge_core::work_item::WorkItemAttempt,
    path: &Path,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
) -> surge_orchestrator::engine::ipc::DaemonResponse {
    use surge_orchestrator::engine::ipc::{DaemonResponse, ErrorCode};
    let result = async {
        let (_, storage) = tracking
            .task_sources()
            .ok_or_else(|| WorkItemError::Invalid("owned run host unavailable".into()))?;
        let detail = storage.work_items().show(attempt.item)?;
        if path != detail.item.workspace.path {
            return Err(WorkItemError::Conflict(
                "owned Resume requires its retained workspace".into(),
            ));
        }
        let command = surge_core::work_item::WorkItemCommand::Continue {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item: attempt.item,
            expected_version: detail.item.version,
            new_session: false,
        };
        crate::work_items::execute(&command, tracking, admission, broadcasts)
            .await
            .map_err(|(_, error)| error)?;
        Ok::<_, WorkItemError>(())
    }
    .await;
    match result {
        Ok(()) => DaemonResponse::ResumeRunOk { request_id },
        Err(error) => DaemonResponse::Error {
            request_id,
            code: ErrorCode::WorkItemRejected,
            message: error.to_string(),
        },
    }
}
pub(crate) async fn stop(
    request_id: u64,
    attempt: &surge_core::work_item::WorkItemAttempt,
    tracking: &TrackingContext,
    admission: &Arc<AdmissionController>,
    broadcasts: &Arc<BroadcastRegistry>,
) -> surge_orchestrator::engine::ipc::DaemonResponse {
    use surge_orchestrator::engine::ipc::{DaemonResponse, ErrorCode};
    let result = async {
        let (_, storage) = tracking
            .task_sources()
            .ok_or_else(|| WorkItemError::Invalid("owned run host unavailable".into()))?;
        let store = storage.work_items();
        if store.cancel_pending_owned_flow(attempt.run)? || !attempt.state.is_active() {
            return Ok(());
        }
        let detail = store.show(attempt.item)?;
        let command = surge_core::work_item::WorkItemCommand::Suspend {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item: attempt.item,
            expected_version: detail.item.version,
        };
        crate::work_items::execute(&command, tracking, admission, broadcasts)
            .await
            .map_err(|(_, error)| error)?;
        Ok::<_, WorkItemError>(())
    }
    .await;
    match result {
        Ok(()) => DaemonResponse::StopRunOk { request_id },
        Err(error) => DaemonResponse::Error {
            request_id,
            code: ErrorCode::WorkItemRejected,
            message: error.to_string(),
        },
    }
}
