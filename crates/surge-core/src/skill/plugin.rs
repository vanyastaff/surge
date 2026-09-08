//! Agent Plugins format: a package with `plugin.json` at its root and a
//! `skills/` subdirectory holding one or more Agent Skills packs.
//!
//! Only the manifest's `version` is read here — it becomes the fallback
//! `version` for a nested skill whose own `SKILL.md` frontmatter omits one.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PluginManifest {
    #[serde(default)]
    version: Option<String>,
}

/// Parse a `plugin.json` manifest, returning its declared `version` (if any).
///
/// The caller (which already knows the file's path) is responsible for
/// wrapping the returned reason into a [`super::error::SkillError`].
///
/// # Errors
/// Returns a human-readable reason when `file` cannot be read or does not
/// parse as JSON.
pub(super) fn parse_plugin_manifest(file: &Path) -> Result<Option<String>, String> {
    let raw = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let manifest: PluginManifest = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    Ok(manifest.version)
}
