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
    pub session_id: SessionId,
    pub agent_label: String,
    pub status: SessionStatus,
    pub bindings: BTreeMap<String, String>,
}

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
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// Agent-flavor input to `SessionConfig`. The bridge derives the subprocess
/// invocation from this. `Mock` short-circuits to the test mock binary.
pub enum AgentKind {
    ClaudeCode { binary: PathBuf, extra_args: Vec<String> },
    Codex { binary: PathBuf, extra_args: Vec<String> },
    GeminiCli { binary: PathBuf, extra_args: Vec<String> },
    Custom { binary: PathBuf, args: Vec<String> },
    /// Used by tests. Bridge launches `mock_acp_agent` from `CARGO_BIN_EXE_*`.
    Mock { args: Vec<String> },
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
pub struct SessionConfig {
    pub agent_kind: AgentKind,
    pub working_dir: PathBuf,
    pub system_prompt: String,
    pub declared_outcomes: Vec<OutcomeKey>,
    pub allows_escalation: bool,
    pub tools: Vec<ToolDef>,
    pub sandbox: Box<dyn Sandbox>,
    pub permission_policy: crate::client::PermissionPolicy,
    pub bindings: BTreeMap<String, String>,
}

impl SessionConfig {
    /// Validate the config before subprocess spawn. Returns the same error
    /// types as `OpenSessionError` so the bridge can `?`-propagate.
    pub fn validate(&self) -> Result<(), super::error::OpenSessionError> {
        if self.declared_outcomes.is_empty() {
            return Err(super::error::OpenSessionError::NoDeclaredOutcomes);
        }
        // Cap bindings (per spec §4.3): 8 entries × 64 chars each.
        if self.bindings.len() > 8 {
            return Err(super::error::OpenSessionError::InvalidToolDefs(
                format!("bindings has {} entries (max 8)", self.bindings.len()),
            ));
        }
        for (k, v) in &self.bindings {
            if k.len() > 64 || v.len() > 64 {
                return Err(super::error::OpenSessionError::InvalidToolDefs(
                    format!("binding {k}=... exceeds 64-char limit"),
                ));
            }
        }
        // Tool name uniqueness — the engine-injected `report_stage_outcome` and
        // optionally `request_human_input` are added in `tools::build_injected_tools`,
        // not by the caller, so we only check the caller-supplied list here.
        let mut seen = std::collections::HashSet::with_capacity(self.tools.len());
        for t in &self.tools {
            if !seen.insert(t.name.as_str()) {
                return Err(super::error::OpenSessionError::InvalidToolDefs(
                    format!("duplicate tool name: {}", t.name),
                ));
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
        let t = |n: &str| ToolDef::new(n, "desc", super::super::tools::ToolCategory::Mcp("x".into()), serde_json::json!({}));
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
            super::super::error::OpenSessionError::InvalidToolDefs(_)
        ));
    }
}
