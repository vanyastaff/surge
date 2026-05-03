//! `Sandbox` trait + interim M3 impls. See spec §4.6 + §6.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Per-tool decision returned by both `Sandbox::visibility` and
/// `Sandbox::allows_tool`. `Elevate` keeps the tool visible to the agent
/// but routes per-call invocations to the engine's elevation flow (M5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxDecision {
    /// Tool is fully permitted.
    Allow,
    /// Tool is blocked; reason carries a human-readable explanation that
    /// the bridge surfaces in `BridgeEvent::ToolCall::sandbox_decision`.
    Deny { reason: String },
    /// Tool needs caller approval before execution. The bridge attaches
    /// the capability tag to `BridgeEvent::ToolCall::sandbox_decision`
    /// so M5 can route to a UI / Telegram elevation flow.
    ///
    /// Expected capability values (M3 contract; M5 may extend):
    /// - `"filesystem_write"` — agent wants to write outside the worktree
    /// - `"shell_exec"` — agent wants to execute a shell command
    /// - `"network"` — agent wants to make an outbound network request
    ///
    /// See RFC-0006 §Tier-2 for the full taxonomy.
    Elevate { capability: String },
}

/// Sandbox surface. Two-method split rationale: `WorkspaceWriteSandbox`
/// in M4 will return `visibility = Allow` for `write_text_file` (the tool
/// must be visible) but `allows_tool = Deny` for paths escaping the worktree.
/// Symmetric impls are valid (and used by `AlwaysAllowSandbox` / `DenyListSandbox`).
pub trait Sandbox: Send + Sync {
    /// Decide whether this tool appears in the agent's visible tool list.
    /// Called once per `ToolDef` at session-open time.
    fn visibility(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision;

    /// Decide whether this tool invocation is allowed. Called once per actual
    /// call from the agent. Bridge attaches the result to `BridgeEvent::ToolCall`.
    fn allows_tool(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision;

    /// Boxed clone — required for `SessionConfig: Clone`-equivalent passes
    /// because `dyn Trait` cannot derive `Clone` directly.
    fn boxed_clone(&self) -> Box<dyn Sandbox>;
}

/// Permits everything. The default for development and the mock agent.
#[derive(Clone, Debug, Default)]
pub struct AlwaysAllowSandbox;

impl Sandbox for AlwaysAllowSandbox {
    fn visibility(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn allows_tool(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

/// Allow-by-default with explicit denylists by tool name and by MCP server id.
///
/// `DenyListSandbox::default()` (empty denylists) is semantically identical to
/// `AlwaysAllowSandbox` and may be used interchangeably in tests that add entries
/// via `denied_tools.insert(...)`. Use `AlwaysAllowSandbox` directly when you never
/// intend to add denies.
///
/// Sufficient for RFC-0006 §Tier-1 enforcement and for the M3 integration
/// test in `tests/bridge_sandbox_filtering.rs`. M4 introduces richer
/// path-aware and OS-enforced impls additively.
#[derive(Clone, Debug, Default)]
pub struct DenyListSandbox {
    /// Tool names that should be hidden from the agent and rejected at
    /// invocation time. Matched as exact ASCII string equality on
    /// `ToolDef::name`.
    pub denied_tools: HashSet<String>,
    /// MCP server ids whose tools should be hidden in their entirety.
    /// Matched against `ToolCategory::Mcp(id)` only — non-MCP tools
    /// (`Builtin`, `Injected`) are never filtered by this set.
    pub denied_mcp_ids: HashSet<String>,
}

impl DenyListSandbox {
    /// Convenience constructor for tests.
    pub fn deny_tools<I, S>(tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            denied_tools: tools.into_iter().map(Into::into).collect(),
            denied_mcp_ids: HashSet::new(),
        }
    }

    fn decide(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision {
        if self.denied_tools.contains(tool) {
            return SandboxDecision::Deny { reason: format!("tool '{tool}' is denied") };
        }
        if let Some(id) = mcp_id {
            if self.denied_mcp_ids.contains(id) {
                return SandboxDecision::Deny { reason: format!("mcp server '{id}' is denied") };
            }
        }
        SandboxDecision::Allow
    }
}

impl Sandbox for DenyListSandbox {
    fn visibility(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision {
        self.decide(tool, mcp_id)
    }
    fn allows_tool(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision {
        // Symmetric with visibility per RFC-0006: a tool denied at visibility
        // would never reach allows_tool because it's filtered out of the agent's
        // tool list. Tested for parity below.
        self.decide(tool, mcp_id)
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_allow_visibility_and_call_both_allow() {
        let s = AlwaysAllowSandbox;
        assert_eq!(s.visibility("anything", None), SandboxDecision::Allow);
        assert_eq!(s.allows_tool("anything", Some("mcp-a")), SandboxDecision::Allow);
    }

    #[test]
    fn always_allow_boxed_clone_round_trip() {
        let s: Box<dyn Sandbox> = Box::new(AlwaysAllowSandbox);
        let cloned = s.boxed_clone();
        assert_eq!(cloned.visibility("x", None), SandboxDecision::Allow);
    }

    #[test]
    fn deny_list_denies_named_tool() {
        let s = DenyListSandbox::deny_tools(["shell_exec"]);
        match s.visibility("shell_exec", None) {
            SandboxDecision::Deny { reason } => assert!(reason.contains("shell_exec")),
            other => panic!("expected Deny, got {other:?}"),
        }
        assert_eq!(s.visibility("write_text_file", None), SandboxDecision::Allow);
    }

    #[test]
    fn deny_list_denies_named_mcp_server() {
        let mut s = DenyListSandbox::default();
        s.denied_mcp_ids.insert("dangerous-mcp".into());
        match s.allows_tool("read_file", Some("dangerous-mcp")) {
            SandboxDecision::Deny { reason } => assert!(reason.contains("dangerous-mcp")),
            other => panic!("expected Deny, got {other:?}"),
        }
        assert_eq!(s.allows_tool("read_file", Some("safe-mcp")), SandboxDecision::Allow);
        assert_eq!(s.allows_tool("read_file", None), SandboxDecision::Allow);
    }

    #[test]
    fn deny_list_visibility_and_allows_tool_parity() {
        let s = DenyListSandbox::deny_tools(["x"]);
        for (tool, mcp) in [("x", None), ("y", None), ("y", Some("a"))] {
            assert_eq!(s.visibility(tool, mcp), s.allows_tool(tool, mcp));
        }
    }
}
