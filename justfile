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
    cargo build {{ workspace_exclude }}

[group("build")]
[doc("Build everything, including the surge-ui desktop shell")]
build-all:
    cargo build --workspace

[group("build")]
[doc("Build only the surge-ui desktop shell")]
build-ui:
    cargo build -p surge-ui

[group("build")]
[doc("Build the surge CLI in release mode")]
build-release:
    cargo build --release -p surge-cli --bin surge

[group("build")]
[doc("Build the mock ACP agent (needed for ignored integration tests)")]
build-mock-agent:
    cargo build -p surge-acp --bin mock_acp_agent

# ──────────────────────────────────────────────────────────────────────────────
# Test
# ──────────────────────────────────────────────────────────────────────────────

[group("test")]
[doc("Run the workspace test suite (excludes surge-ui). Pass extra args after --")]
test *args:
    cargo test {{ workspace_exclude }} {{ args }}

[group("test")]
[doc("Run tests for a single crate (e.g. just test-crate surge-core)")]
test-crate crate *args:
    cargo test -p {{ crate }} {{ args }}

[group("test")]
[doc("Run the ignored M5 engine integration tests (rebuilds mock_acp_agent first)")]
test-ignored: build-mock-agent
    cargo test -p surge-orchestrator --tests -- --ignored

[group("test")]
[doc("Run the full test suite — workspace tests + ignored integration tests")]
test-all: test test-ignored

[group("test")]
[doc("Run tests via cargo-nextest (faster runner; install: just install-tools)")]
nextest *args:
    cargo nextest run {{ workspace_exclude }} {{ args }}

# ──────────────────────────────────────────────────────────────────────────────
# Lint & format
# ──────────────────────────────────────────────────────────────────────────────

[group("lint")]
[doc("Check code formatting without modifying files")]
fmt-check:
    cargo fmt --check

[group("lint")]
[doc("Apply rustfmt to the whole workspace")]
fmt:
    cargo fmt --all

[group("lint")]
[doc("Strict clippy on surge-core and surge-acp (warnings → errors)")]
clippy-strict:
    cargo clippy -p surge-core --all-targets --all-features -- -D warnings
    cargo clippy -p surge-acp --all-targets -- -D warnings

[group("lint")]
[doc("Permissive clippy on the whole workspace (does not fail on warnings)")]
clippy:
    cargo clippy --workspace --all-targets --all-features

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

# ──────────────────────────────────────────────────────────────────────────────
# CI aggregates
# ──────────────────────────────────────────────────────────────────────────────

[group("ci")]
[doc("Mirror the GitHub Actions ci.yml workflow — fmt + clippy + tests")]
ci: fmt-check clippy-strict clippy test

[group("ci")]
[doc("Full local check suite — ci + audit + ignored integration tests")]
ci-full: ci audit test-ignored

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
[doc("Install dev tooling used by recipes above (cargo-audit, cargo-nextest)")]
install-tools:
    cargo install --locked cargo-audit
    cargo install --locked cargo-nextest

[group("tooling")]
[doc("Print project name and current commit derived from git")]
version:
    @echo "surge {{ commit }}"
