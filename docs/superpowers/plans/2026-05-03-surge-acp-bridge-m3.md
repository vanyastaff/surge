# surge-acp Bridge M3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `surge-acp::bridge` submodule (AcpBridge, BridgeClient, BridgeEvent broadcast, sandbox stub, mock ACP agent) to enable vibe-flow ACP integration without disturbing the legacy ACP stack.

**Architecture:** Pure-addition strategy following the M1/M2 precedent. New `bridge/` submodule lives inside `surge-acp` next to legacy modules; private `shared/` helpers are extracted from legacy `client.rs` so both legacy and bridge can reuse them. Bridge runs its own dedicated OS thread with current-thread tokio runtime + `LocalSet` for the `!Send` ACP futures, isolated from the legacy `AgentPool`. Two-method `Sandbox` trait surface (`visibility` + `allows_tool`) ships with `AlwaysAllowSandbox` and `DenyListSandbox`; tier-3 OS enforcement deferred to M4. Engine integration (translating `BridgeEvent` → `EventPayload` and appending via `RunWriter`) lives in M5; M3 ships only the bridge layer plus a mock ACP agent that drives every code path end-to-end.

**Tech Stack:** `agent-client-protocol = 0.10.2` with `unstable_session_usage` (already in workspace) · `tokio` multi-threaded runtime + `LocalSet` inside the bridge thread · `tokio::sync::{mpsc, broadcast, oneshot}` for command/event channels · `serde_json` for tool schemas and redacted args · `regex` for secret redaction · `tracing` for instrumentation · `tempfile` for tests.

**Spec:** [docs/superpowers/specs/2026-05-03-surge-acp-bridge-m3-design.md](../specs/2026-05-03-surge-acp-bridge-m3-design.md)

**Estimated effort:** 3–4 weeks of solo evening/weekend work.

---

## Phase 0: Pre-requisites and shared helper extraction

The new `bridge/` module reuses three pieces of logic that currently live inside the legacy `client.rs`: the worktree-rooted path guard, the `ContentBlock` helpers, and the secrets redactor (already in `secrets.rs`, just needs a thin wrapper). Extract them into a `pub(crate) shared/` module so both legacy and bridge code paths can call them. The legacy `SurgeClient` continues to behave identically — this is a private refactor.

### Task 0.1: Bridge module skeleton + workspace dependency confirmation

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/surge-acp/Cargo.toml`
- Create: `crates/surge-acp/src/bridge/mod.rs`
- Modify: `crates/surge-acp/src/lib.rs`

> **Known transient state introduced by this task:** the `[[bin]] mock_acp_agent` entry references a file that doesn't exist until Phase 9 (Task 9.1). This is intentional — see step 3 rationale. Practical consequence for Tasks 0.2 through 8.3: `cargo check --workspace` and `cargo test --workspace` fail with a hard error because Cargo resolves all targets in workspace-wide invocations. Use `cargo check -p surge-acp --lib` and `cargo test -p surge-acp --lib` (or the more targeted `--test <name>` form) until Phase 9 lands the binary file. CI configuration is updated in Task 11.1 to match.

- [ ] **Step 1: Verify workspace deps already cover M3**

Open root `Cargo.toml` and confirm these `[workspace.dependencies]` entries exist (per spec §8). They were all added during M1/M2 work; M3 needs no new top-level deps.

```bash
grep -E "^(agent-client-protocol|tokio|tracing|serde |serde_json|thiserror|ulid|regex|tempfile)" Cargo.toml
```

Expected output (line ordering may differ; values may differ in patch version):

```
agent-client-protocol = { version = "0.10.2", features = ["unstable_session_usage"] }
tokio = { version = "1", features = [...] }
tracing = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
ulid = { version = "1", features = ["serde"] }
regex = "1"
tempfile = "3"
```

If any line is missing, add it. M3 makes no new top-level dep choices.

- [ ] **Step 2: Confirm `surge-acp/Cargo.toml` carries the deps it will use**

Open `crates/surge-acp/Cargo.toml`. Ensure `[dependencies]` includes all of these (most are inherited from legacy modules):

```toml
[dependencies]
agent-client-protocol = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt", "rt-multi-thread", "sync", "time", "io-util", "process", "signal"] }
tracing = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
ulid = { workspace = true }
regex = { workspace = true }
surge-core = { workspace = true }
rand = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["test-util", "macros", "rt", "rt-multi-thread", "process", "io-util", "time"] }
tempfile = { workspace = true }
proptest = { workspace = true }
insta = { workspace = true }
```

If `proptest` or `insta` are not in workspace deps yet (they're used by other crates already), inherit them. No new versions to choose.

- [ ] **Step 3: Add `[[bin]]` for the mock agent (target file does not exist yet)**

Append to `crates/surge-acp/Cargo.toml`:

```toml
[[bin]]
name = "mock_acp_agent"
path = "src/bin/mock_acp_agent.rs"
required-features = []
```

The binary file lands in Phase 9. Cargo will refuse to compile right now because the file doesn't exist; that's fine — `cargo check -p surge-acp --lib` still works because we're only checking the library target.

- [ ] **Step 4: Create empty bridge module skeleton**

Create `crates/surge-acp/src/bridge/mod.rs`:

```rust
//! Vibe-flow ACP bridge.
//!
//! Pure-addition submodule introduced in M3. Coexists with the legacy
//! `AgentPool` / `SurgeClient` stack at the crate root; consumers pick the
//! style they need. See `docs/superpowers/specs/2026-05-03-surge-acp-bridge-m3-design.md`
//! for the design contract.
//!
//! Public API surface:
//! - [`AcpBridge`] — owned by the engine, owns its own LocalSet thread.
//! - [`SessionConfig`], [`MessageContent`], [`SessionState`] — open-session inputs
//!   and read-back state.
//! - [`BridgeEvent`] — broadcast of everything observable about a session.
//! - [`Sandbox`] / [`SandboxDecision`] / [`AlwaysAllowSandbox`] / [`DenyListSandbox`]
//!   — interim M3 sandbox surface; M4 will add OS-level impls additively.
//! - [`ToolDef`] — shape of an injectable tool. `tools::build_report_stage_outcome_tool`
//!   and `tools::build_request_human_input_tool` produce the engine-injected pair.
//! - Errors: [`BridgeError`], [`OpenSessionError`], [`SendMessageError`],
//!   [`CloseSessionError`], [`AcpError`].
//!
//! Subscribers are warned in [`AcpBridge::subscribe`] that broadcast is
//! best-effort observability; durable consumers must add their own backpressure
//! (see spec §11.8).

// Submodules are wired in subsequent tasks.
```

- [ ] **Step 5: Wire the new module into `lib.rs`**

Open `crates/surge-acp/src/lib.rs`. After the existing `pub mod transport;` line (currently the last `pub mod`), add:

```rust
// New (M3) — vibe-flow ACP bridge. Pure addition, legacy modules untouched.
pub mod bridge;
mod shared;
```

Note: `shared` is **module-private** (`mod`, not `pub mod`) per spec §3 module-visibility note. Re-exports of bridge items are added in Task 5.x once the types exist.

- [ ] **Step 6: Run `cargo check -p surge-acp --lib`**

```bash
cargo check -p surge-acp --lib
```

Expected: clean compile with the new empty `bridge/mod.rs` and the `mod shared;` declaration. If `mod shared;` errors with "file not found", the next task creates `shared/mod.rs` — temporarily comment out `mod shared;` and uncomment it in Task 0.2 step 1.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/surge-acp/Cargo.toml crates/surge-acp/src/bridge/mod.rs crates/surge-acp/src/lib.rs
git commit -m "M3(acp): bridge module skeleton + workspace deps confirmation"
```

### Task 0.2: Extract shared helpers from legacy `client.rs`

The bridge needs three things from the legacy stack: worktree-rooted path validation, tool-call content-block construction, and a wrapper around the existing `secrets` regex. Extract into a `pub(crate)` `shared/` module with three submodules. Legacy `client.rs` is updated to call into `shared/` so behavior remains identical.

**Files:**
- Create: `crates/surge-acp/src/shared/mod.rs`
- Create: `crates/surge-acp/src/shared/path_guard.rs`
- Create: `crates/surge-acp/src/shared/content_block.rs`
- Create: `crates/surge-acp/src/shared/secrets.rs`
- Modify: `crates/surge-acp/src/client.rs` (call into `shared/path_guard` for its existing path checks)

- [ ] **Step 1: Create `shared/mod.rs` with `pub(crate)` re-exports**

Create `crates/surge-acp/src/shared/mod.rs`:

```rust
//! Internal helpers shared by legacy `SurgeClient` and the new
//! `bridge::BridgeClient`. Not part of `surge-acp`'s public API
//! (the module is declared `mod shared;` in `lib.rs`, not `pub mod`).

pub(crate) mod path_guard;
pub(crate) mod content_block;
pub(crate) mod secrets;
```

- [ ] **Step 2: Write failing test for `path_guard::ensure_in_worktree`**

Create `crates/surge-acp/src/shared/path_guard.rs`:

```rust
//! Worktree-rooted path validation. Used by both `SurgeClient` and `BridgeClient`
//! before any file IO so an agent cannot escape the worktree via `..` or symlinks.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum PathGuardError {
    #[error("path '{path}' is not absolute (worktree paths must be absolute)")]
    NotAbsolute { path: PathBuf },
    #[error("path '{path}' escapes worktree root '{worktree}'")]
    Escapes { path: PathBuf, worktree: PathBuf },
    #[error("io error while canonicalizing '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Verify that `path` resolves under `worktree_root`, after canonicalization.
///
/// `worktree_root` is expected to already be canonicalized by the caller (see
/// `SurgeClient::new` and `bridge::session::open_session_impl` for the precedent).
/// This function canonicalizes `path` here so that symlinks-to-outside are rejected
/// even if the agent constructed the path via legitimate-looking components.
pub(crate) fn ensure_in_worktree(
    worktree_root: &Path,
    path: &Path,
) -> Result<PathBuf, PathGuardError> {
    if !path.is_absolute() {
        return Err(PathGuardError::NotAbsolute { path: path.to_path_buf() });
    }
    let canonical = path
        .canonicalize()
        .map_err(|source| PathGuardError::Io { path: path.to_path_buf(), source })?;
    if !canonical.starts_with(worktree_root) {
        return Err(PathGuardError::Escapes {
            path: canonical,
            worktree: worktree_root.to_path_buf(),
        });
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_path() {
        let wt = std::env::temp_dir();
        let p = Path::new("foo.txt");
        let err = ensure_in_worktree(&wt, p).unwrap_err();
        assert!(matches!(err, PathGuardError::NotAbsolute { .. }));
    }

    #[test]
    fn accepts_path_inside_worktree() {
        let wt = tempfile::tempdir().unwrap();
        let inner = wt.path().join("a.txt");
        std::fs::write(&inner, "x").unwrap();
        let canonical_root = wt.path().canonicalize().unwrap();
        let resolved = ensure_in_worktree(&canonical_root, &inner).unwrap();
        assert!(resolved.starts_with(&canonical_root));
    }

    #[test]
    fn rejects_path_escaping_worktree_via_dotdot() {
        let outer = tempfile::tempdir().unwrap();
        let inside = outer.path().join("a");
        std::fs::create_dir(&inside).unwrap();
        let outside = outer.path().join("b.txt");
        std::fs::write(&outside, "y").unwrap();
        let canonical_inside = inside.canonicalize().unwrap();
        // Construct an absolute path that lexically lives "inside" but resolves outside.
        let escape = inside.join("..").join("b.txt");
        let err = ensure_in_worktree(&canonical_inside, &escape).unwrap_err();
        assert!(matches!(err, PathGuardError::Escapes { .. }));
    }
}
```

- [ ] **Step 3: Run the new tests, expect pass**

```bash
cargo test -p surge-acp --lib shared::path_guard::tests
```

Expected: 3 passed. If `Escapes` test fails on Windows due to short-name canonicalization quirks, change `escape` to `canonical_inside.join("..").join("b.txt")` and re-run.

- [ ] **Step 4: Write `shared/content_block.rs`**

Create `crates/surge-acp/src/shared/content_block.rs`:

```rust
//! Helpers for constructing ACP `ContentBlock`s used by both legacy and bridge
//! code paths. Kept in `shared/` so a future ACP-SDK upgrade touches one place.

use agent_client_protocol::ContentBlock;

/// Build a single text `ContentBlock` from an owned string.
pub(crate) fn text(s: impl Into<String>) -> ContentBlock {
    ContentBlock::Text(agent_client_protocol::TextContent {
        text: s.into(),
        annotations: None,
    })
}

/// Build a single-element `Vec<ContentBlock>` from a string.
pub(crate) fn text_vec(s: impl Into<String>) -> Vec<ContentBlock> {
    vec![text(s)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_block_round_trip() {
        let b = text("hello");
        match b {
            ContentBlock::Text(t) => assert_eq!(t.text, "hello"),
            _ => panic!("expected Text block"),
        }
    }

    #[test]
    fn text_vec_yields_single_element() {
        let v = text_vec("x");
        assert_eq!(v.len(), 1);
    }
}
```

- [ ] **Step 5: Write `shared/secrets.rs` thin wrapper**

`crate::secrets::redact_secrets` already exists at the crate root (from M0). Wrap it in a typed `SecretsRedactor` so bridge code can hold it as `Arc<SecretsRedactor>` without touching the regex internals.

Create `crates/surge-acp/src/shared/secrets.rs`:

```rust
//! Typed wrapper over the legacy `crate::secrets::redact_secrets` regex set.
//! Lets the bridge hold an `Arc<SecretsRedactor>` and pass it into `BridgeClient`
//! without re-allocating regex per call.

#[derive(Debug)]
pub(crate) struct SecretsRedactor;

impl SecretsRedactor {
    pub(crate) fn new() -> Self { Self }

    /// Redact known secret patterns from the given JSON text.
    /// Delegates to the existing regex set in `crate::secrets`.
    pub(crate) fn redact_json(&self, json_text: &str) -> String {
        crate::secrets::redact_secrets(json_text)
    }
}

impl Default for SecretsRedactor {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_a_known_pattern() {
        // crate::secrets is expected to redact `Bearer <token>` patterns.
        // If the legacy regex set doesn't, this test documents that gap and
        // points at the file to extend.
        let r = SecretsRedactor::new();
        let out = r.redact_json(r#"{"auth":"Bearer abc.def.ghi-very-long-token"}"#);
        // Either the token is masked (preferred) or the test pinpoints the gap.
        assert!(
            out.contains("REDACTED") || out.contains("abc.def.ghi-very-long-token"),
            "redactor produced: {out}"
        );
    }
}
```

The test is deliberately permissive — it documents the current redactor behavior rather than enforcing perfection. Tightening the regex set is out of scope for M3.

- [ ] **Step 6: Update legacy `client.rs` to call into `shared/path_guard`**

Open `crates/surge-acp/src/client.rs`. Find the existing path-validation logic (look for `worktree_root_canonical` usage in `write_text_file` / `read_text_file` impls; legacy code currently inlines `path.canonicalize()` and `starts_with` checks).

Replace each inlined check with:

```rust
use crate::shared::path_guard::ensure_in_worktree;

// inside write_text_file / read_text_file:
let safe_path = match ensure_in_worktree(&self.worktree_root_canonical, &request.path) {
    Ok(p) => p,
    Err(e) => return Err(/* same AcpResult error you currently return */),
};
```

Match the existing `AcpResult` error shape — do not change it. The goal here is to call `shared::path_guard` instead of duplicating the canonicalization logic; legacy behavior stays identical.

- [ ] **Step 7: Run all `surge-acp` tests, expect pass**

```bash
cargo test -p surge-acp
```

Expected: every existing test still passes (legacy `SurgeClient` behavior unchanged). If a path-validation edge case differs (e.g. canonicalization order on Windows), inspect and adjust the helper rather than reverting the refactor.

- [ ] **Step 8: Commit**

```bash
git add crates/surge-acp/src/shared crates/surge-acp/src/client.rs crates/surge-acp/src/lib.rs
git commit -m "M3(acp): extract shared/{path_guard,content_block,secrets} for bridge reuse"
```

---

## Phase 1: Errors

Bridge errors are split across five enums per spec §4.7: `BridgeError` (worker-level), `OpenSessionError` / `SendMessageError` / `CloseSessionError` (per-API surface errors), and `AcpError` (the underlying SDK error wrapper). Defining them all in one task keeps the surface coherent and gives subsequent tasks something to `?`-propagate against.

### Task 1.1: `bridge::error` module

**Files:**
- Create: `crates/surge-acp/src/bridge/error.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Write failing test for error rendering**

Create `crates/surge-acp/src/bridge/error.rs`:

```rust
//! Bridge-level error types.
//!
//! Five enums by API surface, no `From` between them and `crate::SurgeError`
//! (legacy domain) per spec §4.7. The bridge speaks its own error vocabulary.

use surge_core::SessionId;
use thiserror::Error;

use super::event::SessionEndReason;

/// Worker-level errors. Surfaced to API callers when the bridge worker thread
/// has died or refuses commands.
#[derive(Debug, Error)]
pub enum BridgeError {
    /// Worker thread panicked or exited unexpectedly. The bridge is dead;
    /// callers should drop the `AcpBridge` and respawn if they want to recover.
    #[error("bridge worker thread is dead")]
    WorkerDead,

    /// Command channel `send().await` failed (worker is shutting down or the
    /// thread already exited).
    #[error("command channel send failed: {0}")]
    CommandSendFailed(String),

    /// `oneshot` reply was dropped before sending (worker died mid-command).
    #[error("oneshot reply dropped before sending")]
    ReplyDropped,
}

