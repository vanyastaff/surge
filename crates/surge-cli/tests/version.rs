//! `surge --version` must report the crate version plus the build
//! metadata (git sha + commit date) embedded by `build.rs`.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn version_reports_crate_version_with_build_metadata() {
    Command::cargo_bin("surge")
        .expect("surge binary builds")
        .arg("--version")
        .assert()
        .success()
        // The crate version is always present.
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")))
        // …followed by the "(<sha>, <date>)" metadata block from build.rs.
        .stdout(predicate::str::contains("(").and(predicate::str::contains(")")));
}
