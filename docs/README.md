# Surge Documentation

Detailed docs for the Surge workspace. The project landing page is [`README.md`](../README.md) at the repository root; install commands and a high-level pitch live there. The pages below cover specific topics in depth.

## Pages

| Page | Description |
|---|---|
| [Getting Started](getting-started.md) | Requirements, build, project initialization, run examples, agent configuration, smoke tests |
| [CLI](cli.md) | Command surface, project context commands, execution paths, roadmap amendment commands |
| [Bootstrap](bootstrap.md) | Adaptive prompt → description → roadmap → flow generation |
| [Workflow](workflow.md) | AFK workflow, flow model, intake sources, roadmap amendments, run lifecycle |
| [Architecture](ARCHITECTURE.md) | Canonical architecture: positioning, principles, engine, Agent Client Protocol (ACP) bridge, storage |
| [Decisions (ADRs)](adr/) | Architectural decision records with rationale, alternatives rejected, and revisit triggers |
| [Hooks](hooks.md) | `pre_tool_use` / `post_tool_use` / `on_outcome` / `on_error` lifecycle, matcher, failure modes |
| [Artifact Conventions](conventions/README.md) | Canonical generated artifact names, schemas, validators, and examples |
| [Archetypes](archetypes.md) | Bundled `flow.toml` archetypes with mermaid diagrams |
| [Tracker automation](tracker-automation.md) | Tier labels (L0/L1/L2/L3), idempotency, `surge intake list`, external state reflection |
| [Telegram cockpit](telegram.md) | Setup, pairing, command reference, card lifecycle, snooze re-emission, recovery |
| [MCP server lifecycle](mcp.md) | `[[mcp_servers]]` config, connection/restart/health lifecycle, stderr redaction, sandbox boundary, `surge mcp` |
| [Product strategy](product-strategy.md) | Positioning, research-backed bets (completion machinery, fleet interaction), build-don't-bridge principle, sequencing, metrics |
| [Developer Vibe Coding Harness](vibe-coding-harness.md) | Agentlas source audit, Factory documentation comparison, application-creation journey and measurable completion criteria |
| [Factory product model](factory-product-model.md) | Detailed primary-documentation research on context, authority, validation, recovery, readiness and costs |
| [SuperPlane-informed improvements](superplane-improvements.md) | Accepted six-phase product plan, completion reliability and revision-bound verification |
| [Native UI automation](ui-automation-evaluation.md) | Observed GPUI/egui Computer Use results, limitations and migration decision criteria |
| [Agent OS and coding-agent landscape](agent-os-landscape.md) | Market survey: Pi, Herdr, BridgeMind, Factory Droid, Devin, Codex, Claude Code, 2026 manager-view convergence, orchestrator graveyard |
| [Migrate `.spec.toml` → `flow.toml`](migrate-spec-to-flow.md) | Auto-translator (`surge migrate-spec`) reference and manual-edit guidance for the legacy pipeline retirement |
| [Development](development.md) | `cargo` checks, ignored long-running tests, local runtime state |

> **Documentation convention.** **Current** means implemented enough to try from the repository. **Target** means product direction; command names may still change while the CLI is being aligned.

For agent-context files (`.ai-factory/DESCRIPTION.md`, `.ai-factory/ARCHITECTURE.md`, `.ai-factory/rules/base.md`, root [`AGENTS.md`](../AGENTS.md), [`CLAUDE.md`](../CLAUDE.md)) see the project root.
