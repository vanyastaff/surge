# Competitive plan — September 2026

Status: decision note, written 2026-09-05. Third companion to
[`product-strategy.md`](product-strategy.md) (what we bet on) and
[`agent-os-landscape.md`](agent-os-landscape.md) (what everyone else is doing).
This page records *what changed since the 2026-07-07 survey and what we do about it*.

All star counts and repository facts below were checked via the GitHub API on
2026-09-05 and are quoted, not inferred. Vendor marketing claims are attributed
to their source and treated as claims, not verified behaviour.

---

## 1. What changed since 2026-07-07

### 1.1 A new runtime became the largest in the field: DeepSeek Harness

`deepseek-ai/deepseek-harness` — **213,087 ★**, MIT, TypeScript, developer
preview, active (pushed 2026-09-04). Built on the Cordis kernel with an
"everything is a plugin" architecture. Its own README warns:
*"THERE WILL BE COMPATIBILITY-BREAKING CHANGES."*

Its `packages/` tree is effectively a harness-grade feature catalogue. The
package-group READMEs describe:

| DSH package | What it claims to do | Relevance to Surge |
|---|---|---|
| `acp/` | *"automation-only server that exposes fresh harness agents to programmatic clients over JSON-RPC stdio"* — create/list/resume/close sessions, attach MCP servers, select model options, prompt, cancel, answer permissions, **"without a human in the loop"** | **DSH is drivable over ACP.** Direct opportunity |
| `subagent/subagent-acp` | out-of-process ACP *client* that spawns and drives ACP servers | DSH is also climbing into orchestration |
| `goal/` | one durable completion objective per session, surviving restart, resume, and fork; state in the session log | Runtime-level completion state — overlaps our ledger, but per-session only |
| `workflow/` | model-authored orchestration scripts fanning out subagents; `ralph` tool for fresh-agent iterative loops; *"containment, not a security boundary"* | Non-deterministic orchestration; no verification gate |
| `guard/` | repeat-tool-call reminder + per-call tool timeout policy | Loop hygiene as a first-class concern — **we have none** |
| `spill/` | tool output over a byte cap is written to an artifact; model sees a bounded preview plus a locator | Context-budget discipline — **we have none** |
| `skill/` | skill registry, providers (project/user dirs, bundled, remote), session catalogue, `/name` invocation | Standards alignment — **we have none** |
| `jobs/`, `plan/`, `todo/`, `schedule/`, `compaction/`, `session-query/`, `identity/`, `credentials/`, `sandbox/`, `e2b/`, `lsp/`, `runtime-diagnostics/` | supporting capability families | Feature-parity reference |

**Read for our strategy.** This confirms fact 2 of `product-strategy.md` from the
other direction: thin orchestration gets absorbed — now by *runtimes*, not only by
vendor manager views. DSH ships goal state, plan mode, todo, subagent fan-out and
scripted workflows inside the runtime. What it does **not** ship, by its own
documentation: a cross-run and cross-repo ledger, a verifier that owns the `done`
transition, an event log that is authoritative outside one session, or a
security boundary around fan-out. Our moat is intact — but the perimeter moved,
and "we orchestrate agents" is no longer a differentiator on its own.

### 1.2 Portable skills became a real cross-vendor standard

Two standards now appear across unrelated projects: **Agent Skills**
(agentskills.io) and **Agent Plugins** (agent-plugins.org, `plugin.json` +
`skills/`). Observed adoption:

- `K-Dense-AI/scientific-agent-skills` — 42,919 ★, 163 skills, arXiv:2609.00065,
  explicitly *"Compatible with Cursor, Claude Code, Codex, Pi, Antigravity, and
  the open Agent Skills standard"*.
- `tt-a1i/archify` — 49,302 ★, installed with `npx skills add tt-a1i/archify -g`,
  with an agent switcher for `cursor`, `codex`, `claude-code`, `opencode`, plus a
  DSH plugin build.
- `JetBrains/go-modern-guidelines` — 3,223 ★, shipped as a Junie extension, a
  Claude Code plugin marketplace, and via skills.sh.
- `THU-MAIC/OpenMAIC` (31,896 ★) and `every-app/open-seo` (17,251 ★) both ship
  `SKILL.md` packages as their primary integration surface.
- `NVIDIA/SkillSpector` — 16,277 ★, a security scanner for agent skills
  (prompt injection, data exfiltration, supply-chain risk).

**Read for our strategy.** Capability packaging has standardised around a format
we do not speak. Surge has profiles, flows and MCP servers, but a node cannot
bind a skill. Every runtime we drive already loads skills out of band, which
means capability enters our runs *unaudited and outside the event log* — an
integrity hole in the exact place we claim authority. SkillSpector's existence
is the proof that supply chain is the live risk in this format.

