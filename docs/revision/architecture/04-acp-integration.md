# Architecture 04 · ACP Integration

## Overview

ACP (Agent Client Protocol) is the standard interface for invoking AI coding agents. surge uses ACP to remain agent-agnostic — the same engine can drive Claude Code, Codex CLI, Gemini CLI, or any future ACP-compatible agent.

This document specifies the ACP bridge architecture, session lifecycle, tool injection mechanism, and the `report_stage_outcome` contract.

## Why agent-agnostic via ACP

Alternatives:
- **Direct Anthropic SDK calls**: locks surge to Anthropic. No way for users to use Codex or Gemini.
- **Custom protocol per agent**: explosion of integration code, brittle to upstream changes.
- **ACP**: industry standard (zed, sourcegraph, etc. participate). One integration, many agents.

The cost is some indirection, but it's the right architectural decision long-term.

## Bridge architecture

The ACP SDK (Rust crate) has `!Send` futures in places. Combined with our preference for async/Tokio multi-threaded runtime, this requires the **bridge pattern**: a dedicated OS thread running a single-threaded Tokio runtime with `LocalSet`, communicating with the rest of the engine via channels.

This pattern is what the author has used in `surge-acp` — we either reuse it or reimplement it in surge's `acp` crate.

```
┌─────────────────────────────────────────────────────┐
│ Main engine threads (multi-threaded Tokio runtime)  │
│  ↓ tokio::sync::mpsc                                 │
│                                                      │
│ ┌─────────────────────────────────────────────────┐ │
│ │ ACP Bridge thread                                │ │
│ │  - Single-threaded Tokio runtime                 │ │
│ │  - LocalSet for !Send ACP futures                │ │
│ │  - Owns all ACP sessions                         │ │
│ │  - Receives commands via channel                 │ │
│ │  - Sends events back via channel                 │ │
│ └─────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────┘
```

### Bridge interface

```rust
pub struct AcpBridge {
    cmd_tx: mpsc::Sender<BridgeCommand>,
    event_rx: Arc<Mutex<broadcast::Receiver<BridgeEvent>>>,
}

pub enum BridgeCommand {
    OpenSession { config: SessionConfig, reply: oneshot::Sender<Result<SessionId>> },
    SendMessage { session: SessionId, content: String, reply: oneshot::Sender<Result<()>> },
    CloseSession { session: SessionId, reply: oneshot::Sender<Result<()>> },
    GetSessionState { session: SessionId, reply: oneshot::Sender<Result<SessionState>> },
}

pub enum BridgeEvent {
    SessionEstablished { session: SessionId },
    AgentMessage { session: SessionId, content: String },
    ToolCall { session: SessionId, tool: String, args: Value, call_id: String },
    ToolResult { session: SessionId, call_id: String, result: ToolResult },
    PermissionRequest { session: SessionId, capability: String, reason: String },
    SessionEnded { session: SessionId, reason: SessionEndReason },
    Error { session: Option<SessionId>, error: String },
}

pub struct SessionConfig {
    pub launch: AgentLaunchConfig,
    pub model: String,
    pub system_prompt: String,
    pub tools: Vec<ToolDefinition>,
    pub sandbox: SandboxConfig,
    pub run_dir: PathBuf,
}

pub struct AgentLaunchConfig {
    pub provider: AgentProvider,
    pub mode: LaunchMode,
    pub config_profile: Option<String>,
    pub extra_args: Vec<String>,
}

pub enum LaunchMode {
    ProviderDefault,
    Local,
    Cloud,
    Sandbox,
}

impl AcpBridge {
    pub fn spawn() -> Result<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let (event_tx, _) = broadcast::channel(256);
        let event_tx_clone = event_tx.clone();
        
        let join = std::thread::Builder::new()
            .name("acp-bridge".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to build single-threaded runtime");
                
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, bridge_loop(cmd_rx, event_tx_clone));
            })?;
        
        Ok(Self {
            cmd_tx,
            event_rx: Arc::new(Mutex::new(event_tx.subscribe())),
        })
    }
    
    pub async fn open_session(&self, config: SessionConfig) -> Result<SessionId> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx.send(BridgeCommand::OpenSession { config, reply: reply_tx }).await?;
        reply_rx.await?
    }
    
    // ... other methods
}
```

