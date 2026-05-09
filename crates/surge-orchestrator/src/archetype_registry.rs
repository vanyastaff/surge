//! Runtime registry for `flow.toml` archetype templates.
//!
//! User templates live in `${SURGE_HOME}/templates/*.toml` and shadow bundled
//! templates by name. Bundled templates are provided by `surge-core`.

use std::path::{Path, PathBuf};

use surge_core::error::SurgeError;
use surge_core::graph::Graph;
use surge_core::{BundledFlow, BundledFlows};

/// Resolved archetype template plus provenance.
#[derive(Debug, Clone)]
pub struct ResolvedArchetype {
    /// Lookup name used by the registry.
    pub name: String,
    /// Parsed graph template.
    pub graph: Graph,
    /// Where the template came from.
    pub provenance: ArchetypeProvenance,
}

/// Template provenance for diagnostics and future `list` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchetypeProvenance {
    /// User-authored template file.
    User(PathBuf),
    /// Compile-time bundled template.
    Bundled,
}

/// Registry combining disk templates with bundled fallback templates.
#[derive(Debug, Clone)]
pub struct ArchetypeRegistry {
    disk: Vec<DiskTemplate>,
}

#[derive(Debug, Clone)]
struct DiskTemplate {
    path: PathBuf,
    file_stem: String,
    graph: Graph,
}

impl ArchetypeRegistry {
    /// Load `${SURGE_HOME}/templates/*.toml` and bundled fallback templates.
    ///
    /// # Errors
    /// Returns [`SurgeError`] if `${SURGE_HOME}` cannot be resolved or the
    /// templates directory cannot be read. Individual malformed template files
    /// are logged and skipped so one bad local file does not hide bundled
    /// templates.
    pub fn load() -> Result<Self, SurgeError> {
        let dir = crate::profile_loader::surge_home()?.join("templates");
        Self::from_dir(&dir)
    }

    /// Construct from an explicit user-template directory.
    ///
    /// This is primarily used by tests and keeps environment mutation out of
    /// the registry surface.
    ///
    /// # Errors
    /// Returns [`SurgeError::Io`] when the existing directory cannot be read.
    pub fn from_dir(dir: &Path) -> Result<Self, SurgeError> {
        let disk = scan_disk_templates(dir)?;
        tracing::info!(
            target: "archetype::registry",
            disk_count = disk.len(),
            bundled_count = BundledFlows::all().len(),
            dir = %dir.display(),
            "ArchetypeRegistry loaded"
        );
        Ok(Self { disk })
    }

    /// Resolve a template by name.
    ///
    /// Disk templates shadow bundled templates when their metadata name or
    /// filename stem matches `name`.
    ///
    /// # Errors
    /// Returns [`SurgeError::NotFound`] when no matching disk or bundled
    /// template exists.
    pub fn resolve(&self, name: &str) -> Result<ResolvedArchetype, SurgeError> {
        if let Some(template) = self
            .disk
            .iter()
            .find(|template| template.graph.metadata.name == name || template.file_stem == name)
        {
            tracing::debug!(
                target: "archetype::registry",
                name,
                path = %template.path.display(),
                "resolved disk archetype template"
            );
            return Ok(ResolvedArchetype {
                name: name.to_owned(),
                graph: template.graph.clone(),
                provenance: ArchetypeProvenance::User(template.path.clone()),
            });
        }

        if let Some(BundledFlow { graph, .. }) = BundledFlows::by_name_latest(name) {
            tracing::debug!(
                target: "archetype::registry",
                name,
                "resolved bundled archetype template"
            );
            return Ok(ResolvedArchetype {
                name: name.to_owned(),
                graph,
                provenance: ArchetypeProvenance::Bundled,
            });
        }

        Err(SurgeError::NotFound(format!("archetype template '{name}'")))
    }
}

fn scan_disk_templates(dir: &Path) -> Result<Vec<DiskTemplate>, SurgeError> {
    if !dir.exists() {
        tracing::debug!(
            target: "archetype::disk",
            dir = %dir.display(),
            "template dir does not exist; using bundled templates only"
        );
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
            paths.push(path);
        }
    }
    paths.sort();

    for path in paths {
        match load_disk_template(&path) {
            Ok(template) => entries.push(template),
            Err(e) => tracing::warn!(
                target: "archetype::disk",
                path = %path.display(),
                err = %e,
                "failed to parse archetype template; skipping"
            ),
        }
    }

    Ok(entries)
}

fn load_disk_template(path: &Path) -> Result<DiskTemplate, SurgeError> {
    let raw = std::fs::read_to_string(path)?;
    let graph = toml::from_str::<Graph>(&raw)
        .map_err(|e| SurgeError::Config(format!("template parse failed: {e}")))?;
    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    Ok(DiskTemplate {
        path: path.to_path_buf(),
        file_stem,
        graph,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_linear_3_resolves() {
        let registry = ArchetypeRegistry::from_dir(Path::new("definitely-missing")).unwrap();
        let resolved = registry.resolve("linear-3").unwrap();
        assert_eq!(resolved.name, "linear-3");
        assert_eq!(resolved.graph.metadata.name, "linear-3");
        assert_eq!(resolved.provenance, ArchetypeProvenance::Bundled);
    }

    #[test]
    fn bundled_archetype_templates_validate() {
        let registry = ArchetypeRegistry::from_dir(Path::new("definitely-missing")).unwrap();
        for name in [
            "linear-3",
            "linear-with-review",
            "multi-milestone",
            "bug-fix",
            "refactor",
            "spike",
            "single-task",
        ] {
            let resolved = registry.resolve(name).unwrap();
            crate::engine::validate::validate_for_m6(&resolved.graph)
                .unwrap_or_else(|e| panic!("{name} failed validation: {e}"));
        }
    }

    #[test]
    fn disk_template_shadows_bundled_by_file_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let mut graph = BundledFlows::by_name_latest("linear-3").unwrap().graph;
        graph.metadata.name = "custom-linear".into();
        let toml = toml::to_string(&graph).unwrap();
        std::fs::write(tmp.path().join("linear-3.toml"), toml).unwrap();

        let registry = ArchetypeRegistry::from_dir(tmp.path()).unwrap();
        let resolved = registry.resolve("linear-3").unwrap();

        assert_eq!(resolved.graph.metadata.name, "custom-linear");
        assert!(matches!(resolved.provenance, ArchetypeProvenance::User(_)));
    }
}
