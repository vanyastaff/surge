+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-10-05"
+++

# ADR 0021 — MCP cold recovery by group cleanup and restart

## Status

Accepted and implemented on 2026-10-05 (group stop, best-effort record,
restart); the interrupted-call notice of decision 4 remains open.
Supersedes [ADR-0020](0020-managed-mcp-recovery.md) in full. ADR-0014 (per-run,
supervised, sandbox-delegated MCP) is unchanged.

## Context

ADR-0020 made automatic cold recovery of host-launched MCP servers depend on
two host-issued proofs: an empty containment domain (on macOS, a managed VM)
and durable settlement of every external effect (a host-owned broker). Neither
exists, the host has no VM backend, and the ten MCP lifecycle acceptance cases
that encode positive recovery stay red. Until then the engine refuses every
cold resume of a run with an MCP writer: `mcp_cleanup_refusal`
(`surge-orchestrator::engine::writer_coverage`) rejects any `GroupOnly` MCP
writer, even when its recorded process group is gone, because MCP writers are
registered with `local_effects = false`.

Agent runtimes users already trust (Claude Code, Codex) run stdio MCP servers
as ordinary child processes, stop them on exit, start fresh ones on the next
session and do not prove external-effect settlement. A call cut off by a crash
is surfaced to the agent rather than replayed. The product decision of
2026-10-05 ([product strategy](../product-strategy.md#product-decisions--2026-10-05))
puts local, unattended completion of large projects first. A VM and a broker
are enterprise-grade guarantees whose cost (weeks of backend work, absent
infrastructure) blocks the first release while buying protection against a
rare event.

## Decision

1. **No managed execution domain and no effect broker.** Host-launched MCP keeps
   the native path of ADR-0014: own process group (`process_group(0)`),
   `GroupOnly` writer coverage, sandbox intent delegated. Coverage is never
   upgraded to `CoveredDomain` by this decision.
2. **Cold recovery cleans up, then restarts.** For each prior MCP writer of a run
   being resumed, the daemon:
   - validates the recorded container identity (boot-session-aware leader
     identity and group, as today);
   - if the group is still live, terminates the whole group (TERM, bounded
     grace, then KILL) and probes again;
   - when the group is gone, records a best-effort cleanup event for that writer
     that states the coverage it rests on (`GroupOnly`);
   - starts a fresh server from the frozen accepted manifest (authenticated
     command, arguments, environment and cwd) and resumes the run.
3. **Best-effort is named, never upgraded.** Events, reports and UI say
   "process group stopped" — never "writer closure confirmed". Descendants that
   left the group (`setsid`, double fork) are an accepted residual risk, as in
   other agent runtimes.
4. **An interrupted call is never replayed.** A tool call whose outcome is unknown
   at crash time is recorded as outcome-unknown. On resume the agent is told
   which server and tool were interrupted, when, and that it must check before
   retrying. By default no human gate is raised.
5. **Fail closed where identity or inputs are wrong.** Resume is still refused
   (with an Inbox attention item) when the frozen manifest or private inputs are
   missing, corrupt or changed; when ownership evidence conflicts or no process
   identity was established; and when the group survives termination.
6. **Runtime-launched MCP is untouched.** Servers an ACP agent launches from its
   own configuration remain the runtime's responsibility (ADR-0006).

## Alternatives rejected

- **Keep ADR-0020 (VM + broker) as a release gate.** Blocks the release on
  infrastructure that does not exist, for a guarantee no comparable product
  ships.
- **Ask a human after every MCP-related crash.** Safe, but parks overnight
  unattended runs on a rare event; contradicts the completion promise.
- **Resume automatically only for servers marked read-only.** Adds a setting
  users must understand, while the interrupted-call notice already covers the
  side-effecting case.

## Consequences

- Managed MCP no longer blocks the macOS release. Release stays NO-GO until the
  restart path is implemented and the rewritten acceptance cases pass.
- The ten original MCP acceptance cases are reviewed one by one in the
  [implementation plan](../plans/2026-10-05-001-feat-mcp-restart-recovery-plan.md).
  Exact transport, manifest and private-input oracles stay; requirements that
  only a containment domain could satisfy are replaced by restart-based
  expectations. No case is deleted or skipped without a recorded mapping.
- A non-idempotent call cut off by a crash can be repeated by the agent despite
  the notice (for example a duplicate issue comment).

## Revisit triggers

- Team or shared-machine deployments, where escaped descendants or duplicate
  effects affect other people.
- Reports of duplicate external effects after recovery.
- A supported, lightweight containment primitive on macOS that needs no VM.
