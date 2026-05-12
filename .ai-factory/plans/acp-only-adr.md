# Plan — ACP-only transport ADR + reference updates

**Branch:** `feature/acp-only-adr`
**Base branch:** `main`
**Created:** 2026-05-11
**Refined:** 2026-05-11 (via `/aif-improve` — ADR structure aligned with project convention; AGENTS.md and README.md added to scope)
**Source:** `/aif-plan` (full mode), seeded from `.ai-factory/RESEARCH.md` Active Summary

## Description

Replace the bare declaration in `CLAUDE.md` ("ACP is the ONLY way to communicate with agents") with a normative ADR that records the decision, primary rationale, accepted costs with mitigations, explicit alternatives rejected, out-of-scope items, and revisit conditions. Add cross-references from every primary surface — AI-context summary (`.ai-factory/DESCRIPTION.md`), canonical architecture document (`docs/ARCHITECTURE.md`), project README (`README.md`), and agent-context file (`AGENTS.md`) — so future contributors can challenge the stance on its actual grounds, not on guesses.

This plan is **documentation-only**. No code changes. No tests. The mandatory docs checkpoint at completion routes through `/aif-docs` to confirm cross-document consistency.

Out of scope for this plan (tracked as follow-ups):

- Expanding the `Sandbox delegation matrix` milestone scope in `.ai-factory/ROADMAP.md` to the broader ACP top-tier set — owner: `/aif-roadmap`.
- Risk-register engineering work: pinning `agent-client-protocol` SDK rev in `Cargo.toml`, adding a `--trace-acp` CLI flag, capability cross-check in `surge-acp` — owner: separate `/aif-plan` runs once the ADR has landed.
- Extending the `surge init` PATH-scan list beyond `claude / codex / gemini-cli` — owner: separate `/aif-plan`.

## Settings

| Setting | Value |
|---|---|
| Testing | No — documentation-only change, no code to test |
| Logging | N/A — no code added |
| Docs policy | Yes — mandatory `/aif-docs` checkpoint at completion |
| Branch | Created: `feature/acp-only-adr` from `main` |

## Roadmap Linkage

**Milestone:** `Sandbox delegation matrix`

**Rationale:** This milestone is currently scoped to `{Claude Code, Codex CLI, Gemini CLI}` and is the only open roadmap item that concretely realizes the ACP-only commitment. ADR-0006 establishes the normative justification that the sandbox matrix work will reference and extend; the matrix scope can later widen to the full ACP top-tier set in a follow-up `/aif-roadmap` invocation.

## Research Context

> Carried over from `.ai-factory/RESEARCH.md` Active Summary, 2026-05-11 18:27. Sessions retained in RESEARCH.md.

**Topic:** ACP-only stance as Surge's sole agent-transport mechanism.

**Goal:** Lock in "ACP is the only way to communicate with agents" with an explicit, recorded rationale — replace the bare declaration in CLAUDE.md with an ADR that future contributors can challenge on its actual grounds.

**Constraints:**

- Subscription-based auth (Claude Pro/Max, ChatGPT Plus/Pro, Cursor Pro, etc.); no per-token API-key flows.
- Maintainability: user explicitly flagged "I don't want to chase parser updates for every agent — they release frequently." Headline rationale.
- Surge invariants that must hold: append-only event-sourced run log with deterministic fold; hooks (`pre_tool_use`, `post_tool_use`, `on_outcome`, `on_error`); injected tools (`report_stage_outcome`, `request_human_input`); sandbox elevation roundtrip; multi-channel HumanGate approvals.

**Decisions:**

1. ACP-only confirmed. Non-ACP (raw headless CLI wrapping) explicitly rejected as a parallel backend.
2. Primary rationale, ranked: (a) parser-maintenance avoidance, (b) subscription-CLI coverage by ACP is effectively complete in 2026, (c) structural fit with surge invariants, (d) auth-handshake standardization.
3. The non-ACP "long tail" (Aider, Plandex, Continue, Crush, RA.Aid, Devon, etc.) is **out of scope** — those are API-key-world tools that contradict the subscription constraint.

**Accepted costs of ACP (risk register, not blockers):**