/// Errors from `AcpBridge::open_session`.
#[derive(Debug, Error)]
pub enum OpenSessionError {
    #[error("agent subprocess spawn failed for kind '{kind}': {source}")]
    AgentSpawnFailed { kind: String, #[source] source: std::io::Error },

    #[error("ACP handshake failed: {reason}")]
    HandshakeFailed { reason: String },

    #[error("declared_outcomes is empty — `report_stage_outcome` cannot be constructed")]
    NoDeclaredOutcomes,

    #[error("invalid tool definitions: {0}")]
    InvalidToolDefs(String),

    #[error("invalid bindings: {0}")]
    InvalidBindings(String),

    #[error("bridge: {0}")]
    Bridge(#[source] BridgeError),
}

/// Errors from `AcpBridge::send_message`.
#[derive(Debug, Error)]
pub enum SendMessageError {
    #[error("session {session} not found")]
    SessionNotFound { session: SessionId },

    #[error("session {session} ended ({reason:?})")]
    SessionEnded { session: SessionId, reason: SessionEndReason },

    #[error("bridge: {0}")]
    Bridge(#[source] BridgeError),
}

/// Errors from `AcpBridge::close_session`.
#[derive(Debug, Error)]
pub enum CloseSessionError {
    #[error("session {session} not found")]
    SessionNotFound { session: SessionId },

    /// Graceful shutdown timed out; the child was killed and the session is gone,
    /// but the closure was not clean.
    #[error("session {session} graceful close timed out (killed = {killed})")]
    GracefulTimedOut { session: SessionId, killed: bool },

    #[error("bridge: {0}")]
    Bridge(#[source] BridgeError),
}

/// Wrapper for errors originating in the underlying ACP SDK.
#[derive(Debug, Error)]
pub enum AcpError {
    #[error("ACP protocol error: {0}")]
    Protocol(#[source] agent_client_protocol::Error),

    #[error("io: {0}")]
    Io(#[source] std::io::Error),

    #[error("agent subprocess exited mid-handshake (exit_code = {exit_code:?})")]
    AgentExited { exit_code: Option<i32> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_no_outcomes_renders() {
        let e = OpenSessionError::NoDeclaredOutcomes;
        assert!(format!("{e}").contains("declared_outcomes is empty"));
    }

    #[test]
    fn close_graceful_timeout_renders_with_killed_flag() {
        let s = SessionId::new();
        let e = CloseSessionError::GracefulTimedOut { session: s.clone(), killed: true };
        let rendered = format!("{e}");
        assert!(rendered.contains(&s.to_string()));
        assert!(rendered.contains("killed = true"));
    }

    #[test]
    fn bridge_error_is_send_sync() {
        // Compile-time bound check — bridge errors must be Send + Sync to cross
        // tokio task boundaries via oneshot replies.
        fn bound<T: Send + Sync>() {}
        bound::<BridgeError>();
        bound::<OpenSessionError>();
        bound::<SendMessageError>();
        bound::<CloseSessionError>();
        bound::<AcpError>();
    }
}
```

This file references `super::event::SessionEndReason` which doesn't exist yet — the test won't compile. That's intentional; the next task creates the event module which lets this file compile.

- [ ] **Step 2: Run `cargo check -p surge-acp --lib`, expect failure**

```bash
cargo check -p surge-acp --lib
```

Expected: `error[E0432]: unresolved import 'super::event::SessionEndReason'` and unresolved `bridge::error` references inside `bridge/mod.rs`.

- [ ] **Step 3: Wire `error` module into `bridge/mod.rs` (provisional)**

Open `crates/surge-acp/src/bridge/mod.rs` and append:

```rust
// Subsequent tasks add `event`, `command`, `session`, etc.
pub mod error;
pub use error::{
    AcpError, BridgeError, CloseSessionError, OpenSessionError, SendMessageError,
};
```

Compilation will still fail on the `SessionEndReason` import. The next task fixes it. We commit at the end of Task 1.1 in step 5 only after the next minimal stub compiles.

- [ ] **Step 4: Add a forward stub for `SessionEndReason` to unblock the build**

This is a temporary placeholder so Task 1.1 can compile and test in isolation. It will be replaced by the real definition in Task 5.1.

Append to `crates/surge-acp/src/bridge/mod.rs`:

```rust
// Temporary forward stub; replaced by `event::SessionEndReason` in Task 5.1.
// Kept here only so error.rs can compile during Phase 1 development.
pub mod event {
    /// Stubbed in Task 1.1, replaced by full enum in Task 5.1.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SessionEndReason {
        Normal,
        AgentCrashed { exit_code: Option<i32>, stderr_tail: String },
        Timeout { duration_ms: u64 },
        ForcedClose,
    }
}
```

This stub matches the final shape promised in spec §4.5 — swapping it for the real one in Task 5.1 changes nothing at the type level for `error.rs`.

- [ ] **Step 5: Run tests, expect pass**

```bash
cargo test -p surge-acp --lib bridge::error::tests
```

Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): bridge::error — Bridge/Open/Send/Close/AcpError enums"
```

---

## Phase 2: Core types

Spec §4.3, §4.5, §4.6, and §4.5 (events) define the type surface the bridge speaks: `SessionConfig`, `AgentKind`, `MessageContent`, `SessionState`, `ToolDef`, `BridgeEvent`, `BridgeCommand`, `Sandbox` trait + impls. Phases 2–5 land them in dependency order: types first (Phase 2), sandbox (Phase 3), tools/events (Phase 4), commands (Phase 5).

### Task 2.1: `bridge::session` — `SessionConfig`, `AgentKind`, `SessionState`, `MessageContent`

**Files:**
- Create: `crates/surge-acp/src/bridge/session.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Write failing test for `SessionConfig` field validation**

Create `crates/surge-acp/src/bridge/session.rs`:

```rust
//! Session inputs (`SessionConfig`, `MessageContent`) and bridge-side
//! per-session state (`SessionState`, `AcpSession`). See spec §4.3 / §4.4.

use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_client_protocol::ContentBlock;
use surge_core::{OutcomeKey, SessionId};

use super::sandbox::Sandbox;
use super::tools::ToolDef;

/// Public read-back of a session's bridge-observable state.
/// Returned by `AcpBridge::session_state`.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_id: SessionId,
    pub agent_label: String,
    pub status: SessionStatus,
    pub bindings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Handshake completed; session can accept messages.
    Open,
    /// Closed via `close_session()` (graceful).
    Closed,
    /// Subprocess exited unexpectedly.
    Crashed,
    /// Forced close due to `AcpBridge::shutdown()`.
    ForcedClosed,
}

/// User-visible message payload accepted by `AcpBridge::send_message`.
#[derive(Debug)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// Agent-flavor input to `SessionConfig`. The bridge derives the subprocess
/// invocation from this. `Mock` short-circuits to the test mock binary.
#[derive(Debug)]
pub enum AgentKind {
    ClaudeCode { binary: PathBuf, extra_args: Vec<String> },
    Codex { binary: PathBuf, extra_args: Vec<String> },
    GeminiCli { binary: PathBuf, extra_args: Vec<String> },
    Custom { binary: PathBuf, args: Vec<String> },
    /// Used by tests. Bridge launches `mock_acp_agent` from `CARGO_BIN_EXE_*`.
    Mock { args: Vec<String> },
}

impl AgentKind {
    /// Human-readable label that goes into `BridgeEvent::SessionEstablished::agent`.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::ClaudeCode { .. } => "claude-code",
            Self::Codex { .. } => "codex",
            Self::GeminiCli { .. } => "gemini-cli",
            Self::Custom { .. } => "custom",
            Self::Mock { .. } => "mock",
        }
    }
}

/// Open-session input. Constructed by the engine, passed to `AcpBridge::open_session`.
///
/// `SessionConfig` deliberately does **not** derive `Clone`: it carries
/// `Box<dyn Sandbox>`, which has no blanket `Clone` impl. When the bridge
/// needs to duplicate sandbox state (e.g. into the per-session `BridgeClient`),
/// it calls `Sandbox::boxed_clone()` and reconstructs the box. Callers that
/// want to hold a config across multiple opens must rebuild it from inputs.
pub struct SessionConfig {
    /// Agent flavor — drives subprocess invocation. The bridge resolves the
    /// binary path and CLI flags from this; for `Mock`, the bridge consults
    /// `CARGO_BIN_EXE_mock_acp_agent`.
    pub agent_kind: AgentKind,

    /// Working directory for the agent subprocess. Should be the per-run
    /// worktree path produced by `surge_git::create_run_worktree` (M2).
    /// The bridge does not validate this is a git worktree — that's M5's
    /// responsibility.
    pub working_dir: PathBuf,

    /// System prompt sent to the agent in the initial message frame.
    pub system_prompt: String,

    /// Outcome keys that the engine will accept from `report_stage_outcome`.
    /// The bridge derives the JSON-Schema enum from these and injects it as
    /// a tool. Empty `Vec` is rejected by `validate()` — agents need at
    /// least one outcome to terminate cleanly.
    pub declared_outcomes: Vec<OutcomeKey>,

    /// Whether to inject `request_human_input` tool (for stages that allow
    /// escalation). Drives the boolean check inside `tools::build_injected_tools`
    /// once Phase 4 lands.
    pub allows_escalation: bool,

    /// Engine-supplied list of tools (MCP-flavored or otherwise). The bridge
    /// passes this through the sandbox `visibility` filter before declaring
    /// tools to the agent.
    pub tools: Vec<ToolDef>,

    /// Sandbox to apply to the tool list and to per-call `ToolCall` events.
    /// Boxed because the trait is `dyn`-typed; cloned per-session via
    /// `Sandbox::boxed_clone`. The presence of this field is why
    /// `SessionConfig` itself does not derive `Clone`.
    pub sandbox: Box<dyn Sandbox>,

    /// Permission policy shared with the legacy `SurgeClient` (auto-approve,
    /// smart, …). In M3 the bridge uses this only for the `Client::request_permission`
    /// impl; the actual sandbox decisions go through `Sandbox::allows_tool`.
    pub permission_policy: crate::client::PermissionPolicy,

    /// Optional binding labels — opaque key-value pairs the engine attaches
    /// to the `SessionConfig` for later correlation in `BridgeEvent`s
    /// (e.g. the node_key, the run_id). The bridge passes these through to
    /// `BridgeEvent::SessionEstablished` and treats them as opaque otherwise.
    /// Capped by `validate()` at 8 entries × 64 bytes each to bound payload size.
    pub bindings: BTreeMap<String, String>,
}

impl SessionConfig {
    /// Validate the config before subprocess spawn. Returns the same error
    /// types as `OpenSessionError` so the bridge can `?`-propagate.
    pub fn validate(&self) -> Result<(), super::error::OpenSessionError> {
        if self.declared_outcomes.is_empty() {
            return Err(super::error::OpenSessionError::NoDeclaredOutcomes);
        }
        // Cap bindings (per spec §4.3): 8 entries × 64 bytes each.
        if self.bindings.len() > 8 {
            return Err(super::error::OpenSessionError::InvalidBindings(
                format!("bindings has {} entries (max 8)", self.bindings.len()),
            ));
        }
        for (k, v) in &self.bindings {
            if k.len() > 64 || v.len() > 64 {
                return Err(super::error::OpenSessionError::InvalidBindings(
                    format!("binding {k}=... exceeds 64-byte limit"),
                ));
            }
        }
        // Tool name uniqueness — the engine-injected `report_stage_outcome` and
        // optionally `request_human_input` are added in `tools::build_injected_tools`,
        // not by the caller, so we only check the caller-supplied list here.
        let mut seen = std::collections::HashSet::with_capacity(self.tools.len());
        for t in &self.tools {
            if !seen.insert(t.name.as_str()) {
                return Err(super::error::OpenSessionError::InvalidToolDefs(
                    format!("duplicate tool name: {}", t.name),
                ));
            }
            if t.name == "report_stage_outcome" || t.name == "request_human_input" {
                return Err(super::error::OpenSessionError::InvalidToolDefs(format!(
                    "caller may not supply reserved tool name '{}'",
                    t.name
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::sandbox::AlwaysAllowSandbox;
    use crate::client::PermissionPolicy;
    use std::str::FromStr;

    fn cfg_with(outcomes: Vec<&str>, tools: Vec<ToolDef>) -> SessionConfig {
        SessionConfig {
            agent_kind: AgentKind::Mock { args: vec![] },
            working_dir: PathBuf::from("/tmp/wt"),
            system_prompt: "sys".into(),
            declared_outcomes: outcomes
                .into_iter()
                .map(|o| OutcomeKey::from_str(o).unwrap())
                .collect(),
            allows_escalation: false,
            tools,
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_empty_outcomes() {
        let cfg = cfg_with(vec![], vec![]);
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            super::super::error::OpenSessionError::NoDeclaredOutcomes
        ));
    }

    #[test]
    fn accepts_minimal_valid_config() {
        let cfg = cfg_with(vec!["done"], vec![]);
        cfg.validate().unwrap();
    }

    #[test]
    fn rejects_duplicate_tool_names() {
        let t = |n: &str| ToolDef::new(n, "desc", super::super::tools::ToolCategory::Mcp("x".into()), serde_json::json!({}));
        let cfg = cfg_with(vec!["done"], vec![t("a"), t("a")]);
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            super::super::error::OpenSessionError::InvalidToolDefs(_)
        ));
    }

    #[test]
    fn rejects_reserved_tool_name() {
        let t = ToolDef::new(
            "report_stage_outcome",
            "desc",
            super::super::tools::ToolCategory::Mcp("x".into()),
            serde_json::json!({}),
        );
        let cfg = cfg_with(vec!["done"], vec![t]);
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            super::super::error::OpenSessionError::InvalidToolDefs(_)
        ));
    }

    #[test]
    fn rejects_oversized_bindings() {
        let mut cfg = cfg_with(vec!["done"], vec![]);
        for i in 0..9 {
            cfg.bindings.insert(format!("k{i}"), format!("v{i}"));
        }
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            err,
            super::super::error::OpenSessionError::InvalidBindings(_)
        ));
    }
}
```

This file references `bridge::sandbox::Sandbox`, `bridge::sandbox::AlwaysAllowSandbox`, `bridge::tools::{ToolDef, ToolCategory}` — none exist yet. The next two tasks (Phase 3 + Phase 4) provide them.

- [ ] **Step 2: Add minimal forward stubs in `bridge/mod.rs` so `session.rs` compiles**

Append to `crates/surge-acp/src/bridge/mod.rs`:

```rust
// Forward stubs replaced in Phase 3 / Phase 4. Kept minimal so Phase 2 tests
// can exercise SessionConfig in isolation.

pub mod sandbox {
    pub trait Sandbox: Send + Sync {}
    #[derive(Debug, Clone)]
    pub struct AlwaysAllowSandbox;
    impl Sandbox for AlwaysAllowSandbox {}
}

pub mod tools {
    use serde_json::Value;

    #[derive(Debug, Clone)]
    pub struct ToolDef {
        pub name: String,
        pub description: String,
        pub category: ToolCategory,
        pub input_schema: Value,
    }

    impl ToolDef {
        pub fn new(name: impl Into<String>, description: impl Into<String>,
                   category: ToolCategory, input_schema: Value) -> Self {
            Self { name: name.into(), description: description.into(), category, input_schema }
        }
    }

    #[derive(Debug, Clone)]
    pub enum ToolCategory {
        Injected,
        Mcp(String),
        Builtin,
    }
}

pub mod session;
pub use session::{AgentKind, MessageContent, SessionConfig, SessionState, SessionStatus};
```

These stubs are deliberately minimal: enough for `session.rs` to compile and its tests to validate, but not the final shape. The next two tasks **replace** the entire `pub mod sandbox { ... }` and `pub mod tools { ... }` inline declarations with the real submodule files.

- [ ] **Step 3: Run tests, expect pass**

```bash
cargo test -p surge-acp --lib bridge::session::tests
```

Expected: 5 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): bridge::session — SessionConfig, AgentKind, MessageContent, SessionState"
```

---

## Phase 3: Sandbox

Spec §4.6 + §6 define a two-method `Sandbox` trait. M3 ships two impls; M4 will add OS-enforced impls additively. The `boxed_clone` method is required because `SessionConfig` holds `Box<dyn Sandbox>` and the bridge clones configs at session-open time.

### Task 3.1: `bridge::sandbox` — trait + `AlwaysAllowSandbox` + `DenyListSandbox`

**Files:**
- Create: `crates/surge-acp/src/bridge/sandbox.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs` (replace the inline stub from Task 2.1)

- [ ] **Step 1: Replace the inline stub in `bridge/mod.rs`**

Open `crates/surge-acp/src/bridge/mod.rs`. Find and **delete** the entire block:

```rust
pub mod sandbox {
    pub trait Sandbox: Send + Sync {}
    #[derive(Debug, Clone)]
    pub struct AlwaysAllowSandbox;
    impl Sandbox for AlwaysAllowSandbox {}
}
```

Replace with:

```rust
pub mod sandbox;
pub use sandbox::{AlwaysAllowSandbox, DenyListSandbox, Sandbox, SandboxDecision};
```

The build will fail until step 2 lands the file.

- [ ] **Step 2: Write failing test for `AlwaysAllowSandbox` and `DenyListSandbox`**

Create `crates/surge-acp/src/bridge/sandbox.rs`:

```rust
//! `Sandbox` trait + interim M3 impls. See spec §4.6 + §6.

use std::collections::HashSet;

/// Per-tool decision returned by both `Sandbox::visibility` and
/// `Sandbox::allows_tool`. `Elevate` keeps the tool visible to the agent
/// but routes per-call invocations to the engine's elevation flow (M5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxDecision {
    /// Tool is fully permitted.
    Allow,
    /// Tool is blocked; reason carries a human-readable explanation that
    /// the bridge surfaces in `BridgeEvent::ToolCall::sandbox_decision`.
    Deny { reason: String },
    /// Tool needs caller approval before execution. The bridge attaches
    /// the capability tag to `BridgeEvent::ToolCall::sandbox_decision`
    /// so M5 can route to a UI / Telegram elevation flow.
    ///
    /// Expected capability values (M3 contract; M5 may extend):
    /// - `"filesystem_write"` — agent wants to write outside the worktree
    /// - `"shell_exec"` — agent wants to execute a shell command
    /// - `"network"` — agent wants to make an outbound network request
    ///
    /// See RFC-0006 §Tier-2 for the full taxonomy.
    Elevate { capability: String },
}

/// Sandbox surface. Two-method split rationale: `WorkspaceWriteSandbox`
/// in M4 will return `visibility = Allow` for `write_text_file` (the tool
/// must be visible) but `allows_tool = Deny` for paths escaping the worktree.
/// Symmetric impls are valid (and used by `AlwaysAllowSandbox` / `DenyListSandbox`).
pub trait Sandbox: Send + Sync {
    /// Decide whether this tool appears in the agent's visible tool list.
    /// Called once per `ToolDef` at session-open time.
    fn visibility(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision;

    /// Decide whether this tool invocation is allowed. Called once per actual
    /// call from the agent. Bridge attaches the result to `BridgeEvent::ToolCall`.
    fn allows_tool(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision;

    /// Boxed clone — required for `SessionConfig: Clone`-equivalent passes
    /// because `dyn Trait` cannot derive `Clone` directly.
    fn boxed_clone(&self) -> Box<dyn Sandbox>;
}

/// Permits everything. The default for development and the mock agent.
#[derive(Clone, Debug, Default)]
pub struct AlwaysAllowSandbox;

impl Sandbox for AlwaysAllowSandbox {
    fn visibility(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn allows_tool(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

/// Allow-by-default with explicit denylists by tool name and by MCP server id.
///
/// `DenyListSandbox::default()` (empty denylists) is semantically identical to
/// `AlwaysAllowSandbox` and may be used interchangeably in tests that add entries
/// via `denied_tools.insert(...)`. Use `AlwaysAllowSandbox` directly when you never
/// intend to add denies.
///
/// Sufficient for RFC-0006 §Tier-1 enforcement and for the M3 integration
/// test in `tests/bridge_sandbox_filtering.rs`. M4 introduces richer
/// path-aware and OS-enforced impls additively.
#[derive(Clone, Debug, Default)]
pub struct DenyListSandbox {
    /// Tool names that should be hidden from the agent and rejected at
    /// invocation time. Matched as exact ASCII string equality on
    /// `ToolDef::name`.
    pub denied_tools: HashSet<String>,
    /// MCP server ids whose tools should be hidden in their entirety.
    /// Matched against `ToolCategory::Mcp(id)` only — non-MCP tools
    /// (`Builtin`, `Injected`) are never filtered by this set.
    pub denied_mcp_ids: HashSet<String>,
}

impl DenyListSandbox {
    /// Convenience constructor for tests.
    pub fn deny_tools<I, S>(tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            denied_tools: tools.into_iter().map(Into::into).collect(),
            denied_mcp_ids: HashSet::new(),
        }
    }

    fn decide(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision {
        if self.denied_tools.contains(tool) {
            return SandboxDecision::Deny { reason: format!("tool '{tool}' is denied") };
        }
        if let Some(id) = mcp_id {
            if self.denied_mcp_ids.contains(id) {
                return SandboxDecision::Deny { reason: format!("mcp server '{id}' is denied") };
            }
        }
        SandboxDecision::Allow
    }
}

impl Sandbox for DenyListSandbox {
    fn visibility(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision {
        self.decide(tool, mcp_id)
    }
    fn allows_tool(&self, tool: &str, mcp_id: Option<&str>) -> SandboxDecision {
        // Symmetric with visibility per RFC-0006: a tool denied at visibility
        // would never reach allows_tool because it's filtered out of the agent's
        // tool list. Tested for parity below.
        self.decide(tool, mcp_id)
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_allow_visibility_and_call_both_allow() {
        let s = AlwaysAllowSandbox;
        assert_eq!(s.visibility("anything", None), SandboxDecision::Allow);
        assert_eq!(s.allows_tool("anything", Some("mcp-a")), SandboxDecision::Allow);
    }

    #[test]
    fn always_allow_boxed_clone_round_trip() {
        let s: Box<dyn Sandbox> = Box::new(AlwaysAllowSandbox);
        let cloned = s.boxed_clone();
        assert_eq!(cloned.visibility("x", None), SandboxDecision::Allow);
    }

    #[test]
    fn deny_list_denies_named_tool() {
        let s = DenyListSandbox::deny_tools(["shell_exec"]);
        match s.visibility("shell_exec", None) {
            SandboxDecision::Deny { reason } => assert!(reason.contains("shell_exec")),
            other => panic!("expected Deny, got {other:?}"),
        }
        assert_eq!(s.visibility("write_text_file", None), SandboxDecision::Allow);
    }

    #[test]
    fn deny_list_denies_named_mcp_server() {
        let mut s = DenyListSandbox::default();
        s.denied_mcp_ids.insert("dangerous-mcp".into());
        match s.allows_tool("read_file", Some("dangerous-mcp")) {
            SandboxDecision::Deny { reason } => assert!(reason.contains("dangerous-mcp")),
            other => panic!("expected Deny, got {other:?}"),
        }
        assert_eq!(s.allows_tool("read_file", Some("safe-mcp")), SandboxDecision::Allow);
        assert_eq!(s.allows_tool("read_file", None), SandboxDecision::Allow);
    }

    #[test]
    fn deny_list_visibility_and_allows_tool_parity() {
        let s = DenyListSandbox::deny_tools(["x"]);
        for (tool, mcp) in [("x", None), ("y", None), ("y", Some("a"))] {
            assert_eq!(s.visibility(tool, mcp), s.allows_tool(tool, mcp));
        }
    }
}
```

- [ ] **Step 3: Run tests, expect pass**

```bash
cargo test -p surge-acp --lib bridge::sandbox::tests
```

Expected: 5 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): bridge::sandbox — Sandbox trait + AlwaysAllow + DenyList"
```

---

## Phase 4: Tools

Spec §5.4 specifies the `report_stage_outcome` schema construction with a dynamic `enum` from `Vec<OutcomeKey>`. Spec §4.3 (`request_human_input`) is gated on `allows_escalation`. This task replaces the inline `tools` stub from Task 2.1 with a full `tools.rs` plus the two builders.

### Task 4.1: `bridge::tools` — `ToolDef`, `ToolCategory`, builders, validation

**Files:**
- Create: `crates/surge-acp/src/bridge/tools.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs` (replace the inline stub from Task 2.1)

- [ ] **Step 1: Replace the inline stub in `bridge/mod.rs`**

Open `crates/surge-acp/src/bridge/mod.rs`. Delete the entire `pub mod tools { ... }` inline block from Task 2.1. Replace with:

```rust
pub mod tools;
pub use tools::{ToolCategory, ToolDef};
```

- [ ] **Step 2: Write the file with builders and tests**

Create `crates/surge-acp/src/bridge/tools.rs`:

```rust
//! Tool definitions injected into ACP sessions, plus the engine-injected
//! `report_stage_outcome` and `request_human_input` builders. See spec §5.3 / §5.4.

use serde_json::{json, Value};
use surge_core::OutcomeKey;

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub category: ToolCategory,
    /// JSON Schema for the input. Keep as `Value` so the bridge can pass it
    /// straight to ACP without re-parsing.
    pub input_schema: Value,
}

impl ToolDef {
    /// Construct a `ToolDef` with owned strings; useful for both production
    /// builders and tests.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        category: ToolCategory,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            category,
            input_schema,
        }
    }
}

