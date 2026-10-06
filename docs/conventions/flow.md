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
  when none is set, on its own agent. If that is sent back too, routing takes
  `escalation_exhausted` to a human gate: **Retry once more** returns to the
  loop target for one more attempt, and the counter is not reset, so the next
  rejection asks again. **Stop** ends the run, or fails the current iteration inside a
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

A stage re-entered through a backtrack edge (or the extra attempt) receives a "Feedback from the
previous attempt" section with the sending stage's outcome, summary and
failed checks. A retry through the default gate carries the exhausted stage's
feedback.

## Profile Guidance

Flow Generator writes only `flow.toml`. Its validation uses the specialized
bootstrap retry path so invalid graphs produce `BootstrapEditRequested` feedback
instead of a second generic `on_outcome` rejection path.
