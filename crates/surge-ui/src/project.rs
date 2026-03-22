use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A recently-opened project entry stored in ~/.surge/recent.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub name: String,
    pub path: PathBuf,
    pub last_opened: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub active_tasks: u32,
}

/// Container for the recent.toml file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecentProjects {
    #[serde(default)]
    pub projects: Vec<RecentProject>,
}

impl RecentProjects {
    /// Path to the recent.toml file.
    fn file_path() -> PathBuf {
        dirs_home().join(".surge").join("recent.toml")
    }

    /// Load recent projects from disk. Returns empty if file doesn't exist.
    pub fn load() -> Self {
        let path = Self::file_path();
        if !path.exists() {
            return Self::default();
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        toml::from_str(&content).unwrap_or_default()
    }

    /// Save recent projects to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Add or update a project in the list. Moves it to the top.
    pub fn touch(&mut self, name: &str, path: &Path) {
        // Remove if already exists.
        self.projects.retain(|p| p.path != path);

        let now = chrono_now();
        self.projects.insert(
            0,
            RecentProject {
                name: name.to_string(),
                path: path.to_path_buf(),
                last_opened: now,
                pinned: false,
                active_tasks: 0,
            },
        );

        // Keep max 20.
        self.projects.truncate(20);
    }

    /// Toggle pin for a project.
    pub fn toggle_pin(&mut self, path: &Path) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.path == path) {
            p.pinned = !p.pinned;
        }
        self.sort();
    }

    /// Remove a project from the list (not from disk).
    pub fn remove(&mut self, path: &Path) {
        self.projects.retain(|p| p.path != path);
    }

    /// Sort: pinned first, then by last_opened descending.
    fn sort(&mut self) {
        self.projects.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.last_opened.cmp(&a.last_opened))
        });
    }

    /// Return sorted list for display: pinned first, then by recency.
    pub fn sorted(&self) -> Vec<&RecentProject> {
        let mut refs: Vec<&RecentProject> = self.projects.iter().collect();
        refs.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.last_opened.cmp(&a.last_opened))
        });
        refs
    }
}

fn dirs_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn chrono_now() -> String {
    // Simple ISO 8601 timestamp without chrono dependency.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{now}")
}