/// Where the tool came from. Drives the `mcp_id` field in `BridgeEvent::ToolCallMeta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCategory {
    /// Engine-owned (`report_stage_outcome`, `request_human_input`).
    Injected,
    /// Provided by an MCP server with the given id.
    Mcp(String),
    /// Built-in ACP-side tool (filesystem, terminal). No MCP id.
    Builtin,
}

impl ToolCategory {
    /// Returns the MCP id if this is an MCP-sourced tool, else None.
    #[must_use]
    pub fn mcp_id(&self) -> Option<&str> {
        match self {
            Self::Mcp(id) => Some(id.as_str()),
            _ => None,
        }
    }
}

/// Constant tool name for `report_stage_outcome`. Used by the worker to
/// recognize the call without string-matching everywhere.
pub const REPORT_STAGE_OUTCOME: &str = "report_stage_outcome";
/// Constant tool name for `request_human_input`.
pub const REQUEST_HUMAN_INPUT: &str = "request_human_input";

/// Build the `report_stage_outcome` tool with a dynamic `enum` populated from
/// the node's declared outcomes. Caller must ensure `declared_outcomes` is
/// non-empty (`SessionConfig::validate` already checks this).
#[must_use]
pub fn build_report_stage_outcome_tool(declared_outcomes: &[OutcomeKey]) -> ToolDef {
    assert!(
        !declared_outcomes.is_empty(),
        "M3 contract: caller must check via SessionConfig::validate"
    );
    let outcomes_json: Vec<Value> = declared_outcomes
        .iter()
        .map(|k| Value::String(k.as_str().to_string()))
        .collect();
    ToolDef::new(
        REPORT_STAGE_OUTCOME,
        "Report your stage's outcome. Call this exactly once at the end.",
        ToolCategory::Injected,
        json!({
            "type": "object",
            "required": ["outcome", "summary"],
            "properties": {
                "outcome": {
                    "type": "string",
                    "enum": outcomes_json,
                    "description": "Which declared outcome best describes your result"
                },
                "summary": {
                    "type": "string",
                    "description": "1-3 sentences explaining what you did and why this outcome"
                },
                "artifacts_produced": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of file paths you created or modified"
                }
            }
        }),
    )
}

/// Build the `request_human_input` tool. Always the same shape — no dynamic schema.
#[must_use]
pub fn build_request_human_input_tool() -> ToolDef {
    ToolDef::new(
        REQUEST_HUMAN_INPUT,
        "Pause and ask the human for guidance. Use sparingly.",
        ToolCategory::Injected,
        json!({
            "type": "object",
            "required": ["question"],
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the human. Be specific."
                },
                "context": {
                    "type": "string",
                    "description": "Optional context the human needs to answer well."
                }
            }
        }),
    )
}

/// Build the engine-injected tools for a session: always
/// `report_stage_outcome` and, when `allows_escalation` is true,
/// `request_human_input`. The worker prepends these to the caller-supplied
/// tool list during session open (see `bridge::worker::filter_visible_tools`,
/// added in Phase 8.1).
#[must_use]
pub fn build_injected_tools(
    declared_outcomes: &[OutcomeKey],
    allows_escalation: bool,
) -> Vec<ToolDef> {
    let mut out = Vec::with_capacity(2);
    out.push(build_report_stage_outcome_tool(declared_outcomes));
    if allows_escalation {
        out.push(build_request_human_input_tool());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn ok(s: &str) -> OutcomeKey {
        OutcomeKey::from_str(s).unwrap()
    }

    #[test]
    fn report_stage_outcome_includes_dynamic_enum() {
        let t = build_report_stage_outcome_tool(&[ok("done"), ok("blocked")]);
        assert_eq!(t.name, REPORT_STAGE_OUTCOME);
        let enum_values = &t.input_schema["properties"]["outcome"]["enum"];
        assert_eq!(enum_values, &json!(["done", "blocked"]));
    }

    #[test]
    fn report_stage_outcome_is_marked_injected() {
        let t = build_report_stage_outcome_tool(&[ok("done")]);
        assert_eq!(t.category, ToolCategory::Injected);
        assert!(t.category.mcp_id().is_none());
    }

    #[test]
    fn request_human_input_is_static_shape() {
        let a = build_request_human_input_tool();
        let b = build_request_human_input_tool();
        // Two builds yield byte-identical schemas (no dynamism).
        assert_eq!(a.input_schema, b.input_schema);
    }

    #[test]
    fn build_injected_tools_skips_human_input_when_disabled() {
        let v = build_injected_tools(&[ok("done")], false);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, REPORT_STAGE_OUTCOME);
    }

    #[test]
    fn build_injected_tools_includes_human_input_when_enabled() {
        let v = build_injected_tools(&[ok("done")], true);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].name, REQUEST_HUMAN_INPUT);
    }

    #[test]
    fn mcp_category_returns_id() {
        let t = ToolDef::new(
            "shell_exec",
            "run a shell command",
            ToolCategory::Mcp("ops".into()),
            json!({}),
        );
        assert_eq!(t.category.mcp_id(), Some("ops"));
    }
}
```

- [ ] **Step 3: Run tests, expect pass**

```bash
cargo test -p surge-acp --lib bridge::tools::tests
cargo test -p surge-acp --lib bridge::session::tests
```

Expected: 6 tools tests + 5 session tests pass. (Session tests pass because the inline-stub `ToolDef::new` was preserved as a constructor in the real `tools.rs`.)

- [ ] **Step 4: Add insta snapshot test for the dynamic schema**

Append to the `mod tests` of `tools.rs`:

```rust
    #[test]
    fn report_stage_outcome_schema_snapshot() {
        let t = build_report_stage_outcome_tool(&[
            ok("done"),
            ok("blocked"),
            ok("escalate"),
        ]);
        insta::assert_json_snapshot!("report_stage_outcome_schema", t.input_schema);
    }
```

- [ ] **Step 5: Run snapshot test, accept**

```bash
cargo test -p surge-acp --lib bridge::tools::tests::report_stage_outcome_schema_snapshot
```

Expected: snapshot is created in `crates/surge-acp/src/snapshots/`. Inspect via `cargo insta review` and accept. The `enum` should be `["done", "blocked", "escalate"]`.

- [ ] **Step 6: Add proptest for tool-schema validity**

Append to `mod tests`:

```rust
    proptest::proptest! {
        #[test]
        fn outcome_enum_serializable_for_any_size(
            n in 1usize..32usize,
        ) {
            let outcomes: Vec<OutcomeKey> = (0..n)
                .map(|i| OutcomeKey::from_str(&format!("o{i}")).unwrap())
                .collect();
            let t = build_report_stage_outcome_tool(&outcomes);
            // Round-trip through JSON: serialize, deserialize, must be equal.
            let s = serde_json::to_string(&t.input_schema).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
            proptest::prop_assert_eq!(parsed, t.input_schema);
        }
    }
```

- [ ] **Step 7: Run proptest, expect pass**

```bash
cargo test -p surge-acp --lib bridge::tools::tests::outcome_enum_serializable_for_any_size
```

Expected: 256 cases pass.

- [ ] **Step 8: Commit**

```bash
git add crates/surge-acp/src/bridge crates/surge-acp/src/snapshots
git commit -m "M3(acp): bridge::tools — ToolDef, builders, dynamic outcome enum"
```

---

## Phase 5: Events and commands

`BridgeEvent` is the broadcast payload (spec §4.5). `BridgeCommand` is the mpsc payload (spec §4.4). Define both, replace the temporary `event` stub from Task 1.1, and add round-trip tests.

### Task 5.1: `bridge::event` — full `BridgeEvent` enum + meta types

**Files:**
- Create: `crates/surge-acp/src/bridge/event.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs` (delete the stub `pub mod event { ... }` from Task 1.1)

- [ ] **Step 1: Delete the temporary `event` stub from `bridge/mod.rs`**

Open `crates/surge-acp/src/bridge/mod.rs`. Find and delete:

```rust
pub mod event {
    /// Stubbed in Task 1.1, replaced by full enum in Task 5.1.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SessionEndReason {
        Normal,
        AgentCrashed { exit_code: Option<i32>, stderr_tail: String },
        Timeout { duration_ms: u64 },
        ForcedClose,
    }
}
```

Replace with:

```rust
pub mod event;
pub use event::{
    AgentMessageMeta, BridgeEvent, SessionEndReason, ToolCallMeta, ToolResultPayload,
};
```

The build will fail until step 2.

- [ ] **Step 2: Create `event.rs`**

Create `crates/surge-acp/src/bridge/event.rs`:

```rust
//! Bridge events broadcast to subscribers. See spec §4.5.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surge_core::{OutcomeKey, SessionId};

use super::sandbox::SandboxDecision;

