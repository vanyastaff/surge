# CI Optimization — nebula-style split

**Status:** draft (under review)
**Date:** 2026-05-05
**Scope:** `.github/workflows/ci.yml` (rewrite), `.github/workflows/release.yml` (env hardening), new `.github/workflows/cross-platform.yml`, workspace MSRV pin (already applied), workspace clippy debt cleanup.
**Goal:** fast ubuntu-only feedback on every PR (~2 min), with cross-OS coverage preserved as a separate workflow that triggers on OS-sensitive paths and main-branch pushes. Architecture mirrors the user's other project (nebula).

---

## 1. Current state

`.github/workflows/ci.yml` runs three matrix jobs on `ubuntu-latest`, `windows-latest`, `macos-latest`:

| Job | Cache | Runner | Wall (warm) |
|---|---|---|---|
| Test Suite (windows) | `actions/cache@v4`, separate `target-test-` key | `cargo test --workspace --exclude surge-ui --verbose` | **7m 35s** |
| Test Suite (ubuntu) | same | same | 5m 06s |
| Test Suite (macos) | same | same | 4m 17s |
| Clippy (windows) | `actions/cache@v4`, separate `target-clippy-` key | two clippy invocations (`-p surge-core`, then `-p surge-acp`, then permissive) | 4m 32s |
| Clippy (macos) | same | same | 1m 46s |
| Clippy (ubuntu) | same | same | 1m 57s |
| Format Check | none | `cargo fmt --check` | 16s |

Plus one ubuntu-only step that builds `mock_acp_agent` and runs `surge-orchestrator` ignored tests with `continue-on-error: true` (M5.1 known limitation, tracked separately).

**Observed bottlenecks**

- **Multi-OS on every PR.** Every PR pays the windows cost (~7-8 min wall time) regardless of whether the change is OS-sensitive. Most PRs to surge change pure-Rust logic that ubuntu fully covers.
- **Cache duplication.** test and clippy jobs on the same OS use *different* cache keys (`target-test-…` vs `target-clippy-…`), so each rebuilds dependencies independently. ~50% of clippy job wall time is just compiling deps that the test job already compiled.
- **Plain `cargo test`** is single-process; nextest typically gives ~2× by per-test process parallelism.
- **Two strict clippy invocations** (`-p surge-core` then `-p surge-acp`) compile the dependency graph twice instead of once.
- **No PR cancellation.** Pushing twice to the same PR runs two CI builds in parallel; the older one keeps burning runners until it finishes.
- **`CARGO_INCREMENTAL=1` (default).** Incremental on CI is wasted: the incremental directory never gets reused across runs but still slows down linking and bloats the cache.
- **44 workspace clippy warnings.** Strict clippy (`-D warnings`) currently runs only on `surge-core` + `surge-acp`; everything else is permissive. After the MSRV bump to 1.95, fresh clippy lints surfaced 24 collapsible-if/let-chain candidates and 9 readability `Duration` warnings on the rest of the workspace. Most autofix.

## 2. Reference architectures (May 2026)

Surveyed five Rust projects to ground the design in real-world practice rather than theory:

| Project | Test runner | Cache | sccache | Multi-OS strategy |
|---|---|---|---|---|
| **nebula** (user's other project) | `cargo-nextest` | `Swatinem/rust-cache@v2.9.1` | no | **main CI ubuntu-only**; separate `cross-platform.yml` for selected crates |
| Tokio | nextest | `Swatinem/rust-cache@v2` | no | 3-OS matrix on every PR |
| rust-analyzer | nextest | disabled (intentional) | no | 3-OS on PR |
| Bevy | custom `tools/ci` binary | restore-only `actions/cache` | no | 3-OS on PR |
| ripgrep | cargo | none | no | wide cross-compile matrix |

**Five takeaways drive the design:**

1. **None of them run sccache.** Adds setup overhead and Windows flakiness without a clear win on top of `rust-cache` for normal dependency churn. Reject for surge.
2. **nextest is the de-facto standard** in actively-developed projects (Tokio, rust-analyzer, nebula). Adopt.
3. **`CARGO_INCREMENTAL=0`** is rust-analyzer's pattern with a clear rationale: incremental output is per-run on CI, so it just costs link time and cache bloat. Adopt.
4. **nebula splits OS coverage from main CI.** Main `ci.yml` is ubuntu-only (~2 min); a separate `cross-platform.yml` runs 3-OS smoke on a path-filtered trigger. Surge has the same shape (most logic is OS-portable; only `surge-acp`, `surge-mcp`, `surge-daemon` touch OS-specific subprocess/IPC code) — adopt.
5. **nebula uses a `required` aggregator job.** Branch-protection points at one job (`required`) that depends on all the real ones. Add or remove a sub-job and branch protection doesn't need touching. Adopt.

## 3. Design

### 3.1 Architecture overview

```
┌─────────────────────────────────────────────────────────────────────┐
│ workflow scope env (all three workflows)                            │
│   CARGO_TERM_COLOR=always                                           │
│   CARGO_INCREMENTAL=0      # incremental wasted on CI               │
│   CARGO_NET_RETRY=10       # flaky-network resilience               │
│   RUSTUP_MAX_RETRIES=10    #                                        │
│   RUST_BACKTRACE=short     # smaller logs, still useful             │
└─────────────────────────────────────────────────────────────────────┘

ci.yml — fast PR feedback (ubuntu-only)
  triggers: push to main/develop, pull_request, merge_group
  concurrency: group per (workflow, pr#||sha), cancel-in-progress
  jobs:
    fmt        ─┐
    clippy      │
    test        ├── all run on ubuntu-latest, share cache key surge-ubuntu
    doctests    │
    check       ─┘
    required   ── aggregator, depends on all above

cross-platform.yml — OS coverage (windows + macos + ubuntu smoke)
  triggers:
    - push to main (after merge)
    - pull_request with paths matching crates/surge-acp/**, surge-mcp/**, surge-daemon/**
    - merge_group
    - schedule: weekly Mon 03:00 UTC (sanity)
    - workflow_dispatch (manual)
  concurrency: group per (workflow, ref), cancel-in-progress
  jobs:
    smoke (matrix: ubuntu | windows | macos)
      cargo build + cargo nextest run --workspace --exclude surge-ui
    required-cross  ── aggregator

release.yml — release binaries (unchanged trigger; env hardening)
  triggers: tag v*, workflow_dispatch
  jobs: build-release × 4 targets, create-release
  changes: add Swatinem/rust-cache@v2, add env hardening, --locked
```

### 3.2 ci.yml (ubuntu-only main)

Single-OS workflow that gives every PR a full signal in ~2 minutes. Five real jobs plus one aggregator:

- **fmt** — `cargo fmt --all -- --check` (timeout 5 min). Fastest possible fail signal.
- **clippy** — `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` (timeout 15 min). **Strict on the entire workspace** after the warning-cleanup commit (see §3.6). One invocation, one graph compile.
- **test** — `cargo nextest run --workspace --exclude surge-ui --locked` (timeout 15 min). Includes the ubuntu-only `mock_acp_agent` build + `--run-ignored=ignored-only` for surge-orchestrator engine integration tests, kept with `continue-on-error: true` per the existing M5.1 limitation.
- **doctests** — `cargo test --doc --workspace --exclude surge-ui --locked` (timeout 15 min). Separate job because nextest doesn't run doctests.
- **check** — `cargo check --workspace --all-targets --all-features --locked` (timeout 10 min). Catches cases where `--all-features` fails to compile but the default build was fine.
- **required** — empty job that lists `needs: [fmt, clippy, test, doctests, check]`. Branch protection requires this single job; under the hood it gates on all five.

`surge-ui` is excluded from `test` and `doctests` (no automated tests today; gpui is a heavy dependency that we don't want to compile a test binary against). It **is** kept in `clippy` and `check` coverage — the crate is currently warning-free, and including it prevents new debt from accumulating silently. If gpui compile time ever dominates clippy/check wall time, revisit.

### 3.3 cross-platform.yml (separate workflow, 3 OS smoke)

Triggers itself only when OS-sensitive code might have changed:

```yaml
on:
  push:
    branches: [main]
  pull_request:
    paths:
      - 'crates/surge-acp/**'      # subprocess + ACP transport
      - 'crates/surge-mcp/**'      # interprocess sockets
      - 'crates/surge-daemon/**'   # IPC, process lifecycle
      - 'Cargo.lock'               # any dep change
      - 'Cargo.toml'               # workspace lints/profiles, MSRV pin
      - 'rust-toolchain.toml'      # toolchain bump may surface OS-specific
      - '.github/workflows/cross-platform.yml'
  merge_group:
  schedule:
    - cron: '0 3 * * 1'            # weekly Monday 03:00 UTC
  workflow_dispatch:
```

Single matrix job: `cargo build --workspace --exclude surge-ui --locked` then `cargo nextest run --workspace --exclude surge-ui --locked --no-fail-fast`. No clippy/fmt — those are already enforced on ubuntu in the main CI; running them again on Windows just doubles the cost without adding signal. `fail-fast: false` so windows-failing doesn't hide a macos failure (or vice versa).

A path-filtered PR (e.g. `docs/**` only) won't trigger this workflow at all — fast feedback for non-OS-sensitive changes. The weekly cron + main-push triggers guarantee we still notice if a transitively-pulled dep breaks Windows on a code path we don't touch directly.

### 3.4 release.yml (env hardening)

Drop manual `actions/cache@v4` blocks (separate per target = no sharing) in favour of `Swatinem/rust-cache@v2` keyed on `release-${{ matrix.target }}`. Add the workflow-scope env from §3.1. Add `--locked` to the build command. Toolchain pin already done (`@1.95.0`). No nextest — release builds don't run tests.

### 3.5 Caching strategy

Single action across all workflows: `Swatinem/rust-cache@v2.9.1` (pinned version, supply-chain hardening — copied from nebula). Configuration:

```yaml
- uses: Swatinem/rust-cache@v2.9.1
  with:
    shared-key: surge-${{ matrix.os || 'ubuntu' }}
    save-if: ${{ github.ref == 'refs/heads/main' }}
    cache-on-failure: true
```

- `shared-key: surge-ubuntu` is identical across all five jobs in `ci.yml` so they share warm `target/`. Without this, fmt/clippy/test/doctests/check each compile dependencies separately.
- `cross-platform.yml` uses `surge-${{ matrix.os }}` per OS.
- `save-if: ref == main` means PR builds and `develop` builds only restore; only main writes cache. Prevents PR-poisoning of the canonical cache.
- `cache-on-failure: true` so a failed run still saves cache — the next retry hits warm `target/`.

### 3.6 Strict workspace clippy + warning cleanup

Pre-step (own commit, before the workflow rewrite): `cargo clippy --fix --workspace --all-targets --all-features` autofixes ~40 of 44 warnings (let-chains from MSRV-1.95 lint, `Duration` readability). The remaining ~4 fix manually. Verify: `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` exits 0.

After this, the `clippy` job in `ci.yml` becomes a single strict workspace invocation. The "permissive" / "strict" split disappears — there's no zombie permissive step that nobody reads.

### 3.7 Concurrency & required aggregator

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.sha }}
  cancel-in-progress: true
