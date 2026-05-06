# Component · Telegram Bot

## Overview

Implementation specification for the Telegram bot service. Built with **teloxide**.

This document specifies the bot's process structure, message handlers, callback routing, and integration with the storage layer. Complements RFC-0007.

## Process model

The bot runs as a **singleton daemon process** (`surge-tg`), independent from per-run engine daemons. Started:
- Automatically on first `surge run` invocation
- Manually via `surge telegram start`
- Auto-restart on crash (via systemd-style supervisor or simple loop)

Process lifecycle:
1. Connect to Telegram Bot API (long-poll or webhook)
2. Subscribe to event log for `ApprovalRequested` events (cross-run)
3. Loop: handle outgoing approvals + incoming user actions
4. On shutdown: gracefully complete in-flight Telegram operations, close DB

## Module structure

```
crates/telegram/src/
├── main.rs                  (binary entry point)
├── lib.rs                   (re-exports)
├── service.rs               (BotService struct, main loop)
├── outgoing/
│   ├── mod.rs               (queue consumer)
│   ├── card_builder.rs      (build approval card content)
│   ├── render/
│   │   ├── description.rs
│   │   ├── roadmap.rs
│   │   ├── flow.rs
│   │   ├── human_gate.rs
│   │   ├── elevation.rs
│   │   ├── progress.rs
│   │   ├── completion.rs
│   │   └── failure.rs
│   └── delivery.rs          (send via teloxide)
├── incoming/
│   ├── mod.rs               (callback_query + message handlers)
│   ├── callbacks.rs         (button taps)
│   ├── commands.rs          (slash commands)
│   └── replies.rs           (free-text replies)
├── secrets/
│   ├── filter.rs            (redact API keys, tokens)
│   └── patterns.rs          (regex patterns for detection)
├── setup.rs                 (binding flow)
└── state.rs                 (BotState, Pending message tracking)
```

## Service struct

```rust
pub struct BotService {
    bot: Bot,                                       // teloxide bot handle
    storage: Arc<Storage>,
    config: BotConfig,
    state: Arc<BotState>,
}

pub struct BotState {
    // Tracking sent message IDs per approval to support edit_message after decision
    sent_messages: RwLock<HashMap<ApprovalKey, MessageId>>,
    // Setup tokens awaiting binding
    pending_bindings: RwLock<HashMap<String, BindingRequest>>,
}

#[derive(Hash, Eq, PartialEq)]
pub struct ApprovalKey {
    pub run_id: RunId,
    pub event_seq: u64,
}

impl BotService {
    pub async fn run(self) -> Result<()> {
        let outgoing_task = tokio::spawn(self.clone().outgoing_loop());
        let incoming_task = tokio::spawn(self.clone().incoming_loop());
        
        tokio::select! {
            r = outgoing_task => r??,
            r = incoming_task => r??,
        }
        Ok(())
    }
    
    async fn outgoing_loop(self) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            self.process_pending_approvals().await?;
        }
    }
    
    async fn incoming_loop(self) -> Result<()> {
        // teloxide dispatcher
        let handler = dptree::entry()
            .branch(Update::filter_message().endpoint(Self::handle_message))
            .branch(Update::filter_callback_query().endpoint(Self::handle_callback));
        
        Dispatcher::builder(self.bot.clone(), handler)
            .dependencies(dptree::deps![self.storage.clone(), self.state.clone()])
            .build()
            .dispatch()
            .await;
        Ok(())
    }
}
```

## Outgoing flow

### Polling for pending approvals

Every 500ms, query storage for `pending_approvals` that haven't been delivered:

```rust
async fn process_pending_approvals(&self) -> Result<()> {
    let pending = self.storage.list_undelivered_approvals().await?;
    
    for approval in pending {
        match self.deliver_approval(&approval).await {
            Ok(message_id) => {
                self.storage.mark_approval_delivered(&approval, message_id).await?;
                self.state.sent_messages.write().await.insert(
                    ApprovalKey { run_id: approval.run_id.clone(), event_seq: approval.seq },
                    message_id,
                );
            }
            Err(e) if matches!(e, BotError::RateLimited(_)) => {
                // Skip this round, retry next
                tracing::warn!("Rate limited, retrying later");
                break;
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deliver approval");
                self.storage.record_approval_delivery_failure(&approval, e).await?;
            }
        }
    }
    Ok(())
}
```

### Building approval cards

Card content is built per approval type. Example for bootstrap Description:

