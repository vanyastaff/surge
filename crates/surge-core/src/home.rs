//! Resolution of the Surge home directory (`$SURGE_HOME`, or `~/.surge`).
//!
//! The CLI, the daemon, and the persistence layer each need an answer to
//! "where does this `SURGE_HOME`-isolated process keep its state", and each
//! answers it the same way: `$SURGE_HOME` when set and non-empty, else
//! `~/.surge`. Before this module existed the rule was hand-copied into a
//! private function per crate (`surge-cli`, `surge-daemon`,
//! `surge-persistence`) — each kept correct by hand rather than by the
//! compiler, and easy to forget for a new call site (`surge-ui`'s
//! `RecentProjects` did, for a while, ignoring `SURGE_HOME` isolation
//! entirely). [`surge_home_dir`] is the one place the rule is written down;
//! every crate maps its `None` into whatever error type it already returns
//! for "home directory unknown" rather than this crate taking on their
//! error types.

use std::path::PathBuf;

/// Environment variable that relocates the Surge home directory, overriding
/// the `~/.surge` default. An empty value is treated as unset.
pub const SURGE_HOME_ENV: &str = "SURGE_HOME";

/// Resolve the Surge home directory: `$SURGE_HOME` when set and non-empty,
/// else `~/.surge`.
///
/// Returns `None` only when `$SURGE_HOME` is unset (or empty) **and** the
/// OS/user home directory itself cannot be determined
/// ([`dirs::home_dir`] returns `None`) — callers map that into their own
/// typed "home directory unknown" error.
#[must_use]
pub fn surge_home_dir() -> Option<PathBuf> {
    match std::env::var(SURGE_HOME_ENV) {
        Ok(custom) if !custom.is_empty() => Some(PathBuf::from(custom)),
        _ => dirs::home_dir().map(|home| home.join(".surge")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Process-wide lock serialising this module's tests, each of which
    /// mutates `SURGE_HOME` for its duration. `cargo test` runs a crate's
    /// unit tests as libtest threads inside one process, so without this
    /// lock two tests could observe each other's partial env state; this
    /// crate links no vendored C (no `git2`, no bundled `rusqlite`) into
    /// its own test binary, so the workspace's documented getenv-vs-C
    /// hazard (env mutation racing a C `getenv` that reads `environ`
    /// outside std's lock) does not apply here — only the ordinary
    /// same-process test race between our own `set_var`/`remove_var`
    /// calls and other Rust readers, which std's own env lock already
    /// serialises and this mutex additionally orders test-to-test.
    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    /// Sets `SURGE_HOME` for the guard's lifetime, restoring the previous
    /// value (or its absence) on drop. Holds [`env_lock`] throughout so
    /// concurrent tests in this binary cannot observe a partial mutation.
    struct EnvGuard {
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(value: &str) -> Self {
            let lock = env_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let prev = std::env::var(SURGE_HOME_ENV).ok();
            // SAFETY: `env_lock()` serialises every test in this binary
            // that touches `SURGE_HOME`, and (per `env_lock`'s doc) this
            // crate links no vendored C that reads `environ` outside
            // std's own lock.
            unsafe { std::env::set_var(SURGE_HOME_ENV, value) };
            Self { prev, _lock: lock }
        }

        fn unset() -> Self {
            let lock = env_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let prev = std::env::var(SURGE_HOME_ENV).ok();
            // SAFETY: see `set` above.
            unsafe { std::env::remove_var(SURGE_HOME_ENV) };
            Self { prev, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: still holding `_lock`; env mutation stays serialised
            // against this module's other tests for the same reason `set`
            // and `unset` are sound.
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
    fn uses_surge_home_when_set_and_non_empty() {
        let _guard = EnvGuard::set("/tmp/surge-core-test-custom-home");
        assert_eq!(
            surge_home_dir(),
            Some(PathBuf::from("/tmp/surge-core-test-custom-home"))
        );
    }

    #[test]
    fn falls_back_to_dot_surge_when_surge_home_is_empty() {
        let _guard = EnvGuard::set("");
        let resolved =
            surge_home_dir().expect("dirs::home_dir() resolves in this test environment");
        assert!(
            resolved.ends_with(".surge"),
            "expected a `.surge` suffix, got {resolved:?}"
        );
        assert_ne!(resolved, PathBuf::from(""));
    }

    #[test]
    fn falls_back_to_dot_surge_when_surge_home_is_unset() {
        let _guard = EnvGuard::unset();
        let resolved =
            surge_home_dir().expect("dirs::home_dir() resolves in this test environment");
        assert!(
            resolved.ends_with(".surge"),
            "expected a `.surge` suffix, got {resolved:?}"
        );
    }
}
