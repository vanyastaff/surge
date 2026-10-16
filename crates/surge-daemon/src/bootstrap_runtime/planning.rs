//! Project-default adaptation for the exact first-party bootstrap authoring profiles.
use super::{BootstrapRuntimeError, content_ref, digest};
use surge_core::{
    bootstrap_operation::BootstrapContentRef, graph::Graph, node::NodeConfig,
    profile::registry::Provenance,
};
use surge_orchestrator::profile_loader::{ProfileRegistry, effective_runtime_selector};

pub(super) struct PlanningGraph {
    pub graph: Graph,
    pub selection: BootstrapContentRef,
}

#[derive(serde::Serialize)]
struct NodeSelection {
    node: String,
    profile: String,
    version: String,
    provenance: Provenance,
    adapted: bool,
    effective_selector: String,
}

pub(super) fn adapt(
    base: &Graph,
    profiles: &ProfileRegistry,
    agents: &surge_acp::Registry,
    default: &str,
) -> Result<PlanningGraph, BootstrapRuntimeError> {
    let selected = agents
        .find_normalized(default)
        .ok_or(BootstrapRuntimeError::Configuration)?;
    let mut graph = base.clone();
    let mut decisions = Vec::new();
    for node in graph.nodes.values_mut().chain(
        graph
            .subgraphs
            .values_mut()
            .flat_map(|body| body.nodes.values_mut()),
    ) {
        let NodeConfig::Agent(agent) = &mut node.config else {
            continue;
        };
        let key = surge_core::profile::keyref::parse_key_ref(agent.profile.as_str())
            .map_err(|_| BootstrapRuntimeError::Configuration)?;
        let resolved = profiles
            .resolve(&key)
            .map_err(|_| BootstrapRuntimeError::Configuration)?;
        let adapted = eligible(agent, &resolved);
        if adapted {
            *agent = agent.with_runtime_override(&selected.id, None);
        }
        decisions.push(NodeSelection {
            node: node.id.as_str().into(),
            profile: resolved.profile.role.id.as_str().into(),
            version: resolved.profile.role.version.to_string(),
            provenance: resolved.provenance,
            adapted,
            effective_selector: effective_runtime_selector(agent, &resolved).into(),
        });
    }
    decisions.sort_by(|left, right| left.node.cmp(&right.node));
    let selection = content_ref(
        "bootstrap:planning-selection:v1",
        &(1u32, default, &selected.id, digest(selected)?, decisions),
    )?;
    Ok(PlanningGraph { graph, selection })
}

