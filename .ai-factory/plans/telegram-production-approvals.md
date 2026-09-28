# Telegram production approval completion

Status: source-audited draft; independent design review and implementation pending.
This remains part of the full project-completion goal, not a replacement for the
desktop prompt-to-application journey. No live Telegram messages are needed for
implementation acceptance; use a local HTTP fixture with a dummy token.

## Observed production gaps

- `cockpit/production.rs` discards the callback query id and every successful
  `CallbackOutcome`. Telegram receives no `answerCallbackQuery`; Edit intent never
  creates a ForceReply prompt. `handle_reply` only logs the reply.
- `cockpit/callback.rs` checks admission and whether the card is open, but does not
  compare the card's chat with the originating chat. Resolution always supplies
  `call_id = None`, losing the identity of the specific pending request.
- `cockpit/dispatch.rs` gives every gate attempt index zero. Repeated visits to
  a node reuse a card key. It also leaks node-key strings despite being used in
  production; replace borrowed-static requirements with owned data in this path.
- `telegram/cards.rs` already stores `pending_edit_prompt_message_id`, but lacks
  the production lookup/update operations needed to correlate replies durably.
- Daemon `main.rs` intentionally runs the legacy inbox bot outgoing-only to avoid
  competing getUpdates consumers. Its comment defers routing `inbox:*` callbacks
  into the cockpit's single stream; those visible buttons currently have no
  production handler through that stream.
- Abort/Snooze cockpit callbacks return NotImplemented. `/run` uses
  DeferredRunStarter. These remain explicit open requirements, even if gate
  approve/edit is implemented first.

## Intended behavior and boundaries

Use one Telegram update consumer. Route callback namespaces explicitly to the
owning subsystem, with a single response to the callback query on every handled
path. Keep inbox persistence logic in daemon; inject a narrow adapter into the
shared route rather than adding a daemon dependency to surge-telegram or starting
another poller. Enforce admission and the stored target chat before side effects.

Resolve the exact durable pending gate identified by the card, including repeated
attempts and request/call identity. Derive identity from persisted events rather
than a transient counter. Old cards must not resolve a new visit to the same node.
Review existing schema and recovery contracts before choosing a migration; never
infer a missing old request identity by resolving the currently active gate.

Edit creates a ForceReply prompt and persists its chat/message-to-card mapping.
Restart preserves correlation. Reply text is delivered only for the matching open
request, and a stale/duplicate/foreign reply cannot affect a later gate. Account
explicitly for the sendMessage-success/database-failure boundary; no exactly-once
network guarantee may be claimed. Existing command alternatives should use the
same identity checks, not bypass them with an ambiguous run-only resolution.

Close/update a card only after the authoritative engine decision is accepted.
Transport failure must remain retryable without claiming the decision was applied.
Abort and snooze use the existing owning run/cockpit operations; `/run` should use
the durable bootstrap submission once that path is enabled, not the legacy RAM
queue or an isolated second engine.

## Acceptance evidence required

Start with a failing test through actual ProductionRoutes, real temporary SQLite,
and a local Telegram HTTP fixture. Pure callback mocks do not prove runtime wiring.
Cover approve and reject acknowledgements; malformed, denied, foreign and stale
callbacks with no mutation; Edit prompt, storage reopen and correlated reply;
duplicate reply; repeated node visits; exact request identity; API failure and
database failure boundaries. Verify inbox callbacks traverse the single update
stream and record their action. Extend real Engine coverage to a gate that resolves
and returns to the same node so an old card cannot resolve its successor.

All visible callback verbs and advertised commands need working acceptance before
this requirement is complete. A smaller first implementation slice must list the
remaining gaps explicitly. Run affected tests, strict all-target clippy and
formatting; independent spec review precedes code-quality review.

## Refreshed source map (2026-09-28)

The gaps above remain present after daemon bootstrap and ACP/MCP completion.
Additional constraints found in the production path:

