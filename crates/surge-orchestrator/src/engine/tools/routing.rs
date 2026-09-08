//! `RoutingToolDispatcher` — fans out [`ToolDispatcher::dispatch`]
//! between the engine's built-in tools (e.g.,
//! [`crate::engine::tools::worktree::WorktreeToolDispatcher`]) and an
//! [`McpRegistry`]. Routing decisions are precomputed at
//! construction time from the merged tool catalog.

use crate::engine::tools::{
    DeclaredTool, LoopEscalation, McpEscalation, ToolCall, ToolDispatchContext, ToolDispatcher,
    ToolResultPayload,
};
use crate::guard::{LoopGuard, LoopGuardTrip, Verdict};
use crate::spill::spill_if_oversized;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use surge_core::id::RunId;
use surge_core::loop_config::ToolCallLoopGuardConfig;
use surge_core::spill_config::OutputSpillConfig;
use surge_mcp::{McpContent, McpError, McpRegistry, McpToolEntry};
use surge_persistence::artifacts::ArtifactStore;

/// Surge-injected tool names that are added separately by the ACP
/// bridge (not via any `ToolDispatcher::declared_tools`) and dispatched
/// upstream of this router. They are reserved: an MCP server must never
/// shadow them in the agent's tool catalog or routing table. This is
/// the **single canonical arbitration site** for injected-tool name
/// precedence — see ADR-0006 (uniform injected-tool surface) and
/// ADR-0014. The regression test asserts no second list exists.
pub(crate) const RESERVED_INJECTED_TOOLS: [&str; 2] =
    ["report_stage_outcome", "request_human_input"];

/// One row in the routing table — where a given tool name lives.
#[derive(Clone, Debug)]
enum ToolOrigin {
    /// Engine-built-in tool — delegate to `engine_dispatcher`.
    Engine,
    /// MCP server tool — call via `mcp_registry`.
    Mcp { server: String, timeout: Duration },
}

/// Restart-exhaustion escalation accumulator. `pending` is drained by
/// the agent stage into `EscalationRequested` events; `seen` records
/// every server already escalated so a single permanent outage emits
/// exactly one card instead of one per subsequent tool call — the
/// give-up fact is stable (see `engine::stage::agent`).
#[derive(Default)]
struct EscalationState {
    pending: Vec<McpEscalation>,
    seen: HashSet<String>,
}

/// Per-dispatcher loop-guard state: the guard itself, whether it is
/// *currently* escalating, and escalations pending drain by the agent
/// stage. `currently_escalated` makes recording edge-triggered — a guard
/// that trips, then recovers (a different tool call breaks the repeat
/// streak), then trips again later produces two escalations, not an
/// unbounded stream of one per subsequent call while still tripped.
struct LoopGuardState {
    guard: LoopGuard,
    currently_escalated: bool,
    pending: Vec<LoopEscalation>,
}

impl LoopGuardState {
    fn new(config: ToolCallLoopGuardConfig) -> Self {
        Self {
            guard: LoopGuard::new(config),
            currently_escalated: false,
            pending: Vec::new(),
        }
    }

    /// Record a verdict, edge-triggering `pending` exactly once per trip.
    /// Shared by both entry points that can observe a verdict: a tool call
    /// arriving (`check_loop_guard`) and the timer-driven wall-clock poll
    /// (`poll_wall_clock_deadline`) — a node that never calls a tool must
    /// still trip its budget.
    fn record(&mut self, verdict: &Verdict) {
        match verdict {
            Verdict::Escalate(trip) => {
                if !self.currently_escalated {
                    self.currently_escalated = true;
                    self.pending.push(LoopEscalation { trip: trip.clone() });
                }
            },
            Verdict::Continue => self.currently_escalated = false,
        }
    }
}

