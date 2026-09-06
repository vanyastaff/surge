# Schema Versioning

Surge persists and exchanges four versioned formats. v0.1 **freezes the
first three at version 1** and defines how future bumps are handled; the
memory database versions independently (see below).

| Format | Where | Version constant | v0.1 |
|--------|-------|------------------|------|
| `surge.toml` config | project root | `surge_core::config::CONFIG_SCHEMA_VERSION` | **1** |
| `flow.toml` graph | run definition | `surge_core::graph::SCHEMA_VERSION` | **1** |
| Event payloads | per-run SQLite log | `VersionedEventPayload.schema_version` + `surge_core::migrations` | **6** (see below) |
| Memory DB | `~/.surge/memory.db` | `surge_persistence::memory::schema::SCHEMA_VERSION` | **2** (see below) |

## `surge.toml` (config)

`SurgeConfig` carries an optional `schema_version` field:

```toml
schema_version = 1   # optional; defaults to 1 when omitted
```

- **Absent → 1.** Configs written before the field existed parse unchanged
  and are treated as schema 1.
- **`validate()` rejects any value other than `CONFIG_SCHEMA_VERSION`** with
  an actionable message, so a config authored by a *newer* surge fails fast
  on an older binary instead of being silently misread.

## `flow.toml` (graph)

Every `Graph` serializes a `schema_version` (`SCHEMA_VERSION = 1`). Graph
loading validates structural invariants (reachability, terminal
reachability, edge kinds, profile/template references) — see
[`docs/conventions/flow.md`](conventions/flow.md). A graph with an
unsupported `schema_version` is rejected at load.

## Event payloads (run log)

The per-run event log is the durable source of truth (it drives crash
recovery — see [`docs/crash-recovery.md`](crash-recovery.md)). Each event is
a `VersionedEventPayload { schema_version, payload }`. On read, payloads
older than the current version are run **through the migration chain in
`surge_core::migrations` before the fold**, so an old run remains
replayable after a surge upgrade. This is the one format that must *never*
hard-break across versions — historical runs are immutable.

**Every one of the v2..v6 bumps so far has been a *new enum variant*, not a
field change on an existing one — and that distinction is the actual
reason each bump was required.** A field a reader doesn't recognize can
default (`#[serde(default)]`) and the payload still decodes; a variant the
reader's `EventPayload` enum has never heard of has **no representation to
decode into at all** — a v5-max binary reading a v6 `SkillBound` event
would hit an unknown-tag deserialize error, not a missing-field default.
Bumping the version so that reader instead returns a clean, typed
`SurgeError::SchemaTooNew` — rather than an opaque decode failure — is the
whole reason the migration chain exists (`surge_core::migrations`, each
`IdentityVN` documents this per-version). Describing such a bump as
"purely additive" is only true in the narrow sense that *every prior
version's own payloads* keep decoding unchanged (they never contained the
new variant); it is not additive from an *old reader's* point of view, and
a CHANGELOG entry should say the latter, not just the former — see the
Memory DB v1→v2 entry below for the same caution applied to a different
kind of change (a backfill, not a new variant).

## Memory DB (`surge-persistence`)

A separate, locally-scoped SQLite database (`~/.surge/memory.db`) versioned
independently of the three formats above via its own `schema_version` table
(`surge_persistence::memory::schema::SCHEMA_VERSION`). Unlike the run event
log it is not exchanged between processes or replayed across a run's
lifetime, so it does not share `surge_core::migrations`; `MemoryStore`
carries its own step-by-step migration chain (`MemoryStore::migrate_one_step`,
called once per version behind on open).

- **v1:** `discoveries` / `patterns` / `gotchas` / `file_contexts` — free-text
  entries with tags and timestamps, no provenance.
- **v2:** adds `memory_claims` — memory as claims with per-entry provenance
  (source path, content hash, verifying command, timestamp),
  `surge_core::memory::Confidence` (three-level: `verified` /
  `name_matched` / `asserted` — not a bool), and
  `surge_core::memory::ClaimStatus` (`verified` / `unverified`; anything
  ingested from a transcript or conversation is recorded `unverified` at
  capture time). The v1 tables are additive, not replaced — their rows stay
  in place so existing callers keep working.

  **The bump's basis is not the new table.** Adding `memory_claims` next to
  the v1 tables is, by itself, exactly the kind of additive change the
  "Principles" section below says needs no bump. What actually forces one
  is a **one-time backfill**: every existing discovery/pattern/gotcha/
  file-context row must be materialized into a claim on first open of a v1
  database (an unverified, `Asserted`-confidence claim — nothing is
  deleted, no text is lost, and newly migrated claims cannot inherit a
  trust level they never earned). That data transformation, not a schema
  addition, is the reason `SCHEMA_VERSION` moves and `MemoryStore` carries
  a migration step at all — a real one-time change to what's on disk,
  exactly the case the "Principles" section's additive-fields exception
  does not cover.

## Migration plan for future bumps

When a breaking change to any format is unavoidable:

1. **Bump the constant** (`CONFIG_SCHEMA_VERSION` / `SCHEMA_VERSION` / the
   payload version) by one.
2. **Event payloads:** add a migration step to `surge_core::migrations`
   that upgrades `N → N+1`, and a round-trip/golden test. The fold path
   reads any historical version through the chain. Never delete a migration
   step — they compose.
3. **`surge.toml` / `flow.toml`:** ship a deterministic upgrader. For
   configs, prefer additive/optional fields with serde defaults (no bump
   needed). A real bump should come with `surge migrate-config` /
   `surge migrate-spec`-style tooling and a documented manual path for the
   ambiguous cases.
4. **Document** the change here and in the release notes; keep the previous
   version's reader for at least one minor release (deprecation window).
5. **CI** asserts the version constants (`SCHEMA_VERSION == 1` today) so an
   accidental bump cannot land without updating this document and the
   migration tests.

### Principles

- Additive, optional, serde-defaulted fields do **not** require a bump.
- Removing or repurposing a field, or changing a field's meaning, **does**.
- Event-payload readers are append-only: every historical version stays
  readable forever via the migration chain.
- The additive-fields exception is about a *reader* tolerating a field it
  doesn't recognize yet — it does not cover a **one-time backfill** of
  already-stored rows into a new representation (the Memory DB v1→v2 bump
  above is exactly this: existing rows are rewritten once, on open, into
  `memory_claims`). A backfill needs a bump and a migration step even when
  the new table is purely additive, because the transformation itself —
  not the shape it adds — is the change that must be documented and
  applied exactly once.
