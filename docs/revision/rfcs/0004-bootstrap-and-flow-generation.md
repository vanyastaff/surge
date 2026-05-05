# RFC-0004 · Bootstrap and Flow Generation

## Overview

Bootstrap is the pre-pipeline phase where the user's natural language goal becomes an executable graph. It happens at the start of every run and consists of three sequential agent stages, each gated by human approval:

1. **Description Author** — interprets the user's prompt into a structured `description.md`
2. **Roadmap Planner** — decomposes into milestones with tasks, produces `roadmap.md`
3. **Flow Generator** — selects nodes and structure, produces `flow.toml`

After Flow approval, the generated graph is materialized and pipeline execution begins.

This document specifies what each bootstrap stage does, how adaptive complexity works, and what the Flow Generator's decision logic looks like.

## Why three stages

A single bootstrap call producing description + roadmap + flow at once was considered and rejected. Reasons:

- **Human can stop early.** If Description is wrong, no point reading a long Flow.
- **Each stage benefits from focused attention.** Mixing "what" + "how to break it down" + "which nodes" in one prompt produces shallow output for all three.
- **Approvals scope decisions.** User commits to "this is what I want" before debating "how to structure it".
- **Each artifact is the input to the next.** Description grounds Roadmap. Roadmap grounds Flow. Sequential refinement.

Cost is three LLM calls instead of one (~$0.50–$1.50 extra per run). Acceptable for the quality and control gain.

## Bootstrap stages as graph nodes

Bootstrap stages are not hidden pre-stages. They are first-class Agent nodes in the run's graph, just shipped in `~/.vibe/profiles/_bootstrap/`. This means:

- Same execution mechanics (event log, replay, fork-from-here) apply
- Bootstrap nodes can be customized by power users (override the profile)
- Bootstrap can be skipped via flag (`--skip-bootstrap` for known templates)
- Visualizable on canvas like any other node

The standard graph for any user-initiated run begins with:

```
[Description Author] → [Approve Description] →
[Roadmap Planner] → [Approve Roadmap] →
[Flow Generator] → [Approve Flow] →
[graph dynamically extended with Flow Generator's output]
```

The first six nodes are constant. Everything after `Approve Flow` is generated.

## Stage 1: Description Author

### Inputs

- User's raw prompt (free text from CLI or Telegram).
- Project context: cwd, `git status` output, top-level file listing, existing `AGENTS.md` if any, language detection.

### Output

A markdown artifact `description.md` with structured sections:

```markdown
# Description: <task title>

## Goal
What the user wants to accomplish, in 1–3 sentences.

## Context
- Project type: <e.g., "Rust library crate (workspace member)">
- Current state: <e.g., "Empty crate, just Cargo.toml">
- Stack hints: <e.g., "uses tokio, prefers serde">

## Requirements
- Functional: <bulleted list>
- Non-functional: <performance, compatibility, etc.>

## Out of scope
What is explicitly NOT being done in this run.

## Open questions
If any genuine ambiguity, listed here. Otherwise: "None."
```

### Behavior

The agent:
1. Reads project context using read-only filesystem tools.
2. May search web for unknown technologies if user mentioned them.
3. Asks clarifying questions ONLY if required information is missing — not for "completeness". Asking too many questions is the most common antipattern.
4. Calls `report_stage_outcome` with `done` (description ready) or `unclear` (genuinely cannot proceed).

### Outcomes

- `done` → forward to Approve Description gate
- `unclear` → escalate to HumanGate with specific question

### Outcome of approval gate

- `approve` → forward to Roadmap Planner
- `edit` → re-run Description Author with user's free-text feedback as additional context
- `reject` → Terminal (Aborted)

### System prompt outline

The Description Author profile lives in `~/.vibe/profiles/_bootstrap/description-author.toml`. Key elements:

- Role: "You convert vague human task descriptions into structured technical specifications."
- Constraints: "Do not propose implementation. Do not suggest tools or libraries unless user mentioned them."
- Output format: "Always produce a single `description.md` file."
- Anti-patterns: "Do not invent requirements. Do not pad with generic best practices."

## Stage 2: Roadmap Planner

### Inputs

- `description.md` from previous stage
- Project context (same as before)

### Output

`roadmap.md` with milestone decomposition:

```markdown
# Roadmap: <task title>

## Strategy
1–2 paragraph high-level approach.

## Milestones

### M1: <name>
<1–3 sentences describing what this milestone delivers>
- Complexity: low | medium | high
- Depends on: — | M_n
- Tasks:
  - T1.1: <atomic implementable unit>
  - T1.2: ...

### M2: <name>
...
```

### Behavior

The agent:
1. Reads `description.md` carefully.
2. Decides decomposition granularity:
   - Trivial work → 0 or 1 milestone with 1–3 tasks
   - Medium work → 1 milestone with 3–6 tasks
   - Large work → 2–6 milestones each with 3–8 tasks
