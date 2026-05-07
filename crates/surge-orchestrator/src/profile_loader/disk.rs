//! Disk-resident profile store.
//!
//! `DiskProfileSet::scan(dir)` walks `*.toml` files flat under `dir` and
//! parses each into a [`Profile`]. Per-file parse failures are logged at
//! WARN and the entry is skipped — one bad file does not poison the
//! whole registry.

use std::path::{Path, PathBuf};

use surge_core::error::SurgeError;
use surge_core::profile::Profile;

/// Profiles successfully loaded from a single directory.
#[derive(Debug, Clone, Default)]
pub struct DiskProfileSet {
    profiles: Vec<DiskEntry>,
}

/// A profile alongside the path it was loaded from. The path is kept
/// verbatim so error messages can point at the offending file.
#[derive(Debug, Clone)]
pub struct DiskEntry {
    pub path: PathBuf,
    pub profile: Profile,
}

impl DiskProfileSet {
    /// Empty set, used when the profiles directory does not yet exist on
    /// a fresh install.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Walk `dir` for `*.toml` files and parse each as a [`Profile`].
    ///
    /// Behavior:
    /// - The walk is **flat** (no recursion into subdirectories). Profiles
    ///   live one level deep so a future `presets/` subdir for examples
    ///   does not accidentally get loaded.
    /// - Per-file parse failures are logged at WARN and skipped, not
    ///   returned. If you need strict mode, validate each entry yourself.
    /// - Hidden files (`.foo.toml`) and non-`.toml` siblings are ignored.
    /// - On collisions (two files producing the same `(role.id, role.version)`)
    ///   the first file wins; the duplicate is logged at WARN.
    ///
    /// # Errors
    /// Returns [`SurgeError::Io`] only when the directory itself cannot be
    /// opened. Per-file errors are absorbed into WARN logs.
    pub fn scan(dir: &Path) -> Result<Self, SurgeError> {
        if !dir.exists() {
            tracing::debug!(target: "profile::disk", path = %dir.display(), "profiles dir does not exist; treating as empty");
            return Ok(Self::empty());
        }

        let mut profiles: Vec<DiskEntry> = Vec::new();
        // Linear-time dedup keyed by `(role.id, role.version)`. The
        // previous implementation re-scanned `profiles` for every new
        // file, which is O(n²) in directory size; HashSet keeps it O(n).
        let mut seen_keys: std::collections::HashSet<(String, semver::Version)> =
            std::collections::HashSet::new();
        let read_dir = std::fs::read_dir(dir)?;
        for entry in read_dir {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(target: "profile::disk", err = %e, "failed to read directory entry; skipping");
                    continue;
                },
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if file_name.starts_with('.') {
                continue;
            }
            if !file_name.ends_with(".toml") {
                continue;
            }
            match load_one(&path) {
                Ok(profile) => {
                    let key = (
                        profile.role.id.as_str().to_string(),
                        profile.role.version.clone(),
                    );
                    if !seen_keys.insert(key.clone()) {
                        tracing::warn!(
                            target: "profile::disk",
                            path = %path.display(),
                            id = %key.0,
                            version = %key.1,
                            "duplicate (id, version); first file on disk wins"
                        );
                        continue;
                    }
                    tracing::debug!(
                        target: "profile::disk",
                        path = %path.display(),
                        id = %profile.role.id,
                        version = %profile.role.version,
                        "loaded disk profile"
                    );
                    profiles.push(DiskEntry { path, profile });
                },
                Err(e) => {
                    tracing::warn!(
                        target: "profile::disk",
                        path = %path.display(),
                        err = %e,
                        "failed to parse profile file; skipping"
                    );
                },
            }
        }

        tracing::info!(
            target: "profile::disk",
            count = profiles.len(),
            dir = %dir.display(),
            "disk profile scan complete"
        );

        Ok(Self { profiles })
    }

    /// Borrow the loaded entries.
    #[must_use]
    pub fn entries(&self) -> &[DiskEntry] {
        &self.profiles
    }

    /// Find a disk profile by `(name, version)` exactly.
    #[must_use]
    pub fn by_name_version(&self, name: &str, version: &semver::Version) -> Option<&DiskEntry> {
        self.profiles
            .iter()
            .find(|e| e.profile.role.id.as_str() == name && &e.profile.role.version == version)
    }

    /// Find the highest-version disk profile with the given name.
    #[must_use]
    pub fn by_name_latest(&self, name: &str) -> Option<&DiskEntry> {
        self.profiles
            .iter()
            .filter(|e| e.profile.role.id.as_str() == name)
            .max_by(|a, b| a.profile.role.version.cmp(&b.profile.role.version))
    }
}