### 1.3 Project memory turned into an evidence discipline

`sulabhdubey/rta-smriti-brain` (34 ★, active) is the sharpest published take on
what our A3 pillar is supposed to be. Its README claims: bitemporal truth;
per-claim source path, hash, verification command, timestamp and verification
status; context packs under a hard token budget with *"explainable selection
receipts"*; an **Action Gate** returning `allow` / `warn` / `block` from policy,
readiness, git and freshness signals; canonical root binding that refuses silent
checkout switching and distinguishes clones from git worktrees; ingesting long
threads as *explicitly unverified* prior memory; recording helpful/harmful memory
outcomes and ageing only eligible unverified records.

`EfficientStreet/hindsight` (49 ★) is the write-side discipline in one skill:
review a whole session, extract **root causes rather than symptoms**, discard
one-off specifics, update an existing entry in place instead of near-duplicating,
say *"nothing to report"* on a clean session, and stay on-demand only
(`disable-model-invocation: true`) because writing memory is the user's call.

`Artistsyn/cortex_suite` (25 ★, **Rust**) splits structure from judgment:
`quartz-ctx` parses source live (Rust via `syn`, nine more languages via
tree-sitter) and `cortex` holds learned judgment in SQLite, ingesting the
extractor's output so the two cannot disagree. Two ideas transfer directly:
confidence as a three-way tag (`resolved` → `name_resolved` → `ast_only`) because
*"an agent cannot calibrate what it is told if everything arrives with the same
authority"*, and reporting the **unmatched halves** of a cross-boundary call graph
as the actual finding.

**Read for our strategy.** Our memory is `add` + `search` (verified: 
`crates/surge-cli/src/commands/memory.rs` exposes only those two). The strategy
document already promises provenance and `surge memory audit`; the field has
moved past that to per-claim verification status and gating actions on freshness.

### 1.4 Agent capacity became an operational limit, not a billing detail

`timharris707/modeldeck` (43 ★) exists solely to track *"usage limits across all
your Claude Code and Codex CLI accounts — live '% left' meters per rate-limit
window, reset countdowns, low-capacity alerts, and one-click account switching"*,
local-first, without touching credentials.

**Read for our strategy.** Surge enforces a **spend** budget (USD and tokens), frozen per run and
re-armed on resume (commit `1ed5caa`). It has no model of a provider's
*rate-limit window*. An AFK run that hits a five-hour window does not fail — it
stalls, silently, which is the failure mode our whole product exists to prevent.

### 1.5 Evidence artifacts got a visual, verifiable form

`tt-a1i/archify` compiles a typed JSON IR deterministically into self-contained
HTML/SVG, and — the part that matters to us — compares two validated snapshots as
**Before / Delta / After** with exact added, removed, changed, moved and rerouted
facts, refusing to invent topology. `Kayforkind/reimagine-it` (58 ★) ships a
decision report next to every generated artifact recording *"the selected
direction, candidate scores, seed, rationale, source anchors, and source-fidelity
checks"*, with the input never overwritten.

**Read for our strategy.** Both are our Run Report thesis in a different domain:
a reviewable artifact that carries its own provenance and can be checked without
reading the transcript. Neither is a dependency; both are proof the shape works.

### 1.6 Distribution lessons from the largest repositories in the batch

