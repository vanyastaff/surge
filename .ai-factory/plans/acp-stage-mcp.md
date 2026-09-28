# Protocol-real ACP stage tools

Status: **DESIGN ACCEPTED** for the split implementation. The current-wire MCP
slice precedes the independent Rust SDK 2.2.0 migration. This is on the daemon and
agent critical path requested by the user.

## Verified problem

Surge currently pins `agent-client-protocol 0.10.2` (schema 0.11.2). The bridge
builds and filters `ToolDef` values, but `open_session_impl` sends no MCP server in
`NewSessionRequest`; definitions appear only in the local
`SessionEstablished.tools_visible` event. `SessionUpdate::ToolCall` is
agent-to-client observability. `reply_to_tool` explicitly updates only Surge
bookkeeping and cannot deliver a result to the agent. Controlled mocks that emit
`report_stage_outcome` or `request_human_input` notifications therefore do not
prove a supported provider can discover, invoke, or receive a result from those
tools. Notifications must never carry authority merely because their title looks
like a Surge tool name.

The orchestrator does send its rendered stage instructions as the first ACP user
prompt. Stable wire v1 has no distinct system-role field, so documentation and
tests must describe that boundary accurately. Separately, the new bridge initially
advertised only terminal capability despite implementing filesystem methods; the
lifecycle slice reuses the legacy permission-policy capability mapping to repair
that issue.

## First implementation slice: stable stdio MCP

Expose Surge-owned stage tools through a real stdio MCP server declared in the ACP
session setup. Keep the engine and dispatcher in their existing owning crates; do
not create a second Engine. `surge-mcp` owns the narrow server transport/handler,
the orchestrator owns stage semantics and durable outcomes, and `surge-acp` owns
the session descriptor and provider capability boundary.

Prefer an internal CLI subcommand in the already released `surge` executable over
adding a third release binary. The agent launches the helper from the session's MCP
descriptor. The helper connects to a per-session local socket/named pipe and serves
the exact sandbox-filtered catalog. Its local channel binds run, node, ACP session,
stage generation and tool-call identity. Authentication material must not appear in
argv, logs, persisted events, or model context; use restricted local IPC and an
inherited secret or descriptor. Close/restart invalidates the generation. EOF,
agent exit and session cancellation terminate pending calls and helper ownership
within a bounded cleanup path.

`report_stage_outcome` validates the declared outcome and returns an actual MCP
`tools/call` result only after the engine has durably accepted a nonterminal
candidate receipt. Prompt success and existing validators still govern final
outcome commitment. Duplicate/retried call identities cannot append or complete twice.
`request_human_input` remains a pending MCP call while the existing durable human
workflow resolves, declines, cancels or times out; the answer returns to the agent
as the MCP result. Later ACP elicitation support may improve presentation for
providers that negotiate it, but it does not replace this result-bearing contract.
Generic built-in/MCP tools are excluded from this v1 catalog pending engine-side
sandbox enforcement. Existing internal dispatcher paths are not exposed through
MCP, and display notifications cannot request dispatch.

## Red-first acceptance

Use a protocol-conformant ACP fixture that consumes the advertised stdio MCP
descriptor and performs MCP `initialize`, `tools/list` and `tools/call` rather than
directly emitting a `SessionUpdate::ToolCall`.

1. The peer sees the exact filtered schema and cannot see a denied tool.
2. An invalid outcome returns a tool error and creates no accepted durable outcome.
3. A valid outcome receives a real MCP result, is durably accepted once, and the
   real Engine completes only after prompt success and the accepted outcome.
4. A spoofed notification with the reserved title has zero authority.
5. Duplicate/retried call identity does not append or complete twice.
6. A human-input call remains pending, a durable request reaches the resolver, and
   the exact answer reaches the peer before the prompt continues.
7. Stop, disconnect, helper crash and daemon/session restart do not hang or claim a
   fabricated result; direct child/helper processes are reaped or explicitly
   reported unconfirmed.

After the conformant fixture is green, run an opt-in supported-provider smoke in a
temporary Git worktree: receive stage instructions, discover the MCP tool, perform
one bounded edit, report the declared outcome and complete through the real Engine.
Credentials stay in the provider's supported authentication store/environment and
are never fixtures or chat content.

## Separate SDK migration

