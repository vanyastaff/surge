# Surge — Feature Specification

> Все фичи Aperant + уникальные возможности Surge

---

## Навигация приложения

```
SURGE
├── 🏠 Dashboard                    ← обзор проекта, метрики, quick actions
├── ⬜ Kanban Board             [K]  ← задачи по стадиям
├── 🖥️ Agent Terminals          [A]  ← терминалы подключённых агентов
├── 📊 Insights                 [N]  ← аналитика: токены, время, стоимость
├── 🗺️ Roadmap                  [D]  ← план развития проекта (AI-generated)
├── 💡 Ideation                 [I]  ← AI находит улучшения, баги, уязвимости
├── 📝 Changelog                [L]  ← автогенерация release notes
├── 🧠 Context                  [C]  ← память проекта между сессиями
├── 🔌 Agent Hub                [H]  ← управление агентами (замена MCP Overview)
├── 🌳 Worktrees                [W]  ← изолированные рабочие директории
├── 🐙 GitHub Issues            [G]  ← issue → spec конвертация
├── 🔀 GitHub PRs               [P]  ← AI-powered PR review
├── 📋 Spec Explorer            [S]  ← NEW: браузер спецификаций
├── 🏃 Live Execution           [E]  ← NEW: real-time выполнение с графом
├── 🔬 Agent Benchmark          [B]  ← NEW: сравнение агентов
├── 📐 Design Studio            [U]  ← NEW: интеграция с Pencil/Figma MCP
│
├── AGENTS (подключённые)
│   ├── ⚡ Claude Code
│   ├── 🤖 GitHub Copilot
│   └── ➕ Add Agent...
│
└── ⚙️ Settings
```

---

## 1. Dashboard (🏠)

**Aperant:** Нет отдельного dashboard, сразу Kanban.

**Surge:**
Центральная панель с обзором проекта:

- **Project Health** — общий статус: сколько задач в работе, завершено, failed
- **Active Agents** — какие агенты подключены, их статус (online/offline/rate-limited)
- **Token Budget** — расход токенов за день/неделю/месяц по каждому агенту
- **Quick Actions:**
  - "New Task" — создать задачу
  - "Continue Last" — продолжить последнюю незавершённую задачу
  - "Review Queue" — задачи ждущие review
- **Recent Activity** — лента последних событий (коммиты, завершения, ошибки)
- **Cost Tracker** — оценка стоимости по API провайдерам

---

## 2. Kanban Board (⬜)

**Aperant:** Базовый Kanban с drag-and-drop.

**Surge — всё то же + :**

Колонки: `Draft → Planning → Executing → QA → Review → Merging → Done`

- **Agent badge** — на каждой карточке виден какой агент назначен (🟣 Claude / 🟢 Copilot)
- **Progress ring** — круговой прогресс внутри карточки (5/12 subtasks)
- **Dependency arrows** — визуальные связи между зависимыми задачами
- **Drag to reassign** — перетащить карточку на другого агента
- **Bulk actions** — выделить несколько → запустить / отменить / удалить
- **Filters:**
  - По агенту
  - По сложности (simple/standard/complex)
  - По автору
  - По дате
- **Priority lanes** — горизонтальные линии приоритета внутри колонки
- **Time estimates** — AI-estimated время выполнения на каждой карточке

---

## 3. Agent Terminals (🖥️)

**Aperant:** Терминалы привязаны к Claude Code.

**Surge:**

- **Multi-agent terminals** — отдельный терминал для каждого подключённого агента
- **Split view** — несколько терминалов рядом (Claude слева, Copilot справа)
- **Syntax highlighting** — подсветка кода в выводе агента
- **Search in output** — поиск по логам терминала (Ctrl+F)
- **Scroll lock** — автоскролл / ручной скролл
- **Copy output** — копирование блоков вывода
- **Session history** — история всех сессий с каждым агентом
- **Inject message** — отправить сообщение агенту прямо из терминала
- **Auto-retry** — при краше агента автоматически переподключить и продолжить
- **Resource monitor** — CPU/RAM/token usage per terminal

---

## 4. Insights (📊)

**Aperant:** Базовая аналитика завершённых задач.

**Surge:**

