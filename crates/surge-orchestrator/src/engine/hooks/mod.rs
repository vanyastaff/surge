//! Hook execution chain.
//!
//! Resolves a [`Profile`]'s hooks by trigger + matcher and executes them
//! sequentially. Inheritance via `Profile.role.extends` is **out of scope**
//! and deferred to the `Profile registry & bundled roles` milestone — the
//! executor accepts an already-resolved profile and operates on
//! `profile.hooks.entries` directly.
//!
//! ## Design
//!
//! Process spawning is hidden behind [`HookCommandSpawner`] so tests can
//! substitute an in-memory recorder. Production wiring uses
//! [`ProcessSpawner`], which spawns `tokio::process::Command` with the
//! per-hook `timeout_seconds`.
//!
//! ## Outcome semantics
//!
//! - [`HookOutcome::Proceed`]   — every hook either succeeded or failed in a
//!   non-rejecting mode.
//! - [`HookOutcome::Reject`]    — a hook configured with
//!   `HookFailureMode::Reject` exited non-zero. Iteration short-circuits.
//! - [`HookOutcome::Suppress`]  — `on_error` only: a hook claimed an outcome
//!   key on stdout via `{"action":"suppress","outcome":"<key>"}`. The caller
//!   validates the key against the node's declared outcomes (Task 1.4).
//!
//! Each executed hook produces a [`HookExecutionRecord`] that callers can
//! persist via [`record_hook_executed`] as `EventPayload::HookExecuted`.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use surge_core::hooks::{Hook, HookFailureMode, HookTrigger, MatchContext};
use surge_core::id::SessionId;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::profile::Profile;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::run_writer::RunWriter;
use tokio::io::AsyncReadExt;

/// Per-call dynamic context the engine assembles for the hook chain.
///
/// `node` is required (every hook fires inside a stage); the optional fields
/// reflect the trigger surface — `tool` / `tool_args_text` for tool triggers,
/// `outcome` for `on_outcome`, `last_error` for `on_error`, and `file_path`
/// for matchers using `MatcherSpec::file_glob`.
#[derive(Debug, Clone)]
pub struct HookContext<'a> {
    /// Stage node currently executing — required for matcher node filters and
    /// the `EventPayload::HookExecuted` audit record.
    pub node: &'a NodeKey,
    /// ACP session in flight, if any. Tool / outcome triggers carry one.
    pub session: Option<SessionId>,
    /// Tool name for `pre_tool_use` / `post_tool_use` triggers.
    pub tool: Option<&'a str>,
    /// Raw textual representation of the tool arguments — fed to
    /// `MatcherSpec::tool_arg_contains`.
    pub tool_args_text: Option<&'a str>,
    /// Outcome key for `on_outcome` triggers.
    pub outcome: Option<&'a OutcomeKey>,
    /// Failure reason that triggered an `on_error` hook chain.
    pub last_error: Option<&'a str>,
    /// File path used by `MatcherSpec::file_glob`.
    pub file_path: Option<&'a Path>,
}

impl<'a> HookContext<'a> {
    /// Construct a minimal context for a stage that only knows the node key.
    /// Callers fill the optional fields in builder-style, keeping the
    /// agent-stage call site readable.
    #[must_use]
    pub fn for_node(node: &'a NodeKey) -> Self {
        Self {
            node,
            session: None,
            tool: None,
            tool_args_text: None,
            outcome: None,
            last_error: None,
            file_path: None,
        }
    }

    /// Attach the active ACP session id.
    #[must_use]
    pub fn with_session(mut self, session: SessionId) -> Self {
        self.session = Some(session);
        self
    }

    /// Attach the tool name (and optional textual args) for tool triggers.
    #[must_use]
    pub fn with_tool(mut self, tool: &'a str, args_text: Option<&'a str>) -> Self {
        self.tool = Some(tool);
        self.tool_args_text = args_text;
        self
    }

    /// Attach the candidate outcome key for `on_outcome`.
    #[must_use]
    pub fn with_outcome(mut self, outcome: &'a OutcomeKey) -> Self {
        self.outcome = Some(outcome);
        self
    }

