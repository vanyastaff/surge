//! `/run <archetype-or-path>` — start a fresh run from an archetype template.
//!
//! The handler delegates run-start to the [`RunStarter`] trait so unit
//! tests can substitute a fake without standing up `surge-orchestrator`.
//! Production wires this to `ArchetypeRegistry::resolve(name) → Graph`
//! plus `Engine::start_run`.
//!
//! Admission is checked by the runtime BEFORE this handler is reached
//! (see [`crate::cockpit::run::run_cockpit`]), so this module trusts
//! its inputs.

use async_trait::async_trait;

use crate::commands::CommandReply;
use crate::error::Result;

/// Engine surface for starting a run from a named archetype or template
/// path. Production looks `name` up against `ArchetypeRegistry` and calls
/// `Engine::start_run` with the resolved graph.
#[async_trait]
pub trait RunStarter: Send + Sync {
    /// Start a run. Returns the new run id (short form) on success.
    ///
    /// # Errors
    ///
    /// Returns the `TelegramCockpitError::EngineResolve` variant when the
    /// engine refuses to start (unknown archetype, validation failure,
    /// etc.); the caller formats a recoverable reply.
    async fn start_run(&self, archetype_or_path: &str) -> Result<String>;
}

/// Handle `/run <archetype-or-path>`.
///
/// Empty argument string returns a usage hint. Engine refusal is
/// surfaced as a recoverable [`CommandReply`] (not bubbled up) so the
/// chat sees the reason without the cockpit bouncing.
pub async fn handle_run<R: RunStarter>(
    chat_id: i64,
    args: &str,
    starter: &R,
) -> Result<CommandReply> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(CommandReply::new(
            "Usage: `/run <archetype>` — start a run from a bundled or user archetype.",
        ));
    }

    match starter.start_run(trimmed).await {
        Ok(run_id) => {
            tracing::info!(
                target: "telegram::cmd::run",
                chat_id = %chat_id,
                archetype = %trimmed,
                %run_id,
                "run started",
            );
            Ok(CommandReply::new(format!(
                "✅ Started run `{run_id}` from `{trimmed}`."
            )))
        },
        Err(err) => {
            tracing::warn!(
                target: "telegram::cmd::run",
                chat_id = %chat_id,
                archetype = %trimmed,
                error = %err,
                "run start refused",
            );
            Ok(CommandReply::new(format!(
                "❌ Could not start `{trimmed}`: {err}"
            )))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TelegramCockpitError;
    use std::sync::Mutex;

    struct FakeStarter {
        result: Mutex<Result<String>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeStarter {
        fn ok(id: &str) -> Self {
            Self {
                result: Mutex::new(Ok(id.to_owned())),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn err(reason: &str) -> Self {
            Self {
                result: Mutex::new(Err(TelegramCockpitError::Persistence(reason.to_owned()))),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl RunStarter for FakeStarter {
        async fn start_run(&self, archetype_or_path: &str) -> Result<String> {
            self.calls
                .lock()
                .unwrap()
                .push(archetype_or_path.to_owned());
            match &*self.result.lock().unwrap() {
                Ok(id) => Ok(id.clone()),
                Err(_) => Err(TelegramCockpitError::Persistence("test".into())),
            }
        }
    }

    #[tokio::test]
    async fn empty_args_returns_usage() {
        let starter = FakeStarter::ok("run-1");
        let reply = handle_run(42, "   ", &starter).await.unwrap();
        assert!(reply.text.contains("/run"));
        assert!(starter.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn happy_path_starts_run_and_returns_id() {
        let starter = FakeStarter::ok("01HK-RUN");
        let reply = handle_run(42, "rust-crate", &starter).await.unwrap();
        assert!(reply.text.contains("01HK-RUN"));
        assert!(reply.text.contains("rust-crate"));
        let calls = starter.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], "rust-crate");
    }

    #[tokio::test]
    async fn engine_refusal_is_a_recoverable_reply() {
        let starter = FakeStarter::err("unknown archetype");
        let reply = handle_run(42, "nope", &starter).await.unwrap();
        assert!(reply.text.starts_with("❌"));
    }
}
