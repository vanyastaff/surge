# UI ↔ Core Data Integration Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace demo/hardcoded data in surge-ui with real data from surge-core, surge-acp, surge-git, surge-orchestrator crates via a shared AppState.

**Architecture:** Create an `AppState` struct that holds live references to `AgentPool`, `HealthMonitor`, `Registry`, `GitManager`, and `SurgeConfig`. UI screens read from AppState. Events from `broadcast::Receiver<SurgeEvent>` update state in real-time. No new data models — reuse existing types directly.

**Tech Stack:** Rust, GPUI Entity system, tokio::sync::broadcast, surge-core types

---

## Overview

Current state: all UI screens use inline demo data (hardcoded structs in `new()`).
Target state: screens read from shared `AppState` which connects to real backend crates.

### Layers

```
┌─────────────────────────────────────────────┐
│  UI Screens (read-only views)               │
│  dashboard.rs, kanban.rs, agent_hub.rs ...  │
├─────────────────────────────────────────────┤
│  AppState (shared state, Entity<AppState>)  │
│  agents, tasks, specs, worktrees, config    │
├─────────────────────────────────────────────┤
│  Backend Crates (data sources)              │
│  surge-core, surge-acp, surge-git,         │
│  surge-orchestrator                         │
└─────────────────────────────────────────────┘
```

### Non-goals (this plan)
- Real ACP agent connections (needs running agents)
- Full orchestrator pipeline execution
- SQLite persistence
- Real git worktree operations

### Goals (this plan)
- AppState struct with typed data
- Screens read from AppState instead of hardcoded data
- Event subscription wiring (framework, not real events yet)
- Agent Registry integration (real `which` detection)
- SurgeConfig loading from `surge.toml`
- Type reuse — no duplicate structs between UI and core

---

## Task 1: Create `app_state.rs` — shared state

**Files:**
- Create: `crates/surge-ui/src/app_state.rs`
- Modify: `crates/surge-ui/src/main.rs` (add mod)
- Modify: `crates/surge-ui/Cargo.toml` (add tokio dependency)

**What it does:**
Central state that all screens can reference. Holds data from backend crates.

```rust
// crates/surge-ui/src/app_state.rs
use std::path::PathBuf;
use surge_core::{SurgeConfig, SurgeEvent, TaskState, Spec, SpecId, TaskId};
use surge_acp::{Registry, RegistryEntry, DetectedAgent, AgentHealth, HealthMonitor, AgentCapability};

pub struct AppState {
    // Project
    pub project_path: Option<PathBuf>,
    pub config: Option<SurgeConfig>,

    // Agents (from Registry — real `which` detection)
    pub registry: Registry,
    pub installed_agents: Vec<DetectedAgent>,
    pub health: HealthMonitor,

    // Tasks (in-memory for now, SQLite later)
    pub tasks: Vec<TaskEntry>,
    pub specs: Vec<Spec>,

    // Git
    pub worktrees: Vec<WorktreeEntry>,
    pub current_branch: String,

    // Event channel
    pub event_tx: tokio::sync::broadcast::Sender<SurgeEvent>,
}

pub struct TaskEntry {
    pub id: TaskId,
    pub spec_id: SpecId,
    pub title: String,
    pub state: TaskState,
    pub agent: Option<String>,
    pub created_at: String,
}

pub struct WorktreeEntry {
    pub spec_id: String,
    pub branch: String,
    pub path: PathBuf,
    pub exists: bool,
}
```

**Key decisions:**
- `AppState` is a GPUI `Entity<AppState>` — screens hold `Entity<AppState>` reference
- `Registry::builtin()` provides catalog, `detect_installed()` checks PATH
- `HealthMonitor` tracks agent health (starts empty, fills on first use)
- Tasks/specs start empty, populated when project is opened
- `broadcast::Sender` for event dispatch (screens subscribe)

**Step 1:** Create the file with struct + constructor
**Step 2:** Add `mod app_state` to main.rs
**Step 3:** Add `tokio` to Cargo.toml dependencies
**Step 4:** `cargo check -p surge-ui`
**Step 5:** Commit

---

## Task 2: Wire AppState into SurgeApp

**Files:**
- Modify: `crates/surge-ui/src/app.rs`
- Modify: `crates/surge-ui/src/main.rs`

**What it does:**
Create `Entity<AppState>` at startup, pass to SurgeApp, make available to screens.

```rust
// In main.rs — create AppState before SurgeApp
let app_state = cx.new(|_| AppState::new());
let view = cx.new(|cx| SurgeApp::new(app_state.clone(), cx));

// In app.rs — store in SurgeApp
pub struct SurgeApp {
    state: Entity<AppState>,
    // ... existing fields
}
```

**Key change:** When `open_project(path)` is called:
1. Load `SurgeConfig::load()` from surge.toml
2. Detect installed agents via `Registry::detect_installed()`
3. Update AppState
4. Notify screens to re-read

**Step 1:** Add `state: Entity<AppState>` to SurgeApp
**Step 2:** Create it in main.rs, pass to SurgeApp::new()
**Step 3:** In `open_project()`, call `state.update(cx, |s, _| { s.load_project(path) })`
**Step 4:** `cargo check -p surge-ui`
**Step 5:** Commit

---

## Task 3: Agent Hub — use Registry + HealthMonitor

**Files:**
- Modify: `crates/surge-ui/src/screens/agent_hub.rs`
- Remove: inline demo data structs (ConfiguredAgent, AvailableAgent, etc.)

**What it does:**
Agent Hub reads from `AppState.registry` and `AppState.health` instead of hardcoded data.

**Before:**
```rust
pub struct AgentHubScreen {
    configured: Vec<ConfiguredAgent>,  // hardcoded
    available: Vec<AvailableAgent>,    // hardcoded
}
```

