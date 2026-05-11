# Story 001

## Context
The CLI validator needs examples that cover invalid graph shapes.

## What needs to be done
Add fixture-driven validation tests.

## Architecture decisions
Keep pure validation in `surge-core`.

## Files to modify
- `crates/surge-core/src/artifact_contract.rs`

## Acceptance criteria
- Invalid fixtures emit stable diagnostic codes.

## Out of scope
- Live agent comparison tests.
