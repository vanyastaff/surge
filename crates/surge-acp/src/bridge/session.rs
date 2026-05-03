//! Session inputs (`SessionConfig`, `MessageContent`) and bridge-side
//! per-session state (`SessionState`, `AcpSession`). See spec §4.3 / §4.4.

use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_client_protocol::ContentBlock;
use surge_core::{OutcomeKey, SessionId};

use super::sandbox::Sandbox;
use super::tools::ToolDef;

/// Public read-back of a session's bridge-observable state.
/// Returned by `AcpBridge::session_state`.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// Bridge-assigned session identifier.
    pub session_id: SessionId,
    /// Human-readable agent kind label (e.g. `"claude-code"`, `"mock"`).
    pub agent_label: String,
    /// Current lifecycle status of the session.
    pub status: SessionStatus,
    /// Engine-supplied opaque key-value labels from `SessionConfig::bindings`.
    pub bindings: BTreeMap<String, String>,
}

/// Lifecycle status of a bridge session, returned in `SessionState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Handshake completed; session can accept messages.
    Open,
    /// Closed via `close_session()` (graceful).
    Closed,
    /// Subprocess exited unexpectedly.
    Crashed,
    /// Forced close due to `AcpBridge::shutdown()`.
    ForcedClosed,
}

/// User-visible message payload accepted by `AcpBridge::send_message`.
#[derive(Debug)]
pub enum MessageContent {
    /// Plain text message — the bridge wraps this in an ACP `ContentBlock::Text`.
    Text(String),
    /// Pre-constructed ACP content blocks (for structured or multi-part messages).
    Blocks(Vec<ContentBlock>),
}

/// Agent-flavor input to `SessionConfig`. The bridge derives the subprocess
/// invocation from this. `Mock` short-circuits to the test mock binary.
#[derive(Debug)]
pub enum AgentKind {
    /// Claude Code launched with `--acp` flag.
    ClaudeCode {
        /// Path to the `claude` binary.
        binary: PathBuf,
        /// Extra CLI flags appended after `--acp`.
        extra_args: Vec<String>,
    },
    /// OpenAI Codex CLI launched with `acp` subcommand.
    Codex {
        /// Path to the `codex` binary.
        binary: PathBuf,
        /// Extra CLI flags appended after `acp`.
        extra_args: Vec<String>,
    },
    /// Gemini CLI launched with `--acp` flag.
    GeminiCli {
        /// Path to the `gemini` binary.
        binary: PathBuf,
        /// Extra CLI flags appended after `--acp`.
        extra_args: Vec<String>,
    },
    /// Custom agent binary with fully explicit arguments.
    Custom {
        /// Path to the agent binary.
        binary: PathBuf,
        /// Arguments passed verbatim to the binary.
        args: Vec<String>,
    },
    /// Used by tests. Bridge launches `mock_acp_agent` from `CARGO_BIN_EXE_*`.
    Mock {
        /// Extra arguments forwarded to the mock binary.
        args: Vec<String>,
    },
}

impl AgentKind {
    /// Human-readable label that goes into `BridgeEvent::SessionEstablished::agent`.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::ClaudeCode { .. } => "claude-code",
            Self::Codex { .. } => "codex",
            Self::GeminiCli { .. } => "gemini-cli",
            Self::Custom { .. } => "custom",
            Self::Mock { .. } => "mock",
        }
    }
}

/// Open-session input. Constructed by the engine, passed to `AcpBridge::open_session`.
///
/// `SessionConfig` deliberately does **not** derive `Clone`: it carries
/// `Box<dyn Sandbox>`, which has no blanket `Clone` impl. When the bridge
/// needs to duplicate sandbox state (e.g. into the per-session `BridgeClient`),
/// it calls `Sandbox::boxed_clone()` and reconstructs the box. Callers that
/// want to hold a config across multiple opens must rebuild it from inputs.
pub struct SessionConfig {
    /// Agent flavor — drives subprocess invocation. The bridge resolves the
    /// binary path and CLI flags from this; for `Mock`, the bridge consults
    /// `CARGO_BIN_EXE_mock_acp_agent`.
    pub agent_kind: AgentKind,

    /// Working directory for the agent subprocess. Should be the per-run
    /// worktree path produced by `surge_git::create_run_worktree` (M2).
    /// The bridge does not validate this is a git worktree — that's M5's
    /// responsibility.
    pub working_dir: PathBuf,

    /// System prompt sent to the agent in the initial message frame.
    pub system_prompt: String,

    /// Outcome keys that the engine will accept from `report_stage_outcome`.
    /// The bridge derives the JSON-Schema enum from these and injects it as
    /// a tool. Empty `Vec` is rejected by `validate()` — agents need at
    /// least one outcome to terminate cleanly.
    pub declared_outcomes: Vec<OutcomeKey>,

    /// Whether to inject `request_human_input` tool (for stages that allow
    /// escalation). Drives the boolean check inside `tools::build_injected_tools`
    /// once Phase 4 lands.
    pub allows_escalation: bool,

