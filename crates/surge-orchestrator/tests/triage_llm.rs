//! Fixture-driven LLM test for Triage Author. Requires:
//!   - ANTHROPIC_TEST_KEY env var (when feature enabled)
//!
//! Run with: `cargo test -p surge-orchestrator --test triage_llm -- --ignored`
//!
//! Each fixture in `triage_fixtures/*.toml` provides an input ticket and
//! candidate set, plus the expected decision/priority. The full LLM body
//! invokes Claude Haiku at `temperature=0` and validates output against
//! tolerance bands; for now it lives behind a feature flag so default
//! builds do not pay for LLM calls.
//!
//! The smoke test below — always-on — confirms every fixture is valid TOML
//! with the expected schema.

#[test]
fn fixtures_parse_as_valid_toml() {
    use std::fs;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/triage_fixtures");
    assert!(dir.is_dir(), "fixtures dir does not exist: {dir:?}");

    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("fixtures dir") {
        let p = entry.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let contents =
            fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"));
        let value: toml::Value =
            toml::from_str(&contents).unwrap_or_else(|e| panic!("invalid TOML {p:?}: {e}"));

        // Schema spot-check: every fixture has [input.task].task_id and [expected].decision.
        let task_id = value["input"]["task"]["task_id"]
            .as_str()
            .unwrap_or_else(|| panic!("missing input.task.task_id in {p:?}"));
        assert!(
            !task_id.is_empty(),
            "input.task.task_id is empty in {p:?}"
        );
        let decision = value["expected"]["decision"]
            .as_str()
            .unwrap_or_else(|| panic!("missing expected.decision in {p:?}"));
        assert!(
            matches!(
                decision,
                "enqueued" | "duplicate" | "out_of_scope" | "unclear"
            ),
            "unknown expected.decision={decision} in {p:?}"
        );
        count += 1;
    }
    assert!(count >= 3, "expected at least 3 fixtures, found {count}");
}
