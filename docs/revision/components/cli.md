# Component · CLI

## Overview

The `vibe` CLI is the primary entry point for users. It must be:
- **Fast** to start (<100ms cold start)
- **Scriptable** (JSON output, exit codes)
- **Self-documenting** (`--help` everywhere, examples)
- **Cross-platform** (Linux, macOS, Windows)

This document specifies the full CLI surface, command behavior, and output formats.

## Command structure

```
vibe [GLOBAL OPTIONS] <COMMAND> [SUBCOMMAND] [OPTIONS] [ARGS]
```

Global options (any command):
- `--config <path>` — alternate config file (default: `~/.vibe/config.toml`)
- `--verbose, -v` — increase logging (repeat for more)
- `--quiet, -q` — suppress non-essential output
- `--json` — JSON output format
- `--no-color` — disable ANSI colors
- `--help, -h` — show help

## Commands

### `vibe init`

Initialize vibe-flow for a project (creates `.vibe/` dir, optionally selects template).

```
vibe init [--template <name>] [--dry-run]
```

Options:
- `--template <name>` — use specific template (e.g., `rust-crate-tdd`)
- `--dry-run` — show what would be created without writing

Behavior:
1. Detect project type (language, structure)
2. If `--template` not specified, prompt user (or auto-select if confidence high)
3. Create `.vibe/` directory in project
4. Symlink to bundled template's pipeline.toml or copy as starting point
5. Print success message with next steps

Exit codes:
- 0: success
- 1: already initialized (use `--force` to overwrite)
- 2: project not detected
- 3: template not found

### `vibe run`

Start a new run.

```
vibe run [OPTIONS] [DESCRIPTION]
```

Arguments:
- `DESCRIPTION` — natural language task description (required unless `--from-file`)

Options:
- `--template <name>` — skip Description/Roadmap stages, use template's flow directly
- `--from-file <path>` — read description from file
- `--skip-bootstrap` — skip all bootstrap stages (assumes `--template`)
- `--auto` — auto-approve all bootstrap stages (use with care)
- `--dry-run` — generate flow but don't execute
- `--project <path>` — alternate project directory (default: cwd)
- `--detach, -d` — return immediately after starting daemon (default behavior)
- `--attach, -a` — start run and tail output

Examples:
```
vibe run "add CLI flag for verbose mode"
vibe run --template rust-crate-tdd "build JSON5 parser"
vibe run --auto "fix typo in README"
vibe run --from-file task.md
```

Output (default):
```
✓ Run #0083 started (id: 0190a4b2-...)
  Pipeline: rust-crate-tdd@1.0
  Worktree: /path/to/.vibe-worktrees/abc123
  
Run is now executing. To follow progress:
  vibe attach 0083
  
You'll be pinged on Telegram when approval is needed.
```

Output (--json):
```json
{
  "run_id": "0190a4b2-...",
  "short_id": "0083",
  "status": "started",
  "pipeline": "rust-crate-tdd@1.0",
  "worktree": "/path/to/.vibe-worktrees/abc123",
  "daemon_pid": 12345
}
```

### `vibe list`

List runs (active and recent).

```
vibe list [OPTIONS]
```

Options:
- `--status <s>` — filter by status (active | running | completed | failed | aborted | all)
- `--project <path>` — filter to specific project
- `--limit <n>` — max results (default: 20)
- `--since <duration>` — only runs started after (e.g., `1h`, `7d`)

Output (default):
```
ID    STATUS    PROJECT             DESCRIPTION                ELAPSED   COST
0083  ✓ done    sample-app          Build JSON5 parser         24m 14s   $4.82
0082  ⚡ run    sample-app          Refactor lexer             11m 32s   $1.12
0081  ✗ fail   myproject           Add OAuth                  8m 02s    $0.78
0080  ✓ done   myproject           Update deps                3m 11s    $0.42
```

### `vibe status`

Show current state of a run.

```
vibe status <RUN_ID>
```

Run ID can be:
- Full UUID
- Short ID (4-char prefix, e.g., `0083`)
- Project + index (e.g., `sample-app:1` for most recent)

Output (default):
```
Run #0083 · sample-app
─────────────────────────────────────────────
Status:     ⚡ running
Started:    14:28:42 (24m ago)
Pipeline:   rust-crate-tdd@1.0
Worktree:   .vibe-worktrees/abc123 (branch: vibe/run-abc123)

Progress: ████████████████░░░░░░░ 4 / 7 stages

Current stage: Implementer (attempt 1, 4m elapsed)
  Last tool: write_file: src/parser/ast.rs
  Tokens: 145k / 200k

Pending approvals: 0
Total cost: $3.18
```

