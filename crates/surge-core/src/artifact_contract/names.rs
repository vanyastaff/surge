//! The one authority for "which artifact does this logical name denote?"
//! (#98).
//!
//! Agent stages name a produced artifact after its file stem, and fall back to
//! a sanitized path (`roadmap_toml`, `roadmap_md`) when two declared paths
//! share a stem. Profiles, bindings and gate copy also use dotted
//! (`roadmap.toml`) and dashed (`roadmap-toml`) spellings. Every consumer —
//! bootstrap handoff, stage bindings, roadmap target discovery and the desktop
//! app — asks this module instead of keeping its own list of spellings.
//!
//! # Rules
//! - A name is normalized case-insensitively, with `.` and `_` treated as `-`.
//! - A trailing `-toml` or `-md` names the format; the rest names the kind.
//! - A bare kind name (`roadmap`) carries no format and denotes the kind's
//!   primary format.
//! - [`select_canonical`] never guesses between competing spellings: two
//!   distinct spellings of the same kind and format, or two different refs at
//!   the same sequence, are ambiguous.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::run_state::ArtifactRef;

use super::{ArtifactFormat, ArtifactKind, contract_for};

/// What a logical artifact name denotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactName {
    /// The artifact family.
    pub kind: ArtifactKind,
    /// The format the name spells out, or `None` for a bare kind name (which
    /// denotes the kind's primary format).
    pub format: Option<ArtifactFormat>,
}

impl ArtifactName {
    /// The format this name denotes, resolving a bare name to the kind's
    /// primary format.
    #[must_use]
    pub fn effective_format(self) -> ArtifactFormat {
        self.format
            .unwrap_or_else(|| contract_for(self.kind).primary_format)
    }
}

/// Classify a logical artifact name, accepting every spelling the agent
/// stage's stem/path naming can produce. `None` for names outside every
/// artifact contract.
#[must_use]
pub fn artifact_name(name: &str) -> Option<ArtifactName> {
    let normalized: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch == '.' || ch == '_' { '-' } else { ch })
        .collect();
    let (base, format) = if let Some(base) = normalized.strip_suffix("-toml") {
        (base, Some(ArtifactFormat::Toml))
    } else if let Some(base) = normalized.strip_suffix("-md") {
        (base, Some(ArtifactFormat::Markdown))
    } else {
        (normalized.as_str(), None)
    };
    let kind = match base {
        "description" => ArtifactKind::Description,
        "requirements" => ArtifactKind::Requirements,
        "roadmap" => ArtifactKind::Roadmap,
        "roadmap-patch" => ArtifactKind::RoadmapPatch,
        "discovered-tasks" => ArtifactKind::DiscoveredTasks,
        "verification-report" => ArtifactKind::VerificationReport,
        "spec" => ArtifactKind::Spec,
        "adr" | "architecture-decision-record" => ArtifactKind::Adr,
        "story" | "story-file" => ArtifactKind::Story,
        "plan" | "implementation-plan" => ArtifactKind::Plan,
        "flow" => ArtifactKind::Flow,
        "triage-decision" => ArtifactKind::TriageDecision,
        _ => return None,
    };
    // The flow graph's TOML is its own format.
    let format = match (kind, format) {
        (ArtifactKind::Flow, Some(ArtifactFormat::Toml)) => Some(ArtifactFormat::FlowToml),
        (_, format) => format,
    };
    Some(ArtifactName { kind, format })
}

/// Two or more spellings (or two refs at one sequence) compete for one
/// artifact; the caller must not guess.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ambiguous {kind} artifact: competing names {names:?}")]
pub struct AmbiguousArtifact {
    /// The requested kind.
    pub kind: &'static str,
    /// The competing logical names.
    pub names: Vec<String>,
}

/// Canonical selection requires both unambiguous names and a valid lineage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArtifactSelectionError {
    #[error(transparent)]
    Ambiguous(#[from] AmbiguousArtifact),
    #[error(transparent)]
    Position(#[from] crate::execution_recovery::semantic_position::PositionError),
}
/// The canonical ref for `kind` in `format` (`None`: the primary format):
/// the latest ref per spelling, refusing competing spellings and
/// equal-sequence conflicts instead of guessing.
///
/// # Errors
/// [`AmbiguousArtifact`] when more than one spelling matches, or two
/// different refs of one spelling share a sequence.
pub fn select_canonical<'a>(
    refs: impl IntoIterator<Item = &'a ArtifactRef>,
    kind: ArtifactKind,
    format: Option<ArtifactFormat>,
    lineage: Option<&crate::execution_recovery::semantic_position::SeedLineage>,
) -> Result<Option<&'a ArtifactRef>, ArtifactSelectionError> {
    let wanted = format.unwrap_or_else(|| contract_for(kind).primary_format);
    let mut latest: BTreeMap<&str, &ArtifactRef> = BTreeMap::new();
    let mut conflict = false;
    for artifact in refs {
        let Some(name) = artifact_name(&artifact.name) else {
            continue;
        };
        if name.kind != kind || name.effective_format() != wanted {
            continue;
        }
        let position = artifact
            .produced_at_seq
            .ok_or(crate::execution_recovery::semantic_position::PositionError::Unrecorded)?;
        position.validate(lineage)?;
        let entry = latest.entry(artifact.name.as_str()).or_insert(artifact);
        let previous = entry
            .produced_at_seq
            .ok_or(crate::execution_recovery::semantic_position::PositionError::Unrecorded)?;
        if position.compare(previous, lineage)?.is_gt() {
            *entry = artifact;
        } else if artifact.produced_at_seq == entry.produced_at_seq && *entry != artifact {
            conflict = true;
        }
    }
    if conflict || latest.len() > 1 {
        return Err(AmbiguousArtifact {
            kind: kind.as_str(),
            names: latest.keys().map(|name| (*name).to_owned()).collect(),
        }
        .into());
    }
    Ok(latest.into_values().next())
}

/// The canonical worktree-relative path for `kind` in `format` (`None`: the
/// primary format), from the artifact contract. `None` when the contract has
/// no single path for that format (a pattern such as an ADR's numbered file,
/// or a format the kind is never produced in).
#[must_use]
pub fn canonical_seed_path(kind: ArtifactKind, format: Option<ArtifactFormat>) -> Option<PathBuf> {
    let contract = contract_for(kind);
    let path = match format {
        None => contract.canonical_path,
        Some(format) if format == contract.primary_format => contract.canonical_path,
        Some(ArtifactFormat::Markdown) => contract.markdown_compatibility?,
        Some(_) => return None,
    };
    (!path.contains('<') && !path.contains("NNN")).then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests;
