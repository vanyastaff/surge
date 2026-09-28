# ACP worker lifecycle repair

Approved bounded scope: surge-acp bridge lifecycle, mock ACP child and tests;
surge-orchestrator engine/stage/agent.rs and focused real-engine acceptance tests.
No new dependencies or paid-agent calls. Other engine stream/handle work belongs
to the daemon supervisor builder.

## Ownership and behavior

One bridge control loop owns the session map. Handshake/prompt/process tasks report
typed completions; no task mutates the map or holds its borrow across await.
Connections are shared on the LocalSet with Rc. Opening resources remain owned
before handshake completes. Stderr drains immediately. A single startup deadline
covers initialize and new_session; ordinary prompts have no new deadline.

At most one prompt is in flight per session, with a typed busy error for another.
Control messages and shutdown remain responsive during prompts and handshakes.
Admission must never wait inside the control loop. Caller cancellation of opening
must not orphan a child. send_message retains its completion/error semantics;
correct its formerly inaccurate enqueue-only documentation.

Protocol failures, timeout, cancellation and shutdown share owned cleanup. Keep
the child waiter alive through kill and actual wait/reap. Track SDK/helper tasks.
Drop signals shutdown and never blocks on thread join. Explicit shutdown only
reports success after verified cleanup and worker exit; unconfirmed cleanup is an
error, never a fabricated successful reap. Guarantees concern the direct child,
not an arbitrary wrapper's descendants.

The engine drives its prompt future concurrently with callbacks. Buffer candidate
OutcomeReported until prompt success, then perform existing outcome/task-ledger
writes. Auth/rate-limit/prompt errors retain their classifications. Deadlines
remain responsive under continuous notifications. Permission cancellation follows
the existing durable-safe path; do not cancel an in-progress persistence write.

## Test-first acceptance

1. Real Engine + controlled ACP child requests permission inside prompt: durable
   request, explicit decision, durable decision and completed run (outer RED).
2. Subprocess-watchdog bridge tests for initialize/new_session stall, noisy stderr,
   cancelled opening, shutdown/Drop during opening and child exit during prompt.
3. Public bridge permission roundtrip, same-session prompt busy, long prompt beyond
   handshake deadline, close/shutdown frozen child with verified process exit.
4. Reported outcome followed by prompt error never writes OutcomeReported or
   verified task state. Continuous notifications cannot starve stage deadline.
5. Focused RED/GREEN, affected tests, strict clippy and formatting. Shared Cargo
   coordinated with other builders; use CARGO_PROFILE_DEV_DEBUG=0,
   CARGO_PROFILE_TEST_DEBUG=0 and CARGO_INCREMENTAL=0.

Root performs independent spec review, then quality review before acceptance.

## Provider-capable follow-up discovered during lifecycle review

The lifecycle acceptance above proves process ownership, permission concurrency,
and engine ordering with a controlled ACP child. It does not prove that a supported
provider can receive Surge's stage contract or injected tools. Source inspection
found two independent production gaps in the current `agent-client-protocol 0.10.2`
bridge:

- `SessionConfig.system_prompt` is not used during the bridge handshake. The
  orchestrator does send the same rendered stage text as the first ACP user
  prompt, so instructions are not lost, but stable ACP v1 gives them no distinct
  system role. Tests and documentation must describe that actual wire contract.
- `ToolDef` values are filtered and reported in `SessionEstablished.tools_visible`
  but are not transmitted in `InitializeRequest` or `NewSessionRequest`.
  `SessionUpdate::ToolCall` is an agent-to-client observation, and
  `reply_to_tool` explicitly closes only Surge bookkeeping; it cannot return a
  tool result to the agent on the wire. Therefore the mock's emitted
  `report_stage_outcome`/`request_human_input` notifications are not evidence that
  a real provider can invoke those tools or consume a human answer.

Before calling agents product-ready, evaluate migration to the current official
ACP Rust SDK and use protocol-native mechanisms: stable elicitation for human
input where supported, and MCP attachment or a negotiated extension for
Surge-owned tools. Do not silently depend on draft protocol v2 or one provider's
private behavior. Start with a failing supported-provider or protocol-conformant
agent acceptance that observes the stage instructions, calls the advertised
outcome mechanism, receives an actual response where one is required, edits a
temporary worktree, and completes through the real Engine. Keep credentials out of
fixtures; a live paid-provider smoke is an additional opt-in gate, not the only
proof. Record exact stable-v1, experimental-feature, provider-support, MSRV, and
API-migration boundaries before changing the dependency.

## Implementation evidence and boundaries

The control loop lives in `bridge/lifecycle.rs`; it alone inserts/removes sessions.
Opening tasks own children through handshake cleanup; separate waiters own live
children through actual reap. Close tracks pending session IDs and returns an
explicit unconfirmed error for a second close while cleanup is pending.

Close attempts wire `session/cancel` for an active prompt and waits up to 500 ms
for the owned prompt RPC to settle before tearing down transport. SDK notification
enqueue is not receipt evidence. Then the existing five-second process grace and
kill/reap fallback apply. Ordinary prompts remain unbounded. SDK `Cancelled`
responses cannot become successful prompt results. Public cleanup timeouts never
claim a child was reaped; the worker retains cleanup ownership.

Observed RED evidence:

- Real Engine permission roundtrip: the old engine never persisted the request
  while awaiting prompt completion; a process watchdog bounded the resulting
  Drop hang (`/tmp/surge-acp-permission-red.log`, excerpt of session 56244).
- Captured ACP InitializeRequest advertised no filesystem read support despite
  an allowing policy (session 40339). The bridge now reuses the existing legacy
  capability mapper; no duplicated permission-policy mapping.
- Controlled child never received wire cancellation (session 53728). An initial
  enqueue-then-teardown attempt also failed (8198), proving why prompt completion
  must remain owned and observable through cancellation.

Focused GREEN: bridge permission/busy/long prompt, both handshake deadlines,
noisy stderr, child exit, concurrent close, capability policy, cooperative wire
cancel, ignored-cancel forced reap, cancelled opening, bare Drop with PID barrier;
Engine approval, Stop during approval, outcome followed by prompt error, failed
and successful sealed verification, and deadline under continuous notifications.
The streaming fixture proves durable escalation within three seconds separately
from the allowed cleanup grace. All tests use local controlled children.

Not established by this slice: supported-provider application E2E, injected tool
transport, or profile system-prompt delivery. Current ACP worker does not transmit
its filtered ToolDef catalog or SessionConfig.system_prompt, and reply_to_tool is
local bookkeeping. Generic injected human-input waits still use the prior timeout
path. Cancellation while the engine is opening a session remains bounded by the
bridge handshake deadline rather than immediate run-token propagation. These
remain explicit subsequent integration work, not claims covered by mock GREEN.
