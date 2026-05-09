//! Content-addressed artifact storage for run outputs.
//!
//! Artifacts are stored under `<runs_root>/<run_id>/artifacts/<sha256-hex>`.
//! A lightweight `index.json` maps the user-facing artifact name to the latest
//! content hash produced for that name within the run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use surge_core::content_hash::ContentHash;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::run_state::ArtifactRef;

const INDEX_FILE: &str = "index.json";
const SYNTHETIC_PRODUCER: &str = "artifact_store";

/// Result type for artifact-store operations.
pub type Result<T> = std::result::Result<T, ArtifactStoreError>;

/// Errors returned by [`ArtifactStore`].
#[derive(Debug, thiserror::Error)]
pub enum ArtifactStoreError {
    /// The user's home directory could not be resolved.
    #[error("cannot determine home directory")]
    HomeMissing,
    /// Filesystem operation failed.
    #[error("artifact store I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// `index.json` serialization or parsing failed.
    #[error("artifact index JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// The fallback file did not match the requested content hash.
    #[error("artifact fallback hash mismatch: expected {expected}, got {actual}")]
    HashMismatch {
        /// Expected content hash from the artifact event.
        expected: ContentHash,
        /// Actual hash computed from the fallback file.
        actual: ContentHash,
    },
    /// The internal synthetic producer key was invalid.
    #[error("invalid synthetic artifact producer: {0}")]
    InvalidSyntheticProducer(String),
}

/// Content-addressed store rooted at the global runs directory.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    runs_root: PathBuf,
}

impl ArtifactStore {
    /// Create a store rooted at `<runs_root>`.
    #[must_use]
    pub fn new(runs_root: impl Into<PathBuf>) -> Self {
        Self {
            runs_root: runs_root.into(),
        }
    }