fn load_one(path: &Path) -> Result<Profile, SurgeError> {
    let raw = std::fs::read_to_string(path)?;
    toml::from_str::<Profile>(&raw).map_err(|e| SurgeError::ProfileParse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_profile(dir: &Path, name: &str, version: &str, contents: &str) -> PathBuf {
        let path = dir.join(format!("{name}-{version}.toml"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn minimal_profile_toml(id: &str, version: &str) -> String {
        format!(
            r#"
schema_version = 1

[role]
id = "{id}"
version = "{version}"
display_name = "{id}"
category = "agents"
description = "test"
when_to_use = "test"

[runtime]
recommended_model = "test-model"

[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"

[prompt]
system = "test prompt"
"#
        )
    }

    #[test]
    fn missing_dir_yields_empty_set() {
        let dir = std::path::PathBuf::from("/this/does/not/exist/surely");
        let set = DiskProfileSet::scan(&dir).unwrap();
        assert!(set.entries().is_empty());
    }

    #[test]
    fn empty_dir_yields_empty_set() {
        let tmp = TempDir::new().unwrap();
        let set = DiskProfileSet::scan(tmp.path()).unwrap();
        assert!(set.entries().is_empty());
    }

    #[test]
    fn loads_valid_toml_files() {
        let tmp = TempDir::new().unwrap();
        write_profile(
            tmp.path(),
            "alpha",
            "1.0",
            &minimal_profile_toml("alpha", "1.0.0"),
        );
        write_profile(
            tmp.path(),
            "beta",
            "2.0",
            &minimal_profile_toml("beta", "2.0.0"),
        );

        let set = DiskProfileSet::scan(tmp.path()).unwrap();
        assert_eq!(set.entries().len(), 2);

        let names: std::collections::HashSet<&str> = set
            .entries()
            .iter()
            .map(|e| e.profile.role.id.as_str())
            .collect();
        assert!(names.contains("alpha"));
        assert!(names.contains("beta"));
    }

    #[test]
    fn skips_non_toml_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("readme.md"), "not a profile").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "also not").unwrap();
        write_profile(
            tmp.path(),
            "real",
            "1.0",
            &minimal_profile_toml("real", "1.0.0"),
        );
        let set = DiskProfileSet::scan(tmp.path()).unwrap();
        assert_eq!(set.entries().len(), 1);
    }

    #[test]
    fn skips_hidden_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(".hidden-1.0.toml"),
            minimal_profile_toml("hidden", "1.0.0"),
        )
        .unwrap();
        write_profile(
            tmp.path(),
            "real",
            "1.0",
            &minimal_profile_toml("real", "1.0.0"),
        );
        let set = DiskProfileSet::scan(tmp.path()).unwrap();
        assert_eq!(set.entries().len(), 1);
    }

    #[test]
    fn warn_and_skip_on_parse_failure() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("broken-1.0.toml"),
            "this = is = invalid toml",
        )
        .unwrap();
        write_profile(
            tmp.path(),
            "good",
            "1.0",
            &minimal_profile_toml("good", "1.0.0"),
        );
        let set = DiskProfileSet::scan(tmp.path()).unwrap();
        // Broken file dropped; good file kept.
        assert_eq!(set.entries().len(), 1);
        assert_eq!(set.entries()[0].profile.role.id.as_str(), "good");
    }

    #[test]
    fn duplicate_id_version_first_wins() {
        let tmp = TempDir::new().unwrap();
        write_profile(
            tmp.path(),
            "dup",
            "1.0",
            &minimal_profile_toml("dup", "1.0.0"),
        );
        // Different filename, same (id, version) pair after parse.
        std::fs::write(
            tmp.path().join("dup-renamed.toml"),
            minimal_profile_toml("dup", "1.0.0"),
        )
        .unwrap();
        let set = DiskProfileSet::scan(tmp.path()).unwrap();
        assert_eq!(set.entries().len(), 1);
    }

    #[test]
    fn by_name_version_exact_match() {
        let tmp = TempDir::new().unwrap();
        write_profile(
            tmp.path(),
            "alpha",
            "1.0",
            &minimal_profile_toml("alpha", "1.0.0"),
        );
        write_profile(
            tmp.path(),
            "alpha",
            "2.0",
            &minimal_profile_toml("alpha", "2.0.0"),
        );
        let set = DiskProfileSet::scan(tmp.path()).unwrap();
        let v = semver::Version::new(1, 0, 0);
        let found = set.by_name_version("alpha", &v).unwrap();
        assert_eq!(found.profile.role.version, v);
    }

    #[test]
    fn by_name_latest_picks_highest_semver() {
        let tmp = TempDir::new().unwrap();
        write_profile(
            tmp.path(),
            "alpha",
            "1.0",
            &minimal_profile_toml("alpha", "1.0.0"),
        );
        write_profile(
            tmp.path(),
            "alpha",
            "2.0",
            &minimal_profile_toml("alpha", "2.5.0"),
        );
        write_profile(
            tmp.path(),
            "alpha",
            "1.1",
            &minimal_profile_toml("alpha", "1.1.0"),
        );
        let set = DiskProfileSet::scan(tmp.path()).unwrap();
        let latest = set.by_name_latest("alpha").unwrap();
        assert_eq!(latest.profile.role.version, semver::Version::new(2, 5, 0));
    }
}