/// `ToolDispatcher` impl that routes between engine + MCP. Constructed
/// once per engine session (per agent stage), with the routing table
/// precomputed from the merged catalog.
pub struct RoutingToolDispatcher {
    engine_dispatcher: Arc<dyn ToolDispatcher>,
    mcp_registry: Arc<McpRegistry>,
    routing_table: HashMap<String, ToolOrigin>,
    declared: Vec<DeclaredTool>,
    /// MCP restart-exhaustion escalations observed during dispatch,
    /// drained by the agent stage into `EscalationRequested` events.
    /// De-duplicated per server so one outage cannot spam the
    /// operator surface.
    escalations: Mutex<EscalationState>,
    /// Repeat-tool-call / node-wall-clock guard
    /// (`.autopilot/competitive-waves/spec.md` §15). Defaults to
    /// `ToolCallLoopGuardConfig::default()`; override with
    /// [`Self::with_tool_call_loop_guard_config`] once a construction site
    /// has the run's actual `surge.toml` value.
    loop_guard: Mutex<LoopGuardState>,
    /// Output-spill cap in bytes (§16). Defaults to
    /// `OutputSpillConfig::default()`; override with
    /// [`Self::with_output_spill_config`].
    output_spill_cap: usize,
    /// Existing content-addressed artifact store spilled output is written
    /// to. `None` by default — construction never guesses at
    /// `ArtifactStore::from_default_path()` (`~/.surge/runs`), because a
    /// caller that forgets [`Self::with_artifact_store`] must degrade to
    /// "never spill" (see [`crate::spill::spill_if_oversized`]'s `None`
    /// arm), not silently write into a store that isn't the run's own.
    artifact_store: Option<ArtifactStore>,
}

impl RoutingToolDispatcher {
    /// Build with engine dispatcher + MCP registry + filtered list of
    /// MCP tools that should be exposed for the current session.
    /// Engine-built-in tools (from
    /// [`ToolDispatcher::declared_tools`]) are inserted with
    /// [`ToolOrigin::Engine`] and override any MCP entries with the
    /// same name (collision resolution: engine wins).
    #[must_use]
    pub fn new(
        engine_dispatcher: Arc<dyn ToolDispatcher>,
        mcp_registry: Arc<McpRegistry>,
        mcp_tools: &[McpToolEntry],
        per_server_timeouts: &HashMap<String, Duration>,
    ) -> Self {
        let mut table: HashMap<String, ToolOrigin> = HashMap::new();
        let mut declared: Vec<DeclaredTool> = Vec::new();

        // Sort MCP entries by (server, tool) for deterministic
        // first-wins collision resolution across MCP servers.
        let mut sorted_mcp: Vec<&McpToolEntry> = mcp_tools.iter().collect();
        sorted_mcp.sort_by(|a, b| a.server.cmp(&b.server).then_with(|| a.tool.cmp(&b.tool)));

        for entry in sorted_mcp {
            if table.contains_key(&entry.tool) {
                // Collision: another (sorted-earlier) MCP server already
                // claimed this tool name. Drop this entry and warn so
                // operators can rename or namespace if needed.
                tracing::warn!(
                    server = %entry.server,
                    tool = %entry.tool,
                    "MCP tool name collision; first-wins (by sorted server name) — this entry skipped"
                );
                continue;
            }
            let timeout = per_server_timeouts
                .get(&entry.server)
                .copied()
                .unwrap_or(Duration::from_secs(60));
            table.insert(
                entry.tool.clone(),
                ToolOrigin::Mcp {
                    server: entry.server.clone(),
                    timeout,
                },
            );
            declared.push(DeclaredTool {
                name: entry.tool.clone(),
                description: entry.description.clone(),
                input_schema: entry.input_schema.clone(),
            });
        }

        // Engine tools overwrite MCP collisions (engine wins).
        let engine_tools = engine_dispatcher.declared_tools();
        for et in &engine_tools {
            table.insert(et.name.clone(), ToolOrigin::Engine);
        }
        // Replace any duplicate-named entries in `declared` with the
        // engine's version (description / schema take precedence).
        let engine_names: std::collections::HashSet<&str> =
            engine_tools.iter().map(|t| t.name.as_str()).collect();
        declared.retain(|d| !engine_names.contains(d.name.as_str()));
        declared.extend(engine_tools);

        // Reserved-name guard (single canonical arbitration site):
        // surge-injected tools are dispatched by the ACP bridge before
        // this router ever sees them, so an MCP tool of the same name
        // would shadow the catalog while being unreachable. Drop any
        // such MCP entry from the routing table and the declared
        // catalog, warning once per collision (ADR-0006 / ADR-0014).
        for reserved in RESERVED_INJECTED_TOOLS {
            if let Some(ToolOrigin::Mcp { server, .. }) = table.get(reserved) {
                tracing::warn!(
                    target: "mcp::supervisor",
                    tool = reserved,
                    server = %server,
                    "MCP server advertises a surge-injected tool name; \
                     dropping it — injected tools win (ADR-0006)"
                );
                table.remove(reserved);
            }
            declared.retain(|d| d.name != reserved);
        }

        Self {
            engine_dispatcher,
            mcp_registry,
            routing_table: table,
            declared,
            escalations: Mutex::new(EscalationState::default()),
            loop_guard: Mutex::new(LoopGuardState::new(ToolCallLoopGuardConfig::default())),
            output_spill_cap: OutputSpillConfig::default().max_output_bytes,
            // `None`, not `ArtifactStore::from_default_path().ok()`: the
            // production path always calls `with_artifact_store` with the
            // run's own store, so this default is only ever observed by a
            // caller that forgot to — and that caller must get "never
            // spill", not a silent write into `~/.surge/runs` (a store
            // nothing that reads the run's artifacts back would ever look
            // at).
            artifact_store: None,
        }
    }

