//! Runtime-version probing for [`surge_core::RuntimeKind`] binaries.
//!
//! Production callers (the agent stage at session-open time, `surge doctor`,
//! the agent registry's `--healthcheck`) all need to ask the same question:
//! *what version of the runtime CLI is on PATH, and does it satisfy the
//! declared `min_version` policy?*
//!
//! This module owns:
//! - [`probe_version`] — invokes `<binary> --version` with a tight timeout
//!   and parses semver out of the first stdout line.
//! - [`VersionCache`] — per-daemon-lifetime cache, keyed by canonicalised
//!   binary path, so the engine probes each runtime at most once per
//!   process start.
//! - [`evaluate_against_policy`] — compares a parsed version against the
//!   bundled [`surge_core::RuntimeVersionPolicy`] and yields a
//!   ready-to-append [`surge_core::EventPayload::RuntimeVersionWarning`]
//!   payload (or `None` when the policy is satisfied / undeclared).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use semver::Version;
use surge_core::runtime::{RuntimeKind, RuntimeVersionPolicy};
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

/// Probe timeout. Tight on purpose — production callers run this on the hot
/// path of session open; if the binary cannot answer `--version` within a
/// second it almost certainly cannot drive an interactive ACP session.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Reasons [`probe_version`] failed.
#[derive(Debug, Clone, Error)]
pub enum ProbeError {
    /// `tokio::process::Command::spawn` failed (binary missing, permission
    /// denied). The OS error is rendered as a string so `ProbeError` stays
    /// `Clone` for caching.
    #[error("spawn `{binary}` failed: {message}")]
    SpawnFailed {
        /// Binary that was attempted.
        binary: String,
        /// Renderable OS error.
        message: String,
    },
    /// The probe ran but exceeded [`PROBE_TIMEOUT`].
    #[error("`{binary} --version` timed out after {elapsed_ms}ms")]
    Timeout {
        /// Binary that was attempted.
        binary: String,
        /// Wall-clock elapsed at cancellation.
        elapsed_ms: u64,
    },
    /// The probe exited with a non-zero status. The first 256 bytes of
    /// stderr are captured for diagnostics.
    #[error("`{binary} --version` exited with status {exit_code:?}: {stderr}")]
    NonZeroExit {
        /// Binary that was attempted.
        binary: String,
        /// OS exit code (or `None` when killed by signal).
        exit_code: Option<i32>,
        /// Truncated stderr for diagnostics.
        stderr: String,
    },
    /// stdout did not contain a parseable semver on its first line.
    #[error("`{binary} --version` produced unparseable version: `{raw}`")]
    UnparseableVersion {
        /// Binary that was attempted.
        binary: String,
        /// First whitespace-trimmed line of stdout.
        raw: String,
    },
}

/// Run `<binary> --version` and parse a semver from the first line.
///
/// Normalisation: strips a leading `v` (`v1.2.3` is common) and takes the
/// first whitespace-separated token, which catches the `claude 2.0.1
/// (build-sha)` shape too.
///
/// # Errors
///
/// See [`ProbeError`].
pub async fn probe_version(binary: &Path) -> Result<Version, ProbeError> {
    let started = std::time::Instant::now();
    let binary_str = binary.display().to_string();

    let mut cmd = Command::new(binary);
    cmd.arg("--version");
    cmd.stdin(std::process::Stdio::null());

    let spawn = cmd.output();
    let output = match timeout(PROBE_TIMEOUT, spawn).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(ProbeError::SpawnFailed {
                binary: binary_str,
                message: e.to_string(),
            });
        },
        Err(_elapsed) => {
            return Err(ProbeError::Timeout {
                binary: binary_str,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            });
        },
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let truncated: String = stderr.chars().take(256).collect();
        return Err(ProbeError::NonZeroExit {
            binary: binary_str,
            exit_code: output.status.code(),
            stderr: truncated,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next().unwrap_or_default().trim();

    // Try each whitespace-separated token until one parses as semver.
    // Real-world CLIs ship lines like:
    //   "cargo 1.92.0 (build sha)"
    //   "rustup 1.29.0 (commit date)"
    //   "claude 2.0.1"
    //   "v2.0.1"
    // Scanning past leading non-semver tokens handles all of them.
    for token in first_line.split_whitespace() {
        let stripped = token.strip_prefix('v').unwrap_or(token);
        if let Ok(version) = Version::parse(stripped) {
            return Ok(version);
        }
    }

    Err(ProbeError::UnparseableVersion {
        binary: binary_str,
        raw: first_line.to_string(),
    })
}

/// Compare a detected version against a declared policy.
///
/// Returns:
/// - `None` when `policy` is satisfied (or `policy = None` — no minimum
///   declared).
/// - `Some(RuntimeVersionWarningPayload)` when the detected version is
///   below the policy. The caller appends this as
///   [`surge_core::EventPayload::RuntimeVersionWarning`] and emits a
///   `tracing::warn!` log line.
#[must_use]
pub fn evaluate_against_policy(
    detected: &Version,
    policy: Option<&RuntimeVersionPolicy>,
) -> Option<RuntimeVersionWarningPayload> {
    let policy = policy?;
    if policy.min_version.matches(detected) {
        return None;
    }
    Some(RuntimeVersionWarningPayload {
        runtime: policy.runtime,
        found_version: detected.to_string(),
        min_version: policy.min_version.to_string(),
    })
}

/// Pre-built payload for the version-warning event. The engine appends
/// this directly via `EventPayload::RuntimeVersionWarning { .. }` after
/// emitting a `warn` log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeVersionWarningPayload {
    /// Runtime the warning applies to.
    pub runtime: RuntimeKind,
    /// Version detected on disk.
    pub found_version: String,
    /// Declared minimum version requirement.
    pub min_version: String,
}

