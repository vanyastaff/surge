# Component · Runtime UI

## Overview

The runtime/replay viewer for monitoring active runs and reviewing finished ones. Built with **gpui + gpui-component**.

This document specifies the runtime UI's modes, layout, behaviors, and the killer feature — replay with fork-from-here. It complements RFC-0008.

## Why gpui

GPUI gives polished, fast rendering suitable for daily-driver monitoring use. The runtime UI is what users open multiple times a day to check on their runs. Visual quality and responsiveness matter more than the editor's once-a-week canvas tooling.

## Modes

Three modes, switched via top bar:

### Live mode

- For runs currently executing
- Real-time event stream
- Auto-scrolling event list
- Active node highlighted with pulse animation
- Stats panel updating live (cost, tokens, elapsed time)

### Replay mode

- For runs in terminal status (completed/failed/aborted)
- Time-travel scrubber
- State at any seq position
- Diff viewer comparing to earlier states
- Fork-from-here CTA

### Edit mode (passthrough)

- Clicking "Edit" in top bar opens the editor binary in a new window for the same `flow.toml`
- Doesn't actually edit in runtime UI

## Layout

```
┌───────────────────────────────────────────────────────────────────────────┐
│ TOP BAR · run #0083 · sample-app · [edit][live][replay] · STATUS chip     │
├───────────────────────────────────────────────────────────────────────────┤
│ MODE BAR · LIVE · stage 4/7 · 14m elapsed · $3.18 · 412k tokens           │
├──────────┬─────────────────────────────────────────────┬──────────────────┤
│ EVENTS   │                                             │ DETAIL           │
│          │  GRAPH (snapshot at current seq)            │                  │
│ #001 ... │                                             │ Active stage     │
│ #002 ... │   [done]→[done]→[active*]→[pending]         │  - profile       │
│ #003 ... │                                             │  - elapsed       │
│ ...      │                                             │  - last tool     │
│          │ ──────────────────────────────────────────  │  - tokens used   │
│          │  SCRUBBER (replay mode only)                │                  │
│          │  ◀ [time/seq slider] ▶  [1×][2×][4×]      │ Cost              │
│          │                                             │  per-stage       │
│          │ ──────────────────────────────────────────  │                  │
│          │  BOTTOM PANEL                               │ Artifacts        │
│          │  Tabs: Events | Diff | Artifacts | Cost    │                  │
│          │  ────                                       │ [Fork from here] │
│          │  (tab content)                              │ (replay mode)    │
└──────────┴─────────────────────────────────────────────┴──────────────────┘
```

## Top bar

- **Run identity**: run number, project name
- **Mode tabs**: Edit / Live / Replay (radio-button-style)
- **Status chip**: ⚡ Running, ✓ Completed, ✗ Failed, ◯ Aborted
- **Right side**: notifications icon, settings icon, window controls

## Mode bar

Just below top bar, dense info bar:

- **Live mode**: `LIVE · stage X/Y · Nm elapsed · $cost · tokens`
- **Replay mode**: `REPLAY · cursor at seq N · stage X/Y · cumulative cost as of cursor`

## Events panel (left)

Scrollable list of events. Each event row:
- Seq number
- Timestamp (relative to run start in live, absolute in replay)
- Event kind icon
- Brief description (e.g., "Stage entered: implementer")
- Click → jumps cursor to that seq (replay mode), or selects event (live)

In live mode: auto-scrolls to follow new events. Pause auto-scroll if user manually scrolls.

In replay mode: events past cursor are dimmed.

Search/filter at top: "Filter events..." text input.

## Graph (canvas)

Read-only rendering of the current pipeline graph. Same node types and styling as editor.

State indicators per node:
- **Pending** (dashed outline, gray fill): not yet entered
- **Active** (solid amber border, pulse animation): currently executing
- **Completed** (teal border, ✓ in corner): finished with success outcome
- **Failed** (rose border, ✗ in corner): finished with failure outcome
- **Cursor target** (in replay): violet outline indicating where scrubber is

Click a node → focuses Detail panel on it. Double-click → opens in editor (new window).

For Loop nodes: shows iteration progress (e.g., `loop 3/7`).

## Scrubber (replay mode)

Horizontal slider below graph. Shows:
- Full timeline from seq 1 to max
- Event marks at significant transitions (StageEntered, ApprovalRequested, ArtifactProduced, etc.)
- Cursor (current scrub position) — draggable
- Speed controls: `0.5×`, `1×`, `2×`, `4×`
- Play/pause button

Behavior:
- **Drag cursor**: instantly updates state at scrubbed position
- **Click event mark**: jump to that seq
- **Play**: animates cursor forward at chosen speed, updating UI
- **Keyboard**: ←/→ step events, Shift+←/→ step stages, Home/End jump to start/end

## Bottom panel

Tabbed:

### Events tab

Detailed log of events near cursor (live: latest, replay: around cursor). Each event expanded with:
- Full timestamp
- Event kind
- All fields from payload
- Pretty-printed args/results

### Diff tab

In live mode: shows latest committed diff for the active node.

In replay mode: shows diff at cursor position. Two sub-modes:
- **Single-run diff**: compare current state to start of run (cumulative changes)
- **Compare to other run**: select another run from dropdown, side-by-side comparison

Diff format:
- Per-file unified diffs with syntax highlighting (using gpui text rendering)
- Hunk headers
- Toggle: unified ↔ split view

### Artifacts tab

List of all artifacts produced up to cursor:
- Group by stage
- Click → preview in panel
- Markdown rendered, code highlighted
- "Open externally" button (opens in default app)
- "Copy path" / "Copy contents" buttons

### Cost tab

- Stacked bar chart per stage (prompt tokens, output tokens, cache hits)
- Cumulative cost line
- Per-stage breakdown table
- Live mode: estimated remaining cost

