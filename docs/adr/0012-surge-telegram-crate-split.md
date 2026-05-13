+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-05-13"
+++

# ADR 0012 — `surge-telegram` crate, split from `surge-notify`

## Status

Accepted.

## Context

Surge already has a Telegram surface today: `surge-notify` ships a `TelegramDeliverer` that does outbound `sendMessage` calls for inbox cards and notify-fan-out events. The crate is structured as a **transport library**: small, no event loop, no long-running tasks, no callback receiver. Consumers (the orchestrator, the daemon, the CLI) pull it in for a single purpose — render a payload and send it out — and the crate is a leaf in the dependency tree.

The Telegram cockpit milestone needs substantially more than what `surge-notify` is sized for:

- A long-poll bot loop (or webhook receiver, deferred to a follow-up) that runs for the lifetime of the daemon.
- A callback router converting inline-keyboard taps into engine actions.
- A `BotCommand`-style command handler for `/run`, `/status`, `/abort`, `/runs`, `/snooze`, `/pair`, `/feedback`.
- A SQLite-backed card store (`telegram_cards` table, see [ADR 0011](0011-telegram-card-lifecycle.md)) for idempotency and recovery.
- A pairing-token / admission allowlist surface.
- A rate-limiter against Telegram Bot API quotas.
- A reconciler that runs on daemon startup to refresh open cards.

These pieces are needed by the daemon and only by the daemon. Putting them inside `surge-notify` would force every other consumer of `surge-notify` — short-lived CLI commands, the orchestrator's notify-fan-out, future binaries — to link the entire teloxide runtime, an unrelated SQLite schema, and a bot loop they have no use for.

The pattern of "long-running subsystem split out from a small transport library" already exists in Surge: the inbox subsystem lives in `surge-daemon::inbox`, not in `surge-notify`, for exactly the same reasons.

## Decision

The cockpit lives in a **new crate `surge-telegram`**, separate from `surge-notify`. `surge-notify` remains untouched in scope and unchanged in dependency profile.

### Boundary

`surge-notify` keeps its existing Telegram surface and gets **no** new code from this milestone:

- `TelegramDeliverer` — outbound `sendMessage`, used by notify-fan-out events.
- `TelegramSecretResolver` — interface for bot-token retrieval.
- `format_inbox_card`, `InboxCardRendered`, `InboxKeyboardButton` — primitives for rendering inline keyboards (already shipped, currently used only by the inbox subsystem; the cockpit reuses these types).

`surge-telegram` (new) owns:

- The `teloxide` Bot instance and its long-poll loop.
- Callback router (`cockpit::callback::dispatch`) and command handlers (`cockpit::commands::*`).
- The `telegram_cards`, `telegram_pairings`, `telegram_pairing_tokens` tables and their repositories (migrations live in `surge-persistence`).
- The card renderer (`surge-telegram::card::render`) that produces typed cards (`human_gate`, `bootstrap_<stage>`, `status`, `completion`, `failure`, `escalation`).
- The dispatch table mapping `EventPayload` variants to card actions (`surge-telegram::cockpit::dispatch`).
- The recovery reconciler (`surge-telegram::recover::reconcile_open_cards`).
- The rate limiter (`surge-telegram::rate_limiter::TokenBucket`).
- A separate `TelegramSecretResolver` implementation (cockpit-specific token, separate from `TelegramDeliverer`'s).

### Dependency direction

`surge-telegram` depends on: `surge-core`, `surge-notify` (for primitives only — `format_inbox_card` and `InboxKeyboardButton`), `surge-persistence` (for the cards/pairings repos), `surge-orchestrator` (for `Engine::resolve_human_input`, `RunEventTap`), plus `teloxide`, `tokio`, `rusqlite`, `serde`, `thiserror`, `tracing`.

`surge-notify` continues to depend on: nothing from the orchestrator side. It stays a leaf transport crate.

The daemon (`surge-daemon`) depends on `surge-telegram` to start the cockpit loop; nothing else in the workspace pulls `surge-telegram` in.

## Rationale

1. **Layering follows already-established Surge conventions.** Long-running, daemon-bound subsystems live in their own crates or under `surge-daemon::*`. Short-lived transports live in `surge-notify`. Mixing them violates the explicit dependency-direction rule from `.ai-factory/rules/base.md` ("Dependency direction is downward. `surge-core` is the leaf… Binaries depend on the workspace through stable trait surfaces. No cycles.").
2. **`teloxide` is heavy and unwanted by non-daemon consumers.** `teloxide` 0.13 pulls in `reqwest`, `tower`, `hyper`, `serde_json`, a polling executor, and ~80 transitive dependencies. Forcing this onto `surge-cli` (which runs for a few seconds and exits) is wasted compile time, wasted binary size, and a slower onboarding cycle.
3. **The cockpit's callback receiver needs a long-running tokio task.** `surge-notify` has no concept of "loops" — every method is `async fn deliver` and returns. Retrofitting a callback receiver onto `TelegramDeliverer` would mean introducing background tasks inside a transport library, which complicates teardown semantics for every consumer.
4. **Mirrors the inbox subsystem.** Surge already chose this layering for the inbox: outbound rendering primitives in `surge-notify`, polling/consumer/state machinery in `surge-daemon::inbox`. The cockpit follows the same pattern, scaled up to a full crate because its scope is larger than the inbox subsystem's.
5. **Separate `TelegramSecretResolver` instances enable independent tokens.** The cockpit token (`telegram.cockpit.bot_token`) is distinct from any future notify-channel token. Keeping the resolvers in separate crates lets the cockpit own its key namespace without touching `surge-notify`'s configuration surface.

## Alternatives considered

**Grow `surge-notify` to include the cockpit subsystems.** Rejected for all the reasons above: heavy dependencies leaking into short-lived consumers, layering inversion, and dilution of `surge-notify`'s "transport library" identity.

**Put the cockpit inside `surge-daemon`.** Considered briefly because the cockpit only runs in the daemon. Rejected because (a) the daemon already aggregates many subsystems (inbox, intake completion, run scheduling) and adding the cockpit as another inline module would push the crate's size and compile time up; (b) unit-testing the cockpit's callback router and command handlers in isolation is cleaner when they live in their own crate with their own `tests/` directory; (c) the cockpit may eventually be reused by other binary surfaces (a webhook receiver, a TUI debugger) — keeping it library-shaped from day one preserves that option.

**Split into multiple smaller crates (e.g. `surge-telegram-core`, `surge-telegram-bot`, `surge-telegram-recovery`).** Rejected as premature. The current cockpit scope (~30 milestone tasks) fits comfortably in one crate without exceeding readable file counts or compile-time concerns. If specific submodules grow independently (e.g. the rate limiter becomes generally useful), they can be extracted later without a breaking-API event.

## Consequences

- New workspace member `crates/surge-telegram/`. Standard layout: `Cargo.toml`, `src/lib.rs`, submodules under `src/`, integration tests under `tests/`.
- `surge-daemon` gains a single dependency on `surge-telegram` and a single call site that starts the cockpit loop (passed an `Arc<Engine>`, an `Arc<NotifyMultiplexer>`, and an `Arc<TelegramCardsRepo>`).
- `surge-notify` consumers (orchestrator, CLI) see no change. Their dependency trees are not touched by this milestone.
- The `telegram_cards`, `telegram_pairings`, `telegram_pairing_tokens` SQL migrations live in `surge-persistence/src/migrations/` (canonical location for all schema). The repository wrappers (`TelegramCardsRepo`, `TelegramPairingsRepo`) live in `surge-persistence/src/telegram/`. `surge-telegram` consumes the repos via the `surge-persistence` public API — no direct SQL.
- Documentation: a new `docs/telegram.md` covers cockpit setup, pairing flow, and operator commands. `docs/cli.md` documents `surge telegram setup`. The two crates' boundaries are described in `docs/ARCHITECTURE.md`.

## Revisit conditions

Reopen this ADR when **any** of the following becomes true:

- A new channel arrives (Slack, Discord, an in-house webhook) and the cockpit-shaped responsibilities (callback receiver, command router, card lifecycle, recovery, rate-limit) are clearly worth abstracting into a shared `surge-channels` crate that `surge-telegram` and `surge-slack` both depend on.
- `surge-telegram`'s scope grows beyond what a single crate can readably hold (clear signal: file count blows past ~30 modules or the crate's compile time becomes a top-3 contributor to the workspace build).
- A binary other than `surge-daemon` needs to embed the cockpit (e.g., a standalone `surge-bot` process). The decision then is whether to keep `surge-telegram` library-shaped or extract a smaller `surge-telegram-core` shared between binaries.
