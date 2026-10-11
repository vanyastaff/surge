# Flow Artifact

`flow.toml` is the executable workflow graph consumed by the engine.

Path: `flow.toml`
Validator kind: `flow`
Schema version: `schema_version = 1`, owned by the graph schema.

## Minimal Valid Example

```toml
schema_version = 1
start = "end"
edges = []

[metadata]
name = "single-terminal"
created_at = "2026-05-11T00:00:00Z"

[nodes.end]
id = "end"
declared_outcomes = []

[nodes.end.position]
x = 0.0
y = 0.0

[nodes.end.config]
node_kind = "terminal"

[nodes.end.config.kind]
type = "success"
```

## Checklist

- Top-level `schema_version = 1` is present.
- `[metadata]`, `start`, `[nodes]`, and `edges` are present.
- Every non-terminal node has declared outcomes and reaches a terminal node.
- Agent nodes reference profiles like `implementer@1.0`, not free-form prompts.
- Bundled archetypes include `feature`, `bug-fix`, `refactor`, `performance`, `security`, `docs`, and `migration`.

## Retry loops and escalation

A retry loop is a backtrack edge with `max_traversals` (for example a
verifier's `failed` back to the implementer). When the limit is spent and
`on_max_exceeded = "escalate"` (the default), routing takes the source's
`max_traversals_exceeded` outcome:

- If the flow declares an edge for that outcome, it is used as written.
- Otherwise the engine derives a default ladder at run time
  (`surge_core::escalation`). When the loop target is an agent node, it first
  gets **one extra automatic attempt** with the latest findings, on the agent
  set in `surge.toml` (`[escalation] retry_agent`, optional `retry_model`) or,
  when none is set, on its own agent. If that is sent back too and the loop
  runs inside a loop body (a roadmap task loop), a split planner
  (`task-splitter@1.0`) may replace the task with 1-12 smaller tasks written to
  `discovered-tasks.toml`. They run right after it in the same loop, each with
  its own retry budget, and the replaced task's iteration ends without failure
  (`TaskSplit` in the run log). If the planner reports `cannot_split`, the rung
  is spent, or the loop is not in a loop body, routing reaches a human gate: **Retry once more** returns to the
  loop target for one more attempt, and the counter is not reset, so the next
  rejection asks again. **Accept as is** continues on the exhausted stage's success path
  (its verified outcome, else its first forward edge that does not fail) and
  records `TaskAcceptedByHuman`: completed, never verified, the verifier's
  latest findings kept. **Revise requirement** records the comment as
  `RequirementRevised` and runs the exhausted stage again; every later stage of
  the same task (or of the run, outside task loops) sees the revision first in
  its prompt. **Stop** ends the run, or fails the current iteration inside a
  loop so `on_iteration_failure` applies. The gate has no practical deadline.
  It is derived from the persisted graph and never written into it.
- A stage whose capped edges lead to different targets gets no default gate
  and still fails when exhausted.

```toml
[escalation]
retry_agent = "codex-acp"   # registry id; unknown ids are ignored with a warning
retry_model = "gpt-5"       # optional
```

A stage that runs under a frozen quota plan keeps its planned runtime for the
extra attempt.

## Agent switching on limits

When a stage's agent hits its usage limit, the engine moves that stage to
another agent instead of parking, if `surge.toml` names fallbacks:

```toml
[capacity]
fallback_agents = ["codex-acp", "gemini"]   # tried in order, registry ids
```

The first fallback that is configured, launchable and not itself exhausted is
chosen, preferring one that differs from the stage's verifier/implementer
partner; if only the partner's agent fits it is used and the move is flagged.
The stage opens a fresh session on the new agent (a provider session never
continues across agents) and stays there until it routes an outcome. When no
fallback fits, the run parks and wakes as before. Every move is a
`StageRuntimeRotated` event.

## Loop protection

A stage attempt ends as a **failed attempt** when the engine sees no activity
for `idle_limit_secs`, the same tool call more than `max_repeat_tool_calls`
times in a row, more than `max_tool_calls` tool calls, or the attempt running
past `node_wall_clock_limit_secs`. The engine routes it through the stage's
capped retry outcome (for example an implementer's `partial`, a verifier's
`failed`) with the reason as feedback for the next attempt, so it counts
against the same budget and an exhausted budget climbs the same ladder. A
stage without a capped retry outcome fails the run, as before. Each trip also
records `EscalationRequested` with a loop-guard cause.

```toml
[tool_call_loop_guard]
max_repeat_tool_calls = 3        # 4th identical call in a row ends the attempt
node_wall_clock_limit_secs = 3600
idle_limit_secs = 900            # 0 disables
max_tool_calls = 1000            # 0 disables
```

Retry budgets (`max_traversals`) inside a loop body reset at every iteration:
a task never inherits the traversals another task spent.

A stage re-entered through a backtrack edge (or the extra attempt) receives a "Feedback from the
previous attempt" section with the sending stage's outcome, summary and
failed checks. A retry through the default gate carries the exhausted stage's
feedback.

## Profile Guidance

Flow Generator writes only `flow.toml`. Its validation uses the specialized
bootstrap retry path so invalid graphs produce `BootstrapEditRequested` feedback
instead of a second generic `on_outcome` rejection path.
