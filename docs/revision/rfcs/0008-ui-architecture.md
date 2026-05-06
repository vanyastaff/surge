# RFC-0008 · UI Architecture

## Overview

surge has two desktop UIs:

1. **Editor** — for building and editing pipeline graphs (canvas-based, egui)
2. **Runtime / Replay** — for monitoring active runs and reviewing finished ones (gpui)

Plus a CLI for headless interaction. This document specifies the desktop UIs.

## Why two GUI stacks

The two UIs have different requirements:

| | Editor | Runtime |
|---|---|---|
| Mode | Power-user tool | Daily-driver |
| Layout | Canvas-heavy | Mixed (graph + lists + text) |
| Update freq | Edit-time only | Real-time stream |
| Visual ambition | Functional | Polish-critical |
| Component library | egui-snarl perfect fit | Custom GPUI |

Putting both in one stack means compromise. egui's immediate-mode is great for the canvas editor (egui-snarl gives us node graph for free), but visually constrained for runtime polish. GPUI handles real-time updates and refined visual design well, but lacks node-graph components — building a canvas in GPUI is a 1-2 month project on its own.

The **trade-off is single binary vs visual quality**. We choose two binaries; users open the editor occasionally to design pipelines, run the runtime daily.

## Editor (egui)

### Purpose

Build and edit pipeline graphs visually. Open existing `flow.toml`, modify, save.

### Stack

