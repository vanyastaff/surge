# Surge — Community Pain Points (из issues Aperant)

> Собрано из 1800+ issues, discussions и changelog Aperant/Auto-Claude.
> Каждая боль = возможность для Surge.

---

## Категория 1: Аутентификация и провайдеры

### Боль: OAuth-токены ломаются постоянно
**Issues:** #1281, #1518, #1532, #1747
- <цитата> OAuth flow генерирует API-токен вместо подключения к Max подписке
- Токены истекают через 8-12 часов, задачи продолжают крутиться с 401 ошибками часами
- OAuth token revocation вызывает бесконечный цикл 401 ошибок
- Нужно вручную делать `/login` в каждом терминале после перезапуска

**Surge решение:**
- **ACP-based auth** — Surge не управляет аутентификацией агента. Агент (Claude Code / Copilot) аутентифицируется сам. Surge просто подключается через ACP. Ноль проблем с токенами.
- **Health check** — если агент возвращает auth ошибки, Surge немедленно уведомляет пользователя и ставит задачи на паузу, а не сжигает токены retry-циклами.
- **Auto-pause on auth failure** — задача мгновенно приостанавливается, не через часы.

---

### Боль: Хочу использовать сторонних провайдеров (OpenRouter, Bedrock, Ollama)
**Issues:** #1144, #356, Discussion #195
- Нет документации как использовать OpenRouter
- Auto-Claude перезаписывает `.claude_settings.json` без предупреждения
- Просят поддержку Bedrock, Gemini, Kimi
- Пользователи хотят failover между провайдерами

**Surge решение:**
- **ACP = любой агент** — Surge не знает и не заботится какой провайдер у агента. Claude Code с OpenRouter? Copilot с GPT-5? Всё работает через ACP.
- **Multi-agent из коробки** — не хак через settings файл, а первоклассная фича.
- **Agent routing + failover** — один агент упал → автоматическое переключение на другой.

---

## Категория 2: Задачи застревают и ломаются

### Боль: Subtask stuck в бесконечном retry loop
**Issues:** #189, #1546, #1723
- Подзадачи зацикливаются при tool concurrency errors (400)
- Сжигают токены без прогресса
- Recover не помогает — та же подзадача снова крашится

**Surge решение:**
- **Max retry с exponential backoff** — 3 попытки, потом пропуск с уведомлением.
- **Skip & continue** — пользователь может пропустить проблемную подзадачу и вернуться к ней позже.
- **Circuit breaker** — если N подзадач подряд падают, pipeline ставится на паузу.
- **Subtask isolation** — каждая подзадача в отдельной ACP сессии. Краш одной не влияет на другие.

---

### Боль: Rate limit прерывает задачу, и нужно всё начинать заново
**Discussion:** #1851
- Задача прерывается при rate limit — тихо, без уведомления
- Пользователь обнаруживает часы спустя
- Нет auto-resume после reset лимита
- "Ломает обещание set-it-and-forget-it"

**Surge решение:**
- **Rate limit detection** — агент возвращает 429 → Surge ставит задачу на pause с таймером до reset.
- **Auto-resume** — задача автоматически продолжается после reset лимита. Subtask state сохранён на диске.
- **Notification** — push-уведомление (system tray) что задача на паузе из-за rate limit.
- **Agent fallback** — если Claude rate-limited, переключиться на Copilot для оставшихся подзадач.
- **Estimated time** — "Rate limited. Resuming in ~47 minutes."

---

### Боль: Нельзя дать фидбек на план до начала кодинга
**Discussion:** #1563
- Агент пишет spec и сразу начинает кодить
- Checkbox "Require human review before coding" не работает
- Нельзя изменить план после генерации
- "Я не могу управлять процессом"

**Surge решение:**
- **Explicit gates** — pipeline имеет обязательные точки остановки:
  ```
  Spec Created → [GATE: User Review] → Plan Generated → [GATE: User Review] → Execution
  ```