- **Token Analytics:**
  - Расход по агентам (pie chart)
  - Расход по фазам: planning vs coding vs QA (stacked bar)
  - Trend за неделю/месяц
  - Стоимость в $ по провайдерам
- **Performance:**
  - Среднее время выполнения подзадачи по агентам
  - Success rate по агентам
  - QA iteration count (сколько итераций до approved)
  - Первый vs повторный QA pass rate
- **Code metrics:**
  - LOC added/modified/deleted по задачам
  - File churn — какие файлы меняются чаще всего
  - Test coverage delta per task
- **Agent comparison:**
  - Side-by-side: Claude vs Copilot по метрикам
  - Рекомендация оптимального агента для типа задачи
- **Export:** CSV, JSON для внешней аналитики

---

## 5. Roadmap (🗺️)

**Aperant:** AI-generated roadmap из анализа кодовой базы.

**Surge:**

- **AI Roadmap Generation** — агент анализирует код и генерирует план развития
- **Phase management** — группировка фич по фазам/спринтам
- **Drag to reorder** — приоритизация через drag-and-drop
- **Convert to Task** — одним кликом превратить roadmap item в spec + task
- **Dependency visualization** — граф зависимостей между roadmap items
- **Timeline view** — Gantt-подобная временная шкала с AI-estimates
- **Multi-agent estimation** — оценка сроков с учётом параллелизма агентов
- **Risk highlights** — AI отмечает рискованные пункты (breaking changes, complex integrations)

---

## 6. Ideation (💡)

**Aperant:** AI ищет баги, уязвимости, оптимизации.

**Surge:**

- **Scan categories:**
  - 🐛 Bugs — потенциальные ошибки
  - ⚡ Performance — узкие места
  - 🔒 Security — уязвимости
  - 🎨 Code quality — запахи кода, duplication
  - ♿ Accessibility — проблемы доступности (web-проекты)
  - 📝 Documentation — недокументированный публичный API
  - 🧪 Test gaps — непокрытый тестами код
- **Multi-agent scan** — запустить ideation через разных агентов и мёрджить результаты
- **One-click fix** — из найденной проблемы сразу создать spec и запустить fix
- **Severity scoring** — AI оценивает критичность каждой находки
- **History** — трекинг что было найдено и что исправлено
- **Scheduled scans** — автоматический запуск по расписанию

---

## 7. Changelog (📝)

**Aperant:** Генерация release notes из завершённых задач.

**Surge:**

- **Auto-generation** — из git history + completed specs
- **Format templates:**
  - Keep a Changelog
  - Conventional Commits
  - Custom template (user-defined)
- **Semantic versioning** — AI предлагает version bump (patch/minor/major)
- **Multi-language** — генерация на английском и русском
- **GitHub Release integration** — публикация напрямую как GitHub Release
- **Audience targeting** — техническая vs пользовательская версия

---

## 8. Context / Memory (🧠)

**Aperant:** Graphiti memory layer (Python + Neo4j/FalkorDB).

**Surge:**

- **SQLite-based** — нет внешних зависимостей (vs Neo4j у Aperant)
- **Project knowledge base:**
  - Архитектурные решения и их причины
  - Паттерны кодирования проекта
  - Известные gotchas и pitfalls
  - Результаты прошлых QA-ревью
- **Auto-capture** — автоматически извлекает знания из завершённых задач
- **Inject into prompts** — релевантный контекст автоматически добавляется в промпт агенту
- **Searchable** — полнотекстовый поиск по контексту
- **Manual entries** — пользователь может добавить заметки вручную
- **Export/import** — переносимость контекста между проектами
- **Context budget** — умное ограничение: не перегружать промпт контекстом

---

## 9. Agent Hub (🔌) — NEW (замена MCP Overview)

**Aperant:** MCP Overview — просмотр подключённых MCP серверов.

**Surge — значительно шире:**

- **Agent Registry** — каталог доступных ACP-агентов:
  - Claude Code CLI
  - GitHub Copilot CLI
  - Zed Agent
  - Custom agents
- **One-click connect** — подключение агента из registry
- **Status dashboard:**
  - 🟢 Online — агент работает
  - 🟡 Rate-limited — ждёт cooldown
  - 🔴 Offline — не подключен
  - ⚙️ Configuring — первичная настройка