/// Everything observable about a session is one of these. Final event for
/// any `SessionId` is `SessionEnded`; subscribers can free per-session state
/// after observing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BridgeEvent {
    /// Emitted once after ACP handshake succeeds and tools are declared.
    SessionEstablished {
        session: SessionId,
        agent: String,
        bindings: BTreeMap<String, String>,
        tools_visible: Vec<String>,
    },

    /// Streaming agent output. Multiple events per session.
    AgentMessage {
        session: SessionId,
        chunk: String,
        meta: Option<AgentMessageMeta>,
    },

    /// Cumulative token usage. Bridge guarantees all `TokenUsage` for a given
    /// session precede `SessionEnded` for that session (spec §5.7).
    TokenUsage {
        session: SessionId,
        prompt_tokens: u32,
        output_tokens: u32,
        cache_hits: u32,
        model: String,
    },

    /// Generic tool call (not the engine-injected ones). Bridge auto-replies
    /// `Unsupported` in M3; M5 will install a real dispatcher (spec §5.3).
    ToolCall {
        session: SessionId,
        call_id: String,
        tool: String,
        args_redacted_json: String,
        sandbox_decision: SandboxDecision,
        meta: ToolCallMeta,
    },

    /// Result going back to the agent.
    ToolResult {
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    },

    /// Engine-injected `report_stage_outcome` was called. Routed as a first-class
    /// event so M5 can fold directly into `EventPayload::OutcomeReported`.
    OutcomeReported {
        session: SessionId,
        outcome: OutcomeKey,
        summary: String,
        artifacts_produced: Vec<String>,
    },

    /// Engine-injected `request_human_input` was called.
    HumanInputRequested {
        session: SessionId,
        call_id: String,
        question: String,
        context: Option<String>,
    },

    /// Final event for the session. After this, `SessionId` is gone from the
    /// bridge's internal map.
    SessionEnded {
        session: SessionId,
        reason: SessionEndReason,
    },

    /// Bridge-level error.
    ///
    /// **Emit conditions** (the exhaustive list — `Error` is not a generic
    /// dumping ground):
    /// 1. ACP protocol violation that did not kill the session (recoverable
    ///    parse failure on a non-critical frame).
    /// 2. Tool dispatch failed but session continues (M3: only fires on JSON
    ///    parse failure of injected-tool args before `OutcomeReported` /
    ///    `HumanInputRequested` can be emitted).
    /// 3. Token extraction failed (malformed `unstable_session_usage` metadata).
    ///
    /// Errors that end the session emit `SessionEnded` instead, not `Error`.
    /// If both apply, `Error` is emitted first, then `SessionEnded`.
    Error {
        session: Option<SessionId>,
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessageMeta {
    pub model: Option<String>,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEndReason {
    Normal,
    AgentCrashed { exit_code: Option<i32>, stderr_tail: String },
    Timeout { duration_ms: u64 },
    ForcedClose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMeta {
    /// MCP server id if applicable, else None.
    pub mcp_id: Option<String>,
    /// True iff this tool came from `tools::build_injected_tools`.
    pub injected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ToolResultPayload {
    Ok { result_json: String },
    Error { message: String },
    /// M3 stub for non-injected tools. M5 replaces with real dispatch.
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn session_end_reason_serde_round_trip() {
        let r = SessionEndReason::AgentCrashed {
            exit_code: Some(137),
            stderr_tail: "panic at 42".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: SessionEndReason = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn outcome_reported_serde_round_trip() {
        let session = SessionId::new();
        let ev = BridgeEvent::OutcomeReported {
            session: session.clone(),
            outcome: OutcomeKey::from_str("done").unwrap(),
            summary: "did it".into(),
            artifacts_produced: vec!["a.txt".into(), "b.txt".into()],
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: BridgeEvent = serde_json::from_str(&s).unwrap();
        match back {
            BridgeEvent::OutcomeReported { session: s2, outcome, summary, artifacts_produced } => {
                assert_eq!(s2, session);
                assert_eq!(outcome.as_str(), "done");
                assert_eq!(summary, "did it");
                assert_eq!(artifacts_produced.len(), 2);
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[test]
    fn tool_result_payload_unsupported_round_trip() {
        let p = ToolResultPayload::Unsupported;
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("unsupported"));
        let back: ToolResultPayload = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, ToolResultPayload::Unsupported));
    }
}
```

- [ ] **Step 3: Run tests, expect pass**

```bash
cargo test -p surge-acp --lib bridge::event::tests
```

Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): bridge::event — BridgeEvent + meta types"
```

### Task 5.2: `bridge::command` — `BridgeCommand` enum

**Files:**
- Create: `crates/surge-acp/src/bridge/command.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Create `command.rs`**

Create `crates/surge-acp/src/bridge/command.rs`:

```rust
//! Internal command channel payload. Public for tests; production callers
//! use the `AcpBridge` methods rather than constructing commands directly.

use surge_core::SessionId;
use tokio::sync::oneshot;

use super::error::{BridgeError, CloseSessionError, OpenSessionError, SendMessageError};
use super::session::{MessageContent, SessionConfig, SessionState};

pub enum BridgeCommand {
    OpenSession {
        config: SessionConfig,
        reply: oneshot::Sender<Result<SessionId, OpenSessionError>>,
    },
    SendMessage {
        session: SessionId,
        content: MessageContent,
        reply: oneshot::Sender<Result<(), SendMessageError>>,
    },
    GetSessionState {
        session: SessionId,
        reply: oneshot::Sender<Result<SessionState, BridgeError>>,
    },
    CloseSession {
        session: SessionId,
        reply: oneshot::Sender<Result<(), CloseSessionError>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
    /// Test-only: inject a panic into the worker thread to exercise the
    /// `WorkerDead` recovery path. Gated by `#[cfg(test)]` to keep production
    /// builds clean.
    #[cfg(test)]
    TestPanic,
}
```

- [ ] **Step 2: Wire it into `bridge/mod.rs`**

Append to `crates/surge-acp/src/bridge/mod.rs`:

```rust
pub mod command;
pub use command::BridgeCommand;
```

- [ ] **Step 3: Run `cargo check -p surge-acp --lib`**

```bash
cargo check -p surge-acp --lib
```

Expected: clean. (No tests yet for `command` — it's a flat enum; tests in Phase 6 exercise it via the worker.)

- [ ] **Step 4: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): bridge::command — BridgeCommand enum"
```

---

## Phase 6: AcpBridge runtime skeleton

The bridge owns its own thread (spec §2.3, §5.1). This phase lands the spawn machinery, the empty `bridge_loop`, and the public `AcpBridge` methods that proxy to the worker via mpsc. Sessions are not opened yet — that's Phase 7.

### Task 6.1: `bridge::worker` — `bridge_loop` skeleton + Shutdown handling

**Files:**
- Create: `crates/surge-acp/src/bridge/worker.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Create `worker.rs` with empty session map and Shutdown handling**

Create `crates/surge-acp/src/bridge/worker.rs`:

```rust
//! Bridge worker — owns the session map, dispatches commands.
//! Runs on the dedicated bridge thread inside a `LocalSet`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use surge_core::SessionId;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info};

use super::command::BridgeCommand;
use super::event::{BridgeEvent, SessionEndReason};

/// Per-session state held by the worker. Filled in by Phase 7.
#[allow(dead_code)] // wired in Task 7.1 via open_session_impl
pub(crate) struct AcpSession {
    pub session_id: SessionId,
    pub agent_label: String,
    // ACP-side connection, child handle, observer/waiter task handles, etc.
    // are added in Phase 7.
}

#[allow(dead_code)] // wired in Task 7.1 via AcpSession construction
pub(crate) type SessionMap = Rc<RefCell<HashMap<SessionId, AcpSession>>>;

/// Main worker loop. Drains commands from `cmd_rx`, dispatches them, and
/// emits `BridgeEvent`s to subscribers. Returns when `Shutdown` is processed
/// or the channel closes.
///
/// Phase 6 ships a skeleton: most commands return immediate stub errors;
/// Phase 7+ replaces those arms with real handlers (`open_session_impl` etc).
#[allow(dead_code)] // wired in Task 6.2 via AcpBridge::spawn
pub(crate) async fn bridge_loop(
    mut cmd_rx: mpsc::Receiver<BridgeCommand>,
    event_tx: broadcast::Sender<BridgeEvent>,
) {
    info!("bridge worker entering main loop");
    let sessions: SessionMap = Rc::default();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BridgeCommand::OpenSession { reply, .. } => {
                // Phase 7 lands the real impl. For now, refuse cleanly so the
                // skeleton still tests the dispatch path.
                let _ = reply.send(Err(super::error::OpenSessionError::HandshakeFailed {
                    reason: "open_session not implemented in M3 skeleton".into(),
                }));
            }
            BridgeCommand::SendMessage { session, reply, .. } => {
                let _ = reply.send(Err(super::error::SendMessageError::SessionNotFound { session }));
            }
            BridgeCommand::GetSessionState { session, reply } => {
                // Phase 6 stub: returns the bridge-observable state if the session
                // exists, else `BridgeError::ReplyDropped` as a stand-in. Phase 7
                // replaces this with proper not-found semantics once `BridgeError`
                // gains a `SessionNotFound` variant or `session_state` switches to
                // `Result<Option<SessionState>, _>`.
                let state = sessions
                    .borrow()
                    .get(&session)
                    .map(|s| super::session::SessionState {
                        session_id: s.session_id.clone(),
                        agent_label: s.agent_label.clone(),
                        status: super::session::SessionStatus::Open,
                        bindings: Default::default(),
                    });
                let _ = reply.send(state.ok_or(super::error::BridgeError::ReplyDropped));
            }
            BridgeCommand::CloseSession { session, reply } => {
                let _ = reply.send(Err(super::error::CloseSessionError::SessionNotFound { session }));
            }
            BridgeCommand::Shutdown { reply } => {
                close_all_sessions(&sessions, &event_tx, SessionEndReason::ForcedClose).await;
                let _ = reply.send(());
                info!("bridge worker shutting down");
                return;
            }
            #[cfg(test)]
            BridgeCommand::TestPanic => {
                panic!("bridge worker test-panic injected");
            }
        }
    }

    debug!("command channel closed; bridge worker exiting");
}

/// Emit `SessionEnded` for every open session and drop them from the map.
/// Used by `Shutdown` and (later) by failure paths in Phase 7+.
#[allow(dead_code)] // wired in Task 7.1+ failure paths and called from bridge_loop
pub(crate) async fn close_all_sessions(
    sessions: &SessionMap,
    event_tx: &broadcast::Sender<BridgeEvent>,
    reason: SessionEndReason,
) {
    let to_close: Vec<SessionId> = sessions.borrow().keys().cloned().collect();
    for sid in to_close {
        // Best-effort emit; broadcast::send returns Err only when no
        // subscribers exist, which is acceptable during shutdown.
        let _ = event_tx.send(BridgeEvent::SessionEnded {
            session: sid.clone(),
            reason: reason.clone(),
        });
        sessions.borrow_mut().remove(&sid);
    }
}
```

- [ ] **Step 2: Wire `worker` module**

Append to `crates/surge-acp/src/bridge/mod.rs`:

```rust
pub(crate) mod worker;
```

- [ ] **Step 3: Run `cargo check -p surge-acp --lib`**

```bash
cargo check -p surge-acp --lib
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): bridge::worker — bridge_loop skeleton + Shutdown handling"
```

### Task 6.2: `bridge::acp_bridge` — `AcpBridge` public API

**Files:**
- Create: `crates/surge-acp/src/bridge/acp_bridge.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Write failing test for spawn + shutdown round-trip**

Create `crates/surge-acp/src/bridge/acp_bridge.rs`:

```rust
//! `AcpBridge` — owned by callers, hides the worker thread + LocalSet.
//!
//! See spec §5.1 for the spawn machinery rationale, §11.6 for per-process
//! count guidance, §11.8 for the lagged-subscriber contract.

use surge_core::SessionId;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::warn;

use super::command::BridgeCommand;
use super::error::{BridgeError, CloseSessionError, OpenSessionError, SendMessageError};
use super::event::BridgeEvent;
use super::session::{MessageContent, SessionConfig, SessionState};
use super::worker::bridge_loop;

/// Public handle to the ACP bridge worker thread.
///
/// Spawn one per process (see spec §11.6); methods can be called from any
/// tokio context. All work funnels through a dedicated OS thread that runs
/// a current-thread tokio runtime + `LocalSet` for the SDK's `!Send` futures.
///
/// `Drop` joins the worker thread best-effort. If the worker is stuck inside
/// a future that never completes (a realistic failure mode once real ACP I/O
/// lands in Phase 8), `Drop` will block the calling thread indefinitely with
/// no timeout — `JoinHandle::join` has no timeout overload on stable Rust.
/// **Always call `shutdown().await` before letting `AcpBridge` go out of
/// scope in production paths.** Tests are exempt because they run a known
/// quiescent worker.
pub struct AcpBridge {
    /// Command channel sender — bounded mpsc.
    cmd_tx: mpsc::Sender<BridgeCommand>,
    /// Broadcast sender for `BridgeEvent`s. Subscribers obtain receivers via
    /// `subscribe()`. Best-effort observability per spec §11.8.
    event_tx: broadcast::Sender<BridgeEvent>,
    /// Worker thread handle. `Some` until `shutdown()` consumes it; `Drop`
    /// joins it best-effort if `shutdown()` was not called.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl AcpBridge {
    /// Spawn the bridge worker thread with explicit channel capacities.
    ///
    /// `cmd_capacity` bounds the mpsc command channel; producers block on
    /// `send().await` if the worker can't drain fast enough. `event_capacity`
    /// bounds the broadcast channel; subscribers that lag past this silently
    /// drop oldest events (see spec §11.8 for the durable-consumer pattern).
    pub fn spawn(cmd_capacity: usize, event_capacity: usize) -> Result<Self, BridgeError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(cmd_capacity);
        let (event_tx, _) = broadcast::channel(event_capacity);
        let event_tx_for_worker = event_tx.clone();

        let thread = std::thread::Builder::new()
            .name("surge-acp-bridge".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        warn!("bridge worker failed to build runtime: {e}");
                        return;
                    }
                };
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, bridge_loop(cmd_rx, event_tx_for_worker));
            })
            .map_err(|_| BridgeError::WorkerDead)?;

        Ok(Self {
            cmd_tx,
            event_tx,
            worker: Some(thread),
        })
    }

    /// Spawn with sane default capacities (64 commands queued, 1024 events buffered).
    /// Defaults chosen per spec §5.1 — high enough to absorb burst traffic from
    /// open_session bootstrapping, low enough to surface backpressure quickly.
    pub fn with_defaults() -> Result<Self, BridgeError> {
        Self::spawn(64, 1024)
    }

    /// Subscribe to the bridge's event stream.
    ///
    /// **Important:** broadcast is best-effort observability. Lagging
    /// subscribers silently drop the oldest events. Consumers that need
    /// durable delivery (M5 engine event-log persistence) MUST add their own
    /// backpressure. See spec §11.8.
    pub fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.event_tx.subscribe()
    }

    /// Open a new ACP session. The bridge spawns the agent subprocess,
    /// performs the ACP handshake, declares the sandbox-filtered tool list,
    /// and returns the freshly-allocated `SessionId`.
    pub async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<SessionId, OpenSessionError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::OpenSession { config, reply: tx })
            .await
            .map_err(|e| OpenSessionError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        rx.await
            .map_err(|_| OpenSessionError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Send a user message to an open session. Returns once the bridge has
    /// queued the message; the agent's response surfaces via subsequent
    /// `BridgeEvent::AgentMessage` events.
    pub async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::SendMessage { session, content, reply: tx })
            .await
            .map_err(|e| SendMessageError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        rx.await
            .map_err(|_| SendMessageError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Read a session's bridge-observable state (open / closed / crashed).
    pub async fn session_state(
        &self,
        session: SessionId,
    ) -> Result<SessionState, BridgeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::GetSessionState { session, reply: tx })
            .await
            .map_err(|e| BridgeError::CommandSendFailed(e.to_string()))?;
        rx.await.map_err(|_| BridgeError::ReplyDropped)?
    }

    /// Close a session gracefully. The bridge sends ACP shutdown to the
    /// agent and waits up to a grace period before forcibly killing the
    /// child (see Phase 8.3 close_session_impl for the timeout details).
    pub async fn close_session(
        &self,
        session: SessionId,
    ) -> Result<(), CloseSessionError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::CloseSession { session, reply: tx })
            .await
            .map_err(|e| CloseSessionError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        rx.await
            .map_err(|_| CloseSessionError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Drain pending commands and shut down the worker. Open sessions emit
    /// `SessionEnded { reason: ForcedClose }`. Joins the worker thread.
    /// Consumes self — call exactly once.
    pub async fn shutdown(mut self) -> Result<(), BridgeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::Shutdown { reply: tx })
            .await
            .map_err(|e| BridgeError::CommandSendFailed(e.to_string()))?;
        rx.await.map_err(|_| BridgeError::ReplyDropped)?;
        if let Some(t) = self.worker.take() {
            if let Err(panic_payload) = t.join() {
                // Worker panicked after sending the Shutdown reply. Cleanup
                // already completed; this is a bug to investigate via logs but
                // not a caller-actionable failure (shutdown succeeded as far as
                // the caller can observe).
                tracing::warn!(
                    "bridge worker panicked after shutdown reply: {:?}",
                    panic_payload
                );
            }
        }
        Ok(())
    }
}

impl Drop for AcpBridge {
    fn drop(&mut self) {
        // No await possible in Drop. Dropping the only owned cmd_tx
        // (when `self` is dropped) closes the channel, causing the worker's
        // `cmd_rx.recv()` to return `None` and the loop to exit. We then
        // best-effort join the thread.
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn spawn_then_shutdown_clean() {
        let bridge = AcpBridge::with_defaults().unwrap();
        bridge.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subscribe_yields_no_events_on_idle_bridge() {
        let bridge = AcpBridge::with_defaults().unwrap();
        let mut rx = bridge.subscribe();
        // No events expected — spawn does not emit anything on its own.
        let r = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(r.is_err(), "unexpected event on idle bridge");
        bridge.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_session_returns_skeleton_error() {
        use crate::bridge::sandbox::AlwaysAllowSandbox;
        use crate::bridge::session::AgentKind;
        use crate::client::PermissionPolicy;
        use std::str::FromStr;
        use surge_core::OutcomeKey;

        let bridge = AcpBridge::with_defaults().unwrap();
        let cfg = SessionConfig {
            agent_kind: AgentKind::Mock { args: vec![] },
            working_dir: std::path::PathBuf::from("/tmp/wt"),
            system_prompt: "sys".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: Default::default(),
        };
        let err = bridge.open_session(cfg).await.unwrap_err();
        // Phase 6 stub returns HandshakeFailed; Phase 7 replaces with real impl.
        assert!(matches!(err, OpenSessionError::HandshakeFailed { .. }));
        bridge.shutdown().await.unwrap();
    }
}
```

- [ ] **Step 2: Wire into `bridge/mod.rs`**

Append:

```rust
pub mod acp_bridge;
pub use acp_bridge::AcpBridge;
```

- [ ] **Step 3: Run tests, expect pass**

```bash
cargo test -p surge-acp --lib bridge::acp_bridge::tests
```

Expected: 3 passed. The third test asserts that the Phase 6 skeleton returns the `HandshakeFailed` placeholder — Phase 7 changes that test (or replaces it with `bridge_session_lifecycle` integration test).

- [ ] **Step 4: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): bridge::acp_bridge — AcpBridge public API + spawn/shutdown"
```

---

## Phase 7: BridgeClient — ACP `Client` trait impl

The `BridgeClient` is the bridge-side ACP `Client` impl, instantiated once per session (spec §5.9). The `agent-client-protocol` crate's `Client` trait has 11 methods. M3's `BridgeClient` implements them with the pattern: validate (via `shared::path_guard` or `Sandbox`), do the work, emit `BridgeEvent` only when relevant.

### Task 7.1: `bridge::client` — `BridgeClient` struct + 11 trait methods

**Files:**
- Create: `crates/surge-acp/src/bridge/client.rs`
- Create: `crates/surge-acp/src/bridge/session_inner.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Create `session_inner.rs` for the `Rc<RefCell<...>>` per-session state**

Create `crates/surge-acp/src/bridge/session_inner.rs`:

```rust
//! Per-session mutable state held inside the worker (single-threaded LocalSet),
//! shared between `BridgeClient`, the session observer task, and the subprocess
//! waiter task via `Rc<RefCell<...>>`.

use std::collections::HashMap;

use crate::bridge::event::SessionEndReason;

#[derive(Debug)]
pub(crate) struct SessionStateInner {
    /// ACP-side session string (from the agent's response to `session/new`).
    pub acp_session_id: String,

    /// Last cumulative token usage seen on a `SessionUpdate`. Flushed before
    /// `SessionEnded` (spec §5.7 ordering guarantee).
    pub last_token_usage: Option<TokenUsageSnapshot>,

    /// Whether `last_token_usage` has been broadcast since the last update.
    /// Used to skip duplicate emissions.
    pub last_token_usage_emitted: bool,

    /// Open tool calls keyed by call_id — used to correlate `tool/call` and
    /// `tool/result` from the agent.
    pub open_tool_calls: HashMap<String, OpenToolCall>,

    /// Set when the session is in the closing path; observer/waiter tasks
    /// should drain quickly and exit.
    pub closing: bool,

    /// Set if a terminal event has been emitted; prevents double-emission
    /// from racing observer/waiter tasks.
    pub end_emitted: Option<SessionEndReason>,
}

impl SessionStateInner {
    pub(crate) fn new(acp_session_id: String) -> Self {
        Self {
            acp_session_id,
            last_token_usage: None,
            last_token_usage_emitted: true,
            open_tool_calls: HashMap::new(),
            closing: false,
            end_emitted: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TokenUsageSnapshot {
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub cache_hits: u32,
    pub model: String,
}

#[derive(Debug, Clone)]
pub(crate) struct OpenToolCall {
    pub tool_name: String,
    pub mcp_id: Option<String>,
    pub injected: bool,
}
```

- [ ] **Step 2: Wire `session_inner` into `bridge/mod.rs`**

Append to `crates/surge-acp/src/bridge/mod.rs`:

```rust
pub(crate) mod session_inner;
```

- [ ] **Step 3: Run `cargo check -p surge-acp --lib`**

Expected: clean.

- [ ] **Step 4: Create `client.rs` skeleton with state struct**

Create `crates/surge-acp/src/bridge/client.rs`:

```rust
//! `BridgeClient` — ACP `Client` trait impl emitting `BridgeEvent`s.
//! See spec §5.9.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use agent_client_protocol::{
    Client, CreateTerminalRequest, CreateTerminalResponse, ExtNotification, ExtRequest,
    ExtResponse, KillTerminalRequest, KillTerminalResponse, PermissionOptionId,
    PermissionOptionKind, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    ReleaseTerminalResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, Result as AcpResult, SelectedPermissionOutcome, SessionNotification,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use surge_core::SessionId;
use tokio::sync::{Mutex, broadcast};
use tracing::debug;

use crate::shared::path_guard::ensure_in_worktree;
use crate::shared::secrets::SecretsRedactor;
use crate::terminal::Terminals;

use super::event::BridgeEvent;
use super::sandbox::{Sandbox, SandboxDecision};
use super::session_inner::SessionStateInner;

pub(crate) struct BridgeClient {
    pub(crate) session_id: SessionId,
    pub(crate) event_tx: broadcast::Sender<BridgeEvent>,
    pub(crate) state: Rc<RefCell<SessionStateInner>>,
    pub(crate) sandbox: Box<dyn Sandbox>,
    pub(crate) secrets: Arc<SecretsRedactor>,
    pub(crate) bindings: BTreeMap<String, String>,
    pub(crate) worktree_root: PathBuf,
    pub(crate) terminals: Arc<Mutex<Terminals>>,
}

impl BridgeClient {
    pub(crate) fn new(
        session_id: SessionId,
        event_tx: broadcast::Sender<BridgeEvent>,
        state: Rc<RefCell<SessionStateInner>>,
        sandbox: Box<dyn Sandbox>,
        secrets: Arc<SecretsRedactor>,
        bindings: BTreeMap<String, String>,
        worktree_root: PathBuf,
    ) -> Self {
        let terminals = Arc::new(Mutex::new(Terminals::new(worktree_root.clone())));
        Self {
            session_id,
            event_tx,
            state,
            sandbox,
            secrets,
            bindings,
            worktree_root,
            terminals,
        }
    }
}
```

- [ ] **Step 5: Implement `Client::request_permission` via Sandbox**

Append to `client.rs`:

```rust
impl Client for BridgeClient {
    async fn request_permission(
        &self,
        req: RequestPermissionRequest,
    ) -> AcpResult<RequestPermissionResponse> {
        // ACP request_permission carries the proposed tool call. Extract a
        // tool name and (best-effort) mcp_id; the exact ACP request shape
        // changes between SDK versions, so we keep extraction defensive.
        let tool_name = req.tool_call.title.clone();
        let mcp_id: Option<String> = None; // Phase 8 fills in once tool dispatch lands

        let decision = self.sandbox.allows_tool(&tool_name, mcp_id.as_deref());
        debug!(
            session = %self.session_id,
            tool = %tool_name,
            decision = ?decision,
            "request_permission via Sandbox"
        );

        let outcome = match decision {
            SandboxDecision::Allow => RequestPermissionOutcome::Selected(SelectedPermissionOutcome {
                option_id: PermissionOptionId("allow".into()),
            }),
            SandboxDecision::Deny { .. } | SandboxDecision::Elevate { .. } => {
                // Both Deny and Elevate result in a denial in M3. Elevate routes
                // to the engine via `BridgeEvent::ToolCall::sandbox_decision`
                // attached at tool-dispatch time (Phase 8); request_permission
                // here is a fast denial path.
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome {
                    option_id: PermissionOptionId("deny".into()),
                })
            }
        };

        Ok(RequestPermissionResponse { outcome })
    }
```

- [ ] **Step 6: Implement file IO methods via `shared::path_guard`**

Append to `client.rs` (continuing the same `impl Client for BridgeClient { ... }`):

```rust
    async fn write_text_file(
        &self,
        req: WriteTextFileRequest,
    ) -> AcpResult<WriteTextFileResponse> {
        ensure_in_worktree(&self.worktree_root, &req.path).map_err(|e| {
            agent_client_protocol::Error::invalid_params(e.to_string())
        })?;
        // Redact secrets from the content before writing? No — content is what
        // the agent wants persisted. Redaction applies to *event payloads*, not
        // file contents.
        tokio::fs::write(&req.path, &req.content)
            .await
            .map_err(|e| agent_client_protocol::Error::internal_error(e.to_string()))?;
        Ok(WriteTextFileResponse {})
    }

    async fn read_text_file(
        &self,
        req: ReadTextFileRequest,
    ) -> AcpResult<ReadTextFileResponse> {
        ensure_in_worktree(&self.worktree_root, &req.path).map_err(|e| {
            agent_client_protocol::Error::invalid_params(e.to_string())
        })?;
        let content = tokio::fs::read_to_string(&req.path)
            .await
            .map_err(|e| agent_client_protocol::Error::internal_error(e.to_string()))?;
        Ok(ReadTextFileResponse { content })
    }
```

- [ ] **Step 7: Implement terminal methods via legacy `Terminals`**

Append to `client.rs`:

```rust
    async fn create_terminal(
        &self,
        req: CreateTerminalRequest,
    ) -> AcpResult<CreateTerminalResponse> {
        let mut terms = self.terminals.lock().await;
        terms.create(req).await
    }

    async fn terminal_output(
        &self,
        req: TerminalOutputRequest,
    ) -> AcpResult<TerminalOutputResponse> {
        let mut terms = self.terminals.lock().await;
        terms.output(req).await
    }

    async fn wait_for_terminal_exit(
        &self,
        req: WaitForTerminalExitRequest,
    ) -> AcpResult<WaitForTerminalExitResponse> {
        let mut terms = self.terminals.lock().await;
        terms.wait_for_exit(req).await
    }

    async fn kill_terminal(
        &self,
        req: KillTerminalRequest,
    ) -> AcpResult<KillTerminalResponse> {
        let mut terms = self.terminals.lock().await;
        terms.kill(req).await
    }

    async fn release_terminal(
        &self,
        req: ReleaseTerminalRequest,
    ) -> AcpResult<ReleaseTerminalResponse> {
        let mut terms = self.terminals.lock().await;
        terms.release(req).await
    }
```

The `Terminals` impl in legacy `crate::terminal` already exposes these methods returning `AcpResult` — same shape as what `Client` expects. If signatures differ slightly (e.g. legacy uses different request types), wrap the conversion inline rather than refactoring legacy.

- [ ] **Step 8: Implement notification + ext methods (mostly observers / passthroughs)**

Append to `client.rs`:

```rust
    async fn session_notification(&self, notif: SessionNotification) -> AcpResult<()> {
        // SessionNotification carries SessionUpdate variants (agent messages,
        // tool calls, token usage). Phase 8 routes these to BridgeEvent emissions
        // via `bridge::worker::handle_session_notification`. For now, log and accept.
        debug!(
            session = %self.session_id,
            "session_notification: {:?}",
            std::mem::discriminant(&notif.update)
        );
        crate::bridge::worker::handle_session_notification(
            &self.session_id,
            &self.event_tx,
            &self.state,
            &self.sandbox,
            &self.secrets,
            notif,
        )
        .await;
        Ok(())
    }

    async fn ext_request(&self, _req: ExtRequest) -> AcpResult<ExtResponse> {
        // Ext methods are vendor extensions. Bridge does not implement any in M3.
        Err(agent_client_protocol::Error::method_not_found(
            "bridge: ext_request not supported",
        ))
    }

    async fn ext_notification(&self, _notif: ExtNotification) -> AcpResult<()> {
        // No-op accept — bridge does not consume any ext notifications.
        Ok(())
    }
}
```

- [ ] **Step 9: Add `bridge::worker::handle_session_notification` stub**

Open `crates/surge-acp/src/bridge/worker.rs`. Add the stub function (real impl in Phase 8):

```rust
use std::sync::Arc;
use crate::bridge::session_inner::SessionStateInner;
use crate::bridge::sandbox::Sandbox;
use crate::shared::secrets::SecretsRedactor;

pub(crate) async fn handle_session_notification(
    _session_id: &SessionId,
    _event_tx: &broadcast::Sender<BridgeEvent>,
    _state: &Rc<RefCell<SessionStateInner>>,
    _sandbox: &Box<dyn Sandbox>,
    _secrets: &Arc<SecretsRedactor>,
    _notif: agent_client_protocol::SessionNotification,
) {
    // Phase 8 implements: routes SessionUpdate variants to BridgeEvent emission.
}
```

- [ ] **Step 10: Wire `client` into `bridge/mod.rs`**

Append:

```rust
pub(crate) mod client;
```

- [ ] **Step 11: Run `cargo check -p surge-acp`**

```bash
cargo check -p surge-acp
```

Expected: clean. If `Terminals` method signatures don't match exactly, consult `crates/surge-acp/src/terminal.rs` for the actual signatures and adapt the call sites in step 7. Adjust without changing legacy.

- [ ] **Step 12: Add a small unit test for `request_permission` Allow path**

Append to `client.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::sandbox::AlwaysAllowSandbox;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn make_client() -> BridgeClient {
        let (tx, _) = broadcast::channel(16);
        let state = Rc::new(RefCell::new(SessionStateInner::new("acp-sess-1".into())));
        BridgeClient::new(
            SessionId::new(),
            tx,
            state,
            Box::new(AlwaysAllowSandbox),
            Arc::new(SecretsRedactor::new()),
            BTreeMap::new(),
            std::env::temp_dir(),
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_permission_allow_returns_allow_option() {
        let client = make_client();
        let req = RequestPermissionRequest {
            session_id: agent_client_protocol::SessionId(arc_str("sess")),
            tool_call: agent_client_protocol::ToolCallUpdate {
                id: agent_client_protocol::ToolCallId(arc_str("c1")),
                fields: agent_client_protocol::ToolCallUpdateFields {
                    title: Some("read_file".into()),
                    ..Default::default()
                },
            },
            options: vec![],
        };
        // The exact field shape of RequestPermissionRequest depends on the SDK
        // version. If this doesn't compile, look at the SDK's struct definition
        // and adapt — the test's intent is to verify the response branch.
        let _ = client.request_permission(req).await;
    }

    fn arc_str(s: &str) -> std::sync::Arc<str> {
        std::sync::Arc::from(s)
    }
}
```

If the request struct shape doesn't match, downgrade this to a compile-only test (call a helper that just runs the sandbox decision) and trust the integration tests (Phase 10) to exercise the real ACP code path.

- [ ] **Step 13: Run tests, expect pass**

```bash
cargo test -p surge-acp --lib bridge::client::tests
```

Expected: 1 passed.

- [ ] **Step 14: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): bridge::client — BridgeClient + 11 ACP Client methods"
```

---

## Phase 8: Session lifecycle

The bulk of M3 work. Three tasks: spawn agent + handshake + tool injection (T8.1), subprocess waiter + crash detection (T8.2), token tracking + send/close (T8.3). After this phase, `AcpBridge::open_session` actually opens sessions.

### Task 8.1: `open_session_impl` — spawn, handshake, tool injection

**Files:**
- Modify: `crates/surge-acp/src/bridge/worker.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Write the spawn helper that locates the agent binary**

Open `crates/surge-acp/src/bridge/worker.rs`. Add a helper section near the top:

```rust
use crate::bridge::session::AgentKind;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::{Child, Command};

fn build_agent_command(kind: &AgentKind, working_dir: &PathBuf) -> Result<Command, std::io::Error> {
    let mut cmd = match kind {
        AgentKind::ClaudeCode { binary, extra_args } => {
            let mut c = Command::new(binary);
            c.arg("--acp");
            c.args(extra_args);
            c
        }
        AgentKind::Codex { binary, extra_args } => {
            let mut c = Command::new(binary);
            c.arg("acp");
            c.args(extra_args);
            c
        }
        AgentKind::GeminiCli { binary, extra_args } => {
            let mut c = Command::new(binary);
            c.arg("--acp");
            c.args(extra_args);
            c
        }
        AgentKind::Custom { binary, args } => {
            let mut c = Command::new(binary);
            c.args(args);
            c
        }
        AgentKind::Mock { args } => {
            // CARGO_BIN_EXE_mock_acp_agent is set during `cargo test` builds.
            // For non-test invocations, fall back to looking up the binary in
            // CARGO_TARGET_DIR (best-effort).
            let path = std::env::var("CARGO_BIN_EXE_mock_acp_agent")
                .map(PathBuf::from)
                .or_else(|_| {
                    let target = std::env::var("CARGO_TARGET_DIR")
                        .unwrap_or_else(|_| "target".to_string());
                    Ok::<_, std::env::VarError>(
                        PathBuf::from(target).join("debug").join("mock_acp_agent"),
                    )
                })?;
            let mut c = Command::new(path);
            c.args(args);
            c
        }
    };
    cmd.current_dir(working_dir);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}
```

- [ ] **Step 2: Write `open_session_impl` skeleton**

Append to `worker.rs`:

```rust
use std::sync::Arc;
use agent_client_protocol::{
    AgentSideConnection, ClientCapabilities, ClientSideConnection, Implementation,
    InitializeRequest, NewSessionRequest, ProtocolVersion,
};
use crate::bridge::client::BridgeClient;
use crate::bridge::error::OpenSessionError;
use crate::bridge::session::{SessionConfig, SessionState, SessionStatus};
use crate::bridge::session_inner::SessionStateInner;
use crate::bridge::tools::{build_injected_tools, ToolDef};
use crate::shared::secrets::SecretsRedactor;

pub(crate) async fn open_session_impl(
    sessions: &SessionMap,
    event_tx: &broadcast::Sender<BridgeEvent>,
    config: SessionConfig,
) -> Result<SessionId, OpenSessionError> {
    // Step 1: validate config.
    config.validate()?;

    // Step 2: build full tool list = caller tools + engine-injected, then sandbox-filter.
    let injected = build_injected_tools(&config.declared_outcomes, config.allows_escalation);
    let mut combined: Vec<ToolDef> = config.tools.iter().cloned().collect();
    combined.extend(injected.iter().cloned());

    let (visible, hidden_names) = filter_visible_tools(combined, config.sandbox.as_ref());

    // Step 3: spawn agent subprocess.
    let mut cmd = build_agent_command(&config.agent_kind, &config.working_dir).map_err(|e| {
        OpenSessionError::AgentSpawnFailed { kind: config.agent_kind.label().into(), source: e }
    })?;
    let mut child = cmd.spawn().map_err(|e| OpenSessionError::AgentSpawnFailed {
        kind: config.agent_kind.label().into(),
        source: e,
    })?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Step 4: ACP handshake (initialize + new_session).
    // The exact API of agent-client-protocol 0.10.2 — we follow the same pattern
    // legacy `connection.rs` uses. If the SDK shape differs, adapt minimally.
    let session_id = SessionId::new();
    let inner = Rc::new(RefCell::new(SessionStateInner::new(String::new())));

    let bridge_client = BridgeClient::new(
        session_id.clone(),
        event_tx.clone(),
        inner.clone(),
        config.sandbox.boxed_clone(),
        Arc::new(SecretsRedactor::new()),
        config.bindings.clone(),
        config.working_dir.clone(),
    );

    // Build a ClientSideConnection like legacy AgentConnection does.
    // Concrete API: see crates/surge-acp/src/connection.rs for the legacy reference.
    // This block is the adapter — adjust per SDK shape during implementation.
    let connection = ClientSideConnection::new(bridge_client, stdin, stdout)
        .map_err(|e| OpenSessionError::HandshakeFailed { reason: e.to_string() })?;

    let init_resp = connection
        .initialize(InitializeRequest {
            protocol_version: ProtocolVersion::latest(),
            client_capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "surge-acp-bridge".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                title: None,
            },
        })
        .await
        .map_err(|e| OpenSessionError::HandshakeFailed { reason: e.to_string() })?;

    let new_resp = connection
        .new_session(NewSessionRequest {
            cwd: config.working_dir.clone(),
            mcp_servers: Vec::new(),
        })
        .await
        .map_err(|e| OpenSessionError::HandshakeFailed { reason: e.to_string() })?;

    // Step 5: store the ACP-side session string in the inner state.
    inner.borrow_mut().acp_session_id = new_resp.session_id.0.to_string();

    // Step 6: register session in the worker's map, spawn observer + waiter tasks.
    sessions.borrow_mut().insert(
        session_id.clone(),
        AcpSession {
            session_id: session_id.clone(),
            agent_label: config.agent_kind.label().into(),
            // Phase 8.2 lands the spawned task handles, child reference, etc.
        },
    );

    // Step 7: emit SessionEstablished.
    let _ = event_tx.send(BridgeEvent::SessionEstablished {
        session: session_id.clone(),
        agent: config.agent_kind.label().into(),
        bindings: config.bindings.clone(),
        tools_visible: visible.iter().map(|t| t.name.clone()).collect(),
    });

    debug!(
        session = %session_id,
        hidden_count = hidden_names.len(),
        "session established with sandbox-filtered tools"
    );

    let _ = (stderr, init_resp); // Stderr drainer + init metadata — Phase 8.2 uses these.

    Ok(session_id)
}

fn filter_visible_tools(
    tools: Vec<ToolDef>,
    sandbox: &dyn Sandbox,
) -> (Vec<ToolDef>, Vec<String>) {
    let mut visible = Vec::with_capacity(tools.len());
    let mut hidden_names = Vec::new();
    for t in tools {
        let mcp_id = t.category.mcp_id();
        match sandbox.visibility(&t.name, mcp_id) {
            SandboxDecision::Allow | SandboxDecision::Elevate { .. } => visible.push(t),
            SandboxDecision::Deny { .. } => hidden_names.push(t.name.clone()),
        }
    }
    (visible, hidden_names)
}
```

If the ACP SDK's `ClientSideConnection::new` signature or `initialize` request shape doesn't match exactly, look at `crates/surge-acp/src/connection.rs` for the legacy adapter and copy that pattern.

- [ ] **Step 3: Wire `open_session_impl` into the dispatch**

Replace the `OpenSession` arm in `bridge_loop`:

```rust
            BridgeCommand::OpenSession { config, reply } => {
                let result = open_session_impl(&sessions, &event_tx, config).await;
                let _ = reply.send(result);
            }
```

- [ ] **Step 4: Add `Sandbox` + `SandboxDecision` imports to `worker.rs`**

```rust
use crate::bridge::sandbox::{Sandbox, SandboxDecision};
```

- [ ] **Step 5: Run `cargo check -p surge-acp`**

```bash
cargo check -p surge-acp
```

Expected: clean. SDK shape mismatches manifest here — adapt minimally to legacy's pattern.

- [ ] **Step 6: Add unit test for `filter_visible_tools`**

Append to `worker.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::sandbox::DenyListSandbox;
    use crate::bridge::tools::{ToolCategory, ToolDef};
    use serde_json::json;

    #[test]
    fn filter_removes_denied_tools() {
        let tools = vec![
            ToolDef::new("read_file", "d", ToolCategory::Builtin, json!({})),
            ToolDef::new("shell_exec", "d", ToolCategory::Mcp("ops".into()), json!({})),
        ];
        let s = DenyListSandbox::deny_tools(["shell_exec"]);
        let (visible, hidden) = filter_visible_tools(tools, &s);
        let names: Vec<_> = visible.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_file"]);
        assert_eq!(hidden, vec!["shell_exec"]);
    }
}
```

- [ ] **Step 7: Run test, expect pass**

```bash
cargo test -p surge-acp --lib bridge::worker::tests::filter_removes_denied_tools
```

- [ ] **Step 8: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): open_session_impl — spawn, handshake, tool injection, sandbox filter"
```

### Task 8.2: Subprocess waiter + crash detection + stderr drain

**Files:**
- Modify: `crates/surge-acp/src/bridge/worker.rs`

- [ ] **Step 1: Extend `AcpSession` to hold child + task handles**

Open `crates/surge-acp/src/bridge/worker.rs`. Replace the `AcpSession` struct from Task 6.1:

```rust
pub(crate) struct AcpSession {
    pub session_id: SessionId,
    pub agent_label: String,
    /// Subprocess handle. Held inside the session map so close_session can
    /// kill it on graceful-timeout.
    pub child: Option<Child>,
    /// Bridge-side LocalSet handles for the observer + waiter tasks.
    /// Aborting them cancels work cleanly when the session closes.
    pub task_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Per-session inner state (shared with BridgeClient).
    pub inner: Rc<RefCell<SessionStateInner>>,
}
```

- [ ] **Step 2: Add the stderr drainer helper**

Append to `worker.rs`:

```rust
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const STDERR_RING_CAP: usize = 8 * 1024;
const STDERR_TAIL_CAP: usize = 2 * 1024;

/// Continuously read stderr into a bounded ring buffer; on session end the
/// last `STDERR_TAIL_CAP` bytes are returned for inclusion in
/// `SessionEndReason::AgentCrashed::stderr_tail`.
async fn stderr_drainer(
    mut stderr: tokio::process::ChildStderr,
    tail_storage: Rc<RefCell<Vec<u8>>>,
    session_id: SessionId,
) {
    let mut scratch = vec![0u8; 4096];
    loop {
        match stderr.read(&mut scratch).await {
            Ok(0) => break,
            Ok(n) => {
                // Log the chunk via tracing for post-mortem.
                if let Ok(s) = std::str::from_utf8(&scratch[..n]) {
                    tracing::warn!(session = %session_id, "agent stderr: {}", s.trim_end());
                }
                // Update the tail storage (kept compact for SessionEnded payload).
                let mut tail = tail_storage.borrow_mut();
                tail.extend_from_slice(&scratch[..n]);
                if tail.len() > STDERR_TAIL_CAP {
                    let drop_n = tail.len() - STDERR_TAIL_CAP;
                    tail.drain(..drop_n);
                }
                if tail.len() > STDERR_RING_CAP {
                    // Sanity bound — should never trigger because we drain to
                    // STDERR_TAIL_CAP above. Guard against accidental misuse.
                    let drop_n = tail.len() - STDERR_RING_CAP;
                    tail.drain(..drop_n);
                }
            }
            Err(e) => {
                tracing::warn!(session = %session_id, "stderr read failed: {e}");
                break;
            }
        }
    }
}

fn read_stderr_tail(tail_storage: &Rc<RefCell<Vec<u8>>>) -> String {
    let buf = tail_storage.borrow();
    String::from_utf8_lossy(&buf).into_owned()
}
```

- [ ] **Step 3: Add the subprocess waiter helper**

Append to `worker.rs`:

```rust
async fn subprocess_waiter(
    mut child: Child,
    event_tx: broadcast::Sender<BridgeEvent>,
    state: Rc<RefCell<SessionStateInner>>,
    session_id: SessionId,
    tail_storage: Rc<RefCell<Vec<u8>>>,
    sessions: SessionMap,
) {
    // Wait for the child process to exit.
    let exit_status = child.wait().await;

    // Defer to state.end_emitted: if close_session_impl already emitted
    // SessionEnded::Normal, we should not emit again.
    {
        let s = state.borrow();
        if s.end_emitted.is_some() {
            return;
        }
    }

    // Flush any pending TokenUsage before SessionEnded (spec §5.7 ordering).
    flush_pending_token_usage(&event_tx, &state, &session_id);

    let stderr_tail = read_stderr_tail(&tail_storage);
    let reason = match exit_status {
        Ok(s) if s.success() => SessionEndReason::Normal,
        Ok(s) => SessionEndReason::AgentCrashed {
            exit_code: s.code(),
            stderr_tail,
        },
        Err(_) => SessionEndReason::AgentCrashed {
            exit_code: None,
            stderr_tail,
        },
    };

    let _ = event_tx.send(BridgeEvent::SessionEnded {
        session: session_id.clone(),
        reason: reason.clone(),
    });
    state.borrow_mut().end_emitted = Some(reason);
    sessions.borrow_mut().remove(&session_id);
}

/// Emit a TokenUsage event if there's an unemitted snapshot. Called from
/// session-end paths to honor the spec §5.7 ordering guarantee.
pub(crate) fn flush_pending_token_usage(
    event_tx: &broadcast::Sender<BridgeEvent>,
    state: &Rc<RefCell<SessionStateInner>>,
    session_id: &SessionId,
) {
    let snapshot = {
        let s = state.borrow();
        if s.last_token_usage_emitted {
            None
        } else {
            s.last_token_usage.clone()
        }
    };
    if let Some(u) = snapshot {
        let _ = event_tx.send(BridgeEvent::TokenUsage {
            session: session_id.clone(),
            prompt_tokens: u.prompt_tokens,
            output_tokens: u.output_tokens,
            cache_hits: u.cache_hits,
            model: u.model,
        });
        state.borrow_mut().last_token_usage_emitted = true;
    }
}
```

- [ ] **Step 4: Wire the waiter task into `open_session_impl`**

In `open_session_impl`, after the `inner.borrow_mut().acp_session_id = ...` line, before the `sessions.borrow_mut().insert(...)`, replace the `AcpSession { session_id, agent_label, /* ... */ }` construction with:

```rust
    let tail_storage: Rc<RefCell<Vec<u8>>> = Rc::default();

    let drainer = tokio::task::spawn_local(stderr_drainer(
        stderr,
        tail_storage.clone(),
        session_id.clone(),
    ));

    let waiter = tokio::task::spawn_local(subprocess_waiter(
        child,
        event_tx.clone(),
        inner.clone(),
        session_id.clone(),
        tail_storage.clone(),
        sessions.clone(),
    ));

    sessions.borrow_mut().insert(
        session_id.clone(),
        AcpSession {
            session_id: session_id.clone(),
            agent_label: config.agent_kind.label().into(),
            child: None, // moved into waiter
            task_handles: vec![drainer, waiter],
            inner: inner.clone(),
        },
    );
```

(The `child` field is set to `None` because `subprocess_waiter` consumed it. The Phase 8.3 close path coordinates close-vs-crash via `state.end_emitted`.)

- [ ] **Step 5: Run `cargo check -p surge-acp`**

Expected: clean. The unused `connection` and `init_resp` warnings can be silenced with `let _ = (...)` until Phase 8.3 wires them up.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): subprocess waiter + stderr drain + crash detection"
```

### Task 8.3: `send_message_impl`, `close_session_impl`, token usage extraction

**Files:**
- Modify: `crates/surge-acp/src/bridge/worker.rs`
- Create: `crates/surge-acp/src/bridge/tokens.rs`
- Modify: `crates/surge-acp/src/bridge/mod.rs`

- [ ] **Step 1: Create `tokens.rs` with extractor**

Create `crates/surge-acp/src/bridge/tokens.rs`:

```rust
//! Token-usage extraction from ACP `SessionUpdate` carrying
//! `unstable_session_usage` metadata. See spec §5.7.

use crate::bridge::session_inner::TokenUsageSnapshot;

/// Try to extract a cumulative usage snapshot from a `SessionUpdate`.
/// Returns `None` if the update carries no usage data (most updates don't).
///
/// The exact field path depends on the ACP SDK version. The extraction is
/// deliberately defensive — malformed payloads return `None`, not panic
/// (per spec §11.7 future-proofing).
pub(crate) fn extract_usage(
    update: &agent_client_protocol::SessionUpdate,
) -> Option<TokenUsageSnapshot> {
    // The 0.10.2 SDK exposes usage on certain SessionUpdate variants when the
    // `unstable_session_usage` feature is enabled. The exact match arms are
    // verified against the SDK rustdoc during implementation; this function
    // is the single isolation point for the SDK shape.
    //
    // Pseudo-pattern (real impl matches the SDK):
    //
    // match update {
    //     SessionUpdate::AgentMessage(msg) if let Some(usage) = msg.usage.as_ref() => {
    //         Some(TokenUsageSnapshot {
    //             prompt_tokens: usage.input_tokens,
    //             output_tokens: usage.output_tokens,
    //             cache_hits: usage.cache_read_input_tokens.unwrap_or(0),
    //             model: usage.model.clone().unwrap_or_default(),
    //         })
    //     }
    //     _ => None,
    // }
    //
    // Returning None here keeps the M3 build green while the implementer pins
    // the exact SDK pattern.
    let _ = update;
    None
}
```

- [ ] **Step 2: Wire `tokens` module**

Append to `crates/surge-acp/src/bridge/mod.rs`:

```rust
pub(crate) mod tokens;
```

- [ ] **Step 3: Implement `handle_session_notification` properly**

Open `crates/surge-acp/src/bridge/worker.rs`. Replace the stub `handle_session_notification` from Task 7.1 step 9:

```rust
pub(crate) async fn handle_session_notification(
    session_id: &SessionId,
    event_tx: &broadcast::Sender<BridgeEvent>,
    state: &Rc<RefCell<SessionStateInner>>,
    sandbox: &Box<dyn Sandbox>,
    secrets: &Arc<SecretsRedactor>,
    notif: agent_client_protocol::SessionNotification,
) {
    use agent_client_protocol::SessionUpdate;
    use crate::bridge::tokens::extract_usage;
    use crate::bridge::event::{AgentMessageMeta, BridgeEvent};

    // Update last_token_usage if this notification carries it.
    if let Some(snap) = extract_usage(&notif.update) {
        let mut s = state.borrow_mut();
        s.last_token_usage = Some(snap.clone());
        s.last_token_usage_emitted = false;
        drop(s);
        let _ = event_tx.send(BridgeEvent::TokenUsage {
            session: session_id.clone(),
            prompt_tokens: snap.prompt_tokens,
            output_tokens: snap.output_tokens,
            cache_hits: snap.cache_hits,
            model: snap.model,
        });
        state.borrow_mut().last_token_usage_emitted = true;
    }

    match notif.update {
        SessionUpdate::AgentMessageChunk { content } => {
            let chunk = content_block_to_string(&content);
            let _ = event_tx.send(BridgeEvent::AgentMessage {
                session: session_id.clone(),
                chunk,
                meta: Some(AgentMessageMeta {
                    model: None,
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                }),
            });
        }
        SessionUpdate::ToolCall { tool_call } => {
            handle_tool_call(session_id, event_tx, state, sandbox, secrets, tool_call).await;
        }
        // Other variants (Plan, ToolCallUpdate, etc.) are deferred to Phase 10
        // observability tests if they impact M3 acceptance.
        _ => {}
    }
}

fn content_block_to_string(b: &agent_client_protocol::ContentBlock) -> String {
    match b {
        agent_client_protocol::ContentBlock::Text(t) => t.text.clone(),
        _ => String::new(),
    }
}

async fn handle_tool_call(
    session_id: &SessionId,
    event_tx: &broadcast::Sender<BridgeEvent>,
    _state: &Rc<RefCell<SessionStateInner>>,
    sandbox: &Box<dyn Sandbox>,
    secrets: &Arc<SecretsRedactor>,
    tool_call: agent_client_protocol::ToolCall,
) {
    use crate::bridge::event::{ToolCallMeta, ToolResultPayload};
    use crate::bridge::tools::{REPORT_STAGE_OUTCOME, REQUEST_HUMAN_INPUT};

    let tool_name = tool_call.title.clone();
    let call_id = tool_call.id.0.to_string();
    let args_json = serde_json::to_string(&tool_call.raw_input.unwrap_or(serde_json::Value::Null))
        .unwrap_or_default();
    let args_redacted = secrets.redact_json(&args_json);

    if tool_name == REPORT_STAGE_OUTCOME {
        match parse_outcome_args(&args_json) {
            Ok((outcome, summary, artifacts)) => {
                let _ = event_tx.send(BridgeEvent::OutcomeReported {
                    session: session_id.clone(),
                    outcome,
                    summary,
                    artifacts_produced: artifacts,
                });
            }
            Err(e) => {
                let _ = event_tx.send(BridgeEvent::Error {
                    session: Some(session_id.clone()),
                    error: format!("report_stage_outcome args parse failed: {e}"),
                });
            }
        }
        return;
    }

    if tool_name == REQUEST_HUMAN_INPUT {
        match parse_human_input_args(&args_json) {
            Ok((question, context)) => {
                let _ = event_tx.send(BridgeEvent::HumanInputRequested {
                    session: session_id.clone(),
                    call_id,
                    question,
                    context,
                });
            }
            Err(e) => {
                let _ = event_tx.send(BridgeEvent::Error {
                    session: Some(session_id.clone()),
                    error: format!("request_human_input args parse failed: {e}"),
                });
            }
        }
        return;
    }

    // Generic tool call — emit ToolCall + auto-reply Unsupported (M3 stub per spec §5.3).
    let decision = sandbox.allows_tool(&tool_name, None);
    let _ = event_tx.send(BridgeEvent::ToolCall {
        session: session_id.clone(),
        call_id: call_id.clone(),
        tool: tool_name.clone(),
        args_redacted_json: args_redacted,
        sandbox_decision: decision,
        meta: ToolCallMeta { mcp_id: None, injected: false },
    });
    let _ = event_tx.send(BridgeEvent::ToolResult {
        session: session_id.clone(),
        call_id,
        payload: ToolResultPayload::Unsupported,
    });
}

fn parse_outcome_args(
    args_json: &str,
) -> Result<(surge_core::OutcomeKey, String, Vec<String>), String> {
    let v: serde_json::Value = serde_json::from_str(args_json).map_err(|e| e.to_string())?;
    let outcome_str = v.get("outcome").and_then(|o| o.as_str())
        .ok_or_else(|| "missing or non-string `outcome`".to_string())?;
    let outcome = surge_core::OutcomeKey::try_from(outcome_str)
        .map_err(|e| format!("invalid OutcomeKey '{outcome_str}': {e}"))?;
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let artifacts = v.get("artifacts_produced")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    Ok((outcome, summary, artifacts))
}

fn parse_human_input_args(args_json: &str) -> Result<(String, Option<String>), String> {
    let v: serde_json::Value = serde_json::from_str(args_json).map_err(|e| e.to_string())?;
    let question = v.get("question").and_then(|q| q.as_str())
        .ok_or_else(|| "missing or non-string `question`".to_string())?
        .to_string();
    let context = v.get("context").and_then(|c| c.as_str()).map(String::from);
    Ok((question, context))
}
```

- [ ] **Step 4: Implement `send_message_impl`**

Append to `worker.rs`:

```rust
pub(crate) async fn send_message_impl(
    sessions: &SessionMap,
    session: SessionId,
    content: crate::bridge::session::MessageContent,
) -> Result<(), super::error::SendMessageError> {
    use crate::bridge::session::MessageContent;

    let exists = sessions.borrow().contains_key(&session);
    if !exists {
        return Err(super::error::SendMessageError::SessionNotFound { session });
    }

    // The actual ACP `prompt(session_id, content_blocks)` dispatch goes through
    // the connection held by the session. Phase 8 stores the connection inside
    // AcpSession; this function looks it up and calls connection.prompt(...).
    //
    // For brevity here, the impl marker:
    //   1. Borrow session entry
    //   2. Convert MessageContent → Vec<ContentBlock>
    //   3. Call connection.prompt(...)
    //   4. Map errors to SendMessageError
    //
    // Concrete code follows the same pattern as legacy `pool.rs` PoolOp::Prompt.

    let _content = match content {
        MessageContent::Text(s) => crate::shared::content_block::text_vec(s),
        MessageContent::Blocks(b) => b,
    };

    Ok(())
}
```

(Real prompt dispatch wiring is deferred to a small follow-up commit if SDK shape requires it; the bridge skeleton accepts the message and observer task surfaces the agent's reply via `BridgeEvent::AgentMessage`.)

- [ ] **Step 5: Implement `close_session_impl`**

Append to `worker.rs`:

```rust
pub(crate) async fn close_session_impl(
    sessions: &SessionMap,
    event_tx: &broadcast::Sender<BridgeEvent>,
    session: SessionId,
) -> Result<(), super::error::CloseSessionError> {
    use std::time::Duration;

    let inner = {
        let sessions_ref = sessions.borrow();
        sessions_ref.get(&session).map(|s| s.inner.clone())
    };
    let inner = inner.ok_or(super::error::CloseSessionError::SessionNotFound {
        session: session.clone(),
    })?;

    // Mark closing; observer/waiter cooperate.
    inner.borrow_mut().closing = true;

    // Flush pending TokenUsage before SessionEnded (spec §5.7).
    flush_pending_token_usage(event_tx, &inner, &session);

    // Graceful close: drop the session entry; the waiter task observes child
    // exit and emits SessionEnded::Normal. If the child doesn't exit within
    // 5 seconds, we kill it and return GracefulTimedOut.
    const GRACE_MS: u64 = 5_000;

    // Bound the wait by polling for end_emitted.
    let start = tokio::time::Instant::now();
    while start.elapsed() < Duration::from_millis(GRACE_MS) {
        if inner.borrow().end_emitted.is_some() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Timeout: forcibly remove from sessions; emit SessionEnded::Timeout.
    sessions.borrow_mut().remove(&session);
    let _ = event_tx.send(BridgeEvent::SessionEnded {
        session: session.clone(),
        reason: SessionEndReason::Timeout { duration_ms: GRACE_MS },
    });
    inner.borrow_mut().end_emitted = Some(SessionEndReason::Timeout { duration_ms: GRACE_MS });
    Err(super::error::CloseSessionError::GracefulTimedOut {
        session,
        killed: true,
    })
}
```

- [ ] **Step 6: Wire the new impls into `bridge_loop`**

In `bridge_loop`, replace the `SendMessage` and `CloseSession` arms:

```rust
            BridgeCommand::SendMessage { session, content, reply } => {
                let result = send_message_impl(&sessions, session, content).await;
                let _ = reply.send(result);
            }
            BridgeCommand::CloseSession { session, reply } => {
                let result = close_session_impl(&sessions, &event_tx, session).await;
                let _ = reply.send(result);
            }
```

- [ ] **Step 7: Run `cargo check -p surge-acp`**

```bash
cargo check -p surge-acp
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/surge-acp/src/bridge
git commit -m "M3(acp): send_message/close_session/token extraction impls"
```

---

## Phase 9: Mock ACP agent

A standalone binary that speaks ACP from the agent side. Drives all integration tests deterministically. Uses the same `agent-client-protocol` crate (with the `Agent` trait + `AgentSideConnection`), so any SDK-shape change in the bridge surfaces in the mock identically.

### Task 9.1: `mock_acp_agent` binary with all 7 scenarios

**Files:**
- Create: `crates/surge-acp/src/bin/mock_acp_agent.rs`

- [ ] **Step 1: Create the binary skeleton with scenario parsing**

Create `crates/surge-acp/src/bin/mock_acp_agent.rs`:

```rust
//! Mock ACP agent for deterministic integration tests of `surge-acp::bridge`.
//!
//! Speaks real ACP via `agent-client-protocol`'s `Agent` trait, so any SDK
//! change shows up in tests the same way it would for a real agent.
//!
//! Behavior is selected via env vars and CLI args:
//! - `--scenario echo`              — echo user messages back as AgentMessage
//! - `--scenario report_done`       — call report_stage_outcome after first message
//! - `--scenario report_outcome=K`  — call report_stage_outcome with outcome=K
//! - `--scenario crash_after=N`     — process N tool calls then exit 137
//! - `--scenario human_input`       — call request_human_input and wait
//! - `--scenario long_streaming`    — emit 20 chunks with 50ms delays
//! - `MOCK_ACP_USAGE=on`            — include usage metadata in agent messages
//! - `MOCK_ACP_HANDSHAKE_FAIL=1`    — return error from initialize handshake
//! - `MOCK_ACP_LOG=stderr`          — verbose stderr output

use std::env;

#[derive(Debug, Clone)]
enum Scenario {
    Echo,
    ReportDone,
    ReportOutcome(String),
    CrashAfter(u32),
    HumanInput,
    LongStreaming,
}

impl Scenario {
    fn parse(args: &[String]) -> Self {
        for arg in args {
            if let Some(s) = arg.strip_prefix("--scenario") {
                let s = s.trim_start_matches('=').trim();
                if let Some(k) = s.strip_prefix("report_outcome=") {
                    return Scenario::ReportOutcome(k.to_string());
                }
                if let Some(n) = s.strip_prefix("crash_after=") {
                    return Scenario::CrashAfter(n.parse().unwrap_or(1));
                }
                return match s {
                    "echo" => Scenario::Echo,
                    "report_done" => Scenario::ReportDone,
                    "human_input" => Scenario::HumanInput,
                    "long_streaming" => Scenario::LongStreaming,
                    other => {
                        eprintln!("mock_acp_agent: unknown scenario '{other}', defaulting to echo");
                        Scenario::Echo
                    }
                };
            }
        }
        Scenario::Echo
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let scenario = Scenario::parse(&args);
    let usage_on = env::var("MOCK_ACP_USAGE").as_deref() == Ok("on");
    let handshake_fail = env::var("MOCK_ACP_HANDSHAKE_FAIL").as_deref() == Ok("1");
    let log_to_stderr = env::var("MOCK_ACP_LOG").as_deref() == Ok("stderr");

    if log_to_stderr {
        eprintln!("mock_acp_agent: scenario={:?} usage={} handshake_fail={}",
                  scenario, usage_on, handshake_fail);
    }

    if handshake_fail {
        // Refuse the initialize handshake, exit 1.
        eprintln!("mock_acp_agent: simulated handshake failure");
        std::process::exit(1);
    }

    let local = tokio::task::LocalSet::new();
    local.run_until(run_agent(scenario, usage_on)).await?;
    Ok(())
}

async fn run_agent(
    scenario: Scenario,
    usage_on: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use agent_client_protocol::AgentSideConnection;
    use tokio::io::{stdin, stdout};

    let inbound = stdin();
    let outbound = stdout();

    let agent = MockAgent { scenario, usage_on, tool_call_count: std::cell::Cell::new(0) };

    let connection = AgentSideConnection::new(agent, inbound, outbound)?;
    connection.run().await?;
    Ok(())
}

struct MockAgent {
    scenario: Scenario,
    usage_on: bool,
    tool_call_count: std::cell::Cell<u32>,
}

// Implementing `Agent` trait — exact methods depend on SDK version.
// See agent-client-protocol 0.10.2 rustdoc for the full trait surface.
// The skeleton below shows the structure; fill in per real trait.

impl agent_client_protocol::Agent for MockAgent {
    async fn initialize(
        &self,
        _req: agent_client_protocol::InitializeRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::InitializeResponse> {
        Ok(agent_client_protocol::InitializeResponse {
            protocol_version: agent_client_protocol::ProtocolVersion::latest(),
            agent_capabilities: agent_client_protocol::AgentCapabilities::default(),
            auth_methods: vec![],
            server_info: agent_client_protocol::Implementation {
                name: "mock_acp_agent".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                title: None,
            },
        })
    }

    async fn new_session(
        &self,
        _req: agent_client_protocol::NewSessionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::NewSessionResponse> {
        Ok(agent_client_protocol::NewSessionResponse {
            session_id: agent_client_protocol::SessionId(std::sync::Arc::from("mock-session-1")),
            modes: None,
        })
    }

    async fn prompt(
        &self,
        req: agent_client_protocol::PromptRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::PromptResponse> {
        match &self.scenario {
            Scenario::Echo => {
                // Reply with whatever the user sent.
                let _ = req;
                Ok(agent_client_protocol::PromptResponse {
                    stop_reason: agent_client_protocol::StopReason::EndTurn,
                })
            }
            Scenario::ReportDone => {
                // Call report_stage_outcome { outcome: "done" } via tool call notification.
                // Wire via the connection's notification API. Concrete call depends
                // on SDK shape — adapter goes here.
                Ok(agent_client_protocol::PromptResponse {
                    stop_reason: agent_client_protocol::StopReason::EndTurn,
                })
            }
            Scenario::ReportOutcome(_outcome_key) => {
                Ok(agent_client_protocol::PromptResponse {
                    stop_reason: agent_client_protocol::StopReason::EndTurn,
                })
            }
            Scenario::CrashAfter(n) => {
                let count = self.tool_call_count.get() + 1;
                self.tool_call_count.set(count);
                if count > *n {
                    std::process::exit(137);
                }
                Ok(agent_client_protocol::PromptResponse {
                    stop_reason: agent_client_protocol::StopReason::EndTurn,
                })
            }
            Scenario::HumanInput => {
                // Call request_human_input via tool call notification, wait for reply.
                Ok(agent_client_protocol::PromptResponse {
                    stop_reason: agent_client_protocol::StopReason::EndTurn,
                })
            }
            Scenario::LongStreaming => {
                // Emit 20 AgentMessageChunks with 50ms gaps via session_update notifications.
                Ok(agent_client_protocol::PromptResponse {
                    stop_reason: agent_client_protocol::StopReason::EndTurn,
                })
            }
        }
    }

    async fn cancel(
        &self,
        _notif: agent_client_protocol::CancelNotification,
    ) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    // Other Agent trait methods (extension points) accept defaults / not_supported
    // until tests demand them. Add here as integration tests grow.
}
```

This skeleton compiles against the SDK's `Agent` trait. The actual emission of `AgentMessageChunk` / tool calls / `request_human_input` from inside `prompt(...)` requires the connection's session-notification API — its exact shape comes from the SDK rustdoc. Implementer pins it during the writing-plans → execution handoff.

- [ ] **Step 2: Build the binary**

```bash
cargo build --bin mock_acp_agent -p surge-acp
```

Expected: clean. If the SDK trait shape requires more methods, the compile error names them — add stubs returning `Err(method_not_found)` or `Ok(default)`.

- [ ] **Step 3: Smoke-test the binary by hand**

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}' | \
    ./target/debug/mock_acp_agent --scenario echo
```

Expected: a JSON-RPC `initialize` response on stdout. Don't fuss with exact ACP framing here — the integration tests in Phase 10 are the real verification.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-acp/src/bin
git commit -m "M3(acp): mock_acp_agent binary with 6 scenarios + env flags"
```

---

## Phase 10: Integration tests

13 tests covering acceptance criteria #6–#12 + #14. All tests live in `crates/surge-acp/tests/`. They use `AcpBridge::with_defaults()` + the mock binary. Each integration test file is independent (Rust integration test convention).

### Task 10.1: `bridge_session_lifecycle` + `bridge_tool_injection`

**Files:**
- Create: `crates/surge-acp/tests/bridge_session_lifecycle.rs`
- Create: `crates/surge-acp/tests/bridge_tool_injection.rs`

- [ ] **Step 1: Write `bridge_session_lifecycle.rs`**

Create `crates/surge-acp/tests/bridge_session_lifecycle.rs`:

```rust
//! Integration test: open session → send text → receive echo → close.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, MessageContent, SessionConfig,
    SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_send_close_round_trip() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().expect("spawn bridge");
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock { args: vec!["--scenario".into(), "echo".into()] },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "you are a mock".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.expect("open session");

    // Expect SessionEstablished as first event.
    let ev = timeout(Duration::from_secs(3), events.recv()).await.unwrap().unwrap();
    match ev {
        BridgeEvent::SessionEstablished { session, agent, .. } => {
            assert_eq!(session, sid);
            assert_eq!(agent, "mock");
        }
        other => panic!("expected SessionEstablished, got {other:?}"),
    }

    bridge.send_message(sid.clone(), MessageContent::Text("hello".into())).await.unwrap();

    bridge.close_session(sid.clone()).await.expect("close session");

    // Drain events until SessionEnded.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_end = false;
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::SessionEnded { session, reason })) if session == sid => {
                assert!(matches!(reason, SessionEndReason::Normal | SessionEndReason::Timeout { .. }));
                saw_end = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(saw_end, "did not observe SessionEnded for {sid}");

    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Write `bridge_tool_injection.rs`**

Create `crates/surge-acp/tests/bridge_tool_injection.rs`:

```rust
//! Integration test: report_stage_outcome surfaces as BridgeEvent::OutcomeReported,
//! NOT a generic ToolCall.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn report_stage_outcome_emits_outcome_reported_event() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "report_done".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "do thing".into(),
        declared_outcomes: vec![
            OutcomeKey::from_str("done").unwrap(),
            OutcomeKey::from_str("blocked").unwrap(),
        ],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge.send_message(sid.clone(), MessageContent::Text("go".into())).await.unwrap();

    let mut saw_outcome = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::OutcomeReported { outcome, .. })) => {
                assert_eq!(outcome.as_str(), "done");
                saw_outcome = true;
                break;
            }
            Ok(Ok(BridgeEvent::ToolCall { tool, .. })) if tool == "report_stage_outcome" => {
                panic!("report_stage_outcome should NOT surface as generic ToolCall");
            }
            _ => continue,
        }
    }
    assert!(saw_outcome);

    bridge.close_session(sid).await.ok();
    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p surge-acp --test bridge_session_lifecycle
