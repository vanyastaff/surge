# Surge — Архитектура

## Обзор системы

```
┌──────────────────────────────────────────────────────────────┐
│                     Surge Application                        │
│                                                              │
│  ┌────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │  surge-ui   │  │  surge-cli   │  │   surge-daemon       │  │
│  │  (egui)     │  │  (headless)  │  │   (background)       │  │
│  └──────┬──────┘  └──────┬───────┘  └──────────┬───────────┘  │
│         │                │                      │              │
│  ┌──────┴────────────────┴──────────────────────┴───────────┐ │
│  │                  surge-orchestrator                        │ │
│  │  ┌──────────┐ ┌──────────┐ ┌────────┐ ┌───────────────┐  │ │
│  │  │ Planner  │ │  Coder   │ │   QA   │ │    Merger     │  │ │
│  │  └──────────┘ └──────────┘ └────────┘ └───────────────┘  │ │
│  └──────────────────────┬────────────────────────────────────┘ │
│                         │                                      │
│  ┌──────────────────────┴────────────────────────────────────┐ │
│  │                    surge-acp                               │ │
│  │         ACP Client — trait Client impl                     │ │
│  │  ┌─────────────┐ ┌────────────┐ ┌─────────────────────┐  │ │
│  │  │  Sessions   │ │   Tools    │ │  File System / PTY  │  │ │
│  │  └─────────────┘ └────────────┘ └─────────────────────┘  │ │
│  └──────────────────────┬────────────────────────────────────┘ │
└─────────────────────────┼──────────────────────────────────────┘
                          │ JSON-RPC / stdio / TCP
              ┌───────────┼───────────┐
              │           │           │
        Claude Code   Copilot     Zed Agent
           CLI         CLI        (future)
```

## Workspace (Cargo)

```
surge/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── surge-core/             # Типы, конфиги, ошибки, spec format
│   ├── surge-spec/             # Spec-система: создание, парсинг, валидация
│   ├── surge-acp/              # ACP Client impl — единая точка общения с агентами
│   ├── surge-orchestrator/     # Pipeline: plan → code → qa → merge
│   ├── surge-git/              # Worktree management, merge, diff (git2)
│   ├── surge-terminal/         # PTY management (portable-pty)
│   ├── surge-ui/               # GUI приложение (egui + eframe)
│   ├── surge-cli/              # CLI приложение (clap)
│   └── surge-daemon/           # Фоновый процесс для длительных задач
├── specs/                      # Шаблоны и примеры спеков
├── docs/                       # Документация
└── tests/                      # Интеграционные тесты
```

---

## Крейты в деталях

### surge-core

Фундаментальные типы, разделяемые всеми крейтами.

```rust
// Идентификаторы
pub struct SpecId(Ulid);
pub struct TaskId(Ulid);
pub struct SubtaskId(Ulid);
pub struct SessionId(String);  // ACP session ID

// Состояния задачи (FSM)
#[derive(Debug, Clone, PartialEq)]
pub enum TaskState {
    Draft,
    Planning,
    Planned { subtask_count: usize },
    Executing { completed: usize, total: usize },
    QaReview,
    QaFix { iteration: u32 },
    HumanReview,
    Merging,
    Completed,
    Failed { reason: String },
    Cancelled,
}

// Конфигурация
#[derive(Deserialize)]
pub struct SurgeConfig {
    pub default_agent: AgentRef,
    pub agents: HashMap<String, AgentConfig>,
    pub spec: SpecDefaults,
    pub git: GitConfig,
    pub ui: UiConfig,
}

#[derive(Deserialize)]
pub struct AgentConfig {
    pub command: String,           // "claude", "copilot", path
    pub args: Vec<String>,         // дополнительные аргументы
    pub transport: Transport,      // Stdio | Tcp { host, port }
    pub capabilities: Vec<String>, // фильтр возможностей
}
```

### surge-spec

Спецификация — центральный артефакт. Определяет что нужно сделать, как разбить на подзадачи, критерии приёмки.

```toml
# specs/012-add-auth/spec.toml
[spec]
id = "012-add-auth"
title = "Add OAuth2 authentication"
created = "2026-03-22T10:00:00Z"
complexity = "standard"  # simple | standard | complex
state = "planned"

[spec.description]
text = """
Implement OAuth2 authentication with Google and GitHub providers.
Users should be able to sign in, sign out, and have their session persist.
"""

[spec.acceptance_criteria]
must = [
    "OAuth2 flow works with Google provider",
    "OAuth2 flow works with GitHub provider",
    "Session persists across page reloads",
    "Sign out clears session completely",
]
should = [
    "Error messages are user-friendly",
    "Loading states during auth flow",
]

[[spec.subtasks]]
id = "012-1"
title = "Setup OAuth2 dependencies and config"
state = "pending"
agent = "default"  # или конкретный: "claude", "copilot"
files = ["Cargo.toml", "src/auth/mod.rs", "src/config.rs"]
depends_on = []

[[spec.subtasks]]
id = "012-2"
title = "Implement Google OAuth2 provider"
state = "pending"
files = ["src/auth/google.rs", "src/auth/callback.rs"]
depends_on = ["012-1"]

[[spec.subtasks]]
id = "012-3"
title = "Implement GitHub OAuth2 provider"
state = "pending"
files = ["src/auth/github.rs"]
depends_on = ["012-1"]
parallel_with = ["012-2"]  # можно выполнять параллельно

[[spec.subtasks]]
id = "012-4"
title = "Session management and persistence"
state = "pending"
files = ["src/auth/session.rs", "src/middleware/auth.rs"]
depends_on = ["012-2", "012-3"]
```

