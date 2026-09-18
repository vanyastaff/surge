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
///
/// **v3 (introduced 2026-05):** adds the `#[serde(default)] mcp_server`
/// attribution field to [`EventPayload::ToolCalled`] /
/// [`EventPayload::ToolResultReceived`] (MCP replay parity). The wire shape is
/// unchanged (same JSON wrapper); old v1/v2 payloads decode cleanly with
/// `mcp_server: None`. Per the v2 precedent the version is still bumped so
/// readers know per-server attribution is in scope.
///
/// **v4 (introduced 2026-05):** adds the [`EventPayload::BudgetWarningRaised`]
/// and [`EventPayload::BudgetExceeded`] variants (live budget enforcement).
/// Purely additive — old v1/v2/v3 payloads decode cleanly (they never contain
/// the new variants). The version is bumped so a v3-max reader rejects a
/// budget event with a clean [`SurgeError::SchemaTooNew`] rather than an
/// unknown-variant decode error.
///
/// **v5 (introduced 2026-07):** adds the task-ledger variants
/// [`EventPayload::TaskStatusChanged`], [`EventPayload::TaskDiscovered`], and
/// [`EventPayload::TaskVerified`] (Phase 1 task ledger). Purely additive — old
/// v1..v4 payloads decode cleanly (they never contain the new variants). The
/// version is bumped so a v4-max reader rejects a ledger event with a clean
/// [`SurgeError::SchemaTooNew`] rather than an unknown-variant decode error.
///
/// **v6 (introduced 2026-09):** adds [`EventPayload::SkillBound`] (skill
/// binding + trust gate), carrying `node`, `name`, `provider`, `hash`, and
/// `gate_enabled`. A v5-max reader has no representation for this variant at
/// all — decoding one is not a matter of a missing optional field defaulting
/// cleanly, it is an unknown enum tag the v5 decoder cannot construct — so
/// the version is bumped precisely so that reader fails closed with
/// [`SurgeError::SchemaTooNew`] instead of an opaque unknown-variant decode
/// error (the same reasoning as the v2/v4/v5 bumps above: a new variant is
/// never "purely additive" from an old reader's point of view, even though
/// every *existing* v1..v5 payload keeps decoding unchanged because none of
/// them ever contained it). `SkillBound`'s own field set was still being
/// shaped under this same v6 the week it was introduced — no v6 payload has
/// ever left this development branch — so its five-field shape here is the
/// v6 shape, not a v7 change; see `docs/adr/0015-skill-binding-trust-via-content-hash.md`.
///
/// **v7 (introduced 2026-09):** adds [`EventPayload::RunParked`] and
/// [`EventPayload::RunWokeFromPark`] (Task 12 M1 — provider rate-limit
/// parking, R37/R37.1). Two new self-contained variants, unconditionally
/// bumped for the identical reason as v6: `docs/schema-versioning.md`'s
/// field-composition exception (which let a v6 *field* change on
/// `SkillBound` ride under the version already bumped for its variant) does
/// not extend to a *new variant* — that exception was never about variants,
/// and "no tagged release has shipped v6 yet" is a risk-radius observation,
/// not a version-policy exemption; a v6-max reader still cannot construct
/// either new variant at all (not merely default a missing field), so it
/// must fail closed with [`SurgeError::SchemaTooNew`] rather than an opaque
/// unknown-variant decode error, exactly like v2/v4/v5/v6 before it.
///
/// **v8 (introduced 2026-09):** adds [`crate::run_event::EscalationCause::CapacityBlindParkLimitExceeded`]
/// (Task 12 M4, blind-park-limit escalation). Bumped for the reason
/// [`crate::run_event::EscalationCause`]'s own doc names: that enum has
/// `#[serde(rename_all = "snake_case")]` with no `#[serde(other)]`
/// fallback, so a v7-max reader does not merely miss an optional field on a
/// new tag — it cannot decode the tag at all, the same "unknown enum
/// variant" failure mode a new `EventPayload` variant has (v2/v4/v5/v6/v7).
/// Every `EscalationRequested` payload written before this version keeps
/// decoding unchanged (`cause` defaults to `Unspecified` when absent;
/// existing named causes are untouched) — only a payload that actually
/// carries the new tag needs the bump, exactly like v6/v7's own "old
/// payloads decode cleanly, they simply never contain the new thing."
///
/// **v9 (introduced 2026-09):** adds [`EventPayload::ComposedArtifactInstalled`]
/// (autonomous task orchestration — a composed flow or profile installed
/// into the project's `.surge/` layer after the gate accepted it), carrying
/// `kind`, `path`, `hash`, and `gate_node`. A new top-level variant,
/// bumped for the same reason as v2/v4/v5/v6/v7: a v8-max reader has no
/// representation to decode it into and must fail closed with
/// [`SurgeError::SchemaTooNew`]. Two things are new *about* this bump:
/// the variant is the first to embed
/// [`crate::artifact_contract::ArtifactKind`] in a persisted payload, so
/// from v9 on growing that enum is a v8-style nested-enum bump (its doc
/// says so); and `run_event.rs` now pins the variant-tag set to this
/// constant in a snapshot test, so the next variant added without touching
/// this line fails a test instead of a review.
pub const MAX_SUPPORTED_VERSION: u32 = 9;

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

