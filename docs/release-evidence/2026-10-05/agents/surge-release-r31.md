# R31 technical writer

Implemented README source build/installation paths (paired CLI+daemon, no bare PATH assumption), archive-user onboarding commands and inline terminal-only graph independent of checkout, optional desktop archive scope, Python>=3.11 packaging prerequisite, strict locked check examples and deterministic ignored integration allowlist reference.

Corrected config/graph schema1, payload15 and memory3 directly against current constants; described payload12–15 and memory3 FTS repair. Removed unsafe runtime-delete advice and binary-only downgrade implications. New docs/release-procedure.md contains exact-revision/native validation requirements and R30 complete quiescent snapshot/restore-only procedure; reader-only legacy projection access explicitly does not migrate. Draft release notes retain draft status/no publication date and disclose Windows durable Task Start/MCP fail-closed limitations and allowed transitive dependency audit warnings. docs index links added.

Validation actually performed: git diff --check exit0; Python3.12 local Markdown target checker over seven owned docs returned missing=[]; Python3.12 tomllib inline installed-user terminal graph parse PASS and byte content equal to source terminal graph. Initial system Python3.9 lacked tomllib; rerun correct3.12 passed. No Rust builds, provider tests, native archive workflow runs or production backup/restore performed by this role. No commits.

R25 tracker documentation pending actual accepted implementation confirmation; accepted design alone is not described as implemented. Coordinator notified AGENTS map needs new release-procedure document entry by its owner.

## Final read-only review

Reviewed docs/release-readiness-2026-10-05.md, docs/release-procedure.md, draft release notes, release-evidence index and tracker documentation. No source/doc edits and no Cargo executed in this review.

NEEDS WORK before evidence freeze:
1. Evidence index final receipts still says 36 skipped; frozen coordinator outcome and readiness say 37 skipped (3830 pass+10 fail=3840 executed). Correct index when appending final candidate receipts; current index is explicitly unfinished, so not a completed evidence inventory.
2. Draft highlights still claim 'The daemon survives an unclean exit and resumes in-flight runs' and announcement 'Crash the daemon? It resumes from the log on restart.' Both generalize beyond documented MCP cold-recovery NO-GO. Qualify those exact sentences with supported recovery boundaries or remove unconditional promise. Appended blocker does not make promotional blanket claim correct.
3. Draft graph-engine highlight 'Every NodeKind ... executes end-to-end' includes Loop without qualification while readiness explicitly excludes Loop Replan/complex nested-loop placeholders. Restrict to supported modes or link explicit limitation.

No unsafe backup/reset instruction found in reviewed procedure: settles all writers, complete runtime+project+Git/private/worktree snapshot, permission preserving, read-only integrity validation, quarantine rather than overlay, restore old binary/config, external-effect reconciliation. Starting old daemon without intake is an operational prerequisite, not an implemented new CLI switch; procedure does not invent such a switch.

Readiness main NO-GO, candidate source3136b82, four-platform/native pending, failed MCP denominator and synthetic restore evidence are described honestly. Native source build remains pending; index must append actual identity/receipts only after completion. Windows MCP diagnostic disk-capture non-Unix limitation is not currently listed in notes; optional addition useful, not broader workflow-parity guarantee. Telegram /run correctly deferred now. Tracker docs now describe attempted/uncertain durable records and best-effort notifications rather than guaranteed external delivery.

## Corrected claims re-review

ACCEPTABLE for documentation scope. Read-only re-read confirms all three requested corrections: evidence index37 skips; graph supports types with explicit incomplete/unverified Replan/nested recovery; supported event-sourced recovery/Attention limitation replaces unconditional resume promise in highlight and announcement. Draft now prominently NO-GO, configured targets do not imply built archives, manual branch artifacts gated by validation, native candidate procedure explicit. Complete quiescent snapshot/quarantine/restore safety retained. Final native identity/table still pending and no readiness inferred. No source edits or Cargo.

## Final onboarding/CLI paragraph review

ACCEPTABLE: changed getting-started paragraphs explicitly require committed clean Git source including untracked files, review/commit project context and inline flow, ignore local config/runtime without committing secrets. Stop acknowledges request and wait/restart requirement correctly prevents immediate restart assumptions.

CLI revised paragraphs match engine.rs190–330: durable owner submission for both paths; only daemon&&!watch returns admission; otherwise startup plus watcher; watcher has no local approval handler, waits terminal/error, and interrupts observation without abort RPC. Disk terminal evidence and suspension distinction correctly stated. Read extracted archive green2 receipt; actual PASS with clean committed project removes former onboarding runtime evidence gap. Final native/platform and third-party bundle inventories still separate pending obligations. No docs/source edits or Cargo.