```rust
async fn build_description_card(
    storage: &Storage,
    approval: &ApprovalRequested,
) -> Result<CardContent> {
    let run_handle = storage.open_run(&approval.run_id).await?;
    let description_artifact = run_handle.read_artifact_by_name("description.md").await?;
    let summary = extract_summary_from_markdown(&description_artifact)?;
    
    let body = format!(
        "▸ Description ready · 1 / 3\n\n\
         > {}\n\n\
         stack: {}\n\
         target: {}\n\
         crate: {}",
        summary.goal,
        summary.stack.join(" · "),
        summary.target,
        summary.crate_name,
    );
    
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("✓ Approve", make_callback("approve", approval)),
            InlineKeyboardButton::callback("✎ Edit", make_callback("edit", approval)),
            InlineKeyboardButton::callback("✕ Reject", make_callback("reject", approval)),
        ],
        vec![
            InlineKeyboardButton::url(
                "View full description",
                local_artifact_url(&approval.run_id, "description.md"),
            ),
        ],
    ]);
    
    Ok(CardContent { body, keyboard })
}

fn make_callback(action: &str, approval: &ApprovalRequested) -> String {
    serde_json::to_string(&CallbackData {
        v: 1,
        rid: approval.run_id.short().to_string(),
        ev: approval.seq,
        act: action.to_string(),
    }).unwrap()
}
```

### Delivery

```rust
async fn deliver_approval(&self, approval: &ApprovalRequested) -> Result<MessageId> {
    let card = self.build_card_for(approval).await?;
    
    // Filter for secrets
    let filtered_body = secrets::filter::redact(&card.body);
    
    // Send via Telegram
    let message = self.bot.send_message(self.config.chat_id, filtered_body)
        .reply_markup(card.keyboard)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
    
    Ok(message.id)
}
```

> **MarkdownV2 escaping note.** The `card.body` builder is responsible
> for escaping every character with special meaning in MarkdownV2
> (`_ * [ ] ( ) ~ \` > # + - = | { } . !`) inside any field that comes
> from arbitrary user/run data — `summary.goal`, `summary.target`, file
> paths, agent output snippets, etc. Use
> `teloxide::utils::markdown::escape` (or equivalent) when interpolating
> those into the template. Failure to escape causes silent message
> rejection from Telegram (`Bad Request: can't parse entities`) and
> drops the approval card entirely. The card builder is the right place
> for this, not the delivery function — by the time we reach
> `deliver_approval` the body is treated as a fully-formed MarkdownV2
> document.

## Incoming flow

### Callback query handler

When user taps an inline button:

```rust
async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<Storage>,
    state: Arc<BotState>,
) -> ResponseResult<()> {
    let data: CallbackData = match q.data.as_deref().and_then(|d| serde_json::from_str(d).ok()) {
        Some(d) => d,
        None => {
            bot.answer_callback_query(q.id).text("Invalid action").await?;
            return Ok(());
        }
    };
    
    let run_id = storage.resolve_short_id(&data.rid).await?;
    let approval_event = storage.read_event(&run_id, data.ev).await?;
    
    // Determine outcome + which event payload this action writes.
    // Approval-gate buttons → ApprovalDecided.
    // Sandbox-elevation buttons → SandboxElevationDecided.
    // Bootstrap-stage buttons (description / roadmap / flow) →
    // BootstrapStageDecided. The spec keeps these on different
    // payloads so projection / replay can distinguish a gate
    // approval ("ship the PR") from a one-off sandbox grant
    // ("let this stage write outside the worktree once").
    let decision_event = match data.act.as_str() {
        "approve" | "reject" | "edit" => EventPayload::ApprovalDecided {
            gate: extract_gate_from(&approval_event),
            decision: data.act.clone(),
            channel: ApprovalChannel::Telegram,
            comment: None,
        },
        "allow_once" | "allow_remember" | "deny" => EventPayload::SandboxElevationDecided {
            request_id: extract_elevation_id_from(&approval_event),
            decision: data.act.clone(),
            channel: ApprovalChannel::Telegram,
        },
        "accept_bootstrap" | "regenerate_bootstrap" => EventPayload::BootstrapStageDecided {
            stage: extract_bootstrap_stage_from(&approval_event),
            decision: data.act.clone(),
            channel: ApprovalChannel::Telegram,
        },
        _ => return Err(invalid_action(&data.act)),
    };
    let outcome = data.act.clone(); // for the confirmation banner below
    storage.append_event(&run_id, decision_event).await?;
    
    // Update message: remove keyboard, append confirmation
    let message_id = q.message.as_ref().map(|m| m.id);
    if let Some(mid) = message_id {
        let confirmation = format!("\n\n✓ {} by you · {}", outcome, format_time(now()));
        let original_text = q.message.as_ref().and_then(|m| m.text()).unwrap_or("").to_string();
        bot.edit_message_text(q.from.id.chat_id(), mid, format!("{}{}", original_text, confirmation))
            .await?;
    }
    
    // Acknowledge
    bot.answer_callback_query(q.id).await?;
    Ok(())
}
```

