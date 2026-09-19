//! The flow catalog: every flow template a task can run, with the guidance
//! the selector needs (ADR-0020).
//!
//! Three lanes, in precedence order ([`surge_core::Layer`]):
//!
//! - **Project:** `<repo>/.surge/flows/` — the repository's own templates.
//! - **Home:** `${SURGE_HOME}/flows/` — the operator's templates.
//! - **Bundled:** the compiled-in archetypes ([`BundledFlows`]).
//!
//! A template is a `flow.toml` whose `[metadata]` carries `name`,
//! `when_to_use` (the one-line fit guidance the classifier reads) and
//! `archetype`. The same graph can appear in more than one lane; the version
//! comes from the file name (`bug-fix-1.0.toml`) and the catalog reports
//! which lane won.
//!
//! This module is **paths and parsing only** — it reads the three
//! directories it is given and nothing else. Selecting a template for a task
//! is `QueuePolicy`-adjacent logic that lives with the scheduler, and
//! installing a composed template is the gate's business.

use std::collections::BTreeMap;

use crate::bundled_flows::BundledFlows;
use crate::error::SurgeError;
use crate::flow_ref::FlowRef;
use crate::graph::Graph;
use crate::project_layer::ProjectLayer;

/// A template's identity: its reference and the version it resolved at.
#[derive(Debug, Clone)]
pub struct FlowCatalogEntry {
    /// Catalog reference (`bug-fix@1.0`).
    pub reference: FlowRef,
    /// Lane the winning file came from.
    pub layer: crate::project_layer::Layer,
    /// Path of the winning file; `None` for a bundled template.
    pub path: Option<std::path::PathBuf>,
    /// One-line fit guidance, when the template declares it.
    pub when_to_use: Option<String>,
    /// Archetype tag from the template's metadata.
    pub archetype: Option<String>,
    /// Autonomy level from the template's metadata.
    pub autonomy: Option<crate::graph::AutonomyLevel>,
    /// Parsed graph.
    pub graph: Graph,
}

impl FlowCatalogEntry {
    /// Whether this entry's template declares the fit guidance the selector
    /// needs. Bundled templates are expected to; a project template without
    /// it is still usable when pinned explicitly.
    #[must_use]
    pub fn has_fit_guidance(&self) -> bool {
        self.when_to_use.is_some()
    }
}

/// Errors from catalog discovery.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FlowCatalogError {
    /// A directory could not be read.
    #[error("read flow directory {path}: {source}")]
    Io {
        /// Directory that failed.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A template file did not parse.
    #[error("parse flow template {path}: {source}")]
    Parse {
        /// File that failed.
        path: std::path::PathBuf,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },
    /// A template file's name is not a `name-MAJOR.MINOR.toml` reference.
    #[error(
        "flow template {path} does not name a version (expected `<name>-<MAJOR>.<MINOR>.toml`)"
    )]
    UnversionedName {
        /// File whose name could not be read as a reference.
        path: std::path::PathBuf,
    },
    /// The requested reference is not in the catalog. Never a fallback:
    /// a pinned flow that does not resolve is exactly the error the operator
    /// needs to see.
    #[error("no flow template matches {reference} in project, home or bundled layers")]
    NotFound {
        /// The reference that did not resolve.
        reference: String,
    },
}

/// Every flow template visible to one project.
#[derive(Debug, Clone, Default)]
pub struct FlowCatalog {
    entries: Vec<FlowCatalogEntry>,
}