- **Plan editing** — пользователь может редактировать plan перед запуском: переупорядочить подзадачи, удалить ненужные, добавить свои, сменить агента.
- **Inline feedback** — в UI можно оставить комментарий к конкретной подзадаче, и агент учтёт при выполнении.
- **Configurable gates:**
  ```toml
  [pipeline.gates]
  after_spec = true      # ждать approve после генерации spec
  after_plan = true      # ждать approve после генерации плана
  after_each_subtask = false  # не ждать после каждой подзадачи
  after_qa = true        # ждать approve после QA
  ```

---

## Категория 3: Windows-специфичные проблемы

### Боль: Zombie процессы, CLI not found, encoding errors
**Issues:** #641, #582, #1050, #1331, #1093, #877
- При закрытии Aperant на Windows процессы остаются в фоне навсегда
- `cmd.exe` процессы attached к проекту — нельзя переименовать директорию
- Claude Code "not found" хотя установлен — 251 fix в одном релизе для UTF-8
- `System32\where.exe` и `taskkill.exe` не находятся
- PowerShell vs cmd.exe confusion

**Surge решение:**
- **Rust process management** — `Drop` trait гарантирует cleanup. `tokio::process::Child` с `kill_on_drop(true)`.
- **No shell dependency** — Surge не порождает `cmd.exe`. Все процессы запускаются напрямую через `tokio::process::Command`.
- **Native Unicode** — Rust нативно работает с UTF-8, нет Python encoding issues.
- **Single process** — один бинарник, никаких дочерних npm/python процессов. Только CLI агентов.
- **PID file** — `.surge/surge.pid` для обнаружения и убийства orphaned процессов при перезапуске.

---

## Категория 4: Context / MCP overhead

### Боль: MCP определения съедают контекст в каждой сессии
**Issue:** #1644
- MCP tool definitions инжектируются в каждую сессию, даже когда не нужны
- Несколько MCP серверов — overhead compounds
- Токены расходуются на парсинг MCP определений в каждом API-вызове
- "Каждый шаг платит полную цену за неиспользуемые инструменты"

**Surge решение:**
- **ACP handle MCP** — Surge не управляет MCP напрямую. MCP-серверы конфигурируются на стороне агента (Claude Code / Copilot). Surge только передаёт контекст через ACP.
- **Minimal context injection** — Surge передаёт агенту только то, что нужно для текущей подзадачи: spec, subtask description, relevant files. Не весь проект.
- **Context budget** — конфигурируемый лимит контекста per-subtask.

---

### Боль: Нет интеграции с внешними knowledge bases
**Issue:** #1506
- Memory система самодостаточная, нет поддержки Obsidian, Logseq, Notion
- Разработчики хранят архитектурные решения и паттерны во внешних vault'ах
- Нужно вручную копировать контекст из заметок в промпт

**Surge решение:**
- **Context sources** — конфигурируемые источники контекста:
  ```toml
  [context.sources]
  # Встроенная SQLite память
  builtin = true

  # Дополнительные markdown файлы
  markdown_dirs = ["docs/", "specs/archive/"]

  # Obsidian vault
  obsidian = { path = "~/vaults/work", tags = ["project-x"] }

  # CLAUDE.md / AGENTS.md проекта
  agent_files = true
  ```
- **MCP pass-through** — если у пользователя есть Obsidian MCP или другой MCP, Surge может указать агенту использовать его.

---

## Категория 5: Качество и стабильность UI

### Боль: UI фризы и краши
**Issues/Changelog:** Infinite re-render loops, panel constraint errors, scroll-to-blank, kanban scaling collisions, GPU context exhaustion
- Terminal font settings вызывают infinite re-render
- Kanban board scaling collisions при определённых размерах
- GPU context exhaustion от больших paste-ов
- Panel constraint errors при закрытии терминала
- Insights скроллит к пустому месту
- SIGABRT crash на macOS при shutdown

**Surge решение:**
- **egui immediate mode** — нет virtual DOM, нет reconciliation bugs. UI перерисовывается полностью каждый кадр — невозможен stale state.
- **No web stack** — нет Chromium, нет GPU context проблем, нет CSS layout bugs.
- **Rust memory safety** — нет null pointer, нет use-after-free, нет data races.
- **60fps cap** — egui рисует только при изменениях, минимальная нагрузка на GPU.

---

