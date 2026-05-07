//! Unit tests for the schema-version migration registry.
//!
//! v1 today is the identity migration over the JSON-encoded
//! `VersionedEventPayload` wrapper. Older or newer versions get rejected with
//! the typed `SurgeError::SchemaTooOld` / `SchemaTooNew`.

use surge_core::error::SurgeError;
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{IdentityV1, MigrationChain, migrate_payload};

#[test]
fn v1_identity_roundtrip_preserves_payload() {
    let original = EventPayload::OutcomeReported {
        node: NodeKey::try_from("impl_1").unwrap(),
        outcome: surge_core::keys::OutcomeKey::try_from("done").unwrap(),
        summary: "implementation complete".into(),
    };
    let bytes = serde_json::to_vec(&VersionedEventPayload::new(original.clone())).unwrap();

    let result = migrate_payload(1, &bytes).expect("v1 migration should succeed");
    assert_eq!(result, original);

    // The chain's direct API yields the same outcome.
    let chain = MigrationChain::new();
    let result_chain = chain.migrate(1, &bytes).unwrap();
    assert_eq!(result_chain, original);
}

#[test]
fn schema_v0_returns_schema_too_old() {
    let bytes = b"\x00".to_vec();
    match migrate_payload(0, &bytes) {
        Err(SurgeError::SchemaTooOld { found, min }) => {
            assert_eq!(found, 0);
            assert_eq!(min, 1);
        },
        other => panic!("expected SchemaTooOld for v0, got {other:?}"),
    }
}

#[test]
fn schema_v999_returns_schema_too_new() {
    let bytes = b"{}".to_vec();
    match migrate_payload(999, &bytes) {
        Err(SurgeError::SchemaTooNew { found, max }) => {
            assert_eq!(found, 999);
            assert_eq!(max, 1);
        },
        other => panic!("expected SchemaTooNew for v999, got {other:?}"),
    }
}

#[test]
fn malformed_v1_payload_surfaces_decode_error() {
    let bytes = b"not-json".to_vec();
    match migrate_payload(1, &bytes) {
        Err(SurgeError::Spec(msg)) => {
            assert!(
                msg.contains("v1 payload decode"),
                "unexpected message: {msg}"
            );
        },
        other => panic!("expected SurgeError::Spec, got {other:?}"),
    }
}

#[test]
fn identity_v1_struct_implements_migration() {
    use surge_core::migrations::Migration;
    let m = IdentityV1;
    assert_eq!(m.version(), 1);

    let original = EventPayload::RunFailed {
        error: "boom".into(),
    };
    let bytes = serde_json::to_vec(&VersionedEventPayload::new(original.clone())).unwrap();
    let migrated = m.migrate(&bytes).unwrap();
    assert_eq!(migrated, original);
}
