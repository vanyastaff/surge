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

## Missions and validation contracts

Optional `[[missions]]` group milestones into bounded efforts, each with its
own definition of done. Declare them for a new application or for two or more
vertical slices; a single-milestone roadmap does not need one.

- `id`, `title`, `goal` — identity and the outcome in one or two sentences.
- `milestones` — ids of the milestones the mission owns, in order.
- `[[missions.validation_contract]]` — behavioural assertions: `id` with an
  area prefix (`VAL-AUTH-001`), `title`, `pass_condition` (observable
  behaviour that passes or fails), optional `evidence` (what a verifier must
  capture).
- Tasks claim assertions through `fulfills = ["VAL-AUTH-001"]`: only the leaf
  task that makes an assertion testable claims it; infrastructure tasks claim
  none.

Write the contract from the description before splitting the mission into
tasks, so it states what the user asked for rather than what the tasks happen
to build.

When missions are declared, validation also rejects: a milestone in no mission
or in several, a mission with no milestones or an empty contract, mission
milestone lists that do not follow the `[[milestones]]` order, duplicate
assertion ids, and any assertion not claimed by exactly one task of its own
mission (`validation_contract_coverage`). A task whose `fulfills` names an
unknown assertion is rejected with or without missions. Roadmap patches keep
the partition: an inserted milestone joins its anchor's mission, and an
appended one joins the mission of the current last milestone.

```toml
[[missions]]
id = "mission-1"
title = "Accounts"
goal = "People can create an account and sign in."
milestones = ["accounts"]

[[missions.validation_contract]]
id = "VAL-AUTH-001"
title = "Sign in"
pass_condition = "A registered user signs in with email and password and lands on the dashboard."
evidence = ["end-to-end test output"]

[[milestones]]
id = "accounts"
title = "Accounts"

[[milestones.tasks]]
id = "user-schema"
title = "User table"
size = "s"

[[milestones.tasks]]
id = "sign-in"
title = "Sign-in flow"
size = "m"
depends_on = ["user-schema"]
fulfills = ["VAL-AUTH-001"]
```

## Release stages, priority and parallelism

All of these are optional and additive; `schema_version` stays `2` and older
roadmaps parse unchanged.

**Stages.** `[[stages]]` group milestones into releases, in declared order.
There is no fixed MVP/beta/prod ladder: the planner picks the smallest
structure that fits the approved description. A trivial app needs one stage
(or none) and a couple of milestones; add a stage only at a real release
boundary, where something usable exists before the next stage starts. Never
emit an empty or ceremonial stage, and justify the count in one sentence in
the top-level `stages_rationale`. The human approves the plan or sends it back
for rework.

- `id`, `title`, `goal` — identity and the outcome of the release; `title` is
  free-form ("MVP", "Public beta", "Launch").
- `milestones` — ids of the milestones the stage owns, in order.
- `exit_criteria` — observable conditions that must hold before the stage is done.

Stages partition the milestones exactly like missions do: when stages are
declared, every milestone is in exactly one stage, no stage is empty, and the
stage lists concatenated reproduce the `[[milestones]]` order. The number of
stages is never constrained (one is valid) and validation does not reward more.
Stages and
missions are independent groupings and may both be present. Roadmap patches
keep both partitions (an inserted milestone joins its anchor's stage).
Violations report the `stage_structure` code (or `invalid_reference` /
`duplicate_identifier` for unknown or repeated ids).

**Priority.** `priority = "p0" | "p1" | "p2" | "p3"` on a milestone or a task;
`p0` is the most urgent. Absent means unprioritised and sorts last.

**Parallel groups.** `parallel_group = "<name>"` on a task. Tasks that share a
name may run concurrently, so validation rejects a group in which one member
depends, directly or through other tasks, on another member
(`parallel_group_conflict`), and a blank group name. Use `depends_on` for
ordering; use a group only to state real independence.

`RoadmapArtifact::ready_batches()` turns `depends_on` into dependency-respecting
waves (each wave can run concurrently), ordered inside a wave by task priority,
then milestone priority, then declaration order. Tasks on a cycle are omitted.

```toml
stages_rationale = "One stage: sign-in ships as a single usable release."

[[stages]]
id = "stage-1"
title = "MVP"
goal = "A user can sign in and see their dashboard."
milestones = ["accounts"]
exit_criteria = ["Sign-in works end to end"]

[[milestones]]
id = "accounts"
title = "Accounts"
priority = "p0"

[[milestones.tasks]]
id = "user-schema"
title = "User table"
size = "s"
priority = "p0"

[[milestones.tasks]]
id = "sign-in-ui"
title = "Sign-in form"
size = "m"
depends_on = ["user-schema"]
parallel_group = "sign-in"

[[milestones.tasks]]
id = "sign-in-api"
title = "Sign-in endpoint"
size = "m"
depends_on = ["user-schema"]
parallel_group = "sign-in"
```

`to_markdown` renders each stage as a top-level `# Stage <id>: <title>`
heading with its milestones beneath, and tags tasks with `[p0]` and
`[parallel: <group>]`.

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
- **With missions:** every milestone is in exactly one mission, in order; every
  mission has a non-empty `validation_contract`; every assertion is claimed by
  exactly one task of its own mission.
- **With stages:** as few as the work needs (one is fine), none empty; every
  milestone is in exactly one stage, in order.
- Tasks in one `parallel_group` do not depend on each other.

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
