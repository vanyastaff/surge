//! `surge memory audit --format json` end-to-end, through the real `surge`
//! binary — the second half of R21 (`.autopilot/competitive-waves/tickets/
//! 08-memory-audit.md`): correlating memory claims with runs the engine's
//! loop guard stopped, via the typed `EscalationCause` trail task 17 added,
//! not by parsing `reason` prose.
//!
//! Both the run (with its `EscalationRequested` escalation) and the memory
//! claim are seeded directly through the persistence layer, matching
//! `cli_replay.rs`'s pattern — no agent runtime needed, since the audit is a
//! pure read over already-durable state. Both are seeded under the same
//! `SURGE_HOME` the real binary is launched with, so this test also proves
//! `MemoryStore::default_path()` actually honors `SURGE_HOME` (the third
//! debt item this restart closes) — a `surge memory audit` run against a
//! `MemoryStore` opened at any other path would find no claims at all and
//! this test would fail with an empty `run_correlated`, not a compile error.

use std::path::Path;

use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::memory::MemoryClaim;
use surge_core::run_event::{EscalationCause, EventPayload, VersionedEventPayload};
use surge_core::{ContentHash, RunStatus};
use surge_persistence::memory::MemoryStore;
use surge_persistence::runs::Storage;

/// Seed one run whose durable log carries a loop-guard escalation, then
/// mark its registry status `Crashed` (never `Failed`) — the exact race
/// `list_loop_guard_stopped_runs`/`failed_run_correlated_claims` document:
/// a guard trip that outlives a dead daemon. If the CLI's correlation ever
/// regressed to filtering on `RunStatus::Failed` alone, this run would
/// silently drop out and this test would fail.
async fn seed_loop_guard_stopped_run(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, home, None).await.unwrap();
    let node = NodeKey::try_from("implement").unwrap();
    writer
        .append_events(vec![
            VersionedEventPayload::new(EventPayload::StageEntered { node, attempt: 1 }),
            VersionedEventPayload::new(EventPayload::EscalationRequested {
                stage: None,
                reason: "node loop guard: node has run for 0s, past its 0s wall-clock budget; \
                          escalating"
                    .into(),
                cause: EscalationCause::LoopGuardNodeDeadline,
            }),
        ])
        .await
        .unwrap();
    writer.flush().await.unwrap();
    drop(writer);

    storage
        .set_run_status(&run_id, RunStatus::Crashed, Some(1))
        .await
        .unwrap();
    run_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_audit_json_correlates_a_claim_with_a_loop_guard_stopped_run() {
    let home = tempfile::tempdir().unwrap();
    let run_id = seed_loop_guard_stopped_run(home.path()).await;

    // Written at exactly the path `MemoryStore::default_path()` must
    // resolve to once the real binary below sees `SURGE_HOME=home`.
    let memory_store = MemoryStore::open(&home.path().join("memory.db")).unwrap();
    let claim = MemoryClaim::from_transcript(
        "root cause was the node running past its wall-clock budget",
        format!("transcript:{run_id}#turn-2"),
        ContentHash::compute(b"turn 2"),
    );
    memory_store.add_claim(&claim).unwrap();
    drop(memory_store);

    let assert = assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", home.path())
        .args(["memory", "audit", "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON: {e}\n{stdout}"));

    let run_correlated = json["run_correlated"]
        .as_array()
        .expect("run_correlated array");
    assert_eq!(
        run_correlated.len(),
        1,
        "expected exactly one correlated claim: {stdout}"
    );
    let finding = &run_correlated[0];
    // `RunId`'s `Display` (used by `to_string()`) prefixes with `run-`;
    // its `Serialize` impl (what the JSON output actually carries) does
    // not — compare against the same serialization the CLI produced
    // rather than assuming they match.
    assert_eq!(finding["run_id"], serde_json::to_value(run_id).unwrap());
    assert_eq!(finding["run_status"], "crashed");
    assert_eq!(finding["loop_guard_cause"], "loop_guard_node_deadline");
}

/// The default output format — no `--format` flag — is what an operator
/// actually sees per the ticket's own scenario ("оператор запускает аудит и
/// видит записи"), and grew a `caveats`/`ℹ️  Notes` section plus loop-guard
/// wording this same restart, entirely in hand-written `println!` calls
/// with no test covering them at all until now. Proves the human-readable
/// path renders the loop-guard correlation, not just the JSON one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_audit_text_reports_a_claim_stopped_by_the_loop_guard() {
    let home = tempfile::tempdir().unwrap();
    let run_id = seed_loop_guard_stopped_run(home.path()).await;

    let memory_store = MemoryStore::open(&home.path().join("memory.db")).unwrap();
    let claim = MemoryClaim::from_transcript(
        "root cause was the node running past its wall-clock budget",
        format!("transcript:{run_id}#turn-2"),
        ContentHash::compute(b"turn 2"),
    );
    memory_store.add_claim(&claim).unwrap();
    drop(memory_store);

    assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", home.path())
        .args(["memory", "audit"])
        .assert()
        .success()
        .stdout(predicates::str::contains("stopped by loop guard"))
        .stdout(predicates::str::contains(run_id.to_string()));
}
