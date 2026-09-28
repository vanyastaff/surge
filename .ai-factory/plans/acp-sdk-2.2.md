# ACP SDK 2.2 migration — stable protocol v1

Status: SDK migration COMPLETE after independent Stage5a/Stage5b review and final gates.
Separate MCP transport follow-up: [mcp-stage-hardening.md](mcp-stage-hardening.md).
Scope: replace workspace SDK 0.10.2 with verified latest 2.2.0, preserve stable ACP
wire v1 and the accepted lifecycle/MCP guarantees. No draft v2, no SDK-provided
MCP-over-ACP migration, no live paid provider calls during implementation.

## Verified upstream facts (2026-09-28)

Source inspected: published crates.io archives extracted under
`/tmp/surge-acp-sdk-scout`; no Cargo invocation or workspace dependency change.
The crates.io API reports both newest_version and max_version = 2.2.0.

| Evidence | Finding | Consequence |
|---|---|---|
| [SDK manifest](https://docs.rs/crate/agent-client-protocol/2.2.0/source/Cargo.toml), [registry metadata](https://crates.io/api/v1/crates/agent-client-protocol) | SDK 2.2.0; edition2024; rust-version1.88; exact schema dependency1.9.1, derive2.2.0 | Fits declared Surge1.96 at manifest level; actual resolved dependency/MSRV check still required |
| [SDK exports](https://docs.rs/crate/agent-client-protocol/2.2.0/source/src/lib.rs) | `Client`/`Agent` are roles, not the old async traits; old `ClientSideConnection`/`AgentSideConnection` exports are gone | Real adapter rewrite, not import-only bump |
| [Schema modules](https://docs.rs/crate/agent-client-protocol/2.2.0/source/src/schema/mod.rs) | Wire types under `schema::v1`; shared ProtocolVersion under `schema` | Update imports/reexports, keep Surge's BridgeFacade contract |
| [Version constants](https://docs.rs/crate/agent-client-protocol-schema/1.9.1/source/src/version.rs) | V1 stable; V2 requires unstable_protocol_v2; LATEST=V1 without that feature | Explicitly initialize V1 and assert numeric1 on actual wire |
| [Features](https://docs.rs/crate/agent-client-protocol/2.2.0/source/Cargo.toml), [usage schema](https://docs.rs/crate/agent-client-protocol-schema/1.9.1/source/src/v1/client.rs) | `unstable_session_usage` removed; UsageUpdate stable, still context used/size/cost | Remove old feature; do not reinterpret context occupancy as billable per-turn usage |
| [Ordering guide](https://docs.rs/crate/agent-client-protocol/2.2.0/source/src/concepts/ordering.rs) | Ordered request/notification handlers hold dispatch until callback returns | Never await human permission, terminal exit, nested RPC or long prompt inside handler |
| [JSON-RPC implementation](https://docs.rs/crate/agent-client-protocol/2.2.0/source/src/jsonrpc.rs) | `connect_with` owns driven connection; `ConnectionTo<Agent>` is cloneable; requests use `send_request(...).block_task()`; notifications use send_notification | Keep an explicitly owned driver future, not a detached connection handle |
| [Handlers](https://docs.rs/crate/agent-client-protocol/2.2.0/source/src/jsonrpc/handlers.rs) | callbacks and boxed futures require Send; SDK spawn requires Send | Existing Rc/RefCell BridgeClient and mock Peer cannot be captured directly |
| [JSON-RPC lifecycle](https://docs.rs/crate/agent-client-protocol/2.2.0/source/src/jsonrpc.rs) | Dropping SentRequest requests cancellation/discards reply; clean EOF fails pending requests but does not cancel unrelated connect_with main work | Preserve prompt future through bounded session/cancel; explicitly observe incoming_closed/shutdown |

Published schema/derive manifests both declare MSRV1.88. This is not yet a
transitive MSRV proof. No unstable umbrella feature is needed. In particular,
`unstable_mcp_over_acp` stays disabled. One reviewed exception is
`unstable_end_turn_token_usage`, required to retain existing pool TokensConsumed
from PromptResponse.usage. Stable protocol v1 remains explicitly selected; test
both absent usage (unchanged/unknown) and present usage (exactly one event).

## Exact edit/risk map

Current `rg -l agent_client_protocol crates --glob '*.rs'` finds **28 files**, not
the earlier26 (MCP-related additions changed the inventory). Indirect bridge
reexport consumers also need all-target compilation.

| Zone | Concrete files | Risk / treatment |
|---|---|---|
| Dependency | root Cargo.toml, Cargo.lock | Remove obsolete feature, set2.2.0; inspect lock for schema1.9.1/derive2.2.0 and MSRV drift; no duplicate old SDK retained |
| Primary process owner | surge-acp/src/bridge/worker.rs, lifecycle.rs | Highest risk: replace connection creation/initialize/new_session/prompt/cancel while retaining tracked opening/prompt/close, single map owner, child waiter and verified reap |
| Callback adapter | surge-acp/src/bridge/client.rs, session_inner.rs; proposed new private surge-acp/src/sdk_v1.rs and lib.rs registration | Replace old trait implementation with explicit typed handler routing; keep local owner state; permission/terminal waits cannot hold dispatch or a map borrow |
| Legacy live path | surge-acp/src/connection.rs, client.rs, pool.rs | Still used by CLI ping/UI terminal; migrate through same private transport adapter, not a second SDK version. AgentConnection.connection() currently exposes old connection type: update callers deliberately, document public API change |
| Schema-facing surface | surge-acp/src/bridge/{mod,facade,command,error,acp_bridge,session}.rs; shared/content_block.rs; bridge/tokens.rs | Explicit schema::v1 imports/reexports; preserve error/auth/rate-limit classification, MCP descriptor fields and permission response identity; update stale usage docs without inventing accounting |
| Controlled server | surge-acp/src/bin/mock_acp_agent.rs and mock_acp_agent/stage_mcp.rs | Replace Agent trait/AgentSideConnection with Agent builder handlers. Keep existing Child/Peer ownership; provider permission request and prompt run outside dispatch, otherwise cancel cannot arrive |
| Direct application types | surge-cli/src/main.rs; surge-ui/src/screens/agent_terminal.rs; surge-orchestrator/src/engine/stage/agent.rs | Mechanical v1 type path changes only; no UI layout or Engine receipt semantics change |
| Direct test/example imports | surge-acp/tests/{facade_contract,bridge_rate_limit_classification,bridge_control_lifecycle}.rs; surge-orchestrator/examples/engine_in_daemon.rs; tests/{real_acp_smoke,engine_budget_test,archetypes_mock_test}.rs; tests/fixtures/mock_bridge.rs | Type path/trait compilation; retain tests and adapter opt-ins, no ignored-test workaround |

Crate paths in table are relative to `crates/`. Additional lifecycle/Engine/MCP
integration tests do not directly import the SDK name but are mandatory gates.

## Recommended adapter boundary

Retain Surge's existing dedicated worker thread/LocalSet and child ownership.
Use SDK `Client.builder()` + `ByteStreams::new(writer, reader)` with existing
Tokio/futures compat streams. Do not adopt AcpAgent spawning: it would introduce
another process owner and invalidate recently proved cleanup semantics.

Add one private stable-v1 adapter shared by current bridge and legacy connection:

1. Its SDK handlers capture only a bounded Send mailbox and typed responders.
   Callback admission must be nonblocking (`try_send` or equivalent); overload
   replies with an explicit error, never silently drops requests or blocks cancel.
2. A local owner receives typed callback messages and starts tracked local tasks
   for permission/filesystem/terminal requests. Existing BridgeClient methods can
   become inherent methods, avoiding a counterfeit replacement public SDK trait.
   Reply authority stays in the typed responder; exactly one reply/error/cancel.
3. Session notifications remain ordered: enqueue and acknowledge ingestion before
   releasing a dispatch barrier where necessary, without waiting for future peer
   traffic. Prompt completion must not overtake notifications into false success.
   Characterize this ordering with independent wire fixtures before selecting the
   smallest implementation. Do not make reliable control depend on broadcast.
4. The connection driver publishes its ConnectionTo handle to the existing owner,
   then stays alive until explicit shutdown or incoming_closed. Retain and join
   the driver; EOF must settle pending callback tasks and prompt operations.
5. Owner prompt tasks use send_request(PromptRequest).block_task() and preserve
   complete-prompt Result semantics. Close sends stable CancelNotification, waits
   the current bounded prompt grace, then closes transport and reaps child with
   existing fallback. SDK request-drop cancellation is not a substitute for
   session/cancel or verified child cleanup.
6. All driver/callback tasks are tracked and bounded. No general-purpose scheduler,
   no indefinite callback queue, no global Rc-to-Mutex conversion across awaits.

Alternative: convert every callback state to Arc/Mutex and use SDK spawn directly.
Rejected for this migration: larger shared-state change across permission/session
ownership and mock internals; easily reintroduces locks across await. The bounded
mailbox adapter preserves the already-reviewed local ownership design. SDK spawn
returns errors that can shut the whole connection down; request-level failures
must be sent through responder instead of escalating accidentally.

This shape needs a small compile prototype after handoff: confirm responder Send
and lifetime, connection handle publication, EOF propagation, and ordering. If
those constraints require an ownership change, return to maintainer review before
expanding scope. Do not claim source-only inspection has proved compilation.

## Phased implementation and acceptance

### 1. Freeze behavioral oracles, then compile the adapter

Before changing dependencies, keep fixed raw JSON v1 fixtures for initialize
(protocolVersion1), new_session mcpServers stdio descriptor, session/update,
request_permission, cancel, prompt completion/error. A fixture implemented only
against the same new SDK cannot prove wire compatibility. Preserve existing
watchdogs/PID barriers and run currently-green lifecycle/MCP tests as baseline.

After handoff: change root dependency/features and lock; atomically migrate v1
schema paths, register minimal adapter and callback handlers, then mock server.
Run focused ACP all-target check first. Compile failure from removed APIs is a
migration checkpoint, not behavioral RED evidence. Tests for deadlock/ordering or
security refinements must demonstrate behavioral RED where a defect exists.

### 2. Prove primary worker and legacy live path

Run real controlled subprocess acceptance for handshake stalls/noisy stderr,
opening cancellation/bare Drop, concurrent close, cooperative cancel and forced
kill, child exit during prompt, permission prompt+reply and Stop. Keep exact
auth/rate-limit/prompt failure classification. Verify fs capabilities on the wire.

Test one prompt per session, responsive permission reply and close while prompt is
pending; terminate a peer while request/responders are outstanding and assert no
unsettled join or RefCell panic. Exercise notification immediately before prompt
response to prove delivery ordering. Verify driver EOF shuts down even when its
main closure is otherwise waiting. Legacy pool/connection CLI ping tests must
pass too; they are still real application paths, not removable test debris.

### 3. Prove MCP/Engine semantics unchanged

Rebuild both mock_acp_agent and surge helper before subprocess tests. Run complete
engine_acp_permission matrix plus ACP observational tests and actual-helper
stage_generation test. Keep candidate receipt before final outcome, no verification
on prompt error, human answer returns to provider, Stop/death removes resolver,
old generation revoked, generic tools denied, duplicate/hash-conflict semantics,
spoofed SessionUpdate/legacy broadcast zero authority. No notification fallback.

### 4. Bounded MCP follow-ups included in this task

- **Known literal permission title:** BridgeClient request_permission currently
  logs raw tool_name and await_elevation emits it. Redact known literals in event
  and logging display fields, while evaluating sandbox policy on original input.
  Add controlled permission-title fixture containing inherited credential; verify
  captured logs and persisted permission request exclude literal. Retain explicit
  advisory that chunk-local/encoded arbitrary output is not universally scrubbed.
- **Windows named pipe busy:** surge-mcp/src/stage/local.rs currently calls open
  once. Retry only ERROR_PIPE_BUSY with bounded elapsed deadline and async backoff;
  fail other errors immediately and retain revocation semantics. Unit policy
  fixture may run cross-platform; actual parallel-helper named-pipe acceptance
  requires native Windows, not a claim from macOS green.
- **RMCP stress:** exercise real stdio helper concurrent requests beyond local
  mpsc8/connection16 and receipt128 limits; pending human calls plus unrelated
  valid call/revoke must remain bounded. Test oversized raw MCP input separately
  from local256KiB framing: a cap after JSON allocation does not bound ingress.
  Inspect actual rmcp service/request concurrency limits before configuring them;
  do not assert bounded memory from channel capacity alone. Unknown tools never
  reach dispatcher, overload never fabricates durable acceptance. Kill/reap every
  helper in watchdog failure cleanup. Keep changes limited to existing stage MCP
  boundary, not SDK's experimental MCP integration.

### 5. Gates and evidence

With low-debug environment and coordinated shared Cargo window: focused tests,
strict all-target/all-feature ACP/MCP/orchestrator/CLI (daemon production coherent),
then affected suites and full core migration tests. Run UI all-target compile for
schema paths when UI owner/parent permits; no UI feature work. Actual Rust1.96
check of affected dependency graph; inspect cargo tree features confirming no
unstable_protocol_v2/unstable_mcp_over_acp. Workspace all-features must not silently
enable upstream draft features via an added feature mapping.

Final separately authorized provider smoke: one supported installed ACP provider,
isolated temporary worktree, stable v1 initialize, actual descriptor consumed,
real tools/list/call candidate and human-response roundtrip, cleanup verified.
Authentication/quota/network failures remain separate from protocol compatibility.
No paid live-agent execution in this scout or migration without parent dispatch.
Record mock proof and actual provider proof separately; latest SDK alone does not
prove that a provider discovered the injected tools or can execute a whole task.


## Accepted review constraints

Rollback may touch only migration-owned manifest/lock/adapter changes; preserve all
accepted lifecycle/MCP work. Never retain two SDK versions. Bound concurrent local
callback tasks in addition to mailbox capacity. Overload acceptance must include a
pending permission and Stop. Notification acknowledgement confirms owner processing,
not enqueue; overflow is an explicit connection error. MCP hardening has separately
reported evidence and is not inferred from the SDK migration gate.


### Migration evidence and reviewed refinements

Baseline lifecycle10 passed. Fixed v1 JSON oracle3 passed on0.10.2 after including
an explicit clientInfo.title (old schema serializes null when omitted). Dependency
only check RED `/tmp/surge-sdk2-dependency-red.log`: removed root schema exports,
old connection types and Client/Agent traits. New SDK compiles all ACP targets.
Schema1.9.1 adds an explicit auth:{terminal:false} default to clientCapabilities;
the migrated fixed oracle asserts this precise additive field, not a wildcard.

The sole unstable feature is end_turn_token_usage, preserving legacy pool's
existing response-attached TokensConsumed behavior. Actual pool fixture proves
missing usage emits no fabricated consumption and present usage emits exactly one
80-input/20-output event. Stable wire protocolVersion1 remains mandatory.

Known-literal permission-title test found genuine event and owned-log leakage;
policy still receives the original title, while Surge displays are redacted.
Upstream SDK TRACE logs intentionally expose raw transport payload before Surge
handlers. Parent-approved boundary: assert Surge-owned event/log fields are
redacted and separately demonstrate raw upstream TRACE exposure. Do not enable
agent_client_protocol TRACE in production defaults or claim universal redaction.
No per-driver subscriber wrapper is introduced in this migration.

Default logging inspection: CLI and UI fallback is `surge=info`; daemon uses
tracing_subscriber fmt default INFO. None enables agent_client_protocol TRACE.
An explicit operator RUST_LOG override can enable raw upstream diagnostics; the
separate limitation test intentionally does so via a capture subscriber.

## Repair 1: verified legacy cleanup and notification ownership

Legacy AgentConnection retains its SDK driver and explicit kill/wait_or_kill joins it;
AgentPool shutdown propagates cleanup failures. Drop only signals best-effort cleanup.
Notification callbacks are tracked local tasks, with SDK acknowledgement only after owner
processing. An owned byte pump observes EOF independently of the SDK ordered callback wait;
its original byte buffers were fixed at 8 KiB each (superseded by quality repair2 below). Incoming ACP JSON lines are capped at 1 MiB excluding
the newline, counted incrementally before forwarding. Oversized input returns a sanitized
connection error. This is an ACP transport invariant, not a universal provider/log or RMCP
memory guarantee. EOF cancels outstanding callbacks without fabricating acknowledgement.

## Quality repair 2: bounded SDK ingress and EOF drain

ByteStreams/Lines was replaced with the public SDK Channel. Its Lines actor uses
an unbounded producer; a small byte pipe alone did not bound queued valid frames.
Surge's reader is now the only producer of the SDK incoming Channel and gates at
31 queued frames. The bound additionally includes at most one ordered dispatch,
one capped raw lookahead line (1 MiB plus delimiter), and an 8 KiB reader buffer.
The raw buffer is reused; JSON parsing happens only after queue capacity exists.
A 5 ms capacity poll runs only while saturated. Arrays/batches and malformed
JSON-RPC are rejected before SDK dispatch, without payload-bearing diagnostics.
This is an incoming transport bound, not a universal bound on upstream internals.

Stop is independent of read backpressure. EOF is observed while read capacity
exists and after backlog drains; it cannot bypass arbitrary unread bytes. EOF
allows the shared two-second driver cleanup deadline for ordered dispatch to drain;
completed JoinSet entries are reaped rather than treated as unfinished work.
Tests include immediate response+EOF, delayed valid callbacks over 100 ms, blocked
callback failure, saturated valid-frame Stop, ordered burst drain and EOF, malformed
and batch refusal, cap boundaries, transport read error and closed Channel.
