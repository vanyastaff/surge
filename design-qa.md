# Desktop conversation design QA

Source visual truth: `/Users/vanyastafford/.codex/generated_images/01a1060d-619b-7bf3-8e38-4a21345e5fb0/exec-ffb41305-a36e-4f10-b3c4-154d24787d8c.png` (selected variant 3).

Implementation: native Rust GPUI desktop app, isolated `SurgeDesignPreview.app`, actual daemon-backed task in `/tmp/surge-design-1004/project`.

Viewport: 1440 × 1024 native window points. Source pixels: 1487 × 1058. Implementation screenshot pixels: 1440 × 1024, capture density 1 output pixel per point. CSS viewport/deviceScaleFactor do not apply to native GPUI; CUA returns the logical window capture. Source resized proportionally to 1440 × 1024 for comparison; macOS title-bar differences excluded from fidelity findings.

State: dark monochrome theme, selected durable task, accepted requirements and a recorded operator proposal. The mock depicts an agent review with successful verification; the fixture has no executed attempt. Actual task state, actor, proposal, controls and recorded usage replace mock content. No verification success or agent message is invented.

## Comparison history

First full-view evidence: `.codex/design-review/comparison-first.png`, combining source and `.codex/design-review/implementation-first.png` in one image.

- [P1] Task title clipped by scroll-container alignment. Fix: top-align the conversation and prevent message/title shrinking.
- [P2] Conversation canvas used the brighter hover token. Fix: use the neutral background token.
- [P2] Message icons absent from embedded assets. Fix: embed the existing Lucide MessageCircle resource and check availability.
- [P2] Empty history headings and expanded technical metadata increased density. Fix: omit empty sections, number acceptance criteria and put workspace metadata behind disclosure.
- [P2] Nested rectangular composer border drifted from rounded source. Fix: retain rounded composer shell and remove the inner input border.

Second full-view evidence: `.codex/design-review/comparison-second.png`, combining the source with `.codex/design-review/implementation-second.png` from the default-feature native build. Focused evidence: `comparison-thread-second.png`, `comparison-inspector-second.png`, `comparison-composer-second.png` in the same directory.

The title, top alignment, canvas token, message resources and inspector density fixes are visibly confirmed. Two remaining P2 differences were found: the composer was substantially taller than the source and prose appeared brighter. Applied fix: 48-point two-row textarea with adjacent Send in a 24-point rounded shell; muted prose while retaining primary title/actor text.

Third full-view evidence: `.codex/design-review/comparison-final.png`, combining source and `.codex/design-review/implementation-final.png` at the same size. Focused comparisons: `comparison-thread-final.png`, `comparison-inspector-final.png`, `comparison-composer-final.png`. All earlier source-comparison P1/P2 fixes are visibly confirmed. Remaining differences are expected native/backend adaptations: actual operator/revision labels, real task controls, extra project/branch/search bar, existing Lucide/system-font resources and a two-row composer with a labeled Send button. Unsupported attachments/voice and fictional success controls from the mock are omitted.

Additional real-content check: `.codex/design-review/actual-project-dark.png` revealed a P2 density issue with legacy runs whose full prompt is their title. Fix: clamp the central legacy title to two lines and inspector title to three, retaining the full original request below. Post-fix capture: `.codex/design-review/actual-project-final.png`; source and real-content capture together: `comparison-real-project-final.png`. The title no longer displaces the original request or actions. This completed legacy run intentionally displays actual recorded progress/result controls instead of a discussion composer.

## Interaction evidence

- Native task-list navigation preserves the discussion draft.
- Native workflow editor collapse/reopen preserves its draft.
- Native Workspace details expands and collapses without exposing fabricated usage.
- Native `⌘N` opens task creation; Cancel returns to the task list.
- At 960 × 700 points the inspector stacks below the conversation; both panes scroll and persistent navigation/composer remain accessible. Final compact-composer evidence: `.codex/design-review/implementation-compact.png` and `compact-controls.png`; lower task controls were reached by scrolling the inspector.
- Native Send on the final compact composer produced an actual daemon-confirmed discussion and cleared the input after acknowledgment: `.codex/design-review/native-discussion-accepted.png`.
- Independent code review verified exact task/run routing, filter retention and existing durable operation fences.
- Existing project was reopened in the updated main application; Dark/Monochrome selected through native Appearance settings.

## Verification record

- Meaningful navigation test observed red before the conversation implementation; later passed.
- Parallel Cargo suites intermittently failed `lost_ack_ui_retry_preserves_command_and_one_real_discussion` with a Tokio shutdown panic. The same test passed in isolation; the complete backtrace-enabled unit executable passed 219/219, and integration executable passed 5/5. No ignored tests, suppressed assertions or speculative runtime fix were added.
- `CARGO_INCREMENTAL=0 cargo test -p surge-ui --locked -- --test-threads=1`: passed 219 unit and 5 integration tests after the compact-composer refinement. Serial testing verifies all assertions; it does not establish a fix for intermittent parallel shutdown failures.
- `CARGO_INCREMENTAL=0 cargo build -p surge-ui --locked`: passed after the compact-composer refinement.
- `CARGO_INCREMENTAL=0 cargo clippy -p surge-ui --all-targets --all-features -- -D warnings`: passed after the compact-composer refinement.
- `CARGO_INCREMENTAL=0 cargo +1.96.0 check -p surge-ui --all-targets --locked`: passed.
- `cargo fmt -p surge-ui --check` and `git diff --check`: passed on the latest source.
- `CARGO_INCREMENTAL=0 cargo test -p surge-ui --locked screens::fleet:: -- --test-threads=1`: passed all 16 affected-screen tests after legacy-title polish, including the strengthened long-request navigation/bounds regression and real-daemon draft/retry checks.
- Build, strict Clippy, Rust 1.96 all-target check, formatting and diff checks also passed after legacy-title polish.

## Required fidelity surfaces

- Typography: Helvetica Neue system sans-serif; 28-point title, 16-point normal prose with 26-point line height, primary actors and muted body. Focused final comparison confirms readable hierarchy; exact raster mock font/antialiasing is intentionally adapted to native rendering.
- Spacing/layout: 270-point navigation, 400-point inspector at the target width, bounded conversation and persistent 74-point rounded composer. Final composition and compact scrolling verified.
- Colors: charcoal `#212121` canvas, `#171717` sidebar, neutral `#242424` inspector/composer, muted `#BDBDBD` prose, actual semantic status. Palette text contrast tests pass.
- Image/asset quality: no illustrative raster assets required; retained native icon library, missing message resource fixed.
- Copy/content: product navigation and actual task history preserved; mock review/verification text replaced by factual backend state.

## Implementation checklist

- Source full-view and focused comparison complete.
- Narrow-window and primary native interactions complete.
- Long real-content title and complete request verified after polish.

## Follow-up polish and limits

- [P3] Native multiline clamping does not visibly add an ellipsis to every clipped legacy title. The complete prompt appears immediately below in Original request; no content is discarded.
- Parallel Tokio shutdown flakiness remains unroot-caused. Successful serial and isolated runs establish current assertions passing, not a concurrency fix.
- Only desktop shell/task presentation was changed; other screens retain their established layout and native controls. No new dependencies, daemon/core behavior or remote operations were introduced.

No actionable P0/P1/P2 visual or interaction findings remain. Independent final code review is clean. Main app bundle is updated at `.codex/builds/Surge.app`; temporary isolated preview/daemon stopped after evidence capture. Screenshots under `.codex/` remain local, ignored QA artifacts.

final result: passed