    /// Override the loop-guard thresholds. Defaults to
    /// `ToolCallLoopGuardConfig::default()` otherwise — call this once a
    /// construction site threads through the run's actual `surge.toml`
    /// `tool_call_loop_guard` value.
    #[must_use]
    pub fn with_tool_call_loop_guard_config(self, config: ToolCallLoopGuardConfig) -> Self {
        Self {
            loop_guard: Mutex::new(LoopGuardState::new(config)),
            ..self
        }
    }

    /// Override the output-spill cap. Defaults to
    /// `OutputSpillConfig::default()` otherwise — call this once a
    /// construction site threads through the run's actual `surge.toml`
    /// `output_spill` value.
    #[must_use]
    pub fn with_output_spill_config(mut self, config: OutputSpillConfig) -> Self {
        self.output_spill_cap = config.max_output_bytes;
        self
    }

    /// Override the artifact store spilled output is written to. Defaults
    /// to `ArtifactStore::from_default_path()` (or "never spill", never
    /// "lose the output", if that path cannot be resolved) otherwise. Also
    /// the seam a test uses to point spill at a temporary directory instead
    /// of the real `~/.surge/runs`.
    #[must_use]
    pub fn with_artifact_store(mut self, store: ArtifactStore) -> Self {
        self.artifact_store = Some(store);
        self
    }

    /// Check the loop guard for `call` without dispatching it. `Some` means
    /// the caller must not dispatch — either the node's wall-clock budget
    /// or its repeat-call threshold has been exceeded. Recording into
    /// `pending` is edge-triggered (see [`LoopGuardState`]).
    fn check_loop_guard(&self, call: &ToolCall) -> Option<LoopGuardTrip> {
        let Ok(mut state) = self.loop_guard.lock() else {
            // Poisoned lock: the guard is best-effort safety, not
            // load-bearing control flow — fail open rather than wedge
            // every subsequent dispatch.
            return None;
        };
        let verdict = match state.guard.deadline() {
            escalated @ Verdict::Escalate(_) => escalated,
            Verdict::Continue => state.guard.observe(call),
        };
        state.record(&verdict);
        match verdict {
            Verdict::Escalate(trip) => Some(trip),
            Verdict::Continue => None,
        }
    }

    /// Poll this dispatcher's node wall-clock deadline independent of any
    /// tool call. `check_loop_guard` above only polls the deadline when a
    /// tool call arrives — a node stuck in one long agent turn (streaming
    /// output, no tool calls at all) never reaches it, so its budget would
    /// never trip (`.autopilot/competitive-waves/spec.md` §15: the wall-clock
    /// half of the guard is meant to catch exactly that case). The agent
    /// stage calls this on a timer, alongside `check_loop_guard`, so both
    /// paths feed the same edge-triggered `pending` queue.
    fn poll_wall_clock_deadline_inner(&self) {
        let Ok(mut state) = self.loop_guard.lock() else {
            return;
        };
        let verdict = state.guard.deadline();
        state.record(&verdict);
    }

