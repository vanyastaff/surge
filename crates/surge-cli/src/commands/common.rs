//! Shared helpers for run-targeting CLI commands (`inbox`, `ready`, `ledger`,
//! `resolve`, `steer`, `run`). Extracted to a single home so fixes — e.g. the
//! run-id suffix guards below — land once instead of drifting across copies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use surge_core::RunId;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::operator::{self, OperatorError};
use surge_persistence::runs::Storage;

pub(crate) use surge_orchestrator::operator::RunIdError;

/// Resolve `~/.surge` (honoring `SURGE_HOME`). Delegates to
/// [`surge_core::home::surge_home_dir`], the canonical resolver shared
/// with the daemon's `pidfile::daemon_dir` and the persistence layer's
/// memory store.
pub(crate) fn surge_home_dir() -> Result<PathBuf> {
    surge_core::home::surge_home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))
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
/// by `surge inbox`). Delegates to [`surge_orchestrator::operator::resolve_run_id`];
/// a bad id surfaces as a [`RunIdError`] so callers can tell it from a storage
/// fault with `error.downcast_ref::<RunIdError>()`.
pub(crate) async fn resolve_run_id(storage: &Arc<Storage>, value: &str) -> Result<RunId> {
    operator::resolve_run_id(storage, value)
        .await
        .map_err(|error| match error {
            OperatorError::RunId(bad_id) => anyhow::Error::new(bad_id),
            fault => anyhow::Error::new(fault),
        })
}

/// Connect to the already-running daemon (does not spawn one — a fresh daemon
/// would not hold the run's in-memory state that these commands act on).
pub(crate) async fn connect_daemon() -> Result<DaemonEngineFacade> {
    let socket = surge_daemon::pidfile::socket_path().context("resolve daemon socket path")?;
    connect_daemon_at(socket).await
}

/// [`connect_daemon`] against an explicit socket path (the MCP server resolves
/// its home once at startup instead of re-reading `SURGE_HOME` per call).
pub(crate) async fn connect_daemon_at(socket: PathBuf) -> Result<DaemonEngineFacade> {
    DaemonEngineFacade::connect(socket)
        .await
        .map_err(|e| anyhow!("no running daemon to reach: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_errors_read_as_the_old_messages_and_survive_anyhow() {
        let err: anyhow::Error = RunIdError::NotFound {
            value: "ABCDEF".into(),
        }
        .into();
        assert_eq!(err.to_string(), "no run matching \"ABCDEF\"");
        assert!(matches!(
            err.downcast_ref::<RunIdError>(),
            Some(RunIdError::NotFound { .. })
        ));

        let too_short: anyhow::Error = RunIdError::TooShort { value: "x".into() }.into();
        assert!(too_short.to_string().contains("too short"));

        // A storage fault is anyhow context, never a RunIdError.
        let fault = anyhow::anyhow!("disk").context("list runs for id match");
        assert!(fault.downcast_ref::<RunIdError>().is_none());
    }
}