    /// Attach the failure reason for `on_error`.
    #[must_use]
    pub fn with_error(mut self, reason: &'a str) -> Self {
        self.last_error = Some(reason);
        self
    }

    /// Attach the file path used by `MatcherSpec::file_glob`.
    #[must_use]
    pub fn with_file_path(mut self, path: &'a Path) -> Self {
        self.file_path = Some(path);
        self
    }

    fn to_match_context(&self, trigger: HookTrigger) -> MatchContext<'a> {
        MatchContext {
            trigger,
            tool: self.tool,
            tool_args_text: self.tool_args_text,
            outcome: self.outcome,
            node: Some(self.node),
            file_path: self.file_path,
        }
    }
}

/// Verdict produced by [`HookExecutor::execute_chain`] / [`HookExecutor::run_hooks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// Every matching hook completed without a Reject-mode failure.
    Proceed {
        /// Records for every hook that actually ran (post-matcher).
        executed: Vec<HookExecutionRecord>,
    },
    /// A `HookFailureMode::Reject` hook exited non-zero. Iteration stopped.
    Reject {
        /// Human-readable rejection reason — typically the failing hook's stderr.
        reason: String,
        /// Identifier of the hook that caused the rejection.
        hook_id: String,
        /// Records for every hook that ran up to and including the rejecter.
        executed: Vec<HookExecutionRecord>,
    },
    /// `on_error` only: a hook converted the stage failure into an outcome.
    /// The caller validates `outcome` against the node's declared outcomes.
    Suppress {
        /// Outcome key the hook claimed via stdout JSON directive.
        outcome: OutcomeKey,
        /// Identifier of the suppressing hook.
        hook_id: String,
        /// Records for every hook that ran before suppression.
        executed: Vec<HookExecutionRecord>,
    },
}

impl HookOutcome {
    /// Borrow the audit records produced by the chain regardless of variant.
    #[must_use]
    pub fn executed(&self) -> &[HookExecutionRecord] {
        match self {
            Self::Proceed { executed }
            | Self::Reject { executed, .. }
            | Self::Suppress { executed, .. } => executed,
        }
    }

    /// Convenience predicate for callers that only care about the happy path.
    #[must_use]
    pub fn is_proceed(&self) -> bool {
        matches!(self, Self::Proceed { .. })
    }
}

/// Persisted record describing one hook invocation. Callers append it via
/// [`record_hook_executed`] as `EventPayload::HookExecuted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookExecutionRecord {
    /// `Hook.id` — stable identifier referenced by audit consumers.
    pub hook_id: String,
    /// Process exit status. `124` indicates the spawner killed the command
    /// after `timeout_seconds`.
    pub exit_status: i32,
    /// `HookFailureMode` carried over from the source hook so audit consumers
    /// know how the executor classified the result without re-resolving.
    pub on_failure: HookFailureMode,
    /// `true` when the spawner's timeout fired before the command exited.
    pub timed_out: bool,
}

/// Result of spawning a single hook command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCommandResult {
    /// Exit status as a signed integer. `124` is reserved for "killed by
    /// timeout"; the spawner sets it explicitly when the command exceeded
    /// `timeout_seconds`.
    pub exit_status: i32,
    /// Captured stdout. Used for `on_error` suppression directives.
    pub stdout: String,
    /// Captured stderr. Surfaced in `HookOutcome::Reject::reason`.
    pub stderr: String,
    /// `true` when the timeout fired and the spawner forced exit status 124.
    pub timed_out: bool,
}

/// Strategy interface for running hook commands.
///
/// The default [`ProcessSpawner`] runs the hook through the platform shell and
/// enforces `timeout_seconds`. Tests substitute [`RecordingSpawner`] (in
/// `#[cfg(test)]`) to inspect the call sequence without touching the OS.
#[async_trait]
pub trait HookCommandSpawner: Send + Sync {
    /// Run the hook's command and return the captured outcome. Implementations
    /// must surface timeouts as `exit_status: 124, timed_out: true` rather than
    /// returning an error so the executor can apply `HookFailureMode` mapping.
    async fn spawn(&self, hook: &Hook) -> HookCommandResult;
}

/// Production [`HookCommandSpawner`] backed by `tokio::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessSpawner;