### `vibe attach`

Tail the live output of a running run.

```
vibe attach <RUN_ID> [OPTIONS]
```

Options:
- `--from <seq>` — start from specific event seq (default: latest)
- `--filter <kinds>` — only show specific event kinds (comma-separated)

Output is streaming; Ctrl+C detaches without affecting the run.

### `vibe cancel`

Abort a running run.

```
vibe cancel <RUN_ID> [--force]
```

Behavior:
- Without `--force`: confirmation prompt
- With `--force`: immediate cancel
- Sends SIGTERM to daemon, daemon writes `RunAborted` event, exits

### `vibe replay`

Open replay UI for a finished run.

```
vibe replay <RUN_ID> [OPTIONS]
```

Options:
- `--at <seq>` — open at specific event seq
- `--no-ui` — print event log to stdout instead

Default: spawns `vibe-runtime` GUI in replay mode.

### `vibe fork`

Create a new run forked from an existing run at a specific point.

```
vibe fork <RUN_ID> --at <SEQ> [OPTIONS]
```

Options:
- `--at <seq>` — fork point (required)
- `--edit-prompt <node>` — edit prompt of node before fork
- `--edit-profile <node> <new_profile>` — change profile of node

Behavior:
- Creates new run with copied events 1..=seq
- Snapshots worktree at the corresponding state
- Optional edits applied to nodes that haven't yet executed
- New run starts execution from fork point

### `vibe profile`

Manage profile registry.

#### `vibe profile list`

```
vibe profile list [--category <cat>]
```

Output:
```
ID                       VERSION  CATEGORY  DESCRIPTION
implementer              1.0      agents    Writes Rust code per plan
reviewer                 1.0      agents    Reviews diffs for issues
spec-author              1.0      agents    Writes specs from descriptions
...
```

#### `vibe profile show <ID>`

```
vibe profile show implementer@1.0
```

Output: full profile contents (TOML), validated.

#### `vibe profile install <SOURCE>`

```
vibe profile install ./my-profile.toml
vibe profile install https://example.com/profile.toml
vibe profile install gh:user/repo/path/profile.toml
```

Behavior:
- Validates the profile
- Asks user to confirm trust (untrusted sources)
- Installs to `~/.vibe/profiles/`

#### `vibe profile uninstall <ID>[@VERSION]`

```
vibe profile uninstall implementer@1.0
```

#### `vibe profile validate <PATH>`

Validates a profile file without installing.

#### `vibe profile diff <ID1> <ID2>`

Shows differences between two profile versions.

### `vibe template`

Manage template registry.

Same subcommands as `vibe profile`: `list`, `show`, `install`, `uninstall`, `validate`.

### `vibe telegram`

Telegram bot management.

#### `vibe telegram setup`

Interactive setup wizard:
1. Asks for bot token (or reads from env)
2. Generates ephemeral binding token
3. Shows URL: `https://t.me/<bot>?start=<token>`
4. Polls for user to tap "Start"
5. Stores chat_id, exits

#### `vibe telegram test`

Sends a test message to the configured chat.

#### `vibe telegram unbind`

Removes the chat_id binding (next setup re-registers).

### `vibe doctor`

Diagnose common issues.

```
vibe doctor [--fix]
```

Checks:
- Git installed and accessible
- ACP-compatible agents available (claude-code, codex, gemini)
- Telegram bot configured
- Storage permissions OK
- No orphaned daemon processes
- No corrupted run databases

Output:
```
Checking vibe-flow setup...

✓ Storage:        ~/.vibe/ (7 runs, 2.4GB)
✓ Agents:         claude-code (1.5.2), codex (0.8.1)
✓ Telegram bot:   bound to chat 123456789
✓ Sandbox:        Landlock available
⚠ Worktrees:     2 orphaned worktrees from completed runs (8 days old)
                  Run `vibe gc` to clean up.

Found 1 warning. Use --fix to attempt automatic fixes.
```

With `--fix`:
- Cleans orphaned worktrees
- Rebuilds materialized views if corrupted
- Restarts telegram service if not running

### `vibe gc`

Garbage collect old runs and worktrees.