3. Each task must be atomic enough to be a single Implementer attempt (rough rule: <2 hours of human work equivalent).
4. Calls `report_stage_outcome` with `done` or `unclear` (rare).

### Outcomes

Same structure as Description Author.

### System prompt elements

- Role: "Decompose technical specifications into executable milestones and tasks."
- Constraints: "Avoid creating milestones that are just process artifacts (e.g., 'Setup', 'Testing'). Each milestone should deliver user-visible value."
- Anti-patterns: "No more than 6 milestones. No tasks named 'misc' or 'cleanup'."
- Examples (few-shot): 2–3 sample (description, roadmap) pairs across complexity tiers.

## Stage 3: Flow Generator

### Inputs

- `description.md`
- `roadmap.md`
- Profile registry (list of available roles with metadata)
- Project context

### Output

`flow.toml` — a complete graph definition that conforms to `RFC-0003`. The graph extends the run's existing graph (which currently has the bootstrap nodes leading to this point).

### Behavior — adaptive complexity selection

This is the Flow Generator's main judgment. Decision tree (encoded in its system prompt):

```
Step 1: Estimate scope.
  - Count milestones in roadmap
  - Count total tasks across milestones
  - Detect archetype hints from description

Step 2: Pick base structure.
  IF roadmap has 0 milestones (or 1 milestone with ≤2 tasks):
    → Linear flow (3–7 nodes)
    → No Loop nodes
    → Simple: Implement → Verify → PR (trivial)
    → Or: Spec → Implement → Verify → Review → PR (small)

  IF roadmap has 1 milestone with 3–8 tasks:
    → Linear with one inner Task Loop
    → Architect → Plan → [Loop: Implement → Review → Commit] → PR

  IF roadmap has ≥2 milestones:
    → Outer Milestone Loop with inner per-milestone subgraph
    → Each milestone subgraph: Architect → Plan → [Task Loop] → Verify
    → After all milestones: Final Review → PR Composer → Terminal

Step 3: Apply archetype overrides.
  IF description has bug-fix language ("fix", "broken", "doesn't work"):
    → Add Reproduce stage before Implementer
    → Add Regression Test stage after fix
    → Use bug-fix profile for Implementer

  IF description has refactor language ("refactor", "clean up", "split"):
    → Add Behavior Characterization stage
    → Add Tests-First stage if no tests exist
    → Use diff-min Reviewer profile

  IF description has spike language ("explore", "POC", "spike"):
    → Skip Architect, Reviewer
    → Verify is optional

  IF description mentions critical/security/payments:
    → Add SecurityReviewer in parallel with main Reviewer
    → Add manual approval gate before PR

Step 4: Insert HumanGates strategically.
  - Always: before main pipeline starts (already in bootstrap)
  - Always: before PR (final go/no-go)
  - For >1 milestone: optional gate between milestones (off by default, on for >3 milestones)
  - Never: between every task (would defeat autonomy)

Step 5: Validate output.
  - Every Agent node references a valid profile from registry
  - Every declared outcome has an edge
  - Graph is structurally valid per RFC-0003
```

### Outcomes

- `done` — `flow.toml` is ready for approval
- `cannot_generate` — Flow Generator cannot map roadmap to available profiles (e.g., user asked for ML training and no such profile exists). Escalates to HumanGate.

### Approval gate outcome

- `approve` → engine parses `flow.toml`, materializes pipeline, begins execution
- `edit` → re-run Flow Generator with feedback (e.g., "skip the review stage", "add tests")
- `reject` → Terminal (Aborted)

### Critical constraint

**Flow Generator must select from existing profiles only.** It cannot invent new roles. This is the determinism boundary — agents do work, but they don't define new types of agents at runtime. This is encoded heavily in the system prompt.

## Profile registry as Flow Generator's input

For Flow Generator to make good choices, it needs to know what profiles exist and what they do. The profile registry is exposed to the Flow Generator in its prompt context:

```toml
# This is what gets injected into Flow Generator's system prompt as context

[[available_profiles]]
id = "implementer"
display_name = "Implementer"
description = "Writes Rust code following plan.md. Tests must pass before reporting done."
when_to_use = "Standard implementation work where plan and spec exist."
inputs_expected = ["spec.md", "plan.md", "adr-*.md"]
outputs_produced = ["modified source files", "git commits"]
declared_outcomes = ["done", "blocked", "escalate"]

[[available_profiles]]
id = "reviewer"
display_name = "Reviewer"
description = "Reads diff, reviews for logic errors and architecture issues."
# ... etc
```

This is generated dynamically from the profile registry on each Flow Generator invocation. Add a new profile → it's automatically available for Flow Generator to consider.

## Edit feedback loop

Each approval gate has an `edit` option that lets the user provide free-text feedback. The implementation:

1. User taps "Edit" in Telegram, types feedback as chat reply: e.g., "Skip the verifier stage, I run tests manually" or "This is a security-critical change, add SecurityReviewer".
2. Engine writes `BootstrapEditRequested { stage, feedback }` event.
3. Re-runs the same bootstrap stage, injecting the feedback as additional context:
   ```
   Original output (rejected):
   <previous artifact>
   
   User feedback:
   <free-text feedback>
   
   Produce a revised version.
   ```
4. New artifact replaces old in storage; both versions visible in event log.

This makes correction conversational, not menu-driven. Most edits are 1–2 sentences and pass on first revision.

## Skip bootstrap

For experienced users with a stable template that works well for them:

```bash
vibe run "<description>" --template=rust-crate-tdd
```

This skips Description and Roadmap stages, directly uses the named template's pre-baked flow, and starts execution. Description equivalent is auto-generated from `<description>` argument as a single paragraph; no human approval.

Recommended for repetitive tasks (small bugfixes in known projects, dependency bumps, etc.).

## Bootstrap on canvas

When a run is in bootstrap phase, the canvas shows only the six bootstrap nodes. After Flow Generator approval, the canvas **expands** with the generated nodes appearing connected to the bootstrap chain. This is a visible event — the user sees the graph "grow" in real time as bootstrap completes.

In replay mode, scrubbing back to before Flow Generator's approval shows the smaller bootstrap-only graph; scrubbing forward shows it expanded. This is consistent with the event-sourced model: graph state is part of the run state.

## Telegram approval cards

Each bootstrap stage produces an approval card with similar structure but stage-specific content:

### Description card

- Title: "Description ready · 1 / 3"
- Body: blockquote with the description summary (2–4 lines), then key fields (stack, target, scope)
- Buttons: `Approve` / `Edit` / `Reject`
- Link: "View full description.md" → opens deeplink to local web view

### Roadmap card

- Title: "Roadmap ready · 2 / 3"
- Body: list of milestones with task counts
  ```
  M1: Core lexer (3 tasks · medium)
  M2: AST & parser (4 tasks · high)
  M3: Serde integration (2 tasks · medium)
  M4: Edge cases & fuzzing (3 tasks · medium)
  ```
- Buttons: `Approve` / `Edit` / `Reject`

### Flow card

- Title: "Flow ready · 3 / 3 — last gate before run"
- Body: text mini-diagram of the generated structure (collapsed loops shown as `[Loop: ...]`), plus key parameters
  ```
  Bootstrap: Description → Roadmap → Flow ✓
  
  Milestone Loop (×4):
    Architect → Plan → ADR
    Task Loop (×~3):
      Implement → Review → Commit
    Verify
  
  Final: PR Composer → Notify → Terminal
  
  Sandbox: workspace+network (allowlist: crates.io, github.com)
  Estimated: 28 min · ~$4.20
  ```
- Buttons: `Approve & start` / `Edit` / `Reject`

## Failure modes

### Flow Generator hits "cannot_generate"

Causes:
- Roadmap requires capability not in profile registry
- Description is incoherent (rare, Description Author should catch)

Handling:
- Outcome `cannot_generate` routes to HumanGate
- Card explains: "Flow Generator couldn't find suitable profiles for milestone M2. Consider: (a) simplifying scope, (b) using Generic Agent for that milestone, (c) adding a custom profile."
- User can edit roadmap, edit description, or abort.

### Bootstrap loop

If user keeps editing and re-running same stage indefinitely, that's their choice — no enforced limit. But each edit is a separate event, fully audit-trailed.

### Bootstrap timeout

Default: no timeout on approval gates. User may walk away for hours. Run sits as daemon. On return, taps approve.

Configurable per-user policy: `~/.vibe/config.toml` can set `bootstrap_approval_timeout_hours = 24` to auto-fail after 24h. Default `None`.

## Acceptance criteria

The bootstrap and flow generation are correctly implemented when:

1. Running `vibe run "fix typo in README"` produces a 3–4 node linear flow after bootstrap.
2. Running `vibe run "build a JSON5 parser library with serde support"` produces a multi-milestone nested-loop flow.
3. Running `vibe run "fix the panic in parse_object"` produces a bug-fix archetype flow with reproduce + regression test stages.
4. Description, Roadmap, and Flow stages are visible as nodes in canvas, can be inspected, and their outputs are stored as artifacts.
5. Editing each stage with free-text feedback produces a revised artifact and second approval card.
6. Flow Generator's output passes graph validation (RFC-0003) for 100% of test cases (unit tests with 50+ scenarios).
7. Skipping bootstrap with `--template` flag bypasses the three stages and starts pipeline execution directly with the template's flow.
8. The generated flow.toml is identical when the same description + roadmap + project context + Flow Generator system prompt produce the same output (deterministic if temperature=0).
