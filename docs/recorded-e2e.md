# Recorded end-to-end run (v0.2 M5)

The core Surge thesis is **describe → approve → walk away → return to a PR**.
This page is the reproducible proof: how to drive that loop against a real
public repo with a real agent runtime, plus the CI guard that keeps the script
from rotting.

## CI guard (mock agent, no network)

The scripted onboarding path runs in CI on every change, with the in-process
mock agent so it needs no real runtime or network:

- `crates/surge-cli/tests/examples_smoke.rs::onboarding_smoke_can_init_describe_and_start_example_run`
  drives `surge init` → `surge project describe` → `surge engine run` with
  `SURGE_FORCE_AGENT_MOCK=1`, asserting a run starts and prints its id.

This means the `init → describe → run` plumbing — config discovery, project
scan, graph load, engine start, event-log persistence — can't silently break;
only the live agent + PR legs are exercised by the operator script below.

## Operator script (real repo + real agent)

Prerequisites — the **one** external dependency is an authenticated agent
runtime (Claude Code is the v0.1-validated default; Codex/Gemini are wired and
arg-tested, see [`agent-runtimes.md`](agent-runtimes.md)):

```sh
# 0. One-time: confirm the runtime is reachable and logged in.
surge doctor agent claude-acp                 # dry-run (sandbox matrix)
SURGE_DOCTOR_REAL=1 surge doctor agent claude-acp   # real spawn→handshake→prompt
#   -> "real smoke session: PASS" means spawn + handshake + prompt all work.
#   A FAIL prints the exact stage (spawn / handshake / auth / prompt).

# 1. Onboard the repo.
cd /path/to/your/repo
surge init --default
surge project describe          # writes project.md (captured into runs)

# 2. Describe the task and run it. With a tracker (GitHub Issues / Linear)
#    configured at L3 (`surge:auto`), the daemon will also auto-merge the PR
#    once checks are green + the PR is approved (see tracker-automation.md).
surge engine run --template feature --watch
#   -> prints the run id, streams stage events, ends at a Terminal outcome.

# 3. The result: a branch + PR produced by the agent in an isolated worktree.
#    Inspect or replay the run from its event log at any point:
surge engine replay <run_id>
```

### Recording the proof artifact

To capture a shareable recording (the "recorded" part of this milestone), wrap
step 2 in `asciinema`:

```sh
asciinema rec surge-e2e.cast -c "surge engine run --template feature --watch"
```

Attach `surge-e2e.cast`, the resulting `flow.toml`, and the produced artifacts
(`description.md` / `roadmap.toml`) to the run record.

## Status

- **CI mock guard**: live (`onboarding_smoke`), runs on every change.
- **Real-repo + live-agent run**: operator-run (needs an authenticated runtime;
  cannot run in CI). This script is the reproducible procedure.
- **L3 auto-merge of the resulting PR**: see [`tracker-automation.md`](tracker-automation.md)
  (merge action + readiness gate landed in v0.2 M2).

## Recorded result — 2026-09-29 (real Claude Code, console path)

One unattended `surge bootstrap` from a one-sentence idea ("a small command-line
pomodoro timer in Rust: 25 minute work timer, then a 5 minute break, print a
notice when each ends"), every gate answered `approve`:

| Stage | Outcome |
|---|---|
| `description.md` | Goal, requirements quoted from the idea, explicit *Out of Scope* list |
| `roadmap.toml` | **One** stage (`stages_rationale`: "the timer ships as a single usable release"), 2 milestones, `p0` on the first, dependency M1 → M2 — no padded beta/prod stages |
| `flow.toml` | `multi-milestone` archetype, per-task spec → implement → verify |
| Follow-up run | `RunCompleted`; 5 of 5 tasks and both milestones emitted `TaskVerified` with evidence hashes |
| Result | `cargo test` 4 passed, `cargo clippy -D warnings` clean, `POMODORO_FAST=1 cargo run` prints `Work complete!` / `Break complete!` and exits 0 |

Findings this run produced, all fixed the same day: a profile reasoning floor
that failed sessions on agents without reasoning levels (`990aade`'s parent
`bc78ce8`), console follow-up runs that were not seeded with the approved
artifacts and did not accept the planner's `roadmap_toml` alias (`990aade`), and
an opaque error for a model the account cannot use (`65a30bf`).

Limits of this record: one project, one runtime (Claude Code), verification by the
same runtime (`verifier@2.0`). The cross-vendor verifier could not be exercised
because the Codex account was over its usage limit.
