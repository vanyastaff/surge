# Surge — Tasks

> Задачи разбиты на мелкие таски (5-8 подзадач каждый) для стабильного выполнения в Auto-Claude.
> Выполняй по порядку. Каждый таск — отдельный task в Aperant.

---

## Phase 0: Foundation ✅

### Task 0.1: Доработка surge-core типов ✅
**Complexity:** Simple
**Files:** `crates/surge-core/src/`

- [x] Добавить `spec.rs` — структуры `Spec`, `Subtask`, `AcceptanceCriteria`, `Complexity` enum (simple/standard/complex)
- [x] Добавить `event.rs` — `SurgeEvent` enum для всех событий системы (TaskStateChanged, AgentConnected, SubtaskStarted, etc.)
- [x] Добавить `Serialize`/`Deserialize` для всех типов, покрыть тестами round-trip TOML сериализацию
- [x] Расширить `SurgeConfig` — добавить `RoutingConfig`, `CleanupPolicy`, `IdeConfig`
- [x] Добавить `surge-core/src/spec.rs` тест: загрузить пример `surge.example.toml`, проверить десериализацию
- [x] Обновить `lib.rs` — реэкспорт всех новых модулей

---

### Task 0.2: surge-acp — ACP Client trait implementation ✅
**Complexity:** Standard
**Files:** `crates/surge-acp/src/`

- [x] Создать `client.rs` — `SurgeClient` struct реализующий ACP `Client` trait
- [x] Реализовать `request_permission` — auto-approve mode (Smart policy как заглушка)
- [x] Реализовать `session_update` — логирование через `tracing` и отправка в `broadcast::Sender<SurgeEvent>`
- [x] Реализовать `write_text_file` / `read_text_file` — файловые операции с валидацией пути (только в worktree)
- [x] Реализовать `create_terminal` / `terminal_output` — заглушки, возвращающие `not_supported`
- [x] Создать `connection.rs` — `AgentConnection` struct: spawn CLI процесс, создание ACP connection через stdio
- [x] Добавить тесты: mock Agent trait, проверить что Client methods вызываются корректно
- [x] Обновить `lib.rs` — реэкспорт `SurgeClient`, `AgentConnection`

---

### Task 0.3: surge-acp — AgentPool и multi-agent ✅
**Complexity:** Simple
**Files:** `crates/surge-acp/src/`

- [x] Создать `pool.rs` — `AgentPool` struct с `HashMap<String, AgentConnection>` и lazy connect
- [x] Метод `get_or_connect(&self, name: &str)` — создаёт подключение при первом использовании
- [x] Метод `create_session` — создаёт ACP сессию с опциональным mode
- [x] Метод `prompt` — отправляет промпт в существующую сессию
- [x] Метод `shutdown` — graceful shutdown всех агентов
- [x] Обновить `lib.rs` — реэкспорт `AgentPool`

---

### Task 0.4: surge-cli — ping и prompt команды ✅
**Complexity:** Simple
**Files:** `crates/surge-cli/src/`

- [x] Реализовать `Commands::Ping` — загрузить конфиг, создать AgentPool, вызвать connect + initialize, вывести capabilities
- [x] Реализовать `Commands::Prompt` — создать сессию, отправить промпт, вывести streaming ответ
- [x] Реализовать `AgentCommands::List` — прочитать surge.toml, вывести список агентов
- [x] Реализовать `AgentCommands::Test` — ping конкретного агента по имени
- [x] Добавить `Commands::Init` — создать `surge.toml` в текущей директории с дефолтными значениями
- [x] Добавить цветной вывод через `colored` или ANSI codes

---

## Phase 1: Spec System ✅

### Task 1.1: surge-spec крейт — TOML формат ✅
**Complexity:** Simple
**Files:** `crates/surge-spec/`

- [x] Создать крейт `surge-spec` — добавить в workspace Cargo.toml
- [x] `spec.rs` — полная структура `SpecFile` с TOML сериализацией (spec metadata, description, acceptance_criteria, subtasks)
- [x] `parser.rs` — загрузка spec.toml из файла, валидация обязательных полей
- [x] `builder.rs` — `SpecBuilder` для программного создания спеков
- [x] `templates.rs` — встроенные шаблоны (feature, bugfix, refactor)
- [x] Тесты: round-trip TOML, валидация, builder pattern

