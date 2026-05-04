//! M6 CLI smoke: `surge engine --help` prints subcommand list.

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn engine_help_lists_subcommands() {
    Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "--help"])
        .assert()
        .success()
        .stdout(contains("run"))
        .stdout(contains("watch"))
        .stdout(contains("resume"))
        .stdout(contains("stop"))
        .stdout(contains("ls"))
        .stdout(contains("logs"));
}