/// Identity migration for v3 — the schema bump that introduced the
/// `mcp_server` attribution field on `ToolCalled` / `ToolResultReceived`.
/// The wire shape is unchanged (same JSON-encoded
/// [`VersionedEventPayload`] wrapper); `#[serde(default)]` lets older
/// payloads decode with `mcp_server: None`. The `schema_version` field
/// is the only signal that distinguishes v1/v2 from v3 payloads.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV3;

impl Migration for IdentityV3 {
    fn version(&self) -> u32 {
        3
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v3 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity migration for v4 — the schema bump that introduced the
/// `BudgetWarningRaised` / `BudgetExceeded` variants (live budget
/// enforcement). The wire shape is unchanged (same JSON-encoded
/// [`VersionedEventPayload`] wrapper); old payloads decode cleanly because
/// they never carry the new variants. The `schema_version` field is the only
/// signal that distinguishes v1/v2/v3 from v4 payloads.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV4;

impl Migration for IdentityV4 {
    fn version(&self) -> u32 {
        4
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v4 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity migration for v5 — the schema bump that introduced the task-ledger
/// variants (`TaskStatusChanged` / `TaskDiscovered` / `TaskVerified`). The wire
/// shape is unchanged (same JSON-encoded [`VersionedEventPayload`] wrapper);
/// old payloads decode cleanly because they never carry the new variants. The
/// `schema_version` field is the only signal that distinguishes v1..v4 from v5
/// payloads.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV5;

impl Migration for IdentityV5 {
    fn version(&self) -> u32 {
        5
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v5 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity migration for v6 — the schema bump that introduced the
/// `SkillBound` variant (skill binding + trust gate). The wire shape is
/// unchanged (same JSON-encoded [`VersionedEventPayload`] wrapper); old
/// payloads decode cleanly because they never carry the new variant. The
/// `schema_version` field is the only signal that distinguishes v1..v5 from
/// v6 payloads.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV6;

impl Migration for IdentityV6 {
    fn version(&self) -> u32 {
        6
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v6 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity migration for v7 — the schema bump that introduced the
/// `RunParked` / `RunWokeFromPark` variants (Task 12 M1, provider
/// rate-limit parking). The wire shape is unchanged (same JSON-encoded
/// [`VersionedEventPayload`] wrapper); old payloads decode cleanly because
/// they never carry either new variant. The `schema_version` field is the
/// only signal that distinguishes v1..v6 from v7 payloads.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV7;

impl Migration for IdentityV7 {
    fn version(&self) -> u32 {
        7
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v7 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity migration for v8 — the schema bump that introduced
/// `EscalationCause::CapacityBlindParkLimitExceeded` (Task 12 M4). The wire
/// shape is unchanged (same JSON-encoded [`VersionedEventPayload`]
/// wrapper); old `EscalationRequested` payloads decode cleanly because they
/// never carry this cause tag.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV8;

impl Migration for IdentityV8 {
    fn version(&self) -> u32 {
        8
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v8 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity migration for v9 — the schema bump that introduced the
/// `ComposedArtifactInstalled` variant (composed flow/profile installed
/// behind the gate). The wire shape is unchanged (same JSON-encoded
/// [`VersionedEventPayload`] wrapper); old payloads decode cleanly because
/// they never carry the new variant. The `schema_version` field is the
/// only signal that distinguishes v1..v8 from v9 payloads.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV9;

impl Migration for IdentityV9 {
    fn version(&self) -> u32 {
        9
    }

    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|e| SurgeError::Spec(format!("v9 payload decode failed: {e}")))?;
        Ok(wrapper.payload)
    }
}

/// Ordered registry of [`Migration`]s indexed by their declared version.
pub struct MigrationChain {
    migrations: Vec<Box<dyn Migration>>,
}

impl MigrationChain {
    /// Build the default chain. Contains [`IdentityV1`], [`IdentityV2`],
    /// [`IdentityV3`], [`IdentityV4`], [`IdentityV5`], [`IdentityV6`],
    /// [`IdentityV7`], [`IdentityV8`], and [`IdentityV9`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            migrations: vec![
                Box::new(IdentityV1),
                Box::new(IdentityV2),
                Box::new(IdentityV3),
                Box::new(IdentityV4),
                Box::new(IdentityV5),
                Box::new(IdentityV6),
                Box::new(IdentityV7),
                Box::new(IdentityV8),
                Box::new(IdentityV9),
            ],
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
        assert_eq!(wrapper.schema_version, 9);
    }

    #[test]
    fn schema_version_too_new_is_rejected() {
        let err = migrate_payload(99, b"{}").unwrap_err();
        assert!(matches!(
            err,
            SurgeError::SchemaTooNew { found: 99, max: 9 }
        ));
    }

    #[test]
    fn v6_skill_bound_event_round_trips() {
        use crate::content_hash::ContentHash;
        use crate::skill::SkillProvider;

        let payload = EventPayload::SkillBound {
            node: NodeKey::try_from("implement").unwrap(),
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            hash: ContentHash::compute(b"skill-pack-content"),
            gate_enabled: true,
        };
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
        let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn v7_run_parked_event_round_trips() {
        use crate::capacity::WakeBasis;

        let payload = EventPayload::RunParked {
            wake_at: chrono::Utc::now(),
            runtime: Some("claude-acp".into()),
            worktree: std::path::PathBuf::from("/tmp/run-worktree"),
            basis: WakeBasis::PolicyBackoff,
            reason: "no observed reset time; blind backoff".into(),
        };
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
        let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn v7_run_woke_from_park_event_round_trips() {
        let payload = EventPayload::RunWokeFromPark {};
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
        let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn v8_capacity_blind_park_limit_escalation_round_trips() {
        use crate::run_event::EscalationCause;

        let payload = EventPayload::EscalationRequested {
            stage: None,
            reason: "runtime exhausted 5 consecutive times with no successful dispatch".into(),
            cause: EscalationCause::CapacityBlindParkLimitExceeded,
        };
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
        let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn v9_composed_artifact_installed_round_trips() {
        use crate::artifact_contract::{ArtifactKind, RelPath};
        use crate::content_hash::ContentHash;

        let payload = EventPayload::ComposedArtifactInstalled {
            kind: ArtifactKind::Flow,
            path: RelPath::new(".surge/flows/bug-fix-1.0.toml").unwrap(),
            hash: ContentHash::compute(b"schema_version = 1\n"),
            gate_node: NodeKey::try_from("flow_generator").unwrap(),
        };
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
        let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
        assert_eq!(decoded, payload);

        // Wire shape other crates (and the trust store) will see.
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["schema_version"], 9);
        assert_eq!(json["payload"]["type"], "composed_artifact_installed");
        assert_eq!(json["payload"]["kind"], "flow");
        assert_eq!(json["payload"]["path"], ".surge/flows/bug-fix-1.0.toml");
        assert_eq!(json["payload"]["gate_node"], "flow_generator");
        assert!(
            json["payload"]["hash"]
                .as_str()
                .is_some_and(|h| h.starts_with("sha256:"))
        );
    }

    #[test]
    fn v9_rejects_a_path_that_escapes_the_project_root() {
        // The invariant lives in the type, so it also holds on the read
        // side: a hand-edited or hostile log line cannot smuggle an
        // absolute or `..` path into a trust-store key.
        for bad in ["/etc/passwd", "../outside.toml", ""] {
            let json = serde_json::json!({
                "schema_version": 9,
                "payload": {
                    "type": "composed_artifact_installed",
                    "kind": "profile",
                    "path": bad,
                    "hash": sample_hash_json(),
                    "gate_node": "gen",
                }
            });
            let bytes = serde_json::to_vec(&json).unwrap();
            let err = migrate_payload(9, &bytes).unwrap_err();
            assert!(
                matches!(err, SurgeError::Spec(ref m) if m.contains("v9 payload decode failed")),
                "{bad:?} must be refused, got {err:?}"
            );
        }
    }

    /// A well-formed `sha256:…` string for hand-built JSON payloads.
    fn sample_hash_json() -> String {
        crate::content_hash::ContentHash::compute(b"x").to_string()
    }

    #[test]
    fn v8_bytes_of_pre_v9_variants_still_migrate_unchanged() {
        // A log written by a v8 daemon contains no `ComposedArtifactInstalled`
        // line; every line it does contain must come through the v8 entry of
        // the chain byte-for-byte equal to what a v9 writer would produce for
        // the same payload — the bump added a variant, it did not reshape any.
        use crate::run_event::EscalationCause;

        let payload = EventPayload::EscalationRequested {
            stage: Some(crate::run_event::BootstrapStage::Flow),
            reason: "written under v8".into(),
            cause: EscalationCause::CapacityBlindParkLimitExceeded,
        };
        let v8_wrapper = serde_json::json!({ "schema_version": 8, "payload": payload });
        let bytes = serde_json::to_vec(&v8_wrapper).unwrap();
        assert_eq!(migrate_payload(8, &bytes).unwrap(), payload);
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 256,
            ..Default::default()
        })]

        /// Round-trip law for the v9 variant over its whole value space:
        /// any `kind`, any *valid* relative path (the `RelPath` strategy is
        /// the type's own acceptance law — see `artifact_contract::path`),
        /// any 32-byte hash, any well-formed node key. What this exercises
        /// beyond the derive is the three hand-written serde impls the
        /// variant composes (`RelPath`, `ContentHash`, `NodeKey`) agreeing
        /// with their `PartialEq` after a trip through the migration chain.
        #[test]
        fn v9_composed_artifact_installed_round_trips_for_any_fields(
            is_profile in proptest::bool::ANY,
            // Segments that are always normal components: a `.`-only
            // segment (`.`/`..`) is a `RelPath` *rejection* case, covered
            // by that type's own law tests, not a round-trip input.
            parts in proptest::collection::vec("[a-zA-Z0-9_-][a-zA-Z0-9_.-]{0,11}", 1..6),
            hash_bytes in proptest::array::uniform32(proptest::num::u8::ANY),
            gate in "[A-Za-z][A-Za-z0-9_]{0,31}",
        ) {
            use crate::artifact_contract::{ArtifactKind, RelPath};
            use crate::content_hash::ContentHash;

            let kind = if is_profile { ArtifactKind::Profile } else { ArtifactKind::Flow };
            let path = RelPath::new(parts.join("/")).unwrap();
            let payload = EventPayload::ComposedArtifactInstalled {
                kind,
                path,
                hash: ContentHash::from_bytes(hash_bytes),
                gate_node: NodeKey::try_from(gate.as_str()).unwrap(),
            };
            let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
            let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
            proptest::prop_assert_eq!(decoded, payload);
        }
    }

    #[test]
    fn v5_task_ledger_event_round_trips() {
        use crate::content_hash::ContentHash;

        let payload = EventPayload::TaskVerified {
            task_id: "m1-t1".into(),
            node: NodeKey::try_from("verify_1").unwrap(),
            evidence: ContentHash::compute(b"verification-report"),
        };
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
        let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn v4_budget_event_round_trips() {
        let payload = EventPayload::BudgetExceeded {
            dimension: crate::budget::BudgetDimension::Tokens,
            cost_usd: 0.0,
            total_tokens: 1_500_000,
        };
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
        let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn legacy_tool_events_decode_as_none_through_every_version() {
        use crate::content_hash::ContentHash;
        use crate::id::SessionId;

        // Build a current ToolCalled, then strip the new `mcp_server`
        // key to simulate a payload written before the attribution
        // field existed. It must decode cleanly (mcp_server: None)
        // under v1, v2, and v3 — replay parity across the bump.
        let wrapper = VersionedEventPayload::new(EventPayload::ToolCalled {
            session: SessionId::nil(),
            tool: "read_file".into(),
            args_redacted: ContentHash::compute(b"args"),
            mcp_server: Some("dropped".into()),
        });
        let mut json = serde_json::to_value(&wrapper).unwrap();
        json["payload"]
            .as_object_mut()
            .unwrap()
            .remove("mcp_server");
        let legacy = serde_json::to_vec(&json).unwrap();

        for v in [1u32, 2, 3] {
            let decoded = migrate_payload(v, &legacy)
                .unwrap_or_else(|e| panic!("legacy ToolCalled must decode under v{v}: {e}"));
            match decoded {
                EventPayload::ToolCalled { mcp_server, .. } => {
                    assert_eq!(mcp_server, None, "missing field defaults to None (v{v})");
                },
                other => panic!("expected ToolCalled, got {other:?}"),
            }
        }
    }

    #[test]
    fn v3_attributed_tool_event_round_trips() {
        use crate::content_hash::ContentHash;
        use crate::id::SessionId;

        let payload = EventPayload::ToolResultReceived {
            session: SessionId::nil(),
            success: true,
            result: ContentHash::compute(b"ok"),
            mcp_server: Some("filesystem".into()),
        };
        let bytes = serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap();
        let decoded = migrate_payload(MAX_SUPPORTED_VERSION, &bytes).unwrap();
        assert_eq!(decoded, payload);
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
