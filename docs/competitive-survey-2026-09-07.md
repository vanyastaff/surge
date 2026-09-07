# Competitive survey — 7 September 2026

Status: research note. Fourth companion to [`product-strategy.md`](product-strategy.md)
(what we bet on), [`agent-os-landscape.md`](agent-os-landscape.md) (the July survey of
runtimes and cloud agents) and [`competitive-plan-2026-09.md`](competitive-plan-2026-09.md)
(the 5 September decision note).

This page exists because the two earlier surveys have a shared blind spot. The July note
concluded the orchestrator layer was dying (Vibe Kanban shut down, Crystal deprecated,
Terragon gone) and the 5 September note surveyed runtimes and skill packs. Neither looked
at the *desktop orchestrator layer* again. It did not die. It re-formed, and the largest
member of the new cohort is now bigger than every project either note examined except DSH.

**Method.** GitHub REST API (`gh`), Exa search, and direct page fetches, all on
2026-09-07. Firecrawl is not wired into this environment; page fetches went through
WebFetch and Exa instead. Star counts and repository dates are API-quoted, not inferred.
Vendor marketing claims are attributed and treated as claims. Two findings below are
*negative* results from direct repository inspection and are labelled as such.

---

## 1. The correction: the orchestrator layer re-formed at ten times the scale

The July survey's roster is now mostly dead. Verified `pushed_at` on 2026-09-07:

| July-survey project | Stars | Last push | Read |
|---|---|---|---|
| `BloopAI/vibe-kanban` | 28,032 | 2026-04-24 | abandoned, as reported |
| `stravu/crystal` | 3,115 | 2026-02-26 | abandoned, as reported |
| `devflowinc/uzi` | 582 | 2025-06-04 | abandoned |
| `humanlayer/humanlayer` | 11,479 | 2026-06-19 | stalled mid-rebuild |
| `smtg-ai/claude-squad` | 8,443 | 2026-08-20 | alive, slowing |
| `dagger/container-use` | 4,037 | 2026-09-07 | alive |
| `imbue-ai/sculptor` | 230 | 2026-09-04 | alive, tiny |

