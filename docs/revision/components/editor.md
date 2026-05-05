# Component · Editor

## Overview

The visual graph editor for building and editing pipelines. Built with **egui + egui-snarl + eframe**.

This document specifies the editor's architecture, layout, behaviors, and integration with the rest of the system. It complements RFC-0008 (UI architecture).

## Goals

- View any valid `flow.toml` rendered correctly
- Edit graphs (add/remove nodes, connect/disconnect edges)
- Inspect and edit node configuration
- Real-time validation with visual feedback
- Save changes back to TOML preserving formatting
- Cross-platform native binary (Linux, macOS, Windows)

## Stack

- **egui 0.27+** — immediate-mode UI framework
- **eframe** — native wrapper (handles window creation, OS integration)
- **egui-snarl 0.4+** — node-graph widget with ports, edges, pan/zoom
- **toml_edit** — preserve formatting on save

## Architecture

```rust
// crates/editor/src/main.rs

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("vibe·flow editor"),
        ..Default::default()
    };
    eframe::run_native("vibe-editor", options, Box::new(|cc| Box::new(App::new(cc))))
}

struct App {
    canvas: CanvasState,
    inspector: InspectorState,
    sidebar: SidebarState,
    project: Option<ProjectContext>,
    document: Option<Document>,           // current flow.toml
    storage: Arc<Storage>,
    theme: Theme,
    modal: Option<Modal>,                 // currently-open modal, if any
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_global_shortcuts(ctx);
        
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| self.render_top_bar(ui));
        egui::SidePanel::left("sidebar").show(ctx, |ui| self.render_sidebar(ui));
        egui::SidePanel::right("inspector").show(ctx, |ui| self.render_inspector(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.render_canvas(ui));
        
        if let Some(modal) = &self.modal {
            self.render_modal(ctx, modal);
        }
    }
}
```

## Layout

```
┌──────────────────────────────────────────────────────────────────────────┐
│ TOP BAR · vibe·flow · ~/projects/myapp/flow.toml · [💾] · validation   │
├──────────┬─────────────────────────────────────────────────┬───────────────┤
│ SIDEBAR  │                                                 │ INSPECTOR     │
│          │                                                 │               │
│ Files    │                                                 │ Tabs:         │
│ ──────── │                CANVAS                           │ - General     │
│          │                                                 │ - Context     │
│ Project  │                                                 │ - Prompt      │
│ ──────── │  [agent] ──→ [agent] ──→ [agent]                │ - Tools       │
│          │             ↓                                   │ - Sandbox     │
│ Library  │         [human gate]                            │ - Approvals   │
│ Agent    │             ↓                                   │ - Hooks       │
│ Gate     │         [terminal]                              │ - Outcomes    │
│ Branch   │                                                 │ - Advanced    │
│ ...      │                                                 │               │
│          │                                                 │ [field 1]     │
│ Templates│                                                 │ [field 2]     │
│          │                                                 │ ...           │
└──────────┴─────────────────────────────────────────────────┴───────────────┘
```

## Top bar

Components left-to-right:
- App icon
- File name + path (clickable to copy)
- Save button (highlighted if dirty)
- Undo/redo buttons
- Validation indicator (green check / red errors / yellow warnings count)
- Project context badge ("connected to: sample-app")
- Window controls (depends on platform)

## Sidebar

Three sections:

### Files

- Recent files list
- "Open file..." button
- "New from template..." button

### Library (drag source for canvas)

Categorized list of node types and roles:

```
▼ Agents
  · Spec Author       (drag → spawn Agent node with profile=spec-author)
  · Architect
  · Implementer
  · Test Author
  · Verifier
  · Reviewer
  · PR Composer
  · Generic Agent     (drag → Agent with no profile, configure manually)

▼ Gates
  · Human Gate

▼ Flow
  · Branch
  · Terminal
  · Loop
  · Subgraph

▼ I/O
  · Notify
```

Each entry shows icon + name. Hover shows tooltip with description. Drag onto canvas spawns a node at drop position with sensible defaults from the profile (if any).

### Templates

Browse installed templates. Click to:
- Preview graph (read-only canvas rendering)
- "New project from template" → opens project chooser
- "Open" → loads template's pipeline.toml in editor

## Canvas

Primary work surface. Built on egui-snarl which provides:
- Node rendering with custom widgets per type
- Port-based connections (drag from output to input)
- Pan/zoom
- Multi-select
- Group operations

