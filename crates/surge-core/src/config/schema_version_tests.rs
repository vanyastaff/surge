use super::*;

#[test]
fn default_config_is_schema_version_one() {
    assert_eq!(CONFIG_SCHEMA_VERSION, 1);
    assert_eq!(SurgeConfig::default().schema_version, CONFIG_SCHEMA_VERSION);
}

#[test]
fn config_without_schema_version_defaults_to_one() {
    // Existing surge.toml files predate the field; they must still parse
    // and be treated as schema 1.
    let cfg: SurgeConfig = toml::from_str("default_agent = \"a\"\n").unwrap();
    assert_eq!(cfg.schema_version, 1);
}

#[test]
fn future_schema_version_is_rejected_with_migration_hint() {
    // A config written by a newer surge must not be silently misread by
    // an older binary — reject with an actionable message.
    let cfg: SurgeConfig = toml::from_str("schema_version = 2\n").unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("schema_version"), "got: {err}");
    assert!(
        err.contains('2'),
        "should name the unsupported version; got: {err}"
    );
}
