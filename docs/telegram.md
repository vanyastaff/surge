# Telegram cockpit

The cockpit is Surge's primary remote control surface: a Telegram bot
that mirrors live engine activity as inline-keyboard cards and lets the
operator approve, edit, reject, abort, or snooze every gate from their
phone. The daemon writes only via `editMessageText` (Decision 8) so
operators see one card per gate that updates in place — no scrollback
noise.

> Decision records:
> [ADR 0009](adr/0009-no-human-input-resolver-trait.md) (engine entry
> point is the contract),
> [ADR 0010](adr/0010-telegram-callback-schema.md) (callback wire
> format),
> [ADR 0011](adr/0011-telegram-card-lifecycle.md) (card lifecycle +
> recovery),
> [ADR 0012](adr/0012-surge-telegram-crate-split.md) (crate boundary
> with `surge-notify`).
>
> Source crate: [`crates/surge-telegram/`](../crates/surge-telegram/).

## Setup

### 1. Mint the bot token

In BotFather: `/newbot`, follow the prompts, copy the token.

### 2. Persist token + mint a pairing token

```bash
surge telegram setup --label "operator-phone"
# (or omit --label for the default "operator")
# Paste the bot token at the prompt, or pass --token <TOKEN> for scripts.
```

The command:

- writes the token into `~/.surge/db/registry.sqlite` under the
  `telegram.cockpit.bot_token` key (see `surge_persistence::secrets`),
- mints a 6-character base32 pairing token with a default 10-minute TTL,
- prints instructions like:

  ```
  ✅ Bot token saved.
  Send `/pair <TOKEN>` to your bot from your personal chat within 10 minutes.
  ```

### 3. Pair your chat

Open the chat with your bot in Telegram. Send:

```
/pair <TOKEN>
```

On success the bot replies `✅ Paired this chat as <label>` and the
chat is inserted into the `telegram_pairings` allowlist. Every command
and callback from this chat is admitted from now on; every other chat
gets a generic deny message.

### 4. Start the daemon

```bash
surge daemon start
```

The daemon's cockpit subsystem spawns automatically when both the bot
token (in secrets) and a `[telegram]` chat-id configuration are
present. Cards start landing on the next bootstrap gate / human-gate
event.

### 5. Revoke / list

```bash
surge telegram revoke <chat_id>   # remove a paired chat
surge telegram list               # active pairings
```

## Commands

`/pair` is the only command available to UNPAIRED chats. Every other
command runs an admission check against `telegram_pairings` first and
silently drops unpaired callers (Decision 6).

| Command | Effect |
|---------|--------|
| `/pair <token>` | Consume a pairing token and add the chat to the allowlist. The label travels through to the inbox-card "paired as" string. |
| `/status <run_id>` | Render a `RunStatusSnapshot` for the given run (active node, last outcome, elapsed time). Use `/runs` to discover ids. |
| `/runs` | List recent runs newest-first. |
| `/run <archetype>` | Start a fresh run from a bundled or user archetype. Wraps `Engine::start_run` via `ArchetypeRegistry::resolve`. |
| `/abort <run_id>` | Request graceful cancellation. Reason embeds the originating chat id. |
| `/snooze <duration>` | Reply to a cockpit card with `/snooze 30m` (or `2h`, `1d`, `90s`) to defer it. The card edits back to active state once the timer elapses. See [Snooze](#snooze) below. |
| `/feedback <run_id> <text>` | Keyboard-less edit feedback path. Equivalent to tapping `✏ Edit` and replying. |

Logs target `telegram::cmd::<name>` (DEBUG on dispatch, INFO on visible
state transitions, WARN on engine refusal).

## Card kinds

Every engine event maps to at most one card action (Decision 11).

| Event | Card kind | Buttons |
|-------|-----------|---------|
| `HumanInputRequested` | `human_gate` | Approve / Edit / Reject |
| `BootstrapApprovalRequested` | `bootstrap_<stage>` | Approve / Edit / Redo |
| `BootstrapApprovalDecided` | finalize the same card | (cleared) |
| `RunCompleted` | `completion` | Acknowledge |
| `RunFailed` | `failure` | Acknowledge |
| `EscalationRequested` | `escalation` | Acknowledge |
| `PipelineMaterialized` | finalize the bootstrap-Flow card | (cleared) |
| `StageEntered` | single `status` card per run (edit in place) | (none) |

The cockpit subscribes to a single engine-tap broadcast
(`RunEventTap`) and routes each event through `cockpit::dispatch`. One
event → one `editMessageText` (or one `sendMessage` for the very first
card per `(run_id, node_key, attempt_index)` triple).

## Callback wire format

ADR 0010 fixes the schema: `cockpit:<verb>:<card_id>`.

- `<verb>` is one of `approve`, `edit`, `reject`, `abort`, `snooze`, `ack`.
- `<card_id>` is the cockpit card's ULID (PK of `telegram_cards`).
- The inbox subsystem keeps the legacy `inbox:*` namespace untouched.

The callback router (`cockpit::callback`) parses the payload, runs
admission against `telegram_pairings`, looks the card up by id, and:

- `approve` / `edit` / `reject` → forwards to
  `Engine::resolve_human_input` (the same entry point the CLI
  `surge bootstrap` uses — there is no second mechanism, see ADR 0009).
- `ack` → records an INFO log; no engine call.
- `snooze` / `abort` (callback path) → currently emit `NotImplemented`
  outcomes; use `/snooze` / `/abort` commands instead.

Stale taps (card row missing or `closed_at` set) and admission denials
respond via `answerCallbackQuery` with a benign "no longer active" /
"chat not paired" message — no engine call, no error propagation
(Decision 14).

## Edit feedback (forced reply)

Two paths reach the same engine result:

1. **Inline**. Tap `✏ Edit`. The bot sends a `ForceReply` prompt;
   reply to it with your feedback. The reply lands as the `comment`
   field on `Engine::resolve_human_input(outcome=edit)`.
2. **Command**. `/feedback <run_id> <free text>`. No reply context
   needed. Produces the same JSON.

The forced-reply prompt's `message_id` is stored on the
`telegram_cards` row in `pending_edit_prompt_message_id` so the bot
can correlate the reply on arrival.

## Snooze

Reply to one of our cards with `/snooze <duration>` (`s` / `m` / `h` /
`d` units). The cockpit:

1. Resolves the target card via `reply_to_message.message_id`.
2. Inserts a row into `inbox_action_queue` keyed by
   `(subject_kind='cockpit_card', subject_ref=<card_id>,
   snooze_until=<wake_at>)` — same queue the legacy inbox snoozes use,
   discriminated by `subject_kind`.
3. The cockpit's `CockpitSnoozeRescheduler` polls the queue on a
   configurable interval (`config.inbox.snooze_poll_interval`,
   default 5min).
4. Once `snooze_until <= now`, the rescheduler issues a single
   `editMessageText` appending `🛏 Snooze ended — card is active
   again.` and marks the row processed.

The card itself never leaves the chat — the operator scrolls back to
it and acts when ready.

## Recovery (daemon restart)

`surge_telegram::cockpit::recover::reconcile_open_cards` runs on the
tap loop's `RecvError::Lagged` event (and once at startup via the
daemon supervisor). For every row with `closed_at IS NULL`:

- Resolve the originating run snapshot via `RunSnapshotProvider`.
- If the run has reached a terminal state ⇒ edit the card to its
  terminal body and close (`closed_at = now`).
- If the run is still active and the rendered body differs from
  `content_hash` ⇒ `editMessageText` to refresh.
- If the body hash matches ⇒ no Bot API call (Decision 8).

`reconcile_open_cards` **never** calls `sendMessage`. A daemon restart
in the middle of a gate produces zero duplicate cards.

## Rate limiting

`CockpitRateLimiter` (in `rate_limiter.rs`): token-bucket per
chat-id (1 update/sec sustained, burst 5) plus a global ceiling
(25/sec across all chats, leaving a margin against Telegram's
documented 30/sec soft limit). On HTTP 429 the limiter honours
`Retry-After` verbatim and retries once; a second failure surfaces
as `TelegramCockpitError::RateLimited` and is logged at WARN.

## Errors

The cockpit's outer loops log every error and continue
(`Decision 17`). Operators see chat-side messages only for explicit
actions: a failed `/pair` says why; transient transport errors stay
in the structured log under `telegram::*`.

Error variants live in
[`surge_telegram::TelegramCockpitError`](../crates/surge-telegram/src/error.rs):
`Auth`, `PairingTokenInvalid`, `PairingTokenExpired`, `CardNotFound`,
`CardClosed`, `RateLimited`, `Transport`, `EngineResolve`,
`Persistence`.

## Troubleshooting

| Symptom | Diagnostic | Fix |
|---------|------------|-----|
| `/pair` says "token not recognised" | Token mistyped or expired (10-minute default TTL) | Mint a fresh one with `surge telegram setup`. |
| Bot replies "This bot is not paired with this chat" to everything | Chat not in `telegram_pairings` | `/pair <token>` from THIS chat. |
| Cards stop updating after daemon restart | Restart-time reconcile triggers automatically. Check logs at `telegram::cockpit::recover` | If still stale, restart the daemon — startup pass runs every time. |
| Card buttons show "card no longer active" | Stale tap on a card whose `closed_at` is set | Expected; the gate already resolved. Inspect `surge intake list` or `surge daemon status` for the run state. |
| `surge doctor` reports "no telegram bot token" | Token was not persisted | Run `surge telegram setup`. |
| Rate-limit warnings in logs | A burst exceeded the per-chat or global bucket | Expected under heavy bootstrap activity. The cockpit retries inside `Retry-After`. |

## Webhook vs long-poll

Long-poll via `teloxide::update_listeners::polling_default` is the
default and the only path wired in this milestone. The webhook path
(`tiny_http`-backed listener) is **deferred** — the cockpit's update
loop is generic over `Stream<Item = Update>`, so the receiver type is
a one-line swap once the deployment story for an inbound webhook port
is on the roadmap.

## What is NOT in this milestone

- **Webhook receiver** (above).
- **Sandbox-elevation cards** — owned by the sandbox-delegation
  matrix milestone.
- **Multi-bot or multi-workspace cockpit** — single bot, single
  allowlist for now.
- **Group-chat support** — the cockpit assumes 1:1 chats. Group
  chat-ids land in the allowlist but no UX guarantees for >1 admin
  per chat.
- **Internationalization** — English-only strings.
- **Metrics / Prometheus exporter** — tracing INFO is the only
  observability layer.

## See also

- [Workflow](workflow.md) — how Telegram approvals fit into the
  full run lifecycle.
- [Bootstrap](bootstrap.md) — approval channels for the bootstrap
  three-stage.
- [CLI](cli.md) — `surge telegram setup / revoke / list` reference.
- [Tracker automation](tracker-automation.md) — how tracker-sourced
  runs feed into the cockpit.
- ADRs [0009](adr/0009-no-human-input-resolver-trait.md),
  [0010](adr/0010-telegram-callback-schema.md),
  [0011](adr/0011-telegram-card-lifecycle.md),
  [0012](adr/0012-surge-telegram-crate-split.md).