The published official Rust SDK is `agent-client-protocol 2.2.0` with schema
`1.9.1`; both declare MSRV 1.88, so Surge's MSRV 1.96 can remain until evidence
requires a raise. SDK major version and negotiated ACP wire version are different:
keep stable wire v1 and do not enable draft protocol v2.

Treat 0.10.2 → 2.2.0 as its own red/green migration. It replaces trait-oriented
client/agent setup with builder handlers and role-marked connections, changes
dispatch/task lifetimes and schema imports, and removes `unstable_session_usage`.
Preserve Surge's handshake deadline, concurrent control handling, permission
roundtrip, cancellation and verified child cleanup; SDK ownership is not evidence
that those behaviors survive. Version-pinned source and lockfile, not current-main
documentation, define the migration API. Standard form elicitation may be enabled
only after durable approval routing handles accept/decline/cancel/timeout and schema
validation. Native MCP-over-ACP remains optional/unstable and cannot be the only
cross-provider path.

Affected gates: focused protocol fixture and real Engine outer acceptance first;
all surge-mcp/surge-acp/orchestrator/CLI tests; strict all-target/all-feature clippy;
Rust 1.96; formatting; independent spec review followed by quality/security review.


## Accepted implementation refinements

- MCP success for `report_stage_outcome` means **candidate received; final
  validation pending**. The provider cannot finish its prompt while waiting for a
  tool response, so the receipt must precede prompt completion. Final
  `OutcomeReported` and verification retain the existing successful-prompt and
  validator gates. Missing or rejected final candidates fail the attempt promptly.
- Event wire version 9 adds one typed, nonterminal `StageToolReceipt`, with pinned
  run/node/local ACP session/generation, validated required `call_id`, argument
  hash and typed result. Same identity/hash returns the stored receipt; different
  arguments reject. Version 8 history stays readable; older readers fail with
  `SchemaTooNew`. There is no SQLite migration.
- ACP `SessionUpdate::ToolCall` is now `ToolObserved` only. The worker no longer
  parses reserved titles or invokes a dispatcher from display notifications.
  Authority travels through a bounded per-session channel with a one-shot reply,
  never the shared lossy broadcast.
- Safe v1 exposes only `report_stage_outcome` and, when enabled,
  `request_human_input`. The existing engine `SandboxFactory` currently returns
  `AlwaysAllowSandbox`; provider launch flags do not sandbox engine-side tools.
  Generic dispatcher exposure therefore remains disabled. Before expanding the
  catalog, implement and independently test config/category enforcement and
  sandbox hardening. Unknown tools fail before dispatcher execution.
