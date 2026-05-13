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
use serde::Deserialize;
use surge_core::spec::Spec;

/// On-disk shape of a legacy `.spec.toml` file.
///
/// `path` is populated by [`Self::load`] so consumers can render diagnostic
/// errors with the originating file. The struct is deserialize-only; the
/// integration tests for `surge migrate-spec` inline a tiny serializable
/// twin instead of taking a write dependency on this surface.
#[derive(Debug, Clone, Deserialize)]
pub struct LegacySpecFile {
    /// The spec payload.
    pub spec: Spec,
    /// Path the file was loaded from, when known.
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