impl FlowCatalog {
    /// Scan the project and home lanes and merge them with the bundled set.
    ///
    /// `home_flows_dir` is the caller's `SURGE_HOME/flows` (this crate does
    /// no environment resolution); `None` skips the home lane.
    ///
    /// # Errors
    /// [`FlowCatalogError::Io`] when an existing directory cannot be read;
    /// [`FlowCatalogError::Parse`] / [`FlowCatalogError::UnversionedName`]
    /// for a malformed template.
    pub fn scan(
        project: &ProjectLayer,
        home_flows_dir: Option<&std::path::Path>,
    ) -> Result<Self, FlowCatalogError> {
        let mut entries = Vec::new();
        if let Some(home) = home_flows_dir {
            entries.extend(scan_dir(home, crate::project_layer::Layer::Home)?);
        }
        entries.extend(scan_dir(
            project.flows_dir(),
            crate::project_layer::Layer::Project,
        )?);
        for bundled in BundledFlows::all() {
            entries.push(FlowCatalogEntry {
                reference: FlowRef::new(
                    bundled.name.clone(),
                    bundled.version.major,
                    Some(bundled.version.minor),
                )
                .unwrap_or_else(|_| {
                    // Bundled names/versions are compile-time constants;
                    // a violation is a build bug, not a runtime one.
                    unreachable!("bundled flow name/version is not a valid FlowRef")
                }),
                layer: crate::project_layer::Layer::Bundled,
                path: None,
                when_to_use: bundled.graph.metadata.when_to_use.clone(),
                archetype: bundled
                    .graph
                    .metadata
                    .archetype
                    .as_ref()
                    .map(|a| a.name.as_str().to_string()),
                autonomy: bundled.graph.metadata.autonomy,
                graph: bundled.graph,
            });
        }
        Ok(Self { entries })
    }

    /// Every entry, in lane order (project, home, bundled) and name order
    /// within a lane.
    #[must_use]
    pub fn entries(&self) -> &[FlowCatalogEntry] {
        &self.entries
    }

    /// Entries that declare fit guidance — the classifier's option set.
    #[must_use]
    pub fn described(&self) -> Vec<&FlowCatalogEntry> {
        self.entries
            .iter()
            .filter(|e| e.has_fit_guidance())
            .collect()
    }

    /// Resolve a reference to the highest matching version in the highest
    /// lane.
    ///
    /// # Errors
    /// [`FlowCatalogError::NotFound`] when nothing matches.
    pub fn resolve(&self, reference: &FlowRef) -> Result<&FlowCatalogEntry, FlowCatalogError> {
        self.entries
            .iter()
            .filter(|e| e.reference.name() == reference.name() && reference.matches(&version_of(e)))
            .max_by(|a, b| {
                a.layer
                    .cmp(&b.layer)
                    .reverse()
                    .then(version_of(a).cmp(&version_of(b)))
            })
            .ok_or_else(|| FlowCatalogError::NotFound {
                reference: reference.to_string(),
            })
    }

    /// Render the catalog as the classifier's prompt text: one row per
    /// described entry, project-first.
    #[must_use]
    pub fn render_catalog(&self) -> String {
        let mut out = String::from("Available flow templates (name@version — when to use):\n");
        for entry in self.entries.iter().filter(|e| e.has_fit_guidance()) {
            out.push_str(&format!(
                "- {} — {}",
                entry.reference,
                entry.when_to_use.as_deref().unwrap_or("")
            ));
            out.push('\n');
        }
        out
    }
}

fn version_of(entry: &FlowCatalogEntry) -> semver::Version {
    semver::Version::new(
        entry.reference.major(),
        entry.reference.minor().unwrap_or(0),
        0,
    )
}

fn scan_dir(
    dir: &std::path::Path,
    layer: crate::project_layer::Layer,
) -> Result<Vec<FlowCatalogEntry>, FlowCatalogError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| FlowCatalogError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    for path in files {
        entries.push(load_template(&path, layer)?);
    }
    Ok(entries)
}