### Slash command handler

```rust
async fn handle_message(
    bot: Bot,
    msg: Message,
    storage: Arc<Storage>,
    state: Arc<BotState>,
) -> ResponseResult<()> {
    let text = msg.text().unwrap_or("").trim();
    
    if text.starts_with('/') {
        return handle_command(bot, msg, text, storage, state).await;
    }
    
    if let Some(reply_to) = msg.reply_to_message() {
        return handle_reply(bot, msg, reply_to, storage, state).await;
    }
    
    // Default: help
    bot.send_message(msg.chat.id, HELP_TEXT).await?;
    Ok(())
}

async fn handle_command(...) -> ResponseResult<()> {
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let cmd = parts[0];
    let args = parts.get(1).copied().unwrap_or("");
    
    match cmd {
        "/start" => handle_start(bot, msg, args, state).await?,
        "/run" => handle_run(bot, msg, args, storage).await?,
        "/list" => handle_list(bot, msg, storage).await?,
        "/status" => handle_status(bot, msg, args, storage).await?,
        "/cancel" => handle_cancel(bot, msg, args, storage).await?,
        "/replay" => handle_replay(bot, msg, args, storage).await?,
        "/help" => bot.send_message(msg.chat.id, HELP_TEXT).await?,
        _ => bot.send_message(msg.chat.id, format!("Unknown command: {}", cmd)).await?,
    };
    Ok(())
}
```

### Reply handler (free-text)

When user replies to an approval card:

```rust
async fn handle_reply(
    bot: Bot,
    msg: Message,
    reply_to: &Message,
    storage: Arc<Storage>,
    state: Arc<BotState>,
) -> ResponseResult<()> {
    let approval_key = match find_approval_for_message(reply_to.id, &state).await {
        Some(k) => k,
        None => {
            bot.send_message(msg.chat.id, "Reply to an approval card to provide feedback.").await?;
            return Ok(());
        }
    };
    
    // Capture as edit feedback
    let comment = msg.text().unwrap_or("").to_string();
    storage.append_event(&approval_key.run_id, EventPayload::ApprovalDecided {
        gate: ...,
        decision: "edit".to_string(),
        channel: ApprovalChannel::Telegram,
        comment: Some(comment),
    }).await?;
    
    bot.send_message(msg.chat.id, "Feedback received. Re-running stage...").await?;
    Ok(())
}
```

## Setup flow

```rust
pub async fn setup(home: &Path) -> Result<()> {
    let bot = Bot::from_env_or_config().await?;
    
    // Generate ephemeral binding token
    let token = generate_token(); // 16 chars
    let me = bot.get_me().await?;
    let bot_username = me.username.as_ref().unwrap();
    
    let url = format!("https://t.me/{}?start={}", bot_username, token);
    
    println!("Open this link on your phone (or copy to browser):");
    println!("\n  {}\n", url);
    println!("After tapping 'Start', this will print 'Bound' and exit.");
    
    let state = BotState::new();
    state.pending_bindings.write().await.insert(
        token.clone(),
        BindingRequest { created_at: now(), token: token.clone() },
    );
    
    // Start temporary bot listener for /start <token>
    let (tx, mut rx) = mpsc::channel::<i64>(1);
    let handler = make_setup_handler(state.clone(), token.clone(), tx);
    
    let bot_clone = bot.clone();
    let dispatcher_task = tokio::spawn(async move {
        Dispatcher::builder(bot_clone, handler).build().dispatch().await;
    });
    
    let chat_id = tokio::time::timeout(Duration::from_secs(300), rx.recv()).await??;
    let chat_id = chat_id.ok_or_else(|| anyhow!("Setup channel closed"))?;
    
    // Save binding
    let mut config: UserConfig = read_or_default(home.join("config.toml"))?;
    config.telegram.chat_id = Some(chat_id);
    write_atomic(home.join("config.toml"), &toml::to_string(&config)?)?;
    
    println!("\n✓ Bound to chat {}", chat_id);
    println!("  Sending test message...");
    bot.send_message(ChatId(chat_id), "surge connected. You'll receive approval cards here.").await?;
    
    dispatcher_task.abort();
    Ok(())
}
```