    /// Spill `result`'s content to the artifact store if it exceeds the
    /// configured cap. Anything other than `ToolResultPayload::Ok` passes
    /// through unchanged — there is nothing to spill.
    async fn apply_output_spill(
        &self,
        run_id: RunId,
        call: &ToolCall,
        result: ToolResultPayload,
    ) -> ToolResultPayload {
        let ToolResultPayload::Ok { content } = result else {
            return result;
        };
        let artifact_name = format!("tool-output/{}/{}", call.tool, call.call_id);
        let output = spill_if_oversized(
            run_id,
            &artifact_name,
            content,
            self.output_spill_cap,
            self.artifact_store.as_ref(),
        )
        .await;
        ToolResultPayload::Ok {
            content: output.into_content(),
        }
    }

    /// Record a restart-exhaustion escalation, de-duplicated per
    /// server: the first exhaustion of `server` is queued for the
    /// stage to escalate; later ones (every subsequent tool call hits
    /// the already-`Exhausted` connection) are dropped so one outage
    /// cannot spam the AFK/operator surface. Lock poisoning is
    /// swallowed — an escalation is best-effort telemetry, not
    /// load-bearing control flow.
    fn record_escalation(&self, server: &str, attempts: u32) {
        // Hot path (server already exhausted, hit by every later tool
        // call) does a borrow-keyed lookup with no allocation; only a
        // newly-seen server allocates.
        if let Ok(mut st) = self.escalations.lock()
            && !st.seen.contains(server)
        {
            st.seen.insert(server.to_owned());
            st.pending.push(McpEscalation {
                server: server.to_owned(),
                attempts,
            });
        }
    }
}

