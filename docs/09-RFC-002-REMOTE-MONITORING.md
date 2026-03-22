# RFC-002: Remote Monitoring & Messaging Integration

- **Status:** Draft
- **Created:** 2026-03-22
- **Author:** vanyastaff

## Мотивация

Surge может выполнять задачи часами. Пользователь не хочет сидеть перед экраном и ждать. Нужна возможность:

- Получать уведомления о статусе (started, completed, failed, needs review)
- Смотреть прогресс задач с телефона
- Давать команды удалённо (approve, cancel, skip, resume)
- Получать diff и summary завершённых задач
- Видеть расход токенов и стоимость

## Дизайн

### Архитектура

```
┌──────────────────────────────────────────────────┐
│                    Surge                          │
│                                                   │
│  ┌─────────────┐    ┌─────────────────────────┐  │
│  │ Orchestrator │───▶│    surge-notify          │  │
│  └─────────────┘    │                          │  │
│                     │  ┌────────────────────┐  │  │
│                     │  │  HTTP/WebSocket     │  │  │
│                     │  │  API Server         │──┼──┼──▶ Web Dashboard (future)
│                     │  └────────────────────┘  │  │
│                     │                          │  │
│                     │  ┌────────────────────┐  │  │
│                     │  │  Telegram Bot       │──┼──┼──▶ Telegram
│                     │  └────────────────────┘  │  │
│                     │                          │  │
│                     │  ┌────────────────────┐  │  │
│                     │  │  Discord Bot        │──┼──┼──▶ Discord
│                     │  └────────────────────┘  │  │
│                     │                          │  │
│                     │  ┌────────────────────┐  │  │
│                     │  │  Webhook            │──┼──┼──▶ Slack, n8n, Zapier
│                     │  └────────────────────┘  │  │
│                     │                          │  │
│                     │  ┌────────────────────┐  │  │
│                     │  │  System Tray        │──┼──┼──▶ Desktop Notifications
│                     │  └────────────────────┘  │  │
│                     └─────────────────────────┘  │
└──────────────────────────────────────────────────┘
```

### Крейт: surge-notify

```rust
/// Единый канал нотификаций
pub struct NotifyManager {
    channels: Vec<Box<dyn NotifyChannel>>,
    event_rx: broadcast::Receiver<SurgeEvent>,
}

/// Трейт для каждого канала
#[async_trait]
pub trait NotifyChannel: Send + Sync {
    /// Имя канала для конфигурации
    fn name(&self) -> &str;

    /// Отправить уведомление
    async fn send(&self, notification: &Notification) -> Result<()>;

    /// Получить входящие команды (для Telegram/Discord)
    async fn recv_command(&self) -> Option<RemoteCommand>;
}

pub struct Notification {
    pub level: NotifyLevel,       // Info, Warning, Error, Success
    pub title: String,
    pub body: String,
    pub spec_id: Option<SpecId>,
    pub actions: Vec<Action>,     // Кнопки для интерактивных каналов
}

pub enum NotifyLevel {
    Info,       // task started, subtask completed
    Success,    // task completed, QA approved
    Warning,    // rate limited, retrying
    Error,      // task failed, agent disconnected
    Review,     // needs human review (with action buttons)
}

/// Команды которые можно отправить удалённо
pub enum RemoteCommand {
    Approve { spec_id: SpecId },
    Cancel { spec_id: SpecId },
    Skip { spec_id: SpecId, subtask_id: SubtaskId },
    Resume { spec_id: SpecId },
    Status,                        // запросить текущий статус
    List,                          // список активных задач
    Cost,                          // текущие расходы
}
```

### Конфигурация

```toml
# surge.toml

[notify]
# Какие события отправлять
events = ["task_completed", "task_failed", "needs_review", "rate_limited"]

[notify.telegram]
enabled = true
bot_token = "env:SURGE_TELEGRAM_TOKEN"    # берём из env
chat_id = "env:SURGE_TELEGRAM_CHAT_ID"
commands = true                            # разрешить команды через бота

[notify.discord]
enabled = false
webhook_url = "env:SURGE_DISCORD_WEBHOOK"
commands = false                           # только уведомления

[notify.webhook]
enabled = false
url = "https://hooks.slack.com/..."
secret = "env:SURGE_WEBHOOK_SECRET"

[notify.api]
enabled = true
host = "127.0.0.1"
port = 9876
# Web dashboard будет доступен на http://localhost:9876
```

---

## Telegram Bot

### Уведомления

```
⚡ Surge — Task Started
━━━━━━━━━━━━━━━━━━━━━
📋 012-add-auth: Add OAuth2 authentication
🤖 Agent: Claude Code
📊 Subtasks: 0/6
⏱️ Estimated: ~15 min

──────────────────────

✅ Surge — Task Completed
━━━━━━━━━━━━━━━━━━━━━
📋 012-add-auth: Add OAuth2 authentication
🤖 Agents: Claude (plan), Copilot (code)
📊 Subtasks: 6/6 ✅
⏱️ Duration: 12 min 34 sec
💰 Cost: $0.42
📁 Files changed: 23 (+450, -234)

[🔍 View Diff]  [✅ Approve & Merge]  [❌ Discard]

──────────────────────

⚠️ Surge — Needs Review
━━━━━━━━━━━━━━━━━━━━━
📋 012-add-auth: Add OAuth2 authentication
🔍 QA found 2 issues:
  1. Missing error handling in callback.rs
  2. Session expiry not tested

[✅ Approve Anyway]  [🔄 Fix & Retry]  [❌ Cancel]

──────────────────────

🔴 Surge — Rate Limited
━━━━━━━━━━━━━━━━━━━━━
🤖 Claude Code: rate limit reached
⏳ Auto-resume in: ~47 minutes
📋 Paused tasks: 012-add-auth (subtask 4/6)
💡 Copilot is available — switch?

[🔄 Switch to Copilot]  [⏳ Wait]
```