### Node rendering

Each node is rendered with:

```rust
impl SnarlViewer<NodeData> for VibeFlowViewer {
    fn show_header(&mut self, node: NodeId, ...) {
        ui.horizontal(|ui| {
            ui.label(node.icon());
            ui.label(node.title());
            self.render_status_dot(ui, node);
        });
    }
    
    fn show_body(&mut self, ...) {
        // 2-3 lines of metadata: profile name, model, key config
    }
    
    fn outputs(&mut self, node: NodeId) -> Vec<OutPin> {
        // One pin per declared outcome, color-coded
        node.declared_outcomes.iter().map(|o| OutPin {
            label: o.id.clone(),
            color: outcome_color(o.edge_kind_hint),
        }).collect()
    }
    
    fn inputs(&mut self, node: NodeId) -> Vec<InPin> {
        // Single input pin (top-center)
        vec![InPin::default()]
    }
}
```

### Color coding

| NodeKind  | Accent      |
|-----------|-------------|
| Agent     | Amber       |
| HumanGate | Violet      |
| Branch    | Teal        |
| Terminal  | Rose / Teal |
| Notify    | Yellow      |
| Loop      | Outlined    |
| Subgraph  | Outlined    |

Edge colors by kind:
- Forward: white/light gray
- Backtrack: amber
- Escalate: violet

### Selection

- Click node: select it
- Shift-click: add to selection
- Drag empty space: rubber-band selection
- Cmd/Ctrl-A: select all
- Escape: deselect

Selected nodes render with brighter border. Inspector shows config for selected node (or message "Multiple selected" if many).

### Edge creation

Drag from an output port → drop on an input port → creates edge. Validation runs immediately:
- If connection violates rules: edge appears red, hover shows error
- If valid: edge accepts, written to graph state

Drag from input port → output port also works (reversed direction).

### Edge deletion

Select edge, press Delete. Or right-click edge → Delete.

### Pan/zoom

- Pan: middle-click drag, or space + left-click drag
- Zoom: scroll wheel
- Reset zoom: cmd/ctrl+0
- Fit to view: cmd/ctrl+F (auto-zoom to fit all nodes)

### Loops on canvas

Loop nodes render as **collapsed groups**:
- Outer rectangle with title and iterator info
- Body subgraph not visible
- Click "Expand" or double-click → opens body subgraph in main canvas
- Breadcrumb at top: `flow.toml › milestone_loop.body`
- "Back to outer" button to return

### Validation overlays

Live validation runs as user edits. Issues are highlighted on canvas:

- **Red border** on a node: validation error specific to it
- **Red dangling outcome port**: declared outcome with no edge
- **Yellow border**: warning (e.g., backtrack edge without max_traversals)
- **Tooltip on hover**: full error message with rule reference

Top bar's validation indicator aggregates: shows total error/warning counts. Click to show full validation panel.

## Inspector

Right panel, tabbed UI for the selected node's configuration. Tabs depend on `NodeKind`.

### Agent inspector tabs

#### General

- Node ID (read-only after creation, shown for reference)
- Display name
- Profile selector (dropdown listing installed profiles)
- Position (X, Y)

#### Context

- Bindings list
- For each: source picker (NodeOutput / RunArtifact / GlobPattern / Static)
  + target template variable name
- Add/remove bindings
- Visual indicator of bound vs. expected (if profile declares expected bindings)

#### Prompt

- Read-only preview of resolved system prompt
- "Override prompt" toggle
- If overriding: text editor for custom prompt
- Variable autocomplete (e.g., type `{{` shows available bindings)
- Live preview of rendered prompt with example values

#### Tools

- MCP servers list (checkboxes for each available)
- Skills list (checkboxes)
- Shell allowlist (editable list)
- Filesystem write paths (path picker)

#### Sandbox

- Mode selector (segmented control: read-only | workspace | workspace+net | full-access | custom)
- Network allowlist (editable list of domains)
- Protected paths (editable list)
- Custom mode: detailed read/write/exec configuration

#### Approvals

- Policy dropdown (untrusted | on-request | never)
- Granular flags (toggles for each approval type)
- Channel priority list (drag-reorderable)

#### Hooks

- List of hooks for this node
- Add new hook: trigger / matcher / command / on_failure
- Test hook button (runs hook with example context)
- Inheritance display (which hooks come from profile vs node-specific)

#### Outcomes

