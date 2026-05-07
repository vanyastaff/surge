# surge-daemon

Long-running process that hosts the surge engine and exposes it over
local-socket IPC. Companion to the `surge` CLI — when you run
`surge engine run flow.toml --daemon`, the CLI auto-spawns or
connects to a daemon and the run survives the CLI exiting.

Single-process daemon hosts many runs (one tokio multi-thread runtime,
one engine, broadcast channels per run). Cross-platform: Unix domain
socket on Linux/macOS, named pipe on Windows.

## Quick start

```bash
# Start (foreground; logs to stderr).
surge daemon start

# Start (detached; runs until you stop it).
surge daemon start --detached

# Status.
surge daemon status

# Stop (graceful).
surge daemon stop

# Stop (force, immediate kill).
surge daemon stop --force

# Restart.
surge daemon restart
```

## Files

```
~/.surge/daemon/
├── daemon.pid     # PID of the running daemon
├── daemon.sock    # Unix domain socket file (Linux/macOS only).
│                  # On Windows, no socket file exists — the named
│                  # pipe is derived directly from the path's basename
│                  # via local_socket_name_from_path.
└── version        # Daemon binary version
```

The daemon dir is created on `surge daemon start` and cleaned up
(PID file + Unix socket if any) on graceful exit.

## Configuration

CLI flags on `surge daemon start`:

| Flag | Default | Description |
|------|---------|-------------|
| `--max-active N` | 8 | Concurrent active runs cap. Excess starts queue (FIFO). |
| `--max-queue N` | `max_active * 4` | FIFO admission queue cap. When both `max_active` and `max_queue` are hit, further `StartRun` requests are rejected with `QueueFull` so the daemon's pending-start map cannot grow without bound under load. |
| `--shutdown-grace D` | 30s | Time to wait for in-flight runs to drain on stop. |
| `--detached` | false | Detach from controlling terminal (Unix `setsid`, Windows `DETACHED_PROCESS`). |

## Troubleshooting

**"daemon already running (pid N)"**
The PID file exists and the process is alive. If you want a different
daemon, stop the existing one (`surge daemon stop`).

**"daemon socket did not become readable within 5s"**
The daemon binary may have crashed during startup. Check stderr
output. Common cause: storage directory permissions; surge expects
`~/.surge/` to be writable.

**Stale PID file**
If a daemon process was killed forcefully (`kill -9`, OOM, power
cut), the PID file may persist. `surge daemon start` detects this
via `sysinfo` and overwrites the stale file with a warning.

**Daemon won't accept connections after restart on Linux/macOS**
On rare occasions an old socket file lingers. The server unlinks
stale sockets before bind in normal flow, but if a restart races
with the kernel's socket cleanup, run `rm ~/.surge/daemon/daemon.sock`
and retry.

**Restart hangs**
`surge daemon restart` waits up to 10s for the old daemon to exit
before spawning a new one. If shutdown drain is slower than that,
increase `--shutdown-grace` or use `surge daemon stop --force` first
then `start` separately.

## Mixing daemon and CLI versions

The daemon and CLI must be from the same surge binary set. After
upgrading surge:

```bash
surge daemon restart
```

This is the safest path. Connecting an older CLI to a newer daemon
(or vice versa) may surface IPC schema mismatches as decode errors.
M7 ships a single version; richer version negotiation is M8+.

## Architecture

See [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md)
§3 (architecture decisions), §6 (run lifecycle), §17 (daemon mode
section). Key components:

- `pidfile` — PID + socket file discovery, stale-lock detection.
- `admission::AdmissionController` — FIFO admission queue, cap on
  concurrent active runs.
- `broadcast::BroadcastRegistry` — per-run + global event fan-out.
- `server::run` — accept-and-dispatch IPC loop.
- `lifecycle` — signal handlers + graceful drain.
