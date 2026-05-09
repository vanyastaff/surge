[← CLI](cli.md) · [Back to README](../README.md) · [Workflow →](workflow.md)

# Bootstrap

Bootstrap is the adaptive path from a free-form goal to an executable `flow.toml`.
It runs a bundled three-stage graph:

```text
Description Author -> HumanGate -> Roadmap Planner -> HumanGate -> Flow Generator -> HumanGate
```

Each authoring agent produces one artifact:

| Stage | Artifact | Purpose |
|---|---|---|
| Description | `description.md` | Clarifies the user goal, scope, constraints, and success criteria. |
| Roadmap | `roadmap.md` | Breaks the work into milestones and tasks. |
| Flow | `flow.toml` | Materializes the follow-up graph the engine will execute. |

Run it from a configured project:

```bash
surge bootstrap "add a safer retry policy to the webhook notifier"
```

The CLI asks for console approval at each HumanGate. After the generated flow is
approved, Surge starts a follow-up run and inherits the bootstrap artifacts into
that run through the content-addressed artifact store.

## Approval And Edit Cycle

Each bootstrap gate accepts three decisions:

| Decision | Effect |
|---|---|
| `approve` | Continue to the next bootstrap stage, or finish bootstrap after the flow gate. |
| `edit` | Append `BootstrapEditRequested`, route back to the preceding agent with fresh `edit_feedback`, and regenerate the artifact. |
| `reject` | Fail the bootstrap run. |

The default edit-loop cap is `3` per stage. On the next edit after the cap is
exhausted, the engine emits `EscalationRequested` and fails the run with an
explicit edit-loop-cap error.

Flow generation has an additional validation retry path. If `flow.toml` fails
to parse, fails `validate_for_m6`, or violates the selected archetype topology,
the engine emits `BootstrapEditRequested { stage = Flow }` and backtracks to
the Flow Generator. A later valid `flow.toml` emits `PipelineMaterialized`.

## Archetypes

The Flow Generator chooses one of the bundled archetypes:

| Archetype | Shape |
|---|---|
| `linear-3` | Spec -> Implement -> Verify. |
| `linear-with-review` | Linear flow plus a final review stage. |
| `multi-milestone` | Outer milestone loop with an inner task loop. |
| `bug-fix` | Reproduce -> Implement -> Verify with regression backtrack. |
| `refactor` | Capture behavior, refactor, verify, review. |
| `spike` | Bounded research or experiment. |
| `single-task` | Smallest single-agent task flow. |

User templates under `${SURGE_HOME}/templates/*.toml` can shadow bundled
templates by filename stem or `metadata.name`.

## Skip Bootstrap

Use the graph engine directly when you already know the flow shape:

```bash
surge engine run path/to/flow.toml --watch
surge engine run --template linear-3 --watch
```

`SPEC_PATH` and `--template` are mutually exclusive. Both forms skip bootstrap.

## Resume

If a clean bootstrap run has already been started and interrupted between
stages, resume it by run id:

```bash
surge bootstrap resume <run_id>
```

Resume reads the completed bootstrap event log, extracts the latest
`description`, `roadmap`, and `flow` artifacts, and starts the same inherited
follow-up run as the first-run path.

## Telemetry

Successful bootstrap runs append `BootstrapTelemetry` after materialization.
It records per-stage durations derived from event timestamps, per-stage edit
counts, and the selected archetype metadata when the generated graph declares
one.

## See Also

- [CLI](cli.md) - command surface and template skip behavior
- [Workflow](workflow.md) - where bootstrap fits in the AFK workflow
- [Archetypes](archetypes.md) - diagrams for bundled graph shapes
