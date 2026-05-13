pub mod agent;
pub mod analytics;
pub mod artifact;
pub mod bootstrap;
pub mod config;
pub mod daemon;
pub mod doctor;
pub mod engine;
pub mod feature;
pub mod format;
pub mod git;
pub mod init;
pub mod insights;
pub mod memory;
pub mod migrate_spec;
pub mod profile;
pub mod project;
pub mod registry;
pub mod telegram;
pub mod tracker;

// Fuzzy-resolve a spec id (full ULID, prefix, or filename) to a
// `LegacySpecFile` loaded from `.surge/specs/`. Used by analytics /
// insights / memory subcommands that still query historical persistence
// data tagged with the legacy spec id.
//
// When those queries migrate to `run_id` (engine path), this helper and
// the surrounding `crate::legacy_spec` module can be retired.
pub fn load_spec_by_id(id: &str) -> anyhow::Result<crate::legacy_spec::LegacySpecFile> {
    let path = std::path::Path::new(id);
    if path.exists() {
        return crate::legacy_spec::LegacySpecFile::load(path);
    }

    let specs_dir = crate::legacy_spec::LegacySpecFile::specs_dir()?;
    let with_ext = specs_dir.join(format!("{id}.toml"));
    if with_ext.exists() {
        return crate::legacy_spec::LegacySpecFile::load(&with_ext);
    }

    // `list_all` already calls `LegacySpecFile::load` for every entry and
    // populates `path` on the returned value, so we can return the matched
    // spec directly instead of re-reading it from disk.
    let specs = crate::legacy_spec::LegacySpecFile::list_all()?;
    for (_spec_path, spec_file) in specs {
        if spec_file.spec.id.to_string().contains(id) {
            return Ok(spec_file);
        }
    }

    anyhow::bail!("Spec '{}' not found in .surge/specs/", id)
}
