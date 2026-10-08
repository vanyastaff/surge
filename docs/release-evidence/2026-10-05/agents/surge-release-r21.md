# R21 — integration gate inventory

Status: source inventory completed; execution not performed (coordinator explicitly reserves cargo until R05 baseline build completes). No runtime pass claimed.

## Findings supported by source

- `justfile:85-86` selects only ACP and orchestrator ignored tests. It omits MCP stdio mock tests, controlled daemon/ACP/MCP smoke, and real CLI daemon restart.
- Four ignored M6 tests have empty bodies: `engine_m6_iterable_loop`, `engine_m6_subgraph_with_branch`, `engine_m6_resume_with_loop_frame`, `engine_m6_resume_with_subgraph_frame`. Running these with `--ignored` yields vacuous pass, not integration evidence.
- `engine_human_input_resolved` and `engine_human_input_timeout` are MockBridge API smoke tests with deferred real journey, despite their names. Do not count them as real human approval end-to-end coverage.
- `runs_inner/single_writer.rs:43` cross-process exclusion test is empty/deferred. In-process writer test is real; cross-process claim remains unverified by that test.
- ACP Ollama smoke returns early when credentials/model are absent. Third-party filesystem MCP returns early when opt-in/env or npx unavailable. Such successful test process exit does not verify external service.
- `rate_limit_e2e` exercises pure error/config objects, not live provider recovery. Real mock quota recovery evidence lives in daemon `quota_recovery_route_test`.

## Exact local-only gates (each command separate; execute after baseline build)

```sh
cargo build --locked -p surge-acp --bin mock_acp_agent
cargo build --locked -p surge-cli --bin surge -p surge-daemon --bin surge-daemon
cargo build --locked -p surge-mcp --example mock_mcp_server --features mock-server
cargo nextest run --locked --workspace --exclude surge-ui
cargo test --locked -p surge-acp --test bridge_rate_limit_classification --test reconnect_integration_test -- --ignored
cargo test --locked -p surge-orchestrator --test engine_e2e_linear_pipeline --test engine_concurrent_runs --test engine_resume_after_crash -- --ignored
cargo test --locked -p surge-mcp --features mock-server --test mcp_stdio_e2e -- --ignored
cargo test --locked -p surge-daemon --test live_provider_smoke controlled_daemon_mcp_smoke -- --ignored --exact
cargo test --locked -p surge-cli --test daemon_restart -- --ignored
```

The ordinary workspace suite includes real daemon route suites. If targeted re-verification is needed after late changes:

```sh
cargo test --locked -p surge-daemon --test work_item_route_test --test quota_recovery_route_test --test owned_flow_mcp_recovery
```

Critical evidence in those suites: task creation survives daemon restart with pinned requirements; accepted gate-answer receipt survives completion/restart; planned no-open park survives killed host and periodic wake in new process; uncertain selected session-opening RPC is contained instead of replayed; relative source mutations are refused before provider effects and preserve dirty Git state.

## External or deferred exclusions

Live Codex, Ollama ACP, GitHub/Linear real source, and real npm filesystem MCP are outside local mock-only gate and need explicit credentials/opt-in (live Codex may consume subscription usage). UI native tests are excluded from backend workspace gate and require their separate owner. Four M6 placeholders and persistence cross-process placeholder must remain reported as evidence gaps, even if test runner reports them passed.

## Evidence executed

Read/test inventory via rg and sed; no cargo run, no source mutation, no claims of fixed behavior. Suggested gating improvement needs owner assignment because R07/R20 may own justfile/CI. This role does not change shared gate files without coordinator approval.
