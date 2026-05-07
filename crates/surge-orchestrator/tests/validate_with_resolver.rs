//! Integration test: `validate_for_m6_with_resolver` rejects unknown profiles
//! while the syntactic `validate_for_m6` passes the same graph.

use std::path::Path;
use surge_core::ReferenceResolver;
use surge_core::graph::Graph;
use surge_orchestrator::engine::validate::{validate_for_m6, validate_for_m6_with_resolver};

fn load_example(name: &str) -> Graph {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(name);
    let toml_s = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read example {}: {}", path.display(), e));
    toml::from_str(&toml_s).unwrap_or_else(|e| panic!("parse {}: {}", path.display(), e))
}

struct StaticResolver {
    profiles: Vec<&'static str>,
}

impl ReferenceResolver for StaticResolver {
    fn profile_exists(&self, name: &str) -> bool {
        self.profiles.contains(&name)
    }
    fn template_exists(&self, _: &str) -> bool {
        true
    }
    fn named_agent_exists(&self, _: &str) -> bool {
        true
    }
}

#[test]
fn flow_minimal_agent_validates_with_known_profile() {
    let g = load_example("flow_minimal_agent.toml");
    // Structural pass first.
    validate_for_m6(&g).expect("structural validate should pass");

    let resolver = StaticResolver {
        profiles: vec!["implementer@1.0"],
    };
    validate_for_m6_with_resolver(&g, &resolver)
        .expect("graph should validate when profile is registered");
}

#[test]
fn flow_minimal_agent_fails_when_profile_missing() {
    let g = load_example("flow_minimal_agent.toml");
    let empty = StaticResolver {
        profiles: Vec::new(),
    };
    let result = validate_for_m6_with_resolver(&g, &empty);
    let err = result.expect_err("missing profile must surface as GraphInvalid");
    let msg = err.to_string();
    assert!(
        msg.contains("implementer@1.0"),
        "expected reference message to mention profile name, got: {msg}"
    );
}

#[test]
fn flow_terminal_only_validates_with_no_op_resolver() {
    let g = load_example("flow_terminal_only.toml");
    validate_for_m6(&g).expect("structural validate should pass");
    validate_for_m6_with_resolver(&g, &surge_core::NoOpResolver)
        .expect("terminal-only smoke path should pass NoOpResolver");
}
