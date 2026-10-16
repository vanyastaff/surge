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
just ci-full               # ci + audit + deny + deterministic ignored integrations
```

The cargo commands below are equivalent if you prefer not to install `just`.

Release packaging scripts and `scripts/test_release.py` require Python 3.11+
(`tomllib` is part of the standard library from 3.11). Run their contract tests with
`python3.12 -m unittest discover -s scripts -p test_release.py`.

## Common Checks

Format check, full workspace tests (excluding the GPUI desktop shell), and clippy on the most-touched crates plus the whole workspace:

```bash
cargo fmt --all --check
cargo nextest run --locked --workspace --exclude surge-ui
cargo test --locked --workspace --exclude surge-ui --doc
cargo clippy -p surge-core --all-targets --all-features -- -D warnings
cargo clippy -p surge-acp --all-targets -- -D warnings
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

The strict clippy profile is in [`clippy.toml`](../clippy.toml). Test code relaxes most rules (`allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-print-in-tests`, etc.); production code does not.

## Run-writer ownership

In the current ownership implementation, the low-level
`runs::writer::spawn_writer` helper is crate-private and requires an actor-held
`WriterLease`. The actor and `RunWriter` wrapper retain the same lease, so dropping
or cancelling a wrapper cannot release the OS writer lock while queued writes
remain active. External callers use the existing `Storage` and `RunWriter` APIs;
writer command/configuration exports remain data, not execution authority.

`surge-process` contains the shared standard-library owner-panic boundary:
`install_owner_panic_protection` and `abort_on_owner_panic`. Hosts install the same
hook rather than maintaining separate copies. This boundary does not prove native
child cleanup, domain closure or startup authority.

## Daemon restart check

On Unix, this explicit subprocess test starts an isolated daemon with an
11-second shutdown grace, restarts it through the CLI, and verifies a new PID
and healthy IPC connection. Build the sibling daemon executable first:

```bash
cargo build -p surge-daemon
cargo test -p surge-cli --test daemon_restart -- --ignored
```

The test uses a temporary `SURGE_HOME` and cleans up its own processes.

## Desktop UI checks

