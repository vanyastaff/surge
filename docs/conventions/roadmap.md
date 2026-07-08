# Roadmap Artifacts

Roadmap Planner emits a machine-readable `roadmap.toml` plus human-compatible
`roadmap.md`.

Primary path: `roadmap.toml`
Compatibility path: `roadmap.md`
Validator kind: `roadmap`
Schema version: `schema_version = 2`, owned by the artifact contract.

`schema_version = 1` roadmaps still validate: the v2 task-ledger fields
(`depends_on`, `discovered_from`, `size`, `verified`) default when absent, and
their validation rules apply only at `schema_version = 2`.

## Task-ledger fields (schema v2)

Each `[[milestones.tasks]]` entry carries:

- `depends_on` — task ids (in any milestone) that must complete first.
  Task-granularity edges the ledger uses to answer "what is unblocked".
  Milestone-level ordering stays in `[[dependencies]]`.
- `discovered_from` — the task id this task was discovered from mid-execution.
  Captures work found during a task instead of dropping it.
- `size` — `s` / `m` / `l` context-budget class. **Required at v2.** Every task
  must be completable in one fresh agent session; split anything larger.
- `verified` — set only when a verification-authority node reports the task
  verified. Distinct from `status = "completed"`, which any stage can claim.

`status` gains two ledger transitions: `ready_for_verification` (implementation
done, awaiting a verifier) and `failed_verification` (verifier rejected it).

Validation rejects: duplicate milestone/task ids, `depends_on` /
`discovered_from` / `[[dependencies]]` references to unknown ids, self
references, cycles in the task `depends_on` graph, and (at v2) a task missing
its `size`.

## Minimal Valid TOML

```toml
schema_version = 2

[[milestones]]
id = "artifact-validation"
title = "Artifact validation"

[[milestones.tasks]]
id = "validators"
title = "Add validators"
description = "Validate canonical generated artifacts."
acceptance_criteria = ["Invalid schema versions fail", "Valid fixtures pass"]
size = "m"

[[milestones]]
id = "hook-enforcement"
title = "Hook enforcement"

[[milestones.tasks]]
id = "wire-hooks"
title = "Wire validators into hooks"
acceptance_criteria = ["Rejected outcomes force a retry"]
size = "s"
depends_on = ["validators"]

[[dependencies]]
from = "artifact-validation"
to = "hook-enforcement"
reason = "Hooks need validators to call."

[[risks]]
description = "Validators drift from prompts."
mitigation = "Keep profile prompts linked to this convention."
```

## Minimal Valid Markdown

```markdown
# Roadmap

## Milestones
- Artifact validation

## Dependencies
- Artifact validation before hook enforcement.

## Risks
- Validators may reject legacy output.
```

## Checklist

- TOML has top-level `schema_version = 2` (or `1` for legacy roadmaps).
- TOML has one or more `[[milestones]]`.
- **Schema v2 only:** every task has a `size` (`s` / `m` / `l`) sized to one agent
  session (validation requires `size` at v2; legacy v1 roadmaps omit it).
- Task ids are unique; `depends_on` / `discovered_from` reference real task ids.
- `[[dependencies]]` (if present) reference real milestone ids and are not self-referential.
- The task `depends_on` graph is acyclic.
- Tasks include clear titles and testable acceptance criteria when known.
- Markdown compatibility view has `## Milestones`, `## Dependencies`, and `## Risks`.

## Roadmap Patch Artifacts

`surge feature describe` produces `roadmap-patch.toml` through the Feature
Planner profile. A patch is an amendment proposal, not an immediate mutation.
It includes a target, rationale, ordered operations, optional dependencies,
optional conflicts, and a lifecycle status.

Minimal append example:

```toml
schema_version = 1
id = "rpatch-csv-export"
rationale = "User asked for CSV export after the roadmap was approved."
status = "drafted"

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"

[[operations]]
op = "add_task"
milestone_id = "m2"

[operations.task]
id = "m2-csv-export"
title = "Add CSV export"
acceptance_criteria = ["Exports current table filters", "Handles empty result sets"]

[operations.insertion]
kind = "append_to_milestone"
milestone_id = "m2"
```

Conflicts use stable codes such as `running_milestone`,
`completed_history`, `missing_target`, `duplicate_item`,
`dependency_cycle`, and `unsupported_operation`. Running milestone conflicts
must expose clear operator choices:

```toml
[[conflicts]]
code = "running_milestone"
message = "m2 is already running; choose where the new work should go."
choices = [
  "defer_to_next_milestone",
  "abort_current_run",
  "create_follow_up_run",
  "reject_patch",
]

[conflicts.item]
kind = "milestone"
milestone_id = "m2"
```

When an operator chooses a resolution, Surge records it in
`RoadmapPatchApprovalDecided.conflict_choice` and in the registry patch index.
The patch artifact remains the proposal; events and views carry lifecycle
state so replay stays deterministic.

## Profile Guidance

Roadmap Planner should report both `roadmap.toml` and `roadmap.md`. The bundled
profile validates both artifacts on `drafted`.
