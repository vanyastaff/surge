//! Artifact contract metadata and the canonical contract table.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use super::path::{is_adr_path, is_story_path, normalize_path};

/// Current schema version used by Surge-owned artifact contracts.
pub const ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// Role artifact families that Surge validates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ArtifactKind {
    /// Bootstrap description artifact.
    Description,
    /// Product requirements artifact.
    Requirements,
    /// Roadmap Planner artifact.
    Roadmap,
    /// Roadmap amendment patch artifact.
    RoadmapPatch,
    /// Spec Author artifact.
    Spec,
    /// Architect decision artifact.
    Adr,
    /// Long-form subtask story artifact.
    Story,
    /// Implementation plan artifact.
    Plan,
    /// Executable `flow.toml` graph artifact.
    Flow,
}

impl ArtifactKind {
    /// Stable kebab-case identifier used in profile metadata and CLI flags.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Description => "description",
            Self::Requirements => "requirements",
            Self::Roadmap => "roadmap",
            Self::RoadmapPatch => "roadmap-patch",
            Self::Spec => "spec",
            Self::Adr => "adr",
            Self::Story => "story",
            Self::Plan => "plan",
            Self::Flow => "flow",
        }
    }

    /// Return the contract metadata for this kind.
    #[must_use]
    pub const fn contract(self) -> ArtifactContract {
        contract_for(self)
    }
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse an artifact kind from a stable CLI/profile identifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown artifact kind {input:?}")]
pub struct ParseArtifactKindError {
    input: String,
}

impl FromStr for ArtifactKind {
    type Err = ParseArtifactKindError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "description" | "description-md" => Ok(Self::Description),
            "requirements" | "requirements-md" => Ok(Self::Requirements),
            "roadmap" | "roadmap-md" | "roadmap-toml" => Ok(Self::Roadmap),
            "roadmap-patch" | "roadmap_patch" | "roadmap-patch-toml" => Ok(Self::RoadmapPatch),
            "spec" | "spec-md" | "spec-toml" => Ok(Self::Spec),
            "adr" | "architecture-decision-record" => Ok(Self::Adr),
            "story" | "story-file" => Ok(Self::Story),
            "plan" | "implementation-plan" => Ok(Self::Plan),
            "flow" | "flow-toml" => Ok(Self::Flow),
            _ => Err(ParseArtifactKindError {
                input: input.to_owned(),
            }),
        }
    }
}

/// Primary serialization format for an artifact contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ArtifactFormat {
    /// Markdown with required headings and optional structured sections.
    Markdown,
    /// Human-authored TOML parsed into typed Rust structures.
    Toml,
    /// `flow.toml` graph parsed as [`crate::graph::Graph`].
    FlowToml,
}

/// Component that owns the schema-version field for a contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SchemaVersionOwner {
    /// Generic artifact contract version from [`ARTIFACT_SCHEMA_VERSION`].
    ArtifactContract,
    /// The graph schema version in [`crate::graph::SCHEMA_VERSION`].
    Graph,
    /// No machine-readable schema-version field is required.
    HumanReadable,
}

/// Stable reference embedded in profiles and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ArtifactContractRef {
    /// Artifact family.
    pub kind: ArtifactKind,
    /// Expected schema version for this artifact family.
    pub schema_version: u32,
}

impl ArtifactContractRef {
    /// Create a reference for the current version of `kind`.
    #[must_use]
    pub const fn current(kind: ArtifactKind) -> Self {
        Self {
            kind,
            schema_version: schema_version_for_kind(kind),
        }
    }
}

/// Canonical metadata for one artifact family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArtifactContract {
    /// Artifact family.
    pub kind: ArtifactKind,
    /// Canonical path or path pattern relative to the run worktree.
    pub canonical_path: &'static str,
    /// Primary representation agents should produce.
    pub primary_format: ArtifactFormat,
    /// Optional compatibility artifact when humans also need Markdown.
    pub markdown_compatibility: Option<&'static str>,
    /// Schema-version ownership.
    pub schema_version_owner: SchemaVersionOwner,
    /// Stable validator id used by CLI flags and diagnostics.
    pub validator_kind: &'static str,
    /// Accepted historical or ergonomic aliases for the artifact path/name.
    pub aliases: &'static [&'static str],
}

impl ArtifactContract {
    /// Return a versioned reference to this contract.
    #[must_use]
    pub const fn reference(self) -> ArtifactContractRef {
        ArtifactContractRef::current(self.kind)
    }

    /// True if `path` matches the canonical path or one of the accepted aliases.
    #[must_use]
    pub fn accepts_path(self, path: &Path) -> bool {
        let normalized = normalize_path(path);
        if normalized == self.canonical_path {
            return true;
        }
        self.aliases.iter().any(|alias| normalized == *alias)
            || self.accepts_pattern(normalized.as_str())
    }