#[async_trait]
impl HookCommandSpawner for ProcessSpawner {
    async fn spawn(&self, hook: &Hook) -> HookCommandResult {
        let timeout = hook.timeout_seconds.map(|s| Duration::from_secs(u64::from(s)));
        match spawn_via_shell(&hook.command, timeout).await {
            Ok(res) => res,
            Err(err) => {
                tracing::error!(
                    target: "engine::hooks",
                    hook_id = %hook.id,
                    err = %err,
                    "hook command spawn failed"
                );
                HookCommandResult {
                    exit_status: -1,
                    stdout: String::new(),
                    stderr: format!("spawn error: {err}"),
                    timed_out: false,
                }
            },
        }
    }
}

async fn spawn_via_shell(
    command: &str,
    timeout: Option<Duration>,
) -> Result<HookCommandResult, std::io::Error> {
    use tokio::process::Command;

    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    };

    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    };

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let wait_fut = async {
        let status = child.wait().await?;
        let mut stdout = String::new();
        if let Some(mut p) = stdout_pipe.take() {
            let _ = p.read_to_string(&mut stdout).await;
        }
        let mut stderr = String::new();
        if let Some(mut p) = stderr_pipe.take() {
            let _ = p.read_to_string(&mut stderr).await;
        }
        let exit_status = status.code().unwrap_or(-1);
        Ok::<HookCommandResult, std::io::Error>(HookCommandResult {
            exit_status,
            stdout,
            stderr,
            timed_out: false,
        })
    };

    if let Some(dur) = timeout {
        match tokio::time::timeout(dur, wait_fut).await {
            Ok(res) => res,
            Err(_) => Ok(HookCommandResult {
                exit_status: 124,
                stdout: String::new(),
                stderr: format!("hook command exceeded timeout {dur:?}"),
                timed_out: true,
            }),
        }
    } else {
        wait_fut.await
    }
}

/// Hook execution facade. The default spawner runs a real shell; tests use
/// `HookExecutor::with_spawner(...)` to inject [`HookCommandSpawner`] mocks.
#[derive(Debug, Default, Clone)]
pub struct HookExecutor<S = ProcessSpawner> {
    spawner: S,
}

impl HookExecutor<ProcessSpawner> {
    /// Construct the production executor backed by [`ProcessSpawner`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            spawner: ProcessSpawner,
        }
    }
}

impl<S: HookCommandSpawner> HookExecutor<S> {
    /// Construct an executor with a caller-supplied spawner — used by tests
    /// to substitute deterministic in-memory behaviour.
    #[must_use]
    pub fn with_spawner(spawner: S) -> Self {
        Self { spawner }
    }

    /// Resolve and execute every hook on `profile` matching `trigger` + `ctx`.
    pub async fn execute_chain(
        &self,
        profile: &Profile,
        trigger: HookTrigger,
        ctx: &HookContext<'_>,
    ) -> HookOutcome {
        self.run_hooks(&profile.hooks.entries, trigger, ctx).await
    }