- `!Send` futures from `agent-client-protocol` SDK → dedicated OS thread + single-threaded Tokio + `LocalSet` (already implemented in `surge-acp`).
- `unstable_session_usage` cargo feature → protocol churn risk; pin SDK revision; CI golden tests.
- Adapter quality variance (native vs adapter agents) → three-layer debug surface when something breaks; mitigation via capability cross-check at handshake + `--trace-acp` flag.

**Success signals:**

- ADR at `docs/adr/0006-acp-only-transport.md` capturing the decision, ranked rationale, alternatives rejected, three accepted costs with their mitigations, out-of-scope items, and revisit conditions.
- `CLAUDE.md`, `.ai-factory/DESCRIPTION.md`, `docs/ARCHITECTURE.md`, `README.md`, `AGENTS.md` reference ADR-0006 instead of asserting the stance independently.
- `grep` confirms no bare "ACP is the ONLY way" declarations remain in any primary surface.
- `/aif-docs` checkpoint green.

## Progress

- [x] Task 1: Draft ADR-0006
- [x] Task 2: Update CLAUDE.md
- [x] Task 3: Update .ai-factory/DESCRIPTION.md
- [x] Task 4: Update docs/ARCHITECTURE.md
- [x] Task 5: Cross-reference verification + docs gate
- [x] Task 6: Update README.md
- [x] Task 7: Update AGENTS.md

## Tasks

### Phase 1 — Draft normative document

#### Task 1: Draft ADR-0006: ACP-only agent transport

**File:** `docs/adr/0006-acp-only-transport.md` (new)

Follow the contract documented in `docs/conventions/adr.md` and enforced by `crates/surge-core/src/artifact_contract.rs:validate_adr_frontmatter`: **TOML frontmatter delimited by `+++`** with `status = "accepted"`, `deciders = ["vanyastaff"]`, `date = "2026-05-11"`. H1 title `# ADR 0006 — ACP-only agent transport`.

> Note: the 5 prior ADRs (`0001`–`0005`) use YAML `---` frontmatter and lack the required `## Status` section — they pre-date the artifact contract and are non-compliant. ADR-0006 is the first contract-compliant ADR. A separate cleanup pass should migrate the prior 5.

