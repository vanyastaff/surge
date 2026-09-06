//! `surge skill list|show|verify` (R16) — CLI-surface tests for the seam
//! named in `.autopilot/competitive-waves/interfaces.md` §3: "existing CLI
//! tests, output of new commands." Exercises the real `surge` binary against
//! a real `SKILL.md` pack on disk, not `surge_core::skill`'s internals
//! (those are task 02's own seam).

use assert_cmd::Command;
use predicates::str::contains;

/// Point `surge`'s user-root discovery (`~/.claude/skills`,
/// `~/.claude/plugins`) at an isolated temp `home`, the same override
/// `cli_project_describe_test.rs` uses, so a real `~/.claude` on the host
/// running the test suite can never leak into the discovered catalog.
fn surge_command(home: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("surge").unwrap();
    command.env("HOME", home).env("USERPROFILE", home);
    command
}

/// Write a minimal, well-formed Agent Skills pack at
/// `<project_root>/.claude/skills/<name>/SKILL.md`, with one extra file
/// alongside it so `show`'s file list has more than one entry to prove.
fn write_project_skill(project_root: &std::path::Path, name: &str, version: &str, body: &str) {
    let dir = project_root.join(".claude").join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\nversion: \"{version}\"\n---\n\n{body}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("reference.md"), "supporting reference material\n").unwrap();
}

#[test]
fn skill_list_prints_name_provider_version_and_hash() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_project_skill(
        temp.path(),
        "demo-skill",
        "1.0.0",
        "Follow the demo procedure.",
    );

    // Text format: the human table names the pack by the fields R16 requires.
    surge_command(&home)
        .args(["skill", "list"])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(contains("demo-skill"))
        .stdout(contains("project"))
        .stdout(contains("1.0.0"));

    // JSON format: the same pack, machine-readable, with the full content
    // hash Surge computed from the fixture just written (not a value this
    // test derives by re-running the hashing algorithm itself).
    let output = surge_command(&home)
        .args(["skill", "list", "--format", "json"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "list --format json failed: {output:?}"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let entries = value
        .as_array()
        .expect("list --format json returns an array");
    assert_eq!(
        entries.len(),
        1,
        "expected exactly the one fixture pack, got {entries:?}"
    );
    let entry = &entries[0];
    assert_eq!(entry["name"], "demo-skill");
    assert_eq!(entry["provider"], "project_dir");
    assert_eq!(entry["version"], "1.0.0");
    let hash = entry["hash"].as_str().expect("hash is a string");
    assert!(
        hash.starts_with("sha256:") && hash.len() == "sha256:".len() + 64,
        "expected a full sha256:<64 hex> hash, got {hash:?}"
    );
}

#[test]
fn skill_show_prints_instructions_and_files() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_project_skill(
        temp.path(),
        "demo-skill",
        "1.0.0",
        "Follow the demo procedure carefully, step by step.",
    );

    surge_command(&home)
        .args(["skill", "show", "demo-skill"])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(contains(
            "Follow the demo procedure carefully, step by step.",
        ))
        .stdout(contains("SKILL.md"))
        .stdout(contains("reference.md"));
}

#[test]
fn skill_verify_succeeds_when_hash_matches() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_project_skill(
        temp.path(),
        "demo-skill",
        "1.0.0",
        "Original procedure text.",
    );

    // Pin captured through the tool's own `show` output — not recomputed by
    // this test's own hashing, so the assertion below is a genuine
    // round-trip check (list/show's pin verifies clean), not a tautology.
    let show_output = surge_command(&home)
        .args(["skill", "show", "demo-skill", "--format", "json"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(show_output.status.success());
    let show_json: serde_json::Value = serde_json::from_slice(&show_output.stdout).unwrap();
    let pin = show_json["hash"].as_str().unwrap().to_string();

    surge_command(&home)
        .args(["skill", "verify", "demo-skill", "--hash", &pin])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(contains("MATCH"));
}

#[test]
fn skill_show_with_provider_registry_explains_the_unconfigured_root() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_project_skill(
        temp.path(),
        "demo-skill",
        "1.0.0",
        "Follow the demo procedure.",
    );

    // `--provider registry` is a selectable value (`SkillProviderArg` offers
    // it), but no registry root is ever wired up (`surge.toml` has no such
    // key — Решение §22). The operator must be told that reason, not handed
    // a silent empty/not-found result that reads as "this skill isn't
    // there" rather than "no registry is configured at all".
    surge_command(&home)
        .args(["skill", "show", "demo-skill", "--provider", "registry"])
        .current_dir(temp.path())
        .assert()
        .failure()
        .stderr(contains("registry"))
        .stderr(contains("surge.toml"));
}

#[test]
fn skill_verify_fails_on_real_drift() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_project_skill(
        temp.path(),
        "demo-skill",
        "1.0.0",
        "Original procedure text.",
    );

    // Capture the pin before the pack changes on disk.
    let show_output = surge_command(&home)
        .args(["skill", "show", "demo-skill", "--format", "json"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(show_output.status.success());
    let show_json: serde_json::Value = serde_json::from_slice(&show_output.stdout).unwrap();
    let stale_pin = show_json["hash"].as_str().unwrap().to_string();

    // The pack's content genuinely changes on disk between the pin and the
    // check — not a fabricated foreign hash — so drift is real, not staged.
    write_project_skill(
        temp.path(),
        "demo-skill",
        "1.0.0",
        "Updated procedure text.",
    );

    surge_command(&home)
        .args(["skill", "verify", "demo-skill", "--hash", &stale_pin])
        .current_dir(temp.path())
        .assert()
        .failure()
        .stdout(contains("DRIFT DETECTED"));
}
