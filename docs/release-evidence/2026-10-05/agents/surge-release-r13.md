# Role 13: authentication and session lifecycle

Scope: Surge has no web login. Reviewed ACP provider session identity/opening lifecycle and Telegram pairing/revocation. Read-only; no repository changes or cargo compilation.

## Findings

1. **P1: ACP model/effort option configuration can hang session admission and bridge shutdown.** `crates/surge-acp/src/bridge/worker.rs:509` awaits `session/set_config_option` through `effect_fence::admitted`, which only checks admission then awaits the future. Unlike initialize/new/resume/load, it does not use `handshake_step` (`worker.rs:702`) and therefore observes neither the shared handshake deadline, shutdown cancellation, nor dropped caller reply. `sdk_v1.rs:241` likewise awaits the request without a timeout. Trigger: provider accepts initialize/session creation but never responds to model/effort configuration. Requested correction: reuse bounded handshake phase and preserve secret redaction in failure cleanup. Regression: mock stalled option response; assert bounded HandshakeTimedOut phase, process settlement, subsequent session can open; separately cancel/drop caller during this phase. Retrying published option/new-session operations must remain prohibited (retry is currently initialize only).

2. **P1: revoking Telegram pairing does not stop outbound sensitive cards.** `crates/surge-cli/src/commands/telegram.rs:137` only soft-deletes allowlist membership. Inbound commands/callbacks are correctly admission-gated, but `cockpit/dispatch.rs:130` uses configured fixed `admin_chat_id`, `card/emit.rs:207` sends to stored card chat, and `cockpit/production.rs:148` sends/edits through Bot API without consulting pairings. Reconcile has no outgoing admission seam either. Therefore configured chat receives new prompts/failures and refreshed cards before pairing or after revoke. Requested correction: fail-closed admission at production outgoing card transport boundary (send and edit), leaving pairing acknowledgments available; ensure reconcile uses same gate. Regression: unpaired chat receives no cards; paired receives; revoke then send/edit/reconcile emits no sensitive payload; re-pair restores delivery. Scope caveat: explicit configured destination already grants delivery intent, but CLI calls revoke 'remove a paired chat' and continued delivery undermines expected revocation/privacy.

## Evidence checks performed

- Read attached release requirements and Rust Studio review discipline.
- `rg`/`sed` source tracing across Telegram setup/pair/revoke, persistence pairing transaction, inbound admission, outgoing dispatch/emitter/transport/reconcile; ACP open/restore/option phase, effect fence, SDK method.
- Python static assertions: option phase contains no handshake_step/sleep_until/shutdown.cancelled/reply.closed; outgoing production adapter and dispatch/emitter/reconcile contain no is_admitted checks. Passed, output recorded in agent tool transcript.
- Pairing storage implementation is target-bound, expiry checked with >=, IMMEDIATE transaction atomically consumes and writes admission; rollback on failed admission. Existing tests cover replay, wrong-chat, legacy codes, atomic rollback, races and revocation. Tests inspected, **not run**.
- ACP continuation compares canonical cwd, runtime, launch hash and invocation; unsupported restore fails closed; initialize-only retry avoids replaying published session mutation. Existing tests inspected, **not run**.

## Remaining risks / verdict

NEEDS WORK: both findings require implementation and runtime regression evidence. No live Telegram credentials tested. No authentication security or release readiness certification from static inspection alone. Existing pairing transaction appears well designed but remains unverified by execution in this role.
