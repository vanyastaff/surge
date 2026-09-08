//! Shared helpers for run-targeting CLI commands (`inbox`, `ready`, `ledger`,
//! `resolve`, `steer`, `run`). Extracted to a single home so fixes — e.g. the
//! run-id suffix guards below — land once instead of drifting across copies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use surge_core::RunId;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_persistence::runs::Storage;
use surge_persistence::runs::registry::RunFilter;

/// Minimum length of a run-id suffix accepted by [`resolve_run_id`]. Short/empty
/// suffixes (`""` matches every run via `ends_with`) are rejected outright.
const MIN_SUFFIX_LEN: usize = 6;

/// Upper bound on runs scanned when matching a suffix. Chosen well above any
/// realistic active-run count; if a scan hits it, the match is reported as
/// possibly-truncated rather than silently wrong.
const SUFFIX_SCAN_LIMIT: usize = 5000;

/// Resolve `~/.surge` (honoring `SURGE_HOME`).
pub(crate) fn surge_home_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("SURGE_HOME")
        && !custom.is_empty()
    {
        return Ok(PathBuf::from(custom));
    }
    let base = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
    Ok(base.join(".surge"))
}

/// Resolve the project root: the enclosing git repository's root if `cwd` is
/// inside one, else `cwd` itself. Shared by `surge project describe` and
/// `surge memory audit` — both need "the directory `Provenance::source`'s
/// file-path locators are relative to"
/// (`.autopilot/competitive-waves/interfaces.md`, "Контракт локаторов
/// памяти"), not whichever directory the command happened to be invoked
/// from; a memory claim's source is captured once, elsewhere, and must
/// classify and stale-check the same way regardless of the operator's `cwd`
/// on audit day.
pub(crate) fn project_root(cwd: &Path) -> PathBuf {
    surge_git::GitManager::discover()
        .map(|manager| manager.repo_path().to_path_buf())
        .unwrap_or_else(|_| cwd.to_path_buf())
}

/// Resolve a run id, accepting the full ULID or a unique short suffix (as shown
/// by `surge inbox`).
///
/// Guards: an empty or under-[`MIN_SUFFIX_LEN`] suffix is rejected (`ends_with("")`
/// would match every run); if the run scan hits [`SUFFIX_SCAN_LIMIT`], an
/// otherwise-unique match is treated as ambiguous rather than trusted, since a
/// colliding run could sit beyond the window.
pub(crate) async fn resolve_run_id(storage: &Arc<Storage>, value: &str) -> Result<RunId> {
    let value = value.trim();
    if let Ok(id) = value.parse::<RunId>() {
        return Ok(id);
    }
    if value.len() < MIN_SUFFIX_LEN {
        return Err(anyhow!(
            "run id {value:?} is not a full ULID and is too short to match by suffix \
             (need ≥{MIN_SUFFIX_LEN} chars, or pass the full id)"
        ));
    }
    let runs = storage
        .list_runs(RunFilter {
            status: None,
            project_path: None,
            limit: Some(SUFFIX_SCAN_LIMIT),
        })
        .await
        .context("list runs for id match")?;
    let truncated = runs.len() >= SUFFIX_SCAN_LIMIT;
    let matches: Vec<RunId> = runs
        .iter()
        .filter(|r| r.id.to_string().ends_with(value))
        .map(|r| r.id)
        .collect();
    match matches.as_slice() {
        [one] if !truncated => Ok(*one),
        [_one] => Err(anyhow!(
            "matched run {value:?}, but there are ≥{SUFFIX_SCAN_LIMIT} runs so the match \
             may be ambiguous; pass the full run id"
        )),
        [] => Err(anyhow!("no run matching {value:?}")),
        many => Err(anyhow!(
            "{} runs match {value:?}; use the full run id",
            many.len()
        )),
    }
}

/// Connect to the already-running daemon (does not spawn one — a fresh daemon
/// would not hold the run's in-memory state that these commands act on).
pub(crate) async fn connect_daemon() -> Result<DaemonEngineFacade> {
    let socket = surge_daemon::pidfile::socket_path().context("resolve daemon socket path")?;
    DaemonEngineFacade::connect(socket)
        .await
        .map_err(|e| anyhow!("no running daemon to reach: {e}"))
}