### Bridge internal loop

```rust
async fn bridge_loop(
    mut cmd_rx: mpsc::Receiver<BridgeCommand>,
    event_tx: broadcast::Sender<BridgeEvent>,
) {
    let mut sessions: HashMap<SessionId, AcpSession> = HashMap::new();
    
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BridgeCommand::OpenSession { config, reply } => {
                let result = open_acp_session(&config).await;
                match result {
                    Ok(session) => {
                        let session_id = session.id();
                        sessions.insert(session_id.clone(), session);
                        spawn_session_observer(session_id.clone(), event_tx.clone());
                        let _ = reply.send(Ok(session_id));
                    }
                    Err(e) => { let _ = reply.send(Err(e)); }
                }
            }
            // ... other commands
        }
    }
}
```

## Session lifecycle

### Open

1. Engine constructs `SessionConfig` for the agent stage.
2. Engine calls `AcpBridge::open_session(config)`.
3. Bridge starts the provider using `config.launch`: local process, provider cloud mode, provider sandbox mode, or provider default.
4. Bridge negotiates ACP handshake (protocol version, capabilities).
5. Bridge declares **injected tools** including `report_stage_outcome` and provider-supported MCP servers.
6. Returns `SessionId` to engine.
7. Engine writes `SessionOpened` event.

### Send initial message

After open, engine sends the system prompt + initial user message:

```rust
let initial_message = format!(
    "{system_prompt}\n\n# Inputs\n{bindings_section}\n\n# Task\nProceed with your work. Call `report_stage_outcome` when done."
);
acp_bridge.send_message(session_id, initial_message).await?;
```

### Observe events

The bridge emits `BridgeEvent`s as the agent works:

- `AgentMessage` — text the agent produced (informational, logged but doesn't drive routing)
- `ToolCall` — agent invoked a tool exposed through ACP/MCP. Engine records the event and may run hooks when the provider exposes the call before execution.
- `ToolResult` — tool returned. Engine records as event
- `PermissionRequest` — provider-native request for sandbox/permission elevation, when supported

### Tool call flow

```rust
// Engine observes ToolCall event
match event {
    BridgeEvent::ToolCall { session, tool, args, call_id } => {
        // 1. Record ToolCalled event (with arg redaction)
        let redacted_args = redact_secrets(&args);
        engine.write_event(EventPayload::ToolCalled {
            session: session.clone(),
            tool: tool.clone(),
            args_redacted: redacted_args,
        }).await?;
        
        // 2. Run pre_tool_use hooks
        let hook_result = engine.run_hooks(HookTrigger::PreToolUse, &tool, &args).await?;
        if hook_result.rejected {
            // Send rejection back to agent
            acp_bridge.send_tool_result(session, call_id, ToolResult::Error("Hook rejected")).await?;
            return Ok(());
        }
        
        // 3. Special tool: report_stage_outcome
        if tool == "report_stage_outcome" {
            return engine.handle_outcome_report(args).await;
        }
        
        // 4. Forward to actual tool implementation (MCP server) if surge owns it.
        // Provider-native tools are executed by the provider runtime under its own sandbox.
        let result = engine.execute_tool(&tool, &args).await?;
        
        // 5. Record ToolResultReceived
        engine.write_event(EventPayload::ToolResultReceived {
            session: session.clone(),
            success: result.is_success(),
            result_hash: hash(&result),
        }).await?;
        
        // 7. Run post_tool_use hooks
        engine.run_hooks(HookTrigger::PostToolUse, &tool, &result).await?;
        
        // 8. Send result back to agent
        acp_bridge.send_tool_result(session, call_id, result).await?;
    }
    // ...
}
```

### Close

When the stage completes (outcome reported, hooks pass, edge routed):

1. Engine calls `AcpBridge::close_session(session_id)`.
2. Bridge sends ACP shutdown to subprocess.
3. Subprocess exits gracefully.
4. Bridge cleans up session state.
5. Engine writes `SessionClosed` event.

### Crash mid-session

If the agent process dies unexpectedly:
- Bridge detects via subprocess exit
- Emits `BridgeEvent::SessionEnded { reason: AgentCrashed }`
- Engine treats as `StageFailed`, retries per node policy

## Tool injection

surge injects two categories of tools into every ACP session:

### 1. Engine-provided tools

#### `report_stage_outcome`

The contract that lets the engine know the agent is done.

```json
{
  "name": "report_stage_outcome",
  "description": "Report your stage's outcome. Call this exactly once at the end.",
  "input_schema": {
    "type": "object",
    "required": ["outcome", "summary"],
    "properties": {
      "outcome": {
        "type": "string",
        "enum": ["done", "blocked", "escalate"],
        "description": "Which declared outcome best describes your result"
      },
      "summary": {
        "type": "string",
        "description": "1-3 sentences explaining what you did and why this outcome"
      },
      "artifacts_produced": {
        "type": "array",
        "items": { "type": "string" },
        "description": "List of file paths you created or modified"
      }
    }
  }
}
```

The `enum` values are dynamically populated from the node's `declared_outcomes` at session-open time.

#### `request_human_input`

Allows agent to escalate mid-stage if it hits genuine ambiguity.

```json
{
  "name": "request_human_input",
  "description": "Pause and ask the human for guidance. Use sparingly.",
  "input_schema": {
    "type": "object",
    "required": ["question"],
    "properties": {
      "question": { "type": "string" },
      "context": { "type": "string" }
    }
  }
}
```

When invoked, engine triggers a HumanGate-like flow (Telegram card with the question), waits for response, returns it as tool result.

### 2. Provider-aware MCP tools

The engine determines which MCP servers and tools should be offered based on node/profile tool config and provider capability metadata. Sandboxing remains provider-native; surge does not promise OS-level enforcement for tools executed by the provider runtime.

```rust
fn compute_available_tools(profile: &Profile, sandbox: &SandboxConfig) -> Vec<ToolDefinition> {
    let mut tools = Vec::new();
    
    // Always-injected
    tools.push(report_stage_outcome_tool(&profile.declared_outcomes));
    if profile.allows_escalation {
        tools.push(request_human_input_tool());
    }
    
    // MCP tools
    for mcp_id in &profile.tools.default_mcp {
        let mcp_tools = mcp_registry.tools_for(mcp_id)?;
        for tool in mcp_tools {
            if provider_capabilities.can_expose_tool_under_intent(&tool, mcp_id, sandbox) {
                tools.push(tool);
            }
        }
    }
    
    // Skill-based tools (if any)
    for skill_id in &profile.tools.default_skills {
        let skill = skills_registry.get(skill_id)?;
        tools.extend(skill.tools_for_provider_intent(sandbox));
    }
    
    tools
}
```

The agent should not see tools that are outside the requested intent when the provider/tool surface allows filtering. This is a usability and policy layer, not a replacement for provider-native sandboxing.

## ACP variants and launch modes

Different agents have slightly different ACP implementations. The bridge handles per-agent quirks:

```rust
pub enum AgentProvider {
    ClaudeCode,
    Codex,
    GeminiCli,
    Custom { binary: PathBuf, args: Vec<String> },
}

impl AgentProvider {
    fn invocation(&self, launch: &AgentLaunchConfig) -> Command {
        let mut cmd = match self {
            Self::ClaudeCode => Command::new("claude-code"),
            Self::Codex => Command::new("codex"),
            Self::GeminiCli => Command::new("gemini"),
            Self::Custom { binary, args } => {
                let mut cmd = Command::new(binary);
                cmd.args(args);
                return cmd;
            }
        };
        apply_provider_acp_args(&mut cmd, self);
        apply_provider_launch_mode(&mut cmd, self, launch.mode);
        cmd.args(&launch.extra_args);
        cmd
    }
}
```

Discovery: engine checks `PATH` and provider config for known agent launch modes, lists available ones at `surge doctor`. User configures agent services and role routing in `agents.yml`:

```yaml
version: 1

agents:
  claude-writer:
    provider: claude-code
    launch: { mode: local, profile: default }
    sandbox: { mode: workspace-write }

  codex-review:
    provider: codex
    launch: { mode: sandbox, profile: strict-review }
    sandbox: { mode: read-only }

roles:
  implementer: claude-writer
  reviewer: codex-review

defaults:
  agent: claude-writer
```

The selected provider is resolved independently for every Agent node:

1. Node `agent = "name"` from `flow.toml`, resolved through project/global `agents.yml`.
2. Node `launch_override`, if present.
3. Role route in `agents.yml` based on the node profile's role ID.
4. Profile `[launch]` defaults.
5. Global default agent and launch mode.

This allows a flow like `Spec Author → Implementer(Claude local) → Reviewer(Codex sandbox) → Verifier(Gemini local) → PR Composer` without special multi-agent coordination logic.

## Streaming

ACP supports streaming agent responses (text token-by-token, tool calls as they form). The bridge propagates streams to the engine via `BridgeEvent::AgentMessage` events for partial responses.

The runtime UI subscribes to these events and renders live-updating logs in the bottom panel.

## Cost tracking

Each `BridgeEvent::AgentMessage` and `BridgeEvent::ToolCall` carries token usage metadata when the underlying agent reports it:

```rust
pub struct AgentMessageMeta {
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub cache_hits: u32,
    pub model: String,
}
```

Engine writes `TokensConsumed` events. Materialized view aggregates per-stage costs.

## Multiple concurrent sessions

The bridge supports multiple simultaneous sessions across runs. Each session is identified by its `SessionId`. The bridge's internal state is per-session.

For a single run, only one session is active at a time (one stage at a time). For multiple runs, multiple sessions coexist.

Resource limits: configurable max concurrent sessions (default: 4). Beyond this, new opens block until a session closes.

## Error handling

```rust
pub enum AcpError {
    AgentNotFound { provider: AgentProvider },
    LaunchModeUnsupported { provider: AgentProvider, mode: LaunchMode },
    HandshakeFailed { reason: String },
    SessionTimeout { duration: Duration },
    AgentCrashed { exit_code: Option<i32>, stderr: String },
    ToolDispatchFailed { tool: String, reason: String },
    ProtocolError { details: String },
}
```

Each maps to engine-level handling:
- `AgentNotFound` — fail run start, instruct user to install agent
- `LaunchModeUnsupported` — fail validation/start with a provider-specific message
- `HandshakeFailed` — fail stage, check if agent is supported version
- `SessionTimeout` — fail stage with timeout reason, allow retry
- `AgentCrashed` — fail stage, retry available
- `ToolDispatchFailed` — propagate to agent as tool error, agent decides
- `ProtocolError` — likely bug, fail run, log for diagnostics

## Acceptance criteria

The ACP integration is correctly implemented when:

1. The bridge can open and close sessions with Claude Code, Codex CLI, and Gemini CLI without errors.
2. `!Send` ACP futures execute correctly on the bridge thread without blocking the main runtime.
3. `report_stage_outcome` tool with dynamic enum values is properly recognized by all supported agents.
4. Sandbox filtering prevents disallowed tools from appearing in the agent's tool list.
5. Mid-session crash of agent subprocess is detected and reported to engine within 2 seconds.
6. Multiple concurrent sessions (across runs) operate independently without interference.
7. Streaming agent messages produce `BridgeEvent`s in real-time, observable by runtime UI.
8. Token usage is tracked accurately and accumulated into `TokensConsumed` events.
9. End-to-end: a full pipeline run with ACP-driven agents completes successfully against a real agent (Claude Code).