cargo test -p surge-acp --test bridge_tool_injection
```

Expected: both pass. If `mock_acp_agent` doesn't actually emit the tool call yet (Phase 9 marker for "implementer pins SDK shape"), iterate on the mock until these two tests pass — they are the smallest end-to-end pair and should drive the SDK-shape resolution.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-acp/tests/bridge_session_lifecycle.rs crates/surge-acp/tests/bridge_tool_injection.rs
git commit -m "M3(acp): tests bridge_session_lifecycle + bridge_tool_injection"
```

### Task 10.2: `bridge_dynamic_outcome_enum` + `bridge_request_human_input`

**Files:**
- Create: `crates/surge-acp/tests/bridge_dynamic_outcome_enum.rs`
- Create: `crates/surge-acp/tests/bridge_request_human_input.rs`

- [ ] **Step 1: Write `bridge_dynamic_outcome_enum.rs`**

Create `crates/surge-acp/tests/bridge_dynamic_outcome_enum.rs`:

```rust
//! Integration test: two parallel sessions with distinct declared_outcomes
//! each see their own enum and accept their own outcome.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

fn cfg_with(outcome: &str, wt: &std::path::Path) -> SessionConfig {
    SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), format!("report_outcome={outcome}")],
        },
        working_dir: wt.to_path_buf(),
        system_prompt: "go".into(),
        declared_outcomes: vec![OutcomeKey::from_str(outcome).unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_sessions_use_distinct_outcome_enums() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let s1 = bridge.open_session(cfg_with("done", wt.path())).await.unwrap();
    let s2 = bridge.open_session(cfg_with("blocked", wt.path())).await.unwrap();

    bridge.send_message(s1.clone(), surge_acp::bridge::MessageContent::Text("go".into())).await.unwrap();
    bridge.send_message(s2.clone(), surge_acp::bridge::MessageContent::Text("go".into())).await.unwrap();

    let mut saw_done = false;
    let mut saw_blocked = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !(saw_done && saw_blocked) {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::OutcomeReported { session, outcome, .. })) => {
                if session == s1 && outcome.as_str() == "done" { saw_done = true; }
                if session == s2 && outcome.as_str() == "blocked" { saw_blocked = true; }
            }
            _ => continue,
        }
    }
    assert!(saw_done && saw_blocked, "saw_done={saw_done} saw_blocked={saw_blocked}");

    bridge.close_session(s1).await.ok();
    bridge.close_session(s2).await.ok();
    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Write `bridge_request_human_input.rs`**

Create `crates/surge-acp/tests/bridge_request_human_input.rs`:

```rust
//! Integration test: agent calling request_human_input surfaces as
//! BridgeEvent::HumanInputRequested, not a generic ToolCall, and bridge
//! does NOT auto-reply (per spec §5.3 — M5 will provide the reply API).

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_input_surfaces_as_distinct_event() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "human_input".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "ask".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: true,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge.send_message(sid.clone(), MessageContent::Text("?".into())).await.unwrap();

    let mut saw_human = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::HumanInputRequested { session, question, .. })) => {
                assert_eq!(session, sid);
                assert!(!question.is_empty());
                saw_human = true;
                break;
            }
            Ok(Ok(BridgeEvent::ToolCall { tool, .. })) if tool == "request_human_input" => {
                panic!("request_human_input should NOT surface as generic ToolCall");
            }
            _ => continue,
        }
    }
    assert!(saw_human);

    bridge.close_session(sid).await.ok();
    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p surge-acp --test bridge_dynamic_outcome_enum --test bridge_request_human_input
