+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-05-13"
+++

# ADR 0009 — Reject `HumanInputResolver` trait abstraction

## Status

Accepted.

## Context

The Telegram cockpit milestone introduces a second channel that resolves `HumanGate` outcomes: the operator taps an inline-keyboard button in Telegram, the cockpit translates the callback into a JSON response, and the engine continues the paused run. The existing console-approval path already resolves the same gates from `surge-cli` by reading stdin and calling `Engine::resolve_human_input` directly (see [`crates/surge-cli/src/commands/bootstrap.rs`](../../crates/surge-cli/src/commands/bootstrap.rs)).

When two channels share a contract, the standard reflex is to extract a trait:

```rust
#[async_trait]
trait HumanInputResolver: Send + Sync {
    async fn resolve(&self, run_id: RunId, call_id: Option<String>, response: Value)
        -> Result<(), EngineError>;
}
```

…and have `Engine` hold an `Arc<dyn HumanInputResolver>`, with `ConsoleResolver` and `TelegramResolver` as the two implementations.

For Surge that abstraction is **wrong**. The engine method *is* the contract — both channels are clients of it, not implementations of a peer trait. Introducing the trait imposes cost without behavioral benefit.

## Decision

Surge does **not** introduce a `HumanInputResolver` trait, an `Arc<dyn …>` field on `Engine`, or any other indirection between the two channels and the engine. The `surge-cli` console path and the future `surge-telegram` cockpit path both call `Engine::resolve_human_input(run_id, call_id, response)` directly. This is enforced as a regression assertion in the Telegram cockpit milestone (`cargo grep` must find exactly two production call sites of `Engine::resolve_human_input`).

## Rationale

1. **Direction of the dependency.** The engine owns the gate-resolution state — the `gate_resolutions` and `tool_resolutions` maps keyed by `(RunId, call_id)` (see [`crates/surge-orchestrator/src/engine/engine.rs:683`](../../crates/surge-orchestrator/src/engine/engine.rs)). Any channel that wants to resolve a gate must reach **into** the engine. Pulling the engine method behind a trait inverts the dependency for no reason: the engine would hold an `Arc<dyn HumanInputResolver>` *that just calls a method on the engine* — a cycle dressed as polymorphism.
2. **Zero behavioral difference.** Both channels build the same `serde_json::Value` payload (`{"outcome": "...", "comment": "..."}`) and pass the same `(run_id, call_id)` keys. There is no per-channel branching the engine needs to dispatch — the trait would resolve to the same body each time. The `Send + Sync` and `async_trait` overhead would buy nothing.
3. **Mocking is not blocked.** Integration tests for the Telegram cockpit can drive the engine directly via `Engine::resolve_human_input` from inside the test, treating the cockpit's job as "convert a callback into the right arguments." The cockpit's translation logic is tested by checking *what arguments it passes*, not by mocking a trait that wraps the engine.
4. **YAGNI applies — for now.** If a third channel emerges later (Slack, an HTTP webhook endpoint, a desktop GUI process speaking a different IPC) and its translation logic is significantly different — for example, requiring per-channel rate-limiting or its own credential resolver layered on top of the engine call — the trait becomes worthwhile at that point. Adding it now to "be ready" pays a maintenance cost on every refactor without a counterparty using the abstraction.

## Alternatives considered

**`trait HumanInputResolver` with `ConsoleResolver` and `TelegramResolver` implementations.** Rejected per the rationale above: invents an interior interface around an engine method, costs `Arc<dyn …>` indirection on the hot path, and saves nothing in tests.

**`enum HumanInputChannel { Console, Telegram, … }` dispatched inside the engine.** Rejected for a different reason — the *engine* should not branch on which channel resolved a gate. The gate resolution is channel-agnostic; the channels are *clients*. Pushing channel awareness into the engine breaks the separation that lets each channel evolve independently.

**Make `Engine::resolve_human_input` accept a `&dyn ChannelMetadata` parameter** (so the engine can log which channel resolved each gate). Rejected as overreach for this milestone — if channel-tagged logging becomes valuable, the channels can emit their own `info!` lines with a `target` discriminator (`telegram::callback`, `cli::console`) before calling the engine. The event log already carries enough provenance via the `HumanInputResolved` event's `seq` and the channel's own structured logs.

## Consequences

- The cockpit's callback-translation logic lives in `surge-telegram::callback::dispatch` and ends with a direct call to `Engine::resolve_human_input`. No new public abstraction is exposed on `surge-orchestrator`.
- The console-approval path in `surge-cli` is untouched by this milestone.
- Regression assertion in the cockpit integration tests: `cargo grep` for `resolve_human_input(` finds exactly **two** production call sites (cockpit, console) and any number of test call sites. Adding a third production caller without revisiting this ADR is a review-time red flag.
- A future ADR may supersede this one when a third channel arrives with non-trivial per-channel logic. The condition for revisiting is concrete behavior divergence, not surface count.

## Revisit conditions

Reopen this ADR when **any** of the following becomes true:

- A third channel resolving `HumanGate` outcomes is added (Slack, webhook, desktop IPC) **and** its translation logic carries non-trivial per-channel state that does not belong in the engine.
- Per-channel observability (latency budgets, queue depth, retry policies) becomes a requirement that cannot be satisfied by channel-local logging and metrics.
- The engine grows a second resolve-style method (e.g., for elevation decisions) and the two methods become symmetric enough that a shared trait would deduplicate meaningful code.

Until one of these triggers fires, the engine surface stays the contract.
