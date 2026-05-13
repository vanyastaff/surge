[← Getting Started](getting-started.md) · [Back to README](../README.md) · [Workflow →](workflow.md)

# CLI

Surge ships one binary, `surge`, with a clap-derived command tree. This page lists the current command surface, explains the two execution paths that exist today, and maps product intents onto the closest current command (gaps included).

## Command Groups

Current command groups:

```text
surge init              create or update project-level surge.toml
surge project describe  create or refresh stable project.md context
surge agent ...         manage configured agents
surge registry ...      inspect built-in/remote ACP agent registry
surge bootstrap ...     generate an adaptive flow from a free-form prompt
surge engine ...        execute flow.toml graphs
surge feature ...       draft, approve, list, show, or reject roadmap amendments
surge artifact ...      validate generated artifacts against Surge contracts
surge migrate-spec ...  translate a legacy .spec.toml into a flow.toml
surge daemon ...        manage the long-running local engine host
surge tracker ...       list configured task sources, test connectivity
surge intake ...        inspect tracker-intake state (ticket index)
surge clean             clean up orphaned worktrees and merged branches
surge worktrees         list active worktrees
surge analytics ...     view token/cost analytics
```

## Two Execution Surfaces

- **Bootstrap path:** `surge bootstrap "<prompt>"`. Runs Description Author → Roadmap Planner → Flow Generator, asks for console approvals, then starts the materialized follow-up graph.
- **Graph engine:** `surge engine run <flow.toml>` or `surge engine run --template <name>`. Executes an already-authored or bundled workflow graph with explicit nodes, declared outcomes, and edges.

The structured-spec pipeline (`surge spec`, `surge run <spec_id>`, etc.) was retired in v0.1 — see [`migrate-spec-to-flow.md`](migrate-spec-to-flow.md) for the auto-translator and the [`legacy-parity-checklist`](legacy-parity-checklist.md) for the module-by-module replacement mapping.

## Current → Target Mapping

The product model in [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) describes a richer surface than the current CLI exposes. This table shows the closest current command for each product intent and the gap that remains.

| Product intent | Current closest command | Gap |
|---|---|---|
| Initialize a project | `surge init --default` or interactive `surge init`, then `surge registry detect`, `surge registry add`, or `surge agent add` if needed | Richer desktop/Telegram onboarding is still target UX. |
| Describe or refresh project context | `surge project describe`, optionally with `--dry-run`, `--refresh`, `--output <path>`, or `--author-mode <auto\|agent\|deterministic>` | `auto` uses Project Context Author through ACP when its runtime is installed, otherwise falls back to deterministic local context. |
| Create a focused feature/task run | `surge bootstrap "..."` or `surge engine run --template single-task --watch` | Bootstrap is now available; richer daemon/Telegram approval UX is still target behavior. |
| Run a full roadmap/flow | `surge bootstrap "..."` or manually create `flow.toml`, then `surge engine run <flow.toml> --watch` | Bootstrap generates roadmap and flow; tracker intake and richer approval channels are still target UX. |
| Amend an existing roadmap with a new feature | `surge feature describe "..." --project` or `surge feature describe "..." --run <run_id>` | Active-run pickup is stored through amendment events; Telegram rich cards are still target UX. |
| Run AFK through a daemon | `surge daemon start` and `surge engine run <flow.toml> --daemon --watch` | Daemon exists; the full Telegram approval bot and tracker intake loop are still target UX. |
| Start from GitHub Issues or Linear | No direct CLI equivalent | GitHub / Linear issue intake should normalize tracker payloads into the same bootstrap path; not a user-facing command yet. |

## Bootstrap And Template Skip

`surge bootstrap "<prompt>"` is the adaptive path. It creates the description, roadmap, and flow artifacts through the bundled bootstrap graph, then starts the generated follow-up run with those artifacts inherited.

`surge bootstrap resume <run_id>` resumes a cleanly interrupted bootstrap run and then starts the same materialized follow-up graph once the bootstrap event log is complete.

`surge engine run <flow.toml>` and `surge engine run --template <name>` both skip bootstrap. `SPEC_PATH` and `--template` are mutually exclusive; use a path for a custom graph, or a template name such as `linear-3`, `linear-with-review`, `multi-milestone`, `bug-fix`, `refactor`, `spike`, or `single-task`.

