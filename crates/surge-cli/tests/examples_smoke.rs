//! Smoke test for every `examples/flow_*.toml` archetype.
//!
//! Loads each example, runs the syntactic graph validator, and runs the
//! engine validator with an in-memory profile resolver that knows the
//! placeholder profiles we ship. Driving the actual ACP runtime is the
//! job of Task 5.1 (`crates/surge-orchestrator/tests/archetypes_mock_test.rs`);
//! this test guards against regressions in the example shape itself.

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
        // Profiles referenced by the bundled archetype examples.
        matches!(name, "implementer@1.0" | "planner@1.0")
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
fn bundled_template_names_and_legacy_aliases_start_runs() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let surge_home = home.join(".surge");
    std::fs::create_dir_all(&surge_home).unwrap();
    let worktree = temp.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();

    for template in [
        "feature",
        "bug-fix",
        "bugfix",
        "fix",
        "refactor",
        "performance",
        "perf",
        "security",
        "sec",
        "docs",
        "doc",
        "migration",
        "migrate",
    ] {
        Command::cargo_bin("surge")
            .unwrap()
            .args(["engine", "run", "--template", template])
            .current_dir(&worktree)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("SURGE_HOME", &surge_home)
            .assert()
            .success()
            .stdout(contains("run-"));
    }
}

#[test]
fn onboarding_smoke_can_init_describe_and_start_example_run() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
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

    let example = examples_dir().join("flow_minimal_agent.toml");
    Command::cargo_bin("surge")
        .unwrap()
        .args([
            "engine",
            "run",
            example.to_str().unwrap(),
            "--worktree",
            temp.path().to_str().unwrap(),
        ])
        .current_dir(temp.path())
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .assert()
        .success()
        .stdout(contains("run-"));
}
