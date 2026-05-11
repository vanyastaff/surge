pub mod agent;
pub mod analytics;
pub mod bootstrap;
pub mod config;
pub mod daemon;
pub mod engine;
pub mod format;
pub mod git;
pub mod init;
pub mod insights;
pub mod memory;
pub mod pipeline;
pub mod profile;
pub mod project;
pub mod registry;
pub mod spec;
pub mod tracker;

/// Load a spec by ID or filename.
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

    anyhow::bail!("Spec '{}' not found. Check surge spec list.", id)
}
