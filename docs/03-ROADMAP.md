# Surge — Roadmap

## Принципы развития

- **Vertical slice** — каждая фаза заканчивается работающим продуктом
- **CLI-first** — GUI добавляется после стабилизации ядра
- **Один агент → много** — сначала интеграция с Claude Code, потом мульти-агент

---

## Phase 0: Foundation (2 недели)

**Цель:** Cargo workspace, базовые типы, первое ACP-подключение.

- [ ] Инициализация workspace с крейтами: `surge-core`, `surge-acp`, `surge-cli`
- [ ] Базовые типы в `surge-core`: `SpecId`, `TaskState`, `SurgeConfig`, `AgentConfig`
- [ ] `surge-acp`: реализация `Client` trait из `agent-client-protocol`
  - [ ] `request_permission` — auto-approve mode
  - [ ] `session_update` — вывод в stdout
  - [ ] `write_text_file` / `read_text_file` — базовая FS
- [ ] Подключение к Claude Code CLI через stdio transport
- [ ] `surge-cli`: команда `surge ping` — проверка связи с агентом
- [ ] `surge-cli`: команда `surge prompt "Hello"` — one-shot запрос

**Результат:** `surge prompt "Explain Rust ownership"` работает через ACP → Claude Code CLI.

---

## Phase 1: Spec System (2 недели)

**Цель:** Создание, хранение и парсинг спецификаций.

- [ ] `surge-spec`: TOML-формат спецификации (см. ARCHITECTURE.md)
- [ ] Сериализация / десериализация через serde
- [ ] CLI: `surge spec create "описание задачи"` — интерактивное создание
- [ ] CLI: `surge spec create --interactive` — AI-assisted создание (агент помогает уточнить)
- [ ] CLI: `surge spec list` — список спеков с состояниями
- [ ] CLI: `surge spec show 012-add-auth` — просмотр деталей
- [ ] Автоматическая оценка сложности через агента (simple/standard/complex)
- [ ] Хранение спеков в `.surge/specs/` директории проекта
- [ ] Валидация спеков: обязательные поля, формат, ссылки

**Результат:** Полный цикл создания спецификации с AI-assistом.

---

## Phase 2: Git Worktrees (1 неделя)

**Цель:** Изоляция каждой задачи в отдельном worktree.

- [ ] `surge-git`: обёртка над `git2` для worktree операций
- [ ] Создание worktree: `git worktree add -b surge/{spec_id} .surge/worktrees/{spec_id}`
- [ ] Коммит изменений внутри worktree
- [ ] Diff между worktree и base branch
- [ ] Merge worktree в target branch
- [ ] Discard (удаление) worktree
- [ ] CLI: `surge diff {spec_id}`, `surge merge {spec_id}`, `surge discard {spec_id}`
- [ ] Автоматическая очистка orphaned worktrees

**Результат:** `surge merge 012-add-auth` мёрджит изолированные изменения в main.

---

## Phase 3: Orchestrator MVP (3 недели)

**Цель:** Полный пайплайн выполнения задачи с одним агентом.

- [ ] `surge-orchestrator`: основной pipeline
- [ ] Phase: Planning — агент разбивает spec на subtasks (если не заданы вручную)
- [ ] Phase: Execution — последовательное выполнение подзадач
  - [ ] Формирование контекстного промпта для каждой подзадачи
  - [ ] Отправка через ACP, получение streaming updates
  - [ ] Коммит после каждой подзадачи
- [ ] Phase: QA — агент проверяет acceptance criteria
  - [ ] Создание QA-fix подзадач при обнаружении проблем
  - [ ] QA-цикл с лимитом итераций
- [ ] Граф зависимостей подзадач (petgraph)
- [ ] File watcher: PAUSE файл для паузы, HUMAN_INPUT.md для инструкций
- [ ] CLI: `surge run {spec_id}` — запуск полного pipeline
- [ ] CLI: `surge status {spec_id}` — текущее состояние
- [ ] CLI: `surge logs {spec_id} --follow` — streaming логи

**Результат:** `surge run 012-add-auth` автономно выполняет все подзадачи, проводит QA, и ждёт review.

---

## Phase 4: Параллельное выполнение (2 недели)