```

PR-level group key (PR number) — repeated pushes to the same PR cancel the older run. Main/develop/merge_group runs use SHA so each commit completes independently.

`required` aggregator job:

```yaml
required:
  name: required
  runs-on: ubuntu-latest
  needs: [fmt, clippy, test, doctests, check]
  if: always()
  steps:
    - run: |
        results='${{ toJSON(needs) }}'
        echo "$results" | jq -e 'all(.[]; .result == "success")' > /dev/null
```

Set this single job as required in branch protection. Adding or removing a sub-job in `ci.yml` no longer requires touching branch-protection settings.

### 3.8 Env hardening (workflow scope)

Adopted from nebula:

```yaml
env:
  CARGO_TERM_COLOR: always
  CARGO_INCREMENTAL: 0      # don't waste link time on per-run incremental
  CARGO_NET_RETRY: 10       # GH runner DNS / crates.io flakes
  RUSTUP_MAX_RETRIES: 10
  RUST_BACKTRACE: short     # smaller log output, still useful for debugging
```

Same env block in all three workflows.

### 3.9 Toolchain

Already done in the MSRV bump that preceded this work: `rust-toolchain.toml` pins `1.95.0`. The CI references `dtolnay/rust-toolchain@1.95.0`. To bump (every 1–2 stable releases): edit `rust-toolchain.toml` and the version in three workflow files (one line each).

Action versions are pinned by version tag (`@1.95.0`, `@v2.9.1`, `@v4`) rather than commit SHA. This is one tier weaker than nebula's SHA pinning but materially safer than floating tags (`@stable`, `@v2`). Acceptable for surge's threat model; can be tightened later.

## 4. Expected impact

Estimates based on what these changes did in nebula and similar-sized Rust projects (rough — first real run will give the actual numbers):

| Metric | Before | After (estimate) |
|---|---|---|
| Wall time on a typical PR (no OS-sensitive paths) | ~8 min (3-OS bottlenecked by windows) | **~2 min (ubuntu-only)** |
| Wall time on a PR touching surge-acp/mcp/daemon | ~8 min | ~2 min ubuntu (fast feedback) + ~5-6 min cross-platform in parallel |
| Wall time on push to main | ~8 min | ~2 min ubuntu + ~5-6 min cross-platform in parallel |
| Clippy job runtime | ~2-4 min × 3 OS | ~1-2 min × 1 OS (warm shared cache + single strict invocation) |
| Test job runtime | ~5-7 min × 3 OS | ~2-3 min × 1 OS (nextest + shared cache) |
| PR cancellation | 2 PR pushes → 2 parallel runs | newer push cancels older |
| Strict clippy coverage | `surge-core` + `surge-acp` only | **entire workspace** |
| Cache hit rate after `Cargo.lock` bump | poor (test/clippy each rebuild) | poor for *deps* but jobs share one rebuild |

## 5. Out of scope (deliberately)

These map to nebula workflows that surge doesn't need yet — track separately:

- **`security-audit.yml`** (cargo-audit) — weekly cron + Cargo.lock-changes trigger. Independent concern, deserves own PR.
- **`hygiene.yml`** (typos + taplo) — code hygiene, doesn't affect performance.
- **`semver-checks.yml`** — surge isn't published to crates.io and has no public-API stability commitment.
- **`udeps.yml`** (cargo-udeps) — weekly nightly. Useful but not blocking.
- **`pr-validation.yml`** (convco + conventional commits) — process improvement, independent of perf.
- **`dep-review.yml`** (`actions/dependency-review`) — supply-chain check on PR.
- **`codspeed.yml`** (benchmarks) — surge has no codspeed target yet.

These should land as their own focused PRs after the main CI rewrite stabilizes.

Other deliberate non-decisions:

- **sccache** — surveyed projects don't use it; no win on top of `rust-cache` for our churn rate.
- **`cargo nextest archive` shard split** — worthwhile when a single test job exceeds 10 min; ubuntu-only test job will be 2-3 min.
- **MSRV gate job** — surge is an app, not a library; pinned toolchain is enough.
- **Self-hosted runners** — out of scope.

## 6. Migration plan

1. **(done)** `rust-toolchain.toml` pins `1.95.0`; workspace `rust-version` and `clippy.toml` `msrv` set to `1.95`.
2. Run `cargo clippy --fix --workspace --all-targets --all-features` for autofixable warnings, hand-fix the residue, verify `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` clean.
3. Rewrite `.github/workflows/ci.yml` per §3.2 (ubuntu-only, 5 jobs + aggregator, env hardening, rust-cache, nextest, strict workspace clippy).
4. Create `.github/workflows/cross-platform.yml` per §3.3 (3-OS smoke, path-filtered triggers, weekly cron).
5. Update `.github/workflows/release.yml` per §3.4 (rust-cache, env, `--locked`).
6. Push to feature branch. First run is cold-cache; cache populates. Compare timings against §1 baseline.
7. Open PR. Verify second push cancels first. Verify ubuntu wall is ~2 min on warm cache.
8. After merge to main, verify `cross-platform.yml` triggers on the merge and completes.

## 7. Rollback

Three changed files (`ci.yml`, `release.yml`, new `cross-platform.yml`) plus the workspace warning-cleanup commit. Reverting the CI commits restores the old workflows fully. The clippy-cleanup commit is independently reversible (it's just code edits). No migration of cache state is needed — old `actions/cache@v4` keys won't collide with `Swatinem/rust-cache@v2.9.1` keys.

## 8. References

- [nebula CI](https://github.com/vanyastaff/nebula/tree/main/.github/workflows) — primary template (ubuntu-only main, separate cross-platform, required aggregator, env hardening).
- [Tokio CI](https://github.com/tokio-rs/tokio/blob/master/.github/workflows/ci.yml) — `Swatinem/rust-cache@v2` + nextest baseline.
- [rust-analyzer CI](https://github.com/rust-lang/rust-analyzer/blob/master/.github/workflows/ci.yaml) — source of `CARGO_INCREMENTAL=0` rationale.
- [Bevy CI](https://github.com/bevyengine/bevy/blob/main/.github/workflows/ci.yml) — large workspace, restore-only cache pattern (not adopted, mentioned for context).
- [ripgrep CI](https://github.com/BurntSushi/ripgrep/blob/master/.github/workflows/ci.yml) — counter-example: small CLI projects can skip caching.
- [Swatinem/rust-cache](https://github.com/Swatinem/rust-cache) — action documentation (`shared-key`, `save-if`, `cache-on-failure`).
- [taiki-e/install-action](https://github.com/taiki-e/install-action) — pre-built binary installer for `cargo-nextest`.