#[async_trait]
impl ToolDispatcher for RoutingToolDispatcher {
    async fn dispatch(&self, ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload {
        // Loop guard first: a tripped guard must stop this call from
        // reaching the engine/MCP executor at all — that's what "escalate
        // instead of burning budget" means (§15). Checked before routing so
        // neither origin gets a chance to spend anything on this call.
        if let Some(trip) = self.check_loop_guard(call) {
            return ToolResultPayload::Error {
                message: trip.operator_message(),
            };
        }

        let result = match self.routing_table.get(&call.tool) {
            Some(ToolOrigin::Engine) => self.engine_dispatcher.dispatch(ctx, call).await,
            Some(ToolOrigin::Mcp { server, timeout }) => {
                match self
                    .mcp_registry
                    .call_tool(server, &call.tool, call.arguments.clone(), *timeout)
                    .await
                {
                    Ok(r) if !r.is_error => ToolResultPayload::Ok {
                        content: serde_json::Value::Array(
                            r.content.into_iter().map(content_to_json).collect(),
                        ),
                    },
                    Ok(r) => ToolResultPayload::Error {
                        message: r
                            .content
                            .into_iter()
                            .map(content_to_string)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    },
                    Err(e) => {
                        // Type-safe escalation capture (no string
                        // sniffing): a restart-exhausted server is a
                        // permanent failure the AFK operator must see.
                        // De-duplicated per server inside
                        // `record_escalation`.
                        if let McpError::RestartExhausted { server, attempts } = &e {
                            self.record_escalation(server, *attempts);
                        }
                        ToolResultPayload::Error {
                            message: format!("MCP error: {e}"),
                        }
                    },
                }
            },
            None => {
                // Name the declared catalog, not just the missing tool —
                // `WorktreeToolDispatcher`'s own "not implemented" message
                // used to carry this hint before every stage routed
                // through here; losing it makes a misdeclared dispatcher
                // (see `ToolDispatcher::declared_tools`'s contract note)
                // harder to diagnose than it needs to be.
                let mut known: Vec<&str> = self.declared.iter().map(|t| t.name.as_str()).collect();
                known.sort_unstable();
                ToolResultPayload::Unsupported {
                    message: format!(
                        "unknown tool: {} (declared tools: {})",
                        call.tool,
                        known.join(", ")
                    ),
                }
            },
        };

        // Output spill last: only a successful result has content worth
        // bounding, and both origins above return through this one path.
        self.apply_output_spill(ctx.run_id, call, result).await
    }

    fn declared_tools(&self) -> Vec<DeclaredTool> {
        self.declared.clone()
    }

    fn drain_mcp_escalations(&self) -> Vec<McpEscalation> {
        self.escalations
            .lock()
            .map(|mut st| std::mem::take(&mut st.pending))
            .unwrap_or_default()
    }

    fn drain_loop_escalations(&self) -> Vec<LoopEscalation> {
        self.loop_guard
            .lock()
            .map(|mut st| std::mem::take(&mut st.pending))
            .unwrap_or_default()
    }

    fn poll_wall_clock_deadline(&self) {
        self.poll_wall_clock_deadline_inner();
    }

    fn resolved_origin(&self, tool: &str) -> Option<String> {
        match self.routing_table.get(tool) {
            Some(ToolOrigin::Mcp { server, .. }) => Some(server.clone()),
            // Engine-built-in or unknown → no MCP attribution.
            _ => None,
        }
    }
}

fn content_to_json(c: McpContent) -> serde_json::Value {
    match c {
        McpContent::Text(s) => serde_json::json!({ "type": "text", "text": s }),
        McpContent::Other { kind, summary } => serde_json::json!({
            "type": kind,
            "summary": summary,
        }),
        // `McpContent` is `#[non_exhaustive]`; catch any future variants with
        // a debug representation so callers always get a valid JSON value.
        _ => serde_json::json!({ "type": "unknown" }),
    }
}

fn content_to_string(c: McpContent) -> String {
    match c {
        McpContent::Text(s) => s,
        McpContent::Other { kind, summary } => format!("[{kind}] {summary}"),
        // `McpContent` is `#[non_exhaustive]`; forward-compatible fallback.
        _ => String::from("[unknown content]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tools::{ToolCall, ToolDispatcher, ToolResultPayload};

    /// Single-arbitration-site guard: the reserved injected-tool list
    /// is defined exactly once, here. If this breaks, a second list was
    /// introduced or a name changed without updating the contract —
    /// re-derive against the ACP bridge's injected tools (ADR-0006).
    #[test]
    fn reserved_injected_tools_is_the_canonical_pair() {
        assert_eq!(
            RESERVED_INJECTED_TOOLS,
            ["report_stage_outcome", "request_human_input"],
            "reserved injected-tool contract changed; this const is the \
             single source of truth — verify against the ACP bridge"
        );
    }

    /// Stub engine dispatcher that returns a marker payload echoing the
    /// requested tool name and declares one tool (`"shell_exec"`) so we
    /// can verify collision resolution.
    struct EngineStub;

    #[async_trait]
    impl ToolDispatcher for EngineStub {
        async fn dispatch(
            &self,
            _ctx: &ToolDispatchContext<'_>,
            call: &ToolCall,
        ) -> ToolResultPayload {
            ToolResultPayload::Ok {
                content: serde_json::json!({"engine_handled": call.tool}),
            }
        }
        fn declared_tools(&self) -> Vec<DeclaredTool> {
            vec![DeclaredTool {
                name: "shell_exec".into(),
                description: Some("engine version".into()),
                input_schema: serde_json::json!({}),
            }]
        }
    }

    #[tokio::test]
    async fn engine_tool_wins_collision() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        let mcp_tools = vec![McpToolEntry::new(
            "fake".into(),
            "shell_exec".into(), // colliding name
            Some("from MCP".into()),
            serde_json::json!({}),
        )];
        let r = RoutingToolDispatcher::new(Arc::new(EngineStub), mcp, &mcp_tools, &HashMap::new());
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &surge_core::run_state::RunMemory::default(),
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({}),
        };
        let result = r.dispatch(&ctx, &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                assert_eq!(content["engine_handled"], "shell_exec");
            },
            other => panic!("expected engine route, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mcp_mcp_collision_first_wins_by_server() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        // Two servers that both expose "shared". "a_server" sorts before
        // "z_server", so "a_server"'s entry should win.
        let mcp_tools = vec![
            McpToolEntry::new(
                "z_server".into(),
                "shared".into(),
                Some("from z".into()),
                serde_json::json!({}),
            ),
            McpToolEntry::new(
                "a_server".into(),
                "shared".into(),
                Some("from a".into()),
                serde_json::json!({}),
            ),
        ];
        let r = RoutingToolDispatcher::new(Arc::new(EngineStub), mcp, &mcp_tools, &HashMap::new());
        let declared = r.declared_tools();
        // Exactly one entry for "shared" — first by sorted server name (a_server).
        assert_eq!(declared.iter().filter(|t| t.name == "shared").count(), 1);
        assert_eq!(
            declared
                .iter()
                .find(|t| t.name == "shared")
                .unwrap()
                .description
                .as_deref(),
            Some("from a")
        );
    }

    #[test]
    fn escalations_dedupe_per_server() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        let r = RoutingToolDispatcher::new(Arc::new(EngineStub), mcp, &[], &HashMap::new());
        // A permanently-exhausted server is hit by every later tool
        // call — only the first must escalate.
        r.record_escalation("flaky", 5);
        r.record_escalation("flaky", 5);
        r.record_escalation("other", 5);
        let drained = r.drain_mcp_escalations();
        assert_eq!(
            drained.len(),
            2,
            "one escalation per server, got {drained:?}"
        );
        // A post-drain repeat of an already-escalated server stays
        // suppressed: the give-up fact is recorded once per run.
        r.record_escalation("flaky", 5);
        assert!(
            r.drain_mcp_escalations().is_empty(),
            "re-escalation after drain must stay suppressed"
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_unsupported() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        let r = RoutingToolDispatcher::new(Arc::new(EngineStub), mcp, &[], &HashMap::new());
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &surge_core::run_state::RunMemory::default(),
        };
        let call = ToolCall {
            call_id: "c2".into(),
            tool: "whatever".into(),
            arguments: serde_json::json!({}),
        };
        match r.dispatch(&ctx, &call).await {
            ToolResultPayload::Unsupported { .. } => {},
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    /// Engine-stub that counts how many times it was actually invoked —
    /// the stand-in for "budget burned": every call that reaches this stub
    /// is a call the loop guard failed to stop.
    #[derive(Default)]
    struct CountingEngineStub {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl ToolDispatcher for CountingEngineStub {
        async fn dispatch(
            &self,
            _ctx: &ToolDispatchContext<'_>,
            _call: &ToolCall,
        ) -> ToolResultPayload {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ToolResultPayload::Ok {
                content: serde_json::json!({ "ok": true }),
            }
        }
        fn declared_tools(&self) -> Vec<DeclaredTool> {
            vec![DeclaredTool {
                name: "shell_exec".into(),
                description: None,
                input_schema: serde_json::json!({}),
            }]
        }
    }

    /// This is the guard's RED/GREEN seam: with the guard wired (as it is
    /// here), a runaway repeat stops reaching the engine past the
    /// threshold. If the `check_loop_guard` call were ever removed from
    /// `dispatch`, `stub.calls` would keep incrementing on every iteration
    /// and this assertion would fail — the test cannot pass by construction
    /// alone, only by the guard actually intercepting the call.
    #[tokio::test]
    async fn repeated_identical_call_stops_reaching_the_engine() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        let stub = Arc::new(CountingEngineStub::default());
        let r = RoutingToolDispatcher::new(stub.clone(), mcp, &[], &HashMap::new())
            .with_tool_call_loop_guard_config(ToolCallLoopGuardConfig {
                max_repeat_tool_calls: 3,
                node_wall_clock_limit_secs: 3600,
            });
        let run_memory = surge_core::run_state::RunMemory::default();
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &run_memory,
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({ "cmd": "same" }),
        };

        // First 3 identical calls are within threshold and reach the stub.
        for _ in 0..3 {
            let result = r.dispatch(&ctx, &call).await;
            assert!(matches!(result, ToolResultPayload::Ok { .. }));
        }
        assert_eq!(stub.calls.load(std::sync::atomic::Ordering::SeqCst), 3);

        // The 4th (and every further) identical call is refused before
        // reaching the stub — budget stops being burned.
        for _ in 0..5 {
            let result = r.dispatch(&ctx, &call).await;
            assert!(
                matches!(result, ToolResultPayload::Error { .. }),
                "expected the guard to refuse the repeated call, got {result:?}"
            );
        }
        assert_eq!(
            stub.calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "engine must not be invoked once the guard has tripped"
        );
    }

    #[tokio::test]
    async fn repeated_call_escalation_is_drained_exactly_once() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        let stub = Arc::new(CountingEngineStub::default());
        let r = RoutingToolDispatcher::new(stub, mcp, &[], &HashMap::new())
            .with_tool_call_loop_guard_config(ToolCallLoopGuardConfig {
                max_repeat_tool_calls: 1,
                node_wall_clock_limit_secs: 3600,
            });
        let run_memory = surge_core::run_state::RunMemory::default();
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &run_memory,
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({ "cmd": "same" }),
        };

        for _ in 0..4 {
            r.dispatch(&ctx, &call).await;
        }

        let drained = r.drain_loop_escalations();
        assert_eq!(
            drained.len(),
            1,
            "one escalation per trip, not one per subsequent refused call"
        );
        match &drained[0].trip {
            LoopGuardTrip::RepeatedToolCall {
                tool, threshold, ..
            } => {
                assert_eq!(tool, "shell_exec");
                assert_eq!(*threshold, 1);
            },
            other @ LoopGuardTrip::NodeDeadlineExceeded { .. } => {
                panic!("expected RepeatedToolCall, got {other:?}")
            },
        }
        assert!(
            r.drain_loop_escalations().is_empty(),
            "a second drain with no new trip must be empty"
        );
    }

