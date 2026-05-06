//! Fixture-driven test for Triage Author. Two flavours:
//!
//! 1. Default: smoke check that fixtures parse as TOML.
//! 2. `--features _bootstrap_llm_test`: dispatch real Claude Haiku
//!    against the fixtures and assert decision matches.
//!
//! Run feature-gated:
//!   ANTHROPIC_TEST_KEY=sk-... cargo test -p surge-orchestrator \
//!     --test triage_llm --features _bootstrap_llm_test -- --ignored

#[test]
fn fixtures_compile() {
    use std::fs;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/triage_fixtures");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("fixtures dir") {
        let p = entry.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let contents = fs::read_to_string(&p).unwrap();
        let _: toml::Value = toml::from_str(&contents).expect("valid TOML");
        count += 1;
    }
    assert!(count >= 3, "expected at least 3 fixtures, found {count}");
}

#[cfg(feature = "_bootstrap_llm_test")]
mod llm {
    use std::sync::Arc;
    use std::time::Duration;
    use surge_acp::bridge::acp_bridge::AcpBridge;
    use surge_acp::bridge::facade::BridgeFacade;
    use surge_intake::types::{Priority, TaskDetails, TaskSummary, TriageDecision};
    use surge_orchestrator::triage::{TriageInput, TriageOptions, dispatch_triage};

    fn priority_distance(a: Priority, b: Priority) -> u32 {
        let rank = |p: Priority| -> u32 {
            match p {
                Priority::Low => 0,
                Priority::Medium => 1,
                Priority::High => 2,
                Priority::Urgent => 3,
            }
        };
        rank(a).abs_diff(rank(b))
    }

    #[derive(serde::Deserialize)]
    struct Fixture {
        input: FixtureInput,
        expected: FixtureExpected,
    }
    #[derive(serde::Deserialize)]
    struct FixtureInput {
        task: TaskDetails,
        #[serde(default)]
        candidates: Vec<TaskSummary>,
    }
    #[derive(serde::Deserialize)]
    struct FixtureExpected {
        decision: String,
        #[serde(default)]
        priority: Option<String>,
        #[serde(default)]
        duplicate_of: Option<String>,
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires ANTHROPIC_TEST_KEY and a real claude binary"]
    async fn fixtures_against_real_haiku() {
        let bridge = Arc::new(AcpBridge::with_defaults().expect("AcpBridge"));
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/triage_fixtures");

        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).unwrap();
            let fixture: Fixture = toml::from_str(&raw).unwrap();

            let input = TriageInput {
                task: fixture.input.task,
                candidates: fixture.input.candidates,
                active_runs: vec![],
            };
            let tmp = tempfile::tempdir().unwrap();
            let opts = TriageOptions {
                claude_binary: surge_orchestrator::triage::find_claude_binary(),
                attempt_timeout: Duration::from_secs(180),
                max_attempts: 1,
                scratch_root: tmp.path().to_path_buf(),
                keep_scratch_on_failure: true,
            };
            let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input, opts)
                .await
                .expect("dispatch_triage");

            let actual_decision = match &result {
                TriageDecision::Enqueued { .. } => "enqueued",
                TriageDecision::Duplicate { .. } => "duplicate",
                TriageDecision::OutOfScope { .. } => "out_of_scope",
                TriageDecision::Unclear { .. } => "unclear",
            };
            assert_eq!(
                actual_decision, fixture.expected.decision,
                "fixture {:?}: decision mismatch",
                path
            );

            if let (Some(exp_p), TriageDecision::Enqueued { priority, .. }) =
                (fixture.expected.priority.as_deref(), &result)
            {
                let exp = match exp_p {
                    "urgent" => Priority::Urgent,
                    "high" => Priority::High,
                    "medium" => Priority::Medium,
                    "low" => Priority::Low,
                    other => panic!("fixture has unknown priority: {other}"),
                };
                let dist = priority_distance(exp, *priority);
                assert!(
                    dist <= 1,
                    "fixture {:?}: priority {:?} too far from expected {:?}",
                    path,
                    priority,
                    exp
                );
            }

            if let (Some(exp_dup), TriageDecision::Duplicate { of, .. }) =
                (fixture.expected.duplicate_of.as_deref(), &result)
            {
                assert_eq!(
                    of.as_str(),
                    exp_dup,
                    "fixture {:?}: duplicate_of mismatch",
                    path
                );
            }
        }
    }
}
