//! Reconstructable bootstrap inputs pinned to the daemon's actual engine runtime.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use surge_core::{
    ContentHash, RunId, SurgeConfig,
    bootstrap_operation::{BootstrapCapture, BootstrapIntent},
    graph::Graph,
};
use surge_orchestrator::profile_loader::ProfileRegistry;

/// Runtime capture refusal. Diagnostics deliberately omit configuration and secret values.
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum BootstrapRuntimeError {
    /// The selected project's inputs do not match the engine's immutable runtime.
    #[error("bootstrap configuration is unavailable or differs from the daemon runtime")]
    Configuration,
    /// Isolation could not be pinned or reconciled.
    #[error("bootstrap repository isolation could not be confirmed")]
    Isolation,
    /// The project has uncommitted changes; runs start from a clean commit.
    #[error(
        "the project has uncommitted changes — commit or stash them (git status shows which), then start again"
    )]
    DirtyRepository,
    /// The project has no commit to start from.
    #[error("the project has no commits yet — make an initial commit, then start again")]
    EmptyRepository,
    /// Required source credentials are absent.
    #[error("bootstrap required credential reference is unavailable")]
    Credential,
    /// Pinned project context cannot be read consistently.
    #[error("bootstrap project context could not be confirmed")]
    Context,
}

/// Holds the same immutable registry objects supplied to `Engine::new_full`.
#[derive(Clone)]
pub struct BootstrapRuntime {
    config: Arc<SurgeConfig>,
    profiles: Arc<ProfileRegistry>,
    agents: Arc<surge_acp::Registry>,
    worktrees_root: PathBuf,
    profiles_root: PathBuf,
    graph: Graph,
}

impl BootstrapRuntime {
    /// Construct only after successful daemon configuration loading.
    pub fn new(
        config: Arc<SurgeConfig>,
        profiles: Arc<ProfileRegistry>,
        agents: Arc<surge_acp::Registry>,
        worktrees_root: PathBuf,
        profiles_root: PathBuf,
    ) -> Result<Self, BootstrapRuntimeError> {
        let graph = surge_core::BundledFlows::by_name_latest("bootstrap")
            .ok_or(BootstrapRuntimeError::Configuration)?
            .graph;
        Ok(Self {
            config,
            profiles,
            agents,
            worktrees_root,
            profiles_root,
            graph,
        })
    }

    /// This runtime scoped to one project: the daemon's profiles and bootstrap
    /// graph, but the **project's** agent catalog (its `[agents.*]` over the
    /// builtins). Every project used to need an agent list byte-identical to
    /// the daemon's own `surge.toml`, so a freshly created app — or any
    /// second project — was refused with an opaque configuration error.
    pub fn for_project(&self, repository: &Path) -> Result<Self, BootstrapRuntimeError> {
        let project = SurgeConfig::load(&repository.join("surge.toml"))
            .map_err(|_| BootstrapRuntimeError::Configuration)?;
        supported(&project)?;
        Ok(Self {
            agents: Arc::new(surge_acp::Registry::for_run(&project)),
            ..self.clone()
        })
    }

    /// Agent catalog runs of this (project-scoped) runtime launch with.
    #[must_use]
    pub fn agents(&self) -> Arc<surge_acp::Registry> {
        Arc::clone(&self.agents)
    }

