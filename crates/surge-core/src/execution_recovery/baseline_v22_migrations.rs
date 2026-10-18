//! Schema-version migration registry for persisted `EventPayload` bytes.
//!
//! Persistence reads invoke [`migrate_payload`] to turn the on-disk bytes back
//! into a current [`EventPayload`]. The registry preserves earlier identity
//! migrations; future schema-breaking changes add
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
/// **v9:** adds the nonterminal [`EventPayload::StageToolReceipt`] variant.
/// Accepted candidates still require prompt success and final validation.
/// Older readers reject v9 with [`SurgeError::SchemaTooNew`].
/// **v10:** revision/criteria observations and host-bound verification reports.
/// Old unbound TaskVerified events decode but cannot establish fresh proof.
/// **v11:** immutable persistent-task attempt association.
/// **v12:** recoverable suspension and provider establishment fences.
/// **v13:** host-owned human-decision effect and route commitments.
/// **v14:** immutable public owned-flow startup input snapshots.
/// **v15:** informational permanent owned-flow quota-wake refusal.
/// **v16:** adds [`crate::run_event::EscalationCause::McpSelectedCatalogUnavailable`]
/// (a selected MCP server's catalog failed at stage open). Same nested-enum
/// reasoning as v8: a v15-max reader cannot decode the new cause tag, so it
/// must reject v16 with [`SurgeError::SchemaTooNew`].
/// **v17:** adds [`EventPayload::ExecutionWriterGroupStopped`], ADR-0021
/// best-effort MCP group cleanup. A v16-max reader has no variant to decode it
/// into, so it rejects v17 with [`SurgeError::SchemaTooNew`] instead of
/// misreading best-effort cleanup as confirmed closure.
/// **v18:** adds [`EventPayload::TaskSplit`], the verifier ladder's split rung
/// (a task replaced by smaller tasks in the same loop). A v17-max reader has
/// no variant for it and rejects v18 with [`SurgeError::SchemaTooNew`].
/// **v19:** adds [`EventPayload::TaskAcceptedByHuman`] and
/// [`EventPayload::RequirementRevised`], the human overrides of an exhausted
/// retry ladder. A v18-max reader would otherwise miss that a completed task
/// was accepted by a human rather than verified, so it rejects v19.
/// **v20:** adds [`crate::run_event::EscalationCause::LoopGuardNoProgress`] and
/// [`crate::run_event::EscalationCause::LoopGuardToolCallCap`] (loop
/// protection). Same nested-enum rule as v8: v19 readers reject v20.
/// **v21:** adds [`EventPayload::StageRuntimeRotated`] (a stage moved to a
/// fallback agent on an exhausted usage limit). A v20 reader would replay the
/// stage on the exhausted agent, so it rejects v21.
/// **v22:** adds [`EventPayload::PullRequestOpened`] (the PR carrying a run's
/// work, #127). A v21 reader would not see which PR the merge gate must
/// resolve, so it rejects v22.
pub const MAX_SUPPORTED_VERSION: u32 = 22;

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

/// Identity migration for v9, adding nonterminal authenticated stage-tool receipts.
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

/// Identity decoder for bound verification, retaining legacy optional defaults.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV10;
impl Migration for IdentityV10 {
    fn version(&self) -> u32 {
        10
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v10 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for persistent-task attempt bindings.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV11;
impl Migration for IdentityV11 {
    fn version(&self) -> u32 {
        11
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v11 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for recoverable execution fences.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV12;
impl Migration for IdentityV12 {
    fn version(&self) -> u32 {
        12
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v12 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for host-owned human decision effect and route commitments.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV13;
impl Migration for IdentityV13 {
    fn version(&self) -> u32 {
        13
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v13 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for public immutable owned-flow startup snapshots.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV14;
impl Migration for IdentityV14 {
    fn version(&self) -> u32 {
        14
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v14 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for informational owned-flow wake refusals.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV15;
impl Migration for IdentityV15 {
    fn version(&self) -> u32 {
        15
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v15 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for the selected-MCP-catalog escalation cause.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV16;
impl Migration for IdentityV16 {
    fn version(&self) -> u32 {
        16
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v16 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for best-effort MCP group cleanup records.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV17;
impl Migration for IdentityV17 {
    fn version(&self) -> u32 {
        17
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v17 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for task split records.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV18;
impl Migration for IdentityV18 {
    fn version(&self) -> u32 {
        18
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v18 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for human override records.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV19;
impl Migration for IdentityV19 {
    fn version(&self) -> u32 {
        19
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v19 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for loop-protection escalation causes.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV20;
impl Migration for IdentityV20 {
    fn version(&self) -> u32 {
        20
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v20 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for agent rotation records.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV21;
impl Migration for IdentityV21 {
    fn version(&self) -> u32 {
        21
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v21 payload decode failed: {error}")))?;
        Ok(wrapper.payload)
    }
}

/// Identity decoder for pull-request records.
#[derive(Debug, Default, Copy, Clone)]
pub struct IdentityV22;
impl Migration for IdentityV22 {
    fn version(&self) -> u32 {
        22
    }
    fn migrate(&self, bytes: &[u8]) -> Result<EventPayload, SurgeError> {
        let wrapper: VersionedEventPayload = serde_json::from_slice(bytes)
            .map_err(|error| SurgeError::Spec(format!("v22 payload decode failed: {error}")))?;
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
    /// [`IdentityV7`], [`IdentityV8`], [`IdentityV9`], [`IdentityV10`], [`IdentityV11`],
    /// [`IdentityV12`], [`IdentityV13`], [`IdentityV14`], [`IdentityV15`],
    /// [`IdentityV16`], [`IdentityV17`], [`IdentityV18`], [`IdentityV19`],
    /// [`IdentityV20`], [`IdentityV21`], and [`IdentityV22`].
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
                Box::new(IdentityV10),
                Box::new(IdentityV11),
                Box::new(IdentityV12),
                Box::new(IdentityV13),
                Box::new(IdentityV14),
                Box::new(IdentityV15),
                Box::new(IdentityV16),
                Box::new(IdentityV17),
                Box::new(IdentityV18),
                Box::new(IdentityV19),
                Box::new(IdentityV20),
                Box::new(IdentityV21),
                Box::new(IdentityV22),
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

