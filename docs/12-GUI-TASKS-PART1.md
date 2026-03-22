# Surge — GUI Tasks (Phase 6) — Part 1

> Стек: GPUI + gpui-component + gpui-navigator + gpui-d3rs/gpui-px

## Task 6.0: Scaffold surge-ui крейт
**Complexity:** Simple | **Depends on:** surge-core

- [ ] Создать крейт `surge-ui`, добавить gpui, gpui-component, gpui-navigator
- [ ] `main.rs` — Application::new(), gpui_component::init(cx), открыть окно
- [ ] `app.rs` — SurgeApp root view с Root из gpui-component
- [ ] `theme.rs` — primary #7C3AED, surface #1A1A2E, background #0F0F17
- [ ] `router.rs` — gpui-navigator Navigator с enum Screen (16 экранов)
- [ ] Проверить cargo run -p surge-ui открывает окно с тёмной темой

## Task 6.1: Sidebar навигация
**Complexity:** Simple | **Depends on:** 6.0

- [ ] `sidebar.rs` — вертикальная панель с Lucide иконками
- [ ] Каждый пункт: иконка + название + shortcut badge
- [ ] Выделение активного пункта primary цветом
- [ ] Collapsible sidebar с toggle кнопкой
- [ ] Секция AGENTS внизу — список агентов с status dot
- [ ] Кнопка "Add Agent..."
- [ ] Клик → navigator.push(screen)
- [ ] Keyboard shortcuts Ctrl+1..9

## Task 6.2: Dashboard
**Complexity:** Simple | **Depends on:** 6.1

- [ ] `screens/dashboard.rs` — обзорный экран
- [ ] Project Health: задачи по статусам (gpui-component Card)
- [ ] Active Agents: список с Badge компонентом
- [ ] Token Budget: gpui-px Bar chart
- [ ] Quick Actions: New Task, Continue Last, Review Queue (Button)
- [ ] Recent Activity: gpui-component List с виртуализацией
- [ ] Real-time через broadcast::Receiver<SurgeEvent>

## Task 6.3: Kanban Board
**Complexity:** Standard | **Depends on:** 6.1, surge-spec

- [ ] `screens/kanban.rs` — колонки по TaskState через Dock layout
- [ ] Task карточка: название, Progress, agent badge, complexity badge
- [ ] Drag-and-drop между колонками
- [ ] Клик → Dialog с деталями
- [ ] Фильтры: по агенту, сложности через Select/ToggleGroup
- [ ] New Task кнопка → spec creation
- [ ] Счётчик задач в заголовке колонки
- [ ] Колонки: Draft → Planning → Executing → QA → Review → Done

## Task 6.4: Task Detail Modal
**Complexity:** Standard | **Depends on:** 6.3

- [ ] Dialog size Large с header: название, ID badge, status, progress
- [ ] Meta: labels, complexity, agents, duration, cost, tokens
- [ ] Tabs: Overview, Graph, Subtasks, Files, Logs, PR, Diff
- [ ] Overview: description, acceptance criteria
- [ ] Subtasks: список с state icons, agent badge, duration
- [ ] Actions: Open in IDE, Terminal, Create PR, Merge, Discard
- [ ] Real-time через SurgeEvent subscription

## Task 6.5: Spec Explorer
**Complexity:** Simple | **Depends on:** 6.1, surge-spec

- [ ] Gallery: Card grid всех спеков
- [ ] Search bar с Input
- [ ] Filter chips: status, complexity, agent (ToggleGroup)
- [ ] Template library: feature, bugfix, refactor
- [ ] Клик → Task Detail
- [ ] New Spec кнопка → creation wizard

## Task 6.6: Spec Creation Wizard
**Complexity:** Standard | **Depends on:** 6.5, surge-acp

- [ ] Multi-step Dialog
- [ ] Step 1: Description textarea
- [ ] Step 2: AI analysis → loading → предложенные subtasks
- [ ] Step 3: Review plan — editable список, drag-to-reorder, agent per subtask
- [ ] Step 4: Acceptance criteria — editable must/should
- [ ] Step 5: Confirm — summary, Create & Start / Create Draft
- [ ] Stepper индикатор прогресса

## Task 6.7: Agent Hub
**Complexity:** Simple | **Depends on:** 6.1, surge-acp

- [ ] Список агентов: карточки с status, capabilities, model
- [ ] Health metrics: latency, error rate (gpui-px mini-charts)
- [ ] Usage: tokens/cost/requests today
- [ ] Add Agent dialog: name, command, transport, test connection
- [ ] Test Connection кнопка с результатом
- [ ] Routing rules: Table с file patterns → agent
- [ ] Rate limit bar с reset time