Bundled templates live in the binary; user templates under `${SURGE_HOME}/templates/*.toml` shadow bundled templates by filename stem or `metadata.name`.

## Artifact Validation

Generated role artifacts can be validated directly:

```text
surge artifact validate --kind description description.md
surge artifact validate --kind roadmap roadmap.toml
surge artifact validate --kind flow flow.toml
```

Use `--format json` for structured diagnostics. The same validator surface is
used by bundled profile `on_outcome` hooks for description and roadmap
artifacts. See [Artifact Conventions](conventions/README.md) for every
canonical path and minimal example.

## Roadmap Amendments

`surge feature` is the current roadmap-amendment surface. It asks the bundled Feature Planner profile for a `roadmap-patch.toml`, stores the draft in the run artifact store, records lifecycle events, and mirrors patch metadata into the registry index for quick lookup.

```text
surge feature describe "add CSV export" --project
surge feature describe "add CSV export" --run <run_id> --approval prompt
surge feature describe "add CSV export" --run <run_id> --approval approve --conflict-choice create-follow-up-run
surge feature list --status pending-approval
surge feature show rpatch-...
surge feature reject rpatch-... --reason "out of scope"
```

Approval modes are `prompt`, `approve`, `reject`, and `store`. When a patch conflicts with already-running or terminal roadmap history, choose one conflict resolution: `defer-to-next-milestone`, `abort-current-run`, `create-follow-up-run`, or `reject-patch`. The selected choice is persisted in `RoadmapPatchApprovalDecided` and in the registry row so `feature show` can report it later.

For debugging, set `RUST_LOG=feature_cli=debug,roadmap_amendment=debug,roadmap_patch_index=debug`. Conflict detection logs stable conflict codes at WARN level; approval, apply, follow-up, and registry transitions log at INFO.

## Project Initialization And Context

`surge init --default` is the non-interactive setup path. It writes a complete validated `surge.toml`, chooses the best detected ACP registry agent when possible, and keeps an existing config unchanged. `surge init` without flags enters the wizard and can update onboarding sections in an existing config while preserving unrelated TOML content.

`surge project describe` writes `project.md` at `init.project_context_path` by default. The scanner uses deterministic path ordering, byte budgets, skipped-file summaries, git state, and redaction for token/password/API-key/chat-id-like values. New runs started by bootstrap, `engine run`, daemon requests, or inbox intake capture the current `project.md` as a `project_context` artifact at run start.

Use:

```text
surge project describe --dry-run
surge project describe --refresh
surge project describe --output docs/project-context.md
surge project describe --author-mode agent
surge project describe --author-mode deterministic
```

Set `RUST_LOG=surge=debug` when diagnosing agent detection, config section updates, or project-context scan decisions.

## Tracker Intake

Two command groups expose the tracker-source side. `surge tracker` deals with **configuration** (what's wired up in `surge.toml`); `surge intake` deals with **observed state** (what the daemon's ticket index currently looks like).

```text
surge tracker list                        # task sources configured in surge.toml
surge tracker test <source-id>            # liveness probe — list_open_tasks() against the provider
surge intake list                         # ticket index, newest first (table or JSON)
surge intake list --tracker <source-id>   # filter by source
surge intake list --format json           # stable JSON, pipeable into `jq`
surge intake list --limit 200             # cap (hard ceiling 1000)
```

`surge intake list` columns: `SOURCE | TASK | STATE | PRIO | RUN | LAST SEEN`. State is the `ticket_index` FSM value (`Seen`, `Triaged`, `InboxNotified`, `RunStarted`, `Active`, `Completed`, `Failed`, `Aborted`, `Skipped`, …). See [tracker-automation.md](tracker-automation.md) for tier semantics (L0–L3) and the label conventions that drive them.

## Target Command Ideas

From the product model in [`docs/ARCHITECTURE.md`](ARCHITECTURE.md), command names are not final while the CLI is being aligned:

```text
surge task ...          create a focused task run
```

## See Also

- [Getting Started](getting-started.md) — install Surge and run the first flow
- [Tracker automation](tracker-automation.md) — tier labels (L0–L3) and intake inspection
- [Artifact Conventions](conventions/README.md) — generated artifact contracts and validator examples
- [Workflow](workflow.md) — how runs are bootstrapped, executed, and logged
- [Architecture](ARCHITECTURE.md) — the canonical architecture document