---

### Task 1.2: surge-spec — валидация и граф зависимостей ✅
**Complexity:** Simple
**Files:** `crates/surge-spec/src/`

- [x] `validation.rs` — проверка: все depends_on ссылаются на существующие subtask id, нет циклов, файлы не пустые
- [x] `graph.rs` — построение графа зависимостей через `petgraph`, topological sort
- [x] `graph.rs` — метод `topological_batches()` — группировка подзадач в батчи для параллельного выполнения
- [x] Тесты: граф без зависимостей → один batch, линейная цепочка → N batches, параллельные → grouped
- [x] `graph.rs` — визуализация графа как ASCII (для `surge spec show`)

---

### Task 1.3: surge-cli — spec команды ✅
**Complexity:** Simple
**Files:** `crates/surge-cli/src/`

- [x] Добавить `Commands::Spec` с подкомандами: `create`, `list`, `show`, `validate`
- [x] `spec create "description"` — создать spec.toml интерактивно (промпт через агента для subtask generation)
- [x] `spec create --template feature` — создать из встроенного шаблона
- [x] `spec list` — вывести все спеки из `.surge/specs/` с состоянием
- [x] `spec show {id}` — вывести детали спека включая граф зависимостей (ASCII)
- [x] `spec validate {id}` — прогнать валидацию, вывести ошибки/предупреждения

---

## Phase 2: Git Worktrees ✅

### Task 2.1: surge-git крейт ✅
**Complexity:** Standard
**Files:** `crates/surge-git/`

- [x] Создать крейт `surge-git` — добавить в workspace, зависимость `git2`
- [x] `worktree.rs` — `GitManager` struct с методами для worktree lifecycle
- [x] Метод `create_worktree(spec_id)` — создать worktree в `.surge/worktrees/{id}`, ветка `surge/{id}`
- [x] Метод `commit(worktree, message)` — коммит всех изменений в worktree
- [x] Метод `diff(worktree)` — получить diff между worktree и base branch
- [x] Метод `merge(worktree, target_branch)` — merge worktree в target
- [x] Метод `discard(worktree)` — удалить worktree и ветку
- [x] Метод `list_worktrees()` — список активных worktrees с состоянием

---

### Task 2.2: surge-git — cleanup и CLI интеграция ✅
**Complexity:** Simple
**Files:** `crates/surge-git/src/`, `crates/surge-cli/src/`

- [x] `cleanup.rs` — `LifecycleManager`: обнаружение orphaned worktrees, stale branches
- [x] Метод `cleanup_orphaned()` — удалить worktrees без привязки к spec
- [x] Метод `cleanup_merged_branches()` — удалить merged ветки `surge/*`
- [x] CLI: `surge diff {spec_id}` — вывести diff
- [x] CLI: `surge merge {spec_id}` — merge с подтверждением
- [x] CLI: `surge discard {spec_id}` — discard с подтверждением
- [x] CLI: `surge clean` — интерактивная очистка (worktrees, branches, archive)

---

## Phase 3: Orchestrator MVP ✅

### Task 3.1: surge-orchestrator крейт — pipeline scaffold ✅
**Complexity:** Simple
**Files:** `crates/surge-orchestrator/`

- [x] Создать крейт `surge-orchestrator` — добавить в workspace
- [x] `pipeline.rs` — `Orchestrator` struct с методом `execute(spec_id)`
- [x] `planner.rs` — `PlannerPhase`: отправить spec агенту, получить subtask breakdown
- [x] `phases.rs` — enum `Phase` (Planning, Executing, QaReview, QaFix, HumanReview, Merging)
- [x] `context.rs` — `SubtaskContext`: формирование промпта для подзадачи (spec + subtask + relevant files)
- [x] Интеграция с `surge-acp::AgentPool` и `surge-git::GitManager`

---

### Task 3.2: surge-orchestrator — subtask execution ✅
**Complexity:** Standard
**Files:** `crates/surge-orchestrator/src/`

