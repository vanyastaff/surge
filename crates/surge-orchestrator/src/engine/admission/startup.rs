//! Startup events a fresh or task-started run appends before its first stage
//! (#97: moved out of `Engine` into run admission unchanged).

use std::path::{Path, PathBuf};

use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::run_event::{EventPayload, VersionedEventPayload};

use crate::engine::config::EngineRunConfig;
use crate::engine::error::EngineError;

pub(super) async fn build_startup_events(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    graph: &Graph,
    worktree_path: &Path,
    run_config: &EngineRunConfig,
) -> Result<Vec<VersionedEventPayload>, EngineError> {
    let mut events = startup_run_events(graph, worktree_path, run_config)?;
    events.extend(
        collect_startup_artifact_events(artifact_store, run_id, worktree_path, run_config).await?,
    );
    Ok(events)
}

fn startup_run_events(
    graph: &Graph,
    worktree_path: &Path,
    run_config: &EngineRunConfig,
) -> Result<Vec<VersionedEventPayload>, EngineError> {
    use surge_core::approvals::ApprovalPolicy;
    use surge_core::run_event::RunConfig as CoreRunConfig;
    use surge_core::sandbox::SandboxMode;

    let graph_hash = graph
        .identity()
        .map_err(|e| EngineError::Internal(e.to_string()))?;
    let core_run_config = CoreRunConfig {
        bootstrap_edit_loop_cap: Some(run_config.bootstrap.edit_loop_cap),
        sandbox_default: SandboxMode::WorkspaceWrite,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: if run_config.owned_flow_inputs.is_some() {
            Vec::new()
        } else {
            run_config.mcp_servers.clone()
        },
        // Persist the frozen budget so a daemon-restart resume re-arms spend
        // enforcement instead of reverting to the unlimited default.
        budget: run_config.budget,
    };

    let mut events = vec![
        VersionedEventPayload::new(EventPayload::RunStarted {
            pipeline_template: None,
            project_path: worktree_path.to_path_buf(),
            initial_prompt: run_config.initial_prompt.clone(),
            config: core_run_config,
        }),
        VersionedEventPayload::new(EventPayload::PipelineMaterialized {
            graph: Box::new(graph.clone()),
            graph_hash,
        }),
    ];
    if let Some(inputs) = &run_config.owned_flow_inputs {
        events.push(VersionedEventPayload::new(
            EventPayload::OwnedFlowInputsBound {
                manifest: Box::new(inputs.manifest().clone()),
            },
        ));
    }
    Ok(events)
}

async fn collect_startup_artifact_events(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    worktree_path: &Path,
    run_config: &EngineRunConfig,
) -> Result<Vec<VersionedEventPayload>, EngineError> {
    let mut events = Vec::new();
    if let Some(event) = project_context_artifact_event(artifact_store, run_id, run_config).await? {
        events.push(event);
    }
    if let Some(event) = project_memory_artifact_event(artifact_store, run_id, run_config).await? {
        events.push(event);
    }
    if let Some(event) =
        memory_claim_candidates_artifact_event(artifact_store, run_id, run_config).await?
    {
        events.push(event);
    }
    events.extend(run_seed_artifact_events(run_id, worktree_path, run_config).await?);
    if let Some(event) = initial_prompt_artifact_event(run_id, worktree_path, run_config).await? {
        events.push(event);
    }
    Ok(events)
}

