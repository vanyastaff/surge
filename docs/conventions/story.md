# Story Artifact

Story files carry long-form subtask context that would make `spec.toml` too
large or prose-heavy.

Path pattern: `stories/story-NNN.md`
Validator kind: `story`
Schema version: none; human-readable Markdown contract.

## Minimal Valid Example

```markdown
# Story 001

## Context
The CLI validator needs examples that cover invalid graph shapes.

## What needs to be done
Add fixture-driven validation tests.

## Architecture decisions
Keep pure validation in `surge-core`; compose graph checks in the CLI.

## Files to modify
- `crates/surge-core/src/artifact_contract.rs`

## Acceptance criteria
- Invalid fixtures emit stable diagnostic codes.

## Out of scope
- Live agent comparison tests.
```

## Checklist

- Filename uses three digits, for example `stories/story-001.md`.
- All six required sections are present.
- Acceptance criteria are concrete and testable.
- The story references files or modules without duplicating implementation code.

## Profile Guidance

Spec Author may create story files and reference them from `spec.toml` through
`story_file = "stories/story-NNN.md"`.