fn load_template(
    path: &std::path::Path,
    layer: crate::project_layer::Layer,
) -> Result<FlowCatalogEntry, FlowCatalogError> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    // `<name>-<MAJOR>.<MINOR>`; a bare `bug-fix.toml` has no version and is
    // refused rather than guessed.
    let (name, version) =
        stem.rsplit_once('-')
            .ok_or_else(|| FlowCatalogError::UnversionedName {
                path: path.to_path_buf(),
            })?;
    let (major, minor) =
        version
            .split_once('.')
            .ok_or_else(|| FlowCatalogError::UnversionedName {
                path: path.to_path_buf(),
            })?;
    let major: u64 = major
        .parse()
        .map_err(|_| FlowCatalogError::UnversionedName {
            path: path.to_path_buf(),
        })?;
    let minor: u64 = minor
        .parse()
        .map_err(|_| FlowCatalogError::UnversionedName {
            path: path.to_path_buf(),
        })?;
    let reference =
        FlowRef::new(name, major, Some(minor)).map_err(|_| FlowCatalogError::UnversionedName {
            path: path.to_path_buf(),
        })?;

    let text = std::fs::read_to_string(path).map_err(|source| FlowCatalogError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let graph: Graph = toml::from_str(&text).map_err(|source| FlowCatalogError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(FlowCatalogEntry {
        reference,
        layer,
        path: Some(path.to_path_buf()),
        when_to_use: graph.metadata.when_to_use.clone(),
        archetype: graph
            .metadata
            .archetype
            .as_ref()
            .map(|a| a.name.as_str().to_string()),
        autonomy: graph.metadata.autonomy,
        graph,
    })
}

/// Group a project's catalog by name for display: one row per template name
/// with the versions and the winning lane.
#[must_use]
pub fn group_by_name(catalog: &FlowCatalog) -> BTreeMap<String, Vec<&FlowCatalogEntry>> {
    let mut map: BTreeMap<String, Vec<&FlowCatalogEntry>> = BTreeMap::new();
    for entry in catalog.entries() {
        map.entry(entry.reference.name().to_string())
            .or_default()
            .push(entry);
    }
    map
}

/// Convenience: scan a project layer with the standard `SURGE_HOME` home lane.
///
/// # Errors
/// Propagates [`FlowCatalogError`]; an unresolvable home directory degrades
/// to no home lane rather than failing.
pub fn scan_with_default_home(project: &ProjectLayer) -> Result<FlowCatalog, SurgeError> {
    let home = crate::home::surge_home_dir().map(|h| h.join("flows"));
    FlowCatalog::scan(project, home.as_deref()).map_err(|e| SurgeError::Config(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_template(dir: &std::path::Path, file: &str, name: &str, when: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();
        let when_line = when
            .map(|w| format!("when_to_use = \"{w}\"\n"))
            .unwrap_or_default();
        let body = format!(
            r#"schema_version = 1
start = "only"

[metadata]
name = "{name}"
created_at = "2026-01-01T00:00:00Z"
{when_line}
[nodes.only]
id = "only"

[nodes.only.position]
x = 0.0
y = 0.0

nodes.only.declared_outcomes = []

[nodes.only.config]
node_kind = "terminal"

[nodes.only.config.kind]
type = "success"

[[edges]]
id = "e1"
to = "only"
kind = "forward"

[edges.from]
node = "only"
outcome = "done"

[edges.policy]
on_max_exceeded = "escalate"
"#
        );
        std::fs::write(dir.join(file), body).unwrap();
    }

    fn project(root: &std::path::Path) -> ProjectLayer {
        ProjectLayer::for_project(root)
    }

    #[test]
    fn bundled_templates_are_always_present() {
        let root = tempfile::tempdir().unwrap();
        let catalog = FlowCatalog::scan(&project(root.path()), None).unwrap();
        assert!(
            catalog
                .entries()
                .iter()
                .any(|e| e.reference.name() == "bug-fix")
        );
        assert!(
            catalog
                .entries()
                .iter()
                .any(|e| e.layer == crate::project_layer::Layer::Bundled)
        );
    }

    #[test]
    fn project_template_shadows_bundled_with_the_same_reference() {
        let root = tempfile::tempdir().unwrap();
        let layer = project(root.path());
        write_template(
            layer.flows_dir(),
            "bug-fix-1.0.toml",
            "bug-fix",
            Some("project version"),
        );

        let catalog = FlowCatalog::scan(&layer, None).unwrap();
        let entry = catalog.resolve(&"bug-fix@1".parse().unwrap()).unwrap();
        assert_eq!(entry.layer, crate::project_layer::Layer::Project);
        assert_eq!(entry.when_to_use.as_deref(), Some("project version"));
    }

    #[test]
    fn higher_minor_wins_within_a_lane() {
        let root = tempfile::tempdir().unwrap();
        let layer = project(root.path());
        write_template(
            layer.flows_dir(),
            "custom-1.0.toml",
            "custom",
            Some("older"),
        );
        write_template(
            layer.flows_dir(),
            "custom-1.2.toml",
            "custom",
            Some("newer"),
        );

        let catalog = FlowCatalog::scan(&layer, None).unwrap();
        let entry = catalog.resolve(&"custom@1".parse().unwrap()).unwrap();
        assert_eq!(entry.when_to_use.as_deref(), Some("newer"));
    }

    #[test]
    fn missing_reference_is_a_named_error() {
        let root = tempfile::tempdir().unwrap();
        let catalog = FlowCatalog::scan(&project(root.path()), None).unwrap();
        let err = catalog.resolve(&"nope@1".parse().unwrap()).unwrap_err();
        assert!(matches!(err, FlowCatalogError::NotFound { .. }));
    }

    #[test]
    fn unversioned_filename_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let layer = project(root.path());
        write_template(layer.flows_dir(), "no-version.toml", "no-version", None);
        let err = FlowCatalog::scan(&layer, None).unwrap_err();
        assert!(matches!(err, FlowCatalogError::UnversionedName { .. }));
    }

    #[test]
    fn render_catalog_lists_only_described_entries() {
        let root = tempfile::tempdir().unwrap();
        let layer = project(root.path());
        write_template(
            layer.flows_dir(),
            "custom-1.0.toml",
            "custom",
            Some("custom fit"),
        );
        let catalog = FlowCatalog::scan(&layer, None).unwrap();
        let rendered = catalog.render_catalog();
        assert!(rendered.contains("custom@1.0 — custom fit"));
        // A bundled entry without guidance is not offered.
        assert!(!rendered.contains("bug-fix@1.0 —\n"));
    }

    #[test]
    fn group_by_name_gathers_versions() {
        let root = tempfile::tempdir().unwrap();
        let layer = project(root.path());
        write_template(layer.flows_dir(), "custom-1.0.toml", "custom", None);
        write_template(layer.flows_dir(), "custom-2.0.toml", "custom", None);
        let catalog = FlowCatalog::scan(&layer, None).unwrap();
        let grouped = group_by_name(&catalog);
        let custom = grouped.get("custom").unwrap();
        assert_eq!(custom.len(), 2);
        assert!(grouped.contains_key("bug-fix"));
    }

    #[test]
    fn home_lane_shadows_bundled_but_loses_to_project() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let layer = project(root.path());
        write_template(&home.path().join("flows"), "x-1.0.toml", "x", Some("home"));
        let catalog = FlowCatalog::scan(&layer, Some(&home.path().join("flows"))).unwrap();
        let entry = catalog.resolve(&"x@1".parse().unwrap()).unwrap();
        assert_eq!(entry.layer, crate::project_layer::Layer::Home);

        write_template(layer.flows_dir(), "x-1.0.toml", "x", Some("project"));
        let catalog = FlowCatalog::scan(&layer, Some(&home.path().join("flows"))).unwrap();
        let entry = catalog.resolve(&"x@1".parse().unwrap()).unwrap();
        assert_eq!(entry.layer, crate::project_layer::Layer::Project);
        let _ = PathBuf::new();
    }
}