async fn project_context_artifact_event(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    run_config: &EngineRunConfig,
) -> Result<Option<VersionedEventPayload>, EngineError> {
    let Some(seed) = &run_config.project_context else {
        return Ok(None);
    };
    let project_context_artifact = artifact_store
        .put(
            run_id,
            PROJECT_CONTEXT_ARTIFACT_NAME,
            seed.content.as_bytes(),
        )
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;
    let producer = surge_core::keys::NodeKey::try_from(PROJECT_CONTEXT_PRODUCER_NODE)
        .map_err(|e| EngineError::Internal(format!("project context producer key: {e}")))?;
    tracing::debug!(
        target: "engine::startup",
        run_id = %run_id,
        path = %seed.path.display(),
        bytes = seed.content.len(),
        hash = %project_context_artifact.hash,
        "seeded project_context artifact"
    );
    tracing::info!(
        target: "engine::startup",
        run_id = %run_id,
        path = %seed.path.display(),
        hash = %project_context_artifact.hash,
        "project context captured for run"
    );
    Ok(Some(artifact_produced_event(
        producer,
        project_context_artifact.hash,
        project_context_artifact.path,
        PROJECT_CONTEXT_ARTIFACT_NAME,
    )))
}

async fn project_memory_artifact_event(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    run_config: &EngineRunConfig,
) -> Result<Option<VersionedEventPayload>, EngineError> {
    let Some(seed) = &run_config.project_memory else {
        return Ok(None);
    };
    let artifact = artifact_store
        .put(
            run_id,
            PROJECT_MEMORY_ARTIFACT_NAME,
            seed.content.as_bytes(),
        )
        .await
        .map_err(|e| EngineError::Storage(e.to_string()))?;
    let producer = surge_core::keys::NodeKey::try_from(PROJECT_CONTEXT_PRODUCER_NODE)
        .map_err(|e| EngineError::Internal(format!("project memory producer key: {e}")))?;
    tracing::info!(
        target: "engine::startup",
        run_id = %run_id,
        path = %seed.path.display(),
        hash = %artifact.hash,
        "project memory captured for run"
    );
    Ok(Some(artifact_produced_event(
        producer,
        artifact.hash,
        artifact.path,
        PROJECT_MEMORY_ARTIFACT_NAME,
    )))
}

async fn memory_claim_candidates_artifact_event(
    artifact_store: &surge_persistence::artifacts::ArtifactStore,
    run_id: RunId,
    run_config: &EngineRunConfig,
) -> Result<Option<VersionedEventPayload>, EngineError> {
    let Some(candidates) = &run_config.memory_claim_candidates else {
        return Ok(None);
    };
    let snapshot = crate::project_context::MemoryClaimSnapshot {
        budget: run_config.context_pack.unwrap_or_default(),
        claims: candidates.clone(),
    };
    let body = serde_json::to_vec(&snapshot).map_err(|error| {
        EngineError::Internal(format!("memory claim snapshot serialize: {error}"))
    })?;
    let artifact = artifact_store
        .put(run_id, MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME, &body)
        .await
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    let producer = surge_core::keys::NodeKey::try_from(PROJECT_CONTEXT_PRODUCER_NODE)
        .map_err(|error| EngineError::Internal(format!("project memory producer key: {error}")))?;
    Ok(Some(artifact_produced_event(
        producer,
        artifact.hash,
        artifact.path,
        MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME,
    )))
}

async fn run_seed_artifact_events(
    run_id: RunId,
    worktree_path: &Path,
    run_config: &EngineRunConfig,
) -> Result<Vec<VersionedEventPayload>, EngineError> {
    let mut events = Vec::with_capacity(run_config.seed_artifacts.len());
    for seed in &run_config.seed_artifacts {
        let seeded_artifact = synthesise_run_seed_artifact(worktree_path, seed)
            .await
            .map_err(|e| {
                EngineError::Internal(format!("seed artifact {} synthesis failed: {e}", seed.name))
            })?;
        tracing::debug!(
            target: "engine::startup",
            run_id = %run_id,
            name = seed.name.as_str(),
            path = %seed.relative_path.display(),
            bytes = seed.content.len(),
            hash = %seeded_artifact.hash,
            "seeded run artifact"
        );
        events.push(artifact_produced_event(
            seeded_artifact.producer,
            seeded_artifact.hash,
            seeded_artifact.relative_path,
            &seed.name,
        ));
    }
    Ok(events)
}

