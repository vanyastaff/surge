//! Smoke test for every `examples/flow_*.toml` archetype.
//!
//! Loads each example, runs the syntactic graph validator, and runs the
//! engine validator with an in-memory profile resolver that knows the
//! bundled profiles and the legacy mock planner. Driving the actual ACP runtime is the
//! job of Task 5.1 (`crates/surge-orchestrator/tests/archetypes_mock_test.rs`);
//! this test guards against regressions in the example shape itself.

mod runtime_home_fixture {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}
use runtime_home_fixture::FixtureHome;

#[path = "common/owned_flow.rs"]
mod owned_flow;

use assert_cmd::Command;
use predicates::str::contains;
use std::path::{Path, PathBuf};
use surge_core::ReferenceResolver;
use surge_core::graph::Graph;
use surge_orchestrator::engine::validate::{validate_for_m6, validate_for_m6_with_resolver};

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

fn load(name: &str) -> Graph {
    let path = examples_dir().join(name);
    let toml_s =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    toml::from_str(&toml_s).unwrap_or_else(|e| panic!("parse {}: {}", path.display(), e))
}

struct ArchetypeResolver;

impl ReferenceResolver for ArchetypeResolver {
    fn profile_exists(&self, name: &str) -> bool {
        // Legacy mock examples still use planner; delivery examples must
        // reference profiles that actually ship, rather than a stale allowlist.
        name == "planner@1.0"
            || surge_core::BundledRegistry::all().iter().any(|profile| {
                name == format!(
                    "{}@{}.{}",
                    profile.role.id, profile.role.version.major, profile.role.version.minor
                )
            })
    }
    fn template_exists(&self, _: &str) -> bool {
        true
    }
    fn named_agent_exists(&self, _: &str) -> bool {
        true
    }
}

fn assert_archetype_clean(name: &str) {
    let g = load(name);
    validate_for_m6(&g).unwrap_or_else(|e| panic!("{name}: structural validate failed: {e}"));
    validate_for_m6_with_resolver(&g, &ArchetypeResolver)
        .unwrap_or_else(|e| panic!("{name}: resolver validate failed: {e}"));
}

#[test]
fn flow_terminal_only_validates() {
    assert_archetype_clean("flow_terminal_only.toml");
}

#[test]
fn flow_minimal_agent_validates() {
    assert_archetype_clean("flow_minimal_agent.toml");
}

#[test]
fn flow_linear_3_validates() {
    assert_archetype_clean("flow_linear_3.toml");
}

#[test]
fn flow_single_loop_validates() {
    assert_archetype_clean("flow_single_loop.toml");
}

#[test]
fn flow_multi_milestone_validates() {
    assert_archetype_clean("flow_multi_milestone.toml");
}

#[test]
fn flow_bug_fix_validates() {
    assert_archetype_clean("flow_bug_fix.toml");
}

#[test]
fn flow_refactor_validates() {
    assert_archetype_clean("flow_refactor.toml");
}

#[test]
fn flow_spike_validates() {
    assert_archetype_clean("flow_spike.toml");
}

#[test]
fn bundled_template_names_and_legacy_aliases_resolve_valid_graphs() {
    let temp = tempfile::tempdir().unwrap();
    let registry =
        surge_orchestrator::archetype_registry::ArchetypeRegistry::from_dir(temp.path()).unwrap();
    for (name, canonical) in [
        ("feature", "feature"),
        ("bug-fix", "bug-fix"),
        ("bugfix", "bug-fix"),
        ("fix", "bug-fix"),
        ("refactor", "refactor"),
        ("performance", "performance"),
        ("perf", "performance"),
        ("security", "security"),
        ("sec", "security"),
        ("docs", "docs"),
        ("doc", "docs"),
        ("migration", "migration"),
        ("migrate", "migration"),
        ("code-review", "code-review"),
        ("review", "code-review"),
    ] {
        let resolved = registry.resolve(name).unwrap();
        assert_eq!(resolved.name, canonical);
        validate_for_m6(&resolved.graph).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn onboarding_smoke_can_init_describe_and_start_example_run() {
    let temp = tempfile::tempdir().unwrap();
    let runtime_home = FixtureHome::new().unwrap();
    {
        let home_dir = tempfile::tempdir().unwrap();
        let home = home_dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(temp.path().join("README.md"), "# Smoke\n").unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            r#"[workspace]
resolver = "2"
members = []
"#,
        )
        .unwrap();

        Command::cargo_bin("surge")
            .unwrap()
            .args(["init", "--default"])
            .current_dir(temp.path())
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .assert()
            .success();

        Command::cargo_bin("surge")
            .unwrap()
            .args(["project", "describe", "--author-mode", "deterministic"])
            .current_dir(temp.path())
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .assert()
            .success();

        std::fs::copy(
            examples_dir().join("flow_terminal_only.toml"),
            temp.path().join("flow.toml"),
        )
        .unwrap();
        owned_flow::commit_project(temp.path());
        let daemon = owned_flow::start_daemon(runtime_home.path(), temp.path(), None);
        let workspace = home_dir.path().join("workspace");
        let execution = Command::cargo_bin("surge")
            .unwrap()
            .args([
                "engine",
                "run",
                "flow.toml",
                "--worktree",
                workspace.to_str().unwrap(),
            ])
            .current_dir(temp.path())
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("SURGE_HOME", runtime_home.path())
            .timeout(std::time::Duration::from_secs(15))
            .assert()
            .success()
            .stdout(contains("run-"));
        let run_id = String::from_utf8(execution.get_output().stdout.clone()).unwrap();
        let replay = Command::cargo_bin("surge")
            .unwrap()
            .args(["engine", "replay", run_id.trim(), "--format", "json"])
            .env("SURGE_HOME", runtime_home.path())
            .current_dir(temp.path())
            .timeout(std::time::Duration::from_secs(15))
            .assert()
            .success();
        let state: serde_json::Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
        assert_eq!(state["view"]["terminal"], "completed");
        daemon.close().unwrap();
    }
    runtime_home.close().unwrap();
}
