# RFC-001: Core ACP Integration

- **Status:** Draft
- **Created:** 2026-03-22
- **Author:** vanyastaff

## Мотивация

Surge строится на Agent Client Protocol (ACP) как единственном способе коммуникации с AI-агентами. Этот RFC определяет как именно Surge реализует ACP `Client` trait, управляет подключениями к агентам, и обеспечивает agent-agnostic выполнение.

## Контекст ACP

ACP — JSON-RPC протокол между editor (client) и agent (server). В нашем случае:
- **Client** = Surge (orchestrator)
- **Agent** = Claude Code CLI, Copilot CLI, и т.д.

ACP определяет два трейта:
- `Agent` — то, что реализует Claude Code / Copilot (мы вызываем)
- `Client` — то, что реализуем мы (агент вызывает нас для permissions, file ops, terminals)

## Дизайн

### 1. SurgeClient — реализация ACP Client

```rust
pub struct SurgeClient {
    /// Корень worktree для файловых операций
    worktree: Arc<Worktree>,

    /// Политика разрешений для tool calls
    permission_policy: PermissionPolicy,

    /// Канал для трансляции прогресса в UI/CLI
    event_tx: broadcast::Sender<SurgeEvent>,

    /// Менеджер терминалов
    terminals: Arc<TerminalManager>,

    /// Контекст текущей подзадачи (для фильтрации файлов)
    subtask_context: Option<SubtaskContext>,
}
```

**Ключевое решение:** один `SurgeClient` на subtask execution. Каждая подзадача получает свой инстанс с привязкой к worktree и контекстом подзадачи. Это позволяет:
- Ограничить файловые операции релевантными директориями
- Изолировать терминалы между подзадачами
- Применять разные permission policies

### 2. Permission Policy

```rust
pub enum PermissionPolicy {
    /// Автоматически разрешать всё (аналог --dangerously-skip-permissions)
    AutoApprove,

    /// Разрешать безопасные операции, блокировать опасные
    Smart {
        allow_read: bool,
        allow_write_in_worktree: bool,
        allow_bash_safe: bool,       // ls, cat, grep, cargo, npm
        deny_bash_dangerous: bool,   // rm -rf, sudo, curl | bash
        deny_network: bool,
    },

    /// Спрашивать пользователя для каждого tool call
    Interactive,
}
```

**По умолчанию:** `Smart` с разумными defaults. В отличие от Aperant, который использует `--dangerously-skip-permissions` для всего, Surge фильтрует опасные команды.

### 3. AgentConnection — управление процессом агента

```rust
pub struct AgentConnection {
    /// Имя агента из конфига
    name: String,

    /// ACP connection (предоставляет Agent trait)
    connection: ClientSideConnection,

    /// Child process (если stdio transport)
    process: Option<Child>,

    /// Активные сессии
    sessions: HashMap<SessionId, SessionState>,

    /// Capabilities агента (из initialize response)
    capabilities: AgentCapabilities,
}

impl AgentConnection {
    /// Запустить агента и установить ACP соединение
    pub async fn spawn(config: &AgentConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd.spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        // Создаём ACP connection через stdio transport
        let client = SurgeClient::new(...);
        let connection = ClientSideConnection::new(client, stdout, stdin);

        // ACP initialization handshake
        let init_response = connection.initialize(InitializeRequest {
            protocol_version: PROTOCOL_VERSION,
            capabilities: surge_client_capabilities(),
            client_info: ImplementationInfo {
                name: "surge".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        }).await?;

        Ok(Self {
            name: config.name.clone(),
            connection,
            process: Some(child),
            sessions: HashMap::new(),
            capabilities: init_response.capabilities,
        })
    }
}
```

### 4. AgentPool — мульти-агент менеджмент