    /// Execute the supplied hooks sequentially. Centralizes filtering so
    /// callers without a [`Profile`] (e.g. agent stages reading
    /// `AgentConfig.hooks`) reuse the same engine.
    pub async fn run_hooks(
        &self,
        hooks: &[Hook],
        trigger: HookTrigger,
        ctx: &HookContext<'_>,
    ) -> HookOutcome {
        let match_ctx = ctx.to_match_context(trigger);
        let mut executed = Vec::new();
        let mut resolved = 0_usize;

        for hook in hooks.iter().filter(|h| h.matches(trigger, &match_ctx)) {
            resolved += 1;
            tracing::debug!(
                target: "engine::hooks",
                hook_id = %hook.id,
                trigger = ?trigger,
                node = %ctx.node,
                "hook.start"
            );
            let res = self.spawner.spawn(hook).await;
            tracing::debug!(
                target: "engine::hooks",
                hook_id = %hook.id,
                exit_status = res.exit_status,
                timed_out = res.timed_out,
                "hook.exit"
            );

            executed.push(HookExecutionRecord {
                hook_id: hook.id.clone(),
                exit_status: res.exit_status,
                on_failure: hook.on_failure,
                timed_out: res.timed_out,
            });

            // Treat timeout as a non-zero exit; honour on_failure mapping below.
            let failed = res.exit_status != 0;
            if failed {
                match hook.on_failure {
                    HookFailureMode::Reject => {
                        let reason = if res.stderr.is_empty() {
                            format!("hook '{}' exited {}", hook.id, res.exit_status)
                        } else {
                            res.stderr.trim().to_owned()
                        };
                        tracing::warn!(
                            target: "engine::hooks",
                            hook_id = %hook.id,
                            exit_status = res.exit_status,
                            "hook.reject"
                        );
                        return HookOutcome::Reject {
                            reason,
                            hook_id: hook.id.clone(),
                            executed,
                        };
                    },
                    HookFailureMode::Warn => {
                        tracing::warn!(
                            target: "engine::hooks",
                            hook_id = %hook.id,
                            exit_status = res.exit_status,
                            stderr = %res.stderr.trim(),
                            "hook.warn"
                        );
                    },
                    HookFailureMode::Ignore => {},
                }
                continue;
            }

            // Successful run — only `on_error` may signal suppression via stdout.
            if trigger == HookTrigger::OnError && let Some(suppress) = parse_suppress(&res.stdout) {
                tracing::info!(
                    target: "engine::hooks",
                    hook_id = %hook.id,
                    outcome = %suppress.as_str(),
                    "hook.suppress"
                );
                return HookOutcome::Suppress {
                    outcome: suppress,
                    hook_id: hook.id.clone(),
                    executed,
                };
            }
        }

        tracing::debug!(
            target: "engine::hooks",
            trigger = ?trigger,
            node = %ctx.node,
            resolved,
            "hook.chain_done"
        );
        HookOutcome::Proceed { executed }
    }
}

#[derive(Debug, Deserialize)]
struct SuppressDirective {
    action: String,
    outcome: String,
}

fn parse_suppress(stdout: &str) -> Option<OutcomeKey> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let directive: SuppressDirective = serde_json::from_str(trimmed).ok()?;
    if directive.action != "suppress" {
        return None;
    }
    OutcomeKey::try_from(directive.outcome.as_str()).ok()
}