pub(super) async fn append_task_binding(
    store: &surge_persistence::artifacts::ArtifactStore,
    run: RunId,
    context: surge_core::work_item::WorkItemContext,
    events: &mut Vec<VersionedEventPayload>,
) -> Result<(), EngineError> {
    let bytes = context
        .origin()
        .canonical_bytes()
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    let artifact_name = if context.flow().is_some() {
        "accepted_flow_contract"
    } else {
        "accepted_requirements"
    };
    let artifact = store
        .put(run, artifact_name, &bytes)
        .await
        .map_err(|error| EngineError::Storage(error.to_string()))?;
    events.push(artifact_produced_event(
        "task_requirements"
            .parse()
            .map_err(|error: surge_core::keys::KeyParseError| {
                EngineError::Internal(error.to_string())
            })?,
        artifact.hash,
        artifact.path,
        artifact_name,
    ));
    events.push(VersionedEventPayload::new(
        EventPayload::WorkItemAttemptBound { context },
    ));
    Ok(())
}

async fn initial_prompt_artifact_event(
    run_id: RunId,
    worktree_path: &Path,
    run_config: &EngineRunConfig,
) -> Result<Option<VersionedEventPayload>, EngineError> {
    if run_config.initial_prompt.is_empty() {
        return Ok(None);
    }
    let prompt_artifact =
        synthesise_initial_prompt_artifact(worktree_path, run_config.initial_prompt.as_bytes())
            .await
            .map_err(|e| {
                EngineError::Internal(format!("initial-prompt artifact synthesis failed: {e}"))
            })?;
    tracing::debug!(
        target: "engine::startup",
        run_id = %run_id,
        prompt_len = run_config.initial_prompt.len(),
        "seeded user_prompt artifact",
    );
    Ok(Some(artifact_produced_event(
        prompt_artifact.producer,
        prompt_artifact.hash,
        prompt_artifact.relative_path,
        INITIAL_PROMPT_ARTIFACT_NAME,
    )))
}

fn artifact_produced_event(
    node: surge_core::keys::NodeKey,
    artifact: surge_core::content_hash::ContentHash,
    path: PathBuf,
    name: &str,
) -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::ArtifactProduced {
        node,
        artifact,
        path,
        name: name.to_owned(),
        source_path: None,
    })
}

pub(crate) const INITIAL_PROMPT_ARTIFACT_NAME: &str = "user_prompt";

/// Canonical artifact name for the stable project context captured at run start.
pub(crate) const PROJECT_CONTEXT_ARTIFACT_NAME: &str = "project_context";

/// Canonical artifact name for the accumulating project memory captured at
/// run start (`.surge/memory/`).
pub(crate) const PROJECT_MEMORY_ARTIFACT_NAME: &str = "project_memory";

/// Artifact containing immutable source claims for per-node context packs.
pub const MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME: &str = "memory_claim_candidates";

/// Canonical artifact name for the resolved profile-registry catalogue
/// (`profile_loader::render_profile_catalog`) seeded at run start.
/// `flow-generator@1.0` binds this to pick profiles by what the registry
/// actually holds instead of from names memorized in its own prompt.
pub(crate) const PROFILE_CATALOG_ARTIFACT_NAME: &str = "profile_catalog";

/// Synthetic producer node for the run-level project context seed.
const PROJECT_CONTEXT_PRODUCER_NODE: &str = "project_context_seed";

/// Relative path within the worktree where the seeded profile catalogue is
/// stored.
pub(super) const PROFILE_CATALOG_ARTIFACT_RELPATH: &str = ".surge/profile_catalog.md";

/// Synthetic producer node id recorded on the seeded profile-catalogue
/// `ArtifactProduced` event. No real node produces it — same reasoning as
/// `PROJECT_CONTEXT_PRODUCER_NODE` above.
pub(super) const PROFILE_CATALOG_PRODUCER_NODE: &str = "profile_catalog_seed";