- `engine/engine.rs::resolve_human_input` with `call_id=None` removes the first
  pending HumanGate. HumanGate events currently also carry `call_id=None`.
  Persisting a card's node/sequence alone cannot prevent a check/resolve race.
  The Engine owner needs a narrow atomic expected-request identity check before
  consuming the resolver; propagate it through the facade/IPC as necessary.
  Tool-driven MCP requests already use their actual call identity. Do not substitute
  a synthetic Telegram call ID into that existing tool-resolution namespace.
- `cockpit/recover.rs` only scans existing open cards and closes terminal/unknown
  runs. It cannot recreate a gate missed before card insertion, nor distinguish an
  old gate from a later gate in an active run. The tap loop logs failed dispatch
  and proceeds. Use persisted request/resolution events for startup/lag catch-up
  and retry failed sends; a broadcast is a wake-up hint, not durable authority.
- `PersistenceSnapshots` currently converts every reader error into unknown run;
  reconciliation can then close a valid card. Preserve read errors and use the
  read-only inspection API rather than a writer-capable open for recovery.
- `ProductionRoutes::handle_callback` neither acknowledges successful callbacks
  nor closes accepted cards. `/feedback` uses the same ambiguous run-only
  resolver. Both must share the exact-request acceptance path.
- `/run` currently advertises `<archetype-or-path>`. Durable bootstrap now exists,
  but needs explicit original project/config selection and stable operation ID.
  Do not reinterpret existing arguments silently or infer a project from daemon
  cwd; specify the Telegram command contract before wiring the durable submitter.

### Credentials and admission

Daemon requires a `[telegram]` config plus target chat ID (direct or named env).
Token resolution prefers `telegram.cockpit.bot_token` in the registry secrets
store, then `bot_token_env`. `surge telegram setup` persists the token and mints a
pairing token; it does not supply the daemon's missing target chat configuration.
The existing secrets table explicitly stores plaintext protected by filesystem
permissions. Do not describe it as encrypted. Keep bot tokens out of argv/logs;
the existing optional CLI `--token` path exposes argv and should be replaced or
clearly deprecated in favor of stdin/environment references. Telegram HTTP URLs
contain the bot token, so transport-error logging needs a known-literal token
regression, including surge-notify's reqwest error formatting.

Current admission is chat-based, not individual-user-based. A paired group grants
its members that chat's authority; document this policy or deliberately restrict
pairing to private chats. Enforce both pairing and stored destination chat before
cockpit or inbox side effects. Inbox's reusable `handle_action` validates a token,
not originating chat; its Telegram adapter must add this check.

### First bounded implementation and independent RED oracles

1. Real ProductionRoutes + temporary SQLite + local Bot API HTTP fixture: approve
   an open card and assert one `answerCallbackQuery`, exact Engine acceptance and
   card closure. Current production drops the acknowledgement/outcome. Foreign
   paired-chat test must show zero Engine calls and no card mutation.
2. Actual Engine cycle revisiting one HumanGate: old card must fail atomically
   after the new request is installed; test the race at the owner boundary, not a
   second Telegram preflight. Include MCP human input with its real call identity.
3. ForceReply: send, persist mapping, reopen storage, answer matching reply exactly
   once; stale/foreign reply and API-success/DB-failure remain non-authoritative.
4. Drop a request tap before first card creation, restart/lag-reconcile, and assert
   the pending gate is eventually delivered from durable events. Read failure
   must leave its card open and surface an error rather than treat it as absent.
5. Inject actual `inbox:*` update through the same runtime; assert one durable
   action and one callback acknowledgement. No second getUpdates consumer.

Reuse CardEmitter, teloxide Bot's local API URL test seam, existing cards/pairings,
Engine resolver owner, and daemon inbox action persistence. After replacement E2E,
remove the unused inbox incoming poller/on_callback path, DeferredRunStarter when
its replacement contract is implemented, the node-key leak, and tests that bless
attempt-zero collapse or run-only resolution. Preserve outgoing inbox delivery and
surge-notify's independent general notification deliverer. No code or external
Telegram calls were made during this scout.