/// Persist one [`HookExecutionRecord`] as `EventPayload::HookExecuted`.
///
/// Storage failures are logged but not surfaced — the hook ran; the engine
/// should not abort the stage just because the audit append failed. Callers
/// that need stricter durability can append the event themselves.
pub async fn record_hook_executed(writer: &RunWriter, record: &HookExecutionRecord) {
    let payload = EventPayload::HookExecuted {
        hook_id: record.hook_id.clone(),
        exit_status: record.exit_status,
        on_failure: record.on_failure,
    };
    if let Err(err) = writer.append_event(VersionedEventPayload::new(payload)).await {
        tracing::warn!(
            target: "engine::hooks",
            hook_id = %record.hook_id,
            err = %err,
            "failed to persist HookExecuted"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use surge_core::hooks::{HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
    use surge_core::keys::NodeKey;

    /// Test spawner: returns scripted results in order and records what was
    /// asked of it. Production paths run real shell commands via
    /// `ProcessSpawner`; this lets unit tests stay deterministic.
    #[derive(Default)]
    struct RecordingSpawner {
        calls: Mutex<Vec<String>>,
        scripted: Mutex<Vec<HookCommandResult>>,
    }

    impl RecordingSpawner {
        fn with(results: Vec<HookCommandResult>) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                scripted: Mutex::new(results),
            })
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HookCommandSpawner for Arc<RecordingSpawner> {
        async fn spawn(&self, hook: &Hook) -> HookCommandResult {
            self.calls.lock().unwrap().push(hook.id.clone());
            self.scripted.lock().unwrap().remove(0)
        }
    }

    fn ok_result() -> HookCommandResult {
        HookCommandResult {
            exit_status: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn err_result(stderr: &str) -> HookCommandResult {
        HookCommandResult {
            exit_status: 1,
            stdout: String::new(),
            stderr: stderr.into(),
            timed_out: false,
        }
    }

    fn suppress_result(outcome: &str) -> HookCommandResult {
        HookCommandResult {
            exit_status: 0,
            stdout: format!(r#"{{"action":"suppress","outcome":"{outcome}"}}"#),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn hook(id: &str, trigger: HookTrigger, on_failure: HookFailureMode) -> Hook {
        Hook {
            id: id.into(),
            trigger,
            matcher: MatcherSpec::default(),
            command: "echo placeholder".into(),
            on_failure,
            timeout_seconds: Some(5),
            inherit: HookInheritance::Extend,
        }
    }

    #[tokio::test]
    async fn proceed_when_all_hooks_succeed() {
        let spawner = RecordingSpawner::with(vec![ok_result(), ok_result()]);
        let exec = HookExecutor::with_spawner(spawner.clone());
        let hooks = vec![
            hook("a", HookTrigger::PreToolUse, HookFailureMode::Reject),
            hook("b", HookTrigger::PreToolUse, HookFailureMode::Warn),
        ];
        let node = NodeKey::try_from("agent_1").unwrap();
        let ctx = HookContext::for_node(&node);

        let outcome = exec
            .run_hooks(&hooks, HookTrigger::PreToolUse, &ctx)
            .await;

        assert!(matches!(outcome, HookOutcome::Proceed { .. }));
        assert_eq!(outcome.executed().len(), 2);
        assert_eq!(spawner.calls(), vec!["a".to_owned(), "b".to_owned()]);
    }

    #[tokio::test]
    async fn reject_short_circuits_chain() {
        let spawner =
            RecordingSpawner::with(vec![ok_result(), err_result("blocked"), ok_result()]);
        let exec = HookExecutor::with_spawner(spawner.clone());
        let hooks = vec![
            hook("first", HookTrigger::PreToolUse, HookFailureMode::Warn),
            hook("blocker", HookTrigger::PreToolUse, HookFailureMode::Reject),
            hook("never", HookTrigger::PreToolUse, HookFailureMode::Reject),
        ];
        let node = NodeKey::try_from("agent_1").unwrap();
        let ctx = HookContext::for_node(&node);

        let outcome = exec
            .run_hooks(&hooks, HookTrigger::PreToolUse, &ctx)
            .await;

        match outcome {
            HookOutcome::Reject {
                reason,
                hook_id,
                executed,
            } => {
                assert_eq!(hook_id, "blocker");
                assert_eq!(reason, "blocked");
                assert_eq!(executed.len(), 2, "should not have called 'never'");
            },
            other => panic!("expected Reject, got {other:?}"),
        }
        assert_eq!(spawner.calls(), vec!["first".to_owned(), "blocker".to_owned()]);
    }

    #[tokio::test]
    async fn warn_failure_mode_does_not_block() {
        let spawner = RecordingSpawner::with(vec![err_result("noisy"), ok_result()]);
        let exec = HookExecutor::with_spawner(spawner.clone());
        let hooks = vec![
            hook("warning-only", HookTrigger::PostToolUse, HookFailureMode::Warn),
            hook("clean", HookTrigger::PostToolUse, HookFailureMode::Reject),
        ];
        let node = NodeKey::try_from("agent_1").unwrap();
        let ctx = HookContext::for_node(&node);

        let outcome = exec
            .run_hooks(&hooks, HookTrigger::PostToolUse, &ctx)
            .await;

        assert!(matches!(outcome, HookOutcome::Proceed { .. }));
        assert_eq!(outcome.executed().len(), 2);
    }

    #[tokio::test]
    async fn ignore_failure_mode_is_silent_continue() {
        let spawner = RecordingSpawner::with(vec![err_result("ignored"), ok_result()]);
        let exec = HookExecutor::with_spawner(spawner.clone());
        let hooks = vec![
            hook("ignored", HookTrigger::PostToolUse, HookFailureMode::Ignore),
            hook("clean", HookTrigger::PostToolUse, HookFailureMode::Reject),
        ];
        let node = NodeKey::try_from("agent_1").unwrap();
        let ctx = HookContext::for_node(&node);

        let outcome = exec
            .run_hooks(&hooks, HookTrigger::PostToolUse, &ctx)
            .await;

        assert!(matches!(outcome, HookOutcome::Proceed { .. }));
    }

    #[tokio::test]
    async fn matcher_filters_unmatched_hooks() {
        let spawner = RecordingSpawner::with(vec![ok_result()]);
        let exec = HookExecutor::with_spawner(spawner.clone());
        let mut filtered = hook("filtered", HookTrigger::PreToolUse, HookFailureMode::Reject);
        filtered.matcher = MatcherSpec {
            tool: Some("write_file".into()),
            ..Default::default()
        };
        let runs = hook("runs", HookTrigger::PreToolUse, HookFailureMode::Reject);
        let hooks = vec![filtered, runs];

        let node = NodeKey::try_from("agent_1").unwrap();
        let ctx = HookContext::for_node(&node).with_tool("read_file", None);

        let outcome = exec
            .run_hooks(&hooks, HookTrigger::PreToolUse, &ctx)
            .await;

        assert!(matches!(outcome, HookOutcome::Proceed { .. }));
        assert_eq!(spawner.calls(), vec!["runs".to_owned()]);
    }

    #[tokio::test]
    async fn trigger_mismatch_skips_hook() {
        let spawner = RecordingSpawner::with(vec![]);
        let exec = HookExecutor::with_spawner(spawner.clone());
        let hooks = vec![hook("only-pre", HookTrigger::PreToolUse, HookFailureMode::Reject)];

        let node = NodeKey::try_from("agent_1").unwrap();
        let ctx = HookContext::for_node(&node);

        let outcome = exec
            .run_hooks(&hooks, HookTrigger::OnOutcome, &ctx)
            .await;

        assert!(matches!(outcome, HookOutcome::Proceed { .. }));
        assert!(spawner.calls().is_empty());
    }

    #[tokio::test]
    async fn on_error_suppress_directive_is_recognized() {
        let spawner = RecordingSpawner::with(vec![suppress_result("retry_later")]);
        let exec = HookExecutor::with_spawner(spawner.clone());
        let hooks = vec![hook("recover", HookTrigger::OnError, HookFailureMode::Warn)];

        let node = NodeKey::try_from("impl_1").unwrap();
        let ctx = HookContext::for_node(&node).with_error("transient");

        let outcome = exec.run_hooks(&hooks, HookTrigger::OnError, &ctx).await;

        match outcome {
            HookOutcome::Suppress {
                outcome, hook_id, ..
            } => {
                assert_eq!(outcome.as_str(), "retry_later");
                assert_eq!(hook_id, "recover");
            },
            other => panic!("expected Suppress, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn suppress_only_recognized_for_on_error_trigger() {
        let spawner = RecordingSpawner::with(vec![suppress_result("retry_later")]);
        let exec = HookExecutor::with_spawner(spawner.clone());
        let hooks = vec![hook(
            "post",
            HookTrigger::PostToolUse,
            HookFailureMode::Warn,
        )];

        let node = NodeKey::try_from("impl_1").unwrap();
        let ctx = HookContext::for_node(&node);

        let outcome = exec
            .run_hooks(&hooks, HookTrigger::PostToolUse, &ctx)
            .await;

        // PostToolUse never triggers Suppress even when stdout looks like one.
        assert!(matches!(outcome, HookOutcome::Proceed { .. }));
    }

    #[tokio::test]
    async fn process_spawner_runs_real_command() {
        let exec = HookExecutor::new();

        #[cfg(target_os = "windows")]
        let command = "exit 0";
        #[cfg(not(target_os = "windows"))]
        let command = "true";

        let h = Hook {
            id: "true".into(),
            trigger: HookTrigger::PostToolUse,
            matcher: MatcherSpec::default(),
            command: command.into(),
            on_failure: HookFailureMode::Reject,
            timeout_seconds: Some(5),
            inherit: HookInheritance::Extend,
        };

        let node = NodeKey::try_from("agent_1").unwrap();
        let ctx = HookContext::for_node(&node);

        let outcome = exec.run_hooks(&[h], HookTrigger::PostToolUse, &ctx).await;
        assert!(outcome.is_proceed(), "got {outcome:?}");
        let executed = outcome.executed();
        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].exit_status, 0);
        assert!(!executed[0].timed_out);
    }
}
