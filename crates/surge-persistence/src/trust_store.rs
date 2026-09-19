//! Load-time trust for repo-resident `.surge/` composition (ADR-0020, T13).
//!
//! A repository's `.surge/` directory carries profiles, flows and skills —
//! executable context the system composed *for this repo*. That is exactly
//! the shape a hostile repository would use to run code on a fresh clone:
//! the files are in git, so "the user wrote them" is not true until the
//! user has seen them.
//!
//! The trust store is one TOML file per repository under
//! `SURGE_HOME/trust/<repo-id>.toml`, mapping a project-relative path to the
//! content hash the operator accepted. Before a run loads a project file,
//! [`TrustStore::check`] compares its current hash with the pinned one:
//!
//! - **Pinned and unchanged** → load silently.
//! - **New file, or changed hash** → the run is not started and
//!   `EscalationRequested { cause: UntrustedProjectFile }` is raised with
//!   both hashes; `surge trust accept <path>` pins the current content.
//!
//! The gate that installs a composed artefact pins it in the same step, so
//! the system's own output is never re-prompted.
//!
//! `repo_id` is a hash of the repository's canonical remote URL when it has
//! one, else the canonicalized root path — the same repository cloned to a
//! different directory has the same id, so a fresh clone of a repo whose
//! files were already trusted does not re-prompt.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use surge_core::ContentHash;
use surge_core::artifact_contract::RelPath;

use crate::runs::error::StorageError;

/// One file's trust state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustEntry {
    /// Hash the operator accepted, in `sha256:<hex>` form.
    pub hash: String,
    /// Unix epoch milliseconds the pin was recorded.
    pub pinned_at_ms: i64,
}

/// A file that is not trusted at its current content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedFile {
    /// Project-relative path.
    pub path: RelPath,
    /// Current content hash.
    pub current: ContentHash,
    /// Pinned hash, when a pin exists but differs.
    pub pinned: Option<ContentHash>,
}

/// The trust store for one repository.
#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    entries: BTreeMap<String, TrustEntry>,
    path: Option<PathBuf>,
}

impl TrustStore {
    /// Open the trust file for `repo_id` under `surge_home`, creating the
    /// parent directory but not the file.
    ///
    /// # Errors
    /// [`StorageError`] when the directory cannot be created or the file
    /// exists but does not parse.
    pub fn open(surge_home: &Path, repo_id: &str) -> Result<Self, StorageError> {
        let dir = surge_home.join("trust");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{repo_id}.toml"));
        let entries = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            toml::from_str(&text).map_err(|e| {
                StorageError::MigrationFailed(format!(
                    "trust file {} does not parse: {e}",
                    path.display()
                ))
            })?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            entries,
            path: Some(path),
        })
    }

    /// An in-memory store with no file (tests).
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Check one project file's content against the pin.
    ///
    /// `content` is the bytes about to be loaded; `rel_path` is the
    /// project-relative path (already validated by [`RelPath`]).
    ///
    /// Returns `Ok(None)` when the file is trusted at this content, and
    /// `Ok(Some(untrusted))` when it is new or changed. A caller that gets
    /// `Some` must not proceed to load the file.
    ///
    /// # Errors
    /// [`StorageError`] when the current hash cannot be computed (never for
    /// a byte slice today, but the signature leaves room for a reader).
    pub fn check(
        &self,
        rel_path: &RelPath,
        content: &[u8],
    ) -> Result<Option<UntrustedFile>, StorageError> {
        let current = ContentHash::compute(content);
        match self.entries.get(rel_path.as_str()) {
            Some(entry) => {
                let pinned: ContentHash = entry.hash.parse().map_err(|e| {
                    StorageError::MigrationFailed(format!(
                        "trust entry {:?} has an unparseable hash: {e}",
                        rel_path.as_str()
                    ))
                })?;
                if pinned == current {
                    Ok(None)
                } else {
                    Ok(Some(UntrustedFile {
                        path: rel_path.clone(),
                        current,
                        pinned: Some(pinned),
                    }))
                }
            },
            None => Ok(Some(UntrustedFile {
                path: rel_path.clone(),
                current,
                pinned: None,
            })),
        }
    }

    /// Pin `rel_path` at `hash`, replacing any previous pin, and persist.
    ///
    /// # Errors
    /// [`StorageError`] when the file cannot be written.
    pub fn accept(
        &mut self,
        rel_path: &RelPath,
        hash: ContentHash,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        self.entries.insert(
            rel_path.as_str().to_string(),
            TrustEntry {
                hash: hash.to_string(),
                pinned_at_ms: now_ms,
            },
        );
        self.persist()
    }

    /// Pin the content of `rel_path` as read from `project_root`, and
    /// return the hash pinned.
    ///
    /// This is the `surge trust accept <path>` path and the gate's
    /// post-install pin: one call reads, hashes and records.
    ///
    /// # Errors
    /// [`StorageError`] when the file cannot be read or the store written.
    pub fn accept_file(
        &mut self,
        project_root: &Path,
        rel_path: &RelPath,
        now_ms: i64,
    ) -> Result<ContentHash, StorageError> {
        let absolute = rel_path.join_onto(project_root);
        let content = std::fs::read(&absolute).map_err(|e| {
            StorageError::MigrationFailed(format!("read {}: {e}", absolute.display()))
        })?;
        let hash = ContentHash::compute(&content);
        self.accept(rel_path, hash, now_ms)?;
        Ok(hash)
    }

    /// Every pin, ordered by path.
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<String, TrustEntry> {
        &self.entries
    }

    /// Persist the store to its file (no-op for [`TrustStore::in_memory`]).
    fn persist(&self) -> Result<(), StorageError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string_pretty(&self.entries)
            .map_err(|e| StorageError::MigrationFailed(format!("serialize trust store: {e}")))?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// Whether a project-relative `.surge/` path is executable composition the