    /// Engine-supplied list of tools (MCP-flavored or otherwise). The bridge
    /// passes this through the sandbox `visibility` filter before declaring
    /// tools to the agent.
    pub tools: Vec<ToolDef>,

    /// Sandbox to apply to the tool list and to per-call `ToolCall` events.
    /// Boxed because the trait is `dyn`-typed; cloned per-session via
    /// `Sandbox::boxed_clone`. The presence of this field is why
    /// `SessionConfig` itself does not derive `Clone`.
    pub sandbox: Box<dyn Sandbox>,

    /// Permission policy shared with the legacy `SurgeClient` (auto-approve,
    /// smart, …). In M3 the bridge uses this only for the `Client::request_permission`
    /// impl; the actual sandbox decisions go through `Sandbox::allows_tool`.
    pub permission_policy: crate::client::PermissionPolicy,

    /// Optional binding labels — opaque key-value pairs the engine attaches
    /// to the `SessionConfig` for later correlation in `BridgeEvent`s
    /// (e.g. the node_key, the run_id). The bridge passes these through to
    /// `BridgeEvent::SessionEstablished` and treats them as opaque otherwise.
    /// Capped by `validate()` at 8 entries × 64 bytes each to bound payload size.
    pub bindings: BTreeMap<String, String>,
}

impl SessionConfig {
    /// Validate the config before subprocess spawn. Returns the same error
    /// types as `OpenSessionError` so the bridge can `?`-propagate.
    pub fn validate(&self) -> Result<(), super::error::OpenSessionError> {
        if self.declared_outcomes.is_empty() {
            return Err(super::error::OpenSessionError::NoDeclaredOutcomes);
        }
        // Cap bindings (per spec §4.3): 8 entries × 64 bytes each.
        if self.bindings.len() > 8 {
            return Err(super::error::OpenSessionError::InvalidBindings(format!(
                "bindings has {} entries (max 8)",
                self.bindings.len()
            )));
        }
        for (k, v) in &self.bindings {
            if k.len() > 64 || v.len() > 64 {
                return Err(super::error::OpenSessionError::InvalidBindings(format!(
                    "binding {k}=... exceeds 64-byte limit"
                )));
            }
        }
        // Tool name uniqueness — the engine-injected `report_stage_outcome` and
        // optionally `request_human_input` are added in `tools::build_injected_tools`,
        // not by the caller, so we only check the caller-supplied list here.
        let mut seen = std::collections::HashSet::with_capacity(self.tools.len());
        for t in &self.tools {
            if !seen.insert(t.name.as_str()) {
                return Err(super::error::OpenSessionError::InvalidToolDefs(format!(
                    "duplicate tool name: {}",
                    t.name
                )));
            }
            if t.name == "report_stage_outcome" || t.name == "request_human_input" {
                return Err(super::error::OpenSessionError::InvalidToolDefs(format!(
                    "caller may not supply reserved tool name '{}'",
                    t.name
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::sandbox::AlwaysAllowSandbox;
    use crate::client::PermissionPolicy;
    use std::str::FromStr;

    fn cfg_with(outcomes: Vec<&str>, tools: Vec<ToolDef>) -> SessionConfig {
        SessionConfig {
            agent_kind: AgentKind::Mock { args: vec![] },
            working_dir: PathBuf::from("/tmp/wt"),
            system_prompt: "sys".into(),
            declared_outcomes: outcomes
                .into_iter()
                .map(|o| OutcomeKey::from_str(o).unwrap())
                .collect(),
            allows_escalation: false,
            tools,
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_empty_outcomes() {
        let cfg = cfg_with(vec![], vec![]);
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            super::super::error::OpenSessionError::NoDeclaredOutcomes
        ));
    }

    #[test]
    fn accepts_minimal_valid_config() {
        let cfg = cfg_with(vec!["done"], vec![]);
        cfg.validate().unwrap();
    }

    #[test]
    fn rejects_duplicate_tool_names() {
        let t = |n: &str| {
            ToolDef::new(
                n,
                "desc",
                super::super::tools::ToolCategory::Mcp("x".into()),
                serde_json::json!({}),
            )
        };
        let cfg = cfg_with(vec!["done"], vec![t("a"), t("a")]);
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            super::super::error::OpenSessionError::InvalidToolDefs(_)
        ));
    }

    #[test]
    fn rejects_reserved_tool_name() {
        let t = ToolDef::new(
            "report_stage_outcome",
            "desc",
            super::super::tools::ToolCategory::Mcp("x".into()),
            serde_json::json!({}),
        );
        let cfg = cfg_with(vec!["done"], vec![t]);
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            super::super::error::OpenSessionError::InvalidToolDefs(_)
        ));
    }

    #[test]
    fn rejects_oversized_bindings() {
        let mut cfg = cfg_with(vec!["done"], vec![]);
        for i in 0..9 {
            cfg.bindings.insert(format!("k{i}"), format!("v{i}"));
        }
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            super::super::error::OpenSessionError::InvalidBindings(_)
        ));
    }
}
