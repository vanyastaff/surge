# Surge v0.1 — Release Notes (draft)

> ⚡ Any Agent. One Protocol. Pure Rust.
>
> Local-first meta-orchestrator for AFK AI coding:
> **describe → approve roadmap/flow → walk away → return to a PR.**
> Agent-agnostic (ACP), source-agnostic, sandbox-delegated.

Status: **draft** — finalize the date, crates.io links, and install
one-liners at tag time.

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
  progress, completion/failure cards, `/run` `/status` `/abort` `/runs`.
- **Tracker automation tiers L0–L3** on GitHub Issues + Linear, label-driven.
- **Crash recovery (this release's v0.1 blocker).** The daemon survives an
  unclean exit and resumes in-flight runs from the event log
  ([docs/crash-recovery.md](crash-recovery.md)). Inspect with
  `surge daemon recover --dry-run`.

## Install

> Pending: `cargo publish` of the publishable crates and the Homebrew tap /
> Scoop manifest. Until then, build from source.

From source (stable Rust ≥ 1.85):

```
git clone https://github.com/vanyastaff/surge
cd surge
cargo build --release
./target/release/surge --version
```

Planned at/after tag:

```
cargo install surge-cli            # once published to crates.io
brew install vanyastaff/tap/surge  # Homebrew tap (planned)
scoop install surge                # Scoop manifest, Windows (planned)
```

First run:

```
surge init            # interactive wizard (or `surge init --default`)
surge project describe
surge doctor report
```

## Version string

`surge --version` embeds the git short SHA and commit date, e.g.
`surge 0.1.0 (86da73b, 2026-05-30)`, so a bug report names the exact build.

## Schema stability

`surge.toml`, `flow.toml`, and the per-run event payloads are **frozen at
schema version 1**. Older configs without `schema_version` are read as 1;
event payloads migrate forward on read. Bump policy:
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
- **Dependency advisories:** `bincode` (unmaintained, RUSTSEC-2025-0141) and
  a narrow `rand` unsoundness (RUSTSEC-2026-0097) are tracked via
  `cargo audit`; neither affects the default run path. Migration is planned
  post-v0.1.

---

## Announcement draft (for the AFK-AI-coding audience)

> **Surge v0.1** — point it at a repo, describe what you want, approve the
> roadmap, and walk away. It drives *your* coding agent (Claude Code,
> Copilot CLI, Zed) over ACP, runs each task in an isolated git worktree,
> checkpoints everything to an append-only event log, and pings your phone
> (Telegram) only when it needs a decision. Crash the daemon? It resumes
> from the log on restart. Pure Rust, local-first, zero telemetry.
