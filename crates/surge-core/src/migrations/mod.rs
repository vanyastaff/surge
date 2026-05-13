//! Schema-version migration registry for persisted `EventPayload` bytes.
//!
//! Persistence reads invoke [`migrate_payload`] to turn the on-disk bytes back
//! into a current [`EventPayload`]. Today the registry contains only the v1
//! identity migration (the current shape); future schema-breaking changes add
//! a new [`Migration`] entry with its own version constant.
//!
//! The entry point lives here (in `surge-core`) — not inside
//! `VersionedEventPayload::deserialize` — to keep the layering correct: the
//! persistence adapter reads `schema_version` from the events table column and
//! then asks `surge-core` to translate the payload bytes for that version.

use crate::error::SurgeError;
use crate::run_event::{EventPayload, VersionedEventPayload};

/// Minimum schema version this build understands.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// Maximum schema version this build understands.
///
/// **v2 (introduced 2026-05):** adds [`EventPayload::SandboxElevationTimedOut`]
/// and [`EventPayload::RuntimeVersionWarning`] variants. Purely additive — old
/// v1 payloads parse cleanly through the v2 decoder (they simply never
/// contain the new variants), but the wrapper's `schema_version` is bumped
/// from 1 to 2 for new writes so downstream readers know the variants are in
/// scope.
pub const MAX_SUPPORTED_VERSION: u32 = 2;

/// Single schema-version translator.
pub trait Migration: Send + Sync {
    /// The schema version these bytes were written under.
    fn version(&self) -> u32;
    /// Decode the bytes into the current [`EventPayload`].
    ///
    /// # Errors
    /// Returns [`SurgeError`] when the bytes are malformed or fail to project
    /// onto the current `EventPayload` shape.
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError>;
}

/// Identity migration for v1 — the current on-disk shape, where bytes
/// contain a JSON-encoded [`VersionedEventPayload`] wrapper.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV1;

impl Migration for IdentityV1 {
    fn version(&self) -> u32 {
        1
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v1 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity migration for v2 — the schema bump that introduced
/// `SandboxElevationTimedOut` and `RuntimeVersionWarning`. The wire shape is
/// unchanged from v1 (same JSON-encoded [`VersionedEventPayload`] wrapper),
/// so decoding is identical; the wrapper's `schema_version` field is the
/// only signal that distinguishes v1 and v2 payloads.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV2;

impl Migration for IdentityV2 {
    fn version(&self) -> u32 {
        2
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v2 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Ordered registry of [`Migration`]s indexed by their declared version.
pub struct MigrationChain {
    migrations: Vec<Box<dyn Migration>>,
}

impl MigrationChain {
    /// Build the default chain. Contains [`IdentityV1`] and [`IdentityV2`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            migrations: vec![Box::new(IdentityV1), Box::new(IdentityV2)],
        }
    }

    /// Translate `bytes` written under `version` into the current `EventPayload`.
    ///
    /// # Errors
    /// - [`SurgeError::SchemaTooOld`] when `version < MIN_SUPPORTED_VERSION`.
    /// - [`SurgeError::SchemaTooNew`] when `version > MAX_SUPPORTED_VERSION`.
    /// - [`SurgeError::Spec`] when the chain has no migration registered for
    ///   `version` despite it being in the supported range, or when the
    ///   payload bytes fail to decode.
    pub fn migrate(&self, version: u32, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        if version < MIN_SUPPORTED_VERSION {
            return Err(SurgeError::SchemaTooOld {
                found: version,
                min: MIN_SUPPORTED_VERSION,
            });
        }
        if version > MAX_SUPPORTED_VERSION {
            return Err(SurgeError::SchemaTooNew {
                found: version,
                max: MAX_SUPPORTED_VERSION,
            });
        }
        let migration = self
            .migrations
            .iter()
            .find(|m| m.version() == version)
            .ok_or_else(|| {
                SurgeError::Spec(format!(
                    "migration registry has no entry for schema version {version}"
                ))
            })?;
        migration.migrate(bytes)
    }
}

impl Default for MigrationChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience entry point used by `surge-persistence` on every read.
///
/// Equivalent to `MigrationChain::new().migrate(version, bytes)`. Call sites
/// that need to swap the chain for tests construct their own [`MigrationChain`].
///
/// # Errors
/// See [`MigrationChain::migrate`].
pub fn migrate_payload(version: u32, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
    MigrationChain::new().migrate(version, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::NodeKey;
    use crate::run_event::EventPayload;

    fn make_v1_bytes(payload: EventPayload) -> Vec<u8> {
        // Manually craft a v1-shape wrapper so the migration is exercised with
        // realistic on-disk bytes (rather than re-using the writer that now
        // emits v2 by default).
        let wrapper = serde_json::json!({
            "schema_version": 1,
            "payload": payload,
        });
        serde_json::to_vec(&wrapper).expect("serialize v1 wrapper")
    }

    fn make_v2_bytes(payload: EventPayload) -> Vec<u8> {
        let wrapper = VersionedEventPayload::new(payload);
        serde_json::to_vec(&wrapper).expect("serialize v2 wrapper")
    }

    #[test]
    fn v1_bytes_round_trip_through_chain() {
        let payload = EventPayload::SandboxElevationRequested {
            node: NodeKey::try_from("impl_1").unwrap(),
            capability: "fs-write:./src".into(),
        };
        let bytes = make_v1_bytes(payload.clone());
        let migrated = migrate_payload(1, &bytes).expect("v1 migrates");
        assert_eq!(migrated, payload);
    }

    #[test]
    fn v2_bytes_round_trip_through_chain() {
        let payload = EventPayload::SandboxElevationTimedOut {
            node: NodeKey::try_from("impl_1").unwrap(),
            capability: "network:api.example.com".into(),
            elapsed_seconds: 86_400,
        };
        let bytes = make_v2_bytes(payload.clone());
        let migrated = migrate_payload(2, &bytes).expect("v2 migrates");
        assert_eq!(migrated, payload);
    }

    #[test]
    fn writer_emits_max_supported_version() {
        let wrapper = VersionedEventPayload::new(EventPayload::SandboxElevationTimedOut {
            node: NodeKey::try_from("impl_1").unwrap(),
            capability: "shell:bash".into(),
            elapsed_seconds: 30,
        });
        assert_eq!(wrapper.schema_version, MAX_SUPPORTED_VERSION);
        assert_eq!(wrapper.schema_version, 2);
    }

    #[test]
    fn schema_version_too_new_is_rejected() {
        let err = migrate_payload(99, b"{}").unwrap_err();
        assert!(matches!(
            err,
            SurgeError::SchemaTooNew { found: 99, max: 2 }
        ));
    }

    #[test]
    fn schema_version_too_old_is_rejected() {
        // MIN_SUPPORTED_VERSION is 1, so version 0 is too old.
        let err = migrate_payload(0, b"{}").unwrap_err();
        assert!(matches!(err, SurgeError::SchemaTooOld { found: 0, min: 1 }));
    }

    #[test]
    fn v1_bytes_handle_runtime_version_warning_as_unknown_variant() {
        // RuntimeVersionWarning was introduced in v2. Old v1 bytes will never
        // serialize it, so we only need to verify the v2 path round-trips.
        let payload = EventPayload::RuntimeVersionWarning {
            runtime: crate::runtime::RuntimeKind::ClaudeCode,
            found_version: "1.9.0".into(),
            min_version: ">=2.0.0".into(),
        };
        let bytes = make_v2_bytes(payload.clone());
        let migrated = migrate_payload(2, &bytes).expect("v2 migrates");
        assert_eq!(migrated, payload);
    }
}