```
vibe gc [--older-than <duration>] [--dry-run]
```

Options:
- `--older-than <duration>` — only clean runs older than (default: 30d)
- `--dry-run` — show what would be deleted

Behavior:
- Finds runs in terminal status older than threshold
- Removes their worktrees
- Optionally: archives the event log and artifacts (configurable)

## Daemon mode

When `vibe run` is invoked, it spawns a detached daemon and returns. The CLI's role is:

1. Parse arguments
2. Validate config
3. Create run via Storage
4. Spawn daemon subprocess (detached)
5. Return run ID to user

The daemon is a separate process running `vibe-engine` (the engine binary or `vibe --daemon` mode). It owns the run from start to terminal.

CLI commands like `vibe attach`, `vibe status`, `vibe cancel` communicate with the daemon via the event log (read pending state, append cancel events).

## Output formatting

Three output modes:
- **Text** (default) — colored, formatted for human reading
- **JSON** (`--json`) — machine-readable
- **Quiet** (`--quiet`) — only errors and exit codes

### Color usage

- Green: success / completed
- Yellow: warning / running
- Red: error / failed
- Blue: info / metadata
- Gray: dimmed / secondary

Disabled when output is not a TTY (pipe redirection).

### Progress bars

For long-running operations (file uploads, etc.), `indicatif` progress bars. Suppressed in `--quiet` and `--json` modes.

## Configuration

CLI reads config in this priority order (later overrides earlier):
1. Built-in defaults
2. `~/.vibe/config.toml`
3. Environment variables (prefixed `VIBE_*`)
4. Command-line flags

Example `config.toml`:

```toml
[ui]
default_output = "text"
no_color = false

[telegram]
chat_id = 123456789
mode = "long-poll"

[agents]
default = "claude-code"

[runs]
max_concurrent = 4
auto_attach = false             # whether `vibe run` auto-attaches

[storage]
home = "~/.vibe"
worktrees_location = "auto"     # or absolute path

[gc]
auto_gc_after_days = 30
```

Environment variables:
- `VIBE_HOME` — alternate vibe-flow home (default `~/.vibe`)
- `VIBE_TELEGRAM_BOT_TOKEN` — bot token (overrides config)
- `VIBE_DEFAULT_AGENT` — default agent
- `VIBE_LOG` — log level (e.g., `vibe=debug`)

## Exit codes

Following `sysexits.h` conventions:

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | General error |
| 2    | Misuse of CLI (bad arguments) |
| 64   | Usage error |
| 65   | Data format error (invalid TOML, etc.) |
| 66   | No input (file not found) |
| 67   | No user (e.g., telegram setup not done) |
| 69   | Service unavailable (e.g., daemon crashed) |
| 70   | Software error (engine bug) |
| 73   | Cannot create file |
| 74   | I/O error |
| 78   | Configuration error |

## Help system

Every command supports `--help`. Help text includes:
- Synopsis
- Description
- Options (with defaults and types)
- Examples (at least 2)

`vibe --help` shows top-level commands. `vibe run --help` shows run-specific.

## Shell completion

Generated via `clap`'s built-in completion support:

```
vibe completion bash > /etc/bash_completion.d/vibe
vibe completion zsh > ~/.zfunc/_vibe
vibe completion fish > ~/.config/fish/completions/vibe.fish
```

## Implementation notes

- `clap` for argument parsing (with derive macros for type-safe definitions)
- `indicatif` for progress bars
- `console` for terminal styling
- `serde_json` for `--json` output (use existing serde-derived types)
- `tokio` for async (only when needed; many commands are sync)

The CLI binary should be small and start fast — defer most loading to when commands need them.

## Acceptance criteria

The CLI is correctly implemented when:

1. All commands listed work with proper output formatting in text and JSON modes.
2. `vibe --help` shows all commands; each command's `--help` is informative.
3. Cold-start time of `vibe --help` is < 100ms on a 2024 laptop.
4. Daemon mode: `vibe run` returns within 2 seconds, daemon continues independently.
5. `vibe attach` correctly tails events from a running daemon in real-time.
6. Shell completion works for all commands and major options.
7. Exit codes follow the documented schema.
8. End-to-end: scripted (no TTY) invocation works correctly with `--quiet --json`.
9. Cross-platform: identical behavior on Linux, macOS, Windows for all commands.
10. `vibe doctor --fix` resolves at least 80% of common issues automatically.
