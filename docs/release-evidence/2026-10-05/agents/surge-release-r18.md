# Role 18 — Accessibility review

Scope: read-only source review of desktop UI. Desktop is outside the CLI archive release scope, so findings below do not block that archive release. No builds, tests or current native UI interaction were performed. Read the user attachment, UI source, and prior native evaluation documentation. Checked Rust Studio help catalog for applicable review discipline.

## Findings

1. **P2: Inbox decision buttons lack keyboard activation and focus.** `crates/surge-ui/src/screens/inbox.rs:1115–1194` constructs primary/secondary/danger actions from custom `div()` nodes with Button roles and click handlers, but no focus handles, tab stops, focus indicator or Enter/Space handlers. These include Send response, approve/reject outcomes and Open cockpit. The screen handler at 1443 supports Up/Down queue movement only. Source evidence strongly indicates that keyboard-only users cannot reach and execute these essential approval actions. Fix by using the shared accessible Button component or the established sidebar focus/Tab/Enter/Space pattern. Acceptance: native Tab/Shift+Tab reaches each enabled action, visibly shows focus, Enter and Space activate once, pending decisions cannot submit again.

2. **P2: Submitted Inbox actions expose an enabled Button role.** The same helpers only change opacity and `aria_description("Decision already submitted")` when blocked; they retain their Button role and cursor styling with no disabled accessibility state. Backend submission guards may prevent duplicate effects, but assistive users receive misleading enabled-action semantics. Acceptance: pending/acknowledged controls expose disabled/unavailable in the native accessibility tree and cannot activate via keyboard or accessibility action.

## Positive evidence and limits

- Sidebar source (`sidebar.rs:184–216` and other rows) explicitly implements Button role, name, tab stop, focus handle/indicator and Enter/Space activation.
- Planning textarea and discussion textarea have explicit accessible names. New task fields have stable accessibility IDs and visible labels/placeholders; runtime accessible names were not inspected.
- Theme test `workbench_palette_keeps_secondary_text_readable` (`theme.rs:607`) checks primary/muted/dim text against five backgrounds at 4.5:1, for Monochrome light/dark. This is existing test source, not a fresh passing result. It does not establish accent-button/semantic status/focus-indicator contrast across every theme or opacity blend.
- `docs/ui-automation-evaluation.md` records prior product-native checks and repairs of the mislabeled Fleet button and clipped planning textarea, tied to historical executable hashes. Those historical checks do not certify today's sources or build.
- Current VoiceOver reading order, focus restoration, full keyboard approval journey, theme contrast and platform parity remain unverified. Do not describe desktop accessibility as release-certified.

## Changes / verification

No repository files changed. Read-only searches and source reads completed successfully; per-crate CLAUDE.md was absent. Report saved to `/tmp/surge-release-r18.md`. Proposed tests are not executed results.
