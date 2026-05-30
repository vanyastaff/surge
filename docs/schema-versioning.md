# Schema Versioning

Surge persists and exchanges three versioned formats. v0.1 **freezes all
three at version 1** and defines how future bumps are handled.

| Format | Where | Version constant | v0.1 |
|--------|-------|------------------|------|
| `surge.toml` config | project root | `surge_core::config::CONFIG_SCHEMA_VERSION` | **1** |
| `flow.toml` graph | run definition | `surge_core::graph::SCHEMA_VERSION` | **1** |
| Event payloads | per-run SQLite log | `VersionedEventPayload.schema_version` + `surge_core::migrations` | **1** |

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