- [x] `executor.rs` — `SubtaskExecutor`: выполнение одной подзадачи через ACP
- [x] Формирование контекстного промпта: spec description + subtask details + acceptance criteria
- [x] Streaming progress через `SurgeEvent` канал
- [x] Коммит после каждой подзадачи в worktree
- [x] Обновление состояния subtask в spec.toml (pending → completed / failed)
- [x] Error handling: retry с backoff (max 3 attempts), skip с уведомлением
- [x] Circuit breaker: если 3 подзадачи подряд failed → pause pipeline

---

### Task 3.3: surge-orchestrator — QA loop ✅
**Complexity:** Simple
**Files:** `crates/surge-orchestrator/src/`

- [x] `qa.rs` — `QaReviewer`: отправить агенту spec + acceptance criteria + diff
- [x] Парсинг результата: Approved / NeedsFix(issues)
- [x] `qa.rs` — `QaFixer`: создание fix-подзадач из issues, выполнение через executor
- [x] QA цикл: review → fix → review, max iterations из конфига
- [x] После max iterations → автоматический переход в HumanReview
- [x] Логирование каждой QA итерации в spec history

---

### Task 3.4: surge-orchestrator — gates и human input ✅
**Complexity:** Simple
**Files:** `crates/surge-orchestrator/src/`, `crates/surge-cli/src/`

- [x] `gates.rs` — `GateManager`: check конфигурации gates, pause при необходимости
- [x] File watcher: `.surge/specs/{id}/PAUSE` файл → pause pipeline
- [x] File watcher: `.surge/specs/{id}/HUMAN_INPUT.md` → inject в следующий промпт
- [x] CLI: `surge run {spec_id}` — запуск полного pipeline
- [x] CLI: `surge status {spec_id}` — текущее состояние с прогрессом
- [x] CLI: `surge logs {spec_id} --follow` — streaming логи

---

## Phase 4: Parallel Execution ✅

### Task 4.1: Параллельные подзадачи ✅
**Complexity:** Standard
**Files:** `crates/surge-orchestrator/src/`

- [x] `parallel.rs` — `ParallelExecutor`: выполнение batch подзадач через `tokio::JoinSet`
- [x] Интеграция с `graph.rs`: topological batches → sequential execution of batches, parallel within batch
- [x] Конфигурируемый max_parallel из `PipelineConfig`
- [x] Merge результатов параллельных подзадач в worktree (обнаружение конфликтов)
- [x] Progress tracking: broadcast update при завершении каждой подзадачи
- [x] CLI: `surge run {spec_id} --parallel 3` — override max_parallel
- [x] Тесты: mock agent, проверить что независимые подзадачи реально идут параллельно

---

## Phase 5: Multi-Agent ✅

### Task 5.1: Agent routing ✅
**Complexity:** Simple
**Files:** `crates/surge-acp/src/`

- [x] `router.rs` — `AgentRouter`: определение агента для подзадачи
- [x] Routing priority: subtask.agent → file_rules → phase default → global default
- [x] `FileRoutingRule`: glob-паттерны для маппинга файлов на агентов
- [x] Phase routing: отдельные агенты для planner, coder, qa_reviewer
- [x] CLI: `surge agent add {name} --command {cmd}` — добавить агента в конфиг
- [x] CLI: `surge run {spec_id} --planner claude --coder copilot` — override routing

---

### Task 5.2: Agent fallback и health ✅
**Complexity:** Simple
**Files:** `crates/surge-acp/src/`

- [x] `health.rs` — `HealthMonitor`: отслеживание latency, error rate, rate limits
- [x] Fallback logic: если primary agent rate-limited/failed → переключение на fallback
- [x] Rate limit detection: 429 → auto-pause с estimated resume time
- [x] Auto-resume: background task проверяет rate limit reset, продолжает pipeline
- [x] CLI: `surge agent status` — вывести health всех агентов
- [x] Нотификация в event stream при переключении агента

---

## Phase 6: GUI (отдельные таски)

### Task 6.1: surge-ui scaffold
**Complexity:** Simple
**Files:** `crates/surge-ui/`

- [ ] Создать крейт `surge-ui` — добавить в workspace, зависимости `eframe`, `egui`
- [ ] Базовое окно с sidebar навигацией (список из FEATURES.md)
- [ ] Тема: тёмная, цвета из FEATURES.md
- [ ] Роутинг между панелями через enum `Panel`
- [ ] Подключение к `surge-core` для типов
- [ ] Placeholder для каждой панели

