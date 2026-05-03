//! Worktree-rooted path validation. Used by both `SurgeClient` and `BridgeClient`
//! before any file IO so an agent cannot escape the worktree via `..` or symlinks.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum PathGuardError {
    #[error("path '{path}' is not absolute (worktree paths must be absolute)")]
    NotAbsolute { path: PathBuf },
    #[error("path '{path}' escapes worktree root '{worktree}'")]
    Escapes { path: PathBuf, worktree: PathBuf },
    #[error("io error while canonicalizing '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Verify that `path` resolves under `worktree_root`, after canonicalization.
///
/// `worktree_root` is expected to already be canonicalized by the caller (see
/// `SurgeClient::new` and `bridge::session::open_session_impl` for the precedent).
/// This function canonicalizes `path` here so that symlinks-to-outside are rejected
/// even if the agent constructed the path via legitimate-looking components.
pub(crate) fn ensure_in_worktree(
    worktree_root: &Path,
    path: &Path,
) -> Result<PathBuf, PathGuardError> {
    if !path.is_absolute() {
        return Err(PathGuardError::NotAbsolute {
            path: path.to_path_buf(),
        });
    }
    let canonical = path.canonicalize().map_err(|source| PathGuardError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !canonical.starts_with(worktree_root) {
        // Report the canonical (post-symlink) path so the operator sees where
        // the request actually resolved to, not just the lexical input.
        return Err(PathGuardError::Escapes {
            path: canonical,
            worktree: worktree_root.to_path_buf(),
        });
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_path() {
        let wt = std::env::temp_dir();
        let p = Path::new("foo.txt");
        let err = ensure_in_worktree(&wt, p).unwrap_err();
        assert!(matches!(err, PathGuardError::NotAbsolute { .. }));
    }

    #[test]
    fn accepts_path_inside_worktree() {
        let wt = tempfile::tempdir().unwrap();
        let inner = wt.path().join("a.txt");
        std::fs::write(&inner, "x").unwrap();
        let canonical_root = wt.path().canonicalize().unwrap();
        let resolved = ensure_in_worktree(&canonical_root, &inner).unwrap();
        assert!(resolved.starts_with(&canonical_root));
    }

    #[test]
    fn rejects_path_escaping_worktree_via_dotdot() {
        let outer = tempfile::tempdir().unwrap();
        let inside = outer.path().join("a");
        std::fs::create_dir(&inside).unwrap();
        let outside = outer.path().join("b.txt");
        std::fs::write(&outside, "y").unwrap();
        let canonical_inside = inside.canonicalize().unwrap();
        // Construct an absolute path that lexically lives "inside" but resolves outside.
        // Use canonical_inside as base so the path is absolute on all platforms.
        let escape = canonical_inside.join("..").join("b.txt");
        let err = ensure_in_worktree(&canonical_inside, &escape).unwrap_err();
        assert!(matches!(err, PathGuardError::Escapes { .. }));
    }
}
