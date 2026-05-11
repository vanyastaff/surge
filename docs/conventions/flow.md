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

## Profile Guidance

Flow Generator writes only `flow.toml`. Its validation uses the specialized
bootstrap retry path so invalid graphs produce `BootstrapEditRequested` feedback
instead of a second generic `on_outcome` rejection path.