## Long-poll vs webhook

Default: long-poll (no public IP needed).

```rust
async fn start_polling(bot: Bot, handler: ...) {
    Dispatcher::builder(bot, handler).build().dispatch().await;
}
```

Webhook (configured via `~/.surge/config.toml`):

```rust
async fn start_webhook(bot: Bot, handler: ..., webhook_url: &str, secret_token: &str) {
    let listener = teloxide::dispatching::update_listeners::webhooks::axum(
        bot.clone(),
        teloxide::dispatching::update_listeners::webhooks::Options::new(
            ([0, 0, 0, 0], 8080).into(),
            webhook_url.parse().unwrap(),
        ).secret_token(secret_token.to_string()),
    ).await.unwrap();
    
    Dispatcher::builder(bot, handler)
        .build()
        .dispatch_with_listener(listener, ...)
        .await;
}
```

## Secrets filtering

Before sending any message:

```rust
pub fn redact(text: &str) -> String {
    let mut result = text.to_string();
    
    for pattern in SECRET_PATTERNS.iter() {
        result = pattern.replace_all(&result, "[REDACTED]").to_string();
    }
    
    result
}

lazy_static! {
    static ref SECRET_PATTERNS: Vec<Regex> = vec![
        // OpenAI / Anthropic API keys
        Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap(),
        Regex::new(r"sk-ant-[a-zA-Z0-9-_]{20,}").unwrap(),
        // GitHub tokens
        Regex::new(r"ghp_[a-zA-Z0-9]{36,}").unwrap(),
        Regex::new(r"github_pat_[a-zA-Z0-9_]{82,}").unwrap(),
        // Generic high-entropy tokens (40+ char base64)
        Regex::new(r"\b[A-Za-z0-9+/]{40,}\b").unwrap(),
        // Lines containing common secret keywords (more aggressive)
        Regex::new(r"(?i)(api[_-]?key|secret|token|password)\s*[:=]\s*\S+").unwrap(),
    ];
}
```

Test fixtures verify >90% catch rate on common patterns without false positives on normal content.

## Rate limiting

Telegram Bot API limits:
- 30 messages/second per bot (across all chats)
- 1 message/second to the same chat (with brief bursts allowed)

The bot service implements client-side rate limiting:

```rust
struct RateLimiter {
    chat_buckets: RwLock<HashMap<ChatId, TokenBucket>>,
    global_bucket: TokenBucket,
}

impl RateLimiter {
    async fn acquire(&self, chat_id: ChatId) -> Result<()> {
        // Wait for both global and per-chat budget
        self.global_bucket.acquire(1).await?;
        let mut buckets = self.chat_buckets.write().await;
        let bucket = buckets.entry(chat_id).or_insert_with(|| TokenBucket::new(1, Duration::from_secs(1)));
        bucket.acquire(1).await?;
        Ok(())
    }
}
```

## Monitoring

Bot logs:
- All sent messages (with chat ID, content hash)
- All received callbacks (with action, run ID)
- Errors and rate-limit hits

Logs go to `~/.surge/logs/telegram.log` with rotation.

`surge telegram status` shows:
- Service uptime
- Messages sent/received
- Pending approvals queue size
- Last error

## Acceptance criteria

The Telegram bot is correctly implemented when:

1. Setup flow completes within 30 seconds end-to-end (CLI → user taps Start → CLI exits with success).
2. Approval cards arrive in chat within 5 seconds of `ApprovalRequested` event.
3. Button taps result in `ApprovalDecided` event within 2 seconds.
4. All slash commands work: `/start`, `/run`, `/list`, `/status`, `/cancel`, `/replay`, `/help`.
5. Free-text replies to approval cards correctly capture as edit feedback.
6. Secrets filtering catches >90% of common patterns in test fixtures.
7. Bot survives Telegram API rate-limiting (HTTP 429) by retrying with backoff.
8. Bot survives crash and restart without losing pending approvals (event-sourced recovery).
9. Webhook mode works correctly when configured.
10. Multi-run: 3+ concurrent runs send distinguishable cards without confusion.