**After:**
```rust
pub struct AgentHubScreen {
    state: Entity<AppState>,
    selected: Option<usize>,
    active_tab: HubTab,
    search: String,
    filter: CatalogFilter,
}
```

**Mapping:**
- `ConfiguredAgent` → `DetectedAgent` from `state.installed_agents` + `AgentHealth` from `state.health`
- `AvailableAgent` → `RegistryEntry` from `state.registry.list()` filtered to NOT installed
- `ModelOption`, `EffortLevel`, `PermissionSetting` — keep as UI-only types (agent-specific display config)
- `AgentUsage` — keep as UI-only type (populated from health data)

**Key:** Keep UI-specific display types (badges, colors, sections) but populate them from real data.

**Step 1:** Add `state: Entity<AppState>` to AgentHubScreen
**Step 2:** Replace `configured` with `state.read(cx).installed_agents`
**Step 3:** Replace `available` with registry entries not in installed
**Step 4:** Replace health stats with `state.read(cx).health.get_health(name)`
**Step 5:** `cargo check -p surge-ui`
**Step 6:** Commit

---

## Task 4: Dashboard — use AppState task counts

**Files:**
- Modify: `crates/surge-ui/src/screens/dashboard.rs`

**What it does:**
Dashboard reads task counts, agent status, and recent activity from AppState.

**Mapping:**
- `TaskCounts` → computed from `state.tasks.iter().filter(|t| matches!(t.state, ...))`
- `AgentSummary` → from `state.installed_agents` + `state.health`
- `ActivityEntry` → from SurgeEvent stream (last N events)

---

## Task 5: Kanban Board — use AppState tasks

**Files:**
- Modify: `crates/surge-ui/src/screens/kanban.rs`

**What it does:**
Kanban reads tasks from AppState, maps TaskState to KanbanColumn.

**Mapping:**
```rust
fn task_state_to_column(state: &TaskState) -> KanbanColumn {
    match state {
        TaskState::Draft => KanbanColumn::Draft,
        TaskState::Planning | TaskState::Planned { .. } => KanbanColumn::Planning,
        TaskState::Executing { .. } => KanbanColumn::Executing,
        TaskState::QaReview | TaskState::QaFix { .. } => KanbanColumn::QaReview,
        TaskState::HumanReview => KanbanColumn::HumanReview,
        TaskState::Completed | TaskState::Merging => KanbanColumn::Done,
        TaskState::Failed { .. } | TaskState::Cancelled => KanbanColumn::Done,
    }
}
```

---

## Task 6: Settings — use SurgeConfig

**Files:**
- Modify: `crates/surge-ui/src/screens/settings.rs`

**What it does:**
Settings displays real config from `state.config` (SurgeConfig loaded from surge.toml).

---

## Task 7: Worktrees — use GitManager data

**Files:**
- Modify: `crates/surge-ui/src/screens/worktrees.rs`

**What it does:**
Worktrees screen reads from `state.worktrees` (populated via GitManager::list_worktrees()).

---

## Task 8: Event subscription — real-time updates

**Files:**
- Modify: `crates/surge-ui/src/app_state.rs`
- Modify: `crates/surge-ui/src/app.rs`

**What it does:**
Wire `broadcast::Receiver<SurgeEvent>` to update AppState on events.

```rust
impl AppState {
    pub fn handle_event(&mut self, event: SurgeEvent, cx: &mut Context<Self>) {
        match event {
            SurgeEvent::TaskStateChanged { task_id, new_state, .. } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                    task.state = new_state;
                }
                cx.notify(); // triggers UI re-render
            }
            SurgeEvent::AgentConnected { agent_name } => {
                self.health.register(&agent_name);
                cx.notify();
            }
            // ... other events
        }
    }
}
```

**Key:** `cx.notify()` on AppState tells GPUI to re-render any screen that reads it.

---

## Task 9: Spec Explorer — use AppState specs

**Files:**
- Modify: `crates/surge-ui/src/screens/spec_explorer.rs`

---

## Task 10: Live Execution — use event stream

**Files:**
- Modify: `crates/surge-ui/src/screens/live_execution.rs`

**What it does:**
Subscribe to SurgeEvent stream, build execution graph from SubtaskStarted/SubtaskCompleted events.

---

## Execution Order

```
Task 1: app_state.rs (foundation)
  → Task 2: Wire into SurgeApp
    → Task 3: Agent Hub (highest value — real which detection)
    → Task 4: Dashboard
    → Task 5: Kanban
    → Task 6: Settings
    → Task 7: Worktrees
    → Task 8: Event subscription
    → Task 9: Spec Explorer
    → Task 10: Live Execution
```

Tasks 3-10 are independent after Task 2 — can be done in any order.

---

## Type Reuse Summary

| UI currently uses | Replace with | From crate |
|---|---|---|
| `ConfiguredAgent` | `DetectedAgent` + `AgentHealth` | surge-acp |
| `AvailableAgent` | `RegistryEntry` | surge-acp |
| `AgentCapability` (string) | `AgentCapability` enum | surge-acp |
| `KanbanTask.column` | computed from `TaskState` | surge-core |
| `SpecCard` | `Spec` | surge-core |
| `WorktreeEntry` | `WorktreeInfo` | surge-git |
| `SurgeConfig` fields | `SurgeConfig` directly | surge-core |
| `SessionEntry` | from `SurgeEvent` stream | surge-core |

Types that STAY in UI (display-only):
- `ModelOption`, `EffortLevel`, `PermissionSetting` — agent-specific UI config
- `AgentUsage` enum — display wrapper over health data
- `KanbanColumn`, `HubTab`, `CatalogFilter` — pure UI state
- Theme colors, badge helpers — pure presentation
