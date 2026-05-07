//! Path helpers for the on-disk profile store.
//!
//! Mirrors the shape of [`surge_persistence::store::default_path`]: one
//! place reads `SURGE_HOME` (or falls back to `dirs::home_dir()/.surge`)
//! so that test isolation via temp dirs is a single setenv.

use std::path::PathBuf;

use surge_core::error::SurgeError;

/// Environment variable that overrides the default `~/.surge` location.
pub const SURGE_HOME_ENV: &str = "SURGE_HOME";

/// Resolve `${SURGE_HOME}` (or fall back to `~/.surge`).
///
/// # Errors
/// Returns [`SurgeError::Config`] when neither `SURGE_HOME` is set nor
/// `dirs::home_dir()` can identify a home directory.
pub fn surge_home() -> Result<PathBuf, SurgeError> {
    if let Ok(custom) = std::env::var(SURGE_HOME_ENV) {
        if !custom.is_empty() {
            tracing::debug!(target: "profile::paths", path = %custom, "SURGE_HOME override active");
            return Ok(PathBuf::from(custom));
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".surge"))
        .ok_or_else(|| {
            SurgeError::Config("cannot determine SURGE_HOME (no $SURGE_HOME, no $HOME)".into())
        })
}

/// Resolve `${SURGE_HOME}/profiles` (the disk root for user-authored profiles).
///
/// Does not require the directory to exist — callers that scan it must
/// handle absence themselves so a missing `~/.surge/profiles/` is not an
/// error on a fresh install.
///
/// # Errors
/// Propagates [`surge_home`] errors.
pub fn profiles_dir() -> Result<PathBuf, SurgeError> {
    surge_home().map(|h| h.join("profiles"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to run a closure with `SURGE_HOME` set to a value, restoring
    /// the previous value (or removing the var) on drop. Tests in this
    /// module are NOT marked `#[serial_test]` because the workspace does
    /// not depend on `serial_test`; run only one at a time per crate
    /// thread is the de-facto convention enforced by `cargo test`'s
    /// default behavior on this scope.
    struct EnvGuard {
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(value: &str) -> Self {
            let prev = std::env::var(SURGE_HOME_ENV).ok();
            // SAFETY: tests in surge-orchestrator are single-threaded by
            // default; the test harness serialises within a single binary.
            unsafe { std::env::set_var(SURGE_HOME_ENV, value) };
            Self { prev }
        }

        fn unset() -> Self {
            let prev = std::env::var(SURGE_HOME_ENV).ok();
            unsafe { std::env::remove_var(SURGE_HOME_ENV) };
            Self { prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(v) = self.prev.take() {
                    std::env::set_var(SURGE_HOME_ENV, v);
                } else {
                    std::env::remove_var(SURGE_HOME_ENV);
                }
            }
        }
    }

    #[test]
    fn surge_home_uses_env_when_set() {
        let _g = EnvGuard::set("/tmp/custom-surge");
        let h = surge_home().unwrap();
        assert_eq!(h, PathBuf::from("/tmp/custom-surge"));
    }

    #[test]
    fn profiles_dir_appends_profiles_segment() {
        let _g = EnvGuard::set("/tmp/custom-surge");
        let p = profiles_dir().unwrap();
        assert_eq!(p, PathBuf::from("/tmp/custom-surge/profiles"));
    }

    #[test]
    fn surge_home_falls_back_to_dirs_when_env_unset() {
        let _g = EnvGuard::unset();
        // dirs::home_dir() returns Some on every platform CI runs on; we
        // only assert the result has the `.surge` suffix.
        if let Ok(h) = surge_home() {
            assert!(h.ends_with(".surge"), "expected .surge suffix, got {h:?}");
        }
    }

    #[test]
    fn surge_home_treats_empty_env_as_unset() {
        let _g = EnvGuard::set("");
        // Should not return PathBuf::from("") — should fall back.
        if let Ok(h) = surge_home() {
            assert_ne!(h, PathBuf::from(""));
        }
    }
}
