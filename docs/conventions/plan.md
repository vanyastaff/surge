# Plan Artifact

Plan artifacts break a bounded implementation into executable tasks. AI Factory
plans live under `.ai-factory/plans/`; generic role plans may use `plan.md`.

Path: `plan.md`  
Validator kind: `plan`  
Schema version: none; human-readable Markdown contract.

## Minimal Valid Example

```markdown
# Implementation Plan

## Settings
- Testing: yes
- Docs: yes

## Tasks
- [ ] Task 1: Add validator fixtures.
- [ ] Task 2: Run focused tests.
```

## Checklist

- `## Settings` names relevant execution policy.
- `## Tasks` contains checkboxes that can be updated as work progresses.
- Tasks are ordered by dependency and small enough to verify.
- Do not use a plan as a substitute for `spec.toml` acceptance criteria.

## Profile Guidance

Use plans for execution control and progress tracking. Use Spec artifacts for
the durable contract of what must be true when work is done.
