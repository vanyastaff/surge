//! Credentials never gain a plaintext argv-to-registry production path.
mod runtime_home_fixture {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}
use runtime_home_fixture::FixtureHome;

#[test]
fn plaintext_token_argument_is_rejected_without_echo_or_persistence() {
    let home = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_surge"))
        .args([
            "telegram",
            "setup",
            "--token",
            "73123:REGRESSION_SECRET_LITERAL",
        ])
        .env("SURGE_HOME", home.path())
        .current_dir(home.path())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!text.contains("73123:REGRESSION_SECRET_LITERAL"));
    assert!(
        !output.status.success(),
        "plaintext argv token was accepted and persisted"
    );
    assert!(!home.path().join("db/registry.sqlite").exists());
}

#[test]
fn setup_persists_only_reference_and_target_and_removes_legacy_token() {
    let home = FixtureHome::new().unwrap();
    {
        let pool = surge_persistence::runs::registry::open_registry_pool(
            home.path(),
            &surge_persistence::runs::SystemClock,
        )
        .unwrap();
        let conn = pool.get().unwrap();
        surge_persistence::secrets::set_secret(
            &conn,
            surge_persistence::secrets::TELEGRAM_BOT_TOKEN_KEY,
            "73123:OLD_SECRET_LITERAL",
            0,
        )
        .unwrap();
        std::fs::write(
            home.path().join("surge.toml"),
            "[telegram]\nbot_token = '73123:OLD_CONFIG_SECRET'\n",
        )
        .unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_surge"))
            .args([
                "telegram",
                "setup",
                "--token-env",
                "SURGE_TEST_BOT_TOKEN",
                "--chat-id",
                "-42",
            ])
            .env("SURGE_TEST_BOT_TOKEN", "73123:NEW_SECRET_LITERAL")
            .env("SURGE_HOME", home.path())
            .current_dir(home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains("SECRET_LITERAL"));
        let config = std::fs::read_to_string(home.path().join("surge.toml")).unwrap();
        assert!(!config.contains("SECRET"));
        assert!(config.contains("SURGE_TEST_BOT_TOKEN"));
        assert!(
            !surge_persistence::secrets::has_secret(
                &conn,
                surge_persistence::secrets::TELEGRAM_BOT_TOKEN_KEY
            )
            .unwrap()
        );
        let target: i64 = conn
            .query_row(
                "SELECT target_chat_id FROM telegram_pairing_tokens",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(target, -42);
    }
    home.close().unwrap();
}
