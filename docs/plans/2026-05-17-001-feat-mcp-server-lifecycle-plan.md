---
title: "feat: MCP server lifecycle — production-grade surge-mcp"
type: feat
status: active
date: 2026-05-17
deepened: 2026-05-17
---

# feat: MCP server lifecycle — production-grade surge-mcp

## Summary

Close the unchecked `[ ] MCP server lifecycle` milestone in `.ai-factory/ROADMAP.md` (lines 155–169). The `surge-mcp` crate already has working stdio transport, a lazy per-server registry, and is wired into the engine per run via `surge.toml [[mcp_servers]]`. This plan adds the missing production hardening: structured crash detection, child-stderr capture into `tracing` (+ a bounded per-connection stderr file), a real restart policy (backoff + cap + escalation), a periodic health monitor, deterministic per-run shutdown, replay-parity attribution for MCP tool calls (additive, version-bumped per the repo's own migration convention), reserved-name conflict resolution for surge-injected tools, sandbox-intent plumbing within the ADR-0006 delegation boundary, a request-scoped `surge mcp` operator surface, and a real filesystem-MCP integration test — then flips the roadmap box with a dated Completed row.

---

## Problem Frame

`surge-mcp` is acknowledged in the roadmap as a skeleton. It connects and routes tools, but lacks the liveness, supervision, observability, and operator-control properties an AFK orchestrator needs: a crashed MCP child today re-spawns immediately with no backoff or cap, its stderr is lost to the host fd, there is no health probe, no operator way to inspect or validate configured servers, no replay attribution for which server served a tool, and an MCP server could shadow a surge-injected tool name in the agent's catalog. The milestone is the last non-blocker gate before the "Crash recovery" v0.1 blocker; finishing it cleanly is required for the release train.

---

## Assumptions

*This plan was authored in pipeline mode without synchronous user confirmation. The items below are agent inferences that fill gaps in the milestone text — un-validated bets that should be reviewed before implementation proceeds. They were stress-tested by an architecture and a repo-research deepening pass; the surviving rationale is recorded here and in Alternative Approaches.*

- **`surge mcp` is a request-scoped daemon validate-drop, not a session manager.** Surge's architecture is per-run MCP isolation (one child per run, lazily spawned, dropped at run end — `crates/surge-mcp/README.md` "Lifetime — per-run"); persistent cross-run shared servers are the explicitly-deferred `McpServerRef::isolation = Shared` future (README "M9+"). Under per-run isolation there is no persistent daemon-held MCP child, so "manual lifecycle control" is honestly read as *validate-on-demand*: the daemon builds a transient connection, handshakes, lists tools, reports health, and tears it down in the same request. `surge mcp stop` is therefore near-vacuous by design (an idempotent ack — stated plainly, not worked around by manufacturing persistent state). The third conceivable reading — `stop` terminates the MCP child of a *currently-executing run* — is named and rejected in Alternative Approaches A1 (the per-run `mcp_registry` is owned by the spawned run task in `RunTaskParams`, not reachable from the daemon's `ActiveRun` control surface, and U3 already tears those children down deterministically on terminal outcome/abort, so halting MCP activity is `surge run abort`, not `surge mcp stop`). This avoids introducing the codebase's first runtime-mutable persistent daemon subsystem.
- **Restart *counts/timings* are not event-sourced; the terminal give-up *fact* is.** Restart attempt counters and retry timestamps are timing-dependent and non-deterministic, so they never enter the folded event log (ADR-0006 replay invariant). But the give-up itself is surfaced as an `EscalationRequested` event appended from `surge-orchestrator` — verified replay-safe: `EscalationRequested` is a fold **pass-through** (`run_state.rs:542` catch-all `(state, _) => Ok(state)`; its doc comment designates it for out-of-band dispatchers to surface a message *without* replaying the log). This is the established, version-additive give-up vehicle the Telegram cockpit already renders (`surge-telegram/.../cockpit/dispatch.rs:226` → `CardKind::Escalation`); routing MCP exhaustion only through a direct `surge-notify` call would leave an AFK operator with **no cockpit card** when an MCP server dies permanently mid-run, defeating the "walk away" value prop. The `EscalationRequested` append carries only the stable give-up fact (`server`, reason), not the non-deterministic attempt count. No `surge-mcp → surge-notify` crate edge is added (the append happens in `surge-orchestrator`, which already owns the event writer). Only *tool delegation* additionally gets replay *attribution* (U4) via an additive, version-bumped migration consistent with the repo's v2 precedent.
- **"MCP children honor SandboxIntent where the runtime supports it" means intent plumbing + portable hygiene + the ADR-0006 delegation boundary, not OS-level syscall sandboxing of arbitrary MCP binaries.** There is no common sandbox-flag grammar for arbitrary MCP server binaries (the sandbox matrix is keyed by ACP `RuntimeKind`, not generic child processes). Surge configures/passes intent and applies what is portably enforceable (declared-env-only + minimal PATH, cwd pinned to the run worktree, `ReadOnly` → MCP fully denied), and documents that deeper enforcement is delegated to the runtime per ADR-0006.

---

## Requirements

- R1. A crashed MCP child reconnects under an exponential-backoff policy with a capped attempt count; exhaustion escalates rather than hot-looping. (roadmap: "Restart policy on child crash")
- R2. MCP child stderr is captured and emitted through `tracing` with a stable target, and tee'd to a bounded per-connection stderr file. (roadmap: "Structured logs from MCP child stderr captured via `tracing`")
- R3. A periodic health probe marks a connection unhealthy after N consecutive failures and feeds the restart policy. (roadmap: "Health checks: periodic `tools/list` ping")
- R4. MCP children are deterministically torn down on run terminal outcome (no orphaned processes). (roadmap: "Daemon starts MCP children with run lifecycle; stops on terminal outcome")
- R5. `surge mcp list | start <name> | stop <name> | logs <name>` exist and route to runtime status. (roadmap: 3 CLI bullets)
- R6. MCP tool delegation is attributable per server in the replay log. (roadmap: "Tool delegation surfaced as `ToolCall` / `ToolResult` events for replay parity")
- R7. Surge-injected tools (`report_stage_outcome`, `request_human_input`) win on name collision and are never shadowed in the agent's tool catalog by an MCP tool. (roadmap: "Tool-name conflict resolution")
- R8. MCP children honor the run's sandbox intent within the ADR-0006 delegation boundary. (roadmap: "Sandbox: MCP children honor the run's `SandboxIntent`")
- R9. An integration test exercises a real third-party MCP server (filesystem MCP), not just the mock. (roadmap: "Integration test against a known MCP server")
- R10. Every milestone deliverable is implemented or explicitly verified pre-existing; `cargo build --workspace` + `cargo test` + `cargo clippy --workspace` clean; roadmap box flipped to `[x]` with a dated Completed row.

---

## Scope Boundaries

