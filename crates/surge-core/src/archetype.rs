//! Archetype catalog for bootstrap-generated pipelines.
//!
//! See ADR 0005 for the full rationale.
//!
//! Adding a new archetype requires (in order):
//! 1. A new variant on [`ArchetypeName`].
//! 2. A new bundled flow under `crates/surge-core/bundled/flows/<name>-1.0.toml`.
//! 3. An entry in the Flow Generator system prompt
//!    (`crates/surge-core/bundled/profiles/flow-generator-1.0.toml`).
//! 4. A topology rule in the orchestrator's post-Flow-Generator validator if
//!    the archetype has structural invariants.

use serde::{Deserialize, Serialize};

/// Closed catalog of first-party archetypes Flow Generator can pick.
///
/// User-defined archetypes are not first-class — users author full `flow.toml`
/// values directly instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ArchetypeName {
    /// Spec → Implement → Verify (no Review).
    #[serde(rename = "linear-3")]
    Linear3,
    /// Spec → Implement → Verify → Review.
    LinearWithReview,
    /// Outer `Loop` over `roadmap.milestones`; body subgraph holds inner task `Loop`.
    MultiMilestone,
    /// Reproduce → Implement → Verify → Review.
    BugFix,
    /// BehaviorCharacterization → Refactor → Verify.
    Refactor,
    /// Implement → Verify (no Architect, no Reviewer).
    Spike,
    /// Single Agent node + Terminal.
    SingleTask,
}

impl ArchetypeName {
    /// Stable kebab-case identifier — matches the `--template=<name>` CLI flag
    /// and the bundled-flow filename prefix.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Linear3 => "linear-3",
            Self::LinearWithReview => "linear-with-review",
            Self::MultiMilestone => "multi-milestone",
            Self::BugFix => "bug-fix",
            Self::Refactor => "refactor",
            Self::Spike => "spike",
            Self::SingleTask => "single-task",
        }
    }
}

/// Metadata block attached to a `Graph` describing the archetype the graph
/// implements.
///
/// Carried via `GraphMetadata.archetype: Option<ArchetypeMetadata>` and serialized
/// under `[metadata.archetype]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchetypeMetadata {
    /// Closed enum identifying the archetype.
    pub name: ArchetypeName,
    /// For `multi-milestone` — number of milestones the outer loop iterates over.
    /// `None` for archetypes where this is not meaningful.
    #[serde(default)]
    pub milestones: Option<u32>,
    /// Optional override of the run-level `bootstrap.edit_loop_cap`. Honored only
    /// during bootstrap — not consulted by the post-bootstrap pipeline runtime.
    #[serde(default)]
    pub edit_loop_cap: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archetype_name_round_trips_kebab_case() {
        for &(name, expected) in &[
            (ArchetypeName::Linear3, "linear-3"),
            (ArchetypeName::LinearWithReview, "linear-with-review"),
            (ArchetypeName::MultiMilestone, "multi-milestone"),
            (ArchetypeName::BugFix, "bug-fix"),
            (ArchetypeName::Refactor, "refactor"),
            (ArchetypeName::Spike, "spike"),
            (ArchetypeName::SingleTask, "single-task"),
        ] {
            assert_eq!(name.as_str(), expected);
            // Deserialize accepts the same identifier.
            let de: ArchetypeName =
                serde_json::from_str(&format!("\"{expected}\"")).expect("deserialize");
            assert_eq!(de, name);
        }
    }

    #[test]
    fn archetype_metadata_round_trips_with_optional_fields_omitted() {
        let m = ArchetypeMetadata {
            name: ArchetypeName::Linear3,
            milestones: None,
            edit_loop_cap: None,
        };
        let s = toml::to_string(&m).expect("serialize");
        let parsed: ArchetypeMetadata = toml::from_str(&s).expect("parse");
        assert_eq!(parsed, m);
    }

    #[test]
    fn archetype_metadata_round_trips_with_optional_fields_present() {
        let m = ArchetypeMetadata {
            name: ArchetypeName::MultiMilestone,
            milestones: Some(3),
            edit_loop_cap: Some(5),
        };
        let s = toml::to_string(&m).expect("serialize");
        let parsed: ArchetypeMetadata = toml::from_str(&s).expect("parse");
        assert_eq!(parsed, m);
    }

    #[test]
    fn missing_archetype_block_deserializes_as_none_on_parent() {
        // Smoke test — exercise via TOML at GraphMetadata level so we hitch
        // onto the existing struct's serde path.
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            archetype: Option<ArchetypeMetadata>,
        }
        let w: Wrapper = toml::from_str("").expect("empty parses");
        assert!(w.archetype.is_none());
    }
}