## Prerequisite slice: exact gate identity and durable discovery — ready for Stage5a

Maintainer API review: ACCEPTABLE (root). The first implementation deliberately
stops before Telegram callback/HTTP/token work.

- `GateRequestId` is a core typed ULID. Each HumanGate/skill-trust registration
  mints a fresh ID; existing session=None request/resolution/timeout events carry
  its namespaced string in call_id. Resume registers a fresh ID. No schema change.
- `resolve_gate_input(run, node, GateRequestId, response)` compares and removes
  the resolver under the same owner lock. Facade and IPC preserve that identity.
  The old run-only `resolve_human_input(None)` refuses input; tool calls retain
  their separate scoped call-ID path. CLI/bootstrap/UI consume captured identity.
- The pure latest-pending fold clears input only on exact node+call-ID match.
  Resolved/timeout events have no session field; MCP call keys already contain
  session and generation, and gate IDs are unique registrations. This does not
  introduce a multi-pending projection or change event schema.
- Production Telegram snapshots use read-only inspection. Read errors and missing
  databases with registry/directory evidence are errors, never permission to close
  a card. Only genuinely absent runs return None.
- Startup, periodic retry and tap-lag reconciliation discover current unresolved
  session=None typed gate requests from durable events, checking current cursor,
  graph and nonterminal/unparked state. Old unbound and tool requests are excluded
  from this recovery prerequisite. Live tap remains a delivery hint.
- HumanInputRequested cards use the durable request sequence as their existing
  attempt_index key, so re-delivery is idempotent and newer visits get new cards.
  Production node-key strings are owned; the leaking helper is removed.

Observed REDs: actual Engine old response consumed a second visit's resolver
(`/tmp/surge-telegram-gate-identity-red.log`); corrupt database became Ok(None)
(`/tmp/surge-telegram-recovery-red.log`); durable request without a card was omitted
(`/tmp/surge-telegram-missed-card-red.log`); stale resolution cleared newer input
(`/tmp/surge-telegram-fold-red.log`). Focused GREEN includes actual Engine
reopen/resume, startup runtime card delivery with no tap, and readonly error
preservation. Final gates passed; independent Stage5a review is pending.

Remaining callback slice must load the exact request at the card's durable
sequence, enforce chat/admission, invoke the typed owner API, acknowledge callback
queries, and implement edit/reply correlation. Until then old callback None calls
fail closed. Remove the separate actionable BootstrapApprovalRequested metadata
card mapping when authoritative stage-aware HumanInputRequested rendering replaces
it. Do not claim complete Telegram approvals from this prerequisite.

### Prerequisite gate evidence

All commands used CARGO_PROFILE_DEV_DEBUG=0, CARGO_PROFILE_TEST_DEBUG=0,
CARGO_INCREMENTAL=0. No external Telegram calls or token configuration changes.

- CLI/core/Telegram full all-feature suite: 1098 passed, zero ignored
  (`/tmp/surge-telegram-cli-core-tests.log`). Includes five production recovery
  fixtures and actual resumed-console compatibility.
- Orchestrator library plus bootstrap/gate cancellation/identity integrations:
  361 passed (`/tmp/surge-telegram-orchestrator-tests.log`). Exact identity fixture
  re-run after the additional wrong-node assertion is recorded separately in
  `/tmp/surge-telegram-final-identity.log`.
- Daemon library plus persisted-gate and resume-stream IPC integrations: 112 passed
  (`/tmp/surge-telegram-daemon-tests.log`).
- Actual ACP/MCP Engine compatibility: 14 passed
  (`/tmp/surge-telegram-mcp-compat.log`).
- Native UI all-target/all-feature check passed
  (`/tmp/surge-telegram-ui-check.log`). Combined strict all-target/all-feature
  clippy for core/orchestrator/Telegram/daemon/CLI/UI passed
  (`/tmp/surge-telegram-final-strict.log`). Existing upstream block0.1.6 future
  incompatibility advisory remains; workspace formatting and diff checks passed.

