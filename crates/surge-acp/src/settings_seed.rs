//! Generic agent settings bootstrap.
//!
//! An ACP agent may need files present in its working directory before it
//! can start — most commonly a settings file that pins a non-interactive
//! mode the agent's own global configuration does not accept. Surge does
//! not know which file an agent needs: the agent's registry entry declares
//! it as data ([`surge_core::config::AgentSettingsFile`]), and this module
//! materialises those files at session-open time.
//!
//! This is the whole mechanism. There is no per-vendor code path: the
//! builtin `claude-acp` entry declares `.claude/settings.json`, a user's
//! custom provider declares whatever its own CLI reads, and both take the
//! same route through [`seed_settings_files`].

use std::path::Path;

use surge_core::config::AgentSettingsFile;

/// Materialise each settings file under `worktree` when absent.
///
/// Contract, per file:
/// - `file.path` is relative to the worktree; absolute paths and `..`
///   segments are refused (a registry entry is data, and data must not
///   redirect a write outside the run's workspace).
/// - An existing file is never clobbered — the operator's explicit project
///   choice wins, and `create_new` closes the TOCTOU window an
///   `exists()`-then-write pair would leave open.
/// - Parent directories are created.
/// - A symlinked parent of the target is refused, so a crafted checkout
///   cannot redirect the write outside the worktree.
///
/// Best-effort: failures are logged, never fatal. A missing settings file
/// degrades the agent to its global configuration; it does not fail a run
/// on its own.
pub fn seed_settings_files(files: &[AgentSettingsFile], worktree: &Path) {
    for file in files {
        match seed_one(file, worktree) {
            Ok(Seeded::Written) => tracing::debug!(
                target: "surge_acp.settings",
                path = %file.path,
                worktree = %worktree.display(),
                "seeded agent settings file"
            ),
            Ok(Seeded::Present) => {},
            Err(reason) => tracing::warn!(
                target: "surge_acp.settings",
                path = %file.path,
                worktree = %worktree.display(),
                reason = %reason,
                "skipping agent settings seed"
            ),
        }
    }
}

enum Seeded {
    Written,
    Present,
}

fn seed_one(file: &AgentSettingsFile, worktree: &Path) -> Result<Seeded, String> {
    use std::io::Write as _;

    let relative = safe_relative_path(&file.path)?;
    let target = worktree.join(&relative);

    // Refuse to write through a symlinked parent directory anywhere along
    // the relative path. `symlink_metadata` does not traverse the final
    // component, so each existing ancestor is inspected as itself.
    let mut cursor = worktree.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        cursor.push(component.as_os_str());
        match std::fs::symlink_metadata(&cursor) {
            Ok(md) if md.file_type().is_symlink() => {
                return Err(format!(
                    "refusing to write through symlinked directory {}",
                    cursor.display()
                ));
            },
            Ok(md) if !md.is_dir() => {
                return Err(format!(
                    "{} exists and is not a directory",
                    cursor.display()
                ));
            },
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("stat {}: {e}", cursor.display())),
        }
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let mut handle = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
    {
        Ok(handle) => handle,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(Seeded::Present),
        Err(e) => return Err(format!("create {}: {e}", target.display())),
    };
    handle
        .write_all(file.content.as_bytes())
        .map_err(|e| format!("write {}: {e}", target.display()))?;
    Ok(Seeded::Written)
}

/// Accept only a relative path with no `..` or root components.
fn safe_relative_path(raw: &str) -> Result<std::path::PathBuf, String> {
    use std::path::Component;

    if raw.trim().is_empty() {
        return Err("empty path".to_string());
    }
    let path = Path::new(raw);
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {},
            Component::ParentDir => {
                return Err(format!("path {raw:?} contains '..'"));
            },
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("path {raw:?} must be relative to the worktree"));
            },
        }
    }
    if out.as_os_str().is_empty() {
        return Err(format!("path {raw:?} resolves to nothing"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, content: &str) -> AgentSettingsFile {
        AgentSettingsFile::new(path, content)
    }

    #[test]
    fn writes_a_declared_file() {
        let tmp = tempfile::tempdir().unwrap();
        seed_settings_files(&[file(".claude/settings.json", "{\"a\":1}\n")], tmp.path());
        let written = std::fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap();
        assert_eq!(written, "{\"a\":1}\n");
    }

    #[test]
    fn never_clobbers_an_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::fs::write(tmp.path().join(".claude/settings.json"), "operator").unwrap();
        seed_settings_files(&[file(".claude/settings.json", "surge")], tmp.path());
        let after = std::fs::read_to_string(tmp.path().join(".claude/settings.json")).unwrap();
        assert_eq!(after, "operator", "existing file wins");
    }

    #[test]
    fn refuses_absolute_and_parent_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        seed_settings_files(
            &[file("/etc/surge-test", "x"), file("../escape.json", "x")],
            tmp.path(),
        );
        assert!(!tmp.path().parent().unwrap().join("escape.json").exists());
        assert!(!Path::new("/etc/surge-test").exists());
        // The legitimate sibling case: nothing was written anywhere.
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
        drop(outside);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_write_through_a_symlinked_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join(".claude")).unwrap();
        seed_settings_files(&[file(".claude/settings.json", "x")], tmp.path());
        assert!(
            !outside.path().join("settings.json").exists(),
            "must not follow a symlinked parent outside the worktree",
        );
    }

    #[test]
    fn empty_declaration_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        seed_settings_files(&[], tmp.path());
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}
