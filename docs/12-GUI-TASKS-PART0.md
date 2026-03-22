# Surge — GUI Tasks (Phase 6) — Part 0: Стартовый Flow

> Эти таски выполняются ПЕРЕД Part 1/Part 2
> Без них приложение не знает какой проект открывать

---

## Task 6.00: Welcome Screen / Project Picker
**Complexity:** Standard | **Depends on:** 6.0 (scaffold)

Первый экран при запуске Surge. Как в VS Code, RustRover, Zed.

- [ ] `screens/welcome.rs` — полноэкранный стартовый экран
- [ ] Logo + version + tagline ("Any Agent. One Protocol. Pure Rust.")
- [ ] Секция **Recent Projects** — список последних 10 проектов с:
  - Название проекта (из surge.toml `[project].name` или dirname)
  - Путь на диске
  - Последняя активность (timestamp)
  - Количество активных задач (badge)
  - Иконка: Git status (clean / dirty)
  - Кнопка "Remove from list" (не удаляет файлы)
- [ ] Кнопка **Open Project** → native directory picker dialog (gpui file dialog)
- [ ] Кнопка **Init New Project** → Surge init wizard (Task 6.01)
- [ ] Кнопка **Clone & Open** → git clone URL → init → open
- [ ] Pinned projects: звёздочка для закрепления проекта вверху списка
- [ ] Keyboard: Enter на выделенном проекте → открыть, стрелки для навигации
- [ ] Хранение recent projects: `~/.surge/recent.toml`

```toml
# ~/.surge/recent.toml
[[projects]]
name = "nebula"
path = "C:/Users/vanya/projects/nebula"
last_opened = "2026-03-22T10:00:00Z"
pinned = true
active_tasks = 3

[[projects]]
name = "flui"
path = "C:/Users/vanya/projects/flui"
last_opened = "2026-03-20T14:30:00Z"
pinned = false
active_tasks = 0
```

---

## Task 6.01: Project Init Wizard
**Complexity:** Standard | **Depends on:** 6.00

Создание нового Surge проекта в существующей или новой директории.

- [ ] `screens/welcome/init_wizard.rs` — multi-step dialog
- [ ] Step 1 **Directory**: выбрать существующую папку или создать новую
  - Показать предупреждение если уже есть `.surge/` (предложить re-init или cancel)
  - Показать info если нет `.git/` (предложить git init)
- [ ] Step 2 **Project Info**: название проекта, описание (опционально)
  - Auto-detect: язык (Cargo.toml → Rust, package.json → JS/TS, pyproject.toml → Python)
  - Auto-detect: git remote URL → предложить GitHub integration
- [ ] Step 3 **Agent Setup**: выбрать агента по умолчанию
  - Quick setup: "Claude Code" (auto-detect если установлен)
  - Custom: name, command, transport
  - Test connection кнопка
  - "Skip, configure later" option
- [ ] Step 4 **Pipeline Config**: quick presets
  - Preset "Careful" — все gates включены, max 3 parallel, QA strict
  - Preset "Fast" — только Human Review gate, max 5 parallel, QA lenient
  - Preset "Custom" — показать все опции
- [ ] Step 5 **Confirm**: summary, кнопки "Create Project" / "Cancel"
- [ ] При создании:
  - Создать `.surge/` директорию
  - Создать `surge.toml` с конфигом
  - Создать `.surge/surge.db` (SQLite)
  - Добавить `.surge/` в `.gitignore` (если git)
  - Обновить `~/.surge/recent.toml`
  - Перейти на Dashboard

---

## Task 6.02: Project Switcher (in-app)
**Complexity:** Simple | **Depends on:** 6.00, 6.1 (sidebar)

Быстрое переключение проектов без закрытия приложения.

- [ ] В header (top bar): текущий проект — название + путь
- [ ] Клик на название → dropdown со списком recent projects
- [ ] Каждый проект в dropdown: название, путь, active tasks badge
- [ ] "Open Other..." → native directory picker
- [ ] "New Project..." → Init Wizard (6.01)
- [ ] Keyboard shortcut: Ctrl+Shift+P → project switcher
- [ ] При переключении: сохранить state текущего проекта, загрузить новый

---

## Task 6.03: Top Bar / Header
**Complexity:** Simple | **Depends on:** 6.1 (sidebar)

Верхняя панель приложения — контекст и глобальные действия.

