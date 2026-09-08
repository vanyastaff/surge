//! Typed failure modes for skill pack discovery and resolution.

use std::path::PathBuf;

use super::SkillProvider;

/// Failure modes surfaced while discovering or resolving a skill pack.
///
/// A malformed `SKILL.md` or `plugin.json` always yields
/// [`SkillError::MalformedFrontmatter`] / [`SkillError::MalformedPlugin`] naming the
/// offending file and the parse failure — never a panic and never a silent skip of
/// the specific pack a caller asked for.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SkillError {
    /// No pack matching the requested name/provider/version is on disk.
    #[error("skill '{name}' not found for provider {provider:?}")]
    NotFound {
        /// The requested skill name.
        name: String,
        /// The provider root that was searched.
        provider: SkillProvider,
    },

    /// More than one discovered pack shares `name` and `provider` (and, when
    /// requested, `version`) — resolving would otherwise silently pick
    /// whichever one happened to sort first.
    #[error("skill '{name}' is ambiguous for provider {provider:?}: multiple packs match")]
    Ambiguous {
        /// The requested skill name.
        name: String,
        /// The provider root that was searched.
        provider: SkillProvider,
    },

    /// `SKILL.md` exists but its frontmatter could not be parsed.
    #[error("malformed SKILL.md at {file}: {reason}")]
    MalformedFrontmatter {
        /// Path to the offending `SKILL.md`.
        file: PathBuf,
        /// Human-readable parse failure reason.
        reason: String,
    },

    /// `plugin.json` exists but could not be parsed as an Agent Plugin manifest.
    #[error("malformed plugin manifest at {file}: {reason}")]
    MalformedPlugin {
        /// Path to the offending `plugin.json`.
        file: PathBuf,
        /// Human-readable parse failure reason.
        reason: String,
    },

    /// A file that should be readable (as part of an already-recognized pack)
    /// could not be read.
    #[error("failed to read {path}: {source}")]
    Io {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
}
