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
surge spec ...          manage legacy specs
surge run ...           execute the legacy spec pipeline
surge bootstrap ...     generate an adaptive flow from a free-form prompt
surge engine ...        execute flow.toml graphs
surge daemon ...        manage the long-running local engine host
surge clean             clean up orphaned worktrees and merged branches
surge worktrees         list active worktrees
surge analytics ...     view token/cost analytics
```

## Two Execution Surfaces

Two execution paths coexist while the new graph engine stabilizes:

- **Legacy spec pipeline:** `surge spec ...` + `surge run <spec_id>`. Creates static task plans from templates and runs them through planner / coder / reviewer-style stages.
- **Bootstrap path:** `surge bootstrap "<prompt>"`. Runs Description Author → Roadmap Planner → Flow Generator, asks for console approvals, then starts the materialized follow-up graph.
- **Graph engine:** `surge engine run <flow.toml>` or `surge engine run --template <name>`. Executes an already-authored or bundled workflow graph with explicit nodes, declared outcomes, and edges.

The legacy pipeline is preserved while the flow engine catches up; new work targets the graph engine.

## Current Spec Templates

`surge spec create` ships these templates today:

`feature`, `bugfix`, `refactor`, `performance`, `security`, `docs`, `migration`.

## Current → Target Mapping

The product model in [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) describes a richer surface than the current CLI exposes. This table shows the closest current command for each product intent and the gap that remains.

| Product intent | Current closest command | Gap |
|---|---|---|
| Initialize a project | `surge init --default` or interactive `surge init`, then `surge registry detect`, `surge registry add`, or `surge agent add` if needed | Richer desktop/Telegram onboarding is still target UX. |
| Describe or refresh project context | `surge project describe`, optionally with `--dry-run`, `--refresh`, `--output <path>`, or `--author-mode <auto\|agent\|deterministic>` | `auto` uses Project Context Author through ACP when its runtime is installed, otherwise falls back to deterministic local context. |
| Create a focused feature/task run | `surge bootstrap "..."` or `surge engine run --template single-task --watch` | Bootstrap is now available; richer daemon/Telegram approval UX is still target behavior. |
| Run a full roadmap/flow | `surge bootstrap "..."` or manually create `flow.toml`, then `surge engine run <flow.toml> --watch` | Bootstrap generates roadmap and flow; tracker intake and richer approval channels are still target UX. |
| Amend an existing roadmap with a new feature | Create another spec with `surge spec create ...` or edit roadmap/flow files manually | No `surge feature` command yet that inserts work into a roadmap and wakes the runner. |
| Run AFK through a daemon | `surge daemon start` and `surge engine run <flow.toml> --daemon --watch` | Daemon exists; the full Telegram approval bot and tracker intake loop are still target UX. |
| Start from GitHub Issues or Linear | No direct CLI equivalent | GitHub / Linear issue intake should normalize tracker payloads into the same bootstrap path; not a user-facing command yet. |

## Bootstrap And Template Skip

`surge bootstrap "<prompt>"` is the adaptive path. It creates the description, roadmap, and flow artifacts through the bundled bootstrap graph, then starts the generated follow-up run with those artifacts inherited.

`surge bootstrap resume <run_id>` resumes a cleanly interrupted bootstrap run and then starts the same materialized follow-up graph once the bootstrap event log is complete.

`surge engine run <flow.toml>` and `surge engine run --template <name>` both skip bootstrap. `SPEC_PATH` and `--template` are mutually exclusive; use a path for a custom graph, or a template name such as `linear-3`, `linear-with-review`, `multi-milestone`, `bug-fix`, `refactor`, `spike`, or `single-task`.

Bundled templates live in the binary; user templates under `${SURGE_HOME}/templates/*.toml` shadow bundled templates by filename stem or `metadata.name`.

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

## Target Command Ideas

From the product model in [`docs/ARCHITECTURE.md`](ARCHITECTURE.md), command names are not final while the CLI is being aligned:

```text
surge task ...          create a focused task run
surge feature ...       amend roadmap with a new feature
```

## See Also

- [Getting Started](getting-started.md) — install Surge and run the first flow
- [Workflow](workflow.md) — how runs are bootstrapped, executed, and logged
- [Architecture](ARCHITECTURE.md) — the canonical architecture document