- **Agent capabilities** — что умеет каждый агент (tools, models, modes)
- **MCP pass-through** — трансляция пользовательских MCP серверов к агентам
- **Agent routing rules** — настройка какой агент для какого типа задач
- **Health monitoring** — latency, error rate, uptime per agent
- **Usage quotas** — отслеживание лимитов подписки/API по каждому агенту
- **BYOK config** — настройка API-ключей для каждого провайдера

---

## 10. Worktrees (🌳)

**Aperant:** Базовое управление worktrees с багами (race conditions, crash recovery).

**Surge:**

- **Lifecycle management:**
  - Auto-create при старте задачи
  - Auto-cleanup завершённых/отменённых
  - Orphan detection — найти и очистить "забытые" worktrees
- **Visual diff** — side-by-side diff прямо в UI
- **Branch management:**
  - Автоименование: `surge/{spec-id}`
  - Force-push protection
  - Conflict detection до merge
- **Bulk operations:**
  - Merge all reviewed
  - Discard all failed
  - Prune stale worktrees
- **Terminal integration** — открыть терминал в конкретном worktree
- **File explorer** — просмотр файлов worktree в UI
- **Size indicator** — сколько файлов изменено, +/- строк

---

## 11. GitHub Issues (🐙)

**Aperant:** Базовая интеграция с GitHub issues.

**Surge:**

- **Issue browser** — список issues с фильтрами (labels, milestone, assignee)
- **Issue → Spec** — конвертация issue в spec одним кликом:
  - AI читает issue description
  - Генерирует spec.toml с subtasks
  - Пользователь ревьюит и запускает
- **Bi-directional sync:**
  - Surge обновляет issue когда задача начата / завершена
  - Комментарии с прогрессом
  - Автоматическое закрытие issue при merge
- **Label mapping** — маппинг GitHub labels на spec complexity
- **Templates** — шаблоны issue для разных типов задач
- **GitLab support** — аналогичная интеграция для GitLab

---

## 12. GitHub PRs (🔀)

**Aperant:** AI-powered PR review с XState state machine.

**Surge:**

- **Auto PR creation** — после merge worktree автоматически создаётся PR
- **AI PR review:**
  - Multi-agent review (Claude проверяет логику, Copilot — стиль)
  - Structured feedback: bugs, style, performance, security
  - Line-level comments
  - Suggested fixes с one-click apply
- **PR templates** — автогенерация описания из spec
- **CI status** — отображение статуса CI прямо в Surge
- **Review queue** — список PR ожидающих review
- **Merge strategies** — squash / merge / rebase с preview
- **Branch protection** — предупреждение при попытке merge в protected branch

---

## 13. Spec Explorer (📋) — NEW

**Нет в Aperant.**

Полноценный браузер спецификаций:

- **Spec gallery** — все спеки проекта с карточками
- **Search & filter** — по статусу, сложности, агенту, дате
- **Spec templates library:**
  - `add-feature` — добавление новой фичи
  - `fix-bug` — исправление бага
  - `refactor` — рефакторинг
  - `add-api-endpoint` — новый API endpoint
  - `add-auth` — аутентификация
  - `add-tests` — покрытие тестами
  - `migration` — миграция (DB, framework, library)
  - `performance` — оптимизация производительности
  - Community templates — загрузка из registry
- **Spec diff** — сравнение версий спецификации
- **Spec analytics** — метрики по спекам: success rate, avg time, common issues
- **Import/export** — обмен спеками между проектами
- **Spec linting** — валидация качества спецификации до запуска

---

## 14. Live Execution (🏃) — NEW

**Нет в Aperant** (только логи).

Real-time визуализация выполнения:

- **Dependency graph** — интерактивный граф подзадач:
  - 🔵 Pending
  - 🟡 In Progress (пульсирует)
  - 🟢 Completed
  - 🔴 Failed
  - Анимация переходов
- **Parallel lanes** — визуализация параллельного выполнения
- **Agent activity** — какой агент что делает прямо сейчас
- **File changes stream** — live feed создаваемых/изменяемых файлов
- **Token counter** — real-time расход токенов
- **Time estimate** — ETA до завершения
- **Pause/Resume** — пауза выполнения без потери прогресса
- **Inject instructions** — отправить уточнение агенту на лету
- **Subtask drill-down** — клик на подзадачу → детальный лог

