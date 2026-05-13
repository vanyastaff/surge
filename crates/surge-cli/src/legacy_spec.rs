//! Parser for the legacy `.spec.toml` format.
//!
//! This is the surviving on-disk shape after the `surge-spec` crate was
//! retired. The DTO re-uses [`surge_core::spec::Spec`] for the inner type
//! (so all field-level invariants stay shared), and wraps it with the
//! file-level metadata that the legacy `surge_spec::SpecFile` used to carry.
//!
//! Consumers:
//!
//! - [`crate::commands::migrate_spec`] — the only legacy-to-flow translator.
//! - [`crate::commands::load_spec_by_id`] — fuzzy id resolution for the
//!   analytics / insights / memory subcommands that still query historical
//!   persistence data by spec id.
//!
//! When the analytics surfaces migrate to `run_id`, this module can be
//! retired entirely.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use surge_core::spec::Spec;

/// On-disk shape of a legacy `.spec.toml` file.
///
/// `path` is populated by [`Self::load`] for diagnostics; it is skipped during
/// serialization so a `save` round-trip does not embed the source path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacySpecFile {
    /// The spec payload.
    pub spec: Spec,
    /// Path the file was loaded from, when known. Skipped on serialize.
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl LegacySpecFile {
    /// Load and parse a `.spec.toml` file from disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or the TOML cannot be
    /// decoded into the legacy schema.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut file: Self = toml::from_str(&content)
            .with_context(|| format!("failed to parse {} as a legacy spec", path.display()))?;
        file.path = Some(path.to_path_buf());
        Ok(file)
    }

    /// Serialize the file to TOML and write it to `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory cannot be created, the
    /// TOML serializer fails, or the write itself errors.
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)
            .with_context(|| format!("failed to serialize spec for {}", path.display()))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create directory {}", parent.display())
                })?;
            }
        }
        std::fs::write(path, content)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Resolve the project's default specs directory (`<cwd>/.surge/specs/`).
    ///
    /// # Errors
    ///
    /// Returns an error if the current working directory cannot be read.
    pub fn specs_dir() -> Result<PathBuf> {
        let cwd = std::env::current_dir()?;
        Ok(cwd.join(".surge").join("specs"))
    }

    /// Enumerate every `.toml` file under [`Self::specs_dir`] and load it.
    /// Invalid files are skipped with a `tracing::warn!` event so a corrupt
    /// fixture cannot block the entire listing.
    ///
    /// # Errors
    ///
    /// Returns an error if [`Self::specs_dir`] itself cannot be resolved or
    /// the directory cannot be read.
    pub fn list_all() -> Result<Vec<(PathBuf, Self)>> {
        let dir = Self::specs_dir()?;
        if !dir.exists() {
            return Ok(vec![]);
        }

        let mut specs = vec![];
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                match Self::load(&path) {
                    Ok(spec_file) => specs.push((path, spec_file)),
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "skipping invalid legacy spec file",
                        );
                    },
                }
            }
        }
        Ok(specs)
    }
}