**Цель:** Независимые подзадачи выполняются параллельно.

- [ ] Topological sort графа зависимостей → batches для параллелизма
- [ ] Параллельные ACP сессии к одному или разным агентам
- [ ] Merge результатов параллельных подзадач в worktree
- [ ] Обнаружение и разрешение конфликтов между параллельными изменениями
- [ ] CLI: `surge run {spec_id} --parallel 3` — ограничение параллелизма
- [ ] Прогресс-бар для параллельных подзадач

**Результат:** Независимые подзадачи выполняются одновременно, ускоряя pipeline в 2-4 раза.

---

## Phase 5: Multi-Agent (2 недели)

**Цель:** Разные агенты для разных подзадач и фаз.

- [ ] `AgentPool`: управление множеством подключений
- [ ] Конфигурация агентов в `surge.toml`:
  ```toml
  [agents.claude]
  command = "claude"
  transport = "stdio"

  [agents.copilot]
  command = "copilot"
  transport = "stdio"
  ```
- [ ] Routing подзадач к агентам:
  - По умолчанию из конфига
  - Override в spec.toml per-subtask
  - По фазе: claude для planning, copilot для coding
- [ ] CLI: `surge agent add`, `surge agent test`, `surge agent list`
- [ ] Fallback: если агент недоступен → переключение на альтернативный

**Результат:** `surge run 012-add-auth --planner claude --coder copilot` — планирование через Claude, кодинг через Copilot.

---

## Phase 6: GUI (4 недели)

**Цель:** Нативный GUI на egui.

- [ ] `surge-ui`: egui + eframe приложение
- [ ] Dashboard: Kanban-доска задач с drag-and-drop
- [ ] Spec Editor: создание и редактирование спецификаций
- [ ] Execution Monitor:
  - [ ] Real-time логи от агентов
  - [ ] Граф подзадач с состояниями
  - [ ] Прогресс-бары
- [ ] Agent Panel: статус подключений, выбор агента для задач
- [ ] Diff Viewer: side-by-side просмотр изменений
- [ ] Terminal Panel: встроенные PTY для worktrees
- [ ] Settings: конфигурация агентов, путей, поведения

**Результат:** Полноценный десктопный GUI — прямой конкурент Aperant.

---

## Phase 7: Advanced Features (ongoing)

- [ ] **Remote agents** — ACP через TCP/WebSocket для cloud-hosted агентов
- [ ] **Spec templates** — библиотека шаблонов (add-auth, add-api, fix-bug, refactor)
- [ ] **Project memory** — Graphiti-подобная система на SQLite для контекста между задачами
- [ ] **GitHub/GitLab integration** — создание spec из issue, автоматические PR
- [ ] **Team collaboration** — shared specs, review workflow
- [ ] **Plugin system** — расширение через WASM или dynamic libraries
- [ ] **Metrics & analytics** — стоимость токенов, время выполнения, success rate по агентам
- [ ] **ACP Registry** — browser доступных агентов, one-click подключение
- [ ] **Daemon mode** — фоновое выполнение задач с нотификациями
- [ ] **MCP pass-through** — трансляция MCP-серверов от пользователя к агенту

---

## Милестоуны

| Milestone | Фазы | Срок | Результат |
|-----------|-------|------|-----------|
| **v0.1.0-alpha** | 0-1 | +4 недели | ACP подключение + spec система |
| **v0.2.0-alpha** | 2-3 | +4 недели | Полный pipeline с одним агентом |
| **v0.3.0-beta** | 4-5 | +4 недели | Параллелизм + мульти-агент |
| **v0.5.0-beta** | 6 | +4 недели | GUI |
| **v1.0.0** | 7+ | +8 недель | Production-ready |

Общий срок до v1.0: ~6 месяцев при full-time разработке.

---

## Приоритеты

1. **ACP стабильность** — протокол ещё молодой, нужно отслеживать изменения
2. **Spec формат** — должен быть stable до v1.0, потом breaking changes дорого
3. **UX CLI** — первое впечатление. Команды должны быть интуитивными
4. **Тесты** — интеграционные тесты с реальными агентами в CI
5. **Документация** — README, getting started, examples с первого дня