Review scope: core IDs/latest-pending fold; Engine gate registration/owner/facade/
IPC; captured-identity CLI/bootstrap/UI callers; Telegram readonly snapshots,
current-gate discovery, periodic/startup recovery and sequence-keyed cards; tests.
No callback acknowledgement, ForceReply, inbox route, bot-token or HTTP changes.

### Stage5b repair1 — isolate unreadable journals during missed-card recovery

`RunSnapshotProvider::pending_requests` now returns a structured batch of healthy
requests and per-run inspection/replay failures. Registry discovery errors remain
outer errors. The cockpit logs each failed run, dispatches healthy requests, and
keeps unreadable runs' cards open; failures are never interpreted as absent runs.

Real SQLite + `drive_tap_loop` regression uses two pending runs, no live taps,
and a corrupt journal with an existing card. Genuine RED90102 failed because the
healthy card starved (`/tmp/surge-telegram-isolation-red.log`). GREEN41682 passed
all 115 Telegram tests, including exactly one healthy send across two recovery
ticks, retained corrupt card, and explicit typed per-run failure evidence
(`/tmp/surge-telegram-isolation-green.log`). No HTTP/callback authority changed.
Strict Telegram all-target/all-feature clippy50559 passed with `-D warnings`
(`/tmp/surge-telegram-isolation-strict.log`); workspace fmtcheck and diffcheck
passed. Snapshot frozen for parent repair1 re-review.

### Production callback slice — implementation under verification

Production callbacks acknowledge the Telegram query before storage/engine work,
then send a separate accepted/stale/error result. The resolver receives the card,
loads its exact durable event sequence, validates node + session=None + typed
GateRequestId and current unresolved state, and invokes atomic `resolve_gate_input`.
It never substitutes a newer pending identity. Accepted cards close; duplicate
callbacks cannot resolve twice. Wrong chat and stale cards fail closed.

Edit sends ForceReply and persists its message ID in the existing card column.
Text must reply to that exact prompt in the exact chat; bare/wrong/stale replies
cannot supply authority. `/feedback <run_id>` now directs to Edit rather than
retaining an unbound resolution API. No new registry schema is required.

The sole cockpit polling stream forwards `inbox:*` via an injected daemon-owned
handler. That owner validates the original delivery chat and reuses the existing
inbox action queue. No second getUpdates loop is started. Token provisioning,
`/run`, cockpit Abort/Snooze, and general agent-tool text requests remain deferred.

Bootstrap HumanInputRequested carries `x-surge-bootstrap-stage` in its existing
JSON schema. Telegram renders that authoritative request with stage context and
ignores BootstrapApprovalRequested for card creation (lifecycle event retained).
The superseded duplicate mapping and unbound callback/feedback resolver API were
removed; the old standalone inbox poll entrypoint remains compatibility debt.

Behavioral RED86666: real Engine + SQLite + local Bot API production callback
never acknowledged and failed to resolve the valid gate
(`/tmp/surge-telegram-callback-red.log`). Duplicate-card RED22919 proved metadata
still created a second actionable card (`/tmp/surge-telegram-duplicate-card-red.log`).
Replacement fixtures cover old open card against later gate, duplicate approve,
absent active owner with callback acknowledgement/error, exact ForceReply chat and
message, and the single update stream routing inbox and feedback. No external
Telegram calls or live provider requests were made.

Callback slice focused verification:
- Telegram all-feature suite51413: 116 passed (98 unit,7 existing integration,
  5 actual Engine/local HTTP production callback tests,6 durable recovery).
  `/tmp/surge-telegram-callback-final-tests.log`.
- Shared update-stream test14965 and real daemon inbox-owner SQLite test82500
  passed. `/tmp/surge-telegram-stream-green.log`,
  `/tmp/surge-telegram-inbox-green.log`.
