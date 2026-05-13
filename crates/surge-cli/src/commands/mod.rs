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
pub mod tracker;

// Fuzzy-resolve a spec id (full ULID, prefix, or filename) to a `SpecFile`
// loaded from `.surge/specs/`. Used by analytics/insights/memory subcommands
// that still query historical data tagged with the legacy spec_id.
//
// TODO: Phase 7 — when surge-spec is deleted, these analytics queries must
// migrate to run_id (engine path) or be retired alongside it.
#[allow(deprecated)]
pub fn load_spec_by_id(id: &str) -> anyhow::Result<surge_spec::SpecFile> {
    let path = std::path::Path::new(id);
    if path.exists() {
        return Ok(surge_spec::SpecFile::load(path)?);
    }

    let specs_dir = surge_spec::SpecFile::specs_dir()?;
    let with_ext = specs_dir.join(format!("{id}.toml"));
    if with_ext.exists() {
        return Ok(surge_spec::SpecFile::load(&with_ext)?);
    }

    let specs = surge_spec::SpecFile::list_all()?;
    for (spec_path, spec_file) in specs {
        if spec_file.spec.id.to_string().contains(id) {
            return Ok(surge_spec::SpecFile::load(&spec_path)?);
        }
    }

    anyhow::bail!("Spec '{}' not found in .surge/specs/", id)
}
