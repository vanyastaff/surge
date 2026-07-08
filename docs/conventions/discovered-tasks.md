# Discovered Tasks Artifact

An agent produces `discovered-tasks.toml` when it finds work mid-task that
belongs in the ledger — the "discovered-from" edge that keeps a large project
from silently dropping follow-up work (see [`product-strategy.md`](../product-strategy.md)
Pillar A).

Primary path: `discovered-tasks.toml`
Validator kind: `discovered-tasks`
Schema version: `schema_version = 1`, owned by the artifact contract.

The artifact carries only the **identity** of each discovered task. The engine
attaches the origin: when an agent stage running inside a task loop produces a
`discovered-tasks` artifact, each entry is appended to the ledger as a
`TaskDiscovered` event with `discovered_from` set to the current task. A
malformed artifact is logged and skipped — it never fails the stage.

## Minimal Valid TOML

```toml
schema_version = 1

[[tasks]]
id = "m1-t9"
title = "Handle empty CSV export"
description = "Export produced a header-only file for an empty result set."

[[tasks]]
id = "m1-t10"
title = "Add pagination to the export endpoint"
```

## Checklist

- Top-level `schema_version = 1`.
- One or more `[[tasks]]`, each with a non-empty `id` and `title`.
- `id`s are unique within the artifact.
- Do **not** set `discovered_from` here — the engine derives it from the task
  the agent was working on.
