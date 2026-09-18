//! The project-local `.surge/` layer — paths only, no I/O.
//!
//! A repository may carry its own profiles, flows and skills under
//! `.surge/` so the agents and flows the system composes travel with the
//! code (ADR-0020). [`ProjectLayer`] names those directories for one
//! project root; [`Layer`] names the three places a profile, flow or skill
//! can come from, in precedence order.
//!
//! Precedence is **project → home → bundled**: a file in the repository
//! shadows one with the same name under `SURGE_HOME`, which shadows the
//! bundled copy compiled into the binary. Shadowing is never silent — the
//! resolver records where the winner came from
//! ([`crate::profile::registry::Provenance`]) on every resolution.
//!
//! Nothing here reads the filesystem. Scanning, parsing and (later) trust
//! pinning of project files are the orchestrator's and persistence's
//! business; this module only agrees on *where* to look.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directory name of the project-local layer inside a repository root.
pub const PROJECT_LAYER_DIR: &str = ".surge";

const PROFILES_SUBDIR: &str = "profiles";
const FLOWS_SUBDIR: &str = "flows";
const SKILLS_SUBDIR: &str = "skills";

/// Where a repository keeps its own profiles, flows and skills.
///
/// Built once per run from the project root
/// ([`ProjectLayer::for_project`]) and carried on the run configuration so
/// two repositories served by one daemon never see each other's files: the
/// registry a run resolves against is scoped to *this* layer, not to the
/// process.
///
/// The three directories are always derived from `root` — there is no
/// constructor that takes them separately, and the serialized form is the
/// root alone (`{"root": ".."}`). A run configuration crosses the daemon's
/// IPC boundary, so a client can name *which repository* a run belongs to
/// but cannot point one lane at an arbitrary directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "ProjectLayerWire", into = "ProjectLayerWire")]
pub struct ProjectLayer {
    root: PathBuf,
    profiles_dir: PathBuf,
    flows_dir: PathBuf,
    skills_dir: PathBuf,
}

/// Serialized shape of [`ProjectLayer`]: the root only; the lanes are
/// re-derived on the receiving side.
#[derive(Serialize, Deserialize)]
struct ProjectLayerWire {
    root: PathBuf,
}

impl From<ProjectLayerWire> for ProjectLayer {
    fn from(wire: ProjectLayerWire) -> Self {
        Self::for_project(wire.root)
    }
}

impl From<ProjectLayer> for ProjectLayerWire {
    fn from(layer: ProjectLayer) -> Self {
        Self { root: layer.root }
    }
}

impl ProjectLayer {
    /// Derive the canonical layer for a repository root:
    /// `<root>/.surge/{profiles,flows,skills}`.
    ///
    /// Pure path arithmetic — the directories need not exist. A layer
    /// whose directories are absent is simply empty, which is what a
    /// repository without `.surge/` gets.
    #[must_use]
    pub fn for_project(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let layer_dir = root.join(PROJECT_LAYER_DIR);
        Self {
            profiles_dir: layer_dir.join(PROFILES_SUBDIR),
            flows_dir: layer_dir.join(FLOWS_SUBDIR),
            skills_dir: layer_dir.join(SKILLS_SUBDIR),
            root,
        }
    }

    /// The repository root the layer belongs to (the parent of `.surge/`).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/.surge` — the layer directory itself.
    #[must_use]
    pub fn layer_dir(&self) -> PathBuf {
        self.root.join(PROJECT_LAYER_DIR)
    }

    /// `<root>/.surge/profiles` — project profiles, flat `*.toml`.
    #[must_use]
    pub fn profiles_dir(&self) -> &Path {
        &self.profiles_dir
    }

    /// `<root>/.surge/flows` — project flow graphs.
    #[must_use]
    pub fn flows_dir(&self) -> &Path {
        &self.flows_dir
    }

    /// `<root>/.surge/skills` — project skill packs.
    #[must_use]
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }
}

/// The three places a profile, flow or skill can come from.
///
/// Declaration order is precedence order: [`Layer::Project`] shadows
/// [`Layer::Home`], which shadows [`Layer::Bundled`]. The derived `Ord`
/// follows that order, so `min()` over a set of candidates picks the
/// winner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// The repository's own `.surge/` ([`ProjectLayer`]).
    Project,
    /// The user's `SURGE_HOME` (`~/.surge` by default).
    Home,
    /// Compiled into the binary.
    Bundled,
}

impl Layer {
    /// Every layer, highest precedence first.
    pub const ALL: [Layer; 3] = [Layer::Project, Layer::Home, Layer::Bundled];

    /// Stable lower-case name (`project`, `home`, `bundled`) — the same
    /// spelling serde uses, for log fields and CLI tables.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Project => "project",
            Layer::Home => "home",
            Layer::Bundled => "bundled",
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_project_derives_the_four_paths_under_dot_surge() {
        let layer = ProjectLayer::for_project("/repo");
        assert_eq!(layer.root(), Path::new("/repo"));
        assert_eq!(layer.layer_dir(), PathBuf::from("/repo/.surge"));
        assert_eq!(layer.profiles_dir(), Path::new("/repo/.surge/profiles"));
        assert_eq!(layer.flows_dir(), Path::new("/repo/.surge/flows"));
        assert_eq!(layer.skills_dir(), Path::new("/repo/.surge/skills"));
    }

    #[test]
    fn for_project_does_no_io() {
        // A root that cannot exist still yields a layer: the type is paths only.
        let layer = ProjectLayer::for_project("/definitely/not/here/surge-test");
        assert!(!layer.root().exists());
        assert!(layer.profiles_dir().ends_with(".surge/profiles"));
    }

    #[test]
    fn layer_precedence_is_project_then_home_then_bundled() {
        assert!(Layer::Project < Layer::Home);
        assert!(Layer::Home < Layer::Bundled);
        assert_eq!(Layer::ALL.iter().min(), Some(&Layer::Project));
        assert_eq!(Layer::ALL, [Layer::Project, Layer::Home, Layer::Bundled]);
    }

    #[test]
    fn layer_display_and_serde_agree() {
        for layer in Layer::ALL {
            let json = serde_json::to_string(&layer).unwrap();
            assert_eq!(json, format!("\"{layer}\""));
            let back: Layer = serde_json::from_str(&json).unwrap();
            assert_eq!(back, layer);
        }
    }

    #[test]
    fn project_layer_serializes_as_its_root_only() {
        let layer = ProjectLayer::for_project("/repo");
        let json = serde_json::to_string(&layer).unwrap();
        assert_eq!(json, r#"{"root":"/repo"}"#);
        let back: ProjectLayer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layer);
    }

    #[test]
    fn deserializing_cannot_point_a_lane_outside_the_root() {
        // Lane fields on the wire are unknown and ignored; the lanes are
        // always re-derived from `root`.
        let json = r#"{"root":"/repo","profiles_dir":"/etc/evil"}"#;
        let layer: ProjectLayer = serde_json::from_str(json).unwrap();
        assert_eq!(layer.profiles_dir(), Path::new("/repo/.surge/profiles"));
    }
}