- HumanGate all-feature unit filter60893:11 passed, including bootstrap decision
  persistence and cancellation. `/tmp/surge-telegram-callback-gates.log`.
- Strict41879: Telegram+persistence+orchestrator+daemon all-target/all-feature
  clippy with `-D warnings` passed. `/tmp/surge-telegram-callback-strict.log`.
- Workspace fmtcheck and diffcheck passed. No live Telegram verification claimed.

The five old `/feedback` fake resolver tests were replaced by the one safe
card-reply guidance test because that unbound mutating API was removed, not
weakened. Production callback coverage now checks actual Engine outcomes and
SQLite events. Incoming test updates deserialize JSON text (as the real wire does):
upstream teloxide's borrowed-key UpdateKind visitor can turn `from_value` input
into Error silently, so fixtures also assert the decoded update variant.

Snapshot frozen for Stage5a review; later work remains explicitly deferred above.

### Callback Stage5a repair1 — accepted decision vs card write failure

Genuine RED69952 injected SQLite `BEFORE UPDATE OF closed_at` failures after
Engine acceptance for both Approve and ForceReply Edit. Each run had exactly one
persisted resolution, but the operator incorrectly received “could not be
accepted … retry” (`/tmp/surge-telegram-accepted-close-red.log`).

The callback outcome and edit result now distinguish accepted decisions from
failed card presentation writes. They report “accepted; card status update
failed. Do not submit again,” log the storage error, and retain the consumed
request identity. A repeated callback/reply cannot apply the decision again.

Additional actual Engine/SQLite/local Bot API coverage verifies persisted
ForceReply correlation after reopening Storage and constructing new routes,
reject, paired foreign-chat denial, callback/result Bot API failure, ForceReply
send failure, and successful prompt send followed by failed prompt-ID persistence.
Replies to an unsaved prompt have zero authority and leave the gate usable.
Callback fixture operations have a 30-second watchdog; the mock API failure
fixture completed within that bound. No external Telegram calls were made.

Focused production12 GREEN80480 and combined strict30359 passed. Final full
Telegram regression and final checks recorded below after completion.

Final repair1 verification: full Telegram87142 passed123 (98 unit +7 existing
integration +12 production callback +6 recovery), no ignored tests.
`/tmp/surge-telegram-callback-repair1-full.log`. Final combined strict54090 passed
all four affected crates/all targets/all features with `-D warnings`.
`/tmp/surge-telegram-callback-repair1-strict-final.log`. Workspace fmtcheck and
diffcheck passed. Frozen for repair1 re-review; Cargo released.

### Credentials/pairing/reconciliation slice — approved implementation boundary

Use an environment-variable reference plus explicit nonzero delivery chat. CLI
setup accepts `--token-env NAME --chat-id ID`; plaintext `--token` and stdin are
refused without echo or persistence. The daemon never prefers/falls back to the
legacy plaintext secrets row; it reports migration guidance. Runtime credentials
have redacted Debug/Display and no serialization. Owned transport errors do not
copy upstream API payloads/URLs into logs. Raw upstream transport tracing remains
outside the owned redaction guarantee and must stay off by default.

Registry migration0019 adds nullable target_chat_id to pairing codes. NULL legacy
codes cannot authorize pairing. One IMMEDIATE transaction validates target,
expiry and consumption, conditionally consumes the code, inserts the allowlist
row and commits; any failure rolls everything back. New commands use this one
owner; the old consume-then-pair API is removed.

Card reconciliation uses the card's exact source event sequence and matching
node/session/call identity to observe a later resolution/timeout. A newer gate at
the same node is not authority to close the old card, nor can an old resolution
close the newer card. Unreadable evidence stays an error.

Genuine RED evidence: recovery52536, pair race46700 (8 callers accepted one code,
writer-lock/busy-handler synchronized), plaintext config10884, owned diagnostic
literal leak27802, argv-to-plaintext-registry97850. Logs:
`/tmp/surge-telegram-{resolved-card,pair-race,config,log,argv}-red.log`.