/// Relative path within the worktree where the seeded prompt body is stored.
const INITIAL_PROMPT_ARTIFACT_RELPATH: &str = ".surge/user_prompt.txt";

/// Synthetic producer node id recorded on the seeded `ArtifactProduced` event.
/// Bootstrap graphs do not have a real `start_node` user node, so the
/// engine attributes the prompt to a stable synthetic key.
const INITIAL_PROMPT_PRODUCER_NODE: &str = "start_node";

/// Output of [`synthesise_initial_prompt_artifact`].
pub(crate) struct InitialPromptArtifact {
    pub hash: surge_core::content_hash::ContentHash,
    pub relative_path: PathBuf,
    pub producer: surge_core::keys::NodeKey,
}

/// Persist the operator-supplied prompt body to a stable location inside the
/// run's worktree and compute the metadata needed to record an
/// `ArtifactProduced` event. Returns the content hash, the relative on-disk
/// path, and the synthetic producer node key.
///
/// Writes `<worktree>/.surge/user_prompt.txt`, creating the parent directory
/// when missing. Idempotent for identical content (the file is overwritten
/// each call, and `ContentHash::compute` is purely a function of bytes).
pub(crate) async fn synthesise_initial_prompt_artifact(
    worktree_path: &std::path::Path,
    prompt_bytes: &[u8],
) -> std::io::Result<InitialPromptArtifact> {
    use surge_core::content_hash::ContentHash;
    use surge_core::keys::NodeKey;

    let relative_path = PathBuf::from(INITIAL_PROMPT_ARTIFACT_RELPATH);
    let absolute_path = worktree_path.join(&relative_path);
    if let Some(parent) = absolute_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&absolute_path, prompt_bytes).await?;
    let hash = ContentHash::compute(prompt_bytes);
    let producer = NodeKey::try_from(INITIAL_PROMPT_PRODUCER_NODE).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("initial prompt producer node key: {e}"),
        )
    })?;
    Ok(InitialPromptArtifact {
        hash,
        relative_path,
        producer,
    })
}

pub(crate) async fn synthesise_run_seed_artifact(
    worktree_path: &std::path::Path,
    seed: &crate::engine::config::RunSeedArtifact,
) -> std::io::Result<InitialPromptArtifact> {
    use std::path::Component;

    if seed.relative_path.is_absolute()
        || seed.relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "seed artifact path must stay inside the worktree: {}",
                seed.relative_path.display()
            ),
        ));
    }

    let absolute_path = worktree_path.join(&seed.relative_path);
    if let Some(parent) = absolute_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&absolute_path, seed.content.as_bytes()).await?;
    let hash = surge_core::ContentHash::compute(seed.content.as_bytes());
    Ok(InitialPromptArtifact {
        hash,
        relative_path: seed.relative_path.clone(),
        producer: seed.producer.clone(),
    })
}

#[cfg(test)]
mod memory_claim_snapshot_artifact_tests {
    use super::*;

    #[tokio::test]
    async fn frozen_candidates_are_stored_as_a_reportable_run_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let store = surge_persistence::artifacts::ArtifactStore::new(temp.path());
        let run_id = RunId::new();
        let config = EngineRunConfig {
            memory_claim_candidates: Some(Vec::new()),
            ..EngineRunConfig::default()
        };

        let event = memory_claim_candidates_artifact_event(&store, run_id, &config)
            .await
            .unwrap()
            .unwrap();
        let surge_core::run_event::EventPayload::ArtifactProduced { artifact, name, .. } =
            event.payload
        else {
            panic!("candidate snapshot must be linked into replay as an artifact event");
        };
        assert_eq!(name, MEMORY_CLAIM_CANDIDATES_ARTIFACT_NAME);
        let bytes = store.open(run_id, artifact).await.unwrap();
        let snapshot: crate::project_context::MemoryClaimSnapshot =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(snapshot.claims.len(), 0);
        assert_eq!(
            snapshot.budget,
            surge_core::context_pack::ContextPackConfig::default()
        );
    }
}
