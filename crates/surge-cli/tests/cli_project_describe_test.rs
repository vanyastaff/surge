use assert_cmd::Command;
use predicates::str::contains;

fn write_minimal_project(root: &std::path::Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
resolver = "2"
members = []

[workspace.package]
edition = "2024"
"#,
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "# Example\n").unwrap();
    std::fs::write(
        root.join("AGENTS.md"),
        "- No `unwrap()` / `expect()` in library code.\n- Use `tracing::*` macros.\n",
    )
    .unwrap();
}

fn surge_command(home: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("surge").unwrap();
    command.env("HOME", home).env("USERPROFILE", home);
    command
}

#[test]
fn project_describe_dry_run_reports_change_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_minimal_project(temp.path());

    surge_command(&home)
        .args([
            "project",
            "describe",
            "--dry-run",
            "--author-mode",
            "deterministic",
        ])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(contains("Would update"))
        .stdout(contains("Outcome: would_draft"))
        .stdout(contains("Agent runtime: claude-acp"));

    assert!(
        !temp.path().join("project.md").exists(),
        "--dry-run must not write project.md",
    );
}

#[test]
fn project_describe_writes_then_reports_no_change() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_minimal_project(temp.path());

    surge_command(&home)
        .args(["project", "describe", "--author-mode", "deterministic"])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(contains("Wrote"))
        .stdout(contains("Outcome: drafted"));

    let project_md = temp.path().join("project.md");
    let first = std::fs::read_to_string(&project_md).unwrap();
    assert!(first.contains("surge:project-context scan_hash="));
    assert!(first.contains("## Project name"));

    surge_command(&home)
        .args([
            "project",
            "describe",
            "--dry-run",
            "--author-mode",
            "deterministic",
        ])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(contains("No changes needed"))
        .stdout(contains("Outcome: would_no_change"));

    let second = std::fs::read_to_string(&project_md).unwrap();
    assert_eq!(first, second);
}

#[test]
fn project_describe_from_subdirectory_writes_to_repo_root() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_minimal_project(temp.path());
    let subdir = temp.path().join("crates").join("surge-cli");
    std::fs::create_dir_all(&subdir).unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    surge_command(&home)
        .args(["project", "describe", "--author-mode", "deterministic"])
        .current_dir(&subdir)
        .assert()
        .success()
        .stdout(contains("Wrote"));

    assert!(temp.path().join("project.md").exists());
    assert!(!subdir.join("project.md").exists());
}