Ключевые возможности surge-spec:
- **TOML-формат** — человекочитаемый, git-friendly, типизированный
- **Граф зависимостей** — автоматическое определение параллелизма
- **Agent routing** — разные подзадачи могут идти к разным агентам
- **State machine** — каждая подзадача имеет явное состояние

### surge-acp

Ядро интеграции с агентами через Agent Client Protocol.

```rust
use agent_client_protocol::{Client, Agent, ClientSideConnection};

/// Surge реализует ACP Client trait —
/// предоставляет агенту доступ к файлам, терминалам, permissions
pub struct SurgeClient {
    config: SurgeConfig,
    worktree_root: PathBuf,
    permission_policy: PermissionPolicy,
    progress_tx: broadcast::Sender<ProgressEvent>,
}

#[async_trait]
impl Client for SurgeClient {
    async fn request_permission(&self, req: RequestPermissionParams)
        -> Result<RequestPermissionResponse>
    {
        match self.permission_policy {
            PermissionPolicy::AutoApprove => Ok(approved()),
            PermissionPolicy::DenyDangerous => self.check_safety(&req),
            PermissionPolicy::AskUser => self.prompt_user(&req).await,
        }
    }

    async fn session_update(&self, notification: SessionUpdateNotification)
        -> Result<()>
    {
        // Трансляция прогресса в UI/CLI
        self.progress_tx.send(notification.into())?;
        Ok(())
    }

    async fn write_text_file(&self, req: WriteTextFileParams)
        -> Result<WriteTextFileResponse>
    {
        // Запись файлов только в worktree, не в main
        let path = self.worktree_root.join(&req.path);
        self.validate_path(&path)?;
        tokio::fs::write(&path, &req.content).await?;
        Ok(WriteTextFileResponse {})
    }

    async fn create_terminal(&self, req: CreateTerminalParams)
        -> Result<CreateTerminalResponse>
    {
        // Создание PTY в контексте worktree
        self.terminal_manager.create(req, &self.worktree_root).await
    }

    // ... остальные методы Client trait
}

/// Менеджер подключений к агентам
pub struct AgentPool {
    connections: HashMap<String, AgentConnection>,
}

impl AgentPool {
    /// Подключить агента по имени из конфига
    pub async fn connect(&mut self, name: &str) -> Result<&dyn Agent> { ... }

    /// Создать сессию с конкретным агентом
    pub async fn new_session(&self, agent: &str, mode: SessionMode)
        -> Result<SessionId> { ... }

    /// Отправить промпт агенту
    pub async fn prompt(&self, session: SessionId, prompt: PromptRequest)
        -> Result<PromptResponse> { ... }
}
```

### surge-orchestrator

Мозг системы. Управляет полным жизненным циклом задачи.

```rust
pub struct Orchestrator {
    agent_pool: AgentPool,
    spec_store: SpecStore,
    git: GitManager,
    config: SurgeConfig,
}

impl Orchestrator {
    /// Полный пайплайн выполнения задачи
    pub async fn execute(&self, spec_id: SpecId) -> Result<ExecutionReport> {
        let spec = self.spec_store.load(spec_id)?;

        // Phase 1: Planning (если subtasks ещё не определены)
        if spec.subtasks.is_empty() {
            let plan = self.plan(&spec).await?;
            self.spec_store.update_plan(spec_id, plan)?;
        }

        // Phase 2: Setup worktree
        let worktree = self.git.create_worktree(&spec)?;

        // Phase 3: Execute subtasks (с учётом зависимостей и параллелизма)
        let executor = SubtaskExecutor::new(&self.agent_pool, &worktree);
        let results = executor.execute_graph(&spec.subtasks).await?;

        // Phase 4: QA Validation
        let qa_result = self.qa_review(&spec, &worktree).await?;

        // Phase 5: Handle QA result
        match qa_result {
            QaResult::Approved => {
                self.spec_store.set_state(spec_id, TaskState::HumanReview)?;
            }
            QaResult::NeedsFix(issues) => {
                self.qa_fix_loop(&spec, &worktree, issues).await?;
            }
        }

        Ok(ExecutionReport { spec_id, results, qa_result })
    }
}

/// Исполнитель подзадач с параллелизмом
pub struct SubtaskExecutor<'a> {
    agent_pool: &'a AgentPool,
    worktree: &'a Worktree,
}

impl SubtaskExecutor<'_> {
    /// Выполняет граф подзадач с учётом зависимостей
    pub async fn execute_graph(&self, subtasks: &[Subtask]) -> Result<Vec<SubtaskResult>> {
        let graph = DependencyGraph::build(subtasks)?;

        for batch in graph.topological_batches() {
            // Параллельное выполнение независимых подзадач
            let futures: Vec<_> = batch.iter()
                .map(|subtask| self.execute_one(subtask))
                .collect();

            let results = futures::future::join_all(futures).await;

            // Коммит после каждого batch
            for result in &results {
                if let Ok(r) = result {
                    self.worktree.commit(&r.message)?;
                }
            }
        }

        Ok(results)
    }
}
```

