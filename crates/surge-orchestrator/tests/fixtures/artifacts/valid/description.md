# Description

## Goal
Add artifact validation before downstream stages consume generated outputs.

## Context
Surge already has bundled profiles, hooks, and graph validation.

## Requirements
- Validate canonical artifacts with stable diagnostics.
- Reject malformed outputs before persistence.

## Out of Scope
- Removing the legacy spec pipeline.
