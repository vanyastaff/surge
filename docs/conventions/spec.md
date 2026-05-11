# Spec Artifacts

Spec Author emits a typed `spec.toml` plus a human-compatible `spec.md` for a
single milestone or bounded task set.

Primary path: `spec.toml`  
Compatibility path: `spec.md`  
Validator kind: `spec`  
Schema version: `schema_version = 1`, owned by the artifact contract.

## Minimal Valid TOML

```toml
schema_version = 1

[spec]
id = "artifact-validation"
title = "Artifact validation"
description = "Define and enforce generated artifact contracts."
complexity = "medium"

[[spec.subtasks]]
id = "core-contracts"
title = "Core contracts"
description = "Add pure contract metadata and diagnostics."
complexity = "small"
files = ["crates/surge-core/src/artifact_contract.rs"]

[[spec.subtasks.acceptance_criteria]]
description = "Valid minimal artifacts pass validation."
```

## Minimal Valid Markdown

```markdown
# Spec

## Goal
Enforce artifact contracts before downstream stages consume outputs.

## Subtasks
1. Add pure validators.
   - Acceptance criteria: valid fixtures pass.

## Acceptance Criteria
- [ ] Invalid schema versions fail with stable diagnostics.

## Constraints
- Core validators stay pure and do not read files.
```

## Checklist

- TOML has top-level `schema_version = 1`.
- `[spec]` contains identity, title, description, complexity, and subtasks.
- Each subtask has machine-readable acceptance criteria.
- Markdown has `## Goal`, `## Subtasks`, and `## Acceptance Criteria`.
- Long context belongs in `stories/story-NNN.md`.

## Profile Guidance

Spec Author should report `artifacts_produced = ["spec.toml", "spec.md"]`.
The bundled profile validates both artifacts on `drafted`.
