# Surge — GUI Tasks (Phase 6) — Part 2

## Task 6.8: Agent Terminals
**Complexity:** Standard | **Depends on:** 6.1, surge-acp

- [ ] Split view через Dock с resizable panels
- [ ] Каждый терминал: agent name + status header
- [ ] Syntax highlighting для кода (Editor readonly mode)
- [ ] Tool call cards: file write/bash/read с разворачивающимся содержимым
- [ ] Input field: отправка сообщения агенту
- [ ] Session history dropdown
- [ ] Auto-scroll с toggle
- [ ] Ctrl+F поиск по выводу

## Task 6.9: Live Execution Monitor
**Complexity:** Standard | **Depends on:** 6.1, surge-orchestrator

- [ ] Dependency graph: Wry WebView + D3.js force-directed layout
- [ ] Node states: Pending(синий), Running(жёлтый пульс), Done(зелёный), Failed(красный)
- [ ] Parallel lanes визуализация
- [ ] Right panel: streaming лог (List auto-scroll)
- [ ] Top bar: прогресс X/Y, ETA, tokens, cost
- [ ] Controls: Pause, Resume, Skip, Cancel
- [ ] Клик на node → подсветка лога
- [ ] SurgeEvent subscription

## Task 6.10: Diff Viewer
**Complexity:** Standard | **Depends on:** 6.1, surge-git

- [ ] Side-by-side: gpui-component Editor в двух split панелях
- [ ] Base (left) vs worktree (right) из surge-git
- [ ] Tree-sitter syntax highlighting
- [ ] Line numbers с цветами: зелёный/красный/серый
- [ ] Scroll sync между панелями
- [ ] File navigator слева: список файлов с +/- counts
- [ ] Фильтры: Added / Modified / Deleted
- [ ] Open in IDE кнопка per file

## Task 6.11: File Explorer
**Complexity:** Simple | **Depends on:** 6.1, surge-git

- [ ] Tree view: виртуализированный List
- [ ] Status icons: A/M/D/R per file
- [ ] +/- line counts
- [ ] Группировка: by subtask / by directory / flat (toggle)
- [ ] Клик → Diff Viewer
- [ ] Context menu: Open in IDE, Copy Path, Revert

## Task 6.12: Insights / Analytics
**Complexity:** Standard | **Depends on:** 6.1

- [ ] Token usage: gpui-px Bar chart по агентам
- [ ] Cost: gpui-px Line chart по дням
- [ ] Agent comparison: grouped Bar
- [ ] QA metrics: Pie chart (first-pass rate)
- [ ] Performance table: gpui-component Table
- [ ] Period selector: Today/Week/Month/All (ToggleGroup)
- [ ] Export CSV кнопка

## Task 6.13: Worktrees Panel
**Complexity:** Simple | **Depends on:** 6.1, surge-git

- [ ] Карточки worktrees: spec name, branch, status, files count
- [ ] Actions: Open IDE, Terminal, Diff, Merge, Discard
- [ ] Orphan detection с warning badge
- [ ] Bulk: Merge All, Discard All, Prune
- [ ] Disk usage per worktree
- [ ] Confirmation dialog для destructive actions

## Task 6.14: GitHub Issues
**Complexity:** Simple | **Depends on:** 6.1

- [ ] Table: title, labels, assignee, date
- [ ] Фильтры: label, milestone, state, assignee
- [ ] Search с debounce
- [ ] Convert to Spec кнопка → Wizard
- [ ] Status sync badge
- [ ] Infinite scroll

## Task 6.15: GitHub PRs
**Complexity:** Standard | **Depends on:** 6.1, surge-git

- [ ] PR list: карточки с status, CI, reviews
- [ ] PR detail: markdown description, files, CI checks
- [ ] Inline diff (переиспользовать 6.10)
- [ ] AI Review кнопка → результат inline
- [ ] Merge options: squash/merge/rebase с commit message
- [ ] Auto-cleanup: delete branch, cleanup worktree, archive spec
- [ ] View on GitHub кнопка

## Task 6.16: Roadmap
**Complexity:** Simple | **Depends on:** 6.1

- [ ] Phase sections: collapsible группы
- [ ] Feature cards: название, priority, status
- [ ] Convert to Task кнопка
- [ ] Drag-to-reorder внутри фазы
- [ ] Timeline: gpui-px horizontal Bar
- [ ] Generate Roadmap кнопка (AI)

## Task 6.17: Context / Memory
**Complexity:** Simple | **Depends on:** 6.1

- [ ] Entry list: карточки (decisions, patterns, gotchas)
- [ ] Full-text search через SQLite FTS5
- [ ] Category tabs: Decisions, Patterns, Gotchas, QA, Manual
- [ ] Add entry dialog: title, category, markdown content
- [ ] Auto-captured badge
- [ ] Relevance indicator

## Task 6.18: Settings
**Complexity:** Simple | **Depends on:** 6.1

- [ ] Tabs: General, Agents, Pipeline, Git, Notifications, Appearance
- [ ] General: default IDE, paths, cleanup policy
- [ ] Agents: editable list, routing rules Table
- [ ] Pipeline: gates toggles, max QA, max parallel, timeout
- [ ] Git: default branch, worktree dir, auto-cleanup
- [ ] Notifications: Telegram token, Discord webhook, API toggle
- [ ] Appearance: theme, sidebar, font size

## Task 6.19: Command Palette
**Complexity:** Simple | **Depends on:** 6.1

- [ ] Ctrl+K → Dialog overlay с fuzzy search
- [ ] Категории: Navigation, Tasks, Agents, Git, Settings
- [ ] Команды: New Task, Run, Merge, Discard, Open IDE, Toggle Sidebar
- [ ] Recent: последние 5 команд сверху
- [ ] Keyboard navigation: arrows + Enter
- [ ] Input с filtered list

## Task 6.20: Notification Toasts
**Complexity:** Simple | **Depends on:** 6.0

- [ ] Правый нижний угол: Info/Success/Warning/Error/Review
- [ ] Auto-dismiss 5s для Info/Success
- [ ] Persistent для Error/Review
- [ ] Action buttons на Review: Approve, View Diff, Dismiss
- [ ] Max 3 одновременно, остальные в очереди
- [ ] SurgeEvent subscription

---

## Summary

| Wave | Таски | Фокус |
|------|-------|-------|
| 1 | 6.0, 6.1, 6.20, 6.19 | Каркас: окно, навигация, notifications, palette |
| 2 | 6.2, 6.3, 6.4, 6.7 | Core: Dashboard, Kanban, Task Detail, Agent Hub |
| 3 | 6.5, 6.6, 6.9 | Spec: Explorer, Wizard, Live Execution |
| 4 | 6.10, 6.11, 6.13, 6.15 | Review: Diff, Files, Worktrees, PRs |
| 5 | 6.12, 6.8, 6.14, 6.16, 6.17, 6.18 | Polish: Insights, Terminals, Issues, Roadmap, Context, Settings |

**21 таск, ~140 подзадач**
