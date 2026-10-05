[Back to README](../README.md) · [CLI →](cli.md)

# Getting Started

This page walks through installing Surge, building from source, running the bootstrap path, and configuring an ACP-compatible coding agent.

## Install a Release Archive

When a version is published, download its archive and `SHA256SUMS` from
[GitHub Releases](https://github.com/vanyastaff/surge/releases). Each archive
contains **both** `surge` and `surge-daemon`, plus the README and licenses.
Keep the two executables together: `surge daemon start` locates the daemon
alongside the CLI, then falls back to `PATH`.

For unattended work, start the daemon with `surge daemon start --detached`.
This separates it from the launching terminal and writes logs to
`~/.surge/daemon/daemon.log` (or `$SURGE_HOME/daemon/daemon.log`).

| Platform | Archive |
|---|---|
| Linux x86_64 (GNU) | `surge-x86_64-unknown-linux-gnu.tar.gz` |
| macOS Intel | `surge-x86_64-apple-darwin.tar.gz` |
| macOS Apple Silicon | `surge-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `surge-x86_64-pc-windows-msvc.zip` |

The release workflow builds and smoke-tests the GNU Linux archive on Ubuntu 24.04. It is not a
static musl build: compatible glibc and native libraries (including OpenSSL)
are required. Older Linux distributions and Alpine are not validated release
targets; build from source on your system if necessary. The workflow builds and
smoke-tests macOS archives on macOS 15; older macOS versions are not validated. Windows
archives use the MSVC target. Git and an ACP agent remain separate installs.
These are configured workflow checks; confirm a successful native release run
for the version you install. Local packaging tests do not validate native builds.

On Linux, verify the downloaded archive before extraction (run in the directory
containing the archive and `SHA256SUMS`):

```bash
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf surge-x86_64-unknown-linux-gnu.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 755 surge surge-daemon "$HOME/.local/bin/"
export PATH="$HOME/.local/bin:$PATH"
surge --version
surge-daemon --version
```

On macOS, run `shasum -a 256 --check --ignore-missing SHA256SUMS`, then use
the matching macOS archive name in the extraction command above. Persist the
`PATH` setting in your shell configuration if needed. Check that your chosen
archive reports `OK`; checksums detect corruption, not publisher identity.

On Windows, use PowerShell to verify and extract:

```powershell
$archive = 'surge-x86_64-pc-windows-msvc.zip'
$expected = ((Get-Content SHA256SUMS | Where-Object { $_.EndsWith("  $archive") }) -split '  ')[0]
if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLower() -ne $expected) { throw 'Checksum mismatch' }
Expand-Archive $archive "$HOME\surge-bin"
$env:PATH = "$HOME\surge-bin;$env:PATH"
surge --version
surge-daemon --version
```

Add that directory to your user `PATH` to keep it available in future shells.
crates.io, Homebrew, and Scoop distribution are pending; these instructions do
not require a package manager. A manual release workflow run on a branch only
produces downloadable workflow artifacts and checksums; it does not publish a
GitHub Release.

## Windows runtime limitations

Windows binaries are packaged, but workflow parity is incomplete. Durable Task Start
preparation rejects restricted private inputs because secure Windows preparation locks
are not implemented. MCP executable writer dispatch cannot establish the host process
identity required by writer coverage and fails closed; recovery writer liveness is
unknown. This does not imply that every direct ACP operation is unavailable. Native
archive `--version` checks alone do not validate these workflows.

## Requirements

- Rust `1.96+` and native build tools (only for source builds)
- Git
- Python `3.11+` only for release packaging scripts and their tests
- An ACP-compatible agent on `PATH` for any flow that contains an `Agent` node (Claude Code, Codex, Gemini, or a custom ACP-conformant binary)

## Build

Build both executables for a source installation:

```bash
cargo build --locked --release -p surge-cli -p surge-daemon
./target/release/surge --version
./target/release/surge-daemon --version
```

Install both files together on `PATH` as described above. Linux source builds
need a C/C++ toolchain, `pkg-config`, and OpenSSL development headers (for
example, `build-essential pkg-config libssl-dev` on Ubuntu).

Build the core workspace (excludes the optional GPUI desktop shell):

```bash
cargo build --workspace --exclude surge-ui
```

The desktop UI is optional and has separate GPUI dependencies. On macOS, its
default build requires Xcode's Metal Toolchain. If the build reports a missing
Metal compiler, install the component and verify it:

```bash
xcodebuild -downloadComponent MetalToolchain
xcrun metal --version
```

On Linux, `gpui` links against system X11/Wayland libraries, so their development
packages must be installed first.

On Ubuntu/Debian:

```bash
sudo apt install -y libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libfontconfig1-dev libfreetype6-dev libxcursor-dev libx11-dev
```

On Fedora/RHEL:

```bash
sudo dnf install -y libxcb-devel libxkbcommon-devel libxkbcommon-x11-devel \
  wayland-devel fontconfig-devel freetype-devel libXcursor-devel libX11-devel
```

On Arch (dev symlinks ship in the base packages):

```bash
sudo pacman -S --needed libxcb libxkbcommon libxkbcommon-x11 wayland \
  fontconfig freetype2 libxcursor libx11