    /// Capture clean repository and immutable execution references outside a DB transaction.
    pub async fn capture(
        &self,
        intent: &BootstrapIntent,
    ) -> Result<BootstrapCapture, BootstrapRuntimeError> {
        Self::check_budget(intent)?;
        let project = intent.project_path().to_path_buf();
        let base = tokio::task::spawn_blocking(move || {
            surge_git::GitManager::new(project)?.capture_clean_base()
        })
        .await
        .map_err(|_| BootstrapRuntimeError::Isolation)?
        .map_err(|error| match error {
            surge_git::GitError::DirtyRepository => BootstrapRuntimeError::DirtyRepository,
            surge_git::GitError::EmptyRepository => BootstrapRuntimeError::EmptyRepository,
            _ => BootstrapRuntimeError::Isolation,
        })?;
        let runtime = self.for_project(base.repository())?;
        let planning_run = RunId::new();
        let implementation_run = RunId::new();
        let planning = surge_git::run_worktree::RunWorktreeSpec::new(
            planning_run,
            base.clone(),
            runtime.worktrees_root.join(planning_run.to_string()),
        )
        .map_err(|_| BootstrapRuntimeError::Isolation)?;
        let implementation = surge_git::run_worktree::RunWorktreeSpec::new(
            implementation_run,
            base.clone(),
            runtime.worktrees_root.join(implementation_run.to_string()),
        )
        .map_err(|_| BootstrapRuntimeError::Isolation)?;
        let configuration = runtime.configuration(base.repository())?;
        let context = runtime.context(base.repository()).await?;
        surge_core::bootstrap_operation::BootstrapCaptureFields {
            version: 1,
            planning_run,
            implementation_run,
            repository: base.repository().to_path_buf(),
            git_common_dir: base.git_common_dir().to_path_buf(),
            base_commit: base.commit().to_string(),
            planning_worktree: planning.path().to_path_buf(),
            planning_branch: planning.branch(),
            implementation_worktree: implementation.path().to_path_buf(),
            implementation_branch: implementation.branch(),
            graph: runtime.graph_reference()?,
            project_context: context.as_ref().map(context_ref),
            runtime_identity: runtime.runtime_identity()?,
            configuration,
            credential_refs: runtime.validate_graph(&runtime.graph)?,
        }
        .try_into()
        .map_err(|_| BootstrapRuntimeError::Configuration)
    }

    /// Verify original references without recapturing mutable HEAD or requiring a clean source.
    pub async fn verify(
        &self,
        capture: &BootstrapCapture,
    ) -> Result<Option<surge_orchestrator::engine::config::ProjectContextSeed>, BootstrapRuntimeError>
    {
        let pins = capture.fields();
        let runtime = self.for_project(&pins.repository)?;
        let context = runtime.context(&pins.repository).await?;
        if pins.runtime_identity != runtime.runtime_identity()?
            || pins.configuration != runtime.configuration(&pins.repository)?
            || pins.graph != runtime.graph_reference()?
            || pins.project_context != context.as_ref().map(context_ref)
            || pins.credential_refs != runtime.validate_graph(&runtime.graph)?
        {
            return Err(BootstrapRuntimeError::Configuration);
        }
        Ok(context)
    }

    fn graph_reference(
        &self,
    ) -> Result<surge_core::bootstrap_operation::BootstrapContentRef, BootstrapRuntimeError> {
        Ok(surge_core::bootstrap_operation::BootstrapContentRef {
            name: "bundled:bootstrap".into(),
            digest: ContentHash::compute(
                &serde_json::to_vec(&self.graph)
                    .map_err(|_| BootstrapRuntimeError::Configuration)?,
            ),
        })
    }

    /// The graph captured from the bundled runtime.
    #[must_use]
    pub fn graph(&self) -> Graph {
        self.graph.clone()
    }

    fn check_budget(intent: &BootstrapIntent) -> Result<(), BootstrapRuntimeError> {
        // Per-run USD enforcement cannot be safely transferred across the
        // planning/implementation boundary yet. Refuse rather than reset it.
        if intent.budget().limits.usd.is_some() {
            return Err(BootstrapRuntimeError::Configuration);
        }
        Ok(())
    }

