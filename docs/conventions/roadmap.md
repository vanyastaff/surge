# Roadmap Artifacts

Roadmap Planner emits a machine-readable `roadmap.toml` plus human-compatible
`roadmap.md`.

Primary path: `roadmap.toml`
Compatibility path: `roadmap.md`
Validator kind: `roadmap`
Schema version: `schema_version = 1`, owned by the artifact contract.

## Minimal Valid TOML

```toml
schema_version = 1

[[milestones]]
id = "artifact-validation"
title = "Artifact validation"

[[milestones.tasks]]
id = "validators"
title = "Add validators"
description = "Validate canonical generated artifacts."
acceptance_criteria = ["Invalid schema versions fail", "Valid fixtures pass"]

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

- TOML has top-level `schema_version = 1`.
- TOML has one or more `[[milestones]]`.
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