---

## 15. Agent Benchmark (🔬) — NEW

**Нет ни у кого.**

Объективное сравнение агентов:

- **A/B testing** — одна подзадача → два агента → сравнение результатов
- **Metrics:**
  - Время выполнения
  - Количество токенов
  - Стоимость
  - QA pass rate (с первой попытки)
  - Code quality score
- **Leaderboard** — рейтинг агентов по метрикам для данного проекта
- **Auto-routing recommendation** — "для Rust-задач Claude на 23% быстрее, для TypeScript Copilot на 15% дешевле"
- **Historical trends** — как агенты улучшаются со временем
- **Export reports** — для принятия решений о подписках/бюджетах

---

## 16. Design Studio (📐) — NEW

**Нет в Aperant.**

Интеграция с дизайн-инструментами:

- **Pencil MCP** — подключение Pencil.dev для UI-дизайна
- **Figma MCP** — импорт дизайнов из Figma
- **Design → Spec** — из дизайна автоматически генерируется spec для реализации
- **Screenshot capture** — скриншот UI → агент анализирует → создаёт fix spec
- **Style guide extraction** — извлечение design tokens из проекта
- **Component preview** — предпросмотр UI-компонентов прямо в Surge

---

## Уникальные фичи Surge (summary)

| Фича | Описание | Есть у конкурентов? |
|------|----------|-------------------|
| **Multi-agent routing** | Разные агенты для разных подзадач | ❌ Нигде |
| **Agent Benchmark** | A/B тестирование агентов | ❌ Нигде |
| **Live Execution Graph** | Интерактивный граф зависимостей в реальном времени | ❌ Нигде |
| **Spec Explorer + Templates** | Библиотека шаблонов спецификаций | ❌ Нигде |
| **Cost Tracking** | Real-time стоимость по провайдерам | ❌ Нигде |
| **Smart Permissions** | Гранулярная фильтрация опасных команд | ❌ Все используют skip-permissions |
| **Design Studio** | Pencil/Figma → Spec → Code pipeline | ❌ Нигде |
| **Dry Run mode** | Предпросмотр плана без выполнения | ❌ Нигде |
| **SQLite context** | Нативная память без внешних DB | ❌ Aperant использует Neo4j |
| **Single binary** | 15MB, 0 зависимостей | ❌ Все на Electron |
| **Multi-agent QA** | QA другим агентом чем кодинг | ❌ Нигде |
| **Spec linting** | Валидация качества спека до запуска | ❌ Нигде |

---

## Клавиатурные сочетания

| Сочетание | Действие |
|-----------|----------|
| `Ctrl+K` | Command palette (поиск по всему) |
| `Ctrl+N` | Новая задача |
| `Ctrl+Enter` | Запустить выбранную задачу |
| `Ctrl+.` | Quick actions для текущего элемента |
| `Ctrl+Shift+T` | Новый терминал |
| `Ctrl+1..9` | Переключение между вкладками |
| `Ctrl+P` | Переключение между specs |
| `Ctrl+Shift+P` | Переключение между агентами |
| `Escape` | Пауза текущего выполнения |
| `Ctrl+D` | Diff текущего worktree |
| `Ctrl+M` | Merge текущего worktree |

---

## Тема и брендинг

**Цветовая палитра:**
- Primary: `#7C3AED` (фиолетовый — энергия, инновация)
- Secondary: `#06B6D4` (cyan — технологичность)
- Success: `#10B981` (зелёный)
- Warning: `#F59E0B` (жёлтый)
- Error: `#EF4444` (красный)
- Background: `#0F0F17` (deep dark)
- Surface: `#1A1A2E` (card background)
- Text: `#E2E8F0` (light gray)

**Принципы UI:**
- Тёмная тема по умолчанию (светлая опционально)
- Минимализм — ничего лишнего на экране
- Информативность — каждый пиксель несёт смысл
- Быстрый отклик — никаких спиннеров дольше 100ms
- Keyboard-first — всё доступно без мыши
