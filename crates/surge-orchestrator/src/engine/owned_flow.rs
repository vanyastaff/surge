//! Allowlisted ordinary Flow intent. Host-derived inputs never cross this boundary.
use super::EngineRunConfig;
use super::config::{BootstrapRunConfig, ProjectContextSeed, RunSeedArtifact};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};
use surge_core::{Graph, RunId, id::WorkItemOperationId, mcp_config::McpServerRef};

/// Rejected caller input; diagnostics never contain the submitted body or transports.
#[derive(Debug, thiserror::Error)]
pub enum OwnedFlowInputError {
    /// Operation or project base cannot identify a stable request.
    #[error("owned Flow requires a nonnil operation and absolute project base")]
    Identity,
    /// Locator escapes its lexical project/root or names no file.
    #[error("owned Flow locator is outside its lexical project scope")]
    Locator,
    /// Template key is empty or exceeds its bound.
    #[error("owned Flow template key is empty or oversized")]
    Template,
    /// Exact prompt exceeds the accepted UTF-8 byte bound.
    #[error("owned Flow prompt exceeds its byte limit")]
    Prompt,
    /// Graph is invalid or cannot round trip as an accepted payload.
    #[error("owned Flow graph is invalid")]
    Graph,
    /// A generic config attempted to import host-only authority.
    #[error("owned Flow config contains host-only inputs")]
    HostFields,
    /// Canonical encoding failed.
    #[error("owned Flow canonical encoding failed")]
    Encoding(#[source] serde_json::Error),
}

/// Caller intent, resolved exactly once by the durable host.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FlowInput {
    /// Already captured graph payload.
    Inline {
        /// Exact submitted graph payload.
        graph: Box<Graph>,
    },
    /// Project-relative lexical locator; never resolved by the CLI.
    ProjectFile {
        /// Lexical project-scoped locator.
        locator: PathBuf,
    },
    /// Archetype registry key, resolved by the host on first capture.
    Template {
        /// Exact registry lookup key.
        key: String,
    },
}
impl std::fmt::Debug for FlowInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Inline { .. } => "Inline([captured])",
            Self::ProjectFile { .. } => "ProjectFile([locator])",
            Self::Template { .. } => "Template([key])",
        })
    }
}
/// Workspace creation policy; caller directory adoption grants no ownership.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceRequest {
    /// Allocate one retained host-owned workspace.
    #[default]
    Managed,
    /// Explicit paths are validated/refused rather than silently adopted.
    Explicit {
        /// Caller-requested path, never adoption permission.
        path: PathBuf,
    },
}
/// Absent host defaults and an explicitly empty MCP list have different meanings.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "servers",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum McpSelection {
    /// Resolve project/global precedence on the first snapshot only.
    #[default]
    HostDefault,
    /// Freeze these exact servers, including an empty list.
    Explicit(Vec<McpServerRef>),
}
impl std::fmt::Debug for McpSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostDefault => f.write_str("HostDefault"),
            Self::Explicit(servers) => f.debug_tuple("Explicit").field(&servers.len()).finish(),
        }
    }
}
/// Every field is caller intent; host-owned registry/auth/quota evidence is absent.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OwnedFlowRunConfig {
    /// Override the host human gate timeout.
    #[serde(with = "humantime_serde::option")]
    pub human_input_timeout: Option<Duration>,
    /// Optional stage timeout.
    #[serde(with = "humantime_serde::option")]
    pub stage_timeout_override: Option<Duration>,
    /// Exact initiating text; empty and whitespace are valid.
    pub initial_prompt: String,
    /// Effective MCP selection to capture once.
    pub mcp: McpSelection,
    /// Explicit planning run parent.
    pub bootstrap_parent: Option<RunId>,
    /// Explicit context seed, before automatic host seeding.
    pub project_context: Option<ProjectContextSeed>,
    /// Explicit memory text seed, excluding host memory claims.
    pub project_memory: Option<ProjectContextSeed>,
    /// Explicit pack budget.
    pub context_pack: Option<surge_core::context_pack::ContextPackConfig>,
    /// Ordered exact caller artifact seeds.
    pub seed_artifacts: Vec<RunSeedArtifact>,
    /// Explicit bootstrap knobs.
    pub bootstrap: Option<BootstrapRunConfig>,
    /// None resolves host defaults; Some(unlimited) explicitly disables the cap.
    pub budget: Option<surge_core::budget::BudgetGuard>,
    /// Explicit loop policy, including default-valued settings.
    pub tool_call_loop_guard: Option<surge_core::loop_config::ToolCallLoopGuardConfig>,
    /// Explicit spill policy, including default-valued settings.
    pub output_spill: Option<surge_core::spill_config::OutputSpillConfig>,
}
impl std::fmt::Debug for OwnedFlowRunConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwnedFlowRunConfig([redacted])")
    }
}
impl TryFrom<EngineRunConfig> for OwnedFlowRunConfig {
    type Error = OwnedFlowInputError;
    fn try_from(config: EngineRunConfig) -> Result<Self, Self::Error> {
        if config.agent_registry.is_some()
            || config.owned_flow_inputs.is_some()
            || config.memory_store_path.is_some()
            || config.memory_claim_candidates.is_some()
            || !config.quota_recovery.stages().is_empty()
        {
            return Err(OwnedFlowInputError::HostFields);
        }
        Ok(Self {
            human_input_timeout: Some(config.human_input_timeout),
            stage_timeout_override: config.stage_timeout_override,
            initial_prompt: config.initial_prompt,
            mcp: McpSelection::Explicit(config.mcp_servers),
            bootstrap_parent: config.bootstrap_parent,
            project_context: config.project_context,
            project_memory: config.project_memory,
            context_pack: config.context_pack,
            seed_artifacts: config.seed_artifacts,
            bootstrap: Some(config.bootstrap),
            budget: Some(config.budget),
            tool_call_loop_guard: config.tool_call_loop_guard,
            output_spill: config.output_spill,
        })
    }
}
/// Stable body carried before reading project files or configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedFlowStart {
    /// Immutable operation identity; reuse requires the same explicit body.
    pub operation_id: WorkItemOperationId,
    /// Explicit absolute lexical source project base.
    pub source_project: PathBuf,
    /// First-capture source intent.
    pub input: FlowInput,
    /// Allowlisted caller overrides.
    pub config: OwnedFlowRunConfig,
    /// Retained workspace policy.
    pub workspace: WorkspaceRequest,
}
pub use surge_core::work_item::OwnedFlowReceipt;
impl OwnedFlowStart {
    /// Normalize lexical scope without querying the project filesystem or daemon cwd.
    pub fn normalize(mut self) -> Result<Self, OwnedFlowInputError> {
        if self.operation_id == WorkItemOperationId::nil() || !self.source_project.is_absolute() {
            return Err(OwnedFlowInputError::Identity);
        }
        self.source_project = lexical_path(&self.source_project)?;
        if let FlowInput::ProjectFile { locator } = &mut self.input {
            let target = lexical_path(&self.source_project.join(&*locator))?;
            *locator = target
                .strip_prefix(&self.source_project)
                .map_err(|_| OwnedFlowInputError::Locator)?
                .to_path_buf();
            if locator.as_os_str().is_empty() {
                return Err(OwnedFlowInputError::Locator);
            }
        }
        if let FlowInput::Template { key } = &self.input
            && (key.is_empty() || key.len() > 1024)
        {
            return Err(OwnedFlowInputError::Template);
        }
        if self.config.initial_prompt.len() > 131_072 {
            return Err(OwnedFlowInputError::Prompt);
        }
        if let FlowInput::Inline { graph } = &self.input {
            surge_core::work_item::AcceptedFlowContract::new(
                graph.clone(),
                self.config.initial_prompt.clone(),
            )
            .map_err(|_| OwnedFlowInputError::Graph)?;
        }
        Ok(self)
    }
    /// Encode the exact allowlisted request with recursively sorted object keys.
    /// This private input must be keyed when it contains explicit MCP transport values.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, OwnedFlowInputError> {
        let normalized = self.clone().normalize()?;
        let mut value = serde_json::to_value(normalized).map_err(OwnedFlowInputError::Encoding)?;
        value.sort_all_objects();
        serde_json::to_vec(&value).map_err(OwnedFlowInputError::Encoding)
    }
}
fn lexical_path(path: &std::path::Path) -> Result<PathBuf, OwnedFlowInputError> {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => {
                if !matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    return Err(OwnedFlowInputError::Locator);
                }
                normalized.pop();
            },
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}
impl OwnedFlowRunConfig {
    /// Materialize caller overrides and host defaults once in the actual workspace.
    /// The returned MCP list includes an explicitly frozen empty selection.
    #[must_use]
    pub fn materialize(
        &self,
        workspace: &std::path::Path,
        host: &surge_core::SurgeConfig,
        default_mcp: Vec<McpServerRef>,
    ) -> EngineRunConfig {
        let effective_mcp = match &self.mcp {
            McpSelection::HostDefault => default_mcp,
            McpSelection::Explicit(servers) => servers.clone(),
        };
        let base = EngineRunConfig {
            human_input_timeout: self.human_input_timeout.unwrap_or(Duration::from_secs(300)),
            stage_timeout_override: self.stage_timeout_override,
            initial_prompt: self.initial_prompt.clone(),
            bootstrap_parent: self.bootstrap_parent,
            project_context: self.project_context.clone(),
            project_memory: self.project_memory.clone(),
            context_pack: self.context_pack,
            seed_artifacts: self.seed_artifacts.clone(),
            bootstrap: self.bootstrap.clone().unwrap_or_default(),
            budget: self.budget.unwrap_or_else(|| host.analytics.budget_guard()),
            tool_call_loop_guard: self.tool_call_loop_guard,
            output_spill: self.output_spill,
            ..EngineRunConfig::default()
        };
        let mut seeded = crate::project_context::with_project_context_seed(base, workspace, host);
        seeded.mcp_servers = effective_mcp;
        seeded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_fields_and_unknown_fields_are_refused() {
        for field in [
            "agent_registry",
            "memory_store_path",
            "quota_recovery",
            "memory_claim_candidates",
            "invented",
        ] {
            assert!(
                serde_json::from_value::<OwnedFlowRunConfig>(serde_json::json!({field: null}))
                    .is_err()
            );
        }
        let config = EngineRunConfig {
            memory_claim_candidates: Some(vec![]),
            ..EngineRunConfig::default()
        };
        assert_eq!(
            OwnedFlowRunConfig::try_from(config)
                .unwrap_err()
                .to_string(),
            "owned Flow config contains host-only inputs"
        );
    }
    #[test]
    fn empty_mcp_and_unlimited_budget_remain_explicit() {
        let defaults = OwnedFlowRunConfig::default();
        let explicit = OwnedFlowRunConfig {
            mcp: McpSelection::Explicit(vec![]),
            budget: Some(surge_core::budget::BudgetGuard::default()),
            ..defaults.clone()
        };
        assert_ne!(
            serde_json::to_value(defaults).unwrap(),
            serde_json::to_value(explicit).unwrap()
        );
    }
    #[test]
    fn canonical_request_sorts_env_maps_and_keeps_ordered_arguments() {
        use surge_core::mcp_config::McpTransportConfig;
        let operation = WorkItemOperationId::new();
        let make = |pairs: Vec<(&str, &str)>, args: Vec<&str>| OwnedFlowStart {
            operation_id: operation,
            source_project: std::env::temp_dir().join("source"),
            input: FlowInput::Template {
                key: "single-task".into(),
            },
            workspace: WorkspaceRequest::Managed,
            config: OwnedFlowRunConfig {
                mcp: McpSelection::Explicit(vec![McpServerRef::new(
                    "fixture".into(),
                    McpTransportConfig::stdio(
                        "command".into(),
                        args.into_iter().map(String::from).collect(),
                        pairs
                            .into_iter()
                            .map(|(key, value)| (key.into(), value.into()))
                            .collect(),
                    ),
                    None,
                    Duration::from_secs(60),
                    true,
                )]),
                ..OwnedFlowRunConfig::default()
            },
        };
        let first = make(vec![("a", "one"), ("b", "two")], vec!["one", "two"]);
        let reordered_map = make(vec![("b", "two"), ("a", "one")], vec!["one", "two"]);
        let reordered_args = make(vec![("a", "one"), ("b", "two")], vec!["two", "one"]);
        assert_eq!(
            first.canonical_bytes().unwrap(),
            reordered_map.canonical_bytes().unwrap()
        );
        assert_ne!(
            first.canonical_bytes().unwrap(),
            reordered_args.canonical_bytes().unwrap()
        );
        let debug = format!("{:?}", first.config);
        assert!(!debug.contains("command") && !debug.contains("one"));
    }
    #[test]
    fn direct_inline_dto_rejects_nonfinite_graph_before_canonical_encoding() {
        let mut graph: surge_core::Graph =
            toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml")).unwrap();
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            graph.nodes.values_mut().next().unwrap().position.x = value;
            let request = OwnedFlowStart {
                operation_id: WorkItemOperationId::new(),
                source_project: std::env::temp_dir().join("source"),
                input: FlowInput::Inline {
                    graph: Box::new(graph.clone()),
                },
                workspace: WorkspaceRequest::Managed,
                config: OwnedFlowRunConfig::default(),
            };
            assert!(matches!(
                request.canonical_bytes(),
                Err(OwnedFlowInputError::Graph)
            ));
        }
    }
    #[test]
    fn locator_scope_is_lexical_and_uses_explicit_base() {
        let project = std::env::temp_dir().join("uncreated-surge-flow-project");
        let make = |locator: &str| OwnedFlowStart {
            operation_id: WorkItemOperationId::new(),
            source_project: project.clone(),
            input: FlowInput::ProjectFile {
                locator: locator.into(),
            },
            config: OwnedFlowRunConfig::default(),
            workspace: WorkspaceRequest::Managed,
        };
        let normalized = make("one/../flow.toml").normalize().unwrap();
        assert!(
            matches!(normalized.input, FlowInput::ProjectFile { locator } if locator == std::path::Path::new("flow.toml"))
        );
        assert!(make("../flow.toml").normalize().is_err());
        let absolute = project.join("nested/flow.toml");
        let normalized = make(absolute.to_str().unwrap()).normalize().unwrap();
        assert!(
            matches!(normalized.input, FlowInput::ProjectFile { locator } if locator == std::path::Path::new("nested/flow.toml"))
        );
    }
}
