//! CLI smoke: `surge bootstrap --help` lists prompt and resume forms.

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn bootstrap_help_lists_prompt_and_resume() {
    Command::cargo_bin("surge")
        .unwrap()
        .args(["bootstrap", "--help"])
        .assert()
        .success()
        .stdout(contains("[PROMPT]"))
        .stdout(contains("resume"))
        .stdout(contains("--worktree"));
}
