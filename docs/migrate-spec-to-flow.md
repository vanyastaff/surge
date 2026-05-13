# Migrating `.spec.toml` to `flow.toml`

The legacy structured-spec format is retiring as part of the **Legacy pipeline retirement** milestone. All new work uses `flow.toml`. The `surge migrate-spec` CLI converts existing `.spec.toml` files automatically; this page is the reference for what the translator does, what to review by hand, and how to run the result.

## Why migrate

| Aspect | Legacy `.spec.toml` | `flow.toml` (current) |
|---|---|---|
| Model | A bag of subtasks + a topological sort | A directed graph: nodes, declared outcomes, and edges keyed by outcome |
| Routing | Implicit: phases drive the order | Declarative: `[[edges]]` decide what runs next per outcome |
| Extensibility | Closed phase set inside `surge-orchestrator` | Closed `NodeKind` enum; extensibility via profiles / templates |
| Replay | Folded ad-hoc from disk state | Folded from an append-only event log |
| Bootstrap | Hand-authored | Three-stage Description → Roadmap → Flow generation |
| Engine path | `surge-orchestrator::pipeline::Orchestrator` (deprecated) | `surge-orchestrator::engine::Engine::start_run` |

Architecturally, surge is moving to a single execution path so that replay, fork-from-here, and crash recovery are all folds of the same event log. The legacy pipeline cannot participate in that model; the graph executor is the replacement.

See [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) for the broader picture and [`docs/legacy-parity-checklist.md`](legacy-parity-checklist.md) for the module-by-module replacement mapping.

## Quick start

```bash
# Translate one spec to stdout
surge migrate-spec specs/feature.spec.toml

# Write the result to disk
surge migrate-spec specs/feature.spec.toml --output flows/feature.flow.toml

# Treat warnings as non-fatal (exit 0 even if the translator flagged structural concerns)
surge migrate-spec specs/feature.spec.toml --allow-warnings
```

Exit codes:

| Code | Meaning |
|---|---|
| `0` | Clean migration; no warnings, or warnings allowed via `--allow-warnings`. |
| `1` | Failed to read or parse the legacy spec. |
| `2` | Migration produced warnings and `--allow-warnings` was not passed. The output `flow.toml` is still written; review the warnings before running it. |

The generated `flow.toml` begins with a comment block summarizing warnings raised during translation. Read those first.

## Mapping reference

| Legacy field | `flow.toml` location | Notes |
|---|---|---|
| `spec.title` | `[metadata].name` (slugified), `[metadata].description` fallback | Title is lowercased and stripped of punctuation to form the flow name. |
| `spec.description` | `[metadata].description` | Used verbatim when present; otherwise falls back to the title. |
| `spec.complexity` | (dropped) | Spec-level complexity is informational; the engine does not consume it. |
| `spec.subtasks[i].id` | `[nodes.sN].id` | All ULIDs are remapped to `s1`, `s2`, …, in the order they appear in the spec. The original ULIDs do not survive. |
| `spec.subtasks[i].title` | Used in `[[nodes.sN.declared_outcomes]].description` (`"<title> passed"` / `"<title> failed"`) | |
| `spec.subtasks[i].description` | (dropped from the flow file) | Move long context into `stories/story-NNN.md` and reference from the agent profile if needed. |
| `spec.subtasks[i].complexity` | `[nodes.sN.config.custom_fields].complexity` | Preserved as `"simple"` / `"standard"` / `"complex"` for downstream tooling. |
| `spec.subtasks[i].files` | `[nodes.sN.config.custom_fields].files` | Array of strings, only emitted when non-empty. |
| `spec.subtasks[i].acceptance_criteria[].description` | `[nodes.sN.config.custom_fields].acceptance_criteria` | Array of strings (the `met` flag is runtime state, not migrated). |
| `spec.subtasks[i].depends_on[]` | `[[edges]]` with `from.outcome = "pass"`, `kind = "forward"` | One edge per declared dependency. |
| `spec.subtasks[i].agent` | `[nodes.sN.config.profile]` | Defaults to `implementer@1.0` when absent (raises a `ProfileDefaulted` warning). |
| `spec.subtasks[i].story_file` | (dropped) | Re-attach manually if needed; the flow-level story-file mechanism is still on the roadmap. |
| `spec.subtasks[i].execution` | (dropped) | Runtime state, not configuration. |

