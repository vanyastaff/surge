//! `MinimalBootstrapGraphBuilder` — produces a single-Agent graph for
//! today's inbox-card pipeline. Replaced by `StagedBootstrapGraphBuilder`
//! when RFC-0004 lands.

use crate::bootstrap::builder::{BootstrapBuildError, BootstrapGraphBuilder, BootstrapPrompt};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::PathBuf;
use surge_core::agent_config::{AgentConfig, NodeLimits, PromptOverride};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};

/// Single-stage Agent bootstrap.
///
/// Produces a three-node graph:
///
/// ```text
/// bootstrap_agent  ──done──►  terminal_success
///                  ──blocked──► terminal_failure
/// ```
///
/// The agent uses the built-in `implementer@1.0` profile. The rendered
/// prompt is injected as a system-prompt override so every run starts
/// with the exact ticket text.
#[derive(Debug, Clone, Default)]
pub struct MinimalBootstrapGraphBuilder;

impl MinimalBootstrapGraphBuilder {
    /// Construct a new builder.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BootstrapGraphBuilder for MinimalBootstrapGraphBuilder {
    async fn build(
        &self,
        _run_id: RunId,
        prompt: BootstrapPrompt,
        _worktree: PathBuf,
    ) -> Result<Graph, BootstrapBuildError> {
        if prompt.description.trim().is_empty() {
            return Err(BootstrapBuildError::InvalidPrompt(
                "description must not be empty".into(),
            ));
        }
        let prompt_text = render_prompt(&prompt);
        build_single_agent_graph(&prompt_text).map_err(BootstrapBuildError::GraphBuild)
    }
}

/// Render the structured prompt into the Agent's system prompt override.
fn render_prompt(prompt: &BootstrapPrompt) -> String {
    let mut s = String::new();
    s.push_str("You are working on this ticket.\n\n");
    s.push_str(&format!("Title: {}\n", prompt.title));
    if let Some(url) = &prompt.tracker_url {
        s.push_str(&format!("URL: {url}\n"));
    }
    if !prompt.labels.is_empty() {
        s.push_str(&format!("Labels: {}\n", prompt.labels.join(", ")));
    }
    s.push_str("\nDescription:\n");
    s.push_str(&prompt.description);
    s.push_str(
        "\n\nImplement the request directly in this worktree. Run tests \
         before reporting done. If the request is ambiguous, escalate.",
    );
    s
}

