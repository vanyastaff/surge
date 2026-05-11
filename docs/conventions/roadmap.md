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

## Profile Guidance

Roadmap Planner should report both `roadmap.toml` and `roadmap.md`. The bundled
profile validates both artifacts on `drafted`.