### Боль: Terminal sessions не восстанавливаются после рестарта
**Issue:** #1671
- После краша/рестарта терминалы показывают "Pending Resume"
- Нужно кликать на каждый терминал вручную для resume
- Нельзя resume all сразу

**Surge решение:**
- **Session persistence** — каждая ACP сессия имеет session_id. При рестарте Surge пытается `load_session` через ACP для восстановления.
- **Auto-reconnect** — если агент жив, сессия восстанавливается автоматически. Если агент перезапущен — новая сессия с контекстом из spec.
- **Resume all** — одна кнопка "Resume all paused tasks".

---

## Категория 6: Worktree и Git проблемы

### Боль: Worktree errors, lock files, detached HEAD
**Issues/Changelog:** #1453, #1586, race conditions, lock files, detached HEAD при PR creation
- Worktree ошибки при повторном запуске задачи
- Race condition при параллельном создании worktrees
- Lock files от worktrees мешают merge
- Detached HEAD state ломает PR creation
- Branch pattern validation fails

**Surge решение:**
- **git2 library** — нативная Rust библиотека, не subprocess `git`. Нет shell injection, нет race conditions от параллельных `git` процессов.
- **Mutex per-worktree** — `tokio::sync::Mutex` на каждый worktree предотвращает concurrent access.
- **Pre-flight checks** — перед каждой git операцией проверяем: worktree exists? branch valid? no lock files? no conflicts?
- **Auto-repair** — если worktree в broken state, Surge может пересоздать его из branch.

---

## Категория 7: Planning & Spec

### Боль: Пустые/greenfield проекты ломают планирование
**Changelog:** "Fixed handling of empty/greenfield projects"
- Planning phase крашится на пустых проектах
- Atomic writes prevent 0-byte file corruption
- Implementation plan file watching fails

**Surge решение:**
- **Graceful empty project** — если проект пустой, Surge генерирует базовую структуру (Cargo.toml / package.json / etc) как первую подзадачу.
- **Spec validation** — spec.toml валидируется ДО запуска. Если something missing — ошибка с описанием что нужно добавить, а не краш в середине.
- **Atomic writes** — `tempfile` + `rename` для всех файловых операций. Нет 0-byte corruption.

---

## Сводная таблица: 20 болей → 20 решений

| # | Боль | Severity | Surge решение |
|---|------|----------|---------------|
| 1 | OAuth токены ломаются | 🔴 Critical | ACP — агент аутентифицируется сам |
| 2 | Нет multi-provider | 🟡 Medium | ACP = любой агент |
| 3 | Subtask infinite retry | 🔴 Critical | Circuit breaker + skip & continue |
| 4 | Rate limit kills task | 🔴 Critical | Auto-pause + auto-resume |
| 5 | Нельзя дать feedback на план | 🟠 High | Explicit gates + plan editing |
| 6 | Windows zombie processes | 🟠 High | Rust Drop + kill_on_drop |
| 7 | CLI not found / encoding | 🟠 High | Native Rust, no shell dependency |
| 8 | MCP wastes context tokens | 🟡 Medium | ACP delegates MCP to agent |
| 9 | Нет внешних knowledge bases | 🟡 Medium | Configurable context sources |
| 10 | UI infinite re-render | 🟠 High | egui immediate mode |
| 11 | Terminal не resume | 🟡 Medium | ACP load_session |
| 12 | Worktree race conditions | 🟠 High | git2 + per-worktree mutex |
| 13 | Empty project crash | 🟡 Medium | Graceful empty + validation |
| 14 | Мусор после задач | 🟠 High | Lifecycle manager + auto cleanup |
| 15 | Нельзя видеть файлы/diff | 🟠 High | Rich file explorer + inline diff |
| 16 | Нельзя открыть в IDE | 🟠 High | One-click open in IDE |
| 17 | PR view не работает | 🟠 High | Integrated PR workflow |
| 18 | Settings перезаписываются | 🟡 Medium | Surge config в `.surge/`, read-only |
| 19 | 0-byte file corruption | 🟠 High | Atomic writes (tempfile + rename) |
| 20 | Task stuck после 15 subtasks | 🔴 Critical | Subtask isolation + fresh ACP session |
