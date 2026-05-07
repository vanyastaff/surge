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
pub const MAX_SUPPORTED_VERSION: u32 = 1;

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
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes).map_err(|e| {
            SurgeError::Spec(format!("v1 payload decode failed: {e}"))
        })?;
        Ok(wrapper.payload)
    }
}

/// Ordered registry of [`Migration`]s indexed by their declared version.
pub struct MigrationChain {
    migrations: Vec<Box<dyn Migration>>,
}

impl MigrationChain {
    /// Build the default chain. Contains [`IdentityV1`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            migrations: vec![Box::new(IdentityV1)],
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
