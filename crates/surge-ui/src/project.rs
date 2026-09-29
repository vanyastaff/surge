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

    /// How many unpinned projects are remembered; pinned ones never age out.
    pub const MAX_UNPINNED: usize = 12;

    /// Add or update a project in the list. Moves it to the top, keeps its
    /// pin, and forgets the oldest unpinned entries past [`Self::MAX_UNPINNED`].
    pub fn touch(&mut self, name: &str, path: &Path) {
        let previous = self.projects.iter().position(|p| p.path == path);
        let pinned = previous.is_some_and(|i| self.projects[i].pinned);
        let active_tasks = previous.map_or(0, |i| self.projects[i].active_tasks);
        if let Some(i) = previous {
            self.projects.remove(i);
        }

        self.projects.insert(
            0,
            RecentProject {
                name: name.to_string(),
                path: path.to_path_buf(),
                last_opened: chrono_now(),
                pinned,
                active_tasks,
            },
        );

        let mut unpinned = 0;
        self.projects.retain(|p| {
            if p.pinned {
                return true;
            }
            unpinned += 1;
            unpinned <= Self::MAX_UNPINNED
        });
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

/// Canonical git common directory of the repository rooted exactly at
/// `path` (no upward search). A linked worktree reports its source
/// repository's common dir, which is what ties an isolated run back to
/// the project it was started from. `None` when `path` is not a
/// repository root or no longer exists (e.g. a removed worktree).
pub fn git_common_dir(path: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::open(path).ok()?;
    repo.commondir().canonicalize().ok()
}

/// Which runs belong to the open project.
///
/// Runs record where they executed (`RunStarted.project_path`), which for
/// isolated runs is a worktree under `$SURGE_HOME/worktrees`. Matching on
/// that path alone would hide a project's own runs and matching nothing
/// would mix every project's history into one Fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectScope {
    root: PathBuf,
    git_common_dir: Option<PathBuf>,
}

impl ProjectScope {
    /// Resolve the scope for a project root (does filesystem/git IO once).
    pub fn resolve(root: &Path) -> Self {
        Self {
            root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            git_common_dir: git_common_dir(root),
        }
    }

    /// `Some(true/false)` when ownership is decidable from the run's
    /// recorded identity, `None` when the run's `RunStarted` has not been
    /// observed yet.
    pub fn owns(&self, run_path: Option<&Path>, run_common_dir: Option<&Path>) -> Option<bool> {
        if let (Some(ours), Some(theirs)) = (&self.git_common_dir, run_common_dir) {
            return Some(ours == theirs);
        }
        let run_path = run_path?;
        let run_path = run_path
            .canonicalize()
            .unwrap_or_else(|_| run_path.to_path_buf());
        Some(run_path.starts_with(&self.root))
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn touch_keeps_pin_and_caps_only_unpinned() {
        let mut recent = RecentProjects::default();
        recent.touch("keep", Path::new("/p/keep"));
        recent.toggle_pin(Path::new("/p/keep"));
        for i in 0..(RecentProjects::MAX_UNPINNED + 5) {
            recent.touch(&format!("p{i}"), Path::new(&format!("/p/{i}")));
        }
        recent.touch("keep", Path::new("/p/keep"));

        let keep = recent
            .projects
            .iter()
            .find(|p| p.path == Path::new("/p/keep"));
        assert!(keep.is_some_and(|p| p.pinned), "re-opening must not unpin");
        let unpinned = recent.projects.iter().filter(|p| !p.pinned).count();
        assert_eq!(unpinned, RecentProjects::MAX_UNPINNED);
        // The newest unpinned entries survive; the oldest aged out.
        let last = RecentProjects::MAX_UNPINNED + 4;
        assert!(
            recent
                .projects
                .iter()
                .any(|p| p.path == Path::new(&format!("/p/{last}")))
        );
        assert!(!recent.projects.iter().any(|p| p.path == Path::new("/p/0")));
    }
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

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn scope_owns_worktree_runs_of_its_repository_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let other = tmp.path().join("other");
        for repo in [&project, &other] {
            std::fs::create_dir_all(repo).expect("mkdir");
            git(repo, &["init", "-q", "-b", "main"]);
            git(
                repo,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "base",
                ],
            );
        }
        let worktree = tmp.path().join("run-wt");
        git(
            &project,
            &[
                "worktree",
                "add",
                "-q",
                worktree.to_str().expect("utf8"),
                "-b",
                "run",
            ],
        );

        let scope = ProjectScope::resolve(&project);
        let owns = |path: &Path| scope.owns(Some(path), git_common_dir(path).as_deref());

        assert_eq!(owns(&worktree), Some(true), "isolated worktree run is ours");
        assert_eq!(owns(&project), Some(true), "in-place run is ours");
        assert_eq!(owns(&other), Some(false), "another repository is not ours");
        // A removed worktree has no git identity left; the path decides.
        assert_eq!(
            scope.owns(Some(&tmp.path().join("gone")), None),
            Some(false)
        );
        assert_eq!(scope.owns(None, None), None, "unknown until RunStarted");
    }
}
