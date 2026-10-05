# Surge v0.1 — Release Notes (draft)

> ⚡ Any Agent. One Protocol. Pure Rust.
>
> Local-first meta-orchestrator for AFK AI coding:
> **describe → approve roadmap/flow → walk away → return to a PR.**
> Agent-agnostic (ACP), source-agnostic, sandbox-delegated.

Status: **draft** — finalize the date and verify native release builds at tag time.

## Highlights

- **Agent-agnostic via ACP.** One protocol (Agent Client Protocol) connects
  to any coding agent — Claude Code, Copilot CLI, Zed Agent — with no
  per-CLI stdout parsing ([ADR-0006](adr/0006-acp-only-transport.md)).
- **Graph engine.** Every `NodeKind` (`Agent`, `HumanGate`, `Branch`,
  `Loop`, `Subgraph`, `Notify`, `Terminal`) executes end-to-end from a
  typed `flow.toml`, with replay-deterministic event folding.
- **Adaptive bootstrap.** `describe → roadmap → flow` with a HumanGate after
  each stage; archetype detection picks the right pipeline shape.
- **Profile registry.** Bundled bootstrap + execution roles, versioned
  resolution, inheritance, and an artifact output contract surge owns.
- **Sandbox delegation.** `SandboxIntent` maps to each runtime's native
  flags; no silent downgrades. Inspect with `surge doctor matrix`.
- **MCP server lifecycle.** Per-run, supervised, sandbox-delegated MCP
  children with crash detection + backoff restart ([ADR-0014](adr/0014-mcp-server-lifecycle.md)).
- **Telegram cockpit.** Approve/redo bootstrap + HumanGate cards, live
  progress, completion/failure cards, `/status` `/abort` `/runs`.
  `/run` still returns an explicit deferred response; start workflows through the CLI.
- **Tracker automation tiers L0–L3** on GitHub Issues + Linear, label-driven.
- **Crash recovery (this release's v0.1 blocker).** The daemon survives an
  unclean exit and resumes in-flight runs from the event log
  ([docs/crash-recovery.md](crash-recovery.md)). Inspect with
  `surge daemon recover --dry-run`.

## Install

Release archives contain the CLI **and** its sibling daemon, README, and both
licenses. Four targets are packaged: GNU Linux x86_64, macOS Intel, macOS
Apple Silicon (`.tar.gz`), and Windows x86_64 (`.zip`). Download an archive and
`SHA256SUMS` from [GitHub Releases](https://github.com/vanyastaff/surge/releases)
once the tag is published. Follow the
[checksum and installation instructions](getting-started.md#install-a-release-archive),
keeping both executables in the same directory on `PATH`.

The release workflow builds the Linux archive on Ubuntu 24.04; it requires compatible glibc and
native libraries, including OpenSSL; it is not a static musl binary. macOS
archive smoke tests are configured on macOS 15. All four native archive smoke
tests must pass in the release workflow before publication; a successful native
release run has not yet been verified for this draft. A manual branch run produces workflow artifacts
without publishing a release.

crates.io, Homebrew, and Scoop distribution remain pending.

From source (stable Rust ≥ 1.96):

```shell
git clone https://github.com/vanyastaff/surge
cd surge
cargo build --locked --release -p surge-cli -p surge-daemon
./target/release/surge --version
./target/release/surge-daemon --version
```

Install both executables together on `PATH` before the first run.

First run:

```shell
surge init            # interactive wizard (or `surge init --default`)
surge project describe
surge doctor report
```

## Version string

`surge --version` embeds the git short SHA and commit date, e.g.
`surge 0.1.0 (86da73b, 2026-05-30)`, so a bug report names the exact build.

## Schema stability

`surge.toml` and `flow.toml` use schema **1**; per-run event payloads support
versions through **15**, and the memory database uses schema **3**. Older configs
without `schema_version` are read as 1; event payloads migrate forward on read.
Downgrade requires restoring a pre-upgrade backup rather than opening upgraded
state with older binaries. See [Release and rollback procedure](release-procedure.md).
Bump policy:
[docs/schema-versioning.md](schema-versioning.md).

## Telemetry posture

**Zero telemetry by default.** Surge collects and transmits no usage data.
The only outbound traffic is what you configure: the ACP agent runtime,
declared MCP servers, task trackers (GitHub/Linear), and notification
channels (Telegram/Slack/webhook/email). If anonymous telemetry is ever
added, it will be **explicit opt-in** and documented here before shipping.

## Crash-report path

An unexpected panic prints a crash-report hint (version + issue URL) before
the backtrace; re-run with `RUST_BACKTRACE=1` for full detail. Please file:
<https://github.com/vanyastaff/surge/issues/new>.

## License

Dual-licensed [MIT](../LICENSE-MIT) OR [Apache-2.0](../LICENSE-APACHE).
Third-party license compliance is CI-enforced via `cargo deny`
([THIRD_PARTY.md](../THIRD_PARTY.md)).

## Known limitations / deferred

- **Replay & fork-from-here UI** is post-v0.1 polish (CLI replay/fork land
  first).
- **`kill -9` / power-cut fault-injection harness** for WAL checkpointing is
  a follow-up; WAL durability is configured and the resume-from-log path is
  integration-tested.
- **Windows runtime parity:** durable Task Start preparation and MCP executable
  writer dispatch fail closed at unsupported host ownership boundaries. See
  [Windows runtime limitations](getting-started.md#windows-runtime-limitations).
- **Desktop:** the optional GPUI shell is in development and excluded from archives.
- **Dependency audit:** preparation updated affected locked dependencies and removed
  unused direct `bincode`. Allowed transitive maintenance, unsoundness and yanked
  warnings remain under repository policy; retain exact final audit evidence.

---

## Announcement draft (for the AFK-AI-coding audience)

> **Surge v0.1** — point it at a repo, describe what you want, approve the
> roadmap, and walk away. It drives *your* coding agent (Claude Code,
> Copilot CLI, Zed) over ACP, runs each task in an isolated git worktree,
> checkpoints everything to an append-only event log, and pings your phone
> (Telegram) only when it needs a decision. Crash the daemon? It resumes
> from the log on restart. Pure Rust, local-first, zero telemetry.
