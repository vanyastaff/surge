# Project Base Rules

> Auto-detected conventions from codebase analysis (Cargo.toml, clippy.toml, rustfmt.toml, crates/surge-*/src). Edit as needed.

## Naming Conventions

- **Files / modules:** `snake_case` (e.g., `agent_config.rs`, `human_gate_config.rs`, `run_event.rs`).
- **Crates:** `surge-` prefix in kebab-case (`surge-core`, `surge-acp`, `surge-orchestrator`, …). Internal crate names in `Cargo.toml` use the same prefix.
- **Types (struct / enum / trait):** `PascalCase`. Semantic suffixes are common and should be preserved: `*Error`, `*Config`, `*Kind`, `*Key`, `*Id`, `*State`, `*Event`, `*Source`.
- **Functions, methods, locals, fields:** `snake_case`.
- **Constants and statics:** `SCREAMING_SNAKE_CASE`.
- **Acronyms:** `upper-case-acronyms-aggressive = false`. Treat acronyms as ordinary PascalCase words (`AcpBridge`, `McpClient`, `JsonValue`), except where the term is a recognized identifier (see `doc-valid-idents` in `clippy.toml`: `GitHub`, `JSON`, `YAML`, `TOML`, `UUID`, `ULID`, `WebSocket`, `PostgreSQL`, `gRPC`, `GraphQL`).
- **Identifier minimums:** `min-ident-chars-threshold = 3`. Short names allowed only from the `clippy.toml` allowlist (`id`, `db`, `tx`, `rx`, `fs`, `io`, `fn`, single-letter loop indices, …).
- **Method-name prefixes:** `to_*`, `from_*`, `as_*`, `into_*`, `is_*`, `has_*`, `with_*`. Other prefixes are flagged.

## Workspace & Module Structure

- **One major type per file.** Split out config structs, enums, and traits into their own modules under `crates/<crate>/src/` (the `surge-core` crate is the canonical example: `agent_config.rs`, `branch_config.rs`, `edge.rs`, `node.rs`, …).
- **Workspace-managed deps.** All dependency versions live in the root `Cargo.toml` under `[workspace.dependencies]`. Member crates depend on them with `{ workspace = true }`.
- **Workspace-managed package fields.** `version`, `edition`, `license` come from the workspace via `version.workspace = true`.
- **Dependency direction is downward.** `surge-core` is the leaf (no I/O). Binaries (`surge-cli`, `surge-daemon`, `surge-ui`) depend on the workspace through stable trait surfaces. No cycles.
- **Imports:** `reorder_imports = true`. One `use` statement per line; do not merge granularity (stable rustfmt cannot enforce `imports_granularity`).
- **Module declarations:** `reorder_modules = true`. Public re-exports go through `lib.rs`.

## Error Handling

- **Library crates use `thiserror`.** Define a crate-local error enum (e.g., `SurgeError` in `surge-core`) with `#[error("...")]` per variant and `#[from]` for transparent conversions.
- **The CLI binary (`surge-cli`) uses `anyhow`.** `anyhow::Result` is acceptable only at the binary boundary, never in libraries.
- **No `unwrap()` / `expect()` in library code.** Use `?` propagation or explicit error mapping. Tests and `const` contexts are exempt (allowed via `clippy.toml`: `allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-unwrap-in-consts`, `allow-expect-in-consts`).
- **Functions returning `Result` should carry `#[must_use]`** when the error must not be silently dropped (see `CLAUDE.md`).
- **Public APIs are documented with `///` doc comments.** Document errors in a `# Errors` section.

## Logging & Observability

- **Use `tracing` for structured logging.** Macros: `tracing::info!`, `tracing::warn!`, `tracing::error!`, `tracing::debug!`, `tracing::trace!`. Subscriber setup uses `tracing-subscriber` with `EnvFilter` at the binary entry point.
- **No `println!` / `eprintln!` in library code.** Allowed only in tests (`allow-print-in-tests = true`) and the CLI's user-facing surface.
- **No `dbg!` outside tests.** `allow-dbg-in-tests = true`; production code must not ship `dbg!` calls.

## Async & Concurrency

- **Async runtime:** `tokio` with the `full` feature.
- **The ACP bridge is `!Send`.** It must run on a dedicated OS thread with its own single-threaded Tokio runtime + `LocalSet`. Cross-thread coordination uses `mpsc` / `broadcast` channels and typed `BridgeCommand` / `BridgeEvent` payloads.
- **The daemon is the single writer to the SQLite event log.** CLI / UI / bot are readers. SQLite runs in WAL mode so readers do not block the writer.
- **Use `async-trait` for async traits** until the stable language feature covers all needed cases.

## Testing

