[← Architecture](ARCHITECTURE.md) · [Back to README](../README.md)

# Development

Day-to-day commands for working on the Surge codebase: format checks, tests, lints, and the long-running tests that need an external mock agent.

## Build Automation (`just`)

The repo ships a [`justfile`](../justfile) that wraps every cargo command listed below into short recipes. Install [just](https://just.systems/) once with `cargo install just`, then run `just` (no arguments) to see the list:

```bash
cargo install just
just                       # list all recipes, grouped
just build                 # cargo build --workspace --exclude surge-ui
just test                  # cargo test --workspace --exclude surge-ui
just lint                  # fmt-check + clippy-strict + clippy (mirrors ci.yml)
just smoke                 # run the smallest example flow
just audit                 # cargo audit (install: just install-tools)
just ci                    # full local CI run
just ci-full               # ci + audit + ignored integration tests
```

The cargo commands below are equivalent if you prefer not to install `just`.

## Common Checks

Format check, full workspace tests (excluding the GPUI desktop shell), and clippy on the most-touched crates plus the whole workspace:

```bash
cargo fmt --check
cargo test --workspace --exclude surge-ui
cargo clippy -p surge-core --all-targets --all-features -- -D warnings
cargo clippy -p surge-acp --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features
```

The strict clippy profile is in [`clippy.toml`](../clippy.toml). Test code relaxes most rules (`allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-print-in-tests`, etc.); production code does not.

## Long-Running / External-Agent Tests

Some `surge-orchestrator` tests need the bundled mock ACP agent. Build it and run the ignored tests separately:

```bash
cargo build -p surge-acp --bin mock_acp_agent
cargo test -p surge-orchestrator --tests -- --ignored
```

### Optional: real-agent smoke test

`crates/surge-orchestrator/tests/real_acp_smoke.rs` runs `examples/flow_minimal_agent.toml` against an installed ACP-conformant agent (Claude Code, Codex CLI, or any custom binary). It is opt-in via two env vars:

```bash
SURGE_REAL_ACP_BIN=/path/to/agent-binary \
SURGE_REAL_ACP_PROFILE=implementer@1.0 \
  cargo test -p surge-orchestrator --test real_acp_smoke -- --nocapture
```

The harness infers `claude-code`, `codex`, or `gemini-cli` launch mode from the binary name. Set `SURGE_REAL_ACP_KIND=claude-code|codex|gemini-cli|custom` to override that, and `SURGE_REAL_ACP_ARGS="--flag value"` for extra ACP process args. When the required env vars are missing the test prints a `SKIPPED` banner and exits successfully — CI's deterministic green path stays covered by the mock-ACP suite.

## Performance Bench

`cargo bench --bench stage_transition` measures per-stage transition latency for a synchronous Branch node. The CI gate runs a quick smoke; a full baseline is recorded locally:

```bash
cargo bench -p surge-orchestrator --bench stage_transition -- --save-baseline ga
```

`P95_BUDGET_US` is encoded in the bench source. CI enables `SURGE_STAGE_TRANSITION_BUDGET_CHECK=1`, which makes the bench fail if the sampled P95 exceeds the source-level budget; bumping it requires a deliberate code change so regressions surface in PR review.

CI records its quick benchmark history with:

```bash
cargo bench -p surge-orchestrator --bench stage_transition -- --quick --save-baseline ci
```

## Local Runtime State

Runtime state is stored under `~/.surge/`, including run databases (`~/.surge/runs/<run_id>/events.sqlite`) and daemon metadata. Project-local state may appear under `.surge/` inside the project. Both directories are safe to delete to start fresh.

## Crate-Level READMEs

A few crates document their own internals beyond the workspace-level docs:

- [`crates/surge-daemon/README.md`](../crates/surge-daemon/README.md)
- [`crates/surge-mcp/README.md`](../crates/surge-mcp/README.md)
- [`crates/surge-notify/README.md`](../crates/surge-notify/README.md)

## See Also

- [Getting Started](getting-started.md) — initial build and smoke-test commands
- [CLI](cli.md) — what each `surge` subcommand does
- [Architecture](ARCHITECTURE.md) — crate boundaries, dependency rules, conventions
