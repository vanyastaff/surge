# Justfile for Surge — local-first AFK orchestrator for AI coding workflows.
#
# Usage: just <recipe>   |   just --list   |   just --list-unsorted
# Install: `cargo install just`  (see https://just.systems/)
#
# All recipes are thin wrappers over `cargo`, which is cross-platform; the
# shell-line settings below only affect backtick variables and inline shell
# commands. Bash is used on Unix; PowerShell on Windows.

set shell := ["bash", "-euo", "pipefail", "-c"]
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]
set dotenv-load
set export
set positional-arguments

# ──────────────────────────────────────────────────────────────────────────────
# Variables
# ──────────────────────────────────────────────────────────────────────────────

# Workspace excludes the GPUI desktop shell from default builds because it has
# heavy native deps. Build it explicitly with `just build-ui` / `just build-all`.
workspace_exclude := "--workspace --exclude surge-ui"

# Short commit SHA — works the same in bash and PowerShell.
commit := `git rev-parse --short HEAD`

# ──────────────────────────────────────────────────────────────────────────────
# Default — show help
# ──────────────────────────────────────────────────────────────────────────────

[doc("Show all available recipes")]
default:
    @just --list --unsorted

# ──────────────────────────────────────────────────────────────────────────────
# Build
# ──────────────────────────────────────────────────────────────────────────────

[group("build")]
[doc("Build the core workspace (excludes the GPUI desktop shell)")]
build:
    cargo build --locked {{ workspace_exclude }}

[group("build")]
[doc("Build everything, including the surge-ui desktop shell")]
build-all:
    cargo build --locked --workspace

[group("build")]
[doc("Build only the surge-ui desktop shell")]
build-ui:
    cargo build --locked -p surge-ui

[group("build")]
[doc("Build surge-ui and wrap it in a macOS Surge.app bundle (macOS only)")]
bundle-ui *args: build-ui
    bash scripts/bundle-macos-app.sh {{ args }}

[group("build")]
[doc("Build the surge CLI and daemon in release mode")]
build-release:
    cargo build --locked --release -p surge-cli --bin surge -p surge-daemon --bin surge-daemon

[group("build")]
[doc("Build the mock ACP agent (needed for ignored integration tests)")]
build-mock-agent:
    cargo build --locked -p surge-acp --bin mock_acp_agent

# ──────────────────────────────────────────────────────────────────────────────
# Test
# ──────────────────────────────────────────────────────────────────────────────

[group("test")]
[doc("Run the workspace test suite (excludes surge-ui). Pass extra args after --")]
test *args:
    cargo test --locked {{ workspace_exclude }} {{ args }}

[group("test")]
[doc("Run tests for a single crate (e.g. just test-crate surge-core)")]
test-crate crate *args:
    cargo test --locked -p {{ crate }} {{ args }}

[group("test")]
[doc("Run the required local mock/restart integration allowlist")]
test-ignored: build-mock-agent
    cargo build --locked -p surge-cli --bin surge -p surge-daemon --bin surge-daemon
    cargo build --locked -p surge-mcp --example mock_mcp_server --features mock-server
    cargo test --locked -p surge-acp --test bridge_rate_limit_classification --test reconnect_integration_test -- --ignored
    cargo test --locked -p surge-orchestrator --test engine_e2e_linear_pipeline --test engine_concurrent_runs --test engine_resume_after_crash -- --ignored
    cargo test --locked -p surge-mcp --features mock-server --test mcp_stdio_e2e -- --ignored
    cargo test --locked -p surge-daemon --test live_provider_smoke controlled_daemon_mcp_smoke -- --ignored --exact
    cargo test --locked -p surge-cli --test daemon_restart -- --ignored

[group("test")]
[doc("Run the full test suite — workspace tests + ignored integration tests")]
test-all: test test-ignored

[group("test")]
[doc("Run tests via cargo-nextest (faster runner; install: just install-tools)")]
nextest *args:
    cargo nextest run --locked {{ workspace_exclude }} {{ args }}

# ──────────────────────────────────────────────────────────────────────────────
# Lint & format
# ──────────────────────────────────────────────────────────────────────────────

[group("lint")]
[doc("Check code formatting without modifying files")]
fmt-check:
    cargo fmt --all --check

[group("lint")]
[doc("Apply rustfmt to the whole workspace")]
fmt:
    cargo fmt --all

