# Artifact Conventions

Surge owns the shape of role artifacts so agents can reason freely while the
engine, validators, hooks, and future UI surfaces can treat outputs
consistently. The source of truth in code is `surge_core::artifact_contract`.

Validate any artifact with:

```bash
surge artifact validate --kind <kind> <path>
```

Validator hooks run from the run worktree. Successful validation is logged at
DEBUG by hook/CLI callers; failures are surfaced as short diagnostics and
profile-level `on_outcome` hooks reject the outcome before it is persisted.

| Kind | Canonical artifact | Primary format | Compatibility artifact | Guide |
|---|---|---|---|---|
| Description | `description.md` | Markdown | none | [Description](description.md) |
| Requirements | `requirements.md` | Markdown | none | [Requirements](requirements.md) |
| Roadmap | `roadmap.toml` | TOML | `roadmap.md` | [Roadmap](roadmap.md) |
| Spec | `spec.toml` | TOML | `spec.md` | [Spec](spec.md) |
| ADR | `docs/adr/<NNNN>-<slug>.md` | Markdown | none | [ADR](adr.md) |
| Story | `stories/story-NNN.md` | Markdown | none | [Story](story.md) |
| Plan | `plan.md` | Markdown | none | [Plan](plan.md) |
| Flow | `flow.toml` | TOML graph | none | [Flow](flow.md) |

## Authoring Rules

- Use schema version `1` for TOML artifacts owned by the artifact contract.
- Use `schema_version = 1` for `flow.toml`; that version is owned by the graph schema.
- Keep Markdown headings stable. Validators match required section headings.
- Keep artifact paths relative to the run worktree.
- Do not include secrets or full logs in validation diagnostics.
