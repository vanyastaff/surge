# CLAUDE.md

## Project

Surge — agent-agnostic autonomous coding orchestrator. Pure Rust. Uses ACP (Agent Client Protocol) to connect to any coding agent (Claude Code, Copilot CLI, Zed Agent).

## Commands

```
cargo build                    # build all crates
cargo test                     # run all tests
cargo run -p surge-cli         # run CLI
cargo clippy --workspace       # lint
```

## Architecture

Multi-crate workspace. Read `docs/02-ARCHITECTURE.md` for full details.

- `surge-core` — shared types: SpecId, TaskId, TaskState FSM, SurgeConfig, AgentConfig
- `surge-acp` — ACP Client trait implementation, AgentPool, AgentConnection, event system
- `surge-cli` — clap-based CLI: `surge ping`, `surge prompt`, `surge spec`, `surge run`

## Key Design Decisions

- ACP is the ONLY way to communicate with agents. No direct subprocess hacks.
- Specs use TOML format (not markdown) for type safety and git-friendly diffs.
- Every task runs in an isolated git worktree via `git2` crate.
- State machine for tasks: Draft → Planning → Planned → Executing → QaReview → HumanReview → Merging → Completed
- IDs use ULID (`ulid` crate).
- Async runtime: `tokio`.
- Error handling: `thiserror` for library crates, `anyhow` for CLI.

## Coding Standards

- Rust 2024 edition, stable toolchain
- `#[must_use]` on functions returning Result
- Public API documented with `///` doc comments
- No `unwrap()` in library code — use `?` or explicit error handling
- Tests next to code in `#[cfg(test)]` modules
- Follow RFC-driven design: new features start as RFC in docs/

## References

- ACP spec: https://agentclientprotocol.com
- ACP Rust SDK: https://docs.rs/agent-client-protocol
- Project docs: docs/
