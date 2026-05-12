# Artifact JSON Schemas

Surge owns a small set of **artifact contracts** — the on-disk shapes that
agents (Surge-internal orchestrator, Copilot CLI, Zed Agent, any
ACP-compatible implementation) exchange. Until now those shapes were
documented prose-first and re-encoded by hand in every agent prompt profile.

`surge artifact schema` makes the contract **self-describing**: surge-core
generates a JSON Schema (draft 2020-12) for every TOML-format artifact and
exposes a small introspection API. Agents and IDEs can consume these schemas
directly instead of guessing from documentation.

## Quick start

```bash
# Print the spec.toml schema to stdout
surge artifact schema spec

# Compact (single-line) JSON
surge artifact schema spec --format json

# All nine kinds in one JSON object — useful for prompt rendering
surge artifact schema --all

# Write a schema to disk
surge artifact schema roadmap --output roadmap.schema.json
```

For markdown-only kinds (`description`, `requirements`, `story`, `plan`) the
command exits non-zero with the list of required `## <Section>` headings —
JSON Schema does not describe free-form markdown bodies, so the required
sections are surfaced explicitly instead.

## Coverage matrix

| Kind             | Primary format | JSON Schema?                            |
| ---------------- | -------------- | --------------------------------------- |
| `description`    | markdown       | no — required sections only             |
| `requirements`   | markdown       | no — required sections only             |
| `roadmap`        | TOML           | **yes** (`roadmap.json`)                |
| `roadmap-patch`  | TOML           | **yes** (`roadmap-patch.json`)          |
| `spec`           | TOML           | **yes** (`spec.json`)                   |
| `adr`            | markdown + TOML frontmatter | **yes** (frontmatter only) |
| `story`          | markdown       | no — required sections only             |
| `plan`           | markdown       | no — required sections only             |
| `flow`           | flow-toml      | pending — described by `surge_core::Graph` in Rust |

Each generated schema carries:

- `"$schema": "https://json-schema.org/draft/2020-12/schema"`
- `"$id": "https://surge.dev/schema/v1/<artifact>.json"`
- `"x-surge-schema-version": <ARTIFACT_SCHEMA_VERSION>`

`ARTIFACT_SCHEMA_VERSION` is the version baked into `schema_version` for every
TOML artifact. Bumping this constant is a coordinated, semver-significant
change.

## How an agent should use it

1. At session start, run `surge artifact schema --all` and stash the result.
2. When the agent is about to produce a `spec.toml`, render the spec schema
   into its system prompt as ground truth.
3. Before writing the file, run the resulting TOML through a local JSON
   Schema validator (most languages have one). Treat validation errors as a
   pre-flight check before invoking `surge artifact validate`.
4. For markdown kinds, include the array of required sections from
   `markdown_outline()` (also surfaced by `surge artifact schema --all` under
   `x-surge-no-json-schema` entries) into the prompt.

This keeps prompt profiles aligned with the canonical contract automatically
— if Surge bumps a schema, every consumer can re-fetch and diff in CI.

## Library API

The same data is available to Rust callers without spawning the CLI:

```rust
use surge_core::{contract_summary, json_schema_for, ArtifactKind};

let schema = json_schema_for(ArtifactKind::Spec).unwrap();
let summary = contract_summary(ArtifactKind::Plan);
assert!(summary.json_schema.is_none());
assert_eq!(summary.required_markdown_sections, Some(&["Settings", "Tasks"][..]));
```

See `crates/surge-core/src/artifact_contract.rs` for the full surface.

## Why no schema for `flow.toml` yet

`flow.toml` deserializes into `surge_core::Graph`, which transitively pulls
in the full node, edge, and per-archetype configuration tree. Exporting that
schema requires adding `JsonSchema` derives across roughly two dozen
graph-domain types — a larger surface than the rest of the artifact set
combined. Until that lands, the flow contract continues to be enforced by
`validate_artifact(ArtifactKind::Flow, …)` plus engine-level graph
validation, and the schema export returns an explanatory error.
