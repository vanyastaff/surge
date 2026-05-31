# Live budget enforcement

Surge can stop a run before it overruns a spend budget — the safety rail that
makes "walk away" trustworthy for unattended (AFK) work. Enforcement is
**opt-in**: with no budget configured, runs are unlimited and the engine skips
the check entirely (zero overhead, no event-log churn).

## Configuring a budget

Budgets live in the `[analytics]` section of `surge.toml`:

```toml
[analytics]
budget_usd = 5.0          # hard USD ceiling (omit for no USD limit)
budget_tokens = 2_000_000 # hard token ceiling (prompt + output)
budget_warn_threshold = 80 # warn once spend reaches this % of a limit (0 disables)
```

The CLI freezes these into the run at start, so enforcement is deterministic
and **replayable**: the same event log always reproduces the same budget
decisions (no wall-clock, no live config re-read mid-run).

> Note: USD enforcement requires per-event `cost_usd`, which the current ACP
> bridge path does not yet populate (`cost_usd` is `None`). Until a pricing
> layer lands, set `budget_tokens` for hard enforcement; `budget_usd` still
> drives the analytics surfaces.

## How it works

At every **stage boundary** the engine has just folded the completed stage's
token usage (`TokensConsumed` events) into the run's cumulative cost. It then
evaluates that cost against the resolved limits:

| Verdict    | Condition                                              |
|------------|--------------------------------------------------------|
| `Ok`       | within all limits                                       |
| `Warn`     | reached `budget_warn_threshold`% of a limit, not over   |
| `Exceeded` | reached or passed a limit (USD checked before tokens)   |

The verdict maps to an action via the run's **policy**:

| Policy     | on `Warn`                  | on `Exceeded`                          |
|------------|----------------------------|----------------------------------------|
| `Abort` (default) | emit `BudgetWarningRaised` once | emit `BudgetExceeded`, then `RunAborted` |
| `WarnOnly` | emit `BudgetWarningRaised` once | emit a one-time warning, keep running   |

`Abort` is the default — an unattended run never overruns its budget. The warn
band fires **at most once per run** (idempotent across re-evaluation).

## Events

Both decisions are recorded in the per-run event log, so they show up in
`surge engine replay`, `surge engine logs`, and the cockpit status surfaces:

- `BudgetWarningRaised { dimension, pct, cost_usd, total_tokens }`
- `BudgetExceeded { dimension, cost_usd, total_tokens }`

On `Abort`, a `RunAborted` event follows with a reason citing the dimension and
the accrued cost.

## Scope

- Limits resolve global → run today; per-milestone overrides are a planned
  extension (the resolution point is centralized in `AnalyticsConfig::budget_guard`).
- A dedicated Telegram budget card (warn/exceeded with raise/abort buttons) is a
  follow-up; the underlying events already surface through the event log.
