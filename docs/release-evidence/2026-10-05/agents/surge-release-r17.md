# R17 UX source review — 2026-10-05

Scope: primary CLI onboarding and error paths; read-only inspection. No build, executable CLI interaction, fresh native capture, provider run or desktop usability audit performed. Existing design-qa.md and docs/ui-automation-evaluation.md were read as historical records, not current-run audit evidence. No repository changes.

## Findings

1. P1 README clean-build onboarding fails before initialization. README.md:69 builds workspace; :72 changes to your-project then invokes bare surge, but building does not install surge on PATH. :76 subsequently assumes repository Cargo.toml and examples relative to that new directory. A user following the snippet in a clean shell receives command-not-found, or cannot locate Cargo.toml/example. Fix: explicitly install CLI/daemon or use retained absolute binary/repository paths; keep terminal-flow example in the Surge checkout. Verified by reading the literal sequence and Cargo build/install distinction; clean-shell execution unverified.

2. P2 invalid sandbox choice silently becomes workspace-write. crates/surge-cli/src/commands/init.rs:283-288 accepts only exact choices for read-only/network/full-access and routes everything else to workspace-write. Typing `read-onyl` when intending read-only silently chooses a writable policy. Agent choice :264-266 similarly substitutes first agent for nonnumeric/out-of-range values. Worktree choice :300-308 silently selects sibling for invalid input. Fix: retain blank-as-default but reject/re-prompt nonblank unknown selections with accepted values. Source control flow verified; no runtime wizard test executed.

3. P2 MCP path quoting cannot be represented by wizard. init.rs:347-355 splits free-form argument text with split_whitespace. Input `@modelcontextprotocol/server-filesystem "/Users/Alice/My Project"` produces three arguments after executable rather than two, retaining quote characters and splitting the directory. This yields confusing downstream MCP launch/root errors. Fix: document raw per-argument input or support a defined quoted argument syntax and validation. Source semantics verified; actual subprocess launch unverified.

## Historical desktop evidence and limits

- design-qa.md records successful native discussion submission, preserved navigation/editor drafts, narrow-window scrolling and long-title fixes. It also explicitly retains intermittent parallel Tokio shutdown flakiness. Those records do not verify today's source or binaries.
- docs/ui-automation-evaluation.md records an initial inaccessible GPUI preview and later native migration/component interactions, and states that interactive graph editing/adding specialists remain unimplemented. Those are scope/acceptance questions for release owner, not new reproduced defects.
- Current native screenshots, keyboard/focus traversal, error recovery, task creation and provider-backed execution remain unverified by this role. Product Design audit instructions were read; no screenshot-backed audit was claimed because this role stayed read-only and did not access a native flow.

Recommended acceptance: README steps work in a fresh project without a preinstalled binary; invalid wizard values cannot change intended policy silently; an MCP root containing spaces survives initialization as one argument. Coordinate fixes with CLI/docs owners.
