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

## Fault-injection harness

The durability claim is exercised by a real fault-injection test rather than
trusted on inspection (v0.2 M4).

**Checkpoint seam.** `engine::run_task` carries a debug-only seam: when
`SURGE_CHECKPOINT_EXIT` names the node being entered, the process aborts
**uncleanly** (`std::process::exit(99)` — no async teardown, no `Drop`, like a
`kill -9`) the instant after that node's `StageEntered` event is durably
committed, before the stage body runs. Release builds compile this to a no-op. The match is pinned by a
pure, unit-tested predicate (`checkpoint_exit_matches`).

**Slice 1 — WAL durability under unclean death** (`surge-cli/tests/fault_injection.rs`):
a real `surge engine run` subprocess is aborted via the seam mid-run, then a
fresh `surge engine replay` folds the surviving on-disk log and asserts it
shows the precise mid-run state (`active node: impl_1`, not terminal). This
proves the committed event was not lost and the WAL log is not corrupt after a
hard process death. Tests use `SURGE_HOME` for an isolated, cross-platform
sandbox (HOME overrides are unreliable on Windows).

**Daemon kill → restart → recover cycle (operator harness).** The full
cross-process cycle is runnable but intentionally **not a CI test** — it spawns
the agent (so it needs the `mock_acp_agent` binary, which isn't built or
resolvable from a downstream crate's test sandbox), and cross-process
spawn/IPC/restart timing is a flake magnet. The recovery *decision policy* is
already exhaustively unit-tested (`decide_action` / `plan_recovery` /
`execute_recovery`), and slice 1 above proves the on-disk durability the cycle
depends on. The building blocks are in place:

- `SURGE_HOME` now relocates the **daemon's** `.surge` tree too (pid / socket /
  version via `pidfile::daemon_dir`, runs / worktrees / intake via
  `surge_runs_dir`), so a harness can isolate a daemon into a sandbox instead
  of racing the real `~/.surge/daemon/`.
- The `SURGE_CHECKPOINT_EXIT` seam makes the kill **deterministic** (self-exit
  at a named node, no kill-timing race).

To run the cycle by hand:

```sh
export SURGE_HOME=$(mktemp -d)/.surge
SURGE_FORCE_AGENT_MOCK=1 SURGE_CHECKPOINT_EXIT=impl_1 surge daemon start   # exits at the checkpoint
surge daemon recover --dry-run    # inspect the recovery decision for the killed run
surge daemon start                # restart -> recover_on_startup resumes it
surge engine replay <run_id>      # fold the log: the run reaches a terminal state
```

**Still a follow-up:** the true power-cut case (OS cache loss), which needs the
per-run WAL on `synchronous = FULL` rather than the current `NORMAL`; `kill -9`
/ `process::exit` (OS survives) is covered by slice 1 + this cycle.