Custom rendering using gpui primitives (no Chart.js or similar — minimal deps).

## Detail panel (right)

Context-sensitive based on selection:

### When live and a node is active

- Active stage card:
  - Profile name + version
  - Elapsed time
  - Last tool call (live updating)
  - Tokens used
  - Streaming agent message (if currently typing)

### When replay and node selected

- Stage execution card:
  - Profile, attempt number
  - Started/ended timestamps
  - Outcome
  - Cost breakdown
  - Artifacts produced
  - Tools called (expandable list)

### Cost panel

- Total run cost
- Per-stage breakdown table

### Artifacts panel (mini)

- Quick list of recently produced artifacts
- Click → opens in bottom panel Artifacts tab

### Fork CTA (replay only)

Prominent purple button: **"Fork from here"**

Clicking opens the fork dialog (next section).

## Fork dialog

Modal dialog when "Fork from here" clicked:

```
┌─ Fork run #0083 from seq 412 ────────────────────────────────────┐
│                                                                   │
│ This will create a new run starting from the current cursor      │
│ position. The new run inherits all events up to seq 412 and      │
│ will begin executing from there.                                  │
│                                                                   │
│ The next node to execute will be: implementer (stage 4)          │
│                                                                   │
│ ▼ Optional adjustments                                            │
│                                                                   │
│ □ Edit prompt for next node                                       │
│ □ Change profile of next node                                     │
│ □ Skip next node entirely                                         │
│                                                                   │
│ Branch name: [vibe/run-fork-abc123____________]                   │
│                                                                   │
│                              [Cancel]  [Create fork]              │
└──────────────────────────────────────────────────────────────────┘
```

If user enables "Edit prompt": expanded section with prompt editor.

Confirm → creates new run via Engine API → opens new window with the new run in live mode.

## Live update mechanism

The runtime UI subscribes to event log changes:

```rust
struct LiveSubscriber {
    storage: Arc<Storage>,
    run_id: RunId,
    last_seen_seq: u64,
}

impl LiveSubscriber {
    async fn poll(&mut self) -> Vec<Event> {
        let new_events = self.storage.read_events(
            &self.run_id,
            (self.last_seen_seq + 1)..u64::MAX
        ).await?;
        if let Some(last) = new_events.last() {
            self.last_seen_seq = last.seq;
        }
        new_events
    }
}
```

Polling interval: 100ms. UI re-renders only changed components (gpui's reactive model handles this efficiently).

## State snapshotting in replay

For smooth scrubbing on long runs, periodic state snapshots:

```rust
// Engine writes snapshots every N events (configurable, default 50)
async fn maybe_write_snapshot(&self) -> Result<()> {
    let current_seq = self.current_seq().await?;
    if current_seq % 50 == 0 {
        let state = fold_to_state(...).await?;
        self.storage.write_graph_snapshot(self.run_id, current_seq, state).await?;
    }
    Ok(())
}
```

When scrubber jumps to seq N:
1. Find latest snapshot at seq ≤ N
2. Load snapshot
3. Replay events (snapshot_seq + 1)..=N to refine state
4. Render

This makes any seq position renderable in <100ms even for runs with 10K+ events.

## Multiple runs

Sidebar (collapsible) lists active and recent runs:

```
ACTIVE
  ⚡ #0083 sample-app · Building JSON5 parser (4/7)
  ⚡ #0085 myproject · Refactoring lexer (2/5)

RECENT
  ✓ #0082 sample-app · Add tests (12 min ago)
  ✗ #0081 myproject · OAuth attempt (1 hr ago)
  ✓ #0080 sample-app · Update deps (yesterday)
  ...
```

Click → switches main view to that run. Per-run state (scrubber position, selected tabs) preserved.

## Theming

Same color palette as editor (control-room dark theme). Color tokens defined in `crates/runtime-ui/src/theme.rs`.

## Window management

- Single-window default
- Settings toggle: "Open in new window" for opening another run while keeping current

## Persistence

UI state in `~/.vibe/ui-state.toml`:

```toml
[runtime]
last_run = "0083"
window_size = [1400, 900]
window_position = [200, 100]
sidebar_visible = true
sidebar_width = 280
detail_panel_width = 360
last_bottom_tab = "events"
events_panel_width = 240

[runtime.per_run."0083"]
scrubber_position = 412
selected_node = "impl_2"
zoom = 1.0
pan = [0.0, 0.0]
```

## Performance

Targets:
- Cold start: <1 second
- Idle CPU usage: <2% (when no events arriving)
- Memory: <200MB
- Live update latency: <200ms from event write to UI update
- Scrubbing on 10K-event run: <100ms per seq jump
- 60fps interactions

## Implementation phases

### v0.1

- Live mode only
- Events list, graph rendering, basic detail panel
- No replay, no scrubber, no fork

### v0.2

- Replay mode
- Scrubber
- Diff tab in bottom panel
- Cost tab

### v0.3

- Fork-from-here
- Compare two runs side-by-side
- Multiple-run sidebar

### v1.0

- Everything per this spec
- Polish, accessibility audit

## Acceptance criteria

The runtime UI is correctly implemented when:

1. Live mode shows events arriving in real-time with <200ms latency.
2. Graph visualization correctly reflects per-node execution state.
3. Replay mode scrubber smoothly transitions between any two seq positions.
4. State snapshots correctly accelerate scrubbing on long runs.
5. Diff viewer renders correctly for runs with up to 100 modified files.
6. Cost chart accurately shows per-stage breakdown.
7. Fork-from-here creates a valid new run that resumes execution.
8. Switching between runs in sidebar preserves per-run state.
9. Cold-start time <1 second.
10. Cross-platform: identical behavior on Linux, macOS, Windows.
