use std::process::Stdio;
use std::time::Duration;

mod producer {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/failed_settlement.rs"
    ));
}

const CHILD: &str = "SURGE_HISTORY_BYPASS_ORACLE_CHILD";
const RECEIPT: &str = "SURGE_HISTORY_BYPASS_ORACLE_RECEIPT";
const TEST: &str =
    "commands::engine::history_bypass_tests::history_after_actual_settlement_failure_cli_reopen";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_after_actual_settlement_failure_cli_reopen() {
    if std::env::var_os(CHILD).is_some() {
        let home = super::surge_runs_dir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let run = producer::produce(&home, worktree.path()).await;
        let outcome = super::require_completed_history(run).await;
        worktree.close().unwrap();
        std::fs::write(
            std::env::var_os(RECEIPT).unwrap(),
            if outcome.is_err() {
                "rejected"
            } else {
                "authorized"
            },
        )
        .unwrap();
        return;
    }
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let receipt = tempfile::NamedTempFile::new().unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD, "1")
        .env("SURGE_HOME", home.path())
        .env(RECEIPT, receipt.path())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(60), child.wait()).await;
    let status = match result {
        Ok(status) => Some(status.unwrap()),
        Err(_) => {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
            None
        },
    };
    let observed = std::fs::read_to_string(receipt.path()).unwrap();
    home.close().unwrap();
    receipt.close().unwrap();
    assert!(
        status.is_some_and(|status| status.success()),
        "actual CLI historical oracle child failed: {status:?}"
    );
    assert_eq!(
        observed, "rejected",
        "CLI reopened journal authorized success after actual Engine SettlementFailed"
    );
}