- List of declared outcomes
- For each: ID, description, edge_kind_hint, is_terminal
- Visual indicator: ✓ if connected, ✗ if dangling
- "Add outcome" button (and ability to remove)

#### Advanced

- Limits: timeout, max_retries, circuit_breaker, max_tokens
- Custom fields (from profile's `inspector_ui` definitions)
- Node metadata for debugging

### HumanGate inspector tabs

#### General

- ID, position
- Channels list (drag-reorderable priority)

#### Summary

- Summary template editor (markdown with `{{vars}}`)
- Show artifacts list (which to embed in approval card)
- Live preview (with sample data)

#### Options

- List of approval options (one per outcome user can pick)
- For each: outcome ID, label, style
- Add/remove options

#### Advanced

- Timeout configuration
- on_timeout action
- allow_freetext toggle

### Branch inspector tabs

#### General

- ID, position
- Default outcome (if no arm matches)

#### Predicates

- List of arms (condition + outcome)
- For each: predicate builder (dropdown for predicate type, fields for params)
- Live evaluation preview against current run state (if attached to live run)

### Terminal, Notify, Loop, Subgraph

Similar tab structure, simplified per their config schema.

## File operations

### Open

- Cmd/Ctrl+O: file picker
- Loads TOML, validates, populates canvas
- If invalid: shows error in modal with location

### Save

- Cmd/Ctrl+S: writes back to file using `toml_edit` (preserves comments and formatting)
- If validation fails: blocks save with error message
- If valid but warnings: prompts user (Save anyway / Cancel)
- Atomic write: writes to temp, renames

### New from template

File menu → "New from template" → modal with template browser → select → opens new untitled document with template's flow.

### Recent files

Last 10 files in File menu and sidebar.

## Live run integration

If a flow.toml is currently running (active run uses this graph), the editor can show live state:

- Top bar: "📡 Connected to run #0083"
- Canvas: nodes show real-time status (running with pulse, completed teal, failed red)
- Click "Open in runtime" → spawns vibe-runtime in a new window for that run

Editor doesn't allow saving changes to a flow that's actively running (changes would create inconsistency). User must either save as different file or wait for run to finish.

## Undo/redo

History of editor operations:
- Add node
- Remove node
- Move node
- Add edge
- Remove edge
- Edit node config field

Cmd/Ctrl+Z: undo. Cmd/Ctrl+Shift+Z: redo.

History stored in memory (not persisted). Limit: 100 operations.

## Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| Cmd/Ctrl+S | Save |
| Cmd/Ctrl+O | Open |
| Cmd/Ctrl+N | New from template |
| Cmd/Ctrl+Z | Undo |
| Cmd/Ctrl+Shift+Z | Redo |
| Cmd/Ctrl+A | Select all |
| Cmd/Ctrl+0 | Reset zoom |
| Cmd/Ctrl+F | Fit to view |
| Cmd/Ctrl+/ | Toggle inspector |
| Cmd/Ctrl+\\ | Toggle sidebar |
| Delete | Delete selection |
| Escape | Deselect |
| Space + drag | Pan |
| F2 | Rename selected node |

## Persistence

UI state stored in `~/.vibe/ui-state.toml`:

```toml
[editor]
last_opened_file = "/path/to/flow.toml"
window_size = [1280, 800]
window_position = [100, 100]
sidebar_width = 240
inspector_width = 360
recent_files = [...]
```

## Performance

- Canvas: render only visible nodes (egui-snarl handles culling)
- Validation: incremental (only re-validate affected nodes on edit)
- Auto-save: not in v1 (user explicitly saves)
- Large graphs (50+ nodes): test for smoothness, optimize if needed

## Acceptance criteria

The editor is correctly implemented when:

1. Loading a valid `flow.toml` renders the graph with correct positions, edges, and node types.
2. All seven NodeKind types render with appropriate visual styling.
3. Edge creation by drag-drop works smoothly with live validation.
4. Inspector for each node type shows the correct tabs with editable fields.
5. Save preserves comments and formatting (round-trip identical for unmodified content).
6. Validation runs incrementally; user sees errors within 100ms of edit.
7. Undo/redo correctly reverts each operation type.
8. Pan/zoom is smooth (60fps) on a 50-node graph.
9. Loop body editing (collapse/expand to subgraph) works correctly with breadcrumb navigation.
10. Cross-platform: identical behavior on Linux, macOS, Windows.
