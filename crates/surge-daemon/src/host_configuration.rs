//! Startup-directory configuration discovery, distinct from project admission.

use std::path::{Path, PathBuf};
use surge_core::SurgeConfig;

/// Absence permits validated infrastructure defaults; uncertainty never does.
pub(crate) enum HostConfiguration {
    Missing(Box<SurgeConfig>),
    Loaded {
        path: PathBuf,
        config: Box<SurgeConfig>,
    },
    Invalid {
        path: Option<PathBuf>,
        reason: ConfigurationFailure,
    },
}

/// Diagnostic categories deliberately exclude configuration contents and secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigurationFailure {
    WorkingDirectory,
    Metadata,
    EntryType,
    ReadOrValidation,
    Defaults,
}

pub(crate) fn load() -> HostConfiguration {
    from_directory(std::env::current_dir(), SurgeConfig::load)
}

fn invalid(path: Option<PathBuf>, reason: ConfigurationFailure) -> HostConfiguration {
    HostConfiguration::Invalid { path, reason }
}

fn from_directory(
    cwd: Result<PathBuf, std::io::Error>,
    read: impl FnOnce(&Path) -> Result<SurgeConfig, surge_core::SurgeError>,
) -> HostConfiguration {
    let cwd = match cwd {
        Ok(cwd) if cwd.is_absolute() => cwd,
        _ => return invalid(None, ConfigurationFailure::WorkingDirectory),
    };
    for directory in cwd.ancestors() {
        let path = directory.join("surge.toml");
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                let target = if metadata.file_type().is_symlink() {
                    match std::fs::metadata(&path) {
                        Ok(target) => target,
                        Err(_) => return invalid(Some(path), ConfigurationFailure::Metadata),
                    }
                } else {
                    metadata
                };
                if !target.is_file() {
                    return invalid(Some(path), ConfigurationFailure::EntryType);
                }
                // Once an entry is observed, disappearance, unreadability and a
                // dangling symlink are invalid; never search a more distant parent.
                return match read(&path) {
                    Ok(config) => HostConfiguration::Loaded {
                        path,
                        config: Box::new(config),
                    },
                    Err(_) => invalid(Some(path), ConfigurationFailure::ReadOrValidation),
                };
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // ENOENT also covers a removed parent. It is not confirmed
                // candidate absence unless that directory itself remains valid.
                if !std::fs::metadata(directory).is_ok_and(|metadata| metadata.is_dir()) {
                    return invalid(Some(path), ConfigurationFailure::Metadata);
                }
            },
            Err(_) => return invalid(Some(path), ConfigurationFailure::Metadata),
        }
    }
    let config = SurgeConfig::default();
    match config.validate() {
        Ok(()) => HostConfiguration::Missing(Box::new(config)),
        Err(_) => invalid(None, ConfigurationFailure::Defaults),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_current_directory_is_invalid() {
        assert!(matches!(
            from_directory(Err(std::io::Error::other("unavailable")), SurgeConfig::load),
            HostConfiguration::Invalid {
                path: None,
                reason: ConfigurationFailure::WorkingDirectory
            }
        ));
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            from_directory(Ok(root.path().join("removed")), SurgeConfig::load),
            HostConfiguration::Invalid {
                reason: ConfigurationFailure::Metadata,
                ..
            }
        ));
    }

    #[test]
    fn confirmed_absence_returns_validated_defaults() {
        let root = tempfile::tempdir().unwrap();
        match from_directory(Ok(root.path().canonicalize().unwrap()), SurgeConfig::load) {
            HostConfiguration::Missing(config) => config.validate().unwrap(),
            _ => panic!("fixture ancestry must have no host configuration"),
        }
    }

    #[test]
    fn nearest_file_wins_and_unsupported_schema_is_invalid() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("surge.toml"), "schema_version = 1\n").unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        assert!(matches!(
            from_directory(Ok(child.clone()), SurgeConfig::load),
            HostConfiguration::Loaded { path, .. } if path == root.path().join("surge.toml")
        ));
        let path = child.join("surge.toml");
        std::fs::write(&path, "schema_version = 999\n").unwrap();
        assert!(matches!(
            from_directory(Ok(child.clone()), SurgeConfig::load),
            HostConfiguration::Invalid {
                reason: ConfigurationFailure::ReadOrValidation,
                ..
            }
        ));
        std::fs::write(&path, "schema_version = 1\n").unwrap();
        assert!(matches!(
            from_directory(Ok(child), SurgeConfig::load),
            HostConfiguration::Loaded { path: observed, .. } if observed == path
        ));
    }

    #[test]
    fn closest_directory_entry_never_falls_back_to_ancestor() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("surge.toml"), "schema_version = 1\n").unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let candidate = child.join("surge.toml");
        std::fs::create_dir(&candidate).unwrap();
        assert!(matches!(
            from_directory(Ok(child), SurgeConfig::load),
            HostConfiguration::Invalid { path: Some(path), reason: ConfigurationFailure::EntryType } if path == candidate
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_to_special_files_are_rejected_before_reading() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("surge.toml"), "schema_version = 1\n").unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let fifo = root.path().join("config-pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let candidate = child.join("surge.toml");
        for target in [fifo.as_path(), Path::new("/dev/zero")] {
            std::os::unix::fs::symlink(target, &candidate).unwrap();
            let result = from_directory(Ok(child.clone()), |_| {
                panic!("special-file symlink must be rejected before invoking the config reader")
            });
            assert!(matches!(
                result,
                HostConfiguration::Invalid {
                    reason: ConfigurationFailure::EntryType,
                    ..
                }
            ));
            std::fs::remove_file(&candidate).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_regular_configuration_preserves_supported_loading() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("regular.toml");
        std::fs::write(&target, "schema_version = 1\n").unwrap();
        let candidate = root.path().join("surge.toml");
        std::os::unix::fs::symlink(target, &candidate).unwrap();
        assert!(matches!(
            from_directory(Ok(root.path().canonicalize().unwrap()), SurgeConfig::load),
            HostConfiguration::Loaded { .. }
        ));
    }

    #[test]
    fn removal_after_discovery_is_invalid_without_ancestor_fallback() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("surge.toml");
        std::fs::write(&path, "schema_version = 1\n").unwrap();
        let result = from_directory(Ok(root.path().canonicalize().unwrap()), |candidate| {
            std::fs::remove_file(candidate).unwrap();
            SurgeConfig::load(candidate)
        });
        assert!(matches!(
            result,
            HostConfiguration::Invalid {
                reason: ConfigurationFailure::ReadOrValidation,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_and_unsearchable_directory_are_invalid() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let path = cwd.join("surge.toml");
        std::fs::write(&path, "schema_version = 1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
        let unreadable = from_directory(Ok(cwd.clone()), SurgeConfig::load);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            unreadable,
            HostConfiguration::Invalid {
                reason: ConfigurationFailure::ReadOrValidation,
                ..
            }
        ));
        std::fs::set_permissions(&cwd, std::fs::Permissions::from_mode(0)).unwrap();
        let inaccessible = from_directory(Ok(cwd.clone()), SurgeConfig::load);
        std::fs::set_permissions(cwd, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(
            inaccessible,
            HostConfiguration::Invalid {
                reason: ConfigurationFailure::Metadata,
                ..
            }
        ));
    }
}
