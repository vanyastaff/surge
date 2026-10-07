# Schema Versioning

Surge persists and exchanges four versioned formats. The current release keeps config and graph formats at version 1; event payloads and
the memory database version independently (see below).

| Format | Where | Version constant | v0.1 |
|--------|-------|------------------|------|
| `surge.toml` config | project root | `surge_core::config::CONFIG_SCHEMA_VERSION` | **1** |
| `flow.toml` graph | run definition | `surge_core::graph::SCHEMA_VERSION` | **1** |
| Event payloads | per-run SQLite log | `VersionedEventPayload.schema_version` + `surge_core::migrations` | **21** (see below) |
| Memory DB | `~/.surge/memory.db` | `surge_persistence::memory::schema::SCHEMA_VERSION` | **3** (see below) |

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

Version 10 adds host-bound verification and source/criteria observations. Version 11
adds `WorkItemAttemptBound`, the immutable persistent-task association committed in
one startup batch with `RunStarted`, `PipelineMaterialized`, and accepted requirement
artifact identity. Earlier payloads remain readable through identity migrations;
readers supporting at most version 10 reject version 11 before decoding.


The per-run event log is the durable source of truth (it drives crash
recovery — see [`docs/crash-recovery.md`](crash-recovery.md)). Each event is
a `VersionedEventPayload { schema_version, payload }`. On read, payloads
older than the current version are run **through the migration chain in
`surge_core::migrations` before the fold**, so an old run remains
replayable after a surge upgrade. This is the one format that must *never*
hard-break across versions — historical runs are immutable.

**Every one of the v2..v7 bumps so far has been a *new enum variant*, not a
field change on an existing one — and that distinction is the actual
reason each bump was required.** A field a reader doesn't recognize can
default (`#[serde(default)]`) and the payload still decodes; a variant the
reader's `EventPayload` enum has never heard of has **no representation to
decode into at all** — a v5-max binary reading a v6 `SkillBound` event
would hit an unknown-tag deserialize error, not a missing-field default;
likewise a v6-max binary reading a v7 `RunParked`/`RunWokeFromPark` event
(Task 12 M1, provider rate-limit parking — two new variants under one bump,
not a field change on an existing one either).
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

**v8 is the same failure mode one level deeper: a new variant on a *nested*
enum, not on `EventPayload` itself.** `EventPayload::EscalationRequested`
already existed (unchanged) at v7; what's new is
`EscalationCause::CapacityBlindParkLimitExceeded` (Task 12 M4, blind-park
escalation), a variant of the `cause` field's own enum type. That field is
`#[serde(default)]` (a v6-max reader tolerates the *key being absent*), but
`#[serde(rename_all = "snake_case")]` on `EscalationCause` has no
`#[serde(other)]` catch-all — so once the key is *present* with an
unrecognized tag, decoding fails exactly like an unknown `EventPayload`
variant tag does, not like a missing optional field. Same underlying rule
("a value with no representation to decode into forces a version bump, a
merely-absent one does not"), applied to an enum living inside a variant
rather than to a top-level variant of `EventPayload` — the "Principles"
section below is about `EventPayload`'s own variants, but the same logic
governs any nested closed enum reachable from a persisted payload.

**The additive-field exception itself is not a general rule — it is tied to
one property of the durable encoding, and stops holding the moment that
property does.** An additive `#[serde(default)]` field on an *existing*
variant of `EventPayload` does not require a bump because the durable write
goes through `serde_json::to_vec` (`runs/writer.rs:161,185`) — JSON is
self-describing, and an old reader's `serde_json::from_slice` silently
ignores a key it doesn't recognize. A **new variant** requires a bump
regardless (see above): that is a different failure mode (no representation
to decode into at all, not an ignorable key) and the field-addition
exception was never about it. `EventPayload::to_bincode`/`from_bincode`
(`surge_core::run_event`) exist only for in-memory and test round-trips
today, despite the name — they are not the durable encoding. If the durable
write path ever moves onto true bincode (or any non-self-describing
format), this exception stops applying **in that same commit**: bincode's
enum/struct encoding is positional, so an unrecognized field is not
ignorable, it desyncs the decode. Rewrite this paragraph then, don't carry
it forward on the strength of the *last* format's guarantee.

Versions 12–15 add recoverable execution fences, human-decision effect and route
commitments, immutable owned-flow startup snapshots, and informational owned-flow
wake refusals respectively. Version 16 adds the nested
`EscalationCause::McpSelectedCatalogUnavailable` (an MCP server a stage selected
could not produce its tool catalog; see ADR-0014 decision 8). This is the same
nested-enum rule as v8: v15 readers reject v16 envelopes with `SchemaTooNew`, and
every earlier payload still decodes through the existing identity migrations. The
current maximum supported payload version is 21.

Version 17 adds `EventPayload::ExecutionWriterGroupStopped`, the ADR-0021
best-effort cleanup record for a host-launched MCP writer whose process group
was observed empty. It is a new top-level variant, so v16 readers reject v17
envelopes with `SchemaTooNew`; that is deliberate, because a reader that
ignored it could not tell best-effort cleanup from confirmed closure. Earlier
payloads decode through the identity migrations unchanged.

Version 18 adds `EventPayload::TaskSplit`, the verifier ladder's split rung: a
loop's current task replaced by smaller tasks inserted right after it. It is a
new top-level variant, so v17 readers reject v18 envelopes with `SchemaTooNew`.
A stage route batch may now begin with one `TaskSplit`, committed atomically
with the route and its snapshot.

Version 19 adds `EventPayload::TaskAcceptedByHuman` and
`EventPayload::RequirementRevised`, a human's answers on a default escalation
gate. A v18 reader would show a human-accepted task as merely completed, so v18
readers reject v19 envelopes with `SchemaTooNew`. Either event may lead a stage
route batch (at most two task records per batch). The per-run `task_ledger`
view and the registry `task_ledger_index` gain `accepted_by_human` and
`requirement_revised` columns (per-run migration 0009, registry 0031).

Version 20 adds `EscalationCause::LoopGuardNoProgress` and
`EscalationCause::LoopGuardToolCallCap` (loop protection). Same nested-enum rule
as v8: v19 readers reject v20 envelopes with `SchemaTooNew`.

Version 21 adds `EventPayload::StageRuntimeRotated`: a stage moved to a
fallback agent because its own agent's usage limit was exhausted
(`[capacity].fallback_agents`). A v20 reader would replay the stage on the
exhausted agent, so it rejects v21 envelopes with `SchemaTooNew`.
Older binaries may reject these envelopes before decoding; forward readability does
not provide downgrade support. SQLite storage migrations are separate from payload
versions. See [Release and rollback procedure](release-procedure.md).

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

- **v3:** repairs external-content FTS update/delete triggers and rebuilds indexes.

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
5. **CI** asserts the version constants (config/graph 1, event payload 16, memory DB 3 today) so an
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

### Version 9: stage-tool receipts

`StageToolReceipt` records an authenticated tool result or accepted outcome candidate.
It is nonterminal: outcome candidates still require successful ACP prompt completion
and the existing validators before `OutcomeReported` or task verification. Version 8
histories remain readable through the unchanged identity migration. Version 8 readers
reject version 9 envelopes with `SchemaTooNew` before decoding an unknown variant.
No SQLite schema migration or rewriting of old events is required.