### surge-git

Управление git worktrees — изоляция каждой задачи.

```rust
pub struct GitManager {
    repo: git2::Repository,
    config: GitConfig,
}

impl GitManager {
    /// Создать изолированный worktree для задачи
    pub fn create_worktree(&self, spec: &Spec) -> Result<Worktree> {
        let branch = format!("surge/{}", spec.id);
        let path = self.config.worktree_dir.join(&spec.id.to_string());
        // git worktree add -b surge/012-add-auth .worktrees/012-add-auth
        ...
    }

    /// Merge worktree в target branch
    pub fn merge(&self, worktree: &Worktree, target: &str) -> Result<MergeResult> { ... }

    /// Получить diff для review
    pub fn diff(&self, worktree: &Worktree) -> Result<String> { ... }

    /// Удалить worktree
    pub fn discard(&self, worktree: &Worktree) -> Result<()> { ... }
}
```

### surge-ui (egui)

Нативный GUI без web-технологий.

Основные экраны:
1. **Dashboard** — Kanban-доска задач (Draft → Planning → Executing → Review → Done)
2. **Spec Editor** — создание и редактирование спецификаций
3. **Execution Monitor** — real-time логи, прогресс подзадач, граф зависимостей
4. **Agent Manager** — подключение/отключение агентов, выбор для задач
5. **Diff Viewer** — review изменений перед merge
6. **Terminal Panel** — встроенные терминалы worktrees

### surge-cli

Headless режим для CI/CD и терминальных workflow.

```bash
# Инициализация проекта
surge init

# Создание спецификации
surge spec create "Add OAuth2 authentication"
surge spec create --from-issue github:42
surge spec create --interactive

# Планирование
surge plan 012-add-auth
surge plan 012-add-auth --agent copilot  # конкретный агент для планирования

# Выполнение
surge run 012-add-auth
surge run 012-add-auth --agent claude --parallel 3
surge run 012-add-auth --subtask 012-2  # только одна подзадача

# Мониторинг
surge status
surge status 012-add-auth
surge logs 012-add-auth --follow

# Review и merge
surge review 012-add-auth
surge diff 012-add-auth
surge merge 012-add-auth
surge discard 012-add-auth

# Управление агентами
surge agent list
surge agent add claude --command "claude" --transport stdio
surge agent add copilot --command "copilot" --transport stdio
surge agent test claude
```

---

## Ключевые зависимости (Rust crates)

| Крейт | Назначение |
|--------|-----------|
| `agent-client-protocol` | Официальный ACP SDK |
| `tokio` | Async runtime |
| `git2` | Git operations (worktrees, merge, diff) |
| `portable-pty` | PTY для терминалов |
| `egui` + `eframe` | Нативный GUI |
| `clap` | CLI парсинг |
| `serde` + `toml` | Конфигурация и spec файлы |
| `notify` | Filesystem watcher (PAUSE, HUMAN_INPUT) |
| `tracing` | Structured logging |
| `ulid` | Unique IDs для spec/task/subtask |
| `petgraph` | Граф зависимостей подзадач |

---

## Потоки данных

### Выполнение подзадачи

```
1. Orchestrator берёт subtask из очереди
2. Определяет агента (из subtask.agent или default)
3. AgentPool.connect(agent_name) — запускает CLI процесс если не запущен
4. AgentPool.new_session() — создаёт ACP сессию с нужным mode
5. Формирует промпт с контекстом:
   - spec.toml (полная спецификация)
   - subtask описание и acceptance criteria
   - список файлов для модификации
   - CLAUDE.md / AGENTS.md проекта
6. AgentPool.prompt() — отправляет через ACP
7. SurgeClient получает session_update нотификации:
   - Прогресс → UI
   - Tool calls → permission check
   - File writes → worktree
   - Terminal creates → PTY manager
8. По завершении — git commit в worktree
9. Обновление состояния subtask → next batch
```

### QA цикл

```
1. Все подзадачи выполнены
2. Orchestrator запускает QA сессию (может быть другой агент!)
3. QA-агент получает: spec + acceptance criteria + diff
4. Результат: Approved | NeedsFix(issues)
5. Если NeedsFix — создаём QA-fix подзадачи
6. Цикл повторяется (max 10 итераций, не 50 как в Aperant)
7. После 10 итераций — автоматический human review
```