    fn configuration(
        &self,
        repository: &Path,
    ) -> Result<Vec<surge_core::bootstrap_operation::BootstrapContentRef>, BootstrapRuntimeError>
    {
        let project = SurgeConfig::load(&repository.join("surge.toml"))
            .map_err(|_| BootstrapRuntimeError::Configuration)?;
        supported(&project)?;
        supported(&self.config)?;
        let project_agents = surge_acp::Registry::for_run(&project);
        if registry_hash(&project_agents)? != registry_hash(&self.agents)?
            || digest(&project.capacity)? != digest(&self.config.capacity)?
        {
            return Err(BootstrapRuntimeError::Configuration);
        }
        Ok(vec![content_ref(
            "surge.toml:execution",
            &(
                registry_hash(&project_agents)?,
                &project.capacity,
                &project.init,
                &project.tool_call_loop_guard,
                &project.output_spill,
            ),
        )?])
    }

    fn runtime_identity(&self) -> Result<ContentHash, BootstrapRuntimeError> {
        let loaded = profile_hash(&self.profiles)?;
        let current = surge_orchestrator::profile_loader::ProfileRegistry::new(
            surge_orchestrator::profile_loader::DiskProfileSet::scan(&self.profiles_root)
                .map_err(|_| BootstrapRuntimeError::Configuration)?,
        );
        if loaded != profile_hash(&current)? {
            return Err(BootstrapRuntimeError::Configuration);
        }
        digest(&(
            1_u32,
            loaded,
            registry_hash(&self.agents)?,
            &self.config.capacity,
        ))
    }

    /// Resolve every graph agent through the same registries used by the engine.
    pub fn validate_graph(&self, graph: &Graph) -> Result<Vec<String>, BootstrapRuntimeError> {
        surge_core::validation::validate_with_resolver(graph, self.profiles.as_ref())
            .map_err(|_| BootstrapRuntimeError::Configuration)?;
        let nodes = graph.nodes.values().chain(
            graph
                .subgraphs
                .values()
                .flat_map(|graph| graph.nodes.values()),
        );
        let mut refs = std::collections::BTreeSet::new();
        for node in nodes {
            let surge_core::node::NodeConfig::Agent(agent) = &node.config else {
                continue;
            };
            let key = surge_core::profile::keyref::parse_key_ref(agent.profile.as_str())
                .map_err(|_| BootstrapRuntimeError::Configuration)?;
            let resolved = self
                .profiles
                .resolve(&key)
                .map_err(|_| BootstrapRuntimeError::Configuration)?;
            let entry = self
                .agents
                .find_normalized(&resolved.profile.runtime.agent_id)
                .ok_or(BootstrapRuntimeError::Configuration)?;
            surge_acp::agent_env::resolve(&entry.id, &entry.env)
                .map_err(|_| BootstrapRuntimeError::Credential)?;
            for value in entry.env.values() {
                if let surge_core::config::AgentEnvValue::Inject { from, .. } = value {
                    refs.insert(format!("env:{from}"));
                }
            }
        }
        Ok(refs.into_iter().collect())
    }