### Команды

```
User: /status
Bot:  ⚡ Surge Status
      ━━━━━━━━━━━━━━━
      📋 Active tasks: 2
      ├─ 012-add-auth: Executing (4/6) 🟡
      └─ 013-fix-nav: QA Review 🔵

      🤖 Agents:
      ├─ Claude: 🟢 Online
      └─ Copilot: 🟡 Rate limited (resumes in 12 min)

      💰 Today: $1.23 (Claude: $0.98, Copilot: $0.25)

──────────────────────

User: /approve 012
Bot:  ✅ Task 012-add-auth approved!
      Merging into main...
      🔀 PR #42 created.

──────────────────────

User: /cancel 013
Bot:  ❌ Task 013-fix-nav cancelled.
      🗑️ Worktree cleaned up.

──────────────────────

User: /run "Fix the login page CSS"
Bot:  ⚡ Creating spec from description...
      📋 014-fix-login-css created (Simple, 3 subtasks)
      🤖 Assigned to: Copilot

      [▶️ Start]  [✏️ Edit Spec]  [❌ Cancel]

──────────────────────

User: /cost
Bot:  💰 Cost Report
      ━━━━━━━━━━━━━
      Today:     $2.45
      This week: $18.32
      This month: $67.89

      By agent:
      ├─ Claude:  $52.34 (78%)
      └─ Copilot: $15.55 (22%)

      By phase:
      ├─ Planning: $8.12 (12%)
      ├─ Coding:   $45.67 (67%)
      └─ QA:       $14.10 (21%)
```

---

## HTTP API / WebSocket Server

Для web dashboard и сторонних интеграций:

```
GET    /api/status                    # общий статус
GET    /api/tasks                     # список задач
GET    /api/tasks/:id                 # детали задачи
GET    /api/tasks/:id/logs            # логи
GET    /api/tasks/:id/diff            # diff
POST   /api/tasks/:id/approve         # approve
POST   /api/tasks/:id/cancel          # cancel
POST   /api/tasks                     # создать задачу из описания
GET    /api/agents                    # список агентов
GET    /api/cost                      # стоимость
WS     /ws/events                     # WebSocket stream событий
```

```rust
// Пример: axum server
pub async fn start_api_server(
    config: &ApiConfig,
    event_rx: broadcast::Receiver<SurgeEvent>,
    orchestrator: Arc<Orchestrator>,
) -> Result<()> {
    let app = Router::new()
        .route("/api/status", get(status_handler))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/:id", get(get_task))
        .route("/api/tasks/:id/approve", post(approve_task))
        .route("/api/tasks/:id/cancel", post(cancel_task))
        .route("/api/tasks/:id/diff", get(get_diff))
        .route("/api/agents", get(list_agents))
        .route("/api/cost", get(get_cost))
        .route("/ws/events", get(ws_events_handler))
        .with_state(AppState { orchestrator, event_rx });

    let listener = TcpListener::bind(&config.bind_addr()).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

Web dashboard (future Phase 7+) будет SPA подключённый к этому API.

---

## Mobile Scenario

Типичный сценарий:

1. 🖥️ Утром на компе: `surge run 012-add-auth` → уходишь на обед
2. 📱 В Telegram приходит: "⚡ Task needs review — QA found 1 issue"
3. 📱 Смотришь diff и issue описание прямо в Telegram
4. 📱 Отправляешь `/approve 012` — задача мёрджится
5. 📱 Получаешь: "✅ PR #42 created, worktree cleaned up"
6. 🖥️ Возвращаешься — всё чисто, PR на GitHub, ноль мусора

---

## Зависимости

| Крейт | Назначение |
|--------|-----------|
| `teloxide` | Telegram Bot API |
| `serenity` | Discord Bot API |
| `axum` | HTTP API server |
| `tokio-tungstenite` | WebSocket |
| `reqwest` | Webhook отправка |

---

## Фазы реализации

Это Phase 7+ feature, но закладываем архитектуру сейчас:

### Phase 0-3: Event foundation
- `SurgeEvent` enum уже в surge-core
- `broadcast::Sender<SurgeEvent>` уже в surge-acp
- Все компоненты шлют события → готово для подключения каналов

### Phase 7.1: surge-notify крейт + System Tray
- NotifyChannel trait
- Desktop notifications через system tray
- Базовый HTTP API (status, list tasks)

### Phase 7.2: Telegram Bot
- teloxide integration
- Уведомления + inline кнопки
- Команды: /status, /approve, /cancel, /cost

### Phase 7.3: HTTP API + WebSocket
- Полный REST API
- WebSocket event stream
- Web dashboard skeleton

### Phase 7.4: Discord + Webhooks
- Discord bot
- Generic webhook (Slack, n8n, Zapier)
