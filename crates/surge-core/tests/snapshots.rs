//! Snapshot tests for handcrafted fixtures.
//!
//! Run: `cargo test -p surge-core --test snapshots`
//! Accept new snapshots: `cargo insta accept`

use std::path::Path;
use surge_core::{validate, Graph};

fn load_fixture(name: &str) -> Graph {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/graphs")
        .join(name);
    let toml_s = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("failed to read fixture {}: {}", path.display(), e)
    });
    toml::from_str(&toml_s).unwrap_or_else(|e| {
        panic!("failed to parse fixture {}: {}", path.display(), e)
    })
}

// ── T30: six handcrafted fixtures ────────────────────────────────────────────

#[test]
fn linear_trivial_validates_and_snapshots() {
    let g = load_fixture("linear-trivial.toml");
    let result = validate(&g);
    assert!(result.is_ok(), "linear-trivial validate failed: {:?}", result);
    insta::assert_debug_snapshot!(g);
}

#[test]
fn linear_with_review_validates() {
    let g = load_fixture("linear-with-review.toml");
    assert!(validate(&g).is_ok());
}

#[test]
fn single_milestone_loop_validates() {
    let g = load_fixture("single-milestone-loop.toml");
    let result = validate(&g);
    assert!(result.is_ok(), "single-milestone-loop validate failed: {:?}", result);
}

#[test]
fn nested_3_levels_validates_and_snapshots() {
    let g = load_fixture("nested-3-levels.toml");
    let result = validate(&g);
    assert!(result.is_ok(), "nested-3-levels failed to validate: {:?}", result);
    insta::assert_debug_snapshot!(g);
}

#[test]
fn bug_fix_flow_validates() {
    let g = load_fixture("bug-fix-flow.toml");
    assert!(validate(&g).is_ok());
}

#[test]
fn refactor_flow_validates() {
    let g = load_fixture("refactor-flow.toml");
    assert!(validate(&g).is_ok());
}

#[test]
fn linear_trivial_toml_roundtrips() {
    let g = load_fixture("linear-trivial.toml");
    let toml_s = toml::to_string(&g).unwrap();
    let parsed: Graph = toml::from_str(&toml_s).unwrap();
    assert_eq!(g, parsed);
}

// ── T31: real-world fixture ───────────────────────────────────────────────────

#[test]
fn real_world_roadmap_validates() {
    let g = load_fixture("real-world-roadmap.toml");
    let result = validate(&g);
    assert!(result.is_ok(), "real-world fixture failed: {:?}", result);
    insta::assert_debug_snapshot!(g);
}
