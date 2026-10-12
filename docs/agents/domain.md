# Domain docs

Surge uses a single domain documentation context across its Rust workspace.

## Before exploring

- Read root `GLOSSARY.md` when present.
- Read relevant decisions in `docs/adr/`.
- Use `docs/ARCHITECTURE.md` for canonical architecture.
- Follow `docs/conventions/adr.md` when creating or editing an ADR.

If the glossary is absent, proceed silently.
Do not create it merely to complete setup; domain-modeling creates
definitions when terminology is resolved.

## Layout

- `GLOSSARY.md`: shared domain vocabulary at the repository root.
- `docs/adr/`: shared architectural decisions.
- `crates/`: implementation of the shared domain.

## Vocabulary

Use glossary terms consistently in issues, proposals, code, and tests.
If a needed concept is missing, reconsider the term or note the gap
for domain-modeling.

## Decision conflicts

Explicitly identify any proposal that contradicts an existing ADR,
cite that ADR, and explain why reconsideration is warranted.
Respect recorded supersession between ADRs.