- [ ] Left: Project name (clickable → switcher 6.02) + branch name badge
- [ ] Center: Breadcrumb — текущий экран (Dashboard > Kanban > Task #012)
- [ ] Right zone:
  - Global search (Ctrl+K → command palette 6.19)
  - Notification bell с unread count badge
  - Agent status indicators (mini dots: 🟢🟢🔴)
  - Rate limit warning icon (если <20% осталось)
- [ ] Compact mode на узких окнах: скрыть breadcrumb, иконки only

---

## Task 6.04: Onboarding / First-Run Experience
**Complexity:** Simple | **Depends on:** 6.00, 6.01

Первый запуск Surge — помощь новому пользователю.

- [ ] Detect first run: `~/.surge/recent.toml` не существует
- [ ] Welcome overlay: "Welcome to Surge!" с тремя опциями:
  - "Quick Start" → Init Wizard с preset "Fast"
  - "Guided Setup" → Init Wizard с объяснениями на каждом шагу
  - "I know what I'm doing" → пропустить, показать Project Picker
- [ ] Tooltips на первом проекте: подсветить key UI elements (sidebar, command palette, new task)
- [ ] Dismiss: checkbox "Don't show again"
- [ ] `~/.surge/preferences.toml`: `onboarding_completed = true`

---

## Task 6.05: Global Keybindings
**Complexity:** Simple | **Depends on:** 6.1, 6.02, 6.19

Все глобальные keyboard shortcuts в одном месте.

- [ ] `keybindings.rs` — регистрация всех Actions через GPUI action system
- [ ] Navigation:
  - `Ctrl+1..9` → switch screen (sidebar items)
  - `Ctrl+Shift+P` → project switcher
  - `Ctrl+K` → command palette
  - `Ctrl+B` → toggle sidebar
  - `Ctrl+\`` → toggle terminal panel
- [ ] Tasks:
  - `Ctrl+N` → new task / new spec
  - `Ctrl+Enter` → approve current gate
  - `Ctrl+Shift+Enter` → approve & continue
  - `Escape` → cancel current dialog / close modal
- [ ] Diff:
  - `Ctrl+D` → open diff viewer for current task
  - `]c` / `[c` → next/prev change (vim-style)
- [ ] Customizable через `surge.toml` section `[keybindings]`
- [ ] Help overlay: `Ctrl+?` → показать все shortcuts

---

## Обновлённый порядок выполнения

### Wave 0 — Startup Flow (НОВОЕ):
```
6.00 Welcome / Project Picker
  → 6.01 Init Wizard
  → 6.02 Project Switcher
  → 6.03 Top Bar
  → 6.04 Onboarding
  → 6.05 Global Keybindings
```

### Wave 1 — Каркас:
```
6.0 Scaffold surge-ui
  → 6.1 Sidebar
  → 6.20 Notifications
  → 6.19 Command Palette
```

### Wave 2–5 — без изменений (Part 1 / Part 2)

---

## Полная карта тасков

| # | Таск | Complexity | Подзадач | Depends |
|---|------|-----------|----------|---------|
| **Startup** |
| 6.00 | Welcome / Project Picker | Standard | 8 | 6.0 |
| 6.01 | Init Wizard | Standard | 7 | 6.00 |
| 6.02 | Project Switcher | Simple | 7 | 6.00, 6.1 |
| 6.03 | Top Bar / Header | Simple | 5 | 6.1 |
| 6.04 | Onboarding | Simple | 5 | 6.00 |
| 6.05 | Global Keybindings | Simple | 7 | 6.1 |
| **Каркас** |
| 6.0 | Scaffold surge-ui | Simple | 6 | core |
| 6.1 | Sidebar | Simple | 8 | 6.0 |
| 6.19 | Command Palette | Simple | 6 | 6.1 |
| 6.20 | Notifications | Simple | 6 | 6.0 |
| **Core** |
| 6.2 | Dashboard | Simple | 6 | 6.1 |
| 6.3 | Kanban Board | Standard | 8 | 6.1 |
| 6.4 | Task Detail | Standard | 7 | 6.3 |
| 6.5 | Spec Explorer | Simple | 6 | 6.1 |
| 6.6 | Spec Creation Wizard | Standard | 7 | 6.5 |
| 6.7 | Agent Hub | Simple | 7 | 6.1 |
| **Execution** |
| 6.8 | Agent Terminals | Standard | 8 | 6.1 |
| 6.9 | Live Execution | Standard | 8 | 6.1 |
| 6.10 | Diff Viewer | Standard | 8 | 6.1 |
| 6.11 | File Explorer | Simple | 6 | 6.1 |
| **Integration** |
| 6.12 | Insights / Analytics | Standard | 7 | 6.1 |
| 6.13 | Worktrees | Simple | 6 | 6.1 |
| 6.14 | GitHub Issues | Simple | 6 | 6.1 |
| 6.15 | GitHub PRs | Standard | 7 | 6.1 |
| **Extras** |
| 6.16 | Roadmap | Simple | 6 | 6.1 |
| 6.17 | Context / Memory | Simple | 6 | 6.1 |
| 6.18 | Settings | Simple | 7 | 6.1 |

**Итого: 27 тасков, ~180 подзадач**