    fn accepts_pattern(self, normalized: &str) -> bool {
        match self.kind {
            ArtifactKind::Adr => is_adr_path(normalized),
            ArtifactKind::Story => is_story_path(normalized),
            _ => false,
        }
    }
}

/// Return every canonical artifact contract in stable order.
#[must_use]
pub const fn all_contracts() -> &'static [ArtifactContract] {
    &CONTRACTS
}

/// Return the contract metadata for `kind`.
#[must_use]
pub const fn contract_for(kind: ArtifactKind) -> ArtifactContract {
    match kind {
        ArtifactKind::Description => DESCRIPTION_CONTRACT,
        ArtifactKind::Requirements => REQUIREMENTS_CONTRACT,
        ArtifactKind::Roadmap => ROADMAP_CONTRACT,
        ArtifactKind::RoadmapPatch => ROADMAP_PATCH_CONTRACT,
        ArtifactKind::Spec => SPEC_CONTRACT,
        ArtifactKind::Adr => ADR_CONTRACT,
        ArtifactKind::Story => STORY_CONTRACT,
        ArtifactKind::Plan => PLAN_CONTRACT,
        ArtifactKind::Flow => FLOW_CONTRACT,
    }
}

pub(super) const fn schema_version_for_kind(kind: ArtifactKind) -> u32 {
    match contract_for(kind).schema_version_owner {
        SchemaVersionOwner::Graph => crate::graph::SCHEMA_VERSION,
        SchemaVersionOwner::ArtifactContract | SchemaVersionOwner::HumanReadable => {
            ARTIFACT_SCHEMA_VERSION
        },
    }
}

const DESCRIPTION_ALIASES: &[&str] = &[];
const REQUIREMENTS_ALIASES: &[&str] = &["requirements.md"];
const ROADMAP_ALIASES: &[&str] = &["roadmap.md"];
const ROADMAP_PATCH_ALIASES: &[&str] = &["roadmap_patch.toml"];
const SPEC_ALIASES: &[&str] = &["spec.md"];
const ADR_ALIASES: &[&str] = &["adr.md"];
const STORY_ALIASES: &[&str] = &[];
const PLAN_ALIASES: &[&str] = &["plan.md"];
const FLOW_ALIASES: &[&str] = &[];

const DESCRIPTION_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Description,
    canonical_path: "description.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "description",
    aliases: DESCRIPTION_ALIASES,
};

const REQUIREMENTS_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Requirements,
    canonical_path: "requirements.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "requirements",
    aliases: REQUIREMENTS_ALIASES,
};

const ROADMAP_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Roadmap,
    canonical_path: "roadmap.toml",
    primary_format: ArtifactFormat::Toml,
    markdown_compatibility: Some("roadmap.md"),
    schema_version_owner: SchemaVersionOwner::ArtifactContract,
    validator_kind: "roadmap",
    aliases: ROADMAP_ALIASES,
};

const ROADMAP_PATCH_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::RoadmapPatch,
    canonical_path: "roadmap-patch.toml",
    primary_format: ArtifactFormat::Toml,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::ArtifactContract,
    validator_kind: "roadmap-patch",
    aliases: ROADMAP_PATCH_ALIASES,
};

const SPEC_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Spec,
    canonical_path: "spec.toml",
    primary_format: ArtifactFormat::Toml,
    markdown_compatibility: Some("spec.md"),
    schema_version_owner: SchemaVersionOwner::ArtifactContract,
    validator_kind: "spec",
    aliases: SPEC_ALIASES,
};

const ADR_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Adr,
    canonical_path: "docs/adr/<NNNN>-<slug>.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "adr",
    aliases: ADR_ALIASES,
};

const STORY_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Story,
    canonical_path: "stories/story-NNN.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "story",
    aliases: STORY_ALIASES,
};

const PLAN_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Plan,
    canonical_path: "plan.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "plan",
    aliases: PLAN_ALIASES,
};

const FLOW_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Flow,
    canonical_path: "flow.toml",
    primary_format: ArtifactFormat::FlowToml,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::Graph,
    validator_kind: "flow",
    aliases: FLOW_ALIASES,
};

const CONTRACTS: [ArtifactContract; 9] = [
    DESCRIPTION_CONTRACT,
    REQUIREMENTS_CONTRACT,
    ROADMAP_CONTRACT,
    ROADMAP_PATCH_CONTRACT,
    SPEC_CONTRACT,
    ADR_CONTRACT,
    STORY_CONTRACT,
    PLAN_CONTRACT,
    FLOW_CONTRACT,
];
