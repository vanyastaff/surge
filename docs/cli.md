[← Getting Started](getting-started.md) · [Back to README](../README.md) · [Workflow →](workflow.md)

# CLI

Surge ships one binary, `surge`, with a clap-derived command tree. This page lists the current command surface, explains the two execution paths that exist today, and maps product intents onto the closest current command (gaps included).

## Command Groups

Current command groups:

```text
surge init              create project-level surge.toml
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
| Initialize a project | `surge init`, then `surge registry detect`, `surge registry add`, or `surge agent add` | `init` is not an interactive wizard yet; sandbox, worktree, approvals, and notification choices are separate / manual. |
| Describe or refresh project context | `surge memory add` and `surge memory search` | No `surge project describe` command yet; repo scanning and stable project context generation are target behavior. |
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

## Target Command Ideas

From the product model in [`docs/ARCHITECTURE.md`](ARCHITECTURE.md), command names are not final while the CLI is being aligned:

```text
surge project describe  create or refresh stable project context
surge task ...          create a focused task run
surge feature ...       amend roadmap with a new feature
```

## See Also

- [Getting Started](getting-started.md) — install Surge and run the first flow
- [Workflow](workflow.md) — how runs are bootstrapped, executed, and logged
- [Architecture](ARCHITECTURE.md) — the canonical architecture document
