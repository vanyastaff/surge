use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use surge_core::home::surge_home_dir;

/// A recently-opened project entry stored in `$SURGE_HOME/recent.toml`
/// (or `~/.surge/recent.toml` when `SURGE_HOME` is unset/empty).
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
    /// Path to the recent.toml file: `$SURGE_HOME/recent.toml` when
    /// `SURGE_HOME` is set and non-empty, else `~/.surge/recent.toml`.
    ///
    /// Delegates to [`surge_core::home::surge_home_dir`] — the canonical
    /// resolver the CLI, daemon, and persistence layer already share — so
    /// a `SURGE_HOME`-isolated `surge-ui` process (commit #79) reads and
    /// writes recent-projects state from that same sandbox instead of
    /// always falling back to the operator's real `~/.surge`.
    fn file_path() -> PathBuf {
        Self::file_path_under(surge_home_dir())
    }

    /// `recent.toml` under `surge_home`, or under `./.surge` when
    /// `surge_home` is `None` (the extreme case where `SURGE_HOME` is
    /// unset and the OS/user home directory itself cannot be determined
    /// either). Split out from [`Self::file_path`] so the SURGE_HOME-vs-
    /// fallback branch is unit-testable without reading or mutating
    /// process-wide environment.
    fn file_path_under(surge_home: Option<PathBuf>) -> PathBuf {
        surge_home
            .unwrap_or_else(|| PathBuf::from(".").join(".surge"))
            .join("recent.toml")
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

fn chrono_now() -> String {
    // Simple ISO 8601 timestamp without chrono dependency.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{now}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduces the reported defect: before this fix, [`RecentProjects`]
    /// resolved `recent.toml` from the real `~/.surge` no matter what
    /// `SURGE_HOME` said, silently defeating `SURGE_HOME` isolation
    /// (commit #79) for the one piece of state this screen owns.
    /// [`RecentProjects::file_path`] now delegates to
    /// [`surge_core::home::surge_home_dir`] via
    /// [`RecentProjects::file_path_under`], proven here without mutating
    /// process-wide environment: this binary links vendored C (`libgit2`
    /// via `surge-orchestrator`, bundled `sqlite3` via `surge-persistence`)
    /// whose `getenv` reads `environ` outside std's own lock, so a
    /// `set_var`/`remove_var` race in this test binary is not provably
    /// sound under `cargo test`'s single-process run — see the project
    /// memory note `surge-env-mutation-unsound` and the same
    /// env-mutation-avoiding fix already applied for the same reason in
    /// `surge_orchestrator::project_context` (thread the value through
    /// explicitly instead of reading env inside the function under test).
    #[test]
    fn file_path_honors_a_given_surge_home() {
        let surge_home = PathBuf::from("/tmp/surge-ui-test-custom-home");
        assert_eq!(
            RecentProjects::file_path_under(Some(surge_home.clone())),
            surge_home.join("recent.toml")
        );
    }

    #[test]
    fn file_path_falls_back_to_dot_surge_when_home_is_unknown() {
        // Mirrors `surge_core::home::surge_home_dir`'s own contract: `None`
        // only when neither `SURGE_HOME` nor the OS/user home directory can
        // be determined at all.
        assert_eq!(
            RecentProjects::file_path_under(None),
            PathBuf::from(".").join(".surge").join("recent.toml")
        );
    }
}
