# Project Memory (`.surge/memory/`)

`.surge/memory/` is repo-resident, git-committable **accumulating project
memory**: short markdown notes recording durable invariants and hard-won lessons
that should carry across runs. It complements the one-shot `project.md`
(`project_context`) — memory grows over time and is versioned with the code.

Location: `.surge/memory/*.md` in the project root.
Optional index: `.surge/memory/MEMORY.md` (human-facing; skipped by the seed).

## Read path — seeded into every run

At run start the engine loads `.surge/memory/*.md` (sorted, `MEMORY.md` and
empty notes skipped) into one `project_memory` run artifact, concatenated under
`## <filename>` headers. Any agent node that binds `project_memory` reads the
accumulated knowledge alongside its spec. The bundled `implementer` profiles
bind it optionally, so a project with no memory directory is a no-op.

## Write path — agents accumulate notes

When an agent learns something durable (a gotcha, an architectural constraint, a
fix that must not regress), it writes a short note to `.surge/memory/<topic>.md`
in its worktree and declares that path in `artifacts_produced`. The engine
stamps the note in place with a provenance comment:

```markdown
<!-- surge:memory run=<run_id> node=<node_key> -->
# Auth invariant
Tokens are always HS256; the verifier rejects RS256.
```

The stamp is an HTML comment (invisible in rendered markdown), idempotent across
retries/replay, and travels with the note into the run's diff — so merging the
run's branch commits the memory alongside the code.

## Guidance

- Keep notes **small and specific**. One invariant or lesson per note; do not
  restate the spec or roadmap.
- Name notes by topic (`auth.md`, `migrations.md`), not by run.
- Prune notes that turn out to be wrong — stale memory misleads future runs.
  (A `surge memory audit` correlating notes with failed runs is a planned
  follow-up.)
