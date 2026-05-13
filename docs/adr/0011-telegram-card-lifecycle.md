+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-05-13"
+++

# ADR 0011 — Telegram card lifecycle, idempotency, and recovery

## Status

Accepted.

## Context

The Telegram cockpit (`surge-telegram`) sends *cards* — Telegram messages with inline keyboards — to mirror the engine's pending approvals, stage transitions, and run-level state changes. Cards are the operator's primary mobile surface. Three load-bearing requirements pin the card lifecycle to a specific shape:

1. **Idempotency under the bootstrap edit loop.** A single bootstrap stage can re-enter the same `(run_id, node_key)` up to `edit_loop_cap` times (default 3, see [ADR 0004](0004-bootstrap-human-gate.md)). Each attempt must land on a fresh card — otherwise `editMessageText` would silently overwrite the previous attempt's approval, and the operator's history of the run would lose the prior decision.
2. **No card spam under engine event tails.** The cockpit subscribes to a `RunEventTap` broadcast and reacts to every event affecting a card-bearing surface. A run that progresses through ten `StageEntered` events should produce *one* status card edited ten times, not eleven separate cards.
3. **Crash-safe recovery without duplicate sends.** The daemon may restart between a card emission and the operator's tap. On restart, the cockpit must pick up where it left off — refresh the existing card's content if the run state has moved on, or close the card if the underlying gate has already resolved while the daemon was down. Never re-send.

The constraints together force a specific design: a SQLite table that owns card identity, a correlation triple that captures attempt history, an `editMessageText`-only update path, and a stateless cockpit that derives card content from the run's event log on demand.

## Decision

### 1. Correlation key — `(run_id, node_key, attempt_index)`

Each card is identified in storage by the triple **`(run_id, node_key, attempt_index)`**. `attempt_index` is sourced from `RunMemory.node_visits[node_key]` (added by Task 27 of the bootstrap milestone; the field already exists in `crates/surge-core/src/run_state.rs`). The combined triple is what we hash to derive the card's ULID primary key.

A `UNIQUE(run_id, node_key, attempt_index)` constraint on the `telegram_cards` table makes the triple the sole anchor for idempotency. `INSERT OR IGNORE` on this triple guarantees that a duplicate emission (event tail replayed, daemon restart, retried send) finds the existing row instead of creating a second one.

### 2. Update path — `editMessageText` only

After the initial `sendMessage`, every subsequent state change for an open card is delivered via `editMessageText` on the existing `chat_id/message_id` pair. Sending a fresh card on update is **forbidden** — it would spam the chat, break threading, and lose the operator's history of the run.

The card row stores the message body's `content_hash`. The cockpit emitter computes a fresh hash on every dispatch event and only invokes `editMessageText` when the hash has actually changed. Identical-content updates are no-ops at the wire layer; the Telegram Bot API rate-limit budget is spent on real visual changes only.

### 3. Recovery — `reconcile_open_cards` reads, never writes new cards

On daemon startup, `surge-telegram::recover::reconcile_open_cards` (Task 21 of the milestone) reads every row where `closed_at IS NULL`, derives the current card content from the run's event log via the existing fold helpers, and either:

- calls `editMessageText` to refresh the body when `content_hash` has drifted (daemon restart between an emission and a response);
- marks the row `closed_at = now` when the underlying gate has resolved while the daemon was down (no edit needed — the operator already moved on);
- does nothing when the on-screen hash matches the derived hash.

**Never** issues a `sendMessage` on resume. The triple-key constraint plus the open-cards reconcile path makes "duplicate card after a restart" structurally impossible.

### 4. Cockpit is stateless beyond the `telegram_cards` table

The cockpit holds no in-memory map of active cards. Every operation — receive callback, react to event tap, react to restart — looks the card up by primary key or by `(run_id, node_key, attempt_index)`. The only authoritative state outside Telegram itself is one SQLite table. This is what lets the daemon crash-restart loop converge without ad-hoc bookkeeping.

## Rationale

