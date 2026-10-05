# R24 — Performance verification

## Findings
- The stage-transition benchmark isolates synchronous Branch execution plus SQLite event persistence, avoiding provider latency. It includes StageEntered, branch OutcomeReported, EdgeTraversed and StageCompleted; writer creation/closure are outside timed Criterion transitions.
- Budget harness samples 64 transitions after 8 warmups and checks nearest-rank p95 against 5,000 microseconds when SURGE_STAGE_TRANSITION_BUDGET_CHECK is present. CI enables it and runs Criterion quick mode.
- 5,000 microseconds is explicitly a seed. No historical baseline was found under target/criterion; therefore a +25% regression claim cannot currently be verified. Hardware-specific calibration is intentionally not changed during release preparation.

## Evidence inspected
- crates/surge-orchestrator/benches/stage_transition.rs
- crates/surge-orchestrator/Cargo.toml (harness=false bench declaration)
- .github/workflows/ci.yml (budget env + quick baseline command)
- docs/development.md and .ai-factory/plans/feature-graph-engine-ga.md

## Changes
None: no measured bottleneck exists to justify tuning.

## Checks
Static inspection completed. Benchmark execution pending serialized Cargo slot after R05 release build and R06 lock update. No performance pass claimed.

## Required final measurement
SURGE_STAGE_TRANSITION_BUDGET_CHECK=1 cargo bench -p surge-orchestrator --bench stage_transition -- --save-baseline release-2026-10-05

This measures local macOS performance only; Linux CI timings are not directly comparable. Criterion confidence interval estimates mean transition cost; the sampled p95 budget harness is a separate statistic.
