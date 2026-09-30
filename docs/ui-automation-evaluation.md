# Native UI automation evaluation

Observed on macOS, 2026-09-28. The user requested evaluating a GPUI replacement,
including egui, if Surge cannot be operated and tested through Computer Use.
This is a feasibility result, not a migration decision or a usability benchmark.

## Environment and control

System Settings showed **Codex Computer Use** enabled for Device Control and Data
Access and Screen & System Audio Recording. Reading its accessibility tree,
clicking navigation, raising its window and capturing a screenshot worked.
Missing macOS permissions are not the established cause of the Surge failure.

The tested Surge preview used the current debug binary in a temporary `.app`
bundle. Selecting it with `cua.getApp` repeatedly returned `timeoutReached`.
Replacing its shell launcher with a direct binary bundle did not resolve this.
The process remained alive; a sample showed the main thread in the native event
loop. Neither that sample nor successful GPUI tests proves a usable visible app.

The original preview copy preceded the final form repair. It was subsequently
replaced with `target/debug/surge-ui` built at 2026-09-28 00:11:17 local time;
both files had SHA-256
`6fbec3492fea3c348fb2f10e093bc8571e56aa096cf1fb736eb7588385ee8eae`.
The updated direct-binary bundle also returned `timeoutReached`. The limitation
therefore remained after refreshing the preview, rather than being inferred from
an outdated binary alone.

## Results

| Case | Surge / GPUI 0.2.2 | eframe 0.35.0 prototype | GPUI Kit 0.7.0 prototype |
|---|---|---|---|
| Native accessibility tree | Selection timed out | Named input, button and status exposed | Named input and button with stable IDs; status and custom navigation exposed |
| Exact spaced multiline input | Not reached | Paste/submit exact echo passed | Paste/submit exact echo passed |
| Unicode multiline input | Not reached | Russian text exact echo passed | Russian text exact echo passed |
| Selection and deletion | Not reached | Select-all/Backspace passed | Select-all/Backspace passed |
| Blank submission | Not reached | Validation error observed | Validation error observed |
| `typeText` | Not reached | Entered only a space in focused-input test | Entered only a space in focused-input test |
| `setValue` | Not reached | No text change observed | Exact AX value update passed |
| Custom navigation control | Not reached | Not tested | Named button click incremented visible counter |
| Framework tests | 46 UI + 5 integration tests passed | Two pure tests passed after RED | Three tests, strict Clippy and native build passed |

The first prototype click/type/submit attempt produced the blank-input error.
After explicit focus, clipboard paste succeeded. Do not omit this limitation or
attribute it to egui, keyboard layout or Computer Use without further isolation.
An accessible tree alone is not an end-to-end interaction test.

## Reproduction artifact

Temporary source and command log: `/tmp/surge-egui-cua.BJgmiv/README.md`.
Bundle: `/tmp/surge-egui-cua.BJgmiv/SurgeEguiSpike.app`.
Bundle ID: `dev.surge.egui.cua.BJgmiv`.

The prototype pins eframe `=0.35.0`, disables default features, and enables
`accesskit`, `default_fonts`, and `glow`. Development builds disable debug info
and incremental compilation. It uses a separate target directory and changes
no Surge dependencies. No inspection server was enabled. Build, strict Clippy,
formatting and local bundle signature verification passed. The executable is
directly bundled rather than launched through a shell wrapper.