```rust
pub struct AgentPool {
    /// Конфигурация доступных агентов
    configs: HashMap<String, AgentConfig>,

    /// Активные подключения (lazy — создаются при первом использовании)
    connections: RwLock<HashMap<String, AgentConnection>>,

    /// Default agent для задач без явного указания
    default_agent: String,
}

impl AgentPool {
    /// Получить или создать подключение к агенту
    pub async fn get_or_connect(&self, name: &str) -> Result<&AgentConnection> {
        let connections = self.connections.read().await;
        if connections.contains_key(name) {
            return Ok(&connections[name]);
        }
        drop(connections);

        let config = self.configs.get(name)
            .ok_or_else(|| SurgeError::AgentNotFound(name.to_string()))?;

        let conn = AgentConnection::spawn(config).await?;
        self.connections.write().await.insert(name.to_string(), conn);

        Ok(&self.connections.read().await[name])
    }

    /// Создать сессию с агентом
    pub async fn create_session(
        &self,
        agent_name: &str,
        mode: Option<&str>,
        working_dir: &Path,
    ) -> Result<SessionHandle> {
        let conn = self.get_or_connect(agent_name).await?;

        let response = conn.connection.new_session(NewSessionRequest {
            // ACP session params
        }).await?;

        // Если указан mode и агент его поддерживает
        if let Some(mode) = mode {
            if conn.capabilities.session.modes.contains(mode) {
                conn.connection.set_mode(SetModeRequest {
                    session_id: response.session_id.clone(),
                    mode: mode.to_string(),
                }).await?;
            }
        }

        Ok(SessionHandle {
            session_id: response.session_id,
            agent_name: agent_name.to_string(),
        })
    }

    /// Отправить промпт
    pub async fn prompt(
        &self,
        session: &SessionHandle,
        content: Vec<Content>,
    ) -> Result<PromptResponse> {
        let conn = self.get_or_connect(&session.agent_name).await?;
        conn.connection.prompt(PromptRequest {
            session_id: session.session_id.clone(),
            content,
            // ...
        }).await
    }

    /// Graceful shutdown всех агентов
    pub async fn shutdown(&self) {
        for (_, conn) in self.connections.write().await.drain() {
            conn.shutdown().await;
        }
    }
}
```

### 5. Agent Routing

Определение какой агент выполняет какую подзадачу:

```rust
pub struct AgentRouter {
    config: RoutingConfig,
}

#[derive(Deserialize)]
pub struct RoutingConfig {
    /// Агент по умолчанию
    pub default: String,

    /// Агент для фазы планирования
    pub planner: Option<String>,

    /// Агент для QA review
    pub qa_reviewer: Option<String>,

    /// Правила routing по паттернам файлов
    pub file_rules: Vec<FileRoutingRule>,
}

#[derive(Deserialize)]
pub struct FileRoutingRule {
    pub pattern: String,    // "*.rs", "*.tsx", "Cargo.toml"
    pub agent: String,      // "claude", "copilot"
}

impl AgentRouter {
    /// Определить агента для подзадачи
    pub fn resolve(&self, subtask: &Subtask) -> &str {
        // 1. Explicit agent в subtask spec
        if let Some(agent) = &subtask.agent {
            return agent;
        }

        // 2. File-based routing
        for file in &subtask.files {
            for rule in &self.config.file_rules {
                if glob_match(&rule.pattern, file) {
                    return &rule.agent;
                }
            }
        }

        // 3. Default
        &self.config.default
    }
}
```

### 6. Event System

Единый поток событий для UI/CLI:

```rust
#[derive(Debug, Clone)]
pub enum SurgeEvent {
    // Lifecycle
    TaskStateChanged { spec_id: SpecId, old: TaskState, new: TaskState },
    SubtaskStarted { spec_id: SpecId, subtask_id: SubtaskId, agent: String },
    SubtaskCompleted { spec_id: SpecId, subtask_id: SubtaskId, duration: Duration },
    SubtaskFailed { spec_id: SpecId, subtask_id: SubtaskId, error: String },

    // Agent communication
    AgentConnected { name: String, capabilities: AgentCapabilities },
    AgentDisconnected { name: String, reason: String },
    AgentMessage { session: SessionId, content: String },
    AgentToolCall { session: SessionId, tool: String, status: ToolCallStatus },
    AgentPlanUpdate { session: SessionId, plan: Vec<PlanEntry> },

    // File operations
    FileWritten { path: PathBuf },
    FileRead { path: PathBuf },

    // Git
    CommitCreated { worktree: String, message: String, hash: String },
    MergeCompleted { spec_id: SpecId, target: String },

    // QA
    QaStarted { spec_id: SpecId, iteration: u32 },
    QaResult { spec_id: SpecId, approved: bool, issues: Vec<String> },
}
```

## Нерешённые вопросы

1. **ACP Authentication** — как передавать OAuth-токены агентам? Пока через env vars при spawn, но ACP имеет `authenticate` метод.

2. **Remote agents** — ACP планирует HTTP/WebSocket transport. Нужно заложить абстракцию transport уже сейчас.

3. **Agent capability negotiation** — не все агенты поддерживают все ACP features. Нужен fallback для missing capabilities.

4. **Session persistence** — ACP имеет `load_session`. Нужно ли сохранять сессии между перезапусками Surge?

5. **Concurrent sessions к одному агенту** — поддерживает ли Claude Code / Copilot CLI несколько ACP сессий одновременно?

## Альтернативы (отвергнутые)

### A: Прямое использование Claude Agent SDK
Привязка к одному агенту. Не соответствует миссии "any agent".

### B: Собственный subprocess protocol
Нестандартный, требует поддержки на стороне каждого агента. ACP уже решает эту проблему.

### C: HTTP API напрямую к Anthropic/OpenAI
Теряем tool use, file system, terminals — всю агентную функциональность. Пришлось бы реимплементировать весь Claude Code.