    #[tokio::test]
    async fn node_deadline_guard_blocks_before_any_dispatch() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        let stub = Arc::new(CountingEngineStub::default());
        let r = RoutingToolDispatcher::new(stub.clone(), mcp, &[], &HashMap::new())
            // A zero-second budget is already exceeded the instant the
            // dispatcher is built — deterministic, no sleeping in a test.
            .with_tool_call_loop_guard_config(ToolCallLoopGuardConfig {
                max_repeat_tool_calls: 100,
                node_wall_clock_limit_secs: 0,
            });
        let run_memory = surge_core::run_state::RunMemory::default();
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &run_memory,
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({ "cmd": "whatever" }),
        };

        let result = r.dispatch(&ctx, &call).await;

        assert!(matches!(result, ToolResultPayload::Error { .. }));
        assert_eq!(
            stub.calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a node already past its wall-clock budget must never reach the engine"
        );
        match &r.drain_loop_escalations()[0].trip {
            LoopGuardTrip::NodeDeadlineExceeded { .. } => {},
            other @ LoopGuardTrip::RepeatedToolCall { .. } => {
                panic!("expected NodeDeadlineExceeded, got {other:?}")
            },
        }
    }

    #[tokio::test]
    async fn oversized_output_is_spilled_to_the_existing_artifact_store() {
        struct BigOutputStub;
        #[async_trait]
        impl ToolDispatcher for BigOutputStub {
            async fn dispatch(
                &self,
                _ctx: &ToolDispatchContext<'_>,
                _call: &ToolCall,
            ) -> ToolResultPayload {
                ToolResultPayload::Ok {
                    content: serde_json::json!({ "text": "x".repeat(10_000) }),
                }
            }
            fn declared_tools(&self) -> Vec<DeclaredTool> {
                vec![DeclaredTool {
                    name: "read_file".into(),
                    description: None,
                    input_schema: serde_json::json!({}),
                }]
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path().join("runs"));
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        let r = RoutingToolDispatcher::new(Arc::new(BigOutputStub), mcp, &[], &HashMap::new())
            .with_output_spill_config(OutputSpillConfig {
                max_output_bytes: 256,
            })
            .with_artifact_store(store);
        let run_memory = surge_core::run_state::RunMemory::default();
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &run_memory,
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            arguments: serde_json::json!({ "path": "big.txt" }),
        };

        let result = r.dispatch(&ctx, &call).await;

        let ToolResultPayload::Ok { content } = result else {
            panic!("expected Ok, got {result:?}");
        };
        assert_eq!(content["spilled"], serde_json::json!(true));
        assert!(
            content["preview"].as_str().unwrap().len() < 10_000,
            "the node must see a bounded preview, not the full output"
        );
        assert!(
            content["locator"]
                .as_str()
                .unwrap()
                .starts_with("artifact://")
        );
    }
}