Version 0.35.0 was selected from an examined release, not as a claim that it is
the latest version. [eframe features](https://docs.rs/crate/eframe/0.35.0/features),
[egui release](https://github.com/emilk/egui/releases/tag/0.35.0).

## Decision boundary

The locked GPUI 0.2.2 source contains no matches for `accessibility`, `accesskit`
or `NSAccessibility`. This does not describe newer GPUI. Upstream documents
semantic roles, labels, values and actions, and an improvement for stable native
identifiers. [GPUI issue 61925](https://github.com/zed-industries/zed/issues/61925).

egui has now demonstrated a usable native Computer Use path. Before choosing a
product migration over a GPUI upgrade, verify project navigation, async status,
approval forms, long event lists, markdown/diff presentation, keyboard navigation
and persisted reconnect behavior. Preserve the daemon/domain boundaries and
existing product acceptance criteria. A toy submit form does not prove those
capabilities, replace a full app journey, or establish lower migration cost.

## Upgrade candidate and measured scope

Published manifests provide another candidate: `gpui-kit = "=0.7.0"`, using
component/base/assets 0.7.0 and exact `gpui-pre`/platform 0.3.7. This is a package
family change; the package named `gpui` remains 0.2.2. The macOS backend declares
AccessKit dependencies. Native interaction subsequently passed the cases above.
[Kit manifest](https://docs.rs/crate/gpui-kit/0.7.0/source/Cargo.toml),
[macOS backend](https://docs.rs/crate/gpui-pre-macos/0.3.7/source/Cargo.toml).

The current UI contains 33 Rust files and 18,740 physical lines including tests.
Static counts include 19 Render implementations, 7 input construction sites,
64 click handlers, 21 subscriptions and 11 spawned tasks. These count source
sites, not unique rendered controls or lines necessarily requiring a rewrite.

An upgrade retains the Entity/Context/Render structure but changes startup, Root
hosting, multiline input APIs and tests. Custom controls still need explicit
roles, labels and stable IDs. An egui migration additionally replaces that
retained UI structure and its async bridge across the screen set. Neither path
may discard existing product actions to simplify its acceptance tests.

The standalone Kit prototype is at
`/tmp/surge-gpuikit-cua.LHEirv/SurgeGpuiKitSpike.app`, bundle ID
`dev.surge.gpuikit.cua.LHEirv`. Its direct native binary built successfully in
5m15s. The same Computer Use driver operated its actual native controls; a copied
test result or source assertion was not substituted for interaction.

Current direction: prefer testing an upgrade over a full egui rewrite because
native automation now works while the existing UI structure can be retained.
Next verify the actual planning form, then all production screens and async tests
under the new framework. The candidate is a fresh release; the full Surge
upgrade, project MSRV and cross-platform compatibility remain unverified. Copied
prototype response handlers must not be counted as production async coverage.

The actual `SpecWizardScreen` was then copied into a separate native harness,
with framework/import/textarea/AX adapters and a thin controlled-response host.
At `/tmp/surge-gpui-form.z4GpJ7/SurgeActualForm.app`, Computer Use preserved an
exact spaced Russian multiline request, observed one submission despite a second
click while pending, saw rejection with retained draft, retried/accepted, then
opened the matching run. The harness explicitly states that no daemon is
connected. These observations establish compatibility of the real form logic;
production application routing, recovery and daemon execution still require
their own tests after the upgrade.

## Actual product preview: 2026-09-28

The upgraded product executable, not the copied-form host, opened successfully
through Computer Use at
`/var/folders/5h/kmnftr3s6r1d30h8rtdll_2m0000gn/T/surge-ui-native-upgrade._bk2tmb7/SurgeUpgradePreview.app`.
This preview came from the all-target test build, which unifies the dev dependency's
test-support feature. Its SHA-256 is
`5ee02e57ad44b2276e6aa482353376834b1f244035ae56bfdfd526d2ee1cbf51`.
Default-feature build, strict UI clippy, Rust 1.96 and final native acceptance are
still required. The 54 automated UI checks passed before this native pass.

The app used an isolated SURGE_HOME, one temporary Git project and no daemon.
Observed native behavior:

- Open the recent test project through its actual welcome-screen button.
- Open planning with Ctrl+N; paste an exact spaced, multiline Russian request.
- Submit and receive the real inline daemon-offline error without losing input.
- Back to Backlog, then reopen planning through its button: the draft and error
  remain intact.
- Open the command palette with Ctrl+K, filter to Inbox, then activate that result:
  the app navigates to Inbox.

Two native failures require repairs: Fleet exposed an accessibility label
"Plan a task" for a button that actually opens Inbox; the planning Textarea
rendered at roughly one-row height and clipped the second line despite retaining
the correct full value. These are observed failures, not inferred test failures.
Repaired-source/native verification remains pending. The visible sample runs and
sample approvals were not counted as real execution or tested approval handling.
This proves product-level native access is viable; it does not prove the full
prompt-to-application journey or daemon recovery.

### Default-feature repair verification

The rebuilt, ad-hoc-signed default-feature executable in the same isolated bundle
has SHA-256 `592deb36ebe3dce0f0570d5ba78fdd432fd0448c41ee23b08433758e5d94e7ee`.
All 55 automated UI tests passed before this native retest. Computer Use verified
that Fleet now displays and exposes **Open Inbox**, and clicking it opens Inbox.
Ctrl+N opens the production planning form. A seven-line Russian request, including
leading and trailing spaces, remained exact in the accessibility value and all
seven lines were visible in the approximately 200-pixel-high textarea. Submitting
with the daemon offline displayed the actual inline error and retained the text.
Returning to Backlog and reopening planning through its button restored the exact
seven-line draft and its offline error.
Both previously observed native failures are therefore repaired in this build.
Strict clippy, Rust 1.96 and cross-platform verification remain separate gates;
this offline pass does not establish real agent execution or recovery.

## September 29 product interface pass

The isolated `codex/product-readiness` checkout builds a default-feature native
bundle at `.codex/builds/Surge.app`. Its final executable SHA-256 is
`d86b5d672b980b731a8a8faedf532f63f1b10907dfe07cc0f75c3e048987c9c0`.
Ad-hoc bundle signature verification passed. All 194 UI tests, workspace
formatting, diff whitespace and strict UI Clippy checks passed on Rust 1.98.1.
This pass did not recheck the declared Rust 1.96 MSRV or other platforms.

Computer Use inspected actual retained project runs, rather than sample cards:

- Tasks exposes all four retained runs, filters and the selected request.
- New task accepts exact padded multiline Russian text; Back returns to Tasks.
  This native input check did not submit a new provider execution.
- Workflows opens the actual project → milestone → task template, retaining
  parent breadcrumbs. Four-level and reused-body navigation additionally passed
  GPUI interaction tests with fixed fixtures.
- Profiles exposes role defaults, expected inputs, outcomes and the full prompt
  template through scrolling. Light and dark task surfaces were inspected.
- A 960 × 640 window was inspected during the visual pass. This found shrinking
  detail headings; the final fixed-height heading and accessible result action
  were subsequently checked in the final 1280 × 800 bundle.
- Cmd+Q and closing the last window both terminate the preview process, and
  reopening the same bundle restores a usable welcome screen. The daemon is
  separate and was not stopped.

Visual review drove bounded pane scrolling, stable header height, earlier result
actions, readable diagram text and profile-panel padding. The template inspector
does not claim an effective live session from an unrelated run's node name.
Live instance-specific graph drilldown, process editing and cross-provider
completion remain separate acceptance work.

### Follow-up: connected recorded task processes

The earlier component pass did not establish product readiness or fidelity to the design reference. A native-window follow-up found that expanded roadmap details extended below the viewport without scrolling. Aligning the scroll container content to the start restored scrolling; the process action now precedes long acceptance criteria.

Roadmap plan items now link to their owning implementation run and milestone-qualified task. The recorded-process explorer provides mission/milestone/task/subtask navigation and displays actual step states and agent reports. Selecting another run resets the task scope; external run links reset the previous tab. Missing execution is stated explicitly.

Native verification on the retained timer mission confirmed the roadmap task link, parent navigation, specification/build/check reports, and scrolling to the final report. This verified navigation of recorded work, not a fresh provider execution. Editing a mission process, adding a profile during execution, and live graph editing remain unimplemented product requirements.

### Follow-up: first-open static preview

The primary checkout's native bundle reproduced a persistent black preview of
the completed timer mission. Reload displayed the same application's HTML,
styles and JavaScript. Switching away and reopening reproduced the failure.

The pinned Wry builder started navigation before attaching the native child;
the GPUI wrapper then reset its bounds to zero until prepaint. The preview now
creates the child without a URL and starts navigation on the next frame only
after observing a nonzero layout. Closure cancels pending navigation; origin,
asset and navigation restrictions are retained.

The rebuilt primary-source bundle displayed the real timer without Reload on
first opening and after switching to Changes and back. Clicking Start produced
a countdown from 25:00 to 24:53, confirming script execution. The Changes pane
replaced the native browser correctly. Initial asynchronous loading can still
briefly show an empty surface; a zero-area layout reports an explicit reopen
error. These observations verify the retained static result on macOS, not a new
agent execution or backend application startup.

### Follow-up: recorded participants and operator guidance

Task-process cards show the profile and runtime from `SessionOpened`. A scoped
Work details disclosure exposes the recorded session, resolved binding names
and bound skill names. These are event projections, not current profile
defaults. Retry entry clears the preceding attempt's evidence; repeated task
and subtask node names stay isolated. Legacy events lacking this data are
labelled as not recorded. GPUI interaction regressions exercise disclosure and
scope changes, including human gates and missing runtime information.

The orchestrator now persists `StageInputsResolved` before opening an agent
session, hashing the complete resolved values independently of the capped ACP
echo. Duplicate targets are rejected before launch. Actual stage-launch
integration regressions cover large/static/file inputs, changed file content,
empty inputs and failed resolution/validation/rendering. The local gate passed
398 orchestrator unit tests plus eight integration-harness tests (four exercise
the new stage behaviour), and strict all-target/all-feature Clippy. The running
daemon was not replaced during this pass; this is production-source and mock
bridge evidence, not new context records from a live provider run.

Send guidance keeps drafts until acknowledgement, guards duplicate pending
sends, preserves edits made while waiting, and scopes drafts and responses by
project and run. Delayed responses injected at the production submission and
settlement boundary exercise rejection, retry and late acknowledgements. This
does not prove exactly-once delivery after a lost RPC acknowledgement; the
failure message says that queueing could not be confirmed. Guidance is queued
for a subsequent agent step and does not change the mission graph or add a role.

### Follow-up: exact plan ownership and final local verification

Opening a plan from Decisions carries its run and pending decision sequence.
An unavailable or expired requested decision produces an explicit message rather
than selecting another mission. Plan edits and model input events retain their
owner; detached editors and delayed stored-plan loads cannot modify or replace
the newly selected plan. Content fingerprints refresh a changed flow at the
same path without reparsing unchanged content every frame. GPUI regressions
reproduce the wrong-mission selection, stale same-path content and reordered
stored-plan responses. No live pending provider decision was created for this
verification.

The final local checks passed 208 UI unit tests and five UI gate integration
tests, plus strict UI Clippy across all targets and features. Formatting and
diff checks passed. The rebuilt macOS bundle visibly showed Spec, Build and
Check participants for a retained task; Work details exposed its recorded
session and honestly marked absent legacy inputs and skills. Its static timer
preview loaded without Reload after the initial asynchronous empty surface.
Interactive graph editing and adding specialists remain unimplemented.

### Product correction: Rust-first results

The user clarified that browser preview is unnecessary for their primarily Rust
projects. Preview was removed from result tabs and completed-task actions, and
the native webview lifecycle was disconnected from the results screen. Results
now expose Overview, Changes, Checks and Log. The preview observations above
remain historical evidence, not a current product requirement.
