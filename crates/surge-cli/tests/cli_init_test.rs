use std::path::PathBuf;

use assert_cmd::Command;
use predicates::str::contains;
use surge_core::SurgeConfig;

fn clean_command() -> Command {
    let mut command = Command::cargo_bin("surge").unwrap();
    command
        .env_remove("CLAUDE_ACP_PATH")
        .env_remove("CLAUDE_PATH")
        .env_remove("CLAUDE_BIN")
        .env_remove("CODEX_ACP_PATH")
        .env_remove("CODEX_PATH")
        .env_remove("CODEX_BIN")
        .env_remove("GEMINI_PATH")
        .env_remove("GEMINI_BIN")
        .env_remove("GITHUB_COPILOT_CLI_PATH")
        .env_remove("COPILOT_PATH")
        .env_remove("GH_PATH");
    command
}

#[test]
fn init_help_lists_default_flag() {
    clean_command()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(contains("--default"));
}

#[test]
fn init_default_writes_valid_config() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    clean_command()
        .args(["init", "--default"])
        .current_dir(temp.path())
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .assert()
        .success()
        .stdout(contains("Created surge.toml"))
        .stdout(contains("Project context: project.md"));

    let config_path = temp.path().join("surge.toml");
    let config = SurgeConfig::load(&config_path).unwrap();

    assert_eq!(
        config.init.project_context_path,
        PathBuf::from("project.md")
    );
    assert!(
        config.agents.contains_key(&config.default_agent),
        "default_agent must reference a configured registry or fallback agent",
    );
    assert!(
        config.agents.keys().any(|id| {
            matches!(
                id.as_str(),
                "claude-acp" | "codex-acp" | "gemini" | "github-copilot-cli"
            )
        }),
        "init --default should register a builtin ACP agent",
    );
}

#[test]
fn init_default_existing_config_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    clean_command()
        .args(["init", "--default"])
        .current_dir(temp.path())
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .assert()
        .success();

    let config_path = temp.path().join("surge.toml");
    let before = std::fs::read_to_string(&config_path).unwrap();

    clean_command()
        .args(["init", "--default"])
        .current_dir(temp.path())
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .assert()
        .success()
        .stdout(contains("surge.toml already exists"))
        .stdout(contains("Default agent:"));

    let after = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(before, after, "existing config should be left unchanged");
}