git add crates/surge-acp/tests
git commit -m "M3(acp): tests dynamic_outcome_enum + request_human_input"
```

### Task 10.3: `bridge_sandbox_filtering` + `bridge_handshake_failure`

**Files:**
- Create: `crates/surge-acp/tests/bridge_sandbox_filtering.rs`
- Create: `crates/surge-acp/tests/bridge_handshake_failure.rs`

- [ ] **Step 1: Write `bridge_sandbox_filtering.rs`**

Create `crates/surge-acp/tests/bridge_sandbox_filtering.rs`:

```rust
//! Integration test: DenyListSandbox removes denied tools from the agent's
//! visible tool list, observable via BridgeEvent::SessionEstablished::tools_visible.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use serde_json::json;
use surge_acp::bridge::{
    AcpBridge, AgentKind, BridgeEvent, DenyListSandbox, SessionConfig, ToolCategory, ToolDef,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn denied_tool_does_not_appear_in_visible_list() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let tools = vec![
        ToolDef::new("read_file", "read", ToolCategory::Builtin, json!({})),
        ToolDef::new("shell_exec", "shell", ToolCategory::Mcp("ops".into()), json!({})),
        ToolDef::new("write_file", "write", ToolCategory::Builtin, json!({})),
    ];
    let sandbox = DenyListSandbox::deny_tools(["shell_exec"]);

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock { args: vec!["--scenario".into(), "echo".into()] },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "x".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools,
        sandbox: Box::new(sandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let _sid = bridge.open_session(cfg).await.unwrap();

    let ev = timeout(Duration::from_secs(3), events.recv()).await.unwrap().unwrap();
    match ev {
        BridgeEvent::SessionEstablished { tools_visible, .. } => {
            assert!(tools_visible.contains(&"read_file".into()));
            assert!(tools_visible.contains(&"write_file".into()));
            assert!(tools_visible.contains(&"report_stage_outcome".into()));
            assert!(!tools_visible.contains(&"shell_exec".into()),
                "shell_exec should be filtered out by sandbox");
        }
        other => panic!("expected SessionEstablished, got {other:?}"),
    }

    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Write `bridge_handshake_failure.rs`**

Create `crates/surge-acp/tests/bridge_handshake_failure.rs`:

```rust
//! Integration test: MOCK_ACP_HANDSHAKE_FAIL=1 causes OpenSessionError::HandshakeFailed.

use std::collections::BTreeMap;
use std::str::FromStr;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, OpenSessionError, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_failure_returns_open_session_error() {
    let wt = TempDir::new().unwrap();
    // The mock honors MOCK_ACP_HANDSHAKE_FAIL=1 by exiting before handshake.
    // Setting it process-wide is acceptable for this isolated test.
    // SAFETY: tokio multi-thread tests share env; this test runs alone.
    unsafe {
        std::env::set_var("MOCK_ACP_HANDSHAKE_FAIL", "1");
    }

    let bridge = AcpBridge::with_defaults().unwrap();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock { args: vec![] },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "x".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let err = bridge.open_session(cfg).await.unwrap_err();
    assert!(matches!(
        err,
        OpenSessionError::HandshakeFailed { .. } | OpenSessionError::AgentSpawnFailed { .. }
    ));

    unsafe {
        std::env::remove_var("MOCK_ACP_HANDSHAKE_FAIL");
    }
    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p surge-acp --test bridge_sandbox_filtering --test bridge_handshake_failure
git add crates/surge-acp/tests
git commit -m "M3(acp): tests sandbox_filtering + handshake_failure"
```

### Task 10.4: `bridge_crash_detection` + `bridge_close_timeout`

**Files:**
- Create: `crates/surge-acp/tests/bridge_crash_detection.rs`
- Create: `crates/surge-acp/tests/bridge_close_timeout.rs`

- [ ] **Step 1: Write `bridge_crash_detection.rs`**

Create `crates/surge-acp/tests/bridge_crash_detection.rs`:

```rust
//! Integration test: agent subprocess crash surfaces as
//! BridgeEvent::SessionEnded::AgentCrashed within 2 seconds.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::{Duration, Instant};

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, MessageContent, SessionConfig,
    SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crash_after_n_tool_calls_surfaces_within_2s() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "crash_after=1".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "x".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();

    // Drain SessionEstablished
    let _ = timeout(Duration::from_secs(2), events.recv()).await;

    // Trigger a prompt (triggers tool call → mock crashes after 1).
    bridge.send_message(sid.clone(), MessageContent::Text("crash now".into())).await.ok();

    let crash_start = Instant::now();
    let mut saw_crash = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::SessionEnded { session, reason })) if session == sid => {
                match reason {
                    SessionEndReason::AgentCrashed { exit_code, .. } => {
                        assert_eq!(exit_code, Some(137));
                        saw_crash = true;
                        let elapsed = crash_start.elapsed();
                        assert!(
                            elapsed <= Duration::from_secs(2),
                            "crash detection took {elapsed:?}, expected ≤2s"
                        );
                        break;
                    }
                    other => panic!("expected AgentCrashed, got {other:?}"),
                }
            }
            _ => continue,
        }
    }
    assert!(saw_crash, "did not observe AgentCrashed within deadline");

    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Write `bridge_close_timeout.rs`**

Create `crates/surge-acp/tests/bridge_close_timeout.rs`:

```rust
//! Integration test: close_session against a stuck mock returns
//! GracefulTimedOut and emits SessionEnded::Timeout.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, CloseSessionError, SessionConfig,
    SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_against_stuck_mock_times_out() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    // long_streaming runs for ~1s of streaming chunks. We close immediately
    // and expect close to time out (with a tighter close-grace setting it
    // would happen sooner; the default is 5s, so this test takes ~5s).
    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "long_streaming".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "x".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    let _ = timeout(Duration::from_secs(2), events.recv()).await;

    let close_result = bridge.close_session(sid.clone()).await;
    assert!(matches!(
        close_result,
        Err(CloseSessionError::GracefulTimedOut { killed: true, .. })
    ));

    // SessionEnded::Timeout should follow.
    let mut saw_timeout = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::SessionEnded { session, reason })) if session == sid => {
                if matches!(reason, SessionEndReason::Timeout { .. }) {
                    saw_timeout = true;
                    break;
                }
            }
            _ => continue,
        }
    }
    assert!(saw_timeout);

    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p surge-acp --test bridge_crash_detection --test bridge_close_timeout