- The stdio helper is the hidden `surge internal-stage-mcp` command, resolved beside
  the running executable (or Cargo's debug directory in tests), not a third release
  binary. Its transient ACP descriptor supplies a fresh 256-bit credential and local
  endpoint through the stdio server environment. The provider is already a trusted
  recipient of this capability; helper launch does not rely on inherited provider
  environment. Descriptor Debug output hides the environment values. Unix directories/sockets are private;
  Windows pipes reject remote clients. Frames, concurrent connections and pending
  calls are bounded. Closing an endpoint revokes its generation and idle helpers.
- Direct ACP child cleanup belongs to the bridge; the provider owns its spawned
  MCP helper. The controlled provider explicitly waits/reaps its helper. Surge can
  verify endpoint-task cleanup and signal helper lifetime through endpoint EOF;
  it must not claim OS reap of a non-child process without evidence.

Current legacy debt to retire after replacement acceptance: old tests/fake facades
inject typed stage-control BridgeEvents directly; triage also uses that old seam.
These are not emitted by the real ACP worker. Do not retain them as a second
production authority or mistake their green tests for provider interoperability.

## Implementation evidence before Stage 5a

- Genuine RED: `/tmp/surge-stage-mcp-conformant-red.log` reports missing
  `surge-stage` descriptor from a real MCP-speaking ACP peer. Independent spoof
  RED: `/tmp/surge-stage-mcp-spoof-red.log` observed unauthorized outcome authority.
- Replacement exact GREEN: `/tmp/surge-stage-mcp-conformant-green.log` (real Engine,
  real helper, MCP initialize/list/call, durable receipt before one final outcome).
- `/tmp/surge-stage-mcp-engine-matrix.log`: twelve subprocess cases passed,
  including six conformant MCP cases and six prior lifecycle cases. The added
  in-process spoof fixture initially failed at SQLite's single-threaded runtime
  precondition; after correcting its runtime, the exact test passed in
  `/tmp/surge-stage-mcp-authority-green.log`. No production behavior was weakened.
- Core receipt/id validation/nonterminal/v8 compatibility: two tests passed in
  `/tmp/surge-stage-mcp-core-tests.log`. Local authentication, unknown-tool denial,
  reliable reply and endpoint revocation: two tests passed in
  `/tmp/surge-stage-mcp-transport-tests.log`.
- Sixteen ACP lifecycle/observational tests passed in
  `/tmp/surge-stage-mcp-observational-tests.log`. Former notification-only outcome
  and human-result success tests now assert zero authority. Dynamic outcome enum
  coverage moved to the exact catalog layer; conformant Engine tests prove success.
- Real ACP sessions reject stage-control broadcast variants. Legacy in-process
  fakes explicitly opt into `legacy_stage_event_adapter`; the default is false.
  This temporary, named test/triage seam is not a fallback for a real MCP session.
- Strict/fmt final gate and independent reviews remain required. No supported paid
  provider smoke or native Windows execution was performed in this slice.


### Review repair 1: disconnect and generation coverage

Add actual-helper subprocess coverage for a pending human call interrupted by
helper death, including bounded failure, no fabricated resolution/receipt/outcome,
and removal of the stale resolver. Exercise revoked old-generation credentials
through a real helper while a fresh helper remains usable. These are coverage
additions unless execution exposes a production defect.

Redaction is best-effort and chunk-local for provider notifications. In particular,
`ToolObserved.call_id` is provider-controlled and is not universal secret protection;
a malicious provider can encode or split values across updates. Credentials are
passed to the trusted provider in the transient ACP MCP descriptor environment,
never deliberately placed in argv, model prompts or durable events. Surge-owned
diagnostics redact the capability, but this is not an information-flow guarantee
over arbitrary agent output. Upstream ACP SDK TRACE can expose raw payloads before
Surge redaction; it must remain disabled in production defaults. No native Windows or supported live-provider claim is made.


Generation fixture exposed a genuine RED: `/tmp/surge-stage-mcp-generation.log`
(helper remained alive beyond 3 seconds after endpoint revoke). rmcp stdio uses
Tokio stdin's uncancellable blocking read, which prevents automatic runtime drop
while the provider holds stdin open. The dedicated hidden helper command now
awaits service cleanup, then exits the subprocess explicitly (0 only on successful
cleanup, sanitized generic stderr and 1 on error). This is not shared CLI shutdown
and does not substitute process exit for acknowledged service cleanup. See
https://docs.rs/tokio/latest/tokio/io/fn.stdin.html .


Repair 1 focused results:
- `/tmp/surge-stage-mcp-helper-death.log`: new real Engine helper-kill test passed
  without a production change (coverage addition). After helper-exit repair,
  `/tmp/surge-stage-mcp-helper-death-green.log` passed again in 0.22 seconds.
- `/tmp/surge-stage-mcp-generation-green.log`: the same generation regression is
  GREEN after the hidden CLI fix (1.44 seconds). Revoked and fresh helpers exit 0
  after cleanup; old locator replay and old credential against the fresh endpoint
  exit nonzero. A fresh same-call-id invocation reaches only the new pinned context.
- `/tmp/surge-stage-mcp-schema-green.log`: stale max-version assertion corrected
  from 8 to 9 and the exact core migration test passed.


## Descriptor environment portability repair (2026-09-28)

The controlled provider now clears inherited Surge endpoint/auth variables and
applies only the stdio descriptor environment. Actual daemon smoke RED6787 exposed
the empty descriptor; GREEN10099 completes with exact 1/1/1/2/1/1 durable evidence.
Derived StageMcpConfig Debug failed the independent serialized-diagnostic oracle
(RED3349); custom redacted Debug and descriptor-derived literal redaction now pass.
Production prepare asserts exact helper command/args and both descriptor variables,
with no capability injected into provider process env (GREEN8063). ACP261 unit
and2 permission-log/SDK-TRACE tests,7 actual Engine MCP journeys, and41 MCP tests
(3 preexisting ignored) pass. No live provider retry was performed. This proves
portability under filtered helper inheritance, not the cause of the earlier Codex
timeout. Combined ACP/MCP/orchestrator/daemon all-target/all-feature strict clippy49227
passed. Owned-file fmt and diff checks pass; workspace fmt is pending the active
Telegram regression edits owned by the other builder. Independent review remains
pending.
