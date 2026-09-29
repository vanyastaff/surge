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
/// by `surge inbox`). Delegates to [`surge_orchestrator::operator::resolve_run_id`].
pub(crate) async fn resolve_run_id(storage: &Arc<Storage>, value: &str) -> Result<RunId> {
    operator::resolve_run_id(storage, value)
        .await
        .map_err(operator_failure)
}

/// The CLI-side hint that turns a neutral [`OperatorError`] into an
/// actionable message for someone at a terminal: which flag to pass or which
/// command to run next.
fn hint(error: &OperatorError) -> Option<&'static str> {
    match error {
        OperatorError::NotAwaitingInput { .. } => {
            Some("Bootstrap approvals are answered via `surge bootstrap` or Telegram.")
        },
        OperatorError::MissingToolAnswer => Some("Pass --text or --json."),
        OperatorError::MissingOutcome => {
            Some("Pass --outcome <key>; run `surge resolve <run>` for the options.")
        },
        OperatorError::DeliveryFailed { .. } | OperatorError::SteerFailed { .. } => Some(
            "The run must be active in a running daemon (started with `surge engine run --daemon`).",
        ),
        _ => None,
    }
}

/// Convert an operator failure for the CLI: the full cause chain, followed by
/// [`hint`] when there is one.
pub(crate) fn operator_failure(error: OperatorError) -> anyhow::Error {
    let Some(hint) = hint(&error) else {
        return anyhow::Error::new(error);
    };
    anyhow!("{:#}. {hint}", anyhow::Error::new(error))
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
    fn operator_failure_appends_the_cli_hint_after_the_cause_chain() {
        let error = operator_failure(OperatorError::MissingOutcome);
        let text = error.to_string();
        assert!(text.starts_with("this gate needs an outcome. "), "{text}");
        assert!(text.contains("--outcome"), "{text}");

        let plain = operator_failure(OperatorError::BlankSteer);
        assert_eq!(plain.to_string(), "steer message must not be blank");
    }
}