- `jingyaogong/minimind` — 58,755 ★ for a *teaching* repository ("train a 64M LLM
  in 2h"). Pedagogy is a distribution channel.
- `omacom/omarchy` — 38,282 ★ for an opinionated Linux configuration. Taste and
  first-run experience are the product.
- `bilawalsidhu/gods-eye-view` — 17,959 ★ for real data on a photorealistic globe.
  A cockpit that looks like a cockpit gets adopted.
- `JetBrains/go-modern-guidelines` names the mechanism precisely: agents write
  outdated code because of **training-data lag** and **frequency bias**, and an
  explicit version-aware guideline pack fixes both.
- `google-research/timesfm` (31,278 ★) — a pretrained time-series forecasting
  model. Noted and **rejected** for now: forecasting run cost and quota
  exhaustion from our own analytics tables needs quantiles, not a foundation model.

---

## 2. Verified gaps in Surge (checked in the tree, 2026-09-05)

| # | Gap | Evidence |
|---|---|---|
| G1 | DSH is not a known runtime | `crates/surge-core/src/runtime.rs:23` — `RuntimeKind` has 7 variants: claude-code, codex, gemini, cursor, copilot, opencode, goose |
| G2 | No skills support anywhere | no match for `Skill` in `crates/**/*.rs` |
| G3 | Memory is `add`/`search` only | `crates/surge-cli/src/commands/memory.rs:22` — `MemoryCommands` has two variants; no `audit`, no provenance surface |
| G4 | No Run Report type | no match for `RunReport` in `crates/**/*.rs`; promised in `product-strategy.md` §A2 |
| G5 | No loop hygiene | no repeat-tool detection or per-tool timeout policy at engine level |
| G6 | No provider capacity model | `BudgetLimits` (`crates/surge-core/src/budget.rs:31`) is `usd` + `tokens` only; no rate-limit window, reset time, wall-clock cap, or account rotation |
| G7 | No tool-output spill | oversized tool output is not diverted to the artifact store with a locator |
| G8 | No Rust guideline pack | nothing binds modern-Rust idiom guidance into node context |

Not gaps, for the record: ledger, `ready`, inbox, resolve, steer, fork, replay,
budget freeze/resume, crash recovery, MCP lifecycle, sandbox delegation matrix and
Telegram cockpit are all shipped and reachable from `surge --help`.

---

## 3. The plan

Ordering rule: each wave must either close a gap that lets a competitor's users
reach us, or harden a claim we already make. Nothing here contradicts
[ADR-0006](adr/0006-acp-only-transport.md) or the "build, don't bridge" principle.

### Wave 0 — DSH as a runtime (highest leverage per line of code)

DSH's `packages/acp` is an ACP server exposing **fresh** harness agents to
programmatic clients with no human in the loop. That is precisely our execution
model, so this is a registry entry, not an integration project.

1. Add `RuntimeKind::DeepSeekHarness` (`dsh`) with launch args, discovery and the
   sandbox-delegation row in `docs/sandbox-matrix.md`.
2. Extend the `surge doctor` agent smoke test and the per-runtime launch-arg
   coverage suite (the pattern from commit `ab6de54`) to DSH.
3. Record the *developer preview* status in the runtime table: DSH promises
   breaking changes, so pin the tested version and fail loudly on protocol drift
   rather than degrading silently.

Acceptance: `surge doctor matrix` lists DSH; a bundled flow runs end to end on
DSH in CI against a pinned version.

### Wave 1 — Skills as a typed, audited capability

Goal: a flow node may declare skills, and every skill that enters a run is
recorded in the event log with its source and integrity hash.

1. `SkillRef` in `surge-core` — name, provider (project dir, user dir, registry),
   version, content hash. Bound on a node exactly like a context binding, never
   loaded lazily mid-stage (same rule as `project.md`).
2. Resolve against the Agent Skills layout (`SKILL.md` + frontmatter) and the
   Agent Plugins package shape (`plugin.json` + `skills/`), so existing packs
   work unmodified.
3. Emit `SkillBound { name, provider, hash }` events; the Run Report lists every
   skill a run used.
4. Trust gate: skills are third-party executable context. Reuse the profile-trust
   machinery ([ADR-0002](adr/0002-profile-trust-deferred.md)) — an unpinned or
   unhashed skill requires explicit approval. SkillSpector's existence is the
   argument; we do not depend on it, we make the decision auditable.
5. `surge skill list|show|verify` for inspection.

Acceptance: a stock skill pack (`archify`, `go-modern-guidelines`) binds to a node
and its use is reconstructable from the event log alone.

### Wave 2 — Memory v2: evidence, not notes

Closes G3 and delivers the A3 pillar at the bar the field now sets.

1. **Per-claim provenance**: every memory entry carries source path, content hash,
   the command that verified it, a timestamp, and a verification status. Entries
   ingested from transcripts are stored as *explicitly unverified*.
2. **Confidence as a tag, not a boolean** — adopt cortex_suite's three-way scale so
   a node's context can order direct evidence ahead of low-trust recall.
3. `surge memory audit` — correlate entries with failed and looping runs, flag
   staleness against current file hashes, propose pruning. This is the promised
   command in §A3; it now also ages only *eligible unverified* entries, never
   verified ones.
4. **Write-back discipline** at run boundaries, following hindsight's rules:
   root cause over symptom, update in place over near-duplicate, silence on a
   clean run, and never on the agent's own initiative — a memory write is a node
   outcome, not a side effect.
5. **Context packs under a hard token budget**, with a receipt recording what was
   selected, what was dropped, and why. The receipt is a Run Report input.

Acceptance: a run that used memory can be replayed to show exactly which entries
entered which node's context, and why each was chosen.

### Wave 3 — The Run Report as a first-class artifact

Closes G4. This is the differentiator we already claim and have not shipped.

1. `RunReport` type in `surge-core`, compiled from the event log: nodes, outcomes,
   verifier verdicts, evidence artifacts, cost, skills bound, memory receipts,
   steers, approvals.
2. `surge run report <run_id> --format json|md|html`. The HTML form is a single
   self-contained file with no CDN — archify and reimagine-it both prove the
   format survives review and sharing.
3. **Verified vs unverified rendering everywhere** — a terminal success reached
   without a verifier node must be visually distinct in the report, the inbox and
   the ledger.
4. Optional PR attachment through the existing L3 merge gate.
5. Stretch, only after the above: a **structure delta** section — the typed
   before/after of the changed public surface, in archify's Before/Delta/After
   shape, generated from our own Rust parse rather than a dependency.

Acceptance: a reviewer can accept or reject a completed run from the report alone,
without opening the transcript.

### Wave 4 — Capacity-aware autonomy

Closes G5, G6, G7 — the three ways an AFK run dies quietly.

1. **Capacity model**: per-agent-account rate-limit window, remaining share, reset
   time. Populated from ACP usage signals where available
   (`agent-client-protocol` is already pinned with `unstable_session_usage`) and
   from observed 429s otherwise. Surfaced in `surge doctor` and the inbox.
2. **Scheduling policy**: before dispatching a node, refuse to start work that
   cannot finish inside the remaining window; park the run with a wake time
   instead of stalling. Optional rotation across configured accounts — Surge never
   copies or stores provider credentials, exactly as modeldeck refuses to.
3. **Loop guards**: repeat-tool-call detection and per-node wall-clock policy at
   the engine level, raising `EscalationRequested` (the MCP crash path already
   models this) rather than burning budget.
4. **Output spill**: tool output over a configured cap goes to the artifact store;
   the node sees a bounded preview and a locator. Our artifact store already
   exists; this is a policy plus a rewrite at the bridge.

Acceptance: a run that exhausts its provider window resumes automatically after
reset with its frozen budget intact, and the pause is visible in the inbox.

### Continuous

- **Modern Rust guidelines pack** (G8). JetBrains named the mechanism; Rust has
  the same problem, with a faster-moving stdlib and edition story. Ship it as a
  Surge skill pack bound by default on Rust archetypes, and publish it in the
  Agent Skills format so it is useful outside Surge. This is cheap and is the
  single highest-visibility thing we can give away.
- **Cockpit as a showpiece.** `surge-ui` is the adoption surface. gods-eye-view
  and omarchy are the evidence that a beautiful, opinionated surface with real
  data behind it is worth more than another feature.
- **Teaching repository.** minimind's 58k stars are for explanation, not novelty.
  A short, honest "how an event-sourced agent orchestrator works" write-up backed
  by our real code is our version of that.

---

## 4. Decisions taken here

1. **DSH is a runtime, not a bridge.** We drive it through ACP like every other
   runtime. We do not build against Cordis, and we do not depend on any DSH
   package. Consistent with ADR-0006.
2. **A DSH plugin that launches Surge is a distribution question, not an
   architecture question**, and is deferred. If taken, it must be a thin launcher
   over the `surge` CLI with no shared types, and it must not become the primary
   entry point.
3. **We adopt the Agent Skills format, not an agent's skill runtime.** We read the
   standard layout and bind it into our own typed node contract with hashes and
   events. Loading is ours; the format is theirs.
4. **No dependency on any project surveyed here.** cortex_suite is Rust and
   tempting; its ideas transfer, its code does not — a 25-star single-author crate
   in our critical path is the dependency risk `product-strategy.md` already
   rejected for Herdr and Pi.
5. **timesfm is out of scope.** Forecasting from our own analytics tables is a
   quantile problem, not a foundation-model problem.

## 5. Revisit triggers

- **DSH ships a durable, cross-session ledger with a verification gate.** This is
  the one that would actually threaten the moat. Watch `packages/goal` and
  `packages/workflow` for state that outlives a session and for a `done`
  transition an implementer cannot write.
- **DSH's ACP server stops being automation-only** or breaks compatibility, as its
  README promises it might — pin, test in CI, and treat as a runtime regression.
- **The Agent Skills standard fragments** (a second incompatible format reaches
  comparable adoption) — then bind skills behind our own `SkillRef` abstraction
  and support both, which Wave 1 is already shaped to allow.
- **A memory tool ships an Action Gate that agents actually respect.** If gating
  action on evidence freshness proves out elsewhere, promote it from a Wave 2
  receipt to a Surge policy node.
