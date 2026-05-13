+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-05-13"
+++

# ADR 0010 — Telegram `callback_data` schema

## Status

Accepted.

## Context

Telegram inline-keyboard buttons carry an opaque `callback_data` string the bot receives when a user taps. Telegram's Bot API hard-limits this string to **64 bytes** and the surrounding ergonomics push hard against creative encodings:

- Anything longer than 64 bytes is silently truncated by the platform, producing a callback the bot cannot parse.
- Binary payloads (base64, msgpack) survive the wire but turn into an opaque blob during incident debugging — operators stare at button taps in `tcpdump` or test fixtures and cannot tell what was clicked.
- Including any user-controlled or run-scoped free text in `callback_data` (run titles, ticket slugs) makes the limit harder to budget for and invites injection-shaped bugs.

Surge already routes one kind of Telegram callback today: the inbox subsystem uses `inbox:start:TOKEN`, `inbox:snooze:TOKEN`, `inbox:skip:TOKEN` to drive desktop and Telegram inbox cards (see [`crates/surge-daemon/src/inbox/consumer.rs`](../../crates/surge-daemon/src/inbox/consumer.rs)). The Telegram cockpit adds a second namespace — operator approvals, edits, rejects, run-level commands — that must coexist with the inbox prefix without ambiguity.

## Decision

The cockpit `callback_data` schema is fixed at:

```
cockpit:<verb>:<card_id>
```

- `cockpit:` — namespace prefix. Distinguishes cockpit callbacks from `inbox:*` (existing) and any future namespaces. Router code dispatches on the prefix; namespaces never overlap.
- `<verb>` — one of an explicit closed set: `approve`, `edit`, `reject`, `abort`, `snooze`, `ack`. Adding a new verb requires updating both the producer (renderer in `surge-telegram::card::render`) and the consumer (router in `surge-telegram::callback::dispatch`) — this is a deliberately small set.
- `<card_id>` — the ULID primary key of the `telegram_cards` row created when the card was first sent. 26 ASCII characters, zero-allocation parse, lexically sortable. No run-IDs, node-keys, or human-readable identifiers appear on the wire.

A worked example: `cockpit:approve:01HKQA8YBT7DM3KQXJV4WRZN82` — 41 bytes, well inside the 64-byte limit even with the longest verb (`approve`).

## Rationale

1. **Card-id, not natural keys.** Storing the cockpit's own primary key in `callback_data` lets the router resolve everything else (run_id, node_key, attempt_index, expected outcomes, chat allowlist) through a single `cards.find_by_id(ulid)` lookup. The wire format is opaque to humans on purpose — the source of truth is the database row.
2. **64-byte budget is generous after stripping run-scoped identifiers.** `cockpit:` (8) + longest verb `approve` (7) + two separators (2) + ULID (26) = 43 bytes. ~21 bytes of headroom remain for future verbs without revisiting the schema.
3. **Closed verb set, dispatched by `match`.** A single `dispatch_callback_data(data: &str) -> CockpitVerb` function parses the verb upfront and routes to a typed handler. Unknown verbs are rejected with `answerCallbackQuery { text: "Unknown action", show_alert: false }` — never panic, never silently drop.
4. **Namespace prefix as the only fan-out point.** The top-level router is:
   ```rust
   match data.split_once(':') {
       Some(("inbox", rest)) => inbox::dispatch(rest, …),
       Some(("cockpit", rest)) => cockpit::dispatch(rest, …),
       _ => answer_with("Unknown namespace"),
   }
   ```
   Adding a third namespace later (e.g. `intake:*` for tracker automation tiers) is a one-line addition to this match.
5. **Stale-tap and pruned-card behavior is unambiguous.** Because `card_id` is a row primary key, the handler can distinguish three cases without heuristics: (a) row exists and `closed_at IS NULL` — handle normally; (b) row exists and `closed_at IS NOT NULL` — answer "This card is no longer active"; (c) row missing (DB pruned) — answer "Card expired". See ADR 0011 for the card lifecycle in full.

## Alternatives considered

**Encode `(run_id, node_key)` directly in `callback_data`.** Rejected for two reasons: it bloats the payload (a ULID run_id alone is 26 bytes; node keys can be longer than that), and it bypasses the cards table, which is also the idempotency anchor (see ADR 0011). Going around the cards table means losing card-level state (chat_id, message_id, content_hash) at the moment we most need it.

**Use a tokenized opaque payload like the inbox's existing scheme (`inbox:start:TOKEN`).** The inbox uses random tokens because its tickets predate Telegram and tokens were cheaper than retrofitting tickets with a stable identifier. The cockpit is being built from scratch; it can adopt the cleaner identifier model directly. Carrying both schemes within a single namespace would be confusing.

**Pack metadata into a base64-encoded JSON blob.** Rejected — opacity in incident debugging (one cannot read the meaning off the wire), no length advantage (a small JSON object reaches 50+ bytes fast), and a parsing surface that could become an attack target if any field is ever user-controlled.

**Versioned schema, e.g. `cockpit:v1:approve:<card_id>`.** Deferred. The 64-byte budget can accommodate a version segment later; until the schema actually changes in a backwards-incompatible way, the unversioned form is shorter and equally unambiguous. When a v2 is needed, both the renderer and router will be updated in the same PR.

## Consequences

- `surge-telegram::card::render` is the **only** producer of `cockpit:*` callback data. Tests assert that every rendered button's `callback_data` matches the regex `^cockpit:(approve|edit|reject|abort|snooze|ack):[0-9A-HJKMNP-TV-Z]{26}$`.
- `surge-telegram::callback::dispatch` is the **only** consumer. It rejects unknown verbs and missing cards via `answerCallbackQuery`, never via an unhandled error or panic.
- Adding a verb requires a code change in two well-known files plus the closed-set match. There is no path for a verb to silently appear at runtime.
- The inbox subsystem keeps `inbox:*` unchanged. No migration on the existing inbox tokens; the two namespaces coexist for as long as both subsystems exist.

## Revisit conditions

Reopen this ADR when **any** of the following becomes true:

- A new verb cannot be expressed inside the 64-byte budget — for example, a verb that legitimately needs to carry inline metadata. (Today the answer is "store it in the database keyed by card_id"; the trigger is a use case where that indirection itself is wrong.)
- A second card-bearing surface needs its own namespace (e.g., the "Sandbox elevation card" mentioned in ADR 0006 + roadmap, which is currently out of scope for this milestone). The decision then is whether to add a parallel namespace (`elevation:*`) or to extend `cockpit:*` with a card-kind discriminator.
- Telegram's Bot API changes the 64-byte limit (no sign of this as of 2026-05).