fn eligible(
    node: &surge_core::agent_config::AgentConfig,
    profile: &surge_core::profile::registry::ResolvedProfile,
) -> bool {
    ["description-author", "roadmap-planner", "flow-generator"]
        .contains(&profile.profile.role.id.as_str())
        && profile.profile.role.version.to_string() == "1.0.0"
        && profile.provenance == Provenance::Bundled
        && profile.chain.len() == 1
        && profile.profile.role.extends.is_none()
        && !node.custom_fields.contains_key("runtime")
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_orchestrator::profile_loader::DiskProfileSet;

    fn base() -> Graph {
        surge_core::BundledFlows::by_name_latest("bootstrap")
            .unwrap()
            .graph
    }

    #[test]
    fn eligibility_excludes_other_versions_roles_and_inheritance_even_for_bundled_leaf() {
        let root = tempfile::tempdir().unwrap();
        let profiles = ProfileRegistry::new(DiskProfileSet::scan(root.path()).unwrap());
        let node: surge_core::agent_config::AgentConfig =
            toml::from_str("profile = 'description-author@1.0'").unwrap();
        let profile = profiles
            .resolve(&surge_core::profile::keyref::parse_key_ref("description-author@1.0").unwrap())
            .unwrap();
        assert!(eligible(&node, &profile));
        let mut inherited = profile.clone();
        inherited.chain.push("operator-parent".try_into().unwrap());
        assert!(!eligible(&node, &inherited));
        let mut extending = profile.clone();
        extending.profile.role.extends = Some("operator-parent".try_into().unwrap());
        assert!(!eligible(&node, &extending));
        let mut newer = profile.clone();
        newer.profile.role.version.minor = 1;
        assert!(!eligible(&node, &newer));
        let mut other = profile;
        other.profile.role.id = "verifier".try_into().unwrap();
        assert!(!eligible(&node, &other));
    }

    #[test]
    fn exact_bundled_authors_follow_default_and_repeated_scoping_uses_base() {
        let root = tempfile::tempdir().unwrap();
        let profiles = ProfileRegistry::new(DiskProfileSet::scan(root.path()).unwrap());
        let agents = surge_acp::Registry::builtin();
        let base = base();
        let first = adapt(&base, &profiles, &agents, "codex-acp").unwrap();
        let second = adapt(&base, &profiles, &agents, "claude-acp").unwrap();
        let again = adapt(&base, &profiles, &agents, "codex-acp").unwrap();
        assert_eq!(first.graph, again.graph);
        assert_ne!(first.graph, second.graph);
        for name in ["description_author", "roadmap_planner", "flow_generator"] {
            let NodeConfig::Agent(agent) = &first.graph.nodes[&name.parse().unwrap()].config else {
                panic!()
            };
            assert_eq!(agent.runtime_override(), Some("codex-acp"));
            assert_eq!(agent.model_override(), None);
            assert_eq!(agent.effort_override(), None);
        }
        assert_eq!(base, self::base());
    }

    #[test]
    fn explicit_runtime_model_effort_and_unknown_runtime_fields_remain_exact() {
        let root = tempfile::tempdir().unwrap();
        let profiles = ProfileRegistry::new(DiskProfileSet::scan(root.path()).unwrap());
        let agents = surge_acp::Registry::builtin();
        for runtime in [
            toml::toml! { agent_id = "claude-acp" model = "chosen-model" effort = "high" },
            toml::toml! { model = "chosen-model" },
            toml::toml! { effort = "high" },
            toml::toml! { operator_field = "retained" },
        ] {
            let mut base = base();
            for node in base.nodes.values_mut() {
                if let NodeConfig::Agent(agent) = &mut node.config {
                    agent
                        .custom_fields
                        .insert("runtime".into(), toml::Value::Table(runtime.clone()));
                }
            }
            let selected = adapt(&base, &profiles, &agents, "codex-acp").unwrap();
            assert_eq!(selected.graph, base);
            let other = adapt(&base, &profiles, &agents, "claude-acp").unwrap();
            assert_eq!(selected.graph, other.graph);
            assert_ne!(selected.selection.digest, other.selection.digest);
        }
    }

    #[test]
    fn disk_latest_and_versioned_profiles_remain_operator_owned_and_pin_provenance() {
        let root = tempfile::tempdir().unwrap();
        let agents = surge_acp::Registry::builtin();
        let mut explicit = base();
        for node in explicit.nodes.values_mut() {
            if let NodeConfig::Agent(agent) = &mut node.config {
                *agent = agent.with_runtime_override("claude-acp", None);
            }
        }
        let profiles = ProfileRegistry::new(DiskProfileSet::scan(root.path()).unwrap());
        let first = adapt(&explicit, &profiles, &agents, "codex-acp").unwrap();
        let profile = surge_core::profile::bundled::BundledRegistry::all()
            .into_iter()
            .find(|profile| profile.role.id.as_str() == "description-author")
            .unwrap();
        std::fs::write(
            root.path().join("description-author.toml"),
            toml::to_string(&profile).unwrap(),
        )
        .unwrap();
        let profiles = ProfileRegistry::new(DiskProfileSet::scan(root.path()).unwrap());
        let second = adapt(&explicit, &profiles, &agents, "codex-acp").unwrap();
        assert_eq!(first.graph, second.graph);
        assert_ne!(first.selection.digest, second.selection.digest);
        for reference in ["description-author", "description-author@1.0"] {
            let mut graph = base();
            let NodeConfig::Agent(agent) = &mut graph
                .nodes
                .get_mut(&"description_author".parse().unwrap())
                .unwrap()
                .config
            else {
                panic!()
            };
            agent.profile = reference.try_into().unwrap();
            let selected = adapt(&graph, &profiles, &agents, "codex-acp").unwrap();
            let NodeConfig::Agent(agent) =
                &selected.graph.nodes[&"description_author".parse().unwrap()].config
            else {
                panic!()
            };
            assert_eq!(agent.runtime_override(), None);
        }
    }
}