/// trust gate covers: profiles, flows, skills and the MCP list.
///
/// `roadmap.toml`, `memory/` and other planning data are read as data, not
/// bound into an agent prompt, so gating them would prompt on every roadmap
/// edit without changing what code a run executes. The daemon's dispatch
/// gate and `surge trust list` both call this — one definition, so the two
/// cannot disagree about what "untrusted" means.
#[must_use]
pub fn is_trust_gated_path(rel: &str) -> bool {
    rel.starts_with(".surge/profiles/")
        || rel.starts_with(".surge/flows/")
        || rel.starts_with(".surge/skills/")
        || rel == ".surge/mcp.toml"
}

/// Derive the stable repository id used as the trust file name.
///
/// A canonical remote URL when the repository has one, else the
/// canonicalized root path. The id is the hex digest of the selected string
/// so no path or URL component can escape the `trust/` directory.
#[must_use]
pub fn repo_id(remote_url: Option<&str>, root: &Path) -> String {
    let basis = match remote_url {
        Some(url) if !url.trim().is_empty() => url.trim().to_string(),
        _ => root
            .canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .to_string(),
    };
    ContentHash::compute(basis.as_bytes()).to_hex()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(path: &str) -> RelPath {
        RelPath::new(path).unwrap()
    }

    #[test]
    fn unpinned_file_is_untrusted() {
        let store = TrustStore::in_memory();
        let result = store
            .check(&rel(".surge/profiles/x-1.0.toml"), b"body")
            .unwrap();
        let untrusted = result.expect("unpinned file must be reported");
        assert_eq!(untrusted.pinned, None);
    }

    #[test]
    fn accept_then_check_passes() {
        let mut store = TrustStore::in_memory();
        let path = rel(".surge/flows/bug-fix-1.0.toml");
        store
            .accept(&path, ContentHash::compute(b"body"), 1)
            .unwrap();
        assert!(store.check(&path, b"body").unwrap().is_none());
    }

    #[test]
    fn changed_content_is_untrusted_with_both_hashes() {
        let mut store = TrustStore::in_memory();
        let path = rel(".surge/flows/bug-fix-1.0.toml");
        store
            .accept(&path, ContentHash::compute(b"original"), 1)
            .unwrap();
        let untrusted = store
            .check(&path, b"tampered")
            .unwrap()
            .expect("changed file must be reported");
        assert_eq!(untrusted.current, ContentHash::compute(b"tampered"));
        assert_eq!(untrusted.pinned, Some(ContentHash::compute(b"original")));
    }

    #[test]
    fn repo_id_prefers_the_remote_and_is_stable() {
        let a = repo_id(Some("https://github.com/o/r.git"), Path::new("/one/clone"));
        let b = repo_id(
            Some("https://github.com/o/r.git"),
            Path::new("/another/clone"),
        );
        assert_eq!(a, b, "same remote, same id regardless of checkout path");

        let local_a = repo_id(None, Path::new("/tmp"));
        let local_b = repo_id(None, Path::new("/tmp"));
        assert_eq!(local_a, local_b);
        assert_ne!(a, local_a);
    }

    #[test]
    fn store_round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = rel(".surge/profiles/x-1.0.toml");
        {
            let mut store = TrustStore::open(dir.path(), "abc").unwrap();
            store
                .accept(&path, ContentHash::compute(b"body"), 42)
                .unwrap();
        }
        let store = TrustStore::open(dir.path(), "abc").unwrap();
        assert!(store.check(&path, b"body").unwrap().is_none());
        assert_eq!(store.entries().len(), 1);
    }

    #[test]
    fn a_different_repo_does_not_see_anothers_pins() {
        let dir = tempfile::tempdir().unwrap();
        let path = rel(".surge/profiles/x-1.0.toml");
        {
            let mut store = TrustStore::open(dir.path(), "repo-a").unwrap();
            store
                .accept(&path, ContentHash::compute(b"body"), 1)
                .unwrap();
        }
        let other = TrustStore::open(dir.path(), "repo-b").unwrap();
        assert!(other.check(&path, b"body").unwrap().is_some());
    }

    #[test]
    fn accept_file_reads_hashes_and_pins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".surge/profiles")).unwrap();
        std::fs::write(dir.path().join(".surge/profiles/x-1.0.toml"), "body").unwrap();
        let mut store = TrustStore::open(dir.path(), "abc").unwrap();
        let path = rel(".surge/profiles/x-1.0.toml");
        let hash = store.accept_file(dir.path(), &path, 7).unwrap();
        assert_eq!(hash, ContentHash::compute(b"body"));
        assert!(store.check(&path, b"body").unwrap().is_none());
    }
}
