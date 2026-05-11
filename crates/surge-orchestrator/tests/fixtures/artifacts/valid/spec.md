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