- **Unit tests live next to the code** in `#[cfg(test)] mod tests { … }` blocks (per `CLAUDE.md`).
- **Test harnesses available:** `proptest` for property tests, `insta` for snapshot tests (yaml / json / redactions enabled), `wiremock` for HTTP mocks, `tokio-test` for runtime utilities, `tempfile` for filesystem fixtures.
- **Benches use `criterion` with `harness = false`** under `crates/<crate>/benches/`. Existing benches in `surge-core`: `fold_events`, `validate_graphs`, `toml_roundtrip`, `bincode_roundtrip`.
- **Tests may panic / unwrap / index liberally.** Production code may not.

## Complexity & Code Shape (clippy.toml-enforced)

- **Function length:** ≤ 100 lines (`too-many-lines-threshold`). Split larger functions.
- **Cognitive complexity:** ≤ 25 per function.
- **Nesting:** ≤ 5 levels.
- **Function arguments:** ≤ 7. ≤ 3 boolean parameters per function.
- **Struct booleans:** ≤ 3. Prefer enums or a small config struct.
- **Trait bounds on a single item:** ≤ 3.
- **Type complexity:** ≤ 250 (use type aliases for compound generics).

## Memory-Footprint Heuristics (clippy.toml-enforced)

- **Enum variant size:** ≤ 200 bytes (use `Box<…>` for large variants).
- **Pass-by-value cap:** ≤ 256 bytes (above that, take by reference).
- **Trivial-copy cap:** ≤ 8 bytes (above that, do not derive `Copy`).
- **Stack-frame cap:** ≤ 200 bytes per frame.
- **Future size:** ≤ 16 KiB (boxes are cheaper than oversized state machines).
- **`Vec<Box<T>>`:** flag when `T` is small (< 4 KiB).

## Formatting (rustfmt.toml-enforced)

- **Edition:** `2024`. `style_edition = "2024"`.
- **Max width:** 100 columns. Hard tabs disabled. Unix newlines.
- **Match arms keep trailing commas** (`match_block_trailing_comma = true`).
- **Field-init shorthand:** enabled. **`?`-shorthand:** enabled.
- **Derives merged** on a single attribute (`merge_derives = true`).
- **Stable rustfmt only.** Nightly-only options (`wrap_comments`, `imports_granularity`, `format_code_in_doc_comments`) live in the parent monorepo and are intentionally NOT mirrored here.

## Derives & Serialization

- **Conventional derive order:** `Debug, Clone, [Copy], [Default], (Partial)Eq, [Hash], Serialize, Deserialize`.
- **Configuration types:** TOML for human-edited config (`flow.toml`, `surge.toml`, profiles); preserve comments via `toml_edit`.
- **Event payloads:** `bincode` on disk; deterministic ordering (no `HashMap` in serialized payloads — use `BTreeMap` or ordered `Vec<(K, V)>`).
- **IDs:** `ulid` (sortable, monotonic). `chrono` for timestamps with `serde` integration.

## Persistence Conventions

- **One SQLite file per run** under `~/.surge/runs/<run_id>/events.sqlite`.
- **Append-only events table.** Triggers must reject UPDATE and DELETE on `events`.
- **Materialized views are maintained in the same transaction as the event append.** Rebuildable by replaying the event log.
- **`schema_version` field per event.** Add a migration step to the chain when the payload shape changes.
- **Folding is deterministic.** No wall-clock reads, no random IDs introduced inside a fold.

## Git & Worktrees

- **One git worktree per run** via `git2`. Cleaned up on terminal outcome (merged or discarded).
- **Default base branch:** `main`. Feature branches use the `feature/` prefix (configured in `.ai-factory/config.yaml`).
- **`.worktrees/` is the local convention** for in-progress branches; do not commit it.

## CLI / Binary Conventions

- **`anyhow` is allowed only in `surge-cli` and other `*-cli` / `*-daemon` binary crates.**
- **Top-level subcommand structure** uses `clap` derive macros.
- **`surge.toml` and `surge.example.toml` are the user-facing config files.** Do not introduce a parallel format.

## Documentation

- **Public APIs:** `///` doc comments. Document panics (`# Panics`), errors (`# Errors`), and safety (`# Safety`) sections where applicable.
- **`docs/ARCHITECTURE.md`** is the canonical architecture document. `.ai-factory/DESCRIPTION.md` is the AI-context summary. Keep them in sync — update both when architecture changes.
- **Recognized acronyms in doc comments** (no `clippy::doc_markdown` warning): `GitHub`, `OAuth2`, `JSON`, `YAML`, `TOML`, `UUID`, `ULID`, `WebSocket`, `PostgreSQL`, `gRPC`, `GraphQL`.