**Canonical section layout** (matches the artifact contract's required sections + project-specific extras):

1. `## Status` — short statement, e.g. `Accepted.` (required by contract)
2. `## Context` — the 2026 ACP landscape: ~33 agents registered at [github.com/agentclientprotocol/registry](https://github.com/agentclientprotocol/registry); the registry enforces standardized `authMethods` in the ACP handshake; every commercial-subscription coding CLI surge targets is ACP-conformant or in adapter form (Claude Code, Codex CLI, Gemini CLI, Cursor CLI, Copilot CLI in public preview, Junie, Augment/Auggie, Kimi, OpenCode, Goose). Reference surge's surrounding invariants: event-sourced run log with deterministic fold; declared hooks; injected tools; sandbox elevation roundtrip; multi-channel HumanGate approvals.
3. `## Decision` — ACP is the sole agent-transport mechanism in surge. Non-ACP fallback (raw headless CLI wrapping) is rejected as a parallel backend.
4. `## Rationale` — four ranked forces, in priority order:
   1. **Parser-maintenance avoidance** (primary). Agent CLIs release frequently; per-CLI stdout parsers would be a permanent treadmill. ACP gives a versioned contract that absorbs CLI changes upstream.
   2. **Subscription-CLI coverage.** Every commercial-subscription CLI surge cares about supports ACP in 2026.
   3. **Structural fit with surge invariants.** Event log, hooks, injected tools, sandbox elevation roundtrip all map to ACP primitives.
   4. **Auth-handshake standardization.** ACP registry requires agents to return valid `authMethods` — surge selects subscription/API-key via protocol, not per-CLI config-file logic.
5. `## Alternatives considered` (required by contract) —
   - **Non-ACP raw headless CLI wrapping.** Would require N stdout parsers per agent, no permission roundtrip (only preapprove-all flags), broken event-log determinism across CLI versions.
   - **Direct LLM API calls bypassing CLIs.** Forces API-key billing model; contradicts the subscription constraint that anchors the user-facing value proposition.
6. `## Consequences` — `surge-acp` remains the sole agent-side adapter crate; `surge init` PATH-scan will widen as ACP coverage grows; "Sandbox delegation matrix" milestone scope widens accordingly.

   `### Accepted costs and mitigations` (subsection) — three concrete costs each paired with a mitigation:
   - `!Send` futures from the SDK → dedicated OS thread + single-threaded Tokio + `LocalSet` (already implemented in `surge-acp`).
   - `unstable_session_usage` feature → pin the SDK rev in the workspace `Cargo.toml`; CI golden-file tests against real ACP agents.
   - Adapter quality variance (native vs adapter agents) → capability cross-check at handshake (surge declares expected capabilities per profile, fails fast on mismatch); `--trace-acp` flag for the debug surface.

7. `## Out of Scope` — the non-ACP long tail (Aider, Plandex, Continue, Crush, RA.Aid, Devon, etc.) is out of scope because those are API-key-world tools that contradict the subscription constraint.
8. `## Revisit conditions` — explicit triggers that would reopen this decision:
   - ACP specification stalls or stagnates without movement on capabilities/auth.
   - A dominant commercial-subscription coding CLI emerges that refuses ACP for >12 months.
   - Surge's primary user base shifts to API-key workflows (incompatible with current subscription-only stance).
   - Major breaking change in the SDK between point releases that the team cannot absorb.

**Footnote-style references** at the bottom of the file (matches `docs/ARCHITECTURE.md` convention):

```markdown
[acp]: https://agentclientprotocol.com
[registry]: https://github.com/agentclientprotocol/registry
```

Do not add a separate `## References` section — none of the existing ADRs do.

**Logging requirements:** N/A — markdown only.

**Acceptance:** ADR file exists at the canonical path; **TOML frontmatter (`+++`) parses with required keys `status`, `deciders`, `date`**; all eight sections present in order (Status / Context / Decision / Rationale / Alternatives considered / Consequences / Out of Scope / Revisit conditions); no broken external links; footnote-style references present; `surge artifact validate --kind adr docs/adr/0006-acp-only-transport.md` would pass.

---

### Phase 2 — Reference updates (parallel after Task 1 lands)

Tasks 2, 3, 4, 6, 7 depend only on Task 1 having a stable ADR file at the canonical path. They edit disjoint files and can be implemented in any order or in parallel.

#### Task 2: Update CLAUDE.md to reference ADR-0006

**File:** `CLAUDE.md` — replace the bare declaration around line 29.

**Before:**
```markdown
- ACP is the ONLY way to communicate with agents. No direct subprocess hacks.
```

**After:**
```markdown
- **Agent transport: ACP only.** No direct subprocess hacks or per-CLI stdout parsers. Rationale and accepted costs in [ADR-0006](docs/adr/0006-acp-only-transport.md).
```

Do not touch any other lines.

**Logging requirements:** N/A.

**Acceptance:** bare declaration gone; new line links to ADR-0006; `Grep "ACP is the ONLY"` in `CLAUDE.md` returns zero hits.

---

#### Task 3: Update .ai-factory/DESCRIPTION.md to reference ADR-0006

**File:** `.ai-factory/DESCRIPTION.md` — two surgical edits.

1. The Overview paragraph (around line 9): keep agent-agnostic framing but qualify the transport.

   **Before:**
   ```markdown
   Surge is **agent-agnostic** (speaks ACP to Claude Code, Codex, Gemini, or any conformant agent), **source-agnostic** ...
   ```

   **After:**
   ```markdown
   Surge is **agent-agnostic via ACP** (any ACP-conformant agent: Claude Code, Codex, Gemini, Cursor, Copilot, OpenCode, ...; see [ADR-0006](../docs/adr/0006-acp-only-transport.md)), **source-agnostic** ...
   ```

2. The ACP bridge bullet in Core Features (around line 13): append an ADR pointer.

   **Before:**
   ```markdown
   - **ACP bridge** to any conformant coding agent. The bridge runs on a dedicated OS thread with a single-threaded Tokio runtime (`!Send` futures from the SDK).
   ```

   **After:**
   ```markdown
   - **ACP bridge** to any conformant coding agent. The bridge runs on a dedicated OS thread with a single-threaded Tokio runtime (`!Send` futures from the SDK). See [ADR-0006](../docs/adr/0006-acp-only-transport.md) for the rationale behind ACP-only transport.
   ```

Do not change the Tech Stack entry (around line 32) or the Crate Layout row (around line 77) — they are factual statements about the dependency.

**Logging requirements:** N/A.

**Acceptance:** both lines updated as above; relative ADR link resolves; AI-context summary still readable as a one-screen project overview.

---

#### Task 4: Update docs/ARCHITECTURE.md section 5 to reference ADR-0006

**File:** `docs/ARCHITECTURE.md`, section heading `## 5. ACP bridge — agent integration`.

The section currently reads (right after the heading):
```markdown
The ACP bridge runs on a **dedicated OS thread** with its own single-threaded Tokio runtime + `LocalSet`, because the [Agent Client Protocol][acp] SDK uses `!Send` futures.

[acp]: https://agentclientprotocol.com
```

Insert a blockquote pointer **immediately after** the `[acp]: https://agentclientprotocol.com` line (that line is a unique anchor in the file). Result:
```markdown
The ACP bridge runs on a **dedicated OS thread** with its own single-threaded Tokio runtime + `LocalSet`, because the [Agent Client Protocol][acp] SDK uses `!Send` futures.

[acp]: https://agentclientprotocol.com

> Why ACP and only ACP — see [ADR-0006](adr/0006-acp-only-transport.md).
```

Do not rewrite the rest of section 5. Leave the differentiator-table row earlier in the file unchanged — it describes posture, the ADR provides rationale.

**Logging requirements:** N/A.

**Acceptance:** blockquote pointer present immediately after the `[acp]: ...` footnote definition in section 5; existing technical content untouched; relative ADR link resolves.

---

#### Task 6: Update README.md to reference ADR-0006

**File:** `README.md` — the declarative bullet around line 31.

**Before:**
```markdown
- **Agent-agnostic** — speaks ACP to Claude Code, Codex, Gemini, or any conformant agent.
```

**After:**
```markdown
- **Agent-agnostic via ACP** — works with any ACP-conformant agent: Claude Code, Codex, Gemini, Cursor, Copilot, OpenCode, and more. See [ADR-0006](docs/adr/0006-acp-only-transport.md) for the rationale.
```

The descriptive mentions at line 20 (`surge-acp` row in crates table) and line 58 (Architecture link row) remain factual and need no edits.

**Logging requirements:** N/A.

**Acceptance:** bullet replaced as above; `Grep "Agent-agnostic" README.md` returns the new line; ADR link resolves.

---

#### Task 7: Update AGENTS.md to reference ADR-0006

**File:** `AGENTS.md` — the introductory sentence around line 7.

**Before:**
```markdown
Surge is a local-first meta-orchestrator for AFK AI coding workflows in Rust. A run is a `flow.toml` workflow graph executed by a long-running daemon, with ACP-based agent integration, event sourcing, and Telegram-first approvals. See `.ai-factory/DESCRIPTION.md` for the full summary and `docs/ARCHITECTURE.md` for the canonical architecture.
```

**After:**
```markdown
Surge is a local-first meta-orchestrator for AFK AI coding workflows in Rust. A run is a `flow.toml` workflow graph executed by a long-running daemon, with ACP-based agent integration (see [ADR-0006](docs/adr/0006-acp-only-transport.md)), event sourcing, and Telegram-first approvals. See `.ai-factory/DESCRIPTION.md` for the full summary and `docs/ARCHITECTURE.md` for the canonical architecture.
```

Do not touch line 13 (Agent protocol entry), line 38 (crate-tree comment), or line 106 (Architecture link row) — those are factual statements.

**Logging requirements:** N/A.

**Acceptance:** intro sentence updated as above; relative ADR link resolves; rest of AGENTS.md untouched.

---

### Phase 3 — Verification

#### Task 5: Cross-reference verification + docs gate

**Blocked by:** Tasks 2, 3, 4, 6, 7.

Final cross-reference sweep, then hand off to `/aif-docs`:

1. **Grep — bare declaration is gone everywhere except RESEARCH.md.** Run:
   ```text
   Grep "ACP is the ONLY|ACP is the only|only way to communicate"
   ```
   Expected: only `.ai-factory/RESEARCH.md` hits (historical record). Zero hits in `CLAUDE.md`, `docs/`, `AGENTS.md`, `README.md`, `.ai-factory/{DESCRIPTION,ARCHITECTURE}.md`.

2. **Grep — ADR is referenced from every primary surface.** Run:
   ```text
   Grep "0006-acp-only-transport"
   ```
   Expected: at least 6 hits — the ADR file itself, `CLAUDE.md`, `.ai-factory/DESCRIPTION.md`, `docs/ARCHITECTURE.md`, `README.md`, `AGENTS.md`.

3. **Rendered ADR sanity check.** Read the ADR file end-to-end; confirm frontmatter parses, the seven section headings are present in order (Context, Decision, Rationale, Alternatives Rejected, Consequences, Out of Scope, Revisit conditions), footnote-style references at the bottom are valid links.

4. **Cross-document agent-list consistency.** The list of named ACP agents in DESCRIPTION.md, README.md, and the ADR Context section should match (Claude Code, Codex, Gemini, Cursor, Copilot, OpenCode at minimum). Spot-check by Grep.

5. **Mandatory docs checkpoint** (per `Docs: yes` setting). Hand off to `/aif-docs` for cross-document consistency checks. Since this plan already updates `README.md`, `AGENTS.md`, `CLAUDE.md`, `DESCRIPTION.md`, and both `ARCHITECTURE.md` copies, `/aif-docs` should mostly find nothing to fix. If it suggests additional doc files (e.g., `docs/getting-started.md`, `docs/workflow.md`, `docs/development.md`) need ADR pointers, those are reasonable to apply via `/aif-docs`'s patch flow, NOT in this plan.

**Deliverable:** clean grep output (point 1 zero hits in primary surfaces; point 2 ≥ 6 hits); working ADR link; seven canonical sections in ADR; cross-document agent list aligned; docs gate green.

**Logging requirements:** N/A.

## Task Graph

```text
   [1] Draft ADR-0006
        |
        +-- [2] Update CLAUDE.md            \
        |                                    \
        +-- [3] Update DESCRIPTION.md         \
        |                                      +--> [5] Verify + /aif-docs gate
        +-- [4] Update docs/ARCHITECTURE.md   /
        |                                    /
        +-- [6] Update README.md            /
        |                                  /
        +-- [7] Update AGENTS.md          /
```

Tasks 2/3/4/6/7 are independent (disjoint files) and can run in parallel after Task 1 lands.

## Commit Plan

Single conventional commit at the end (7 tasks, but all changes are tightly coupled documentation around one ADR — splitting would just be noise).

```text
docs: add ADR-0006 for ACP-only agent transport

Replaces the bare "ACP is the ONLY way" declaration in CLAUDE.md with
a normative ADR following project convention (Context / Decision /
Rationale / Alternatives Rejected / Consequences / Out of Scope /
Revisit conditions). Ranked rationale: parser-maintenance avoidance
(primary), subscription-CLI coverage, structural fit with surge
invariants, auth-handshake standardization. Three accepted costs paired
with mitigations: !Send (dedicated thread), unstable feature (pin SDK
rev), adapter variance (capability cross-check + --trace-acp).

Cross-references added from .ai-factory/DESCRIPTION.md,
docs/ARCHITECTURE.md, README.md, and AGENTS.md.

Source: .ai-factory/RESEARCH.md Active Summary 2026-05-11.
```

## Next Steps

After this plan is implemented and the docs gate is green:

- `/aif-roadmap` to widen `Sandbox delegation matrix` milestone scope to the broader ACP top-tier set (Cursor, Copilot, Junie, Augment, OpenCode, Goose).
- `/aif-plan` for the risk-register engineering: pin `agent-client-protocol` SDK rev in workspace `Cargo.toml`; add `--trace-acp` CLI flag; capability cross-check in `surge-acp` at session open.
- `/aif-plan` for extending `surge init` PATH-scan to include `cursor`, `copilot`, `opencode`, `goose`, `augment` binaries.

To start implementation:

```text
/aif-implement
```

To view tasks:

```text
TaskList
```