    /// Resolve the default artifact root, mirroring `~/.surge/runs/`.
    ///
    /// # Errors
    /// Returns [`ArtifactStoreError::HomeMissing`] when the home directory
    /// cannot be determined.
    pub fn default_path() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or(ArtifactStoreError::HomeMissing)?;
        Ok(home.join(".surge").join("runs"))
    }

    /// Create a store rooted at [`Self::default_path`].
    ///
    /// # Errors
    /// Returns [`ArtifactStoreError::HomeMissing`] when the home directory
    /// cannot be determined.
    pub fn from_default_path() -> Result<Self> {
        Ok(Self::new(Self::default_path()?))
    }

    /// Store `content` for `run_id` under `name`.
    ///
    /// Returns an [`ArtifactRef`] whose path points at the content-addressed
    /// canonical copy. `produced_by` and `produced_at_seq` are placeholders;
    /// event folding replaces them from the `ArtifactProduced` event itself.
    ///
    /// # Errors
    /// Returns an error when filesystem writes, index serialization, or the
    /// internal producer key conversion fail.
    pub async fn put(&self, run_id: RunId, name: &str, content: &[u8]) -> Result<ArtifactRef> {
        let hash = ContentHash::compute(content);
        let artifacts_dir = self.artifacts_dir(run_id);
        tokio::fs::create_dir_all(&artifacts_dir).await?;

        let target = artifacts_dir.join(hash.to_hex());
        if tokio::fs::metadata(&target).await.is_err() {
            let tmp = artifacts_dir.join(format!("{}.tmp-{}", hash.to_hex(), ulid::Ulid::new()));
            tokio::fs::write(&tmp, content).await?;
            match tokio::fs::rename(&tmp, &target).await {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let _ = tokio::fs::remove_file(&tmp).await;
                },
                Err(e) => return Err(ArtifactStoreError::Io(e)),
            }
        }

        let mut index = self.read_index(run_id).await?;
        index.insert(name.to_owned(), hash);
        self.write_index(run_id, &index).await?;

        let produced_by = NodeKey::try_from(SYNTHETIC_PRODUCER)
            .map_err(|e| ArtifactStoreError::InvalidSyntheticProducer(e.to_string()))?;

        Ok(ArtifactRef {
            hash,
            path: target,
            name: name.to_owned(),
            produced_by,
            produced_at_seq: 0,
        })
    }

    /// Read an artifact by content hash.
    ///
    /// # Errors
    /// Returns an error when the artifact file is absent or unreadable.
    pub async fn open(&self, run_id: RunId, hash: ContentHash) -> Result<Vec<u8>> {
        Ok(tokio::fs::read(self.artifacts_dir(run_id).join(hash.to_hex())).await?)
    }

    /// Read an artifact, falling back to the event's original path.
    ///
    /// This keeps runs created before the content-addressed store readable:
    /// their event log may point at a worktree-relative file while no
    /// `index.json` or content-addressed copy exists yet.
    ///
    /// # Errors
    /// Returns an error when neither the store nor the fallback path can be
    /// read, or when fallback bytes do not match the recorded hash.
    pub async fn open_ref(
        &self,
        run_id: RunId,
        artifact: &ArtifactRef,
        fallback_root: impl AsRef<Path>,
    ) -> Result<Vec<u8>> {
        match self.open(run_id, artifact.hash).await {
            Ok(bytes) => return Ok(bytes),
            Err(ArtifactStoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e),
        }

        let fallback_path = if artifact.path.is_absolute() {
            artifact.path.clone()
        } else {
            fallback_root.as_ref().join(&artifact.path)
        };
        let bytes = tokio::fs::read(fallback_path).await?;
        let actual = ContentHash::compute(&bytes);
        if actual != artifact.hash {
            return Err(ArtifactStoreError::HashMismatch {
                expected: artifact.hash,
                actual,
            });
        }
        Ok(bytes)
    }

    fn artifacts_dir(&self, run_id: RunId) -> PathBuf {
        self.runs_root.join(run_id.to_string()).join("artifacts")
    }

    fn index_path(&self, run_id: RunId) -> PathBuf {
        self.artifacts_dir(run_id).join(INDEX_FILE)
    }

    async fn read_index(&self, run_id: RunId) -> Result<BTreeMap<String, ContentHash>> {
        let path = self.index_path(run_id);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(ArtifactStoreError::Io(e)),
        }
    }

    async fn write_index(
        &self,
        run_id: RunId,
        index: &BTreeMap<String, ContentHash>,
    ) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(index)?;
        tokio::fs::write(self.index_path(run_id), bytes).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(root: &Path) -> ArtifactStore {
        ArtifactStore::new(root.join("runs"))
    }

    #[tokio::test]
    async fn put_then_open_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = test_store(tmp.path());
        let run_id = RunId::new();

        let artifact = store
            .put(run_id, "description", b"bootstrap description")
            .await
            .unwrap();
        let bytes = store.open(run_id, artifact.hash).await.unwrap();

        assert_eq!(bytes, b"bootstrap description");
        assert_eq!(artifact.name, "description");
        assert_eq!(
            artifact.path.file_name().unwrap(),
            artifact.hash.to_hex().as_str()
        );
    }

    #[tokio::test]
    async fn identical_content_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let store = test_store(tmp.path());
        let run_id = RunId::new();

        let first = store.put(run_id, "flow", b"same content").await.unwrap();
        let second = store.put(run_id, "flow", b"same content").await.unwrap();

        assert_eq!(first.hash, second.hash);
        assert_eq!(first.path, second.path);
    }

    #[tokio::test]
    async fn open_ref_falls_back_to_legacy_relative_path_without_index() {
        let tmp = tempfile::tempdir().unwrap();
        let store = test_store(tmp.path());
        let run_id = RunId::new();
        let content = b"legacy artifact";
        tokio::fs::write(tmp.path().join("description.md"), content)
            .await
            .unwrap();

        let artifact = ArtifactRef {
            hash: ContentHash::compute(content),
            path: PathBuf::from("description.md"),
            name: "description".into(),
            produced_by: NodeKey::try_from("description_author").unwrap(),
            produced_at_seq: 4,
        };

        let bytes = store.open_ref(run_id, &artifact, tmp.path()).await.unwrap();

        assert_eq!(bytes, content);
        assert!(!store.index_path(run_id).exists());
    }
}