```

Without these, `cargo build -p surge-ui` fails at link time with
`library not found: xcb` — the runtime `.so.1` files are not enough; the
linker needs the bare `-lxcb` / `-lxkbcommon` / `-lxkbcommon-x11` symlinks
that the `-dev` packages provide.

```bash
cargo build -p surge-ui
```

## Initialize a Project

For a fresh repository, create project configuration and stable project context:

```bash
surge init --default
surge project describe
```

`surge init --default` writes a validated `surge.toml` with safe onboarding defaults and the best detected Agent Client Protocol (ACP) agent, falling back to an installable `claude-acp` entry when no agent is found. Run `surge init` without `--default` for the interactive wizard.

`surge project describe` scans high-signal files such as `AGENTS.md`, `README.md`, `Cargo.toml`, `justfile`, formatter/lint config, and git state, then writes `project.md`. In `--author-mode auto` (the default), it uses the Project Context Author ACP profile when the configured runtime is installed and otherwise falls back to deterministic local rendering. This file is separate from `.ai-factory/` agent context: it is the stable project summary captured into new runs at start time. Use `--dry-run` to preview whether it would change, and `--refresh` to rewrite after meaningful project changes.

## Run the Smallest Flow

Create `flow-terminal.toml` in your project with the following contents. This smoke
test needs no agent and works with an installed archive without a source checkout:

```toml
schema_version = 1
start = "end"
edges = []

[metadata]
name = "flow_terminal_only"
created_at = "2026-05-05T00:00:00Z"

[nodes.end]
id = "end"
declared_outcomes = []

[nodes.end.position]
x = 0.0
y = 0.0

[nodes.end.config]
node_kind = "terminal"

[nodes.end.config.kind]
type = "success"
```

```bash
surge engine run flow-terminal.toml --watch
```

Run the same flow through the daemon — start the daemon, run the flow against it, list runs, then stop the daemon:

```bash
surge daemon start --detached
surge engine run flow-terminal.toml --daemon --watch
surge engine ls --daemon
surge daemon stop
```

Detached startup redirects daemon output to `~/.surge/daemon/daemon.log`
(or `$SURGE_HOME/daemon/daemon.log` when `SURGE_HOME` is set). The start
command prints this path and includes it in startup errors.

The `surge daemon restart` command waits up to 45 seconds for the old process,
covering its default 30-second shutdown grace. If the daemon was launched with
a longer `--shutdown-grace`, use `restart --wait-timeout-secs <seconds>`.

## Configure Agents in a Project

Inspect or detect available ACP agents on the host:

```bash
surge registry list
surge registry detect
surge agent list
```

Then edit `surge.toml`, rerun `surge init`, or use registry commands to add an ACP agent. The annotated [`surge.example.toml`](../surge.example.toml) shows local, `npx`, custom, TCP, MCP-flavored agents, sandbox defaults, worktree defaults, approvals, Telegram env placeholders, and inbox defaults side by side.

## Smoke-Test an Agent

Once an ACP agent is configured, verify it answers a simple ping and a one-shot prompt:

```bash
surge ping --agent claude
surge prompt "Summarize this repository" --agent claude
```

## Run the Bootstrap Path

The canonical first useful run is bootstrap:

```bash
surge bootstrap "add a small health-check command to this project"
```

Bootstrap generates `description.md`, `roadmap.toml`, `roadmap.md`, and
`flow.toml`, asks for console approval after each artifact, then starts the
generated follow-up graph.
See [Bootstrap](bootstrap.md) for the edit loop, archetypes, and resume path.

You can validate generated artifacts directly while debugging a run:

```bash
surge artifact validate --kind description description.md
surge artifact validate --kind roadmap roadmap.toml
surge artifact validate --kind flow flow.toml
```

## Run the Minimal Agent Graph

In a source checkout, `examples/flow_minimal_agent.toml` is the smallest flow that opens an ACP session. Run it once at least one agent is wired up:

```bash
surge engine run examples/flow_minimal_agent.toml --watch
```

You can also skip bootstrap with a bundled template:

```bash
surge engine run --template linear-3 --watch
```

## Local State

Local runtime state lives under `~/.surge/`, including run databases and daemon metadata. Project-local state may appear under `.surge/` inside the project. These directories contain durable run history, task state and recovery metadata. Do not delete them as a troubleshooting step. Stop all writers and preserve a complete backup before upgrading or resetting state; see [Release and rollback procedure](release-procedure.md).

For setup troubleshooting, run commands with `RUST_LOG=surge=debug` to see agent detection, project-context scan decisions, and skipped optional files.

## See Also

- [CLI](cli.md) — full `surge` command surface and current-to-target mapping
- [Artifact Conventions](conventions/README.md) — generated artifact names, schemas, and validator examples
- [Bootstrap](bootstrap.md) — adaptive flow generation from a free-form prompt
- [Workflow](workflow.md) — how a run flows through bootstrap, the engine, and the event log
- [Development](development.md) — running tests, lints, and ignored long-running checks