On macOS 27, Rust 1.96 can produce an unloadable proc-macro dylib when stripping
debug information (misaligned LINKEDIT string pool; see
[Rust issue 157750](https://github.com/rust-lang/rust/issues/157750)). For the
release MSRV check on this host, retain symbols explicitly:

```bash
RUSTFLAGS='-C strip=none' CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 OPENSSL_STATIC=1 MACOSX_DEPLOYMENT_TARGET=15.0 cargo +1.96.0 check --locked --workspace --exclude surge-ui
```

This checks the non-desktop workspace; desktop MSRV verification remains separate.

The desktop uses the workspace-pinned `gpui-kit` 0.7.0 facade and its
`gpui-pre` 0.3.7 type family. Import GPUI and widgets through `gpui_kit`;
adding the older `gpui` package creates incompatible entity/window types.
The production `gpui_kit::open_window` helper installs one Root overlay host.
Do not also render notification or modal layers in the application shell.

```bash
cargo test -p surge-ui
cargo clippy -p surge-ui --all-targets --all-features -- -D warnings
cargo +1.96.0 check -p surge-ui --all-targets --locked
cargo fmt -p surge-ui --check
```

On macOS, native builds require the Xcode Metal toolchain. For a smaller local
build cache, set `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and
`CARGO_INCREMENTAL=0` in the command environment.

On macOS, `just bundle-ui` builds `surge-ui`, `surge`, and `surge-daemon`
with `--locked`, then packages all three as siblings in `Surge.app/Contents/MacOS`.
Use `just bundle-ui --release` to build and package the release profile. Arguments
such as `--out "path with spaces"` and `--surge-home "isolated state path"` are
forwarded intact; the latter embeds only `SURGE_HOME` in the app's plist.
Direct `bash scripts/bundle-macos-app.sh` packages existing binaries, while
`--build` first builds the selected profile for the native Darwin Rust host,
pinning Cargo output to `target/<host>/<profile>` regardless of ambient target
or target-directory configuration. This uses a separate target-qualified Cargo
cache. All three executables are checked
before replacing an existing bundle. Packaging keeps the existing ad-hoc signing
policy. Run the isolated packaging checks with
`python3 -m unittest scripts/test_release_bundle_macos.py` (requires `just`).

Custom clickable divs need an element identity, an accessibility role and a
meaningful label. Use the item's domain identity when a list can reorder or
filter. Planning uses `TextareaState` so Enter inserts a newline, while the
single-line command and search inputs keep their Enter actions.

Render tests and the production submission-boundary tests cover different
contracts. Native accessibility still needs a separate application check:
open a project, navigate, enter a multiline task, observe submission failure
and retry, filter the command palette, inspect an approval, and scroll run
status. A controlled-response form harness does not prove a live daemon or
agent workflow. Record the platform and input method used; native paste,
accessibility value assignment, and synthetic typing are distinct paths.

## Long-Running / External-Agent Tests

The deterministic ignored integration allowlist is maintained in `just integration`
and the reusable CI workflow. Run it separately from optional external service tests:

```bash
just integration
```

### Optional: real-agent smoke test

`crates/surge-orchestrator/tests/real_acp_smoke.rs` runs `examples/flow_minimal_agent.toml` against an installed ACP-conformant agent (Claude Code, Codex CLI, or any custom binary). It is opt-in via two env vars:

```bash
SURGE_REAL_ACP_BIN=/path/to/agent-binary \
SURGE_REAL_ACP_PROFILE=implementer@1.0 \
  cargo test -p surge-orchestrator --test real_acp_smoke -- --nocapture
```

The harness infers `claude-code`, `codex`, or `gemini-cli` launch mode from the binary name. Set `SURGE_REAL_ACP_KIND=claude-code|codex|gemini-cli|custom` to override that, and `SURGE_REAL_ACP_ARGS="--flag value"` for extra ACP process args. When the required env vars are missing the test prints a `SKIPPED` banner and exits successfully — CI's deterministic green path stays covered by the mock-ACP suite.

### Optional: real filesystem-MCP smoke test

`crates/surge-mcp/tests/filesystem_mcp_e2e.rs` exercises the real `@modelcontextprotocol/server-filesystem` MCP server end-to-end (list tools → read a file → deterministic shutdown). It is doubly gated — `#[ignore]` plus `SURGE_MCP_REAL=1` — so the default and `--ignored` CI paths stay mock-only. Needs `npx` on `PATH` (resolved Windows-safely via rmcp's `which_command`); the server package is fetched on first run:

```bash
SURGE_MCP_REAL=1 cargo test -p surge-mcp --test filesystem_mcp_e2e -- --ignored --nocapture
```

When `SURGE_MCP_REAL` is unset (or `npx` is absent) the test prints a `SKIPPED` banner and exits successfully.

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

Runtime state is stored under `~/.surge/`, including run databases (`~/.surge/runs/<run_id>/events.sqlite`) and daemon metadata. Project-local state may appear under `.surge/` inside the project. They contain durable history and ownership/recovery state. Stop all writers and back up the complete runtime and associated project workspaces before a reset or upgrade; see [Release and rollback procedure](release-procedure.md).

## Crate-Level READMEs

A few crates document their own internals beyond the workspace-level docs:

- [`crates/surge-daemon/README.md`](../crates/surge-daemon/README.md)
- [`crates/surge-mcp/README.md`](../crates/surge-mcp/README.md)
- [`crates/surge-notify/README.md`](../crates/surge-notify/README.md)

## See Also

- [Getting Started](getting-started.md) — initial build and smoke-test commands
- [CLI](cli.md) — what each `surge` subcommand does
- [Architecture](ARCHITECTURE.md) — crate boundaries, dependency rules, conventions


### Exact-model ACP transport journey

The ignored daemon/MCP smoke uses a temporary repository and runtime home. It
checks a human-response nonce through authenticated stage tools, unchanged Git
checkouts, one durable provider session, an exact offered model ID and a successful
same-session model selection before one prompt. A profile recommendation alone
is insufficient. Ordinary tests use the controlled mock and never call a provider.

Build `mock_acp_agent`, `surge`, and the daemon example `acp_smoke_proxy`, then run
`controlled_daemon_mcp_smoke` and
`proxy_cancellation_settles_observed_descendant_and_owned_container` with
`cargo test -p surge-daemon --test live_provider_smoke -- --ignored --exact TEST`.
The proxy is test support, not a production runtime or sandbox.

To opt into one real invocation, set `SURGE_LIVE_CODEX=1`,
`SURGE_LIVE_ACP_COMMAND` to the adapter executable (or `node`),
`SURGE_LIVE_ACP_ARGS_JSON` to a JSON array of arguments (for Node, the adapter's
entry script), and `SURGE_LIVE_ACP_MODEL` to the exact advertised ID such as
`gpt-6.1-sol`. Run the ignored exact test `live_codex_daemon_mcp_smoke`.
No cache path or fallback model is embedded in the test. Existing provider login
is used; the fixture never logs in, installs an adapter or changes credentials.

The watchdog bounds the journey to 150 seconds. The bridge retains Unix group /
Windows Job ownership of proxy and provider descendants; the outer fixture owns
its own process group / native Windows Job. Capture contains only bounded adapter,
model and session metadata. Raw ACP frames, prompts, auth, tool values and provider
stderr are withheld. This proves the adapter acknowledged the selected model; it
does not independently attest the provider's model implementation.

For test-only desktop journey observation, the proxy additionally accepts
`--capture-dir PRIVATE_DIRECTORY --initial-provider EXACT_MODEL COMMAND ARGS...`.
Each launch publishes a fresh, exclusively created capture file. The directory
must already be private; on Windows it must be the `daemon` directory of a retained
Surge runtime home. Initial observation is an explicit separate mode: it requires
an exact categorical offered/current model on the correlated new session, without
adding a model-selection RPC. Strict smoke mode never falls back to it.

For `@agentclientprotocol/codex-acp` version `2.2.2` only, the observer cross-checks
its legacy `base[effort]` field against the exact categorical base and an offered
reasoning effort, preserving raw/base/effort separately. Contradictions, unknown
composite versions and malformed representations fail before prompt. This parser
is confined to the test harness.

The daemon smoke publishes a separate private, bounded typed diagnostic after
provider and MCP cleanup. Its parent prints only fixed failure categories, event
type counts, requested-model comparisons, and selection provenance. Provider
metadata stays in the private capture. Failed runs still publish diagnostics
before returning failure. If writer settlement cannot be confirmed, the parent
skips artifact reads and retains the temporary paths for reconciliation.
