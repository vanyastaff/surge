---
status: accepted
deciders: vanyastaff
date: 2026-05-07
supersedes: none
---

# ADR 0005 — Archetype catalog and selection

## Context

The roadmap's "Bootstrap & adaptive flow generation" milestone calls for archetype detection inside Flow Generator: linear-3, linear-with-Review, multi-milestone outer-loop, bug-fix, refactor, spike, single-task. We also ship a `--template=<name>` skip path that bypasses bootstrap entirely.

Each archetype is a graph topology shaped to a category of work. Without a stable catalog, Flow Generator output is implicitly typed by graph shape only, telemetry has nothing to bucket runs by, and `--template` lookup has no canonical name set.

## Decision

### Closed `ArchetypeName` enum

Seven first-party archetype names live as a closed enum in `surge-core`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ArchetypeName {
    Linear3,
    LinearWithReview,
    MultiMilestone,
    BugFix,
    Refactor,
    Spike,
    SingleTask,
}
```

`#[non_exhaustive]` retrofit so adding an archetype later is non-breaking for downstream pattern-matchers. Adding a new archetype requires:

1. A new variant.
2. A new bundled `crates/surge-core/bundled/flows/<name>-1.0.toml`.
3. An entry in the Flow Generator system prompt (`crates/surge-core/bundled/profiles/flow-generator-1.0.toml`).
4. A validator topology rule in `crates/surge-orchestrator/src/engine/validate.rs` if the topology has structural invariants (e.g., `MultiMilestone` requires the outer loop over `roadmap.milestones`).

User-defined archetypes are not first-class — users author full `flow.toml` files instead.

### `ArchetypeMetadata` carrier

`ArchetypeMetadata` lives on the existing `GraphMetadata` struct (`crates/surge-core/src/graph.rs:18-24`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchetypeMetadata {
    pub name: ArchetypeName,
    pub milestones: Option<u32>,
    pub edit_loop_cap: Option<u32>,
}

pub struct GraphMetadata {
    pub name: String,
    pub description: Option<String>,
    pub template_origin: Option<TemplateKey>,
    pub created_at: DateTime<Utc>,
    pub author: Option<String>,
    #[serde(default)]
    pub archetype: Option<ArchetypeMetadata>,
}
```

Older graphs without the field deserialize to `archetype: None` — backward compatible with all M6 / M7 fixtures.

### TOML surface

The TOML surface uses the existing `[metadata]` table and a nested `[metadata.archetype]` table:

```toml
[metadata]
name = "feature-cart-discount"
created_at = "2026-05-07T10:00:00Z"

[metadata.archetype]
name = "multi-milestone"
milestones = 3
edit_loop_cap = 3
```

Top-level `[archetype]` shortcut is **not** supported. The nesting matches existing TOML conventions across the codebase (`tools.default_mcp` etc.) and is unambiguous.

### Catalog (file → archetype mapping)

| Archetype name      | Bundled flow asset                                                    | Topology summary                                                                                                |
| ------------------- | --------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `linear-3`          | `crates/surge-core/bundled/flows/linear-3-1.0.toml`                   | Spec → Implement → Verify (no Review).                                                                          |
| `linear-with-review`| `crates/surge-core/bundled/flows/linear-with-review-1.0.toml`         | Spec → Implement → Verify → Review.                                                                             |
| `multi-milestone`   | `crates/surge-core/bundled/flows/multi-milestone-1.0.toml`            | Outer `Loop` over `roadmap.milestones`; body subgraph contains an inner task `Loop` with HumanGate per task.    |
| `bug-fix`           | `crates/surge-core/bundled/flows/bug-fix-1.0.toml`                    | Reproduce → Implement → Verify → Review. The Reproduce node is the differentiator.                              |
| `refactor`          | `crates/surge-core/bundled/flows/refactor-1.0.toml`                   | BehaviorCharacterization → Refactor → Verify. Test author runs first to pin behavior before changes.            |
| `spike`             | `crates/surge-core/bundled/flows/spike-1.0.toml`                      | Implement → Verify. No Architect, no Reviewer. Output is documented learning, not merged code.                  |
| `single-task`       | `crates/surge-core/bundled/flows/single-task-1.0.toml`                | Single Agent node with terminal. Used for trivial bot-friendly tasks.                                           |

Each bundled file is a copy of (or close descendant of) its `examples/flow_*.toml` sibling that already exists today and was verified to pass `validate_for_m6` during the planning code-review pass.

### Validator topology rules

Beyond the generic graph invariants enforced by `validate_for_m6`, the post-Flow-Generator hook (Task 11 of the plan) enforces:

- `multi-milestone`: graph must contain a `Loop` node whose `iterates_over` resolves to `Artifact("roadmap.milestones")`. The body subgraph must contain at least one inner `Loop`. Mismatch → `BootstrapError::BootstrapArchetypeMismatch`.
- `bug-fix`: graph must contain at least one Agent node bound to a profile whose `role.id == "bug-fix-implementer"` (or that extends it). Soft check — a WARN-level log if missing, not a fail; archetypes are guidance, not strict enforcement.
- `refactor`: similar soft check for a `refactor-implementer` profile.
- All other archetypes: generic validation only.

Strict enforcement is reserved for shapes that the runtime depends on for correctness (e.g., the multi-milestone outer loop is consumed by milestone-progression code in the runtime). Profile-bind soft checks are advisory.

### `--template=<name>` lookup

The `ArchetypeRegistry` (Task 14) mirrors `ProfileRegistry` from M7 with a 3-way lookup:

1. User template at `~/.surge/templates/<name>.toml`
2. User template at `~/.surge/templates/<name>-<version>.toml`
3. Bundled template at `crates/surge-core/bundled/flows/<name>-1.0.toml`

The template name on the CLI matches `ArchetypeName::serialize()` (kebab-case). User templates are validated as full `Graph` values on load — no embedded archetype-name discipline is enforced.

## Consequences

**Preserves:**

- Closed-enum invariants on `NodeKind`, `EdgeKind`, terminal outcomes (no new variants).
- Backward compatibility with all existing `flow.toml` fixtures (the new field is `Option<_>` with `#[serde(default)]`).
- The "engine is dumb" principle — archetype metadata is a tag the engine reads to surface telemetry; it does not pick the next node.

**Enables:**

- Telemetry buckets runs by archetype.
- Replay UI labels each run with its archetype.
- `--template=<name>` is symmetric with bootstrap output (same on-disk shape).
- Future archetypes are additive — drop a `<name>-1.0.toml`, add an enum variant, ship a Flow Generator prompt update.

**Forces:**

- Flow Generator's prompt (Task 12) MUST list these seven archetypes and emit an `[metadata.archetype]` block in its output. The post-stage hook (Task 11) rejects output without the block.

## Out of Scope

- User-defined archetype names with first-class status — users author full `flow.toml` files; we do not provide a "extends archetype X" shortcut.
- Mid-run archetype mutation (e.g., starting as `bug-fix` and morphing into `linear-3`) — the field is set once at materialization.
- Cross-archetype composition — composing two archetypes into one bigger one is a Flow Generator responsibility, not a graph-engine feature.
