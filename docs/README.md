# Surge Documentation

Detailed docs for the Surge workspace. The project landing page is [`README.md`](../README.md) at the repository root; install commands and a high-level pitch live there. The pages below cover specific topics in depth.

## Pages

| Page | Description |
|---|---|
| [Getting Started](getting-started.md) | Requirements, build, project initialization, run examples, agent configuration, smoke tests |
| [CLI](cli.md) | Command surface, project context commands, execution paths, current → target mapping |
| [Bootstrap](bootstrap.md) | Adaptive prompt → description → roadmap → flow generation |
| [Workflow](workflow.md) | AFK workflow, flow model, intake sources, run lifecycle |
| [Architecture](ARCHITECTURE.md) | Canonical architecture: positioning, principles, engine, ACP bridge, storage |
| [Hooks](hooks.md) | `pre_tool_use` / `post_tool_use` / `on_outcome` / `on_error` lifecycle, matcher, failure modes |
| [Artifact Conventions](conventions/README.md) | Canonical generated artifact names, schemas, validators, and examples |
| [Archetypes](archetypes.md) | Bundled `flow.toml` archetypes with mermaid diagrams |
| [Development](development.md) | `cargo` checks, ignored long-running tests, local runtime state |

> **Documentation convention.** **Current** means implemented enough to try from the repository. **Target** means product direction; command names may still change while the CLI is being aligned.

For agent-context files (`.ai-factory/DESCRIPTION.md`, `.ai-factory/ARCHITECTURE.md`, `.ai-factory/rules/base.md`, root [`AGENTS.md`](../AGENTS.md), [`CLAUDE.md`](../CLAUDE.md)) see the project root.
