# surge-notify

Pluggable notification delivery for `NodeKind::Notify` graph nodes.

## Overview

`surge-notify` decouples the `surge-orchestrator` engine from specific notification
channels. The core trait is [`NotifyDeliverer`]; the default implementation is
[`MultiplexingNotifier`], which dispatches each delivery to the appropriate
channel backend based on the node's `NotifyChannel` config.

## Channels

| Channel | Type | Feature gate |
|---|---|---|
| Desktop | OS native toast (via `notify-rust`) | always built |
| Webhook | HTTP POST (via `reqwest`) | always built |
| Slack | `chat.postMessage` API | always built |
| Email | SMTP via `lettre` | always built |
| Telegram | Bot API `sendMessage` | always built |

## Quick start

```rust
use surge_notify::{MultiplexingNotifier, WebhookDeliverer};
use std::sync::Arc;

let notifier = Arc::new(
    MultiplexingNotifier::new()
        .with_webhook(Arc::new(WebhookDeliverer::new()))
);
// Pass `notifier` to `Engine::new_with_notifier`.
```

## Template rendering

Notification message bodies support Mustache-style `{{variable}}` substitution.
Available context variables:

- `{{run_id}}` — UUID of the current run
- `{{node_id}}` — key of the Notify node
- `{{outcome}}` — outcome string that triggered this notification

## Design notes

- Each channel returns `NotifyError::ChannelNotConfigured` when the
  `MultiplexingNotifier` has no registered handler for that channel variant.
  The engine treats this as a non-fatal warning so runs are never blocked by
  missing notification credentials.
- Credentials for Slack/Telegram/Email are resolved via the `*SecretResolver`
  trait to allow test doubles without touching real endpoints.
- Desktop notifications require a running desktop session; they fail silently
  on headless CI with `NotifyError::DeliveryFailed`.

## M7+ roadmap

- `#[ignore]`d tests (`engine_m6_iterable_loop`, `engine_m6_loop_retry`, etc.)
  will be activated in M7/M8 as those engine features land.
- Rate-limiting and deduplication of repeat notifications is planned for M8.