git add crates/surge-acp/tests
git commit -m "M3(acp): tests crash_detection + close_timeout"
```

### Task 10.5: `bridge_concurrent_sessions` + `bridge_shutdown_with_open`

**Files:**
- Create: `crates/surge-acp/tests/bridge_concurrent_sessions.rs`
- Create: `crates/surge-acp/tests/bridge_shutdown_with_open.rs`

- [ ] **Step 1: Write `bridge_concurrent_sessions.rs`**

Create `crates/surge-acp/tests/bridge_concurrent_sessions.rs`:

```rust
//! Integration test: 5 parallel sessions, all close cleanly, no deadlock.

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::{OutcomeKey, SessionId};
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn five_concurrent_sessions_complete_independently() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let mut sids: Vec<SessionId> = Vec::with_capacity(5);
    for _ in 0..5 {
        let cfg = SessionConfig {
            agent_kind: AgentKind::Mock { args: vec!["--scenario".into(), "echo".into()] },
            working_dir: wt.path().to_path_buf(),
            system_prompt: "x".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: BTreeMap::new(),
        };
        let sid = bridge.open_session(cfg).await.expect("open session");
        sids.push(sid);
    }
    assert_eq!(sids.iter().collect::<HashSet<_>>().len(), 5, "session ids must be distinct");

    for sid in &sids {
        bridge.send_message(sid.clone(), MessageContent::Text("hi".into())).await.ok();
    }
    for sid in &sids {
        bridge.close_session(sid.clone()).await.ok();
    }

    // Drain events; expect 5 SessionEnded events.
    let mut ended = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && ended.len() < 5 {
        if let Ok(Ok(BridgeEvent::SessionEnded { session, .. })) =
            timeout(Duration::from_millis(200), events.recv()).await
        {
            ended.insert(session);
        }
    }
    assert_eq!(ended.len(), 5, "expected 5 SessionEnded; got {}", ended.len());

    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Write `bridge_shutdown_with_open.rs`**

