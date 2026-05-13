//! Per-session mutable state held inside the worker (single-threaded LocalSet),
//! shared between `BridgeClient`, the session observer task, and the subprocess
//! waiter task via `Rc<RefCell<...>>`.

use std::collections::HashMap;

use agent_client_protocol::RequestPermissionResponse;
use tokio::sync::oneshot;

use crate::bridge::event::SessionEndReason;

/// Per-session mutable state. Accessed via `Rc<RefCell<...>>` because every
/// task touching it runs on the same LocalSet thread (no cross-thread sharing).
#[allow(dead_code)] // wired in Task 8.1 open_session_impl + Task 8.3 handle_session_notification
#[derive(Debug)]
pub(crate) struct SessionStateInner {
    /// ACP-side session string (from the agent's response to `session/new`).
    pub acp_session_id: String,

    /// Last cumulative token usage seen on a `SessionUpdate`. Flushed before
    /// `SessionEnded` (spec §5.7 ordering guarantee).
    pub last_token_usage: Option<TokenUsageSnapshot>,

    /// Whether `last_token_usage` has been broadcast since the last update.
    /// Used to skip duplicate emissions.
    pub last_token_usage_emitted: bool,

    /// Open tool calls keyed by call_id — used to correlate `tool/call` and
    /// `tool/result` from the agent.
    pub open_tool_calls: HashMap<String, OpenToolCall>,

    /// Outstanding `request_permission` calls awaiting an engine decision.
    /// Keyed by the bridge-generated `request_id`; the stored `oneshot` is
    /// fulfilled by `AcpBridge::reply_to_permission`, which the engine calls
    /// once the operator decides the elevation.
    pub pending_permissions: HashMap<String, oneshot::Sender<RequestPermissionResponse>>,

    /// Set when the session is in the closing path; observer/waiter tasks
    /// should drain quickly and exit.
    pub closing: bool,

    /// Set if a terminal event has been emitted; prevents double-emission
    /// from racing observer/waiter tasks.
    pub end_emitted: Option<SessionEndReason>,
}

impl SessionStateInner {
    /// Create a new `SessionStateInner` for the given ACP session ID.
    #[allow(dead_code)] // wired in Task 8.1 open_session_impl
    pub(crate) fn new(acp_session_id: String) -> Self {
        Self {
            acp_session_id,
            last_token_usage: None,
            // `false` matches the initial state where there is nothing to emit.
            // Phase 8.3 flush logic should be a no-op when `last_token_usage`
            // is `None` regardless, but `false` here avoids a subtle inversion
            // (a `true` initial value would suggest an emission has already
            // happened, which is the opposite of reality on a fresh session).
            last_token_usage_emitted: false,
            open_tool_calls: HashMap::new(),
            pending_permissions: HashMap::new(),
            closing: false,
            end_emitted: None,
        }
    }
}

/// Cumulative token usage snapshot read from the most recent ACP update.
#[allow(dead_code)] // wired in Task 8.3 token extraction
#[derive(Debug, Clone)]
pub(crate) struct TokenUsageSnapshot {
    /// Number of prompt (input) tokens consumed.
    pub prompt_tokens: u32,
    /// Number of output tokens generated.
    pub output_tokens: u32,
    /// Number of cache hit tokens (prompt tokens served from the prompt cache).
    pub cache_hits: u32,
    /// Model identifier the usage was reported against.
    pub model: String,
}

/// One pending tool call (engine-injected or otherwise) the agent has
/// initiated and the bridge is awaiting a result for.
#[allow(dead_code)] // wired in Task 8.3 tool dispatch
#[derive(Debug, Clone)]
pub(crate) struct OpenToolCall {
    /// Human-readable tool name from the agent's tool call.
    pub tool_name: String,
    /// MCP server id, if this call targets an MCP-backed tool.
    pub mcp_id: Option<String>,
    /// True when this tool call was injected by the bridge engine (e.g.
    /// `report_stage_outcome` or `request_human_input`).
    pub injected: bool,
}