[group("lint")]
[doc("Strict clippy on surge-core and surge-acp (warnings → errors)")]
clippy-strict:
    cargo clippy --locked -p surge-core --all-targets --all-features -- -D warnings
    cargo clippy --locked -p surge-acp --all-targets -- -D warnings

[group("lint")]
[doc("Strict clippy on the whole workspace (all and default features)")]
clippy:
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo clippy --locked --workspace --all-targets -- -D warnings

[group("lint")]
[doc("Run all lints — fmt-check + clippy-strict + clippy (mirrors ci.yml)")]
lint: fmt-check clippy-strict clippy

# ──────────────────────────────────────────────────────────────────────────────
# Run
# ──────────────────────────────────────────────────────────────────────────────

[group("run")]
[doc("Run a flow.toml through the engine in-process (e.g. just engine examples/flow_minimal_agent.toml)")]
engine flow:
    cargo run -p surge-cli --bin surge -- engine run {{ flow }} --watch

[group("run")]
[doc("Smoke-test the smallest flow (terminal node only — no agent needed)")]
smoke:
    cargo run -p surge-cli --bin surge -- engine run examples/flow_terminal_only.toml --watch

[group("run")]
[doc("Smoke-test the minimal agent flow (requires a configured ACP agent on PATH)")]
smoke-agent:
    cargo run -p surge-cli --bin surge -- engine run examples/flow_minimal_agent.toml --watch

[group("run")]
[doc("Start the surge daemon detached")]
daemon-start:
    cargo run -p surge-cli --bin surge -- daemon start --detached

[group("run")]
[doc("Stop the surge daemon")]
daemon-stop:
    cargo run -p surge-cli --bin surge -- daemon stop

[group("run")]
[doc("Ping a configured ACP agent (e.g. just ping claude)")]
ping agent:
    cargo run -p surge-cli --bin surge -- ping --agent {{ agent }}

[group("run")]
[doc("Send a one-shot prompt to a configured ACP agent (e.g. just prompt claude \"summarize\")")]
prompt agent message:
    cargo run -p surge-cli --bin surge -- prompt {{ message }} --agent {{ agent }}

# ──────────────────────────────────────────────────────────────────────────────
# Bench
# ──────────────────────────────────────────────────────────────────────────────

[group("bench")]
[doc("Run all surge-core criterion benchmarks")]
bench:
    cargo bench -p surge-core

[group("bench")]
[doc("Run a specific bench by name (e.g. just bench-one fold_events). Available: fold_events, validate_graphs, toml_roundtrip, bincode_roundtrip")]
bench-one name:
    cargo bench -p surge-core --bench {{ name }}

# ──────────────────────────────────────────────────────────────────────────────
# Security
# ──────────────────────────────────────────────────────────────────────────────

[group("security")]
[doc("Audit Cargo.lock against the RustSec advisory DB (install: just install-tools)")]
audit:
    cargo audit

[group("security")]
[doc("Check locked dependencies against advisory, license and source policy")]
deny:
    cargo deny --locked check advisories licenses bans sources

# ──────────────────────────────────────────────────────────────────────────────
# CI aggregates
# ──────────────────────────────────────────────────────────────────────────────

[group("ci")]
[doc("Mirror the GitHub Actions ci.yml workflow — fmt + clippy + tests")]
ci: fmt-check clippy-strict clippy test

[group("ci")]
[doc("Full local check suite — ci + audit + deny + local integration tests")]
ci-full: ci audit deny test-ignored

# ──────────────────────────────────────────────────────────────────────────────
# Maintenance
# ──────────────────────────────────────────────────────────────────────────────

[group("maintenance")]
[doc("Remove cargo build artifacts (target/)")]
clean:
    cargo clean

[group("maintenance")]
[doc("Clean up orphaned surge worktrees and merged branches (delegates to surge CLI)")]
clean-worktrees:
    cargo run -p surge-cli --bin surge -- clean

# ──────────────────────────────────────────────────────────────────────────────
# Tooling
# ──────────────────────────────────────────────────────────────────────────────

[group("tooling")]
[doc("Install dev tooling used by recipes above (audit, deny, nextest)")]
install-tools:
    cargo install --locked cargo-audit --version 0.22.2
    cargo install --locked cargo-deny --version 0.20.2
    cargo install --locked cargo-nextest

[group("tooling")]
[doc("Print project name and current commit derived from git")]
version:
    @echo "surge {{ commit }}"