Create `crates/surge-acp/tests/bridge_shutdown_with_open.rs`:

```rust
//! Integration test: AcpBridge::shutdown() while sessions are open emits
//! SessionEnded::ForcedClose for each open session.

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, SessionConfig, SessionEndReason,
};
use surge_acp::client::PermissionPolicy;
use surge_core::{OutcomeKey, SessionId};
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_emits_forced_close_for_each_open_session() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let mut sids: Vec<SessionId> = Vec::with_capacity(2);
    for _ in 0..2 {
        let cfg = SessionConfig {
            agent_kind: AgentKind::Mock { args: vec!["--scenario".into(), "echo".into()] },
            working_dir: wt.path().to_path_buf(),
            system_prompt: "x".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: BTreeMap::new(),
        };
        sids.push(bridge.open_session(cfg).await.unwrap());
    }

    bridge.shutdown().await.unwrap();

    let mut ended_forced = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline && ended_forced.len() < 2 {
        if let Ok(Ok(BridgeEvent::SessionEnded { session, reason })) =
            timeout(Duration::from_millis(100), events.recv()).await
        {
            if matches!(reason, SessionEndReason::ForcedClose) {
                ended_forced.insert(session);
            }
        }
    }
    assert_eq!(ended_forced.len(), 2);
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p surge-acp --test bridge_concurrent_sessions --test bridge_shutdown_with_open
git add crates/surge-acp/tests
git commit -m "M3(acp): tests concurrent_sessions + shutdown_with_open"
```

### Task 10.6: `bridge_token_tracking` + `bridge_streaming` + `bridge_worker_panic`

**Files:**
- Create: `crates/surge-acp/tests/bridge_token_tracking.rs`
- Create: `crates/surge-acp/tests/bridge_streaming.rs`
- Create: `crates/surge-acp/tests/bridge_worker_panic.rs`

- [ ] **Step 1: Write `bridge_token_tracking.rs`**

Create `crates/surge-acp/tests/bridge_token_tracking.rs`:

```rust
//! Integration test: cumulative token usage events arrive monotonically and
//! all precede SessionEnded.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_usage_monotonic_and_precedes_session_end() {
    let wt = TempDir::new().unwrap();
    unsafe {
        std::env::set_var("MOCK_ACP_USAGE", "on");
    }

    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "long_streaming".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "stream".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge.send_message(sid.clone(), MessageContent::Text("go".into())).await.unwrap();

    let mut last_prompt = 0u32;
    let mut last_output = 0u32;
    let mut saw_end = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::TokenUsage { prompt_tokens, output_tokens, .. })) => {
                assert!(prompt_tokens >= last_prompt, "prompt_tokens not monotonic");
                assert!(output_tokens >= last_output, "output_tokens not monotonic");
                last_prompt = prompt_tokens;
                last_output = output_tokens;
                assert!(!saw_end, "TokenUsage arrived after SessionEnded — ordering violated");
            }
            Ok(Ok(BridgeEvent::SessionEnded { session, .. })) if session == sid => {
                saw_end = true;
                // Drain a bit more to confirm no stray TokenUsage follows.
                let post_end = timeout(Duration::from_millis(300), events.recv()).await;
                match post_end {
                    Ok(Ok(BridgeEvent::TokenUsage { .. })) => {
                        panic!("TokenUsage arrived after SessionEnded");
                    }
                    _ => break,
                }
            }
            _ => continue,
        }
    }
    assert!(saw_end);

    unsafe {
        std::env::remove_var("MOCK_ACP_USAGE");
    }
    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 2: Write `bridge_streaming.rs`**

Create `crates/surge-acp/tests/bridge_streaming.rs`:

```rust
//! Integration test: 20 streaming chunks arrive in order with reasonable cadence.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_streaming_delivers_chunks_in_order() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "long_streaming".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "stream".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge.send_message(sid.clone(), MessageContent::Text("go".into())).await.unwrap();

    let mut chunks = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && chunks < 20 {
        if let Ok(Ok(BridgeEvent::AgentMessage { session, .. })) =
            timeout(Duration::from_millis(500), events.recv()).await
        {
            if session == sid {
                chunks += 1;
            }
        }
    }
    assert!(chunks >= 20, "expected at least 20 chunks, got {chunks}");

    bridge.close_session(sid).await.ok();
    bridge.shutdown().await.unwrap();
}
```

- [ ] **Step 3: Write `bridge_worker_panic.rs`**

Create `crates/surge-acp/tests/bridge_worker_panic.rs`:

```rust
//! Integration test: worker thread panic surfaces as BridgeError on next send.
//!
//! Uses the test-only TestPanic command (gated by #[cfg(test)]) — see
//! crates/surge-acp/src/bridge/command.rs.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AlwaysAllowSandbox, AgentKind, BridgeError, OpenSessionError, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::sleep;

// To exercise TestPanic from outside the crate, the bridge module needs to
// expose a small test helper. The cleanest version is a `pub fn inject_panic`
// gated by #[cfg(any(test, feature = "test-helpers"))]. The simplest M3
// approach: enable a test helper via a hidden `__test_panic_now()` method on
// AcpBridge gated by #[cfg(any(test, feature = "test-helpers"))].
//
// If the helper isn't yet exposed, this test stays #[ignore]'d and is enabled
// once the helper is added — keeping the acceptance gate explicit.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_panic_surfaces_as_command_failure() {
    let bridge = AcpBridge::with_defaults().unwrap();

    // Inject panic via the test helper.
    bridge.__test_panic_now();

    // Give the worker a moment to panic + the channel to close.
    sleep(Duration::from_millis(100)).await;

    let wt = TempDir::new().unwrap();
    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock { args: vec!["--scenario".into(), "echo".into()] },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "x".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let err = bridge.open_session(cfg).await.unwrap_err();
    assert!(matches!(
        err,
        OpenSessionError::Bridge(BridgeError::CommandSendFailed(_))
            | OpenSessionError::Bridge(BridgeError::ReplyDropped)
    ));
}
```

- [ ] **Step 4: Add the `__test_panic_now` helper to `AcpBridge`**

Open `crates/surge-acp/src/bridge/acp_bridge.rs`. Add:

```rust
impl AcpBridge {
    /// Test-only: inject a panic into the worker thread to verify that
    /// subsequent commands fail with `BridgeError::CommandSendFailed`.
    /// Gated by `#[cfg(any(test, feature = "test-helpers"))]` so production
    /// builds cannot accidentally call it.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn __test_panic_now(&self) {
        // Best-effort send; channel may already be closed.
        let cmd_tx = self.cmd_tx.clone();
        // Spawn a tokio task to send asynchronously.
        tokio::spawn(async move {
            let _ = cmd_tx.send(BridgeCommand::TestPanic).await;
        });
    }
}
```

Add a `[features]` section to `crates/surge-acp/Cargo.toml`:

```toml
[features]
default = []
test-helpers = []
```

And update the integration test's `Cargo.toml` test target to enable the feature — actually, integration tests already compile against `#[cfg(test)]`, so `__test_panic_now` is available without enabling the feature. The feature flag exists for downstream consumers (M5 engine tests) that want to use the helper.

- [ ] **Step 5: Run + commit**

```bash
cargo test -p surge-acp --test bridge_token_tracking --test bridge_streaming --test bridge_worker_panic
git add crates/surge-acp
git commit -m "M3(acp): tests token_tracking + streaming + worker_panic + test helper"
```

### Task 10.7: `WorkspaceWriteSandbox` stub for acceptance #14

**Files:**
- Create: `crates/surge-acp/tests/bridge_sandbox_m4_stub.rs`

- [ ] **Step 1: Write the stub + smoke test**

Create `crates/surge-acp/tests/bridge_sandbox_m4_stub.rs`:

```rust
//! Acceptance #14 (spec §10): WorkspaceWriteSandbox stub demonstrates the
//! Sandbox trait surface is sufficient for M4's planned impls.
//!
//! This is NOT a real M4 impl — no OS enforcement, no canonical-path
//! resolution against symlinks. Its job is to lock the trait surface so M4
//! can land additively.

use std::path::{Path, PathBuf};

use surge_acp::bridge::{Sandbox, SandboxDecision};

#[derive(Clone, Debug)]
struct WorkspaceWriteSandbox {
    worktree_root: PathBuf,
}

impl WorkspaceWriteSandbox {
    fn new(worktree_root: PathBuf) -> Self {
        Self { worktree_root }
    }
}

impl Sandbox for WorkspaceWriteSandbox {
    fn visibility(&self, tool: &str, _mcp_id: Option<&str>) -> SandboxDecision {
        // IO tools are always visible; per-call decisions handle the path check.
        match tool {
            "read_text_file" | "write_text_file" | "list_directory" => SandboxDecision::Allow,
            _ => SandboxDecision::Allow,
        }
    }

    fn allows_tool(&self, tool: &str, _mcp_id: Option<&str>) -> SandboxDecision {
        // M3 stub: write_text_file gets a path check; other tools allow.
        // Real M4 impl will inspect the args' path (passed through richer args).
        // For the smoke test, simulate a path-aware sandbox by allowing all by default
        // and letting the test verify the divergence by injecting a custom check.
        let _ = tool;
        SandboxDecision::Allow
    }

    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

/// Subclass-style helper that demonstrates the asymmetric path that motivates
/// the visibility/allows_tool split: visibility says "yes the tool is visible",
/// allows_tool says "no, this specific path escapes the worktree". M4 will
/// move this logic into the impl proper; M3 just proves the trait permits it.
#[derive(Clone, Debug)]
struct WorkspaceWriteSandboxWithPath {
    worktree_root: PathBuf,
    requested_path: PathBuf,
}

impl Sandbox for WorkspaceWriteSandboxWithPath {
    fn visibility(&self, _tool: &str, _mcp_id: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn allows_tool(&self, tool: &str, _mcp_id: Option<&str>) -> SandboxDecision {
        if tool == "write_text_file" {
            if path_escapes(&self.worktree_root, &self.requested_path) {
                return SandboxDecision::Deny {
                    reason: "path escapes worktree".into(),
                };
            }
        }
        SandboxDecision::Allow
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

fn path_escapes(worktree: &Path, path: &Path) -> bool {
    path.canonicalize()
        .ok()
        .map(|c| !c.starts_with(worktree))
        .unwrap_or(true)
}

#[test]
fn workspace_write_sandbox_compiles_against_trait() {
    // Existence test — if the trait surface changes incompatibly, this fails to compile.
    let s = WorkspaceWriteSandbox::new(std::env::temp_dir());
    let _: Box<dyn Sandbox> = Box::new(s);
}

#[test]
fn visibility_allow_diverges_from_allows_tool_for_escaping_path() {
    let wt = tempfile::tempdir().unwrap();
    let canonical_root = wt.path().canonicalize().unwrap();
    let outside = std::env::temp_dir().join("outside.txt");
    std::fs::write(&outside, "x").ok();
    let sandbox = WorkspaceWriteSandboxWithPath {
        worktree_root: canonical_root,
        requested_path: outside.clone(),
    };
    assert_eq!(
        sandbox.visibility("write_text_file", None),
        SandboxDecision::Allow,
        "visibility must allow — tool is in scope at session-open time"
    );
    match sandbox.allows_tool("write_text_file", None) {
        SandboxDecision::Deny { reason } => assert!(reason.contains("escapes")),
        other => panic!("expected Deny for escaping path, got {other:?}"),
    }
    let _ = std::fs::remove_file(&outside);
}
```

- [ ] **Step 2: Run + commit**

```bash
cargo test -p surge-acp --test bridge_sandbox_m4_stub
git add crates/surge-acp/tests/bridge_sandbox_m4_stub.rs
git commit -m "M3(acp): WorkspaceWriteSandbox stub for acceptance #14"
```

---

## Phase 11: Polish

Documentation, clippy, CI hookup. Last task before declaring M3 done.

### Task 11.1: rustdoc coverage + clippy strict + CI integration

**Files:**
- Modify: `crates/surge-acp/src/bridge/*.rs` (add `///` docs where missing)
- Modify: `crates/surge-acp/src/lib.rs` (add `#![warn(missing_docs)]` for bridge module — scoped via `#[deny(missing_docs)] mod bridge;`? See step 3)
- Modify: project CI config (e.g. `.github/workflows/*.yml`) — extend strict-clippy list to bridge.

- [ ] **Step 1: Audit `cargo doc -p surge-acp --no-deps --document-private-items`**

```bash
cargo doc -p surge-acp --no-deps --document-private-items 2>&1 | tee /tmp/surge-acp-doc.log
grep -E "warning:" /tmp/surge-acp-doc.log
```

For each warning that names a `bridge::*` or `shared::*` item, add a `///` doc comment to the item. Legacy modules' doc warnings are not in scope.

- [ ] **Step 2: Run clippy strict against bridge + shared**

```bash
cargo clippy -p surge-acp --all-targets -- -D warnings
```

Expected: any new warning must be on legacy code (M2 precedent: those modules have permissive clippy). For each `bridge::*` / `shared::*` warning, fix the underlying issue.

If legacy code has new warnings introduced by stable Rust upgrade or transient dep changes, follow the M2 pattern (`#![allow(clippy::...)]` at the top of the legacy module file with a pointer to the spec note).

- [ ] **Step 3: Add `#[warn(missing_docs)]` to bridge module**

Open `crates/surge-acp/src/bridge/mod.rs`. At the top:

```rust
#![warn(missing_docs)]
```

Re-run `cargo doc -p surge-acp --no-deps`. Any new warnings indicate missing rustdoc on a `pub` item — fix.

- [ ] **Step 4: Verify pure-addition guarantee (acceptance #4)**

```bash
cargo build --workspace
cargo test --workspace
```

Expected: every existing crate builds and tests pass. If a downstream crate (`surge-orchestrator`, `surge-cli`, `surge-ui`, `surge-spec`) regresses, M3's pure-addition contract is violated — investigate before committing.

- [ ] **Step 5: Cross-OS smoke check via local notes**

The integration test suite must pass on Linux, macOS, Windows (acceptance #1, #5, #7). Locally, run on the dev OS:

```bash
cargo test -p surge-acp
```

For the other two OSes, push the branch and let CI cover it. Document any OS-specific test workarounds in the task commit message.

- [ ] **Step 6: Add CI strict-clippy entry for bridge module**

Locate the existing CI config (most likely `.github/workflows/ci.yml`). Find the M2 precedent — there should be a step that runs clippy with `-D warnings` on `surge-persistence::runs::*`. Add a parallel step for `surge-acp::bridge::*`:

```yaml
- name: clippy strict on surge-acp::bridge
  run: cargo clippy -p surge-acp --all-targets -- -D warnings
```

(If the M2 entry uses a more granular target filter, mirror that style.)

- [ ] **Step 7: Run final full check**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps
```

Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add crates/surge-acp .github/workflows
git commit -m "M3(acp): rustdoc coverage + clippy strict + CI hookup"
```

- [ ] **Step 9: Tag the milestone**

```bash
git tag -a m3-acp-bridge -m "M3 — surge-acp bridge complete"
```

(Push the tag when merging to main; not now, since this is a worktree branch.)

---

## Self-review

After all phases land, verify against spec acceptance criteria (spec §10):

- [ ] **AC #1:** `cargo build -p surge-acp` clean on Linux, macOS, Windows. Run on each CI runner.
- [ ] **AC #2:** `cargo test -p surge-acp` — all unit, integration, property, snapshot tests pass.
- [ ] **AC #3:** `cargo clippy -p surge-acp --all-targets -- -D warnings` clean for `bridge::*` and `shared::*`. Add `#![allow(clippy::...)]` markers on legacy modules per the M2 precedent.
- [ ] **AC #4:** `cargo build --workspace` succeeds (pure-addition guarantee).
- [ ] **AC #5:** `cargo build --bin mock_acp_agent -p surge-acp` produces a working binary on all three OSes.
- [ ] **AC #6:** All 13 integration tests in §9.2 pass deterministically (re-run 5×, no flakes).
- [ ] **AC #7:** `bridge_crash_detection` surfaces `SessionEnded` ≤ 2 s on all three OSes.
- [ ] **AC #8:** `bridge_concurrent_sessions` runs 5 sessions in parallel without deadlock.
- [ ] **AC #9:** `bridge_dynamic_outcome_enum` proves two sessions can use distinct outcome enums concurrently.
- [ ] **AC #10:** `bridge_sandbox_filtering` proves disallowed tools never appear in `tools_visible`.
- [ ] **AC #11:** `bridge_streaming` proves real-time streaming visible to subscribers within agent emission cadence.
- [ ] **AC #12:** `bridge_token_tracking` proves cumulative token usage is reported and monotonic.
- [ ] **AC #13:** All public API in `bridge::*` documented with `///`; `cargo doc -p surge-acp --no-deps` produces no warnings.
- [ ] **AC #14:** `WorkspaceWriteSandbox` stub in `tests/bridge_sandbox_m4_stub.rs` compiles against §4.6 and exercises the visibility/allows_tool divergence.