In addition, the translator always synthesizes:

- A `success` terminal node, reached by `pass` edges from every leaf subtask.
- A `failure` terminal node, reached by `fail` edges from every subtask.
- A `[[nodes.sN.declared_outcomes]]` pair (`pass` + `fail`) on every Agent node.
- Default per-node limits: `timeout_seconds = 900`, `max_retries = 3`, `max_tokens = 200000`.
- Default per-edge policy: `on_max_exceeded = "escalate"`.

## Warnings and manual edits

The translator emits soft warnings that do not prevent output but do flag structural decisions you should review. Each warning is rendered as a comment block at the top of the generated `flow.toml` and as a tracing event on stderr.

### `NonLinearDeps`

A subtask declares two or more `depends_on` entries (fan-in or diamond). The translator wires every dependency as a `pass`-forward edge into the destination node, but you should verify that the merge semantics are what you want — the graph engine fires the destination once per converging edge.

Manual edit options:

- Replace the converging Agent node with a `Loop` body if the destination should run once per upstream iteration.
- Insert a dedicated `HumanGate` ahead of the converging node when the dependent work needs human confirmation that all upstream lanes are done.

### `ProfileDefaulted`

A subtask had no `agent` field, so `implementer@1.0` was chosen as a safe default. The bundled profile registry includes more specialised roles (architect, verifier, bug-fix implementer, refactor implementer, security reviewer, migration implementer); pick the one that matches the work and overwrite the `profile = …` line.

Run `surge profile list` to see what is available.

### `MultipleRoots`

The spec had two or more subtasks with no incoming dependencies. The translator picks the first one in spec order as the flow `start`. If a different starting point is intended, edit the `start = "sN"` field. If multiple lanes should run in parallel from the beginning, restructure the prefix into a single seed node that fans out via outcomes.

## After migration

1. **Review the warnings.** Read the comment block at the top of the generated file.
2. **Inspect the graph.** `surge engine validate flows/feature.flow.toml` (or open the file in an editor) — verify nodes, outcomes, and edge wiring.
3. **Adjust profiles and limits.** Defaults are conservative; project-specific timeouts or specialised roles usually need tuning.
4. **Run.**

   ```bash
   surge engine run flows/feature.flow.toml
   ```

   Equivalent to the legacy `surge run <spec_id>` invocation; everything else (status, logs, cancellation, replay) lives under `surge engine *`.

## Things the translator does not handle

These cases require human authorship after the auto-translation:

- **Mid-flow human gates.** Spec format had no concept of human checkpoints; the engine has `HumanGate` nodes. Add them where you need approval breakpoints.
- **Loops over an item collection.** If a subtask logically iterates ("for each file, do X"), promote it to a `Loop` node with an items binding rather than copying the subtask N times.
- **Subgraph reuse.** Common sub-flows (e.g. `verify_and_commit`) become `Subgraph` nodes referenced from multiple parents.
- **Notify side-effects.** Email/Slack/Telegram alerts on stage transitions belong in `Notify` nodes wired to the relevant outcomes.

The flow-authoring reference is [`docs/conventions/flow.md`](conventions/flow.md); archetype examples live in [`docs/archetypes.md`](archetypes.md).

## Removed surfaces

The retirement milestone also removes the legacy CLI surfaces that operated on `spec_id`. `surge engine *` (`run`, `status`, `logs`, `stop`, `resume`) supersedes them. See the milestone notes in [`.ai-factory/ROADMAP.md`](../.ai-factory/ROADMAP.md) for the full list.

## See also

- [`docs/conventions/flow.md`](conventions/flow.md) — `flow.toml` authoring reference.
- [`docs/conventions/spec.md`](conventions/spec.md) — historical reference for the legacy spec format.
- [`docs/legacy-parity-checklist.md`](legacy-parity-checklist.md) — module-by-module mapping of the legacy pipeline to the graph executor.
- [`docs/cli.md`](cli.md) — current CLI surface.
- [`docs/archetypes.md`](archetypes.md) — bundled flow shapes with diagrams.
