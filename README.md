# ⚡ Surge

[![CI](https://github.com/vanyastaff/surge/workflows/CI/badge.svg)](https://github.com/vanyastaff/surge/actions)

**Any Agent. One Protocol. Pure Rust.**

Surge is an agent-agnostic autonomous coding orchestrator built entirely in Rust. It uses the [Agent Client Protocol (ACP)](https://agentclientprotocol.com) to connect to any compatible AI coding agent — Claude Code, GitHub Copilot, Zed Agent, or any future ACP agent — through a single unified interface.

## Why Surge?

Current autonomous coding tools (Aperant, Cursor Background Agents) are locked to a single AI provider, built on fragile multi-runtime stacks (Electron + Python + Node.js), and break constantly. Surge takes a different approach:

- **ACP-First** — One protocol, any agent. Use Claude for planning, Copilot for coding, or mix and match per subtask.
- **Pure Rust** — Single ~15MB binary. No dependencies. Starts in <50ms. Uses ~30MB RAM.
- **Spec-Driven** — Structured TOML specifications with dependency graphs, agent routing, and acceptance criteria.
- **Zero Garbage** — Automatic cleanup of worktrees, branches, and temp files. Surge cleans up after itself.

## Status

🚧 **Early development** — Not ready for use yet.

## Testing & CI

Surge uses GitHub Actions to ensure cross-platform reliability:

- **Tests** run on Windows, macOS, and Linux for every push and pull request
- **Linting** via `cargo clippy` on all platforms
- **Formatting** checks via `cargo fmt`
- **Release builds** for x86_64-pc-windows-msvc, x86_64-apple-darwin, aarch64-apple-darwin, and x86_64-unknown-linux-gnu

This prevents platform-specific regressions and ensures Surge works reliably across all major operating systems. Unlike tools like Aperant that require hundreds of Windows-specific fixes per release, Surge's pure Rust implementation and comprehensive CI testing catch issues early.

## Architecture

```
surge/
├── crates/
│   ├── surge-core/          # Types, config, spec format, FSM
│   ├── surge-acp/           # ACP Client implementation
│   └── surge-cli/           # CLI application
├── docs/                    # Project documentation & RFCs
└── specs/                   # Spec templates
```

See [docs/02-ARCHITECTURE.md](docs/02-ARCHITECTURE.md) for the full architecture.

## Roadmap

| Phase | Focus | Status |
|-------|-------|--------|
| 0 | Foundation + first ACP connection | 🔄 In Progress |
| 1 | Spec system | ⬜ Planned |
| 2 | Git worktrees | ⬜ Planned |
| 3 | Orchestrator MVP | ⬜ Planned |
| 4 | Parallel execution | ⬜ Planned |
| 5 | Multi-agent | ⬜ Planned |
| 6 | GUI (egui) | ⬜ Planned |
| 7 | Advanced features | ⬜ Planned |

See [docs/03-ROADMAP.md](docs/03-ROADMAP.md) for details.

## Documentation

- [Vision](docs/01-VISION.md) — Mission, philosophy, key differentiators
- [Architecture](docs/02-ARCHITECTURE.md) — Crate structure, types, data flows
- [Roadmap](docs/03-ROADMAP.md) — Development phases and milestones
- [RFC-001: ACP Integration](docs/04-RFC-001-ACP-INTEGRATION.md) — Core ACP design
- [Competitive Analysis](docs/05-COMPETITIVE-ANALYSIS.md) — How Surge compares
- [Features](docs/06-FEATURES.md) — Complete feature specification
- [UX Solutions](docs/07-UX-PAIN-POINTS.md) — Pain points and how Surge solves them
- [Community Pain Points](docs/08-COMMUNITY-PAIN-POINTS.md) — Issues from existing tools

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