---

### Task 6.2: Kanban Board UI
**Complexity:** Standard
**Files:** `crates/surge-ui/src/`

- [ ] `kanban.rs` — Kanban board с колонками по `TaskState`
- [ ] Карточки задач: название, прогресс, agent badge, complexity
- [ ] Drag-and-drop между колонками (для ручного перемещения)
- [ ] Клик на карточку → task detail panel
- [ ] Фильтры: по агенту, сложности, статусу
- [ ] Кнопка "New Task" → spec creation dialog

---

### Task 6.3: Execution Monitor UI
**Complexity:** Standard
**Files:** `crates/surge-ui/src/`

- [ ] `execution.rs` — real-time execution view
- [ ] Dependency graph visualization (egui canvas)
- [ ] Subtask states: pending/running/completed/failed с цветами
- [ ] Streaming log panel
- [ ] Progress bar
- [ ] Pause/Resume кнопки
- [ ] Token counter и cost estimate

---

### Task 6.4: Agent Hub UI
**Complexity:** Simple
**Files:** `crates/surge-ui/src/`

- [ ] `agent_hub.rs` — список подключённых агентов
- [ ] Status indicators: online/offline/rate-limited
- [ ] Capabilities display для каждого агента
- [ ] "Add Agent" dialog
- [ ] "Test Connection" кнопка
- [ ] Health metrics: latency, error rate

---

### Task 6.5: Diff Viewer и File Explorer
**Complexity:** Standard
**Files:** `crates/surge-ui/src/`

- [ ] `diff_viewer.rs` — side-by-side diff с syntax highlighting
- [ ] `file_explorer.rs` — список изменённых файлов с +/- counts
- [ ] Группировка файлов по подзадачам
- [ ] Фильтры: Added/Modified/Deleted
- [ ] "Open in IDE" кнопка для каждого файла и worktree
- [ ] "Copy path" для worktree

---

### Task 6.6: Terminal Panel
**Complexity:** Standard
**Files:** `crates/surge-ui/src/`

- [ ] Добавить `portable-pty` зависимость
- [ ] `terminal.rs` — встроенный терминал в egui
- [ ] Split view: несколько терминалов рядом
- [ ] Автоскролл с toggle
- [ ] Search в output (Ctrl+F)
- [ ] Привязка терминала к worktree
- [ ] Input: отправка команд агенту

---

## Quick Reference

| Task | Phase | Complexity | Подзадач | Статус |
|------|-------|------------|----------|--------|
| 0.1 | 0 | Simple | 6 | ✅ |
| 0.2 | 0 | Standard | 8 | ✅ |
| 0.3 | 0 | Simple | 6 | ✅ |
| 0.4 | 0 | Simple | 6 | ✅ |
| 1.1 | 1 | Simple | 6 | ✅ |
| 1.2 | 1 | Simple | 5 | ✅ |
| 1.3 | 1 | Simple | 6 | ✅ |
| 2.1 | 2 | Standard | 8 | ✅ |
| 2.2 | 2 | Simple | 7 | ✅ |
| 3.1 | 3 | Simple | 6 | ✅ |
| 3.2 | 3 | Standard | 7 | ✅ |
| 3.3 | 3 | Simple | 6 | ✅ |
| 3.4 | 3 | Simple | 6 | ✅ |
| 4.1 | 4 | Standard | 7 | ✅ |
| 5.1 | 5 | Simple | 6 | ✅ |
| 5.2 | 5 | Simple | 6 | ✅ |
| 6.1 | 6 | Simple | 6 | ⬜ |
| 6.2 | 6 | Standard | 6 | ⬜ |
| 6.3 | 6 | Standard | 7 | ⬜ |
| 6.4 | 6 | Simple | 6 | ⬜ |
| 6.5 | 6 | Standard | 6 | ⬜ |
| 6.6 | 6 | Standard | 7 | ⬜ |

**Всего: 22 таска, ~140 подзадач**
**Phase 0-5: ✅ Завершены (16/22 тасков, 100 тестов, 0 clippy warnings)**
**Phase 6: ⬜ GUI — отдельный этап**