/// Per-daemon-lifetime cache wrapping [`probe_version`].
///
/// Keyed by the canonicalised binary path so different `PATH` entries for
/// the same logical agent (e.g. `~/.local/bin/claude` vs
/// `/usr/local/bin/claude`) probe separately — the user may have installed
/// each from a different source.
#[derive(Debug, Default)]
pub struct VersionCache {
    inner: Mutex<HashMap<PathBuf, Result<Version, ProbeError>>>,
}

impl VersionCache {
    /// New empty cache.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Probe (or return a cached result) for `binary`. Paths are canonicalised
    /// before lookup so callers can pass relative paths without breaking
    /// cache reuse.
    pub async fn probe(&self, binary: &Path) -> Result<Version, ProbeError> {
        let key = binary.canonicalize().unwrap_or_else(|_| binary.to_path_buf());
        if let Some(cached) = self.inner.lock().await.get(&key) {
            return cached.clone();
        }
        let result = probe_version(&key).await;
        self.inner.lock().await.insert(key, result.clone());
        result
    }

    /// Current cache size (for tests and observability).
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// `true` when no probes have been recorded.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::VersionReq;

    fn policy(min: &str) -> RuntimeVersionPolicy {
        RuntimeVersionPolicy::new(RuntimeKind::ClaudeCode, VersionReq::parse(min).unwrap())
            .with_note("test policy")
    }

    #[test]
    fn evaluate_satisfied_returns_none() {
        let detected = Version::parse("2.1.0").unwrap();
        assert!(evaluate_against_policy(&detected, Some(&policy(">=2.0.0"))).is_none());
    }

    #[test]
    fn evaluate_below_minimum_returns_warning() {
        let detected = Version::parse("1.9.0").unwrap();
        let warning = evaluate_against_policy(&detected, Some(&policy(">=2.0.0")))
            .expect("below minimum");
        assert_eq!(warning.runtime, RuntimeKind::ClaudeCode);
        assert_eq!(warning.found_version, "1.9.0");
        assert!(warning.min_version.contains(">=2.0.0"));
    }

    #[test]
    fn evaluate_without_policy_returns_none() {
        let detected = Version::parse("0.0.1").unwrap();
        assert!(evaluate_against_policy(&detected, None).is_none());
    }

    #[tokio::test]
    async fn probe_missing_binary_returns_spawn_failed() {
        // A path that definitely does not exist.
        let result = probe_version(Path::new("/__definitely-not-a-real-binary__/x")).await;
        assert!(matches!(result, Err(ProbeError::SpawnFailed { .. })));
    }

    #[tokio::test]
    async fn probe_known_binary_parses_semver() {
        // Use `cargo` itself as a known-installed binary that prints semver
        // on `--version`. Skip the test on PATHs that don't have it.
        let cargo = which::which("cargo").ok();
        let Some(cargo) = cargo else {
            eprintln!("skipping: cargo not on PATH");
            return;
        };
        let version = probe_version(&cargo).await.expect("cargo --version probes");
        // Cargo versions are stable semver — just assert the major is at
        // least 1 to confirm we parsed something sensible.
        assert!(version.major >= 1);
    }

    #[tokio::test]
    async fn version_cache_is_idempotent_per_path() {
        let cargo = which::which("cargo").ok();
        let Some(cargo) = cargo else {
            eprintln!("skipping: cargo not on PATH");
            return;
        };
        let cache = VersionCache::new();
        let first = cache.probe(&cargo).await.expect("probe");
        let second = cache.probe(&cargo).await.expect("probe");
        assert_eq!(first, second);
        assert_eq!(cache.len().await, 1);
    }
}