The conclusion drawn from that ("no proven business model, category consolidating
violently") was correct about those projects and wrong about the category. A second
cohort formed between October 2025 and March 2026 and is now an order of magnitude
larger:

| Project | Stars | Created | Lang | License | What it calls itself |
|---|---|---|---|---|---|
| `stablyai/orca` | **63,485** | 2026-03-17 | TS | MIT | "ADE for working with a fleet of parallel agents" |
| `pingdotgg/t3code` | 21,968 | 2026-02-08 | TS | MIT | "agent harness control surface" |
| `getpaseo/paseo` | 16,358 | 2025-10-13 | TS | AGPL-3.0 | "the control plane for coding agents" |
| `superset-sh/superset` | 13,872 | 2025-10-21 | TS | Elastic-2.0 | "agentic IDE to orchestrate 100+ coding agents" |
| `omnigent-ai/omnigent` | 9,767 | 2026-06-11 | Python | Apache-2.0 | "meta-harness" (Databricks) |
| `Emanuele-web04/synara` | 1,721 | 2026-03-27 | TS | MIT | "focused workspace for coding agents" |
| `coollabsio/jean` | 1,261 | 2026-01-23 | TS/Rust | Apache-2.0 | "dev environment for AI agents" |

All seven were pushed to on 2026-09-07. Orca went from creation to 63k stars in under six
months — faster than anything in the July roster ever moved. The "orchestrator graveyard"
framing in `agent-os-landscape.md` §"2026 convergence" needs amending: the graveyard is
real, but it is a *generation*, not the category.

**Read for our strategy.** Two of the July note's conclusions survive intact — the visual
language is generic (all seven ship the same dark sidebar-plus-diff layout) and nobody has
a business model beyond "free, bring your own subscription" (Superset states it plainly:
*"The desktop app is free forever… Anything we charge for will be an optional service on
top."*). One conclusion does not survive: "orchestrators monetize poorly, therefore the
category is dying." The category is not dying; it is being funded as distribution.

### 1.1 What the cohort actually ships

Per-product, from READMEs and product docs fetched 2026-09-07. Table stakes across all
seven — worktree per task, diff review with inline comments, integrated terminal, per-
workspace dev server and port, agent-agnostic launcher, PR creation — are not repeated.

- **Orca** (Stably, YC-backed per its own site). 29 named agents plus "any CLI agent."
  Signature features beyond table stakes: **SSH worktrees** (run agents on a remote box
  with full local file editing, git and terminals, auto-reconnect and port forwarding);
  **Design Mode** (a real Chromium window per worktree — click any UI element and its
  HTML, CSS and a cropped screenshot go into the agent's prompt); an **`orca` CLI that
  agents drive themselves** (`worktree create`, `snapshot`, `click`, `fill` — computer
  use as a scriptable surface); **account switcher with usage tracking**; iOS and Android
  companions paired through a relay whose source is in-repo under `cloud/`.
- **Superset** (source-available, ELv2). The most machine-drivable product in the cohort:
  desktop app, single-binary CLI, TypeScript SDK, **MCP server**, OpenAPI 3.1 description,
  an A2A agent card, RFC 9727 API catalogue, `llms.txt` for docs/API/blog, and an official
  skill pack (`npx skills add superset-sh/skills`) so agents can drive Superset itself.
  **Automations** (cron-scheduled agent runs) are the most mature in the cohort.
  `.superset/config.json` declares per-workspace `setup`/`teardown`/`run` scripts.
  Built-in chat with **inline tool approvals and plan review**.
- **Paseo** (AGPL-3.0, solo maintainer). **Daemon-first**: a local daemon owns the
  agents; desktop, web, mobile and CLI are all clients of it. End-to-end encrypted relay
  for pairing, Docker image, `paseo run --host workstation.local:6767` to drive a remote
  daemon. Claims no telemetry and no forced login. Full mobile feature parity, not a
  read-only companion.
- **T3 Code** (Ping Labs). Explicit non-business-model (*"Nothing. We built T3 Code
  because we wanted the best possible development experience with agents"*), mid-
  conversation model switching, per-thread branches, draft/stacked/amend PR tooling,
  documented multi-account support for Codex and Claude, background-service mode.
  Notably: *"We are (mostly) not accepting contributions yet."*
- **Omnigent** (Databricks, Apache-2.0, open-sourced 2026-07-06). Covered in §3.
- **Synara** (fork of T3 Code). Signature: **provider handoffs** — move an in-progress
  task from Claude Code to Codex keeping full context. MCP-native in both directions.
- **Jean** (Coolify's team). Execution modes **Plan / Build / Yolo with plan-approval
  flows**; "Magic Commands" (investigate issue/PR/workflow, code review *with finding
  tracking*, AI commit messages, merge-conflict resolution, release notes); a headless
  `jean-server` that serves the same UI over HTTP+WebSocket with token auth.

---

## 2. Two negative findings, both material

### 2.1 Nobody in the leading cohort speaks ACP

GitHub code search across the five largest orchestrators, 2026-09-07:

| Repo | `agent-client-protocol` hits | `node-pty` hits |
|---|---|---|
| `stablyai/orca` | **0** | 446 |
| `superset-sh/superset` | **0** | 49 |
| `getpaseo/paseo` | **0** | 11 |
| `coollabsio/jean` | **0** | 0 (Rust/Tauri PTY) |
| `pingdotgg/t3code` | 1 | 18 |

The whole cohort wraps a pseudo-terminal and scrapes it. That is why Orca can claim 29
agents, Superset "100+", and Paseo "34+", while Surge supports seven `RuntimeKind`
variants. **Reach is where ACP-only costs us, and the gap is roughly an order of
magnitude.** It is not a small marketing gap; it is the first thing a user checks.

The counter-evidence is real and worth putting next to it. The harness-engineering study
(arXiv:2609.00006, source-diffed eleven harnesses in April and again in July 2026) reports
**ACP now ships in six of the eleven harnesses**, and that it has acquired a third role
beyond client and agent — *harness hosting*. OpenHands runs Codex CLI as an interchangeable
ACP backend. So ACP is winning *inside* the runtimes while the orchestrator cohort ignores
it, because PTY-scraping needs no cooperation from the runtime and reaches everything
today.

**Decision this forces.** Not "abandon ACP" — ADR-0006's reasoning holds, and a scraped
terminal cannot produce the typed events our event log, ledger and Run Report are built
on. But "we support seven runtimes" is a weak answer to "it runs any CLI agent," and the
honest framing is a trade we should state rather than hide: *we support fewer runtimes and
we can prove what happened inside each run; they support everything and can prove nothing.*
Two concrete moves are available and both are cheap:

1. **Publish the runtime matrix as a feature, not a limitation** — for each supported
   runtime, what typed signal we get (usage, permission requests, tool calls, plan
   updates) that a PTY wrapper cannot get. `surge doctor matrix` already holds the data.
2. **Consider a non-authoritative PTY fallback runtime** — explicitly second-class,
   marked `unverified` everywhere in the ledger, inbox and Run Report, and refused by
   any flow with a verifier node. This buys reach without diluting the authority claim,
   and the "verified vs unverified rendering everywhere" rule from Wave 3 already exists
   to carry it. **This is a real ADR, not a task** — it touches ADR-0006 directly.

### 2.2 Orca's capacity feature is a read-out, not a scheduler

This matters because Wave 4 is in flight on this branch. Orca's own documentation, fetched
2026-09-07:

- It shows consumption against the active account's plan, reset times for "5-hour, daily,
  weekly, and Claude Fable weekly windows (where applicable)", and warns at 80% of any
  limit.
- The data source is *the local usage state each agent already writes to disk* under
  `~/.claude`, `~/.codex` and equivalents — no API calls, no extra authentication.
- Account switching is **manual**. The docs contain no automatic switching, no pausing of
  work, and no scheduling behaviour based on remaining capacity.

Two things follow, and they point in opposite directions:

**Steal the data source.** `crates/surge-core/src/capacity.rs` currently builds a
`CapacityWindow` from observed 429s and whatever ACP `unstable_session_usage` gives us,
which is why `remaining` is in practice only ever `None` or `EXHAUSTED` and why reset time
is usually unknown. Reading the runtimes' own on-disk usage bookkeeping is a third source
that costs no credentials and no requests, and it is the only one that gives a *reset time
before the first 429*. It fits `CapacitySource` as a new variant. This directly attacks
the "one 429 parks a runtime forever" failure mode: a row that expires on an observed
reset instead of a guess.

**Keep the scheduler — it is the differentiator.** Everything the cohort ships is a
dashboard reading. Wave 4's actual claim — refuse to dispatch a node that cannot finish
inside the remaining window, park the run with a wake time, resume automatically with the
frozen budget intact, and show the pause in the inbox — is unoccupied. Nothing found in
this survey does it. That acceptance criterion is the thing to defend in review.

---

## 3. Three layers that appeared since July

### 3.1 The integration spine — our largest unclaimed gap

A whole tool class formed around one problem: worktrees do not remove merge conflicts,
they postpone them to integration time. The numbers being cited are specific. AgenticFlict
(arXiv:2604.03551, 107K+ agent PRs) measures a **27.67% merge-conflict rate on
agent-authored PRs against 10–20% for human ones**, and on co-active pairs splits it
**19.8% intra-agent vs 41.7% cross-agent** — agents have no horizontal awareness of each
other. A separate EMSE result puts badly-resolved conflicts at up to ~26× the bug density
of ordinary code.

Three implementations, all local-first, all built in the last two months:

- **`yongjip/mergetrain`** (MIT, created 2026-06-28). The most rigorous. A local merge
  queue with four stated guarantees: **exact validated train identity** (the train you
  approved is the train that ships, byte for byte), a **lease-fenced single runner**, an
  **atomic multi-ref push**, and **crash-safe exactly-once deploys** — after a crash,
  recovery reconciles the local queue *against the remote*, so a landed train is never
  re-pushed and a lost one is never mislabelled as shipped. Gates run once over the
  assembled train; on failure the runner **bisects and names the conflicting pair**
  (`conflict_with`) rather than shipping around the breakage. Every state is JSON with an
  explicit `next_action` so an agent can follow the result instead of inferring it. Its
  fault-injection matrix SIGKILLs a real `git push --atomic` with the remote's refs
  applied and without.
- **`alwh1te/agent-semaphore`** (MIT, created 2026-08-18). Attacks the same problem
  earlier: **intent-carrying leases** on path scopes (TTL, renewal by activity, monotonic
  fencing epochs, mandatory `reason`, atomic all-or-nothing acquisition in canonical path
  order so deadlocks are impossible by construction); a `PreToolUse` hook that denies a
  write into another agent's scope with **deny text written for the model** — naming the
  holder, its intent, its branch and the exact next call; and a **radar** that compares
  *dirty* worktrees pairwise with `git merge-tree --write-tree`, building the tree twice
  and reporting `UNSTABLE` if the two builds disagree. Its own honest finding is the best
  part: *"Conflicts are removed by the catch-up, not by the claim"* — a claim serializes
  writing, it does not hand you the peer's result, so releasing a scope must **wake the
  waiters and name the branch to rebase onto**.
- **`funador/claude-code-merge-queue`** (MIT, 125★). The simple version: numbered lanes,
  a pre-push hook that makes `land` non-optional, one configured `checkCommand` as the
  only gate, and a deliberately visible emergency hatch. Its stated non-goal is worth
  quoting: *"No human reviews any of this before it lands."*

**Where Surge stands.** Verified in the tree: `crates/surge-daemon/src/automation_merge_gate.rs`
delegates to `TaskSource::check_merge_readiness`, which is GitHub PR checks plus approving
reviews. That is a **per-branch, PR-first gate**. Nothing in `crates/` detects that two
concurrent runs touched overlapping paths, and nothing validates the *combined* state of
several finished runs before they land. For a product whose entire premise is parallel
worktree-isolated runs, that is the sharpest gap this survey found — sharper than skills or
Run Report, because it is a correctness gap, not a visibility gap. Two runs can each be
green, each pass their verifier, each merge cleanly, and turn `main` red, and Surge's event
log will faithfully record that everything succeeded.

**Why Surge can do this better than any of the three.** They are bolt-ons that discover
scopes by hook or by lease. Surge already knows, before dispatch, which flow node touches
what — and it has a durable event log, a ledger, crash recovery and a verifier concept the
merge-queue tools have to invent from scratch. mergetrain had to build write-ahead markers
and pin refs to survive a laptop dying mid-push; `surge-daemon::recovery` already scans
non-terminal runs at startup off the event log. The train gate is a *node kind*, and its
verdict is a Run Report section.

### 3.2 The meta-harness, and the one idea worth taking outright

**Omnigent** (Databricks, Apache-2.0, open-sourced 2026-07-06, 9,767★) is the closest
architectural competitor found in this survey — closer than anything in the July roster.
Agents are declared in YAML as *prompt + executor harness + tools*, so swapping which
harness or model runs an agent is a one-line edit. Policies ("stateful spend caps, model
routing, and risk-based escalation") and sandboxing apply at the meta level across all
harnesses; it lists nine sandbox backends (Modal, Daytona, Islo, E2B, CoreWeave,
Kubernetes, OpenShell, Boxlite, Databricks). Credentials come from first-party keys,
Claude Pro/Max and ChatGPT subscriptions via the official CLIs, OpenAI-compatible gateways,
or a Databricks workspace.

That is our sentence — *"policy at the layer above the harness"* — shipped by a company
with a distribution channel, six weeks before this note. What it does not have: a durable
cross-run ledger, an event log authoritative outside one session, or a verifier that owns
the `done` transition. Same moat conclusion as DSH in the 5 September note, from a
different direction, and it is now the second independent confirmation.

**The idea to steal is Polly.** Omnigent bundles an orchestrator agent that writes no code
itself: she plans, delegates to coding sub-agents in parallel git worktrees, and then
**routes each diff to a reviewer from a different vendor than the one that wrote it** —
Claude Code writes, a Codex- or Pi-backed reviewer reads, and they swap on the next task.
Model heterogeneity used as a check on any single model's systematic blind spots.

This is the single most valuable thing in this survey for Surge, because it converts our
weakest pillar into our strongest. `agent-os-landscape.md` and the July analysis both rate
agent-agnosticism as timely but leaky and not uniquely ownable. **Cross-vendor verification
is the one capability that is only possible if you are agent-agnostic**, and it lands on
machinery we already have: a verifier node with a `RuntimeKind` distinct from the
implementer node's, enforced by a validation rule, recorded in the event log, and rendered
in the Run Report as *"written by X, verified by Y."* A single-vendor orchestrator
structurally cannot make that claim. Neither can a PTY wrapper make it *checkable*.

Concretely: a flow-graph validation rule that a verifier node's runtime must differ from
the runtime of the node it verifies (warn by default, error under a strict profile), plus
a Run Report line. The engine already supports per-node runtime selection; the twenty-one
validation rules from commit `12a7474` are the place this rule goes.

### 3.3 Platform absorption is no longer a risk, it is a schedule

R1 in the July analysis was "platform absorption (biggest)". Since then, verified from
vendor documentation and release notes:

- **Anthropic Managed Agents** (hosted, public beta since April 2026) is architecturally
  our shape: a **session log**, a swappable **harness**, and a **sandbox**, deliberately
  decoupled — Anthropic's own engineering post frames it as *"a meta-harness… unopinionated
  about the specific harness."* Multiagent orchestration ships a coordinator with a roster
  of up to 20 agents and 25 concurrent context-isolated threads, an **advisor** consulted
  mid-turn, per-thread event streams, lifecycle webhooks, and a memory-store API on its own
  beta header (`agent-memory-2026-07-22`). Tool-permission requests from a subagent are
  **cross-posted to the primary thread** with the originating thread id — a decision inbox,
  built in.
- **Two capabilities announced there are ours by name.** *Outcomes* defines a measurable
  success condition for a hosted agent — that is a verifier owning `done`. *Dreaming* runs
  test scenarios against past data before executing live.
- **Claude Code** shipped **Agent View (`claude agents`)**, a unified dashboard over all
  active local sessions; `/goal`; background-first subagents that commit, push and open a
  draft PR; **Dynamic Workflows** (model-authored orchestration fanning to hundreds of
  subagents, checkpointed, results checked before folding); and Agent Teams behind
  `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` with `TeammateIdle` / `TaskCreated` /
  `TaskCompleted` hooks and teammates that can be **required to plan in read-only mode
  until the lead approves**.
- **Codex CLI** v0.149.0+ ships a `codex agents` dashboard over a **thread-tree** where
  every sub-session has a traceable parent, plus `codex queue` to deliver a message to any
  node by UUID or name.
- **Claude Code 2.1.259–2.1.263** (2–6 September 2026) is an unattended-operation release
  train: `--permission-prompts none` so a headless run **denies instead of hanging**,
  `managedMcpServers` for central MCP config, `/skill-doctor` reporting which skills cost
  context without earning it, and output caps raised to 128K via `bashOutputMaxChars` /
  `taskOutputMaxChars`.

**Read for our strategy.** The rebuttal in the July note — "platform owners are
disincentivized to route to rivals" — still holds and is now the *whole* argument. Every
single-vendor manager view has to stop at its own vendor's boundary. Two of the four items
above are worth mirroring immediately because they are cheap and they harden the AFK claim:
a headless deny-instead-of-hang policy is exactly our loop-guard escalation path, and
`/skill-doctor`'s question ("which skills burn context for nothing") is answerable *better*
from our event log than from a single session's transcript — we can measure a skill's cost
against outcomes across every run that bound it, which is a Wave 1 + Wave 2 join nobody
else can compute.

---

## 4. Take, in priority order

Each item names the Surge surface it lands on and why our version can be better rather
than merely equivalent.

| # | Take | From | Lands on | Why ours is better |
|---|---|---|---|---|
| 1 | **Cross-vendor verification**: verifier node's runtime must differ from the implementer's | Omnigent Polly | `engine/validate.rs` rule + Run Report line | Only possible because we are agent-agnostic; only *checkable* because we have typed events. Converts the weakest pillar into the strongest |
| 2 | **Integration spine**: combined-state gate over concurrently finished runs, with bisection naming the conflicting pair | mergetrain, agent-semaphore | new node kind + merge gate; Run Report section | We know node→path intent before dispatch; we already have the event log, ledger and crash recovery they had to reinvent |
| 3 | **On-disk runtime usage as a capacity source** (`~/.claude`, `~/.codex`) | Orca | `CapacitySource` variant in `capacity.rs` | Gives a reset time *before* the first 429, which fixes the never-expiring capacity row. They stop at display; we schedule on it |
| 4 | **Deny-instead-of-hang under headless policy** | Claude Code 2.1.259 | loop guards / `EscalationRequested` | A denial is a result the ledger can record and the inbox can rank; a hang is the exact AFK failure we exist to prevent |
| 5 | **Skill cost accounting** — which bound skills cost context without changing outcomes | Claude Code `/skill-doctor` | Wave 1 `SkillBound` × Wave 2 receipts | They measure one session; we can measure across every run that bound the skill, against verifier verdicts |
| 6 | **Machine-drivable product surface**: CLI + SDK + MCP server + OpenAPI + `llms.txt` + an official skill pack | Superset | `surge` CLI is already the substrate | Our CLI is already the API; what is missing is the MCP server and the published skill pack — and *our* MCP tools can return typed run state, not screen-scraped text |
| 7 | **Daemon-first topology**: one daemon owns the runs, every UI is a client | Paseo, Jean `jean-server` | `surge-daemon` already is this | We are one HTTP/WS surface away from phone and web reaching the same runs; the daemon, recovery and event log already exist |
| 8 | **Release-the-scope-and-name-the-branch** notification | agent-semaphore | inbox / steer | Their hard-won finding: a claim serializes writing but does not deliver the peer's result. Our inbox is the right place to say "rebase onto X, it landed" |
| 9 | **Plan/Build/Yolo modes with a plan-approval gate**, and read-only planning until approved | Jean, Claude Agent Teams | `Planned→Executing` human gate | Already our FSM's cheapest checkpoint; what is missing is that the *mode* is a first-class per-run setting rather than policy buried in a profile |
| 10 | **Cross-posting a subagent's permission request to the primary stream** with its origin id | Managed Agents | inbox ranking | This is literally our Inbox thesis, shipped by a vendor inside one session. Ours spans runs, repos and vendors |

### What not to take

- **PTY-scraping to inflate the runtime count.** It would make every downstream claim
  ("reconstructable from the event log alone") false for the scraped runtimes. If reach is
  needed, it goes behind an explicitly `unverified` second-class runtime and an ADR — see
  §2.1 — never silently.
- **Mobile apps and a pairing relay.** Four of the cohort ship them; it is a large,
  permanent surface area for a solo-maintained project. The daemon HTTP/WS surface (take
  #7) gets most of the value from a phone browser over Tailscale, which is exactly what
  Jean documents.
- **Kanban stages.** Vibe Kanban's own co-founder said ~50% of columns were redundant, and
  the new cohort dropped them — all seven use a status-grouped list, not a board.
- **A dependency on any project surveyed here.** Unchanged from the 5 September note. The
  merge-queue tools in particular are single-author and weeks old.
- **Design Mode / computer-use in the orchestrator.** Genuinely clever (Orca), genuinely
  outside our thesis, and enormous.

---

## 5. Revisit triggers

- **Any cohort member ships a durable cross-run ledger or a verification gate.** Superset
  is the one to watch — it already has the CLI, SDK, MCP server and OpenAPI surface that
  such a thing would hang off. Jean's "code review with finding tracking" is the nearest
  gesture today.
- **Omnigent adds an event log that outlives a session, or a policy that owns `done`.**
  Its policy engine is already stateful and risk-based; that is one step away.
- **mergetrain or agent-semaphore gets adopted by one of the big four orchestrators.**
  That would make the integration spine table stakes and close take #2's window.
- **Anthropic's Managed Agents opens to third-party harnesses**, or *Outcomes* becomes
  callable against a non-Anthropic runtime. That is the version of platform absorption
  that reaches our moat rather than stopping at the vendor boundary.
- **ACP adoption stalls below the six-of-eleven line** reported in arXiv:2609.00006 while
  the PTY cohort keeps growing. That is when the §2.1 fallback ADR stops being optional.

---

# Part II — the runtime layer, and the projects that grew fastest

Added the same day, after a second pass aimed at the question "what gained thousands of
stars in a week." All figures API-quoted 2026-09-07.

## 6. Growth is happening in the runtime layer, not the orchestrator layer

Repositories created since 2026-06-01 that passed 2,000 stars, filtered to agent
infrastructure:

| Repo | Stars | Created | Lang | What it is |
|---|---|---|---|---|
| `deepseek-ai/deepseek-harness` | **215,102** | 2026-08-13 | TS | DSH — "everything is a plugin" |
| `DietrichGebert/ponytail` | 130,854 | 2026-06-12 | JS | a *prompt pack*: "think like the laziest senior dev in the room" |
| `earendil-works/pi` | 102,743 | 2025-08-09 | TS | Pi agent harness (toolkit + CLI) |
| `can1357/oh-my-pi` | 30,003 | 2025-12-31 | TS | Pi fork "with the IDE wired in" |
| `xai-org/grok-build` | 26,546 | 2026-07-14 | **Rust** | SpaceXAI's harness + TUI, **speaks ACP** |
| `anywhere-labs/dsh-desktop` | 24,229 | 2026-08-13 | TS | third-party DSH desktop |
| `andrewyng/openworker` | 17,428 | 2026-07-20 | Python | — |
| `langchain-ai/openwiki` | 16,199 | 2026-06-22 | TS | agent-written, self-maintaining codebase wiki |
| `awesome-dsh-plugin` | 14,782 | 2026-08-13 | — | curated DSH plugin list |
| `yc-software/qm` | 14,672 | 2026-07-29 | TS | "multiplayer agent harness for work" |
| `lidge-jun/opencodex` | 13,800 | 2026-06-18 | TS | universal provider proxy for Codex & Claude Code |
| `XiaomiMiMo/MiMo-Code` | 12,961 | 2026-06-10 | TS | Xiaomi's harness |
| `openai/codex-security` | 10,558 | 2026-07-13 | TS | find / **validate** / fix security findings |
| `omnigent-ai/omnigent` | 9,767 | 2026-06-11 | Python | Databricks meta-harness (Part I §3.2) |
| `cloudflare/computer` | 9,117 | 2026-06-05 | TS | "give your agent a computer" |
| `shadcn/improve` | 9,091 | 2026-06-10 | — | audit skill that writes plans for cheaper models |
| `unicity-aos/aos-ce` | 8,550 | 2026-07-12 | **Rust** | "open agent operating system" |
| `CopilotKit/OpenBot` | 4,425 | 2026-08-17 | TS | AI coworkers, one computer each |
| `dmmulroy/anti-slop` | 4,157 | 2026-08-12 | TS | lint rules rejecting low-evidence code |
| `yetone/cumora` | 3,518 | 2026-08-17 | TS | team chat where agents are first-class teammates |
| `vercel-labs/fx` | 2,790 | 2026-08-11 | **Zig** | "Unix like coding agent" |

The shape of this list is the finding. **Not one of the top entries is an orchestrator.**
They are runtimes, prompt/skill packs, and single-purpose verification or context tools.
The orchestrator cohort of Part I grew large by mid-2026 and has stopped being where new
mass forms. Two distribution lessons repeat, and they are the ones `competitive-plan-2026-09.md`
already spotted with minimind and omarchy:

- `DietrichGebert/ponytail` at **130,854 stars is a prompt pack**, not software. So is
  `shadcn/improve` at 9k. The unit of distribution in this field is now a *skill or
  guideline pack*, not a binary. That is the strongest available argument for the
  "Modern Rust guidelines pack" currently sitting in `Вне рамок` as a §Continuous item.
- Three of the top five entries are **Chinese-ecosystem plays** around DSH. The English
  survey in Part I misses them entirely.

## 7. DSH: the plugin ecosystem is the story, not the harness

`competitive-plan-2026-09.md` recorded DSH at 213,087★ on 5 September and read its
`packages/` tree as a feature catalogue. Two things it did not record:

**The growth curve.** DSH was created **2026-08-13**. It reached 215,102 stars and
**25,364 forks in twenty-five days**, with releases every one to three days
(`dsh-v0.1.2-rc.1` on 03-09, `-alpha.1` on 04-09, `-alpha.2` on 07-09) — still on `0.1.x`,
still alpha. Nothing else in this survey has that curve.

**A third-party ecosystem formed in days, not months.** Within 24 hours of the DSH repo
appearing, independent projects were shipping into it, and several are now larger than
every orchestrator in Part I except Orca:

| Repo | Stars | Created | What |
|---|---|---|---|
| `anywhere-labs/dsh-desktop` | 24,229 | 2026-08-13 | desktop shell where "the desktop itself is a plugin" |
| `awesome-dsh-plugin` | 14,782 | 2026-08-13 | curated plugin index |
| `yjh051108/dsh-routing-suite` | 7,125 | 2026-08-14 | injector + router-standard kit |
| `zhu1090093659/dsh-web` | 7,104 | 2026-08-12 | web plugin aggregation |
| `dataelement/dsh-desktop` | 4,434 | 2026-08-13 | a *second* desktop |
| `xiaobright/dsh-anchored-standard` | 3,822 | 2026-08-14 | two-phase preset (minimal bootstrap → full standard) |
| `omdsh-dev/DSH-better-sidebar` | 3,408 | 2026-08-07 | sidebar base with third-party page registration |
| `dsh-market/dsh-market` | 3,370 | 2026-08-14 | in-harness plugin market, one-click install |
| `ccch1mneyyy/dsh-TUI` | 2,865 | 2026-08-13 | TUI plugin |

**Read for our strategy.** "Everything is a plugin" produced a marketplace, two competing
desktops and a routing standard in under a month. That is what an extension point with no
gatekeeper does. It also means DSH's plugin surface is now the fastest-moving supply-chain
risk in the field — thousands of third-party plugins, no signing story visible, entering
runs as executable context. Our Wave 1 trust gate is aimed at exactly this, and the DSH
ecosystem is the concrete argument for it that the ticket currently lacks.

The current `packages/` tree is also considerably larger than the one recorded on
5 September. Present today: `acp api attachment boot bundle client code-runtime compaction
context core credentials e2b experimental extensions feedback fs goal guard hooks host
identity interaction jobs llm lsp mcp plan preset runtime-diagnostics sandbox schedule sdk
session-query session settings shell skill spill storage subagent subprocess terminal
test-support todo typert util web webhook workflow workspace`. New since that note, and
worth a second look each: **`hooks`**, **`webhook`**, **`sdk`**, **`context`**, **`bundle`**,
**`preset`**, **`extensions`**, **`storage`**. `acp` is still there, so Wave 0's premise
holds.

## 8. Pi: 102k stars, and ACP-only cannot reach it

`agent-os-landscape.md` has a Pi section written in July. The repository has moved and so
has the org — it is now `earendil-works/pi`, **102,743★**, MIT, pushed daily. It is not one
agent; it is a harness *toolkit* split into publishable packages:

| Package | What it is |
|---|---|
| `pi-ai` | unified multi-provider LLM API |
| `pi-agent-core` | agent runtime with tool calling and state management |
| `pi-coding-agent` | the interactive CLI |
| `pi-tui` | terminal UI with differential rendering |
| `chord` | application-composition runtime: services, replicated state, RPC, plugins |
| `pi-telemetry` | **vendor-neutral telemetry contracts, reference adapter, conformance tests, typed schemas** |
| `evals` | **behavioral, model-backed checks** for Pi workflows |

**The blocking finding: Pi does not speak ACP.** Verified by code search, not by reading
marketing — `agent-client-protocol` returns **0 hits** in the repository, and its
`packages/protocol` README describes something else entirely: *"Runtime-neutral routed
envelopes, CBOR encoding, and byte-stream framing for the experimental Pi protocol.
Protocol version 8"*, with payload semantics owned by Chord. It is a competing wire
protocol, not an ACP profile.

So the second-largest open harness in the field is **unreachable under ADR-0006**, and
`oh-my-pi` (30,003★) inherits that. Superset, Orca and Paseo all list Pi as supported —
because they scrape a PTY and do not care what protocol it speaks. This is the §2.1 reach
gap with a name and a star count attached, and it is the strongest single piece of
evidence for opening that ADR.

Two further details worth carrying into our own work:

- **Pi ships no permission system at all.** Its README states it *"does not include a
  built-in permission system for restricting filesystem, process, network, or credential
  access"* and runs with the launching user's permissions, pointing instead at three
  containerisation patterns (a Gondolin micro-VM, plain Docker, OpenShell). If Pi is ever
  added to `docs/sandbox-matrix.md`, its row is **"delegates nothing — Surge must supply
  the boundary"**, which no current row says.
- **Its supply-chain policy is the best in the survey** and is directly transferable to
  our skill trust gate: exact-pinned direct dependencies, `min-release-age=2` in `.npmrc`
  so a same-day dependency release cannot be resolved, lockfile as ground truth with a
  pre-commit block, `--ignore-scripts` on every install path, and scheduled
  `npm audit signatures`. **A minimum-age rule is a cheap, high-value trust signal we do
  not have**: an unpinned skill whose content hash changed *today* is a different risk
  from one that has been stable for a month, and Wave 1 currently treats them identically.

## 9. grok-build: a Rust ACP runtime that should be in the matrix

`xai-org/grok-build` — 26,546★, **Rust**, created 2026-07-14, pushed 2026-09-07, synced
from SpaceXAI's monorepo with a `SOURCE_REV` file recording the upstream commit. Its README
states it runs *"interactively, headlessly for scripting/CI, or embedded in editors via the
Agent Client Protocol (ACP)."* Code search returns **199 path matches for `acp`**, so this
is not a marketing line.

That makes it the cheapest available runtime addition after DSH: same shape as Wave 0 —
a `RuntimeKind` variant, launch args, discovery, a sandbox-matrix row, a `doctor` smoke
test and a launch-arg coverage case. `RuntimeKind` currently has eight variants
(`crates/surge-core/src/runtime.rs:23`) after DSH landed; this is the ninth, and unlike
Pi it costs no ADR.

## 10. Two governance patterns worth copying outright

**`yc-software/qm`** (14,672★) — "a multiplayer agent harness for work", Slack and web.
It is not a competitor to Surge (its domain is company-wide assistant work, not code
delivery), but its **scope model** is the best-designed thing found in this survey:
each person and each room gets its own scoped memory, files, keychain view, permissions,
crons, web apps and durable sandbox. And its skills model is a governance model, not a
loader: *"Skills are scope-owned and shareable by grant, with admin-gated promotion to the
whole org and skill packs imported from git repositories."* It also states the same
agnostic bet we make, from the harness side: *"Pick your own harness and model and switch
between them — Pi, OpenCode, Codex, and Claude Code all drive the same core."*

Our Wave 1 trust gate answers "may this skill run at all." qm answers "who owns it, who
may grant it, and what promotes it to everyone" — the question after ours, and the one a
team-scale deployment asks first.

**`unicity-aos/aos-ce`** (8,550★, Rust) — "the open agent operating system", built from
capsules and distributions with a `Unicity Audit` surface. Its release integrity is the
part to steal: every release publishes checksums, **Sigstore bundles, GitHub
build-provenance attestations**, and a `runtime-compatibility.toml` pinning the exact
runtime release and WIT commit — and a tag cannot publish unless both a machine-readable
runtime-compatibility gate and an upgrade/self-heal gate are true, the latter approved only
after the candidate boots against a frozen clone of a previous home. That is the discipline
our bundled flows, `versions.toml` pins and (soon) published skill packs should aim at,
and it is a far better model than "pin a version string and hope."

## 11. The verification and context cohort — closest to our thesis

Four separate projects converged on ideas Waves 2 and 3 are building:

- **`langchain-ai/openwiki`** (16,199★) ships **Grounded Claims**: *"material facts in
  code wikis now carry versioned source evidence. When that evidence changes or
  disappears, OpenWiki knows exactly which propositions need to be confirmed, rewritten,
  or retired."* That is Wave 2's per-claim provenance and staleness audit, shipped by
  LangChain, in a neighbouring domain. Two mechanics beyond ours: a **resumable page-job
  architecture** (`begin → submit_plan → next_page → submit_page → … → finish`) with a
  durable ordered queue, per-page source checkpoints and **partial-progress PRs for
  ephemeral CI**; and output as **OKF v0.2**, a portable Open Knowledge Format bundle
  carrying *deterministic generation provenance and validated trust and lifecycle
  metadata*. If a portable format for provenance-carrying claims is becoming a standard,
  our memory store should know the format exists before it freezes its schema.
- **`shadcn/improve`** (9,091★) — an Agent Skill that audits a codebase and writes
  implementation plans, and **never implements anything**: *"use your most capable model
  for the part where intelligence compounds… and hand execution to cheaper models. The
  plan is the product."* Its verb list is a workflow we already have the graph for:
  `plan`, `review-plan` (critique and tighten an existing plan), `execute` (dispatch a
  cheaper executor, **review its work**), `reconcile` (verify, unblock, retire the
  backlog). Together with Omnigent's Polly this is one coherent pattern —
  **heterogeneous node assignment**: expensive model plans, cheap model executes,
  different-vendor model verifies. Our flow graph already assigns runtime per node; what
  is missing is that anyone ever says this is *why* you would.
- **`dmmulroy/anti-slop`** (4,157★) — Oxlint rules that *"reject low-evidence and
  low-signal patterns"*, distributed as an agent skill (`npx skills add`) and explicitly
  **meant to be vendored, not depended on**: *"There is no official npm package. Copy the
  rules into your repository."* Two confirmations for us: skills are the install channel,
  and vendoring-over-dependency is the field's own answer to the risk our
  no-dependency decision names. It is also the sharpest statement yet of what a guidelines
  pack is *for* — not idiom, but refusing code the model cannot justify.
- **`openai/codex-security`** (10,558★) — CLI plus SDK to *find, **validate**, and fix*
  security findings, with a run shaped by explicit budget knobs (`mode: deep`, `workers`,
  `subagents`, **`stopAfterNoNew`**, `maxDiscoveryRuns`, **`maxTimeHours`**) and a
  `reportPath` artifact. `stopAfterNoNew` is a loop guard expressed as *diminishing
  returns* rather than repetition — a third guard shape next to Wave 4's repeat-tool-call
  detection and wall-clock cap, and a better one for search-shaped nodes.
- **Pi's `evals` package** — behavioural, model-backed checks that run a real
  `AgentSession` in isolated temporary project and agent directories, attach native
  session artifacts, and exist *"to compare prompts, tools, skills, models, or other
  harness configurations."* This is the missing half of Wave 1: we will be able to prove
  a skill **entered** a run, and still have no way to say whether binding it **helped**.

## 12. Amendments to the take list in §4

Part I's ten items stand. Five additions and one reprioritisation:

| # | Take | From | Lands on |
|---|---|---|---|
| 11 | **`grok-build` as the ninth runtime** — Rust, 26.5k★, native ACP, no ADR needed | grok-build | `RuntimeKind`, sandbox matrix, doctor smoke test |
| 12 | **Minimum-age trust signal for skills** — content that changed today is not content that has been stable a month | Pi `.npmrc` `min-release-age` | Wave 1 trust gate |
| 13 | **`stopAfterNoNew` as a third loop-guard shape** — stop on diminishing returns, not only on repetition or wall clock | codex-security | Wave 4 guards |
| 14 | **Heterogeneous node assignment as a named pattern** — expensive plans, cheap executes, different vendor verifies | Omnigent Polly + shadcn/improve | flow graph docs + `engine/validate.rs` rule from take #1 |
| 15 | **Scope-owned skills with grants and admin promotion** | qm | after Wave 1; the question a team asks the day after the trust gate |
| — | **Reprioritise: the Rust guidelines pack out of §Continuous.** Two of the five largest new repositories in this field are prompt/skill packs (ponytail 130k, improve 9k). It is our cheapest distribution act and it is currently parked | ponytail, improve, anti-slop | `Вне рамок` R48 |

And one item that must **not** be taken quietly: Pi cannot be reached over ACP. Adding it
is an ADR about ADR-0006, not a runtime ticket — see §2.1 and §8.

---

# Part III — the DSH plugin ecosystem

Part II established that DSH grew a third-party ecosystem in days. This part looks inside
it, because what 3,363 people chose to build is better evidence about demand than any
product's feature list.

**Method and its limits, stated up front.** The catalogue
(`awesome-dsh-plugin/awesome-dsh-plugin`, 14,782★) publishes structured data next to its
README: `data/plugins/*.yml` (one metadata file per plugin), `stars.json`, `downloads.json`
and `added-dates.json`. Its live badge endpoint reported **3,363 plugins** on 2026-09-07.
The measurement files are smaller than that and **stale**: 1,488 star records, 624 download
records, 447 added-dates, all stamped `checkedAt: 2026-08-19`. The category buckets below
are my own classification **by repository name keyword**, not the catalogue's, so treat
them as shape, not census. The metadata-schema finding in §15 is from a random sample of
40 files (seed 7) and is exact for that sample.

## 13. What people actually built

Bucketing the 1,488 measured plugins by name keyword, with total recorded downloads:

| n | downloads | bucket |
|---:|---:|---|
| 85 | 149,019 | UI / theme / shell |
| **74** | 12,696 | **usage / quota / cost** |
| 64 | 36,275 | notification / remote / mobile |
| 57 | 32,769 | memory / context |
| 46 | 15,050 | verification / review |
| 45 | 12,591 | session / history |
| 40 | 9,195 | security / permission |
| 36 | 9,919 | skills |
| 36 | 30,685 | model / provider / router |
| **32** | **4,421** | **git / PR / diff / merge / worktree** |
| 19 | 13,272 | subagent / teams / orchestration |
| **2** | 3,804 | **resume / unattended autonomy** |

Four readings, in descending order of how much they should change what we do.

**(a) Capacity is the most-attempted, least-solved problem in the ecosystem.** Usage,
quota and cost is the **second-largest bucket by plugin count (74)** and one of the
smallest by downloads per plugin — the biggest is `dsh-ui-usage-billing` at 1,227
downloads, against 82,491 for the plugin market itself. Seventy-four separate people built
a quota panel and none of them won. The names are almost interchangeable: `dsh-balance`,
`dsh-cost-meter`, `dsh-usage-chart`, `dsh-quota-panel`, `dsh-deepseek-balance`,
`TokenLedger`, `dsh-api-balance`, `dsh-opencode-usage`. In a random 40-file sample of the
catalogue metadata, `usage` was also the single most common declared category.

This is the strongest demand signal in the dataset, and it says the same thing §2.2 said
about Orca from the other end: **everyone builds the read-out, nobody builds the
scheduler.** Seventy-four displays, zero park-and-resume. Wave 4's acceptance criterion —
a run that exhausts its window resumes after reset with its budget intact — is not just
unoccupied by competitors, it is unoccupied by an ecosystem of thousands of people who
clearly feel the pain daily.

**(b) Unattended autonomy is essentially absent.** Two plugins in the whole measured
catalogue match resume/unattended/watchdog naming: `dsh-auto-continue` (3,804 downloads)
and `dsh-autoresume` (0). Against 85 UI plugins. The ecosystem is optimising the
experience of *watching* an agent, not the problem of not having to. That is our thesis,
and the plugin layer of the largest harness in the field has barely touched it.

**(c) Memory is the most-starred single concern.** `dsh-memory-plugin` is the top plugin
in the catalogue by stars — **29,567★** — ahead of the harness's own satellite projects.
The bucket has 57 entries and the sub-genres are precisely Wave 2's design questions:
`dsh-mnemon`, `dsh-context`, `dsh-auto-memory`, `dsh-layered-memory`, `dsh-recall-plugin`,
and notably **`dsh-memory-gate`**. One catalogue entry describes itself in terms we could
have written: *"Experience notes auto-dedup… Every recall includes a traceable citation"*
(`00080000/dsh-project-memory`). Provenance-carrying memory with deduplication is not our
private insight; it is converging design. What no one there has is a memory whose entries
are tied to *run outcomes*, because a plugin cannot see across runs.

**(d) The delivery half is nearly empty — and that is the boundary.** Git, PR, diff, merge
and worktree together are **32 plugins and 4,421 downloads**, the smallest meaningful
bucket, one twelfth of the UI bucket's volume. Verification/review is 46 plugins but the
downloads are thin and the names are small-scale (`dsh-verify`, `dsh-doublecheck`,
`dsh-auto-review`, `dsh-checkpoint-rewind`, `030611/dsh-verification-receipt` at 204
downloads).

So the largest harness ecosystem in existence is overwhelmingly about **the chat surface**
— how the session looks, what it remembers, what it costs — and almost not at all about
**getting work landed**. That is the cleanest statement of where the harness stops and
Surge starts that this survey produced, and it did not come from a competitor's positioning
page; it came from what 3,363 people chose to spend a weekend on.

## 14. The architecture that made it possible: 70 capability seams

DSH's `docs/capability-seams.md` is machine-generated (`scripts/gen-doc-graphs.ts`,
"services are discovered from Cordis declarations") and enumerates **70 named services** on
a `ctx.*` registry, each with its owning package, its known implementations, and its direct
consumers. A selection, chosen because each one is a decision we have also had to make:

| Seam | What it is |
|---|---|
| `ctx.approval` | approval seam |
| `ctx.permissionPresets` | permission presets |
| `ctx.sandbox` / `ctx.sandboxPolicy` | process-sandbox seam and its policy home |
| `ctx.invariants` | package-owned invariant registry |
| `ctx.sessionProjections` / `ctx.sessionProjectionCache` | session projection units, persisted projection cache |
| `ctx.sessionQuery` | session reads, traces, filters, search |
| `ctx.sessionPersistence` | durable session persistence seam |
| `ctx.spillStore` | spill storage seam |
| `ctx.tokenMeter` | replay token measurement |
| `ctx.toolResultPruner` | model-free tool-result pruning |
| `ctx.skills` | skill provider registry |
| `ctx.subagents` | subagent provider and continuation service |
| `ctx.agentTeams` | agent-teams coordination domain |
| `ctx.goals` | same-session goal domain |
| `ctx.planMode` | plan collaboration state |
| `ctx.workflowEngine` | workflow script engine |
| `ctx.jobs` | background job registry |
| `ctx.webhookRuntime` | webhook rule runtime |
| `ctx.userQuestions` | human question/answer seam |
| `ctx.systemPrompt` | system-prompt assembly registry |
| `ctx.tools` | tool registry and guarded execution pipeline |
| `ctx.credentials` | credential seam |
| `ctx.sessionTelemetry` | session telemetry seam |

**Read for our strategy.** "Everything is a plugin" is a slogan; the thing that actually
produced 3,363 plugins in twenty-five days is duller and copyable: *every capability is a
named service with a declared contract, a dependency-inverted implementation slot, and an
auto-generated graph of who owns, implements and consumes it.* Contributors did not have to
read the codebase to find where to attach.

Surge has extension points — profiles, flow TOML, hooks, MCP servers, and skills once
Wave 1 lands — and **no published seam map at all**. Nobody outside the repository can see
where Surge is extensible or what contract they would be implementing. Generating that map
from our own declarations is a small piece of work with an outsized effect on whether
anyone can build on us.

Two seams deserve individual notice because they land on live tickets:
`ctx.sessionProjections` plus `ctx.sessionProjectionCache` is event-sourced projection with
a persisted cache — the same shape as our Run Report, which commit `13b44f9` describes as
"a projection of the log." And `ctx.spillStore` is Wave 4's output spill as a swappable
service. Convergent design on both, arrived at independently, which is mild evidence both
are right.

## 15. The trust hole, stated exactly

The catalogue's per-plugin metadata schema, verified across a random 40-file sample, is
**four fields**:

```yaml
url:          https://github.com/<owner>/<repo>
name:         <owner>/<repo>
category:     memory | usage | ui | security | tools | …
description:  { en: …, zh: … }
```

plus an optional `tarball:` pointing at a GitHub release `.tgz` (present in 4 of the 40
sampled). Keys matching hash, sha, signature, checksum, integrity, permission, scope or
version across that sample: **none**.

So a plugin is distributed as a repository URL or a raw release tarball, with no integrity
metadata, no declared permissions, no pinned version, and no signature — into a harness
whose own `SAFETY.md` says:

> "It has not undergone a security audit and must not be treated as secure or
> production-ready."
>
> "The project can execute model-generated code and commands, **load third-party plugins**,
> and access the network, processes, credentials, and files made available to it.
> Incorrect model output, defects, misconfiguration, malicious input, or **untrusted
> plugins** may damage the host computer, modify or delete files, disclose data or
> credentials, or cause other unintended effects."
>
> "**Review plugins, configuration, and proposed commands before allowing them to run.**"

The instruction in that last line is correct and, given the schema above, unactionable:
there is nothing to review against — no hash to compare, no version to pin, no declared
capability set to check. The ecosystem is already patching around it from inside, with more
unsigned plugins: `030611/dsh-telemetry-redactor` exists to strip *"supported secret
patterns from the `session-telemetry/record` export copy before configured telemetry
backends receive it"* — i.e. an unverified third-party plugin is the mitigation for
credentials leaking through the telemetry path.

**Read for our strategy.** Wave 1's trust gate (R15) was written from a principle — skills
are third-party executable context, so an unpinned or unhashed one requires approval. It
now has the concrete argument the ticket lacked: the largest capability ecosystem in the
field distributes thousands of executable extensions with a four-field manifest and no
integrity story, and its own vendor's safety page tells users to do a review the format
makes impossible. `SkillRef`'s `provider + version + content hash` is not bureaucracy
against that backdrop; it is the minimum.

It also sharpens C14 (minimum-age signal). With no version and no hash in the catalogue,
"when did this content last change" is one of the very few trust signals actually
obtainable from the outside.

## 16. Take, added to §4 and §12

| # | Take | From | Lands on |
|---|---|---|---|
| 16 | **Publish a generated seam map** — every extension point in Surge as a named contract with owner, implementations and consumers, generated from our own declarations, not hand-written | DSH `capability-seams.md` | docs + a generator; prerequisite for anyone building on us |
| 17 | **Hard evidence for the skill trust gate** — the four-field manifest, the absent integrity fields, and DSH's own SAFETY.md wording go into the Wave 1 ADR as its justification | §15 | Wave 1 / R15 |

And one strategic fact to carry rather than build: the DSH plugin ecosystem is a **chat
surface** ecosystem. 85 UI plugins and 74 quota panels against 32 git/PR plugins and 2
unattended-autonomy plugins. Harnesses are being extended toward *watching*; nobody there
is extending toward *landing*. That is the boundary we sit on, now measured rather than
asserted.
