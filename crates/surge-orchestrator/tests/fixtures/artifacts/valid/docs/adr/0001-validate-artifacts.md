+++
status = "accepted"
deciders = ["core maintainers"]
date = "2026-05-11"
+++

# ADR 0001: Validate Generated Artifacts

## Status
Accepted.

## Context
Agent outputs need stable shapes before downstream stages consume them.

## Decision
Use Surge-owned artifact contracts with pure validators and hook enforcement.

## Alternatives considered
- Keep conventions in prompts only.
- Validate only in CI.

## Consequences
Generated artifacts can be rejected before persistence.
