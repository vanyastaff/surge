//! Integration checks for the first-party bootstrap flow asset.

use surge_core::profile::bundled::BundledRegistry;
use surge_core::{BundledFlows, ReferenceResolver, parse_key_ref};
use surge_orchestrator::engine::validate::{validate_for_m6, validate_for_m6_with_resolver};

struct BundledProfileResolver;

impl ReferenceResolver for BundledProfileResolver {
    fn profile_exists(&self, name: &str) -> bool {
        let Ok(key_ref) = parse_key_ref(name) else {
            return false;
        };
        match key_ref.version {
            Some(version) => {
                BundledRegistry::by_name_version(key_ref.name.as_str(), &version).is_some()
            },
            None => BundledRegistry::by_name_latest(key_ref.name.as_str()).is_some(),
        }
    }

    fn template_exists(&self, _: &str) -> bool {
        true
    }

    fn named_agent_exists(&self, _: &str) -> bool {
        true
    }
}

#[test]
fn bundled_bootstrap_flow_validates() {
    let flow = BundledFlows::by_name_latest("bootstrap").expect("bootstrap flow is bundled");
    validate_for_m6(&flow.graph).expect("bootstrap graph passes engine structural validation");
    validate_for_m6_with_resolver(&flow.graph, &BundledProfileResolver)
        .expect("bootstrap graph references only bundled profiles");
}

#[test]
fn bundled_bootstrap_flow_uses_expected_profiles() {
    let flow = BundledFlows::by_name_latest("bootstrap").expect("bootstrap flow is bundled");
    let mut profiles = flow
        .graph
        .nodes
        .values()
        .filter_map(|node| match &node.config {
            surge_core::NodeConfig::Agent(agent) => Some(agent.profile.as_str().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    profiles.sort();
    assert_eq!(
        profiles,
        vec![
            "description-author@1.0".to_owned(),
            "flow-generator@1.0".to_owned(),
            "roadmap-planner@1.0".to_owned(),
        ]
    );
}
