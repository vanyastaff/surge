# Crash Recovery

> How the daemon survives an unclean exit (crash, `kill -9`, power loss) and
> brings in-flight runs back to life with no AFK regression.

Surge's durability contract: **the per-run SQLite event log is the only
source of truth.** A run's full state is reconstructable by folding its
event log. So recovery is not "restore in-memory state" — it is "re-open
the log, fold to the last cursor, and continue."

## Where it runs

Recovery executes **once at daemon startup**, before the IPC listener
accepts connections (`surge-daemon/src/main.rs` → `recovery::recover_on_startup`).
Resumed runs flow through the same admission controller and broadcast
registry the live server uses, so a recovered run still:

- counts against `max_active`,
- is subscribable via `surge engine watch <run_id> --daemon`,
- publishes `RunFinished` globally — keeping tracker-completion comments
  and the L3 auto-merge gate working for recovered runs.

PID-file and socket staleness is handled separately: `pidfile::acquire_lock`
overwrites a stale PID file, and the server unlinks a stale Unix socket on
bind (`server.rs`, guard `F2`).

## The decision policy

For every run the registry believes was in flight, recovery gathers facts
and applies a pure decision function (`recovery::decide_action`). Candidate
runs are those **not** genuinely terminal — i.e. status in `Running`,
`Bootstrapping`, or `Crashed`. (`Crashed` is `RunStatus::is_terminal()` for
listing purposes but is the *prime* recovery candidate, so the three real
terminal states — `Completed` / `Failed` / `Aborted` — are matched
explicitly.)

`Storage::list_runs` runs **stale-PID detection** first: any
`Running`/`Bootstrapping` row whose recorded daemon PID is no longer alive
is flipped to `Crashed`. After a daemon crash that is exactly the
population recovery scans.

Decision order (first match wins):

| # | Condition | Action |
|---|-----------|--------|
| 1 | Run already active in this engine process | `SkipAlreadyActive` (idempotency) |
| 2 | Registry status `Completed`/`Failed`/`Aborted` | `SkipTerminal` |
| 3 | Event log already reached a terminal event | `ReconcileTerminal { failed }` — update the registry to match |
| 4 | Worktree directory is gone | `MarkFailedWorktreeLost` |
| 5 | No new events for > 24h (`DEFAULT_STUCK_THRESHOLD`) | `FlagStuck { idle_ms }` — human-attention card, no auto-resume |
| 6 | otherwise | `Resume` |

The log-terminal check (3) deliberately precedes the worktree check (4): a
run that genuinely completed has its worktree cleaned up, so an absent
worktree on a finished run is *expected*, not a failure.

Schema-version migration of older event payloads happens transparently
inside the per-run reader/replay path, before the fold.

## Per-stage recovery

`Engine::resume_run` replays the log to the last cursor and resumes the
stage that was interrupted:

- **Agent mid-turn** → the stage re-executes (retry).
- **HumanGate pending** → the gate re-enters and re-emits
  `ApprovalRequested`. Deduplication against still-open Telegram cards is
  the cockpit recovery reconciler's job (card-id correlation), so a
  re-emitted approval reuses the existing card rather than spamming a new
  one.
- **Notify mid-flight** → retried.
- **Terminal not yet appended** → appended on stage completion.

The recovery decision carries the run's last `StageEntered` node
(`active_node`) so operators get a per-stage view of where crashed runs
were ("which stage were they at?").

## Telemetry

Every pass logs a `surge.recovery` summary: `scanned`, `resumed`,
`reconciled`, `failed_worktree`, `flagged_stuck`, `skipped`, `errors`. Each
resumed run logs its prior status and stage.

## CLI

```shell
surge daemon recover --dry-run     # preview decisions, no side effects
surge daemon recover               # apply registry-safe reconciliations
                                   # (only while the daemon is stopped)
```

`--dry-run` is the read-only inspector — run it any time to see what
recovery would do. Plain `recover`:

- refuses to mutate anything if a daemon is already running (recovery is
  that daemon's job),
- otherwise applies the **registry-safe** actions standalone
  (`MarkFailedWorktreeLost` → `Failed`, `ReconcileTerminal` → matching
  terminal status) and reports `Resume` decisions as pending the next
  `surge daemon start` (resuming requires the live engine).

## Idempotency

Re-running recovery is a no-op:

- runs marked `Failed`/`Completed` by a prior pass are now genuinely
  terminal and filtered out of the candidate set;
- runs already live in-process resolve to `SkipAlreadyActive`;
- `Engine::resume_run` itself guards with `RunAlreadyActive` and short-
  circuits runs whose log is already terminal.

## Deferred

- **`kill -9` / power-cut fault-injection harness.** WAL durability is
  provided by SQLite WAL mode (configured in the per-run pragmas) and the
  resume-from-log path is covered by integration tests; a dedicated
  process-kill harness that asserts checkpoint behavior under hard kill is
  a follow-up.
