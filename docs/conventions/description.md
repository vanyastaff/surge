# Description Artifact

`description.md` is the first bootstrap artifact. It turns the user's request
into stable context for planners and generators.

Path: `description.md`  
Validator kind: `description`  
Schema version: none; human-readable Markdown contract.

## Minimal Valid Example

```markdown
# Description

## Goal
Add artifact validators so generated role outputs can be checked before the run advances.

## Context
Surge already has bundled profiles, hooks, and graph validation.

## Requirements
- Validate malformed outputs before downstream stages consume them.
- Keep diagnostics short and safe to show in operator surfaces.

## Out of Scope
- Removing legacy `surge-spec`.
```

## Checklist

- `## Goal` explains the what and why in one short paragraph.
- `## Context` names the existing project surface touched by the request.
- `## Requirements` uses testable bullets.
- `## Out of Scope` states what will not be done.

## Profile Guidance

Description Author should write only `description.md` and report
`artifacts_produced = ["description.md"]`. Its bundled `on_outcome` hook rejects
`drafted` when this contract fails.
