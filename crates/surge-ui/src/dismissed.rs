//! Failed runs the operator has acknowledged in the Inbox.
//!
//! A failed run stays in the event log forever; without acknowledgement it
//! also stayed in "needs you" forever, so the badge only ever grew. The set is
//! UI state (not run state), kept in `$SURGE_HOME/ui/dismissed-runs.json`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use surge_core::id::RunId;

/// Persisted set of acknowledged failed runs.
#[derive(Debug, Default)]
pub struct DismissedRuns {
    ids: HashSet<RunId>,
    path: Option<PathBuf>,
    #[cfg(windows)]
    runtime_home: Option<PathBuf>,
}

impl DismissedRuns {
    /// Load from the default location under `SURGE_HOME`.
    pub fn load() -> Self {
        surge_core::home::surge_home_dir()
            .map(|home| {
                #[cfg(windows)]
                {
                    let mut loaded = Self::load_from(home.join("ui").join("dismissed-runs.json"));
                    loaded.runtime_home = Some(home);
                    loaded
                }
                #[cfg(not(windows))]
                Self::load_from(home.join("ui").join("dismissed-runs.json"))
            })
            .unwrap_or_default()
    }

    /// Load from `path`; a missing or unreadable file is an empty set.
    pub fn load_from(path: PathBuf) -> Self {
        let ids = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<RunId>>(&text).ok())
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default();
        Self {
            ids,
            path: Some(path),
            #[cfg(windows)]
            runtime_home: None,
        }
    }

    pub fn contains(&self, run_id: &RunId) -> bool {
        self.ids.contains(run_id)
    }

    /// Acknowledge `run_id` and persist. Persistence failure keeps the
    /// in-memory acknowledgement for this session and is logged.
    pub fn insert(&mut self, run_id: RunId) {
        if !self.ids.insert(run_id) {
            return;
        }
        if let Some(path) = &self.path
            && let Err(error) = self.save_to(path)
        {
            tracing::warn!(%error, path = %path.display(), "could not persist dismissed runs");
        }
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        #[cfg(windows)]
        let _runtime_home = self
            .runtime_home
            .as_deref()
            .map(surge_persistence::RuntimeHomeOwner::prepare)
            .transpose()
            .map_err(std::io::Error::other)?;
        save(path, &self.ids)
    }
}

fn save(path: &Path, ids: &HashSet<RunId>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Serialize through serde (bare ULIDs) so `load_from` reads it back;
    // `Display` adds a `run-` prefix that serde does not accept.
    let mut sorted: Vec<&RunId> = ids.iter().collect();
    sorted.sort_by_key(|id| id.to_string());
    let text = serde_json::to_string_pretty(&sorted).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::DismissedRuns;
    use surge_core::id::RunId;

    #[test]
    fn dismissals_survive_a_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ui").join("dismissed-runs.json");
        let run = RunId::new();

        let mut first = DismissedRuns::load_from(path.clone());
        assert!(!first.contains(&run));
        first.insert(run);

        let reloaded = DismissedRuns::load_from(path);
        assert!(reloaded.contains(&run));
        assert!(!reloaded.contains(&RunId::new()));
    }
}