    async fn context(
        &self,
        repository: &Path,
    ) -> Result<Option<surge_orchestrator::engine::config::ProjectContextSeed>, BootstrapRuntimeError>
    {
        let config = SurgeConfig::load(&repository.join("surge.toml"))
            .map_err(|_| BootstrapRuntimeError::Configuration)?;
        if !config.init.project_context_auto_seed {
            return Ok(None);
        }
        if config
            .init
            .project_context_path
            .components()
            .any(|component| {
                !matches!(
                    component,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            return Err(BootstrapRuntimeError::Context);
        }
        let path = repository.join(&config.init.project_context_path);
        let canonical = match tokio::fs::canonicalize(&path).await {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(BootstrapRuntimeError::Context),
        };
        if !canonical.starts_with(repository) {
            return Err(BootstrapRuntimeError::Context);
        }
        let content = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|_| BootstrapRuntimeError::Context)?;
        Ok(Some(
            surge_orchestrator::engine::config::ProjectContextSeed::new(canonical, content),
        ))
    }
}

fn supported(config: &SurgeConfig) -> Result<(), BootstrapRuntimeError> {
    if !config.mcp_servers.is_empty()
        || config.tool_call_loop_guard
            != surge_core::loop_config::ToolCallLoopGuardConfig::default()
        || config.output_spill != surge_core::spill_config::OutputSpillConfig::default()
        || config.agents.values().any(|agent| {
            !agent.mcp_servers.is_empty()
                || agent.env.values().any(|value| {
                    !matches!(
                        value,
                        surge_core::config::AgentEnvValue::Inject { default: None, .. }
                    )
                })
        })
    {
        return Err(BootstrapRuntimeError::Configuration);
    }
    Ok(())
}

fn digest(value: &impl serde::Serialize) -> Result<ContentHash, BootstrapRuntimeError> {
    let canonical =
        serde_json::to_value(value).map_err(|_| BootstrapRuntimeError::Configuration)?;
    Ok(ContentHash::compute(
        &serde_json::to_vec(&canonical).map_err(|_| BootstrapRuntimeError::Configuration)?,
    ))
}
fn content_ref(
    name: &str,
    value: &impl serde::Serialize,
) -> Result<surge_core::bootstrap_operation::BootstrapContentRef, BootstrapRuntimeError> {
    Ok(surge_core::bootstrap_operation::BootstrapContentRef {
        name: name.into(),
        digest: digest(value)?,
    })
}
fn context_ref(
    seed: &surge_orchestrator::engine::config::ProjectContextSeed,
) -> surge_core::bootstrap_operation::BootstrapContentRef {
    surge_core::bootstrap_operation::BootstrapContentRef {
        name: seed.path.to_string_lossy().into_owned(),
        digest: seed.hash,
    }
}
fn profile_hash(profiles: &ProfileRegistry) -> Result<ContentHash, BootstrapRuntimeError> {
    let mut values = profiles
        .list()
        .into_iter()
        .map(|entry| serde_json::to_value(entry.profile))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| BootstrapRuntimeError::Configuration)?;
    values.sort_by_key(serde_json::Value::to_string);
    digest(&values)
}
fn registry_hash(registry: &surge_acp::Registry) -> Result<ContentHash, BootstrapRuntimeError> {
    let mut entries: Vec<_> = registry.list().iter().collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    digest(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::budget::BudgetGuard;
    use surge_orchestrator::profile_loader::DiskProfileSet;

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository(root: &Path) -> PathBuf {
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("surge.toml"), "schema_version = 1\n").unwrap();
        git(&project, &["init"]);
        git(&project, &["config", "user.name", "Surge Test"]);
        git(&project, &["config", "user.email", "test@example.invalid"]);
        git(&project, &["add", "."]);
        git(&project, &["commit", "-m", "configured repository"]);
        project.canonicalize().unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn configured_clean_repository_captures_pins_without_creating_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let project = repository(temp.path());
        let config = Arc::new(SurgeConfig::load(&project.join("surge.toml")).unwrap());
        let profiles = Arc::new(ProfileRegistry::new(
            DiskProfileSet::scan(&temp.path().join("profiles")).unwrap(),
        ));
        let agents = Arc::new(surge_acp::Registry::for_run(&config));
        let worktrees = temp.path().canonicalize().unwrap().join("worktrees");
        let runtime = BootstrapRuntime::new(
            config,
            profiles,
            agents,
            worktrees.clone(),
            temp.path().join("profiles"),
        )
        .unwrap();
        let prompt = "  build a useful app\nwith exact intent  ";
        let intent =
            BootstrapIntent::new(project.clone(), prompt.into(), BudgetGuard::default()).unwrap();
        let captured = runtime.capture(&intent).await.unwrap();
        let pins = captured.fields();
        assert_eq!(pins.repository, project);
        assert_ne!(pins.planning_run, pins.implementation_run);
        assert!(pins.planning_worktree.starts_with(&worktrees));
        assert_eq!(pins.base_commit.len(), 40);
        assert!(
            !worktrees.exists(),
            "capture must not perform execution preparation"
        );
        assert_eq!(intent.prompt(), prompt);
    }
}
