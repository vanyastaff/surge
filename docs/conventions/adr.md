# ADR Artifact

Architect writes ADRs for decisions that affect multiple modules, public
contracts, or future work.

Path pattern: `docs/adr/<NNNN>-<slug>.md`
Validator kind: `adr`
Schema version: none; TOML frontmatter plus Markdown body.

## Minimal Valid Example

```markdown
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
- Keep conventions in prompts only; rejected because prompts drift.
- Validate only in CI; rejected because bad outputs would already affect runs.

## Consequences
Generated artifacts can be rejected before persistence, but profile authors must keep hooks current.
```

## Checklist

- Path starts with `docs/adr/` and uses a four-digit number.
- TOML frontmatter is delimited by `+++`.
- Frontmatter includes `status`, `deciders`, and `date`.
- Body has `## Status`, `## Context`, `## Decision`, `## Alternatives considered`, and `## Consequences`.

## Profile Guidance

Architect should report `drafted` only when it writes an ADR. If no durable
decision is needed, report `no_decision_needed` and do not create a placeholder.