- Not adding non-stdio MCP transports (HTTP/SSE/socket) — `McpTransportConfig` stays `#[non_exhaustive]` Stdio-only; the milestone does not require them.
- Not refactoring the `ensure_connected` mutex-held-across-spawn limitation into an explicit `Connecting` state with shared join handle (documented M7+ note in `connection.rs`) — out of scope unless health/restart work forces it.
- Not generalizing a shared daemon supervisor primitive across inbox/cockpit/MCP — see Deferred (and note: no reusable supervisor exists today; the cockpit's is doc-only).
- Not policing MCP child syscalls / building an OS sandbox wrapper for arbitrary MCP binaries — explicitly delegated to the runtime per ADR-0006 / `docs/ARCHITECTURE.md:150`.
- Not introducing a runtime-mutable persistent daemon subsystem or daemon-resident MCP session/ring-buffer state — `surge mcp` is request-scoped (see Alternative Approaches A1).

### Deferred to Follow-Up Work

- **Persistent cross-run shared MCP servers** (`McpServerRef::isolation = Shared`, warm reuse across runs): future shared-server milestone — conflicts with current per-run isolation; the README already records this as M9+.
- **Shared `surge-daemon` supervisor extraction**: confirmed there is no reusable supervisor helper today (the cockpit's "3-retry-then-give-up" is documented at `main.rs:1471-1478` but **not implemented** — `spawn_cockpit` is a plain `tokio::spawn`+`select!`). Extraction across inbox/cockpit/MCP is a separate refactor PR; this plan inlines the policy in `surge-mcp`.
- **Capturing MCP child exit codes for diagnostics**: rmcp consumes the child exit status internally and only `tracing`-logs it (no API surface); scraping rmcp's internal tracing event is deferred — not required by any milestone bullet.

---

## Context & Research

### Relevant Code and Patterns

- `crates/surge-mcp/src/connection.rs` — `McpServerConnection` (`config`, `state` only — no cwd/path today), `ConnState { Disconnected, Running(Arc<RunningService<RoleClient,()>>), Crashed { last_exit } }`. `ensure_connected` (`:118`) holds the state mutex across spawn+handshake; the single spawn block is `:131-151` (`tokio::process::Command::new` → `.args` → `env` loop → `TokioChildProcess::new`); `().serve` at `:159`; `classify_rmcp_error` string heuristic at `:32`; `mark_crashed` at `:253`; test constructors at `:279,342`.
- `crates/surge-mcp/src/registry.rs` — `McpRegistry { servers: HashMap<String, Arc<McpServerConnection>> }`, `from_config(refs: &[McpServerRef])` (lazy, `:86` — no cwd param today), `list_all_tools` (`:105`, sorted), `call_tool` (`:146`); test constructors at `:197,279`.
- `crates/surge-mcp/src/error.rs` — `McpError` `#[non_exhaustive]` thiserror. `ServerCrashed`/`ToolNotFound` defined but never constructed.
- `crates/surge-core/src/mcp_config.rs` — `McpServerRef { name, transport, allowed_tools, call_timeout, restart_on_crash }`, `McpTransportConfig::Stdio { command, args, env }`, both `#[non_exhaustive]` with constructors. No sandbox field.
- `crates/surge-core/src/run_event.rs:191-200` — `ToolCalled { session, tool, args_redacted: ContentHash }`, `ToolResultReceived { session, success, result: ContentHash }`. `discriminant_str` (`:416-467`) keys on **variant name only** (`Self::ToolCalled { .. } => "ToolCalled"`) — unaffected by a new field. `to_bincode`/`from_bincode` (`:402-408`, JSON under the hood). `EscalationRequested` at `:382`. `RunConfig.mcp_servers` at `:503-514`. Fold pass-through for `ToolCalled` at `crates/surge-core/src/run_state.rs:528-542` (catch-all `(state, _)`).
- `crates/surge-core/src/migrations/mod.rs` — **`pub const MAX_SUPPORTED_VERSION: u32 = 2;` (`:27`)**. Documented precedent (`:21-26`, `IdentityV2` `:58-76`): v2 added `SandboxElevationTimedOut`/`RuntimeVersionWarning` — *purely additive, old payloads parse cleanly, but the wrapper version was bumped 1→2 so readers know the variants are in scope*. `MigrationChain::new()` at `:88`; `writer_emits_max_supported_version` test at `:189-197`. (Path is `migrations/mod.rs`, not `migrations.rs`.)
- `crates/surge-core/src/sandbox.rs:38-52` — `SandboxMode` `#[non_exhaustive]` (`ReadOnly | WorkspaceWrite | WorkspaceNetwork | FullAccess | Custom`), `SandboxConfig`.
- `crates/surge-orchestrator/src/engine/engine.rs:284-290` (start_run builds `Arc<McpRegistry>`; `worktree_path` is a sibling `start_run` arg landing in `RunTaskParams.worktree_path` `:319`, **not** joined to the registry today), `:622-628` (resume_run mirror — builds a second registry), `:294` (`CancellationToken::new()` per-run pattern), `:332-333` (`RunTaskParams.mcp_registry`).
- `crates/surge-orchestrator/src/engine/config.rs:45-65` — `EngineRunConfig` has `mcp_servers` but **no worktree field**.
- `crates/surge-orchestrator/src/engine/tools/routing.rs:43-105` (`RoutingToolDispatcher::new`), `:87-97` engine tools overwrite MCP collisions, `:110-141` dispatch returns only `ToolResultPayload`; `ToolOrigin` (`:18-23`) and `routing_table` are **private**. `crates/surge-orchestrator/src/engine/tools/mod.rs:65-104` — `DeclaredTool`, `ToolDispatcher` trait (consumed via `Arc<dyn ToolDispatcher>`).
- `crates/surge-orchestrator/src/engine/stage/agent.rs:282-354` filter pipeline, `:649` injected-tool bridge bypass, `:713-746` `ToolCalled`/`ToolResultReceived` emission (construction sites `:718,739`), `:1633-1660` `sandbox_allows_mcp_tool` stopgap (call site `:323`).
- `crates/surge-persistence/src/runs/views.rs:360-361` — destructures these events with `{ .. }` (no positional break risk).
- CLI: `crates/surge-cli/src/commands/doctor.rs` (canonical multi-subcommand + format + `async fn run`), `crates/surge-cli/src/commands/daemon.rs:102` (`DaemonEngineFacade::connect`). Register in `crates/surge-cli/src/commands/mod.rs` + `crates/surge-cli/src/main.rs:36-169` / `:323-499` / `:287-303` (orphan exclusion).
- IPC/daemon: `crates/surge-orchestrator/src/engine/ipc.rs:60-166` (`DaemonRequest` `#[non_exhaustive]` `#[serde(tag="method")]`), **`request_id()` at `:168-187` is an exhaustive `match self` with NO wildcard** (new variants must add arms or it won't compile), `DaemonResponse` `:190+`. `crates/surge-daemon/src/server.rs:270-833` dispatcher (`_ => BadRequest`), `:384-391` `facade.start_run(... worktree_path ...)`. `crates/surge-daemon/src/main.rs:1313-1439` `spawn_inbox_subsystems` (immutable-after-spawn), `:1471-1604` cockpit (doc-comments a supervisor that is not implemented). Engine control-channel precedent: `ActiveRun` map behind `Arc<RwLock>` + `roadmap_amendments` mpsc + per-run `CancellationToken` (`engine.rs:296-309`, `stop_run` `:821-838`).
- Tests: `crates/surge-mcp/tests/mcp_stdio_e2e.rs` (`#[tokio::test] #[ignore]`, binary-locate walk-up, `server_ref` helper), `crates/surge-mcp/tests/fixtures/mock_mcp_server.rs` (under `--features mock-server`), `crates/surge-orchestrator/tests/{engine_m7_routing_dispatcher.rs,real_acp_smoke.rs}` (env-gated SKIPPED-banner pattern), `crates/surge-daemon/tests/` (15 `daemon_*` files). `docs/development.md:39-58` ignored-test convention.

### Institutional Learnings

- **ADR-0006 (ACP-only transport)** + `docs/ARCHITECTURE.md:150`: provider-native/MCP tools execute under the agent runtime's own sandbox; surge sees them as ToolCall/ToolResult but **does not enforce** them. Replay must produce byte-identical state at every `seq`. Cite for R7/R8.
- **Migration convention (verified in `migrations/mod.rs`)**: additive event variants/fields that don't break the wire shape **still bump `MAX_SUPPORTED_VERSION`** and add an `IdentityVN` chain entry, specifically so downstream readers know the new payloads are in scope (the v2 precedent). U4 follows this exactly — the earlier "no bump" assumption was wrong and has been corrected throughout.
- **ADR-0013 (tracker automation tiers)**: closed-enum + `#[non_exhaustive]` resolved at one canonical site, graceful degradation (unknown config → WARN + safe fallback). Applied to the sandbox-policy resolver (U6).
- **No reusable supervisor / backoff module exists.** The retired `circuit_breaker.rs`/`retry.rs` were folded into engine `on_error` hooks (stage-outcome routing, not process liveness). The cockpit "3-retry supervisor" is **documented but not implemented** (`main.rs:1471-1478` doc vs `spawn_cockpit` plain `tokio::spawn`+`select!`). The only reusable shape is the `CancellationToken` + `select!`-on-`cancelled()` *idiom* (real examples: the inbox/cockpit `select!` loops). U2's restart/escalation is genuinely net-new — design it from rmcp semantics, not by mirroring a non-existent supervisor.
- **House plan contract**: verbose logging with explicit `tracing` targets, **no TRACE** (DEBUG/INFO/WARN/ERROR only); +50% estimate buffer; half-implementations forbidden (decide-or-defer); mandatory docs checkpoint (`docs/mcp.md` + `docs/cli.md` + `docs/ARCHITECTURE.md` + ROADMAP flip + Completed row).

### External References

rmcp `>=1.6, <2.0` (1.6.0 ↔ 1.7.0 byte-identical for child-process transport):

- **stderr capture (supported):** `rmcp::transport::child_process::TokioChildProcess::builder(cmd).stderr(std::process::Stdio::piped()).spawn() -> io::Result<(TokioChildProcess, Option<tokio::process::ChildStderr>)>`. `new` inherits stderr (unreadable); the builder's `spawn()` unconditionally overwrites Command-level stdio, so manual `Command.stderr()` is ignored — **must** use the builder.
- **liveness:** no exit-code/`Child` API. `RunningService::is_closed() -> bool` (`&self`, cheap) is the primary dead-transport signal. No `ping` verb for `RoleClient`. Active probe = `Peer::list_tools(None) -> Result<ListToolsResult, ServiceError>` (single page; `list_all_tools()` paginates — discovery only).
- **shutdown:** `RunningService::cancel(self).await -> Result<QuitReason, JoinError>` (or `close(&mut self)`); drop is async best-effort (orphan risk) — explicit `cancel().await` required. Underlying `graceful_shutdown` waits 3s then `kill()`.
- **errors are structured (no string-matching):** `rmcp::ServiceError` `#[non_exhaustive]` { `McpError(ErrorData)`, `TransportSend(_)`, `TransportClosed`, `UnexpectedResponse`, `Cancelled{reason}`, `Timeout{timeout}` }. `TransportClosed | TransportSend` ⇒ child dead → restart; `McpError(_)` ⇒ server-level (alive). `().serve()` errors with `ClientInitializeError` `#[non_exhaustive]`.
- **Windows:** project targets Windows; `tokio::process::Command` won't resolve `npx`/`.cmd` by bare name. Add rmcp feature `which-command` and use `rmcp::transport::which_command("npx")` for the filesystem-MCP test. **Workspace-inheritance shape (do NOT add a standalone version-pinned line):** `crates/surge-mcp/Cargo.toml` uses `rmcp.workspace = true`; the rmcp version + feature set live at the workspace-root `Cargo.toml` (currently `default-features=false, features=["client","transport-child-process"]`). Add `which-command` either at the workspace root features array, or crate-side additively as `rmcp = { workspace = true, features = ["which-command"] }`. The crate manifest must not carry its own `version=`/`default-features=` for rmcp.

---

## Key Technical Decisions

- **Replace `classify_rmcp_error` string heuristic with structured `ServiceError` matching.** Match `TransportClosed`/`TransportSend(_)` vs `McpError(_)`; `_ =>` arm (non_exhaustive) defaults conservatively to "service, do not mark crashed" + WARN. Foundation for trustworthy crash/health detection.
- **stderr capture via the builder, tee'd to a bounded file.** Spawn path uses `TokioChildProcess::builder(cmd).stderr(Stdio::piped()).spawn()`; a detached `BufReader::lines()` task forwards each line to `tracing` (target `mcp::child::stderr`, INFO, with `server` field) **and** appends to a bounded per-connection stderr file (so `surge mcp logs` has a source without any daemon-resident ring buffer).
- **Restart policy is a new dedicated mechanism in `McpServerConnection`**, not routed through engine `on_error` hooks. Exponential backoff (base, factor, cap) + max attempts; runtime-only bookkeeping (`attempts`, `next_retry_at`), **never** in any event payload. Fast-return before `next_retry_at` without spawning; the no-hot-loop guarantee is explicitly contingent on rate-limited callers (request-driven agent stage; interval-driven health monitor whose interval ≥ backoff cap). Exhaustion → new `McpError::RestartExhausted { server, attempts }` + a single ERROR `mcp::supervisor` line **and** an `EscalationRequested` event appended by `surge-orchestrator` (U3's engine error/terminal path). `EscalationRequested` is verified replay-safe — a fold pass-through (`run_state.rs:542` catch-all), explicitly designed for out-of-band give-up surfacing — and is the **only** vehicle the Telegram cockpit renders give-up from (`cockpit/dispatch.rs:226` → `CardKind::Escalation`); without it an AFK operator gets no card when an MCP server dies permanently mid-run. The event payload carries only the stable give-up fact (`server`, reason), not the non-deterministic attempt count. No `surge-mcp → surge-notify` crate edge (the orchestrator owns the event writer; it would otherwise pull `surge-intake`/`reqwest`/`lettre` into the near-leaf crate).
- **Replay attribution = additive field + version bump (repo convention).** Add `#[serde(default)] mcp_server: Option<String>` to `ToolCalled`/`ToolResultReceived`, **bump `MAX_SUPPORTED_VERSION` 2→3 and add `IdentityV3` to `MigrationChain::new()`**, mirroring the `IdentityV2` precedent (old v1/v2 logs decode cleanly via serde default; the bump is the documented in-scope signal). `discriminant_str` unchanged (keyed on variant name); no positional-destructure breaks (only `agent.rs:718,739` construct; readers use `{ .. }`/wildcard). Origin is surfaced via a **new `ToolDispatcher::resolved_origin(&self, tool: &str) -> Option<String>` trait method** (default `None`; `RoutingToolDispatcher` overrides via its private `routing_table`) — the stage holds only an `Arc<dyn ToolDispatcher>`, so a trait-method seam is required (the routing table cannot be "read back").
- **Deterministic per-run shutdown + cancellation seam in U3.** `McpRegistry::shutdown(&self)` `cancel().await`s every `Running` connection and cancels the registry `CancellationToken`; the engine calls it on terminal outcome (time-bounded so a hung child can't wedge completion). The `CancellationToken` + `McpHealth` status enum + `statuses()` surface are introduced in U3; the health monitor (U11) is sequenced after U3 so its task is never born without a cancellation source.
- **Reserved injected-tool names**: one canonical `RESERVED_INJECTED_TOOLS` const in `routing.rs`; `RoutingToolDispatcher::new` drops reserved-named MCP entries from the declared catalog + routing table with one WARN per collision; regression test asserts a single arbitration site. Cite ADR-0006.
- **Sandbox**: add `#[serde(default)] sandbox: Option<SandboxMode>` to `McpServerRef`; canonical `mcp_spawn_policy(run_mode, server_override) -> {Denied|Allowed}` replacing `sandbox_allows_mcp_tool` (`ReadOnly ⇒ Denied`; writable/network/full/custom ⇒ Allowed; unknown ⇒ Denied + WARN). Spawn hygiene (declared-env-only + minimal PATH, cwd = run worktree) lands in U1 (same spawn block); U6 owns the field + resolver + ADR-0014.
- **`surge mcp` = request-scoped daemon validation, not a session manager.** No runtime-mutable subsystem, no daemon-resident sessions, no ring buffer. New IPC verbs `McpProbe { name: Option<String> }` (build a transient connection from `SurgeConfig.mcp_servers`, `ensure_connected` + `list_tools(None)`, report `McpHealth` + tool count, `shutdown().await` immediately) and `McpLogs { name, tail }` (tail the bounded stderr file for the most recent probe/run of `name`); `surge mcp stop` is a CLI-side idempotent ack (no persistent child exists under per-run isolation — stated, not worked around). `DaemonRequest::request_id()` exhaustive match gains arms for the new verbs (not "purely additive" on the ipc.rs side).
- **Logging discipline**: targets `mcp::child::stderr`, `mcp::supervisor`, `daemon::mcp`. DEBUG/INFO/WARN/ERROR only — **no TRACE**.

---

## Open Questions

### Resolved During Planning

- *Detect MCP child death reliably?* — Structured `ServiceError::{TransportClosed,TransportSend}` + `RunningService::is_closed()`; no exit code.
- *New event variants for tool delegation?* — No; additive `mcp_server: Option<String>` + `MAX_SUPPORTED_VERSION` 2→3 + `IdentityV3` (repo convention).
- *How does the stage learn the serving server?* — New `ToolDispatcher::resolved_origin()` trait method (routing table is private; dispatcher is a trait object).
- *What does `surge mcp start/stop` mean under per-run isolation?* — Request-scoped validate-drop; `stop` is an idempotent ack; persistent shared servers deferred (Alternative A1).
- *Reusable supervisor/backoff helper?* — None exists (cockpit's is doc-only); inline in `surge-mcp`, reuse only the `CancellationToken`+`select!` idiom.
- *`surge-mcp → surge-notify` edge?* — No; escalate via `McpError::RestartExhausted` + orchestrator-side notify.
- *MCP children honor sandbox without a generic OS sandbox?* — Intent plumbing + ReadOnly-deny + env/cwd hygiene; deeper enforcement delegated per ADR-0006.

### Deferred to Implementation

- Exact backoff constants (base/factor/cap/max-attempts), health interval (≥ backoff cap), N, stderr-file capacity — sensible documented defaults; promote to config only if a test demands.
- Whether to repurpose dead `McpError::ServerCrashed` or add a fresh variant alongside `RestartExhausted` — decide when touching `error.rs` (U2).
- Whether `DaemonResponse` has a symmetric exhaustive accessor needing arms — verify at U7 (`ipc.rs:190+`).

---

## Alternative Approaches Considered

### A1. `surge mcp` surface (load-bearing fork)

- **Option A — persistent daemon `McpDiagnosticManager` subsystem** (`HashMap<name,session>` + per-session stderr ring buffer, 4 IPC verbs, `spawn_inbox_subsystems`-wired). *Rejected:* introduces the codebase's first runtime-mutable persistent daemon subsystem and a new cross-boundary surface to satisfy an AFK config-validation bullet; partly exists only to give the vestigial `stop` something to kill. Also would require an `Arc<Mutex<…>>`/mpsc command-channel race-control layer (no spawn-and-forget precedent fits a mutable manager).
- **Option B — in-process one-shot `surge mcp check`, no daemon.** *Rejected as pure form:* correct shape for `list`, but leaves `stop` vestigial and `logs` sourceless → half-implementation (house rule forbids).
- **Option C — daemon owns long-lived MCP children runs attach to.** *Rejected:* this is `McpServerRef::isolation = Shared`, deferred to M9+; re-architects per-run isolation for a CLI bullet.
- **Option D — `stop` terminates the MCP child of a *currently-executing run*.** This is the interpretation an operator most naturally expects from "manual lifecycle control". *Rejected as out-of-scope for this milestone, with rationale (not silently dropped):* the per-run `Arc<McpRegistry>` is owned by the spawned run task via `RunTaskParams.mcp_registry` (`engine.rs:332`), not held in the daemon's `ActiveRun` map — reaching it from the IPC dispatcher would need new plumbing to expose the per-run registry through the run-control surface. More fundamentally it is *redundant*: U3 already tears down per-run MCP children deterministically on terminal outcome **and** on run abort (the run `CancellationToken` path), so an operator who wants to halt MCP activity in a live run uses the existing `surge run abort`/run-control surface, not `surge mcp stop`. Exposing a second, narrower kill path for one subsystem of a run is scope the roadmap bullet does not require and would fragment run lifecycle control. If a future need arises it pairs naturally with the deferred shared-server mode (Scope Boundaries).
- **CHOSEN — Option A′: request-scoped daemon validate-drop.** Daemon builds a transient connection per request, validates, tears down; `logs` tails the U1 bounded stderr file; `stop` is an explicit idempotent ack (its honest scope under per-run isolation; live-run halt is `surge run abort` per Option D). Honors all four CLI bullets, preserves per-run isolation, no runtime-mutable subsystem, minimal IPC surface.

### A2. Restart/health supervision & give-up surfacing

Dedicated `McpServerConnection` mechanism (no reusable shared supervisor exists; the cockpit's is doc-only). Shared-supervisor extraction across inbox/cockpit/MCP deferred to a separate refactor PR. **Give-up is surfaced via an `EscalationRequested` event** (appended in `surge-orchestrator`), *not* a direct-`surge-notify`-only path: `EscalationRequested` is a verified fold pass-through (`run_state.rs:542`), explicitly the replay-safe out-of-band give-up vehicle, and is the only event the Telegram cockpit renders give-up from (`cockpit/dispatch.rs:226`). A notify-only route was rejected because it is invisible to the cockpit and the event tap, breaking AFK visibility of permanent MCP failure. Only the non-deterministic attempt count is kept out of the payload.

### A3. Tool-call attribution

Additive serde-default field + `MAX_SUPPORTED_VERSION` 2→3 + `IdentityV3` (repo convention) over new `RunEvent` variants — additive, replay-clean, follows the documented v2 precedent.

### A4. stderr capture

`TokioChildProcess::builder(...).stderr(piped())` over `::new` (which inherits stderr unreadably; builder `spawn()` overwrites Command-level stdio anyway).

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

Connection lifecycle (state additions are runtime-only, never in the event log):

```
                 first use / restart
 Disconnected ───────────────────────────▶ Connecting ──ok──▶ Running ──┐
      ▲                                         │ err               │   │ health probe (U11, after U3)
      │ reset (successful connect only)         ▼                    │   │ is_closed() | list_tools(None)
      │                                  backoff(attempt)            │   ▼
      └──────────────── Crashed ◀───────── transport-dead ◀──────────┘  N consecutive fails
                           │                  (ServiceError::                ⇒ unhealthy ⇒ restart
                           │ attempts > max    TransportClosed/Send)
                           ▼
                  RestartExhausted ⇒ McpError + ERROR(mcp::supervisor)
                  + orchestrator-side surge-notify (replay-invisible)
```

Tool-call attribution & conflict resolution (engine side, replay-additive, schema v3):

```
agent tool call ─▶ Arc<dyn ToolDispatcher>
   ├─ name ∈ {report_stage_outcome, request_human_input}  ⇒ reserved: never in MCP catalog (ADR-0006)
   ├─ resolved_origin(tool) == None        ⇒ engine dispatcher ⇒ ToolCalled{ mcp_server: None }
   └─ resolved_origin(tool) == Some(srv)   ⇒ McpRegistry(srv)   ⇒ ToolCalled{ mcp_server: Some(srv) }
```

---

## Implementation Units

Grouped into 5 phases (Deep tier). Each unit is an independently-buildable commit. U-IDs are stable: U1–U10 retain identity; U11 (added during deepening) is the next unused number — no renumbering.

### U1. Structured error classification + stderr capture/tee + spawn hygiene + cwd/env plumbing

**Goal:** Replace the string-heuristic classifier with structured `rmcp::ServiceError` matching; switch the spawn path to the stderr-capturing builder, forward stderr to `tracing` and a bounded per-connection file; land all spawn-block hygiene (declared-env-only + minimal PATH + cwd = run worktree) and the one-time `McpServerConnection::new`/`McpRegistry::from_config` signature change so the spawn site is rewritten exactly once.

**Requirements:** R2, R8 (portable-hygiene half; foundation for R1, R3)

**Dependencies:** None

**Files:**
- Modify: `crates/surge-mcp/src/connection.rs` (classifier, builder spawn, stderr tee+redaction, `new(config, cwd: Option<PathBuf>)` + struct field + Debug impl `:261`, test constructors `:279,342`)
- Modify: `crates/surge-mcp/src/registry.rs` (`from_config(refs, cwd: Option<&Path>)`, test constructors `:197,279`)
- Modify: `crates/surge-mcp/src/error.rs`
- Modify: workspace-root `Cargo.toml` rmcp features array OR `crates/surge-mcp/Cargo.toml` (`rmcp = { workspace = true, features = ["which-command"] }` — see External References; no standalone version-pinned line)
- Modify: `crates/surge-orchestrator/src/engine/engine.rs` (start_run `:287` pass `&worktree_path`; resume_run `:622-628` mirror)
- Modify (signature-fanout — required for an independently-buildable commit; R10 needs `cargo test`/`build --workspace` clean): `crates/surge-mcp/tests/mcp_stdio_e2e.rs` (`:55,71` `server_ref`/`from_config` call sites), `crates/surge-orchestrator/tests/engine_m7_routing_dispatcher.rs` (`:35,62`), `crates/surge-orchestrator/src/engine/tools/routing.rs` (test mod `:202,232,266`) — pass `None` for the new `cwd` arg
- Test: `crates/surge-mcp/src/connection.rs` (`#[cfg(test)] mod tests`)

**Approach:**
- Delete `classify_rmcp_error` + `ErrorClass`; classify on `ServiceError`: `TransportClosed | TransportSend(_)` ⇒ transport-dead (mark crashed); `McpError(_) | UnexpectedResponse` ⇒ service; `Timeout{..}` ⇒ `McpError::Timeout`; `Cancelled{..}` ⇒ service; `_ =>` conservative service + WARN.
- Spawn: `TokioChildProcess::builder(tokio_cmd).stderr(Stdio::piped()).spawn()` → `(transport, Option<ChildStderr>)`; on `Some`, detached `BufReader::lines()` task → INFO `target:"mcp::child::stderr"` (field `server`) **and** append to a bounded per-connection stderr file. Each line passes a **redaction pass before emit/append** — strip/mask common secret shapes (`Bearer `, `api_key=`, `token=`, `password=`, long base64/hex blobs); redaction is opt-out per server (`mcp_servers.redact_stderr = false`, default on) so a noisy MCP server cannot leak credentials into surge logs or the on-disk file. Stderr file path is **run-scoped**: `<worktree>/.surge/mcp-stderr/<server>.log` for run connections (deleted with the worktree); daemon probes (U7, `cwd: None`) write to a daemon-scoped temp path. Capacity a documented const; overflow is a ring/rotate (not silent truncate) so `surge mcp logs` shows the tail of a crash, not the head.
- Hygiene in the same block: build the child env from declared `env` + a **minimal PATH** (documented allowlist of system dirs — `/usr/bin:/bin` on Unix, the System32 dir on Windows — not the inherited shell PATH); `tokio_cmd.current_dir(cwd)` when `Some`. Thread `cwd: Option<PathBuf>` (owned) into `McpServerConnection` (stored as `Option<PathBuf>`); `McpRegistry::from_config(refs, cwd: Option<&Path>)` clones into each connection. Update the test constructors and both engine build sites (start_run + resume_run) and the signature-fanout test call sites listed in Files.
- `error.rs`: keep `#[non_exhaustive]`; note dead `ServerCrashed`/`ToolNotFound` for U2's variant decision.

**Patterns to follow:** existing error mapping in `connection.rs`; `#[non_exhaustive]` + `_ =>`-with-`tracing::warn!` (`routing.rs:157`); rmcp builder per External References.

**Test scenarios:**
- Happy path: `ServiceError::TransportClosed` ⇒ transport class, marks crashed; `ServiceError::McpError(..)` ⇒ service, not crashed.
- Edge case: `_ =>` arm classifies unknown as service + WARN (classifier returns a class enum testable without a live server).
- Edge case: child env contains only declared keys + PATH (assert via the mock server echoing env, gated/`#[ignore]`); cwd is the supplied dir.
- Error path: builder spawn of a non-existent command yields `McpError::StartFailed` (preserve `call_tool_on_bad_command_returns_start_failed`).
- Edge case: a stderr line containing `Authorization: Bearer abc123` / `api_key=...` is masked before it reaches the `tracing` capture and the file (redaction-pass unit test, no live server needed).
- Integration: a line on child stderr appears via the `tracing` forwarder and in the run-scoped stderr file (gated; reuse U9 harness — see U11 note).

**Verification:** `classify_rmcp_error` gone; `ensure_connected` spawn block rewritten once (builder + env + cwd + redacted tee); `McpServerConnection`/`from_config` carry `cwd`; both engine sites + the signature-fanout test call sites (`mcp_stdio_e2e.rs`, `engine_m7_routing_dispatcher.rs`, `routing.rs` test mod) updated; rmcp feature added via workspace inheritance (no standalone version line in the crate manifest); `cargo build --workspace` + `cargo test` clean; existing connection tests pass.

---

### U2. Restart policy: exponential backoff + capped attempts + escalation

**Goal:** A crashed connection reconnects under exponential backoff with a capped attempt count; exhaustion produces `McpError::RestartExhausted` + a single ERROR line and stops hot-looping. No new crate edge.

**Requirements:** R1

**Dependencies:** U1

**Files:**
- Modify: `crates/surge-mcp/src/connection.rs`
- Modify: `crates/surge-mcp/src/error.rs` (add `RestartExhausted { server, attempts }`)
- Test: `crates/surge-mcp/src/connection.rs` (`#[cfg(test)] mod tests`)

**Approach:**
- Runtime-only bookkeeping (`attempts: u32`, `next_retry_at: Option<Instant>`) — not persisted, not in events.
- Reconnect branch of `ensure_connected`: if `now < next_retry_at` return a fast `McpError` **without spawning** and **without resetting `attempts`** (only a successful connect resets); on spawn failure increment `attempts`, set `next_retry_at = now + min(base*factor^attempts, cap)`; when `attempts > max` transition to terminal exhausted and return `McpError::RestartExhausted { server, attempts }`.
- Exhaustion: emit one ERROR `target:"mcp::supervisor"` (`mcp_supervisor_gave_up server={} after_attempts={}`). No notify here and no `surge-notify` dep — the orchestrator maps `RestartExhausted` to an `EscalationRequested` event append (U3's engine edit), which the cockpit/notify channels consume; `surge-mcp` only returns the typed error + ERROR log.
- Document the rate-limited-caller invariant: the fast-return does not itself enforce backoff for a tight-looping caller; callers (agent stage; U11 monitor with interval ≥ cap) provide the rate limit.

**Patterns to follow:** real `select!`/error-logging loops (not the non-existent cockpit supervisor); `#[non_exhaustive]` enums; backoff as a pure function for deterministic testing.

**Test scenarios:**
- Happy path: after a transport-dead classification, a call past `next_retry_at` re-spawns and resets `attempts`.
- Edge case: backoff delay function is monotonic non-decreasing and clamps at `cap` (pure-fn unit test).
- Edge case: a call before `next_retry_at` returns fast without spawning **and does not reset `attempts`** (closes the cap-defeat gap).
- Error path: exceeding `max` yields `McpError::RestartExhausted`; connection refuses further spawn until reset; exactly one ERROR escalation line (single-site assertion).

**Verification:** killing a mock child does not hot-loop; backoff observed; exhaustion escalates exactly once; no `surge-mcp → surge-notify` dependency added.

---

### U3. Deterministic per-run shutdown + cancellation seam + registry status surface

**Goal:** MCP children are deterministically torn down on run terminal outcome (no orphans); introduce the registry `CancellationToken` seam, the `McpHealth` status enum, and `statuses()` so U11/U7/U8 build against them; surface restart give-up as an `EscalationRequested` event so the AFK cockpit renders it.

**Requirements:** R4, R1 (give-up surfacing half — completes U2; enables R3 via U11, R5)

**Dependencies:** U2

**Files:**
- Modify: `crates/surge-mcp/src/registry.rs` (`shutdown(&self)`, `statuses()`, `CancellationToken` field, `McpHealth` enum)
- Modify: `crates/surge-mcp/src/connection.rs` (`shutdown(&self)`, `status()` snapshot)
- Modify: `crates/surge-orchestrator/src/engine/engine.rs` (call `registry.shutdown()` on terminal outcome; map `McpError::RestartExhausted` → append an `EscalationRequested` event via the run's event writer)
- Test: `crates/surge-mcp/src/registry.rs` (`#[cfg(test)]`); `crates/surge-orchestrator/tests/engine_mcp_terminal_shutdown.rs` (+ assert the escalation append)
- Verify-and-touch if needed: `crates/surge-core/src/run_event.rs` (`EscalationRequested` payload — confirm it carries `server`/reason or a generic message field usable for MCP give-up; do **not** add a new variant)

**Approach:**
- `enum McpHealth { Connecting, Healthy, Unhealthy, Crashed, Exhausted }` + `McpServerConnection::status()` snapshot (non-mutating).
- `McpServerConnection::shutdown(&self)`: if `Running`, take the `RunningService` and `cancel().await` (orphan-free per rmcp research); → `Disconnected`.
- `McpRegistry`: add a `CancellationToken`; `shutdown(&self)` concurrently shuts down all connections (join set) **and** cancels the token (the seam U11's health task binds to); idempotent.
- Engine: on terminal outcome (RunCompleted/Failed/Aborted) call `per_run_mcp_registry.shutdown().await` wrapped in `tokio::time::timeout` before the `Arc` drops; force-abandon + WARN on timeout.
- Give-up surfacing: when a tool call / probe path observes `McpError::RestartExhausted { server, attempts }`, append an `EscalationRequested` event through the run's event writer (carrying only `server` + the give-up reason — not `attempts`, which is non-deterministic). `EscalationRequested` is a verified fold pass-through (`run_state.rs:542`), so this is replay-safe and version-additive (no fold-arm change). The Telegram cockpit already renders it as `CardKind::Escalation` (`cockpit/dispatch.rs:226`) and the notify multiplexer consumes it from the event tap — no `surge-mcp → surge-notify` edge. Confirm `EscalationRequested`'s payload shape suffices for an MCP give-up message before implementing; if it is run/stage-scoped only, carry the server name in its message/context field rather than introducing a new variant.
- `statuses() -> Vec<(String, McpHealth, ...)>` sorted by server name.

**Patterns to follow:** engine per-run `CancellationToken` (`engine.rs:294`), run-task teardown around `:332-333`; deterministic sorted iteration (`registry.rs:107-134`).

**Test scenarios:**
- Happy path: `registry.shutdown()` → all `Running` connections `Disconnected`; idempotent on second call; token cancelled.
- Edge case: shutdown on an all-`Disconnected` registry returns promptly.
- Error path: a connection whose `cancel()` exceeds the timeout is force-abandoned (WARN) and does not block registry shutdown.
- Integration (`engine_mcp_terminal_shutdown.rs`): an engine run reaching a terminal node invokes registry shutdown exactly once; a simulated `RestartExhausted` appends exactly one `EscalationRequested` event to the run log (assert via the event log/tap, not just `tracing`). (Add the test seam — this is mandatory for R4/R1-give-up, not conditional.)

**Verification:** no orphaned child after a run completes (observable in U9); `shutdown` idempotent + token-cancelling; engine terminal path calls it within the timeout budget; `RestartExhausted` produces a replayable `EscalationRequested` event the cockpit can render; named integration test exists and passes.

---

### U4. Replay-parity: per-server attribution + version bump + dispatcher origin seam

**Goal:** MCP tool delegation is attributable to the serving server in the replay log, additively, with the `MAX_SUPPORTED_VERSION` bump the repo convention requires.

**Requirements:** R6

**Dependencies:** None (Phase B; sequence after Phase A for clean commits)

**Files:**
- Modify: `crates/surge-core/src/run_event.rs` (add `#[serde(default)] mcp_server: Option<String>` to `ToolCalled`/`ToolResultReceived`)
- Modify: `crates/surge-core/src/migrations/mod.rs` (`MAX_SUPPORTED_VERSION` 2→3; add `IdentityV3` to `MigrationChain::new()` `:88`)
- Modify: `crates/surge-orchestrator/src/engine/tools/mod.rs` (`ToolDispatcher::resolved_origin(&self, tool:&str)->Option<String>`, default `None`)
- Modify: `crates/surge-orchestrator/src/engine/tools/routing.rs` (override `resolved_origin` from `routing_table`)
- Modify: `crates/surge-orchestrator/src/engine/stage/agent.rs` (call `resolved_origin` at `:716` before constructing the events `:718,739`)
- Test: `crates/surge-core/src/run_event.rs`, `crates/surge-core/src/migrations/mod.rs`, `crates/surge-orchestrator/tests/engine_m7_routing_dispatcher.rs`

**Approach:**
- Add the serde-default field to both variants; `#[serde(default)]` ⇒ pre-existing v1/v2 logs decode with `None`. **Bump `MAX_SUPPORTED_VERSION` 2→3 and add `IdentityV3`** to the chain (mirror `IdentityV2` `:58-76`). Update **both** hardcoded `max: 2`/version-2 assertions in `migrations/mod.rs` — `writer_emits_max_supported_version` (`~:196`, expects `schema_version == 2`) **and** `schema_version_too_new_is_rejected` (`~:204`, asserts `SurgeError::SchemaTooNew { found: 99, max: 2 }`); the tests that already use the `MAX_SUPPORTED_VERSION` constant (`migrations_v1_roundtrip.rs:48`, `migration_payload_v1.rs:77`) need no change. `discriminant_str` unchanged (variant-name keyed). State affirmatively that no positional destructure breaks (only `agent.rs:718,739` construct; `views.rs:360-361`/fold use `{ .. }`/wildcard).
- `ToolDispatcher::resolved_origin` default `None`; `RoutingToolDispatcher` returns `Some(server)` for `ToolOrigin::Mcp`. Stage calls it on the `Arc<dyn ToolDispatcher>` immediately before emitting; engine-only dispatchers return `None` ⇒ `mcp_server: None` falls out for free.
- Confirm fold pass-through (`run_state.rs:528-542` catch-all) unaffected; add a fold-determinism assertion.

**Patterns to follow:** `IdentityV2` migration entry + its round-trip test; `agent.rs:713-746` emission with `ContentHash::compute`; `to_bincode`/`from_bincode` round-trip tests.

**Test scenarios:**
- Happy path: an MCP-routed call emits `ToolCalled { mcp_server: Some("fs") }` + matching `ToolResultReceived`; an engine tool (dispatcher `resolved_origin == None`) emits `mcp_server: None`.
- Edge case: a legacy v1/v2 event JSON without the field decodes to `None` (`v3_bytes_round_trip_through_chain` + legacy-decode test).
- Integration: fold over attributed MCP events is byte-identical across two runs; `discriminant_str` unchanged; new writes emit version 3.

**Verification:** `MAX_SUPPORTED_VERSION == 3` with `IdentityV3` registered; round-trip + legacy-decode + fold-determinism green; `resolved_origin` seam present with default `None`.

---

### U5. Reserved injected-tool names — conflict resolution hardening

**Goal:** `report_stage_outcome` / `request_human_input` can never be shadowed in the agent tool catalog by an MCP tool of the same name; arbitration at one canonical site.

**Requirements:** R7

**Dependencies:** None (sequence after U4)

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/tools/routing.rs`
- Test: `crates/surge-orchestrator/tests/engine_m7_routing_dispatcher.rs`

**Approach:**
- One canonical `const RESERVED_INJECTED_TOOLS: [&str; 2]` in `routing.rs`. In `RoutingToolDispatcher::new`, after collision resolution, drop reserved-named MCP `declared` entries and never insert them into `routing_table`; one WARN `target:"mcp::supervisor"` per dropped collision (offending server named).
- Regression test asserts (a) a reserved-named MCP tool is absent from declared catalog + routing table, (b) exactly one reservation site (grep-style single-site assertion, cockpit-discipline style).

**Patterns to follow:** `routing.rs:87-97` engine-wins block; single-site regression discipline; ADR-0006 cited in a doc comment.

**Test scenarios:**
- Happy path: MCP `echo` routable; MCP `request_human_input` dropped + WARN.
- Edge case: two servers both advertising a reserved name — both dropped, two WARNs, no panic.
- Integration: existing engine-wins / MCP-vs-MCP first-wins tests still pass.

**Verification:** reserved names never in `declared_tools()`; single-site assertion green; ADR-0006 cited.

---

### U6. Sandbox intent field + canonical policy resolver + ADR-0014

**Goal:** `McpServerRef` can express a sandbox intent; one canonical resolver replaces the stopgop heuristic; the ADR-0006 delegation boundary + the U5 conflict + health/restart-not-event-sourced decisions are recorded in a new ADR. (Spawn-time env/cwd hygiene already landed in U1.)

**Requirements:** R8

**Dependencies:** U1 (owns spawn-path incl. env/cwd hygiene & cwd plumbing), U5 (ADR consolidates both decisions)

**Files:**
- Modify: `crates/surge-core/src/mcp_config.rs` (`#[serde(default)] sandbox: Option<SandboxMode>` + setter; keep existing constructor call sites compiling)
- Modify: `crates/surge-orchestrator/src/engine/stage/agent.rs` (replace `sandbox_allows_mcp_tool` `:1633-1660` + call site `:323` with the resolver)
- Create: `docs/adr/0014-mcp-server-lifecycle.md`
- Test: `crates/surge-core/src/mcp_config.rs`, `crates/surge-orchestrator/src/engine/stage/agent.rs` (`#[cfg(test)]`)

**Approach:**
- Canonical `mcp_spawn_policy(run_mode: &SandboxMode, server_override: Option<&SandboxMode>) -> McpSpawnPolicy { Denied | Allowed }`: effective = `server_override.unwrap_or(run_mode)`; `ReadOnly ⇒ Denied`; `WorkspaceWrite|WorkspaceNetwork|FullAccess|Custom ⇒ Allowed`; `_ ⇒ Denied + WARN` (fail-closed, ADR-0013). When the effective mode is `FullAccess` or `WorkspaceNetwork`, emit a one-line **WARN** `target:"mcp::supervisor"` naming the server + effective mode (operators get a log signal that this MCP child runs outside any surge-enforced OS boundary — the ADR-0006 delegation is otherwise silent). Replace all `sandbox_allows_mcp_tool` call sites; delete the stopgap.
- `McpServerRef.sandbox` serde-default `None` ⇒ inherit run intent. The spawn-time application (env/cwd) is U1's; U6 only adds the field + resolver.
- ADR `docs/adr/0014-mcp-server-lifecycle.md` (next number after 0013) per `docs/conventions/adr.md`: records (1) supervision is a dedicated mechanism (no reusable helper); (2) injected-tool reservation rationale; (3) sandbox delegation boundary + ReadOnly-deny + hygiene + the `FullAccess`/`WorkspaceNetwork` "unconstrained, operator-trusted binaries only" caveat; (4) restart give-up is surfaced via `EscalationRequested` (replay-safe pass-through), only attempt counts/timings are non-event-sourced; (5) `surge mcp` request-scoped (no mutable subsystem); (6) **daemon-IPC authz assumption** — the daemon control socket is accessible only to the OS user that started the daemon (Unix: socket file mode `0600` at bind; Windows: named-pipe DACL restricted to the creating user's SID); no additional per-verb authz is applied, and `surge mcp logs` therefore exposes captured stderr only to that user.

**Patterns to follow:** ADR-0013 closed-enum-single-resolver + degrade-with-WARN; `docs/conventions/adr.md`; `SandboxMode` `#[non_exhaustive]` matching.

**Test scenarios:**
- Happy path: `WorkspaceWrite` no override ⇒ `Allowed`; `ReadOnly` ⇒ `Denied`.
- Edge case: server override `ReadOnly` on a `FullAccess` run ⇒ `Denied` (restrictive override wins).
- Error path: unknown `SandboxMode` ⇒ `Denied` + WARN.
- Edge case: effective `FullAccess` (or `WorkspaceNetwork`) ⇒ `Allowed` **and** exactly one `mcp::supervisor` WARN naming the server + mode (operator-visibility for the delegated boundary).
- Integration: an MCP stage under a `ReadOnly` run sees no MCP tools (parity with the deleted stopgap — no regression).

**Verification:** `sandbox_allows_mcp_tool` deleted; one resolver site; ADR-0014 well-formed (incl. authz assumption + FullAccess caveat); FullAccess/WorkspaceNetwork WARN emitted; ReadOnly parity preserved.

---

### U7. Daemon-side request-scoped MCP validation + IPC verbs

**Goal:** The daemon can probe configured MCP servers on demand (validate config) and serve tailed stderr — without any runtime-mutable subsystem or daemon-resident state.

**Requirements:** R5 (daemon half)

**Dependencies:** U1 (cwd `Option`, stderr file), U3 (registry shutdown/status), U6 (spawn policy)

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/ipc.rs` (add `DaemonRequest::{McpProbe { name: Option<String> }, McpLogs { name, tail }}` + paired `DaemonResponse`; **add the new arms to the exhaustive `request_id()` match `:168-187`** and any symmetric `DaemonResponse` accessor)
- Modify: `crates/surge-daemon/src/server.rs` (request-scoped handlers in the dispatcher — NOT a `spawn_inbox_subsystems` subsystem)
- Modify: the daemon socket bind path (Unix: `set` socket file mode `0600` after bind; Windows named pipe: restrict the DACL to the creating user's SID) — enforces the ADR-0014 authz assumption that only the daemon's OS user can invoke verbs / read `McpLogs`
- Test: `crates/surge-daemon/tests/mcp_probe_ipc.rs`; `crates/surge-orchestrator/src/engine/ipc.rs` (`#[cfg(test)]` serde round-trip)

**Approach:**
- `McpProbe { name: Option<String> }`: resolve from `SurgeConfig.mcp_servers`; for each target build a transient `McpServerConnection` (`cwd: None`), `ensure_connected` + `list_tools(None)`, capture `McpHealth` + tool count, then `connection.shutdown().await` **in the same request** (no session retained). `name=None` ⇒ probe all (covers `surge mcp list`); `Some` ⇒ one (covers `surge mcp start`).
- `McpLogs { name, run_id: Option<RunId>, tail }`: resolve the **run-scoped** stderr file (`<worktree>/.surge/mcp-stderr/<server>.log` when `run_id` is `Some`; otherwise the daemon-probe-scoped path, and the response states which scope was read so a caller cannot silently get another run's stderr). No daemon ring buffer. `surge mcp logs` defaults `run_id` to the most recent run for that server and labels the source in output.
- Pure request handlers invoked from the `server.rs` `match req` dispatcher (the `_ => BadRequest` arm covers the transitional state); no `mcp_diag.rs` subsystem, no `main.rs` wiring, no `Arc<Mutex>` session map.
- `request_id()` is an exhaustive match with no wildcard → the two new variants MUST get arms (compilation gate). `DaemonResponse::request_id()` (`ipc.rs:~288`) is confirmed symmetrically exhaustive — add the paired response arms there too or `surge-daemon` won't compile.

**Patterns to follow:** `server.rs` run-scoped request handlers; `daemon.rs:102` facade; `ipc.rs` `#[non_exhaustive]` `#[serde(tag="method")]` additive variants (wire side) + the exhaustive-`request_id()` arm requirement.

**Test scenarios:**
- Happy path: `McpProbe{Some("fs")}` on a configured server returns `Healthy` + tool count; the transient connection is torn down (no leaked daemon session — assert via status/`tracing`).
- Edge case: `McpProbe{Some("unknown")}` ⇒ structured error, not a panic; `McpProbe{None}` returns all configured servers' health.
- Error path: a server whose command fails to spawn ⇒ probe returns the `McpError::StartFailed` reason; dispatcher stays alive.
- Edge case: `McpLogs{tail:N}` returns ≤ N lines, ≤ file capacity; empty when nothing logged.
- Edge case: `McpLogs` for server `fs` with `run_id=Some(A)` returns run A's stderr, not run B's, even when both ran `fs` (run-scoping — no cross-run leak); the response labels which scope it read.
- Integration: serde round-trip for both verbs (incl. the `run_id` field); `request_id()` / `DaemonResponse::request_id()` return the right id for each (regression for the exhaustive-match arms); socket bound with owner-only access (Unix mode `0600` / Windows pipe DACL) — assert the bind applies the restriction.

**Verification:** both verbs dispatch; no daemon-resident MCP session/ring buffer; `request_id()` (request + response) compiles with the new arms; `McpLogs` is run-scoped (no cross-run stderr leak); socket access restricted to the daemon's OS user; round-trip green.

---

### U8. `surge mcp` CLI command surface

**Goal:** `surge mcp list | start <name> | stop <name> | logs <name>` exist, mirror house CLI conventions, and route to the request-scoped daemon verbs.

**Requirements:** R5 (CLI half)

**Dependencies:** U7

**Files:**
- Create: `crates/surge-cli/src/commands/mcp.rs`
- Modify: `crates/surge-cli/src/commands/mod.rs`
- Modify: `crates/surge-cli/src/main.rs` (`Commands` enum arm, `run_command` arm, orphan-check exclusion)
- Test: `crates/surge-cli/src/commands/mcp.rs` (`#[cfg(test)]`)

**Approach:**
- `McpCommands { List { format }, Start { name }, Stop { name }, Logs { name, #[arg] run_id: Option<String>, #[arg] tail: Option<usize> } }`; `McpFormat { Text, Json }` (`default_value="text"`), mirroring `commands/doctor.rs`.
- Routing: `List` → `McpProbe{None}`; `Start{name}` → `McpProbe{Some(name)}`; `Logs{name,run_id,tail}` → `McpLogs` (the rendered output labels which run/scope the stderr came from); `Stop{name}` → **idempotent ack** rendered with a one-line note that under per-run isolation there is no persistent daemon-held child to stop, and that to halt MCP activity in a live run the operator aborts the run (Alternative A1 Option D). Connect via `DaemonEngineFacade::connect` (`daemon.rs:102`); render `Text` (aligned `println!`) / `Json` (`serde_json`); daemon-down ⇒ actionable message (mirror `daemon.rs`).
- Register in `main.rs:36-169`, `:323-499`, add to orphan-exclusion `:287-303`.
- R10 mapping: `surge mcp stop` is an explicit idempotent ack *by design under per-run isolation*, not a stub — record it that way in U10's traceability table (cite ADR-0014 / Alternative A1 Option D) so the milestone-close audit does not read it as an unimplemented deliverable.

**Patterns to follow:** `crates/surge-cli/src/commands/doctor.rs`, `crates/surge-cli/src/commands/daemon.rs`.

**Test scenarios:**
- Happy path: `surge mcp list --format json` parses, maps to `McpProbe{None}` (faked facade).
- Edge case: `surge mcp logs fs --tail 50` ⇒ `tail=Some(50)`; omitted ⇒ `None`.
- Edge case: `surge mcp stop fs` ⇒ idempotent success with the per-run-isolation note (no IPC error path).
- Error path: daemon socket absent ⇒ clear "daemon not running" message, non-zero exit, no panic.

**Verification:** all four subcommands parse and route correctly; `--format json` valid JSON; `stop` is a clean ack; daemon-down UX actionable; orphan prompt not triggered.

---

### U9. Real filesystem-MCP integration test

**Goal:** An integration test exercises a real `@modelcontextprotocol/server-filesystem` end-to-end, Windows-safe, opt-in.

**Requirements:** R9

**Dependencies:** U1 (builder spawn + `which-command` + cwd), U3 (shutdown assertion)

**Files:**
- Create: `crates/surge-mcp/tests/filesystem_mcp_e2e.rs`
- Modify: `docs/development.md`

**Approach:**
- `#[tokio::test] #[ignore = "requires npx + @modelcontextprotocol/server-filesystem; run with --ignored"]` + env-gate (`SURGE_MCP_REAL=1`) with a SKIPPED banner + early success when unset (mirror `real_acp_smoke.rs`).
- Resolve the launcher with `rmcp::transport::which_command("npx")` (Windows `.cmd`). Build an `McpServerRef`/`McpTransportConfig::stdio` for `npx -y @modelcontextprotocol/server-filesystem <tempdir>`; through `McpRegistry::from_config` (cwd = tempdir): `list_all_tools` returns its tools; call a read tool on a file the test wrote; assert content; `registry.shutdown().await`; assert idempotent + no orphan.
- Mirror `mcp_stdio_e2e.rs` temp/helper shape.

**Patterns to follow:** `crates/surge-mcp/tests/mcp_stdio_e2e.rs`; `crates/surge-orchestrator/tests/real_acp_smoke.rs`; `docs/development.md:39-58`.

**Test scenarios:**
- Happy path (gated): lists tools; read of a known temp file returns content; shutdown leaves no orphan.
- Edge case: env unset ⇒ SKIPPED + success (CI determinism).
- Error path: `npx`/package unavailable ⇒ clear `McpError::StartFailed`, bounded by `call_timeout` (no hang).

**Verification:** `cargo test -p surge-mcp -- --ignored` with env set passes on Windows; default `cargo test` SKIPPED + green; `docs/development.md` documents the run.

---

### U10. Docs + roadmap flip + acceptance sweep

**Goal:** Document the MCP lifecycle surface, flip the roadmap milestone to `[x]` with a dated Completed row, run the final acceptance sweep.

**Requirements:** R10

**Dependencies:** U1–U9, U11

**Files:**
- Create: `docs/mcp.md`
- Modify: `docs/cli.md`, `docs/README.md`, `docs/ARCHITECTURE.md`, `crates/surge-mcp/README.md`, `.ai-factory/ROADMAP.md`, `CLAUDE.md`

**Approach:**
- `docs/mcp.md`: `[[mcp_servers]]` config (incl. new `sandbox` and `redact_stderr`), lifecycle (spawn/health/restart/backoff/give-up→`EscalationRequested`/cockpit card), stderr-via-tracing + run-scoped file + redaction default-on, `surge mcp` reference (incl. `stop` semantics + `logs --run-id`), sandbox boundary + `FullAccess`/`WorkspaceNetwork` "operator-trusted only" caveat (ADR-0006 + ADR-0014), daemon-socket authz assumption (owner-only), per-run isolation + deferred shared-server note.
- `crates/surge-mcp/README.md` — **body, not just Status**: rewrite `Lifecycle` (structured `ServiceError`, backoff+cap+escalation, deterministic `shutdown`); delete/correct the string-heuristic paragraph (`:138-141`) and the `McpError::ServerCrashed` troubleshooting entry (`:179-182`); add `RestartExhausted`; add the `sandbox` row to the field-reference table.
- `.ai-factory/ROADMAP.md`: flip `[ ] MCP server lifecycle` → `[x]`; append an **inline deferral annotation** to the milestone line (mirroring the Telegram-cockpit and Tracker-tiers rows, which both carry their deferral caveats inline so the release train reads the caveat next to the green checkbox): note that `surge mcp stop` is an idempotent ack under per-run isolation (live-run halt is `surge run abort`), OS-level sandboxing of MCP children is delegated to the runtime per ADR-0006 (`FullAccess`/`WorkspaceNetwork` MCP binaries run unconstrained — operator-trusted only), and persistent shared servers are deferred to M9+ — cross-link ADR-0014. Add `| MCP server lifecycle | 2026-05-17 |` to `## Completed`. `docs/conventions/roadmap.md` is the artifact-contract framing.
- Acceptance sweep: `cargo build --workspace`, `cargo test`, `cargo clippy --workspace` clean; **complete bullet→U-ID traceability table covering all 13 roadmap sub-bullets** — including the two satisfied pre-existing (rmcp stdio child-process transport wired end-to-end; `surge.toml [[mcp_servers]]` registry-driven launch — both via PR #63, recorded as "verified pre-existing" with a smoke/cargo-test cite per R10's "implemented or explicitly verified pre-existing"), `surge mcp stop` recorded as "explicit idempotent ack by design (ADR-0014 / A1 Option D)", and R3→U11 / R1→U2+U3 (backoff/cap in U2, "then escalate" give-up surfacing in U3). Confirm `MAX_SUPPORTED_VERSION==3` + `IdentityV3` + replay-determinism tests green; confirm a `RestartExhausted`→`EscalationRequested`→cockpit-card path test exists.

**Patterns to follow:** existing ROADMAP Completed table/checkbox style; `docs/README.md` page table; `docs/conventions/adr.md` cross-links.

**Test scenarios:**
- Test expectation: none — docs/roadmap edits (no behavioral change). Acceptance is the workspace build/test/clippy sweep + the traceability table.

**Verification:** roadmap `[x]` + dated Completed row; `docs/mcp.md` exists and linked; no README paragraph describes the deleted heuristic or a non-existent variant; field table includes `sandbox`; full sweep clean; every roadmap sub-bullet traces to a unit.

---

### U11. Periodic health monitor

**Goal:** A periodic per-connection health probe marks a connection unhealthy after N consecutive failures and drives the U2 restart policy, bound to U3's cancellation seam so the task is never orphaned.

**Requirements:** R3

**Dependencies:** U2 (restart policy), U3 (`CancellationToken` seam + `McpHealth`/`statuses()`)

**Files:**
- Modify: `crates/surge-mcp/src/connection.rs` (`spawn_health_monitor`)
- Modify: `crates/surge-mcp/src/registry.rs` (start monitors on first use; bind to the registry token)
- Test: `crates/surge-mcp/src/connection.rs` (`#[cfg(test)]`)

**Approach:**
- `spawn_health_monitor(self: &Arc<McpServerConnection>, token: CancellationToken)`: `select! { _ = token.cancelled() => break, _ = interval.tick() => probe }`. Probe = `is_closed()` then `peer().list_tools(None)`; transport-class failure increments a consecutive-fail counter; at N (default 3) mark `Unhealthy` + invoke the U2 restart policy; reset on success.
- Started by the registry when a connection first reaches `Running`, using the registry `CancellationToken` from U3 — so the task always has a cancellation source (sequenced after U3; no orphan across commits).
- **Probe interval ≥ backoff cap** so the monitor can never become the hot-loop the U2 fast-return assumes a rate-limited caller prevents.

**Patterns to follow:** `CancellationToken` + `select!`-on-`cancelled()` idiom (real inbox/cockpit loops); U2 backoff; U3 `McpHealth`.

**Test scenarios:**
- Happy path: a healthy connection stays `Healthy` across probe ticks.
- Error path: N consecutive transport-class probe failures flip status to `Unhealthy` and trigger a restart attempt (deterministic via a fault-injected/seam-driven connection; the live mock-server variant is `#[ignore]` reusing the `mcp_stdio_e2e.rs` binary harness — no fake-transport seam exists, so the CI-deterministic coverage is the counter/threshold logic).
- Edge case: probe interval ≥ backoff cap — the monitor does not re-enter `ensure_connected` faster than `next_retry_at` (no monitor-driven hot-loop).
- Integration: registry shutdown (U3) cancels the monitor token; the task exits and does not outlive the run.

**Verification:** unhealthy after exactly N failures; restart driven; monitor cancelled by registry shutdown (no orphaned task); interval ≥ cap asserted.

---

## System-Wide Impact

- **Interaction graph:** new tracing targets (`mcp::child::stderr`, `mcp::supervisor`, `daemon::mcp`); new `ToolDispatcher::resolved_origin` trait method (default `None`, all existing dispatchers unaffected); `RestartExhausted → EscalationRequested` event append in the orchestrator (no new crate edge; consumed by the existing cockpit `CardKind::Escalation` path + notify multiplexer via the event tap); new `McpProbe`/`McpLogs` IPC verbs consumed by `surge mcp`; `RoutingToolDispatcher` gains a reservation pass; engine terminal path gains a time-bounded shutdown call; daemon socket bind gains an owner-only access restriction.
- **Error propagation:** structured `ServiceError` → `McpError` (incl. new `RestartExhausted`); give-up additionally surfaces as a replay-safe `EscalationRequested` event so AFK surfaces (cockpit/notify) see permanent MCP failure; `surge mcp` surfaces `McpError` reasons over IPC; daemon-down is an actionable CLI error.
- **State lifecycle risks:** U11's health task is bound to U3's registry token (sequenced after U3 — never orphaned); registry shutdown is time-bounded so a hung child can't wedge run completion; stderr forwarder terminates when the pipe closes; stderr file is run-scoped, redacted, capacity-bounded (ring/rotate, not silent truncate), deleted with the worktree.
- **Security boundary:** MCP child stderr is redacted before it reaches logs/file (secret-shape masking, default-on); child env is declared-keys + minimal-PATH only (no host-env leak); `McpLogs` is run-scoped (no cross-run stderr disclosure); the daemon control socket is owner-only (Unix `0600` / Windows pipe DACL); `FullAccess`/`WorkspaceNetwork` MCP children are unconstrained-by-design (ADR-0006) and emit an operator-visible WARN.
- **API surface parity:** `McpServerRef.sandbox` and `ToolCalled`/`ToolResultReceived.mcp_server` are serde-default `#[non_exhaustive]`-compatible additions; `ToolDispatcher::resolved_origin` has a default impl.
- **Integration coverage:** real filesystem-MCP test (gated) is the cross-process proof; mock + pure-fn tests cover the deterministic CI path; `engine_mcp_terminal_shutdown.rs` proves the engine↔registry teardown edge.
- **Unchanged invariants:** `discriminant_str` unchanged (variant-name keyed); no positional-destructure breaks; per-run MCP isolation unchanged; ReadOnly-denies-MCP behavior preserved; ACP-only transport (ADR-0006) unchanged. **Changed deliberately:** `MAX_SUPPORTED_VERSION` 2→3 with `IdentityV3` (per the documented additive-migration convention — old logs still decode).

---

## Risk Analysis & Mitigation

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| rmcp drop-without-cancel orphans MCP children | Med | High | U3 explicit `cancel().await` on terminal outcome, time-bounded; U9 asserts no orphan |
| U11 health task orphaned across the U2→U3 commit boundary | Low | High | U11 depends on U3 (token exists before the task is born) — the dependency edge *is* the mitigation; not bundled into U2 |
| Registry shutdown blocks run completion on a hung child | Low | High | Per-connection `cancel()` wrapped in `tokio::time::timeout`; force-abandon + WARN on expiry |
| Shipping new attributed events under schema v2 (silent convention violation) | Med | High | U4 bumps `MAX_SUPPORTED_VERSION` 2→3 + `IdentityV3` per the repo's documented v2 precedent; **both** hardcoded `max:2` assertions updated; legacy-decode + version-emit tests |
| Permanent MCP failure invisible to AFK operator (no cockpit card) | Med | High | Give-up appends a replay-safe `EscalationRequested` event (verified fold pass-through) the cockpit already renders as `CardKind::Escalation`; integration test asserts the append |
| MCP child stderr leaks secrets into surge logs/files | Med | Med | Default-on redaction pass before tracing/file emit; opt-out per server; run-scoped file deleted with worktree |
| `surge mcp logs` discloses another run's stderr | Low | Med | `McpLogs` run-scoped (`<worktree>/.surge/mcp-stderr/<server>.log`); response labels the scope read |
| `surge mcp stop` read as an unimplemented deliverable at milestone-close | Med | Med | A1 Option D names+rejects the live-run reading; U10 traceability records `stop` as explicit-ack-by-design; ROADMAP row carries the inline caveat |
| U1 signature change leaves workspace test targets uncompilable | Med | High | U1 Files enumerate all cross-crate `new`/`from_config` call sites; `cargo test`/`build --workspace` in U1 Verification |
| `mcp_server` attribution unreachable at emission site | Med | Med | New `ToolDispatcher::resolved_origin` trait method (default `None`); not a "read back" of the private routing table |
| `surge mcp` over-built into a mutable daemon subsystem | Med | Med | Resolved to request-scoped validate-drop (Alternative A1, ADR-0014); no subsystem/ring buffer |
| New `DaemonRequest` verbs don't compile (`request_id()` exhaustive) | Med | Low | U7 adds the `request_id()` arms explicitly as a compilation gate; serde round-trip test |
| Windows `npx` not resolvable ⇒ filesystem test flaky | Med | Med | `rmcp` `which-command` + `which_command("npx")` (U1/U9); env-gated SKIPPED default |
| Monitor becomes the hot-loop U2's fast-return assumes away | Low | Med | U11 probe interval ≥ backoff cap; fast-return does not reset `attempts` (U2 test) |
| Backoff/health constants wrong for real servers | Low | Low | Documented defaults; config knobs deferred unless a test demands |

---

## Documentation Plan

- New `docs/mcp.md`; `docs/cli.md` (`surge mcp`); `docs/README.md` page-table entry; `docs/ARCHITECTURE.md` + `crates/surge-mcp/README.md` (body rewrite, not skeleton); `docs/adr/0014-mcp-server-lifecycle.md`; `docs/development.md` (gated test); `.ai-factory/ROADMAP.md` flip + Completed row.

---

## Operational / Rollout Notes

- Logging: DEBUG/INFO/WARN/ERROR only (**no TRACE**); targets `mcp::child::stderr`, `mcp::supervisor`, `daemon::mcp`.
- Restart exhaustion = single ERROR `mcp::supervisor` line + one orchestrator-side `EscalationRequested` event append (replay-safe fold pass-through; rendered by the cockpit as `CardKind::Escalation`, fanned to notify channels via the event tap). Only the non-deterministic attempt count stays out of the log; the give-up fact is recorded.
- Security defaults: stderr redaction is **on by default** (opt-out per server via `redact_stderr=false`); child env is declared-keys + minimal-PATH; the daemon control socket is owner-only; `FullAccess`/`WorkspaceNetwork` MCP servers log a WARN — operators must only configure trusted binaries for those modes (ADR-0006 delegation is documented in `docs/mcp.md` + ADR-0014).
- Estimate calibration: apply the project's +50% buffer to raw effort; no estimates asserted here per ce-plan rules.
- Half-implementations forbidden: every unit is decide-or-defer; persistent shared-server mode, supervisor extraction, exit-code scraping deferred with rationale (Scope Boundaries); `surge mcp stop` is a deliberate explicit ack, not a stub.
- Default CI path stays mock-only/deterministic and green; the real filesystem-MCP test is opt-in (`#[ignore]` + env gate).
- `MAX_SUPPORTED_VERSION` 2→3 is a read-compatible bump (old logs decode); downstream readers keying on the wrapper version see v3 and know the attributed field is in scope.

---

## Sources & References

- Roadmap milestone: `.ai-factory/ROADMAP.md:155-169` (acceptance source of truth, 13 sub-bullets)
- Crate: `crates/surge-mcp/` (`connection.rs`, `registry.rs`, `error.rs`, `README.md`), `crates/surge-core/src/{mcp_config.rs,run_event.rs,sandbox.rs,config.rs}`, `crates/surge-core/src/migrations/mod.rs`
- Engine: `crates/surge-orchestrator/src/engine/{engine.rs,config.rs,tools/routing.rs,tools/mod.rs,stage/agent.rs,ipc.rs}`
- Replay/give-up: `crates/surge-core/src/run_state.rs:542` (`EscalationRequested` fold pass-through, catch-all `(state, _)`), `crates/surge-core/src/run_event.rs` (`EscalationRequested` payload)
- CLI/daemon/cockpit: `crates/surge-cli/src/commands/{doctor.rs,daemon.rs,mod.rs}`, `crates/surge-cli/src/main.rs`, `crates/surge-daemon/src/{main.rs,server.rs,lifecycle.rs}`, `crates/surge-telegram/.../cockpit/dispatch.rs:226` (`EscalationRequested → CardKind::Escalation` — the only give-up render path)
- Persistence: `crates/surge-persistence/src/runs/views.rs:360-361`; `crates/surge-notify` (event-tap consumer; not a `surge-mcp` dependency)
- Tests: `crates/surge-mcp/tests/mcp_stdio_e2e.rs`, `crates/surge-orchestrator/tests/{engine_m7_routing_dispatcher.rs,real_acp_smoke.rs}`, `crates/surge-daemon/tests/`
- Decisions: `docs/adr/0006-acp-only-transport.md`, `docs/adr/0013-tracker-automation-tiers.md`, `docs/ARCHITECTURE.md:150`, `docs/conventions/{roadmap.md,adr.md}`
- Related PRs: #63 (surge-mcp via surge.toml), #64 (Telegram cockpit — note the supervisor is doc-only)
- External: rmcp `>=1.6,<2.0` (`transport::child_process::TokioChildProcess` builder, `RunningService`, `ServiceError`, `which_command`)
