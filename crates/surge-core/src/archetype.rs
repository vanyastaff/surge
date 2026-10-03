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
    /// Design/plan → Implement core → Integrate and verify.
    Feature,
    /// Baseline/profile → Optimize → Benchmark against the baseline.
    Performance,
    /// Audit → Fix findings → Security regression tests.
    Security,
    /// Outline → Write → Review against the code.
    Docs,
    /// Migration plan → Implement → Validate forward and rollback.
    Migration,
    /// Review scope → Security pass → Independent verification (no code written).
    CodeReview,
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
            Self::Feature => "feature",
            Self::Performance => "performance",
            Self::Security => "security",
            Self::Docs => "docs",
            Self::Migration => "migration",
            Self::CodeReview => "code-review",
        }
    }

    /// Every first-party archetype, in catalog order.
    ///
    /// The Flow Generator prompt, the bundled flow assets and this list must
    /// name the same set; tests on both sides check it.
    pub const ALL: [Self; 13] = [
        Self::Linear3,
        Self::LinearWithReview,
        Self::MultiMilestone,
        Self::BugFix,
        Self::Refactor,
        Self::Spike,
        Self::SingleTask,
        Self::Feature,
        Self::Performance,
        Self::Security,
        Self::Docs,
        Self::Migration,
        Self::CodeReview,
    ];

    /// Parse a kebab-case archetype name, the inverse of [`Self::as_str`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|archetype| archetype.as_str() == name)
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
    /// Optional human estimate for a typical graph node, in seconds.
    /// Historical scheduler estimates still come only from completed runs.
    #[serde(default)]
    pub node_capacity_estimate: Option<crate::capacity::WorkEstimateConfig>,
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
            (ArchetypeName::Feature, "feature"),
            (ArchetypeName::Performance, "performance"),
            (ArchetypeName::Security, "security"),
            (ArchetypeName::Docs, "docs"),
            (ArchetypeName::Migration, "migration"),
            (ArchetypeName::CodeReview, "code-review"),
        ] {
            assert_eq!(name.as_str(), expected);
            // Deserialize accepts the same identifier.
            let de: ArchetypeName =
                serde_json::from_str(&format!("\"{expected}\"")).expect("deserialize");
            assert_eq!(de, name);
            assert_eq!(ArchetypeName::from_name(expected), Some(name));
        }
    }

    #[test]
    fn all_lists_every_variant_once() {
        let names: std::collections::HashSet<&str> = ArchetypeName::ALL
            .iter()
            .map(ArchetypeName::as_str)
            .collect();
        assert_eq!(names.len(), ArchetypeName::ALL.len());
        assert_eq!(ArchetypeName::from_name("bootstrap"), None);
    }

    #[test]
    fn archetype_metadata_round_trips_with_optional_fields_omitted() {
        let m = ArchetypeMetadata {
            name: ArchetypeName::Linear3,
            milestones: None,
            edit_loop_cap: None,
            node_capacity_estimate: None,
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
            node_capacity_estimate: None,
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
