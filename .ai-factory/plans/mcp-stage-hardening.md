# Dedicated stage MCP helper hardening

Status: COMPLETE after independent review and focused gates.
SDK migration adapter remains frozen.

## Boundary and constraints

The dedicated helper exposes only `report_stage_outcome` and `request_human_input`.
Engine still owns authentication-pinned identity, durable receipts, idempotency,
human resolvers and final outcome validation. Transport admission never fabricates
acceptance. No generic Engine tool dispatcher or experimental ACP MCP feature.

RMCP1.6's AsyncRwTransport defaults to an unlimited line codec; its service spawns
request handlers before calling ServerHandler and retains completed response-send
JoinSet entries until shutdown. A semaphore in call_tool alone is insufficient.

Use one owned RMCP WorkerTransport to enforce bounds before service dispatch:

- Incoming newline-delimited JSON is capped at256KiB before JSON allocation. Use
  a capped line decoder plus payload-free serde failure, avoiding the RMCP codec's
  raw malformed-line debug logging. Reject batch, malformed and oversized frames.
- Admit at most8 active request IDs. Hold the slot through matching response flush,
  not merely handler completion. Reject duplicate active IDs and excess requests
  explicitly without forwarding them to RMCP or the authenticated stage endpoint.
- Cap total requests at128 per helper process; request129 receives resource
  exhaustion and the dedicated connection closes. This is a helper lifecycle
  invariant bounding RMCP's retained completed tasks, not an MCP-standard limit.
- Notifications do not consume request quota. Initialize notification passes once;
  cancellation is handled inline with per-request cancellation tokens and never
  becomes an unbounded task source. Stop/revocation remains independent of input.
- Worker channels, pending output and handler admission are bounded. Output stalls
  have a cleanup deadline; no detached writer task or unbounded rejection queue.
- Windows local connect retries only ERROR_PIPE_BUSY231 with an elapsed deadline
  and asynchronous backoff. Other errors fail immediately. Native Windows behavior
  is not inferred from macOS tests.

## Ownership / edit zones

surge-mcp/src/stage/helper.rs, new bounded_stdio module, local.rs, module declaration;
surge-mcp/Cargo.toml may enable existing workspace futures/tokio-util codec features.
Actual hidden CLI remains the existing service-settle then process-exit boundary.
No SDK adapter, core event/schema, Engine authority or new runtime dependency.

## Acceptance

Real hidden subprocess over stdio + authenticated StageEndpoint:

1. More than8 pending valid calls yields deterministic overload; no ninth stage
   invocation or durable success. With a slot freed, unrelated valid call works.
2. Slot persists until response flush. Pending human cancellation closes its local
   call and releases authority; endpoint revocation settles helper and all calls.
3. Request129 fails deterministically; notifications do not spend request quota.
4. Oversized unfinished line terminates promptly before JSON parse; malformed and
   batch frames fail without payload-bearing diagnostics. Exact boundary, fragmented
   and multiple frames have focused parser tests.
5. Flood plus Stop remains bounded. Every fixture has a watchdog and child reap.
6. Windows policy tests prove busy→success, deadline, immediate other-error and
   cancellation; compile Windows target if installed, report native evidence absent.
7. Existing conformant Engine/MCP, generation/revocation, auth and owned-field
   permission redaction proofs remain green. Strict affected clippy and fmt pass.

Record genuine behavioral RED before implementation. No RSS claim from channel
capacity alone; test the actual admission and pre-JSON framing boundaries.

## Observed evidence

- Genuine baseline RED: ninth pending call reached the owner, and an oversized
  unfinished line left the helper alive (`/tmp/surge-mcp-hardening-red.log`).
- Windows policy RED: busy errors failed immediately instead of retrying or timing
  out (`/tmp/surge-mcp-pipe-red.log`); all four policy tests now pass.
- Additional RED: duplicate active RPC ID produced an ambiguous competing response
  instead of closing (`/tmp/surge-mcp-duplicate-red.log`). Duplicate IDs now close
  before admission; the original pending authority is cancelled.
- Full MCP suite: 41 passed, 3 existing ignored, including seven real-helper
  subprocess fixtures, generation revocation, slot-through-flush and framing tests
  (`/tmp/surge-mcp-hardening-final-tests.log`).
- Actual Engine ACP/MCP matrix: 14 passed
  (`/tmp/surge-mcp-hardening-engine.log`). Permission owned-field redaction and the
  explicit raw upstream TRACE limitation: 2 passed
  (`/tmp/surge-mcp-hardening-redaction.log`).
- Strict all-target/all-feature clippy across MCP, ACP, orchestrator and CLI passed
  (`/tmp/surge-mcp-hardening-clippy.log`); workspace fmt and diff checks passed.
- Rust 1.96 MCP all-target/all-feature check passed
  (`/tmp/surge-mcp-hardening-msrv.log`). Windows MSVC cross-check passed
  (`/tmp/surge-mcp-windows-check.log`). Native Windows runtime remains unverified.

These are dedicated-helper admission and framing guarantees, not a general MCP
server resource guarantee. The request count includes initialization and ordinary
requests; cancellation notifications do not consume it. Output stalls have a
2-second deadline. No live paid provider or universal secret-redaction claim.