1. **The triple captures the only invariant that matters: "the same attempt of the same gate."** Two different stages, two different attempts of the same stage, or two different runs all produce distinct triples — and therefore distinct cards. Within an attempt, every event tail emission collapses into edits on the same row.
2. **`attempt_index` is already deterministic.** `RunMemory.node_visits` is a deterministic fold over `EdgeTraversed { kind: Backtrack }` events (Task 27 of the bootstrap milestone). The cockpit reads it; it does not own its definition. Replaying a run produces the same `attempt_index` sequence, which produces the same card triples, which means deterministic test fixtures.
3. **`editMessageText` is the documented happy path for evolving cards.** Telegram caches inline-keyboard updates against the existing `message_id`, threading the operator's view smoothly. Sending fresh cards on every update would defeat threading and chew through the per-chat rate-limit budget unnecessarily (see ADR-adjacent rate-limit handling in the milestone plan).
4. **A `content_hash` short-circuit costs one hash per event tap and saves real network calls.** The hash is computed over `(body_md, keyboard_serialized)`. A `StageEntered` event tail that does not change the rendered status card body is filtered out at the hash check before any Bot API call — meaningful when a run emits dozens of small events per minute.
5. **Stateless recovery is the only design that survives kill -9.** Any in-memory card map would be lost on a forced restart, and the cockpit would have to either accept duplicate sends or build a write-ahead log on top of the existing event log. The cards table *is* that log, derived from the same source of truth as the rest of the engine state.

## Alternatives considered

**One card per `(run_id, node_key)` — no `attempt_index`.** Rejected because the bootstrap edit loop legitimately re-emits the same gate up to 3 times by design, and the operator must see each attempt as its own card (otherwise approving attempt 2 would visually overwrite attempt 1 with no audit trail). The cost of carrying `attempt_index` is one `INTEGER` column; the cost of not carrying it is losing audit fidelity exactly when it matters most.

**Update model where every event sends a fresh `sendMessage`.** Rejected because (a) Telegram chats become unreadable under a high-event-rate run (a typical bootstrap emits 30+ events in two minutes), (b) the per-chat rate limit (1 msg/sec sustained) is consumed by spam instead of real updates, (c) operator approval clicks land on whichever card happens to be visible, not the canonical one — race conditions become user-visible.

**Re-emit cards on restart from scratch.** Rejected — same reasons as above, plus duplicates a card the operator may already have responded to (a forced-restart race where the response message landed but the daemon crashed before persisting `closed_at`).

**In-memory `BTreeMap<CardKey, CardState>` mirroring the cards table.** Rejected because it adds a second source of truth that must be kept in sync, complicates recovery, and produces nothing the SQLite query layer cannot serve directly. The cards table has at most a few hundred open rows in any reasonable deployment; the read path is cheap.

## Consequences

- `telegram_cards` table schema is **fixed** at: `card_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, node_key TEXT NOT NULL, attempt_index INTEGER NOT NULL, kind TEXT NOT NULL, chat_id INTEGER NOT NULL, message_id INTEGER NOT NULL, content_hash TEXT NOT NULL, pending_edit_prompt_message_id INTEGER NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, closed_at INTEGER NULL, UNIQUE(run_id, node_key, attempt_index)`.
- The repo guarantees three invariants in code review:
  - Only one production call site of `sendMessage` for cockpit cards (in `surge-telegram::dispatch::send_new_card`).
  - Every other state transition uses `update_content_hash` → `editMessageText`.
  - No code path queries Telegram for card state — the database row is authoritative.
- Tests assert `INSERT OR IGNORE` idempotency on the triple, `update_content_hash` returns `false` on no-op, and `reconcile_open_cards` makes zero `sendMessage` calls under a synthetic restart scenario.
- A stale tap on a closed or pruned card responds with `answerCallbackQuery` only (see [ADR 0010](0010-telegram-callback-schema.md)) — never an error, never a panic.

## Revisit conditions

Reopen this ADR when **any** of the following becomes true:

- A new card kind needs to span multiple Telegram messages (e.g., a diff preview that exceeds the 4096-byte body limit and must be split across messages). The `(run_id, node_key, attempt_index)` triple would need to become `(triple, fragment_index)`.
- The recovery model needs to handle a hostile-network case (Telegram unreachable for hours): today the cockpit simply skips reconcile and retries; if this becomes unacceptable, a per-card "needs-reconcile" flag may be needed.
- Telegram's Bot API changes its `editMessageText` semantics (e.g., loses the no-op short-circuit, or starts charging the rate-limit budget on no-op edits). At that point the content-hash check would need to short-circuit even earlier in the dispatch path.