/// Build the `Graph` value: 1 Agent node + 2 Terminal nodes + 2 edges.
///
/// Shape:
/// - `bootstrap_agent` (Agent, profile `implementer@1.0`, outcomes `done`/`blocked`)
/// - `terminal_success` (Terminal, Success)
/// - `terminal_failure` (Terminal, Failure exit_code=1)
/// - edge `e_agent_done`: agent.done → terminal_success (Forward)
/// - edge `e_agent_blocked`: agent.blocked → terminal_failure (Escalate)
fn build_single_agent_graph(prompt_text: &str) -> Result<Graph, String> {
    let agent_key =
        NodeKey::try_from("bootstrap_agent").map_err(|e| format!("invalid node key: {e}"))?;
    let success_key =
        NodeKey::try_from("terminal_success").map_err(|e| format!("invalid node key: {e}"))?;
    let failure_key =
        NodeKey::try_from("terminal_failure").map_err(|e| format!("invalid node key: {e}"))?;

    let outcome_done =
        OutcomeKey::try_from("done").map_err(|e| format!("invalid outcome key: {e}"))?;
    let outcome_blocked =
        OutcomeKey::try_from("blocked").map_err(|e| format!("invalid outcome key: {e}"))?;

    let profile =
        ProfileKey::try_from("implementer@1.0").map_err(|e| format!("invalid profile key: {e}"))?;

    // Agent node with prompt override
    let agent_node = Node {
        id: agent_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![
            OutcomeDecl {
                id: outcome_done.clone(),
                description: "Implementation complete".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
            },
            OutcomeDecl {
                id: outcome_blocked.clone(),
                description: "Blocked; needs human review".into(),
                edge_kind_hint: EdgeKind::Escalate,
                is_terminal: false,
            },
        ],
        config: NodeConfig::Agent(AgentConfig {
            profile,
            prompt_overrides: Some(PromptOverride {
                system: Some(prompt_text.to_owned()),
                append_system: None,
            }),
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: Vec::new(),
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks: Vec::new(),
            custom_fields: BTreeMap::new(),
        }),
    };

    let success_node = Node {
        id: success_key.clone(),
        position: Position::default(),
        declared_outcomes: Vec::new(),
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: Some("Run completed successfully.".into()),
        }),
    };

    let failure_node = Node {
        id: failure_key.clone(),
        position: Position::default(),
        declared_outcomes: Vec::new(),
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Failure { exit_code: 1 },
            message: Some("Run blocked; manual intervention required.".into()),
        }),
    };

    let edges = vec![
        Edge {
            id: EdgeKey::try_from("e_agent_done").map_err(|e| format!("invalid edge key: {e}"))?,
            from: PortRef {
                node: agent_key.clone(),
                outcome: outcome_done,
            },
            to: success_key,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
        Edge {
            id: EdgeKey::try_from("e_agent_blocked")
                .map_err(|e| format!("invalid edge key: {e}"))?,
            from: PortRef {
                node: agent_key.clone(),
                outcome: outcome_blocked,
            },
            to: failure_key,
            kind: EdgeKind::Escalate,
            policy: EdgePolicy::default(),
        },
    ];

    let mut nodes = BTreeMap::new();
    nodes.insert(agent_key.clone(), agent_node);
    nodes.insert(
        NodeKey::try_from("terminal_success").map_err(|e| format!("invalid node key: {e}"))?,
        success_node,
    );
    nodes.insert(
        NodeKey::try_from("terminal_failure").map_err(|e| format!("invalid node key: {e}"))?,
        failure_node,
    );

    Ok(Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "bootstrap-minimal".into(),
            description: Some(
                "Single-agent bootstrap graph generated by MinimalBootstrapGraphBuilder.".into(),
            ),
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: agent_key,
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::RunId;
    use surge_intake::types::Priority;

    fn sample_prompt() -> BootstrapPrompt {
        BootstrapPrompt {
            title: "Fix parser panic".into(),
            description: "Stack overflow on deep nesting in parse_object.".into(),
            tracker_url: Some("https://github.com/o/r/issues/42".into()),
            priority: Some(Priority::High),
            labels: vec!["surge:enabled".into(), "bug".into()],
        }
    }

    #[tokio::test]
    async fn produces_graph_for_valid_prompt() {
        let builder = MinimalBootstrapGraphBuilder::new();
        let _graph = builder
            .build(RunId::new(), sample_prompt(), std::env::temp_dir())
            .await
            .expect("builds");
        // Structural validation is performed by the engine on start_run.
        // Here we only assert that no error was returned.
    }

    #[tokio::test]
    async fn rejects_empty_description() {
        let builder = MinimalBootstrapGraphBuilder::new();
        let mut p = sample_prompt();
        p.description = "   ".into();
        let err = builder
            .build(RunId::new(), p, std::env::temp_dir())
            .await
            .unwrap_err();
        assert!(matches!(err, BootstrapBuildError::InvalidPrompt(_)));
    }

    #[test]
    fn render_prompt_includes_title_url_labels_description() {
        let p = sample_prompt();
        let rendered = render_prompt(&p);
        assert!(rendered.contains("Fix parser panic"));
        assert!(rendered.contains("https://github.com/o/r/issues/42"));
        assert!(rendered.contains("surge:enabled"));
        assert!(rendered.contains("Stack overflow"));
        assert!(rendered.contains("Implement the request"));
    }
}
