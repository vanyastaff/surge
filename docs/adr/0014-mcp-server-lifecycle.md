+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-05-17"
+++

# ADR 0014 — MCP server lifecycle: supervision, escalation, sandbox boundary, request-scoped operator surface

## Status

Accepted. Decision 4 is superseded for the future managed opt-in path by
[ADR-0020](0020-managed-mcp-recovery.md); the native implementation remains unchanged.

## Context

The `surge-mcp` crate connected to stdio MCP servers and routed their tools,
but lacked the liveness, supervision, observability, and operator-control
properties an AFK orchestrator needs. The "MCP server lifecycle" milestone
closes that gap. Several decisions cut across crates and are load-bearing
enough to record once here rather than re-deriving them per unit.

## Decision

1. **MCP supervision is a dedicated mechanism, not an engine `on_error` hook.**
   The retired `circuit_breaker.rs` / `retry.rs` were folded into engine
   `on_error` hooks, which route *stage-outcome* decisions — a different
   concern from *process liveness*. The exponential-backoff + capped-attempt
   restart policy lives in `McpServerConnection`. Restart attempt counts and
   retry timestamps are runtime-only and never enter the folded event log
   (replay determinism, ADR-0006). No reusable shared supervisor exists today
   (the cockpit's documented one is not implemented); extraction across
   inbox / cockpit / MCP is deferred to a separate refactor.

2. **Restart give-up is surfaced as an `EscalationRequested` event.** When the
   restart policy exhausts, the orchestrator (not `surge-mcp`) appends
   `EscalationRequested { stage: None, reason }`. `EscalationRequested` is a
   verified fold pass-through (`run_state.rs`), explicitly designed for
   out-of-band give-up surfacing, and is the only event the Telegram cockpit
   renders give-up from (`CardKind::Escalation`). A direct-`surge-notify`-only
   path was rejected because it is invisible to the cockpit and the event tap,
   defeating the AFK "walk away" value proposition. Only the stable give-up
   fact (`server`, reason) is recorded — the non-deterministic attempt count
   is in the prose message, not a folded field. No `surge-mcp → surge-notify`
   crate edge is introduced (it would pull `surge-intake`/`reqwest`/`lettre`
   into the near-leaf crate); the typed `McpError::RestartExhausted` is
   carried to the orchestrator via a `ToolDispatcher` trait-method seam.

3. **Surge-injected tools win on name collision.** `report_stage_outcome` and
   `request_human_input` are added by the Agent Client Protocol (ACP) bridge and dispatched upstream
   of the router. An MCP server advertising one of those names is dropped from
   the routing table and the declared catalog at a single canonical
   arbitration site (`RESERVED_INJECTED_TOOLS` in `routing.rs`), warning once
   per collision. Consistent with ADR-0006's uniform injected-tool surface.

4. **Sandbox is intent-plumbing within the ADR-0006 delegation boundary, not
   OS-level syscall sandboxing of arbitrary MCP binaries.** There is no common
   sandbox-flag grammar for arbitrary MCP server binaries (the sandbox matrix
   is keyed by ACP `RuntimeKind`, not generic child processes). `McpServerRef`
   gains an optional `sandbox` override resolved at one canonical site
   (`mcp_spawn_policy`): `ReadOnly` denies MCP entirely; the `#[non_exhaustive]`
   catch-all fails closed. Portable hygiene is applied at spawn (declared-env
   + minimal PATH only, child cwd pinned to the run worktree). Deeper
   enforcement is **delegated to the agent runtime per ADR-0006**;
   `FullAccess` / `WorkspaceNetwork` MCP children run unconstrained and emit a
   one-time operator-visibility WARN — operators must only configure trusted
   binaries for those modes.

5. **`surge mcp` is a request-scoped daemon validation surface, not a
   runtime-mutable subsystem.** Under per-run MCP isolation there is no
   persistent daemon-held MCP child. `surge mcp list|start` perform a
   transient spawn → handshake → `tools/list` → teardown via a request-scoped
   daemon IPC verb (no persistent sessions, no daemon-resident ring buffer, no
   new mutable subsystem). `surge mcp stop` is an explicit idempotent ack:
   under per-run isolation there is no persistent child to stop; to halt MCP
   activity in a live run the operator aborts the run (`surge` run-control),
   not `surge mcp stop`. `surge mcp logs` tails the bounded, redacted,
   run-scoped stderr file produced by the connection. Persistent shared
   servers (`McpServerRef::isolation = Shared`) remain deferred to a future
   milestone.

6. **Daemon-IPC authz assumption.** The daemon control socket is accessible
   only to the OS user that started the daemon (Unix: socket file mode `0600`
   at bind; Windows: named-pipe DACL restricted to the creating user's SID).
   No additional per-verb authz is applied, so `surge mcp logs` exposes
   captured stderr only to that user. Captured stderr is redacted (secret-shape
   masking, always on in v0.1) before it reaches `tracing` or the file.

7. **Startup has its own deadline, separate from `call_timeout`** (amended
   2026-10-05). The handshake wait includes process spawn and interpreter
   startup, which a per-RPC budget does not model: a Python stdio server with
   `call_timeout = 275ms` missed its handshake on a hosted macOS runner and the
   stage silently ran without its tools. `McpServerRef` gains an optional
   `startup_timeout` bounding spawn + `initialize`; `call_timeout` bounds only
   RPCs (`tools/call`, one deadline across `tools/list` pages, the health
   probe). The registry's caller budget for a tool call applies after the
   connection is running, so a reconnect is never cut short by it. A missed
   startup deadline is the distinct `McpError::StartupTimeout` and counts as a
   failed restart attempt. Unset resolves to `max(30s, call_timeout)`: never
   shorter than the previous shared deadline, so no existing configuration
   regresses.

   *Snapshot and hash contract.* The field is `#[serde(default,
   skip_serializing_if = "Option::is_none")]` on both `McpServerRef` and
   `OwnedFlowMcpManifestEntry`. Unset values serialize to the exact bytes
   written before the field existed, so frozen owned-flow server objects,
   envelopes and their HMACs from earlier runs keep verifying, and the
   manifest `schema_version` stays 1. An explicit value is part of the frozen
   public entry and the authenticated private object, so changing it after
   acceptance fails hydration like any other policy change. An older binary
   given a manifest with an explicit value rejects it (`deny_unknown_fields`
   on the entry, canonical-bytes mismatch on the server object). That is
   fail-closed, never a silent fallback to `call_timeout`. A resumed run that
   predates the field gets the new unset resolution, which is never shorter
   than its original deadline. Changing `DEFAULT_STARTUP_TIMEOUT` therefore
   changes resumed runs and must be recorded here.

8. **A selected server's catalog failure is escalated, not silently
   degraded** (amended 2026-10-05). When a stage selects a server and the
   sandbox policy allows it, the operator chose that capability. Losing it at
   session open used to be a single WARN, and one failing server also dropped
   every other selected server's tools. The catalog is now built per server
   (`McpRegistry::list_tools_per_server`). Healthy servers keep their tools.
   Each failing server produces `EscalationRequested { cause:
   McpSelectedCatalogUnavailable }` with the stage, server and opaque
   `McpError` text, and the stage proceeds. Failing the stage was rejected:
   the agent can often finish with engine tools, and the existing `on_error`
   routing is for stage outcomes, not tool availability (decision 1). The
   operator now sees the degradation and can stop the run. The new cause is a
   nested-enum variant, so the event payload schema is bumped to v16 (see
   `docs/schema-versioning.md`). Escalations are per stage start, not
   deduplicated across stages, which mirrors the per-dispatcher
   restart-exhaustion dedup.

## Alternatives Rejected

- **Persistent daemon `McpDiagnosticManager` subsystem** (HashMap of live
  sessions + ring buffer + 4 IPC verbs): introduces the codebase's first
  runtime-mutable persistent daemon subsystem for an AFK config-validation
  bullet; over-built; partly exists only to give the vestigial `stop`
  something to act on. Rejected for the request-scoped validate-drop.
- **Notify-only give-up** (direct `surge-notify`, no event): invisible to the
  cockpit and event tap. Rejected per decision 2.
- **Widen `call_timeout` to cover startup** (the PR 89 fixture repair): this
  keeps one knob for two different costs and makes every RPC wait as long as
  a cold `npx` start. Rejected for decision 7.
- **Fail the stage when a selected catalog is unavailable**: this turns a
  transient startup miss into a stage failure even when engine tools would
  do. Rejected for decision 8's escalate-and-proceed.
- **OS sandbox wrapper for arbitrary MCP binaries**: no common flag grammar;
  conflicts with ADR-0006's runtime-owns-enforcement boundary.

## Consequences

- Replay stays deterministic: only the give-up *fact* is event-sourced;
  health/restart telemetry flows through `tracing` + `surge mcp` status.
- AFK operators get a Telegram Escalation card on permanent MCP failure.
- Crate layering is preserved: `surge-mcp` stays a near-leaf (no notify edge).
- Schema version bumped to v3 for the additive `mcp_server` attribution field
  (per the v2 precedent — old logs decode cleanly).
- `surge mcp stop` does little by design; this is documented in `docs/mcp.md`
  and the roadmap Completed row so milestone-close does not read it as an
  unimplemented deliverable.

## Revisit Triggers

- Operators routinely abort runs after `mcp_selected_catalog_unavailable`.
  That would mean selection should be a hard stage precondition (fail
  closed) rather than escalate-and-proceed.

- A persistent shared-server mode (`McpServerRef::isolation = Shared`) lands —
  `surge mcp start/stop` then gain real long-lived semantics.
- A reusable daemon supervisor primitive is extracted (3rd consumer:
  inbox / cockpit / MCP).
- An MCP transport other than stdio is added.
