# Elevation Runbook

When an ACP agent needs a capability the configured sandbox does not grant, it issues an ACP `request_permission` call mid-turn. Surge intercepts that call, surfaces it to the operator through the run's event log, awaits a decision (or times out), and replies back to the agent. This document covers the lifecycle, configuration, and audit trail.

## Lifecycle

```text
                              ┌──────────────────────────────────┐
                              │  agent (inside ACP session)      │
                              └──────────────────────────────────┘
                                              │
                                  request_permission(tool_call)
                                              ▼
┌──────────────────────────────────────────────────────────────────┐
│  surge-acp bridge — `BridgeClient::request_permission`           │
│    1. generates `request_id` (ULID)                              │
│    2. parks `oneshot::Sender<RequestPermissionResponse>`         │
│       in `SessionStateInner::pending_permissions[request_id]`    │
│    3. broadcasts `BridgeEvent::PermissionRequested`              │
│    4. awaits the parked receiver                                 │
└──────────────────────────────────────────────────────────────────┘
                                              │
                                              ▼
┌──────────────────────────────────────────────────────────────────┐
│  surge-orchestrator — agent stage event loop                     │
│    1. observes `PermissionRequested`                             │
│    2. appends `SandboxElevationRequested` to the run event log   │
│    3. registers `PendingElevation` in `PendingElevations`        │
│    4. `tokio::select!` over:                                     │
│       - the parked receiver                                      │
│       - `tokio::time::sleep(elevation_timeout)`                  │
└──────────────────────────────────────────────────────────────────┘
                                              │
                ┌─────────────────────────────┼─────────────────────────────┐
                ▼                             ▼                             ▼
        operator decision                 timeout                    session ended
                │                             │                             │
                ▼                             ▼                             ▼
   `Engine::resolve_elevation`     append `SandboxElevationTimedOut`   sender drops →
   fires the registered            then `SandboxElevationDecided`     rx returns Err
   oneshot with `Allow`,           with `decision: Deny`              → reply Cancelled
   `AllowAndRemember`, or          → reply Cancelled
   `Deny`
                │
                ▼
   append `SandboxElevationDecided { decision, remember }`
                │
                ▼
   call `AcpBridge::reply_to_permission(request_id, response)`
                │
                ▼
   agent receives `RequestPermissionResponse` and continues / errors
```

## Event-log contract

Every elevation produces at least two events. The audit-trail tests in `crates/surge-orchestrator/tests/elevation_audit.rs` enforce these shapes.

| Event | When | Fields |
| --- | --- | --- |
| `SandboxElevationRequested` | Bridge translates the ACP request. | `node`, `capability` |
| `SandboxElevationDecided` | Operator decides, or timeout fires Deny. | `node`, `decision` (`allow` / `allow_and_remember` / `deny`), `remember` |
| `SandboxElevationTimedOut` | Timeout path only — paired with a `SandboxElevationDecided{Deny}`. | `node`, `capability`, `elapsed_seconds` |

**No tool arguments or prompt content** are persisted in these payloads. The audit row is structural; full request bodies live in the ACP `tool_call` payload which surge does not copy into the run event log.

## Configuration

`ApprovalConfig` (in `crates/surge-core/src/approvals.rs`) controls elevation behaviour:

```toml
[approvals]
elevation = true
elevation_channels = [
    { type = "telegram", chat_id_ref = "$DEFAULT" },
    { type = "desktop", duration = "transient" },
]
elevation_timeout = "1h"   # humantime-friendly; default 24h
```

| Field | Default | Effect |
| --- | --- | --- |
| `elevation` | `true` | Master switch — when `false`, the bridge's `Sandbox::allows_tool` decides immediately without parking a oneshot. |
| `elevation_channels` | `[]` | Notification channels surge surfaces the pending elevation through. Reuses the same `ApprovalChannel` taxonomy as HumanGate. |
| `elevation_timeout` | `24h` | Wait window before surge appends `SandboxElevationTimedOut` and replies `Cancelled` to the agent. Humantime-friendly (`"1h"`, `"30m"`, `"15s"`). |

## Operator workflow

Surge appends `SandboxElevationRequested` to the event log; downstream consumers (Telegram bot, desktop card, REST surface) subscribe to the event log and render the operator UI from there. The decision flows back via `Engine::resolve_elevation`:

```rust
use surge_orchestrator::engine::elevation::EngineElevationDecision;
use surge_core::run_event::ElevationDecision;

engine.resolve_elevation(
    run_id,
    session_id,
    request_id,
    EngineElevationDecision {
        decision: ElevationDecision::Allow,
        remember: false,
        option_id: "allow-once".to_string(), // from ACP request.options[..]
    },
).await?;
```

The `option_id` must be one of the IDs the agent offered in `RequestPermissionRequest.options`; surge's bridge falls back to `"allow"` / `"deny"` if the operator's value doesn't match, but a faithful pick keeps the agent's allow-once vs allow-always semantics intact.

## Failure modes

| Symptom | Likely cause |
| --- | --- |
| `Unknown { request_id }` from `resolve_elevation` | The elevation already resolved (timeout, session end, or earlier `resolve` call). The agent has already received its response. |
| `ReceiverDropped { request_id }` | The agent stage was cancelled (run abort) before the operator decided. Surface as "run aborted before approval". |
| `SandboxResolveError::UnverifiedRuntime` at run start | The matrix has a declared-but-unverified row for `(runtime, mode)`. Use `surge doctor matrix` to see; either switch modes, switch runtimes, or run via `surge doctor agent <name>` to probe. |
| `SandboxResolveError::UnsupportedCombo` at run start | No row at all in the matrix. Add a row in `crates/surge-core/bundled/sandbox/matrix.toml` or pick a supported `(runtime, mode)` pair. |

## See also

- [Sandbox matrix](sandbox-matrix.md) — capability → flag mapping per runtime.
- [Architecture](ARCHITECTURE.md) §sandbox — overall delegation model.
- [ADR-0006](adr/0006-acp-only-transport.md) — ACP-only transport.
