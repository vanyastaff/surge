# R01 Release analyst

Read-only repository review; no implementation edits, no build/test success claims. Read requested pasted brief, README, release notes, development docs, competitive manifest, release/CI workflows, schema/crash-recovery/recorded E2E docs, Cargo and justfile. `git status --short` returned empty at observation.

## Existing nearest release

Workspace version 0.1.0; Rust edition 2024/MSRV 1.96. Local-first Rust daemon + CLI drive flow.toml graphs with ACP runtimes, SQLite event sourcing, Git worktrees, human gates and notifications. The configured distributable is two sibling executables (`surge`, `surge-daemon`), README and MIT/Apache licenses. Four native archive targets: Linux GNU x86_64, macOS Intel, macOS arm64, Windows MSVC x86_64. GPUI desktop exists but is not in release archive. No web/browser release required; crates.io/Homebrew/Scoop pending. Tag must match workspace version; manual branch workflow makes artifacts without publication. Publishing needs separate human authorization.

## Observable acceptance already grounded in repository

- Final locked release build of CLI and daemon; both extracted --version succeed; package contract tests and four-target complete archive/checksum collection.
- Format, strict workspace clippy (accepted manifest A14 and user release request), workspace nextest and doctests. Native surge-ui tests are separate from CLI archive delivery; avoid extrapolating controlled render harnesses to native accessibility or live daemon.
- MSRV 1.96 check; CI presently excludes UI while development docs explicitly list UI MSRV check.
- Mock ACP ignored integrations, terminal-only flow smoke, daemon restart/recovery, main human-gate/bootstrap scenario, and relevant budget/ownership/recovery tests. Optional authenticated real-runtime smoke must be marked unverified if credentials unavailable; historical Claude recording is evidence for one runtime/project, not today's tree.
- Dependency audit/deny and performance stage-transition P95 gate are existing checks. Existing ignored real MCP smoke is optional and intentionally external.

## Findings and priorities

P1: Schema release documentation is materially wrong for rollback/compatibility. `docs/release-notes-v0.1.md` says event schema frozen at 1; `docs/schema-versioning.md` table says 11 and its opening says first three frozen at 1; actual `crates/surge-core/src/migrations/mod.rs:100` is MAX_SUPPORTED_VERSION=15. Config/flow=1 and memory=2 verified by source. Update writer-owned docs with actual versions and honest compatibility/rollback boundaries, not downgrade promises.

P1: Release workflow depends only on metadata/build/package collect, not CI. A tag from an untested commit can publish if archive --version succeeds. CI triggers only main/develop push/PR. Require a checked exact release commit before human-authorized tag/publication; CI agent should make it enforceable without weakening gates.

P1: CI deliberately permissive workspace clippy contradicts accepted manifest A14 strict workspace gate. `ci.yml:173-177`, justfile clippy/lint/ci preserve historical debt exception. `ci.yml:115-117` ignored mock integration suite is continue-on-error=true. Green CI alone does not demonstrate user-requested full release readiness.

P1 scope honesty: Accepted competitive requirements still open: R21 memory audit acceptance, R22-R23 aging/write-back acceptance; R34-R36 capacity model/current sources; R37 estimator; R38 ordinary Flow/ACP accounting dependencies; R41 full wake+budget E2E; U01 multi-provider planning implementation/acceptance. Manifest rule permits only user to retire requirements. Preserve as open; do not conflate narrower configured task-owned acceptance with full capacity/account rotation claim. Wave 5 proposed opportunities, explicit stretch R32, hosted/mobile, semantic conflict resolution and distribution channels are documented future work, not invented release blockers. Product owner decides whether these accepted open items belong to nearest release; code correctness of shipped paths remains mandatory.

P2: Release notes explicitly draft, date not finalized, and native four-target release run not verified. Cannot call multi-platform ready from one macOS build. Remote/native evidence unavailable is exact external blocker, while local checks continue.

P2: Real recorded E2E limited to Claude same-runtime verification; source documents this correctly. Maintain limitations, avoid expanding claim to all listed runtimes.

## Evidence commands

Executed cat/sed reads of documents and workflows; rg release/gate references; rg schema constants; `git status --short`. One initial rg on migrations.rs returned exit 2 because migration source is a directory; follow-up rg on migrations/ successfully located version 15. No tests/builds executed in this role to avoid duplicating assigned owners and claiming absent evidence.

## Suggested handoff

CI owner: exact-commit release gates and strict lint/ignored deterministic integration enforcement. Writer owner: schema notes, status/limitations and date readiness. Migration/rollback owner: schema-15 upgrade/read-old/reject-new evidence and stop/backup/restore procedure without deleting user runtime data. Independent reviewer: final same-tree logs, command exit codes, skipped denominators, four-native archive evidence and explicit GO/NO-GO.