- **Framework**: egui (latest stable)
- **Canvas component**: egui-snarl (node-graph widget with ports, edges, pan/zoom built-in)
- **Window/native**: eframe (egui's native wrapper, cross-platform)
- **TOML serialization**: serde + toml-edit (preserves comments and formatting)

### Layout

```
┌─────────────────────────────────────────────────────────────┐
│ TOP BAR · surge editor · ~/projects/myapp/flow.toml [💾]│
├──────────┬──────────────────────────────────┬───────────────┤
│ SIDEBAR  │                                  │ INSPECTOR     │
│          │                                  │               │
│ Projects │                                  │ Selected node │
│ ──────── │                                  │   config      │
│          │                                  │               │
│ Templates│         CANVAS                   │ Tabs:         │
│ ──────── │                                  │ - General     │
│          │      [node]──→[node]──→[node]    │ - Context     │
│ Library  │              ↓                    │ - Prompt      │
│  Agent   │           [node]                 │ - Tools       │
│  Gate    │              ↓                    │ - Outcomes    │
│  Branch  │           [terminal]             │ - Approvals   │
│  ...     │                                  │ - Hooks       │
│          │                                  │ - Sandbox     │
│          │                                  │               │
└──────────┴──────────────────────────────────┴───────────────┘
```

### Canvas behaviors

- **Pan**: middle-click drag or space+drag
- **Zoom**: scroll wheel, ctrl+/-
- **Select node**: left click; rubber-band for multi-select
- **Move node**: drag selected
- **Connect ports**: drag from output port to input port
- **Delete edge**: select edge, press delete
- **Add node**: drag from sidebar onto canvas, or right-click → "Add node here"
- **Edit node**: double-click → opens inspector tab in main view (or right panel if pinned)
- **Validation**: live, problems highlighted in red on canvas

### Node rendering

Each node on canvas is a rectangle with:
- Header: icon + title + status dot
- Body: 1-3 lines of metadata (model, profile, key config)
- Output ports: bottom edge, one per declared outcome, color-coded
- Input port: top edge, single dot

Color coding:
- Agent: amber accent
- HumanGate: violet accent
- Branch: teal accent  
- Terminal: rose/teal accent
- Notify: yellow accent
- Loop / Subgraph: outlined container

### Inspector

The inspector is the entire node configuration UI from the mockups. Tabs based on node kind:

- **Agent**: General, Context, Prompt, Tools, Sandbox, Approvals, Hooks, Outcomes, Advanced
- **HumanGate**: General, Channels, Summary, Options
- **Branch**: General, Predicates
- **Terminal**: General, Termination kind
- **Notify**: General, Channel, Template
- **Loop**: General, Iteration, Body (opens body subgraph in canvas)
- **Subgraph**: General, Inputs/Outputs, Body

Inspector is right-panel by default, can be detached as floating window.

### Loop body editing

When user opens a Loop node's body:
- Main canvas zooms into the body subgraph
- Breadcrumb at top: `flow.toml › milestone_loop.body`
- "Back to outer" button to return
- Inputs/outputs of the loop visible as virtual entry/exit nodes

### Save flow

- Cmd/Ctrl+S triggers validation; if valid, saves to `flow.toml`
- Save preserves user formatting (toml-edit roundtrip)
- "Diff vs disk" view showing pending changes before save

### Template browser

Sidebar > Templates section lists all available templates. Click template:
- Preview pane shows graph thumbnail and metadata
- "New project from template" button creates a new dir with that template's `pipeline.toml`
- "Open as starting point" opens template in editor for customization

### Out of scope for editor

- Real-time collaboration (one user at a time)
- Mobile/web editor
- Visual debugging (that's runtime UI's job)

## Runtime / Replay (gpui)

### Purpose

Monitor active runs and review completed runs. Two modes: live and replay.

### Stack

- **Framework**: gpui (Zed editor's Rust UI framework)
- **Components**: gpui-component (shadcn-like component library for GPUI)
- **Charts/graphs**: custom rendering using gpui primitives
- **Diff viewer**: custom (using gpui text rendering)
- **Mode**: retained-mode UI, suitable for streaming updates

### Layout

```
┌─────────────────────────────────────────────────────────────────┐
│ TOP BAR · run #0083 · sample-app · [edit][live][replay] · STATUS │
├─────────────────────────────────────────────────────────────────┤
│ MODE BAR · LIVE · stage 4/7 · 14m elapsed · $3.18 · 412k tokens  │
├─────────┬───────────────────────────────────────────┬───────────┤
│ EVENTS  │                                           │ DETAIL    │
│         │ GRAPH (snapshot of current state)         │           │
│ event 1 │                                           │ Active    │
│ event 2 │   [done]→[done]→[active*]→[pending]       │ stage     │
│ event 3 │                                           │ details   │
│ ...     │                                           │           │
│         │ ────────────────────────────────────────  │ Tools     │
│         │ SCRUBBER (replay only)                    │ Cost      │
│         │ ◀ [time/seq slider] ▶  [1×] [2×] [4×]   │ Artifacts │
│         │                                           │           │
│         │ ────────────────────────────────────────  │ Fork CTA  │
│         │ BOTTOM PANEL                              │           │
│         │ Tabs: Events | Diff | Artifacts | Cost  │           │
│         │                                           │           │
└─────────┴───────────────────────────────────────────┴───────────┘
```

### Modes

#### Live

For runs currently executing.

- Top bar: amber accent indicating live
- Events list updates as new events arrive (auto-scrolling, pause on user scroll)
- Graph: highlights currently active node with pulse animation
- Bottom panel: tail of logs / streaming tool calls
- Detail panel: live tool calls of active session, current cost
- Fork: disabled (can't fork a live run; stop it first)

#### Replay

For runs completed (success or failure).

- Top bar: violet accent indicating replay
- Events list: cursor at current scrub position; future events dimmed
- Graph: snapshot at scrub position (completed nodes teal, active-at-cursor amber, future nodes dashed)
- Scrubber: full timeline with event marks at key transitions
- Bottom panel: diff/artifacts/cost relative to scrub position
- Detail panel: state at cursor; tool calls from past with relative times (`-9.2s`)
- Fork CTA: prominent "Fork from here" button

#### Edit (placeholder)

Clicking "edit" mode in top bar opens the editor binary in a new window for the same `flow.toml`. Two windows, one tool each — no inline editing of structure in runtime view.

### Live update mechanism

- Runtime UI subscribes to event log via SQLite trigger or file watcher
- New events trigger UI updates (incremental, only changed components)
- Backpressure: if events arrive faster than UI can render, batch updates at 60fps boundary

### Scrubber UX

- Drag the cursor: continuously updates state at scrubbed position
- Click on event mark: jump to that seq
- Keyboard: `←/→` step events, `Shift+←/→` step stages, `Home/End` jump to start/end
- Speed selector: `0.5×, 1×, 2×, 4×` for play-mode (auto-advance through events at given speed)
- Play button: starts replay animation, useful for understanding flow timing

### Diff view

When user is on a stage that produced file changes, bottom panel "Diff" tab shows:

- Per-file unified diffs with syntax highlighting
- Line numbers (old, new)
- Hunk headers
- File metadata: stage attribution, size delta
- Toggle: unified ↔ split view

When in replay mode comparing two runs:

- Side-by-side graphs (run A vs run B)
- Divergence point highlighted
- Side-by-side artifact diffs

### Artifacts viewer

Bottom panel "Artifacts" tab lists all artifacts produced by the run:

- Group by stage
- Click artifact to preview in panel
- Markdown rendered, code highlighted
- Open externally button (opens in default app)
- Copy path / copy contents

### Cost & tokens chart

Bottom panel "Cost" tab shows:

- Stacked bar chart per stage (prompt tokens, output tokens, cache hits)
- Cumulative cost line
- Per-stage breakdown table
- Estimated remaining cost (live mode only)

### Fork-from-here

The killer feature. Clicking the violet CTA on detail panel:

1. Opens a confirmation dialog with what will happen
2. User can edit prompts/profiles for nodes that will be re-executed
3. Confirms → engine creates new run, copies events 1..=cursor, branches worktree
4. New run opens in new tab/window

### Multiple runs

Sidebar shows list of active and recent runs. Switching between them swaps the entire UI to that run's data. Per-run state (scrubber position, selected tab) persists across switches.

## CLI

### Commands

```
surge init                         # initialize project (creates pipeline.toml or selects template)
surge run <description>            # start a new run
surge run --template <name> <desc> # use specific template, skip bootstrap
surge list                         # list runs (active + recent)
surge status <run_id>              # show current state of a run
surge attach <run_id>              # tail logs of a running run
surge cancel <run_id>              # abort a run
surge replay <run_id>              # open replay mode in runtime UI
surge fork <run_id> --at <seq>     # fork from event seq
surge profile <subcmd>             # profile management
surge template <subcmd>            # template management
surge telegram setup               # set up Telegram bot
surge telegram test                # send test message
surge doctor                       # diagnose common issues
```

### Output formats

- Default: human-readable (color, formatted)
- `--json`: machine-readable JSON for scripting
- `--quiet`: suppress non-essential output

### Daemon mode

`surge run` starts a daemon subprocess for the run that persists across CLI invocations. CLI returns control immediately after run starts. Subsequent `surge attach` reconnects to view live progress.

Daemon lifecycle:
- Linux/macOS: daemon detaches via `setsid`, parent process can exit
- Windows: spawned as detached process

Daemon writes PID + status to `~/.surge/runs/<run_id>/.daemon`. CLI uses this to communicate.

## Cross-cutting concerns

### Theming

Both UIs use the same color palette (control-room dark theme):

```
bg-0:    #0a0c10  (deep background)
bg-1:    #0e1116  (panel background)
bg-2:    #13171e  (card background)
bg-3:    #181d26  (interactive background)
text-1:  #e8eaef  (primary text)
text-2:  #a0a6b1  (secondary text)
text-3:  #6a7180  (tertiary)
text-4:  #444b58  (disabled)
amber:   #ff9d4a  (active/highlight)
teal:    #4ad6b8  (success/completed)
rose:    #ff5e7a  (error/failure)
violet:  #9b8cff  (replay/special)
yellow:  #f5c969  (warning)
```

Light theme: not in v1.0 (dark only).

### Typography

- UI: Geist (sans-serif, modern)
- Mono: JetBrains Mono (code, IDs, timestamps)

### Accessibility

- Keyboard navigation throughout
- Focus indicators visible
- Color isn't the only distinguishing signal (icons + text accompany colors)
- Sufficient contrast (WCAG AA)

### Persistence of UI state

Editor remembers:
- Last opened file
- Window size/position
- Inspector pin state
- Sidebar width

Runtime remembers:
- Last open run
- Per-run scrubber position
- Selected bottom panel tab

State stored in `~/.surge/ui-state.toml`.

## Implementation phasing

### v0.1 (MVP)

- CLI fully functional (all commands work)
- Telegram bot fully functional
- Editor: read-only mode, can view existing `flow.toml`
- Runtime: live mode only, no replay/scrubber

### v0.2

- Editor: full edit mode with inspector
- Runtime: replay mode with scrubber

### v0.3

- Diff view (compare runs)
- Fork-from-here feature
- Template browser in editor

### v1.0

- Full feature parity with this RFC
- Polished UX, accessibility audit pass
- Multi-platform binaries

## Open questions

### Drag-drop between editor and runtime

Could a node from runtime be drag-dropped to editor for inspection? Probably not — different processes, different state. Use deeplinks instead (right-click in runtime → "Open in editor" → spawns editor with cursor on that node).

### Floating windows

egui supports multiple windows in single process; gpui similar. Useful for inspecting inspector while editing canvas. Decide based on user feedback in v0.2.

### Touch support

Out of scope. v1 is desktop-only with mouse + keyboard.

### Web build

egui can target WASM, gpui cannot (yet). Out of scope for v1. If demand emerges in v2, build a separate web view of runtime data.

## Acceptance criteria

The UI architecture is correctly implemented when:

1. Editor binary opens any valid `flow.toml` and renders it correctly with all node types, edges, and positions.
2. Editing on canvas (add/move/delete nodes, connect ports) saves valid `flow.toml`.
3. Runtime binary connects to a live run within 2 seconds of `surge attach`.
4. Live updates render at 60fps for runs producing up to 10 events/second.
5. Replay scrubber smoothly transitions between events with state correctly snapshotted at any cursor position.
6. Fork-from-here creates a new run with state matching the cursor position, verified by event log comparison.
7. Both binaries build and run on Linux, macOS, Windows.
8. Cold-start time < 1 second for both UIs.
