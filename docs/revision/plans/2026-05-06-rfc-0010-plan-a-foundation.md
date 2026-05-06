# RFC-0010 Issue-Tracker Integration · Plan A — Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the foundation for issue-tracker integration: new `surge-intake` crate with `trait TaskSource`, persistence schema for ticket lifecycle, computational dedup, and event multiplexer. After Plan A the system can detect ticket events from a `MockTaskSource`, deduplicate against active runs, and emit them downstream — but no real provider (Linear/GitHub) yet.

**Architecture:** New library crate `surge-intake` exposes the trait, shared types, mock implementation, dedup module, candidate-enumeration module, and `TaskRouter` for multiplexing. Persistence layer extended with `ticket_index` and `task_source_state` SQLite tables in `surge-persistence`. No async networking yet — that's Plan B (Linear/GitHub adapters). Plan A is fully unit-testable without external services.

**Tech Stack:** Rust 2024 stable, tokio, async-trait, serde, thiserror, ulid, rusqlite (via existing `surge-persistence`), insta (snapshot tests), proptest (property tests), tokio-test (async test utilities).

---

## File structure

### Created
- `crates/surge-intake/Cargo.toml` — new crate manifest
- `crates/surge-intake/src/lib.rs` — module declarations + re-exports
- `crates/surge-intake/src/types.rs` — `TaskId`, `TaskSummary`, `TaskDetails`, `TaskEvent`, `TaskEventKind`, `Priority`, `TriageDecision`, `Tier1Decision`
- `crates/surge-intake/src/source.rs` — `trait TaskSource` definition
- `crates/surge-intake/src/dedup.rs` — `Tier1PreFilter` (active-run lookup)
- `crates/surge-intake/src/candidates.rs` — keyword-overlap candidate enumerator for Triage Author input
- `crates/surge-intake/src/router.rs` — `TaskRouter` (multiplexer)
- `crates/surge-intake/src/testing.rs` — `MockTaskSource` for unit tests
- `crates/surge-intake/src/error.rs` — crate-local `Error` enum
- `crates/surge-persistence/migrations/0007_ticket_index.sql`
- `crates/surge-persistence/migrations/0008_task_source_state.sql`
- `crates/surge-persistence/src/intake.rs` — `TicketIndex` row type, `IntakeRepo` accessor

### Modified
- `Cargo.toml` (workspace root) — add `surge-intake` member
- `crates/surge-persistence/src/lib.rs` — re-export new `intake` module
- `crates/surge-persistence/src/models.rs` — add `TicketState` enum + helpers if applicable

---

## Task 0.1 — Add `surge-intake` to workspace

**Files:**
- Create: `crates/surge-intake/Cargo.toml`
- Create: `crates/surge-intake/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create the directory and stub `lib.rs`**

```bash
mkdir -p crates/surge-intake/src
```

Create `crates/surge-intake/src/lib.rs`:

```rust
//! Issue-tracker / task-source integration for Surge.
//!
//! Defines [`TaskSource`] trait and the shared types and computational
//! pipelines (dedup, candidate enumeration, multiplexer) that feed
//! incoming work into the vibe-flow bootstrap pipeline.
//!
//! See `docs/revision/rfcs/0010-issue-tracker-integration.md`.

pub mod candidates;
pub mod dedup;
pub mod error;
pub mod router;
pub mod source;
pub mod testing;
pub mod types;

pub use error::{Error, Result};
pub use source::TaskSource;
pub use types::{
    Priority, TaskDetails, TaskEvent, TaskEventKind, TaskId, TaskSummary, Tier1Decision,
    TriageDecision,
};
```

- [ ] **Step 2: Create `Cargo.toml`**

Create `crates/surge-intake/Cargo.toml`:

```toml
[package]
name = "surge-intake"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
surge-core = { workspace = true }
surge-persistence = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
async-trait = { workspace = true }
tracing = { workspace = true }
chrono = { workspace = true }
ulid = { workspace = true }
futures = { workspace = true }
tokio-stream = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }
insta = { workspace = true }
proptest = { workspace = true }
tempfile = { workspace = true }
tokio-test = "0.4"
```

- [ ] **Step 3: Add stub modules so `lib.rs` compiles**

Create empty placeholders:

```bash
touch crates/surge-intake/src/candidates.rs
touch crates/surge-intake/src/dedup.rs
touch crates/surge-intake/src/router.rs
touch crates/surge-intake/src/source.rs
touch crates/surge-intake/src/testing.rs
touch crates/surge-intake/src/types.rs
```

Each stub file gets:

```rust
// Filled in by later tasks.
```

Create `crates/surge-intake/src/error.rs`:

```rust
//! Error type for `surge-intake`.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Storage(#[from] surge_persistence::Error),

    #[error("invalid task id: {0}")]
    InvalidTaskId(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    #[error("internal: {0}")]
    Internal(String),
}
```

(Note: `surge_persistence::Error` will be wired through. If it doesn't yet exist as a public re-export, this `From` impl will be added in Task 2.4.)

For now, replace the `#[from] surge_persistence::Error` line with:

```rust
    #[error("storage error: {0}")]
    Storage(String),
```

We will tighten the type in Task 2.4.

- [ ] **Step 4: Add to workspace members**

Read the current root `Cargo.toml`:

```bash
sed -n '1,30p' Cargo.toml
```

Find the `[workspace]` section's `members` array and add `"crates/surge-intake"` before the closing bracket. Final state of that array (illustrative — preserve existing entries):

```toml
[workspace]
members = [
    "crates/surge-acp",
    "crates/surge-cli",
    "crates/surge-core",
    "crates/surge-daemon",
    "crates/surge-git",
    "crates/surge-intake",
    "crates/surge-mcp",
    "crates/surge-notify",
    "crates/surge-orchestrator",
    "crates/surge-persistence",
    "crates/surge-spec",
    "crates/surge-ui",
]
```

- [ ] **Step 5: Verify build**

Run:

```bash
cargo build -p surge-intake
```

Expected: success, no errors. (The crate compiles because every module is empty.)

- [ ] **Step 6: Verify clippy clean**

Run:

```bash
cargo clippy -p surge-intake --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/surge-intake/
git commit -m "feat(intake): scaffold surge-intake crate

Adds empty crate to workspace with module skeleton:
types, source, dedup, candidates, router, testing, error.

Part of RFC-0010 (issue-tracker integration), Plan A."
```

---

## Task 1.1 — `TaskId` newtype with serde + property tests

**Files:**
- Modify: `crates/surge-intake/src/types.rs`
- Test: `crates/surge-intake/src/types.rs` (in-file `#[cfg(test)]` mod)

- [ ] **Step 1: Write the failing test**

Open `crates/surge-intake/src/types.rs` and write:

```rust
//! Shared types for `surge-intake`.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifier of an external ticket, formatted as `provider:scope#id`.
///
/// Examples:
/// - `"github_issues:user/repo#1234"`
/// - `"linear:wsp_acme/ABC-42"`
///
/// `TaskId` is opaque; the only operation supported is creation from a string
/// (via `try_new`) and serialization. Provider-specific parsing belongs to
/// the implementation, not to this type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    pub fn try_new(s: impl Into<String>) -> Result<Self, String> {
        let s = s.into();
        if s.is_empty() {
            return Err("task id must not be empty".into());
        }
        if !s.contains(':') {
            return Err(format!("task id must contain provider prefix: {s}"));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        assert!(TaskId::try_new("").is_err());
    }

    #[test]
    fn rejects_no_provider_prefix() {
        assert!(TaskId::try_new("just-a-string").is_err());
    }

    #[test]
    fn accepts_valid() {
        let id = TaskId::try_new("github_issues:user/repo#1234").unwrap();
        assert_eq!(id.as_str(), "github_issues:user/repo#1234");
    }

    #[test]
    fn round_trip_serde_json() {
        let id = TaskId::try_new("linear:wsp_acme/ABC-42").unwrap();
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(s, "\"linear:wsp_acme/ABC-42\"");
        let back: TaskId = serde_json::from_str(&s).unwrap();
        assert_eq!(back, id);
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run:

```bash
cargo test -p surge-intake --lib types::tests
```

Expected: 4 passed.

- [ ] **Step 3: Add proptest for round-trip stability**

Append to `crates/surge-intake/src/types.rs`:

```rust
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn round_trip(provider in "[a-z_]{3,15}", scope in "[a-zA-Z0-9_/-]{1,40}", num in 0u32..1_000_000) {
            let raw = format!("{provider}:{scope}#{num}");
            let id = TaskId::try_new(&raw).unwrap();
            let s = serde_json::to_string(&id).unwrap();
            let back: TaskId = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(back, id);
        }
    }
}
```

- [ ] **Step 4: Run proptest**

Run:

```bash
cargo test -p surge-intake --lib types::proptests
```

Expected: pass (default 256 cases).

- [ ] **Step 5: Commit**

```bash
git add crates/surge-intake/src/types.rs
git commit -m "feat(intake): add TaskId newtype with serde + proptest"
```

---

## Task 1.2 — `Priority`, `TriageDecision`, `Tier1Decision` enums

**Files:**
- Modify: `crates/surge-intake/src/types.rs`

- [ ] **Step 1: Write the failing tests for Priority**

Append to `crates/surge-intake/src/types.rs`:

```rust
/// Priority levels assigned by Triage Author from ticket text and labels.
///
/// Ordering reflects scheduling precedence: `Urgent > High > Medium > Low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Medium,
    High,
    Urgent,
}

impl Priority {
    /// Stable string label, used in tracker labels (`surge-priority/<level>`).
    pub fn label(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
}

/// Triage Author's verdict on whether a ticket should enter the bootstrap pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum TriageDecision {
    Enqueued {
        priority: Priority,
        reasoning: String,
        summary: String,
    },
    Duplicate {
        of: TaskId,
        reasoning: String,
    },
    OutOfScope {
        reasoning: String,
    },
    Unclear {
        question: String,
    },
}

/// Output of Tier-1 (computational) dedup pre-filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1Decision {
    /// New ticket: pass to Triage Author.
    Pass,
    /// Already an active run for this exact ticket; skip the LLM stage entirely.
    EarlyDuplicate { run_id: String },
}
```

Add tests:

```rust
#[cfg(test)]
mod priority_tests {
    use super::*;

    #[test]
    fn priority_ordering() {
        assert!(Priority::Urgent > Priority::High);
        assert!(Priority::High > Priority::Medium);
        assert!(Priority::Medium > Priority::Low);
    }

    #[test]
    fn priority_label_is_stable() {
        assert_eq!(Priority::Urgent.label(), "urgent");
        assert_eq!(Priority::Low.label(), "low");
    }

    #[test]
    fn priority_serializes_as_lowercase() {
        let s = serde_json::to_string(&Priority::High).unwrap();
        assert_eq!(s, "\"high\"");
    }
}

#[cfg(test)]
mod triage_decision_tests {
    use super::*;

    #[test]
    fn enqueued_round_trip() {
        let d = TriageDecision::Enqueued {
            priority: Priority::High,
            reasoning: "production crash".into(),
            summary: "Fix panic".into(),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: TriageDecision = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn duplicate_round_trip() {
        let d = TriageDecision::Duplicate {
            of: TaskId::try_new("github_issues:user/repo#1").unwrap(),
            reasoning: "same parser path".into(),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: TriageDecision = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p surge-intake --lib priority_tests triage_decision_tests
```

Expected: 5 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-intake/src/types.rs
git commit -m "feat(intake): add Priority, TriageDecision, Tier1Decision enums"
```

---

## Task 1.3 — `TaskEvent`, `TaskEventKind`, `TaskDetails`, `TaskSummary`

**Files:**
- Modify: `crates/surge-intake/src/types.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/surge-intake/src/types.rs`:

```rust
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TaskEventKind {
    NewTask,
    StatusChanged { from: String, to: String },
    LabelsChanged {
        added: Vec<String>,
        removed: Vec<String>,
    },
    TaskClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub source_id: String,
    pub task_id: TaskId,
    pub kind: TaskEventKind,
    pub seen_at: DateTime<Utc>,
    pub raw_payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDetails {
    pub task_id: TaskId,
    pub source_id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub labels: Vec<String>,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub assignee: Option<String>,
    pub raw_payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    pub task_id: TaskId,
    pub title: String,
    pub status: String,
    pub url: String,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod event_tests {
    use super::*;

    fn sample_task_id() -> TaskId {
        TaskId::try_new("github_issues:user/repo#1234").unwrap()
    }

    #[test]
    fn event_round_trip_new_task() {
        let ev = TaskEvent {
            source_id: "github_issues:user/repo".into(),
            task_id: sample_task_id(),
            kind: TaskEventKind::NewTask,
            seen_at: DateTime::parse_from_rfc3339("2026-05-06T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            raw_payload: serde_json::json!({"id": 1234}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: TaskEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn event_round_trip_labels_changed() {
        let ev = TaskEvent {
            source_id: "linear:wsp1".into(),
            task_id: TaskId::try_new("linear:wsp1/ABC-42").unwrap(),
            kind: TaskEventKind::LabelsChanged {
                added: vec!["surge:enabled".into()],
                removed: vec![],
            },
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: TaskEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn details_round_trip() {
        let d = TaskDetails {
            task_id: sample_task_id(),
            source_id: "github_issues:user/repo".into(),
            title: "Fix parser panic".into(),
            description: "Stack overflow on deep nesting".into(),
            status: "open".into(),
            labels: vec!["surge:enabled".into(), "priority/high".into()],
            url: "https://github.com/user/repo/issues/1234".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            assignee: None,
            raw_payload: serde_json::json!({}),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: TaskDetails = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p surge-intake --lib event_tests
```

Expected: 3 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-intake/src/types.rs
git commit -m "feat(intake): add TaskEvent, TaskDetails, TaskSummary types"
```

---

## Task 1.4 — `trait TaskSource` definition

**Files:**
- Modify: `crates/surge-intake/src/source.rs`

- [ ] **Step 1: Write the trait + minimal compile-check test**

Open `crates/surge-intake/src/source.rs` and write:

```rust
//! `TaskSource` trait — the contract every task-source adapter implements.
//!
//! Implementations live in this crate (Linear, GitHub Issues) or in
//! downstream adapter crates (`surge-intake-discord`, etc.).

use crate::types::{TaskDetails, TaskEvent, TaskId, TaskSummary};
use crate::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;

/// Adapter to an external task tracker (Linear, GitHub Issues, future Discord/Jira/...).
///
/// `TaskSource` exposes only the operations that respect the
/// **tracker is master** authority model: read tickets, write comments,
/// set labels. Status changes / assignments are intentionally absent.
#[async_trait]
pub trait TaskSource: Send + Sync {
    /// Stable identifier (e.g. `"linear:wsp_acme"`). Used as foreign key in storage.
    fn id(&self) -> &str;

    /// Human-readable name (shown in inbox cards, logs).
    fn display_name(&self) -> &str;

    /// Provider type tag (`"linear"`, `"github_issues"`, ...).
    fn provider(&self) -> &'static str;

    /// Stream of incoming task events. Implementations may use polling,
    /// long-poll, or webhook delivery — the consumer doesn't care.
    fn watch_for_tasks<'a>(&'a self) -> BoxStream<'a, Result<TaskEvent>>;

    /// Fetch full details of a single task on demand (used by Triage Author).
    async fn fetch_task(&self, id: &TaskId) -> Result<TaskDetails>;

    /// List currently open tasks (bounded — provider-specific cap).
    /// Used to assemble Triage Author's candidate set.
    async fn list_open_tasks(&self) -> Result<Vec<TaskSummary>>;

    /// Mark that we've seen and started processing a task. Idempotent.
    /// (Used to gate retries; storage-side only, no provider call required.)
    async fn acknowledge_task(&self, id: &TaskId) -> Result<()>;

    /// Post a comment on the task. Idempotency is the implementation's
    /// responsibility (Linear has idempotency keys; GitHub does not — use
    /// telltale-prefix detection there).
    async fn post_comment(&self, id: &TaskId, body: &str) -> Result<()>;

    /// Set or remove a label on the task. Natively idempotent.
    async fn set_label(&self, id: &TaskId, label: &str, present: bool) -> Result<()>;

    /// Read the current labels on the task.
    async fn read_labels(&self, id: &TaskId) -> Result<Vec<String>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-only check: trait is object-safe (`dyn TaskSource`).
    #[allow(dead_code)]
    fn assert_object_safe(_: Box<dyn TaskSource>) {}
}
```

- [ ] **Step 2: Verify build**

```bash
cargo build -p surge-intake
```

Expected: success.

- [ ] **Step 3: Run trait test**

```bash
cargo test -p surge-intake --lib source::tests
```

Expected: 0 ran, 0 passed (assert_object_safe is a compile-only check).

- [ ] **Step 4: Commit**

```bash
git add crates/surge-intake/src/source.rs
git commit -m "feat(intake): define trait TaskSource"
```

---

## Task 1.5 — `MockTaskSource` for unit tests

**Files:**
- Modify: `crates/surge-intake/src/testing.rs`

- [ ] **Step 1: Write the mock implementation**

Open `crates/surge-intake/src/testing.rs` and write:

```rust
//! Test utilities. Always available (not gated behind a feature) so that
//! consumer crates can use `MockTaskSource` in their integration tests.

use crate::source::TaskSource;
use crate::types::{TaskDetails, TaskEvent, TaskId, TaskSummary};
use crate::{Error, Result};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use std::collections::HashMap;
use tokio::sync::Mutex;

/// In-memory `TaskSource` for tests.
///
/// Push events with `push_event`, then assert outcomes via the inspection
/// methods (`posted_comments`, `set_labels`).
pub struct MockTaskSource {
    id: String,
    display_name: String,
    provider: &'static str,
    events: Mutex<Vec<TaskEvent>>,
    open_tasks: Mutex<HashMap<TaskId, TaskDetails>>,
    posted_comments: Mutex<Vec<(TaskId, String)>>,
    set_labels: Mutex<Vec<(TaskId, String, bool)>>,
    fail_post_comment: Mutex<bool>,
}

impl MockTaskSource {
    pub fn new(id: impl Into<String>, provider: &'static str) -> Self {
        let id = id.into();
        Self {
            display_name: format!("Mock · {id}"),
            id,
            provider,
            events: Mutex::new(Vec::new()),
            open_tasks: Mutex::new(HashMap::new()),
            posted_comments: Mutex::new(Vec::new()),
            set_labels: Mutex::new(Vec::new()),
            fail_post_comment: Mutex::new(false),
        }
    }

    pub async fn push_event(&self, ev: TaskEvent) {
        self.events.lock().await.push(ev);
    }

    pub async fn put_task(&self, details: TaskDetails) {
        self.open_tasks.lock().await.insert(details.task_id.clone(), details);
    }

    pub async fn posted_comments(&self) -> Vec<(TaskId, String)> {
        self.posted_comments.lock().await.clone()
    }

    pub async fn set_labels(&self) -> Vec<(TaskId, String, bool)> {
        self.set_labels.lock().await.clone()
    }

    pub async fn arm_post_comment_failure(&self) {
        *self.fail_post_comment.lock().await = true;
    }
}

#[async_trait]
impl TaskSource for MockTaskSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn provider(&self) -> &'static str {
        self.provider
    }

    fn watch_for_tasks<'a>(&'a self) -> BoxStream<'a, Result<TaskEvent>> {
        let events_handle = self.events.clone();
        Box::pin(async_stream_inner(events_handle))
    }

    async fn fetch_task(&self, id: &TaskId) -> Result<TaskDetails> {
        self.open_tasks
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Internal(format!("task not found: {id}")))
    }

    async fn list_open_tasks(&self) -> Result<Vec<TaskSummary>> {
        let map = self.open_tasks.lock().await;
        Ok(map
            .values()
            .map(|d| TaskSummary {
                task_id: d.task_id.clone(),
                title: d.title.clone(),
                status: d.status.clone(),
                url: d.url.clone(),
                updated_at: d.updated_at,
            })
            .collect())
    }

    async fn acknowledge_task(&self, _id: &TaskId) -> Result<()> {
        Ok(())
    }

    async fn post_comment(&self, id: &TaskId, body: &str) -> Result<()> {
        if *self.fail_post_comment.lock().await {
            return Err(Error::Network("simulated post_comment failure".into()));
        }
        self.posted_comments
            .lock()
            .await
            .push((id.clone(), body.to_string()));
        Ok(())
    }

    async fn set_label(&self, id: &TaskId, label: &str, present: bool) -> Result<()> {
        self.set_labels
            .lock()
            .await
            .push((id.clone(), label.to_string(), present));
        Ok(())
    }

    async fn read_labels(&self, id: &TaskId) -> Result<Vec<String>> {
        Ok(self
            .open_tasks
            .lock()
            .await
            .get(id)
            .map(|d| d.labels.clone())
            .unwrap_or_default())
    }
}

// Drains queued events as a stream. Once the queue is empty the stream yields
// pending forever (mirrors a long-running polling source). Use `tokio::time::timeout`
// in tests to bound test duration.
fn async_stream_inner(
    events: tokio::sync::Mutex<Vec<TaskEvent>>,
) -> impl futures::Stream<Item = Result<TaskEvent>> + Send {
    // Implementation note: we capture `events` by value; access via locking.
    stream::unfold(events, |events| async move {
        let next = {
            let mut guard = events.lock().await;
            if guard.is_empty() {
                None
            } else {
                Some(guard.remove(0))
            }
        };
        match next {
            Some(ev) => Some((Ok(ev), events)),
            None => {
                // Idle: yield no more for now (terminate stream). Tests that need
                // an open-ended stream should call `push_event` before consuming.
                None
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskEventKind;
    use chrono::Utc;
    use futures::StreamExt;

    fn sample_event() -> TaskEvent {
        TaskEvent {
            source_id: "mock:test".into(),
            task_id: TaskId::try_new("mock:test#1").unwrap(),
            kind: TaskEventKind::NewTask,
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn mock_emits_pushed_events() {
        let src = MockTaskSource::new("mock:test", "mock");
        src.push_event(sample_event()).await;
        let mut stream = src.watch_for_tasks();
        let first = stream.next().await.unwrap().unwrap();
        assert!(matches!(first.kind, TaskEventKind::NewTask));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn mock_records_posted_comments() {
        let src = MockTaskSource::new("mock:test", "mock");
        let id = TaskId::try_new("mock:test#1").unwrap();
        src.post_comment(&id, "hello").await.unwrap();
        let comments = src.posted_comments().await;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].1, "hello");
    }

    #[tokio::test]
    async fn mock_post_comment_failure() {
        let src = MockTaskSource::new("mock:test", "mock");
        src.arm_post_comment_failure().await;
        let id = TaskId::try_new("mock:test#1").unwrap();
        let err = src.post_comment(&id, "x").await.unwrap_err();
        assert!(matches!(err, Error::Network(_)));
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p surge-intake --lib testing::tests
```

Expected: 3 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-intake/src/testing.rs
git commit -m "feat(intake): add MockTaskSource for unit tests"
```

---

## Task 2.1 — Migration `0007_ticket_index.sql`

**Files:**
- Create: `crates/surge-persistence/migrations/0007_ticket_index.sql`

- [ ] **Step 1: Locate the migrations directory**

Verify with:

```bash
ls crates/surge-persistence/migrations/ 2>/dev/null
```

If the directory does not exist:

```bash
mkdir -p crates/surge-persistence/migrations
```

Determine the next migration number:

```bash
ls crates/surge-persistence/migrations/ 2>/dev/null | sort | tail -3
```

If the last existing is `0006_*.sql`, the new file is `0007_*.sql`. If the highest existing is different, increment by 1 and use that. The plan assumes 0007; adjust if your tree has additional migrations.

- [ ] **Step 2: Write the migration**

Create `crates/surge-persistence/migrations/0007_ticket_index.sql`:

```sql
-- 0007_ticket_index.sql
-- Tracks lifecycle of external tickets ingested via surge-intake.
-- See docs/revision/rfcs/0010-issue-tracker-integration.md, decision #22.

CREATE TABLE IF NOT EXISTS ticket_index (
    task_id          TEXT PRIMARY KEY,
    source_id        TEXT NOT NULL,
    provider         TEXT NOT NULL,
    run_id           TEXT,
    triage_decision  TEXT,
    duplicate_of     TEXT,
    priority         TEXT,
    state            TEXT NOT NULL,
    first_seen       TEXT NOT NULL,
    last_seen        TEXT NOT NULL,
    snooze_until     TEXT,

    FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE SET NULL,
    FOREIGN KEY (duplicate_of) REFERENCES ticket_index(task_id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_ticket_index_source ON ticket_index(source_id);
CREATE INDEX IF NOT EXISTS idx_ticket_index_run    ON ticket_index(run_id);
CREATE INDEX IF NOT EXISTS idx_ticket_index_state  ON ticket_index(state);
```

- [ ] **Step 3: Verify migration runner picks it up**

`surge-persistence` runs migrations on startup. Find the runner:

```bash
grep -rn "migrations" crates/surge-persistence/src/ | head -10
```

Likely there is a function that reads the directory, sorts files, and applies them. Run the persistence test suite:

```bash
cargo test -p surge-persistence --lib
```

Expected: existing tests pass; no errors about the new file.

If the runner uses a static include (e.g. `include_str!`), the new file must be added to the runner's array. If so, edit the runner accordingly. Likely path: `crates/surge-persistence/src/store.rs` or `crates/surge-persistence/src/lib.rs`.

For example, if you find:

```rust
const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init.sql", include_str!("../migrations/0001_init.sql")),
    ("0006_runs.sql", include_str!("../migrations/0006_runs.sql")),
];
```

…add:

```rust
    ("0007_ticket_index.sql", include_str!("../migrations/0007_ticket_index.sql")),
```

- [ ] **Step 4: Add SQL syntax test**

Create `crates/surge-persistence/tests/migration_0007.rs`:

```rust
//! Smoke test that 0007_ticket_index.sql applies cleanly on a fresh DB
//! and creates the expected schema.

use rusqlite::Connection;

#[test]
fn migration_0007_creates_ticket_index_table() {
    let conn = Connection::open_in_memory().unwrap();
    // 0007 depends on `runs` table for FK; use minimal stub.
    conn.execute_batch(
        "CREATE TABLE runs (id TEXT PRIMARY KEY);",
    )
    .unwrap();

    let sql = include_str!("../migrations/0007_ticket_index.sql");
    conn.execute_batch(sql).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ticket_index'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Verify indexes exist.
    let idx_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='ticket_index' AND name LIKE 'idx_ticket_index_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(idx_count, 3);
}
```

- [ ] **Step 5: Run the test**

```bash
cargo test -p surge-persistence --test migration_0007
```

Expected: 1 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-persistence/migrations/0007_ticket_index.sql crates/surge-persistence/tests/migration_0007.rs
# also stage the runner edit if you changed src/store.rs
git commit -m "feat(persistence): add ticket_index table (migration 0007)"
```

---

## Task 2.2 — Migration `0008_task_source_state.sql`

**Files:**
- Create: `crates/surge-persistence/migrations/0008_task_source_state.sql`

- [ ] **Step 1: Write the migration**

Create `crates/surge-persistence/migrations/0008_task_source_state.sql`:

```sql
-- 0008_task_source_state.sql
-- Per-source polling cursor + failure counter used by surge-intake's TaskRouter.

CREATE TABLE IF NOT EXISTS task_source_state (
    source_id            TEXT PRIMARY KEY,
    last_seen_cursor     TEXT,
    last_poll_at         TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0
);
```

- [ ] **Step 2: Add to migration runner**

If the runner uses `include_str!`, add the entry mirroring Task 2.1.

- [ ] **Step 3: Add SQL syntax test**

Create `crates/surge-persistence/tests/migration_0008.rs`:

```rust
use rusqlite::Connection;

#[test]
fn migration_0008_creates_task_source_state_table() {
    let conn = Connection::open_in_memory().unwrap();
    let sql = include_str!("../migrations/0008_task_source_state.sql");
    conn.execute_batch(sql).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_source_state'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Insert + read back round trip.
    conn.execute(
        "INSERT INTO task_source_state(source_id, last_seen_cursor, last_poll_at, consecutive_failures) VALUES (?,?,?,?)",
        rusqlite::params!["linear:wsp1", "cursor_42", "2026-05-06T10:00:00Z", 0_i64],
    )
    .unwrap();

    let cursor: String = conn
        .query_row(
            "SELECT last_seen_cursor FROM task_source_state WHERE source_id = ?",
            ["linear:wsp1"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cursor, "cursor_42");
}
```

- [ ] **Step 4: Run the test**

```bash
cargo test -p surge-persistence --test migration_0008
```

Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/migrations/0008_task_source_state.sql crates/surge-persistence/tests/migration_0008.rs
git commit -m "feat(persistence): add task_source_state table (migration 0008)"
```

---

## Task 2.3 — `TicketState` enum + `IntakeRow` model

**Files:**
- Create: `crates/surge-persistence/src/intake.rs`
- Modify: `crates/surge-persistence/src/lib.rs` (re-export `intake`)

- [ ] **Step 1: Write the failing test**

Create `crates/surge-persistence/src/intake.rs`:

```rust
//! Storage layer for `surge-intake`'s `ticket_index` and `task_source_state` tables.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TicketState {
    Seen,
    Tier1Dup,
    Triaged,
    TriagedDup,
    TriagedOOS,
    TriagedUnclear,
    InboxNotified,
    Snoozed,
    Skipped,
    RunStarted,
    Active,
    Completed,
    Failed,
    Aborted,
    Stale,
    TriageStale,
}

impl TicketState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Seen => "Seen",
            Self::Tier1Dup => "Tier1Dup",
            Self::Triaged => "Triaged",
            Self::TriagedDup => "TriagedDup",
            Self::TriagedOOS => "TriagedOOS",
            Self::TriagedUnclear => "TriagedUnclear",
            Self::InboxNotified => "InboxNotified",
            Self::Snoozed => "Snoozed",
            Self::Skipped => "Skipped",
            Self::RunStarted => "RunStarted",
            Self::Active => "Active",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Aborted => "Aborted",
            Self::Stale => "Stale",
            Self::TriageStale => "TriageStale",
        }
    }
}

impl FromStr for TicketState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Seen" => Ok(Self::Seen),
            "Tier1Dup" => Ok(Self::Tier1Dup),
            "Triaged" => Ok(Self::Triaged),
            "TriagedDup" => Ok(Self::TriagedDup),
            "TriagedOOS" => Ok(Self::TriagedOOS),
            "TriagedUnclear" => Ok(Self::TriagedUnclear),
            "InboxNotified" => Ok(Self::InboxNotified),
            "Snoozed" => Ok(Self::Snoozed),
            "Skipped" => Ok(Self::Skipped),
            "RunStarted" => Ok(Self::RunStarted),
            "Active" => Ok(Self::Active),
            "Completed" => Ok(Self::Completed),
            "Failed" => Ok(Self::Failed),
            "Aborted" => Ok(Self::Aborted),
            "Stale" => Ok(Self::Stale),
            "TriageStale" => Ok(Self::TriageStale),
            other => Err(format!("unknown TicketState: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakeRow {
    pub task_id: String,
    pub source_id: String,
    pub provider: String,
    pub run_id: Option<String>,
    pub triage_decision: Option<String>,
    pub duplicate_of: Option<String>,
    pub priority: Option<String>,
    pub state: TicketState,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub snooze_until: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_state_round_trip() {
        for s in [
            TicketState::Seen,
            TicketState::Triaged,
            TicketState::Active,
            TicketState::Completed,
            TicketState::TriageStale,
        ] {
            let str_form = s.as_str();
            let back: TicketState = str_form.parse().unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn ticket_state_unknown_errors() {
        let err = TicketState::from_str("Garbage").unwrap_err();
        assert!(err.contains("Garbage"));
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p surge-persistence --lib intake::tests
```

Expected: 2 passed.

- [ ] **Step 3: Re-export in `lib.rs`**

Add to `crates/surge-persistence/src/lib.rs`:

```rust
pub mod intake;
```

(Add this near other `pub mod` declarations.)

- [ ] **Step 4: Verify build**

```bash
cargo build -p surge-persistence
cargo build -p surge-intake
```

Expected: both succeed.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/intake.rs crates/surge-persistence/src/lib.rs
git commit -m "feat(persistence): add TicketState enum + IntakeRow model"
```

---

## Task 2.4 — `IntakeRepo` accessor (insert, update_state, lookup_active)

**Files:**
- Modify: `crates/surge-persistence/src/intake.rs`
- Modify: `crates/surge-intake/src/error.rs` (tighten `From` impl)

- [ ] **Step 1: Write the failing test for insert + read**

Append to `crates/surge-persistence/src/intake.rs`:

```rust
pub struct IntakeRepo<'a> {
    conn: &'a Connection,
}

impl<'a> IntakeRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, row: &IntakeRow) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO ticket_index(\
                task_id, source_id, provider, run_id, triage_decision, duplicate_of,\
                priority, state, first_seen, last_seen, snooze_until\
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                row.task_id,
                row.source_id,
                row.provider,
                row.run_id,
                row.triage_decision,
                row.duplicate_of,
                row.priority,
                row.state.as_str(),
                row.first_seen.to_rfc3339(),
                row.last_seen.to_rfc3339(),
                row.snooze_until.map(|d| d.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn upsert_last_seen(
        &self,
        task_id: &str,
        last_seen: DateTime<Utc>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET last_seen = ?1 WHERE task_id = ?2",
            params![last_seen.to_rfc3339(), task_id],
        )?;
        Ok(())
    }

    pub fn update_state(
        &self,
        task_id: &str,
        state: TicketState,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE ticket_index SET state = ?1 WHERE task_id = ?2",
            params![state.as_str(), task_id],
        )?;
        Ok(())
    }

    pub fn fetch(&self, task_id: &str) -> rusqlite::Result<Option<IntakeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, source_id, provider, run_id, triage_decision, duplicate_of,\
                    priority, state, first_seen, last_seen, snooze_until\
             FROM ticket_index WHERE task_id = ?1",
        )?;
        let mut rows = stmt.query(params![task_id])?;
        if let Some(r) = rows.next()? {
            let state_str: String = r.get(7)?;
            let state: TicketState = state_str
                .parse()
                .map_err(|e: String| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, e.into()))?;
            let first_seen: String = r.get(8)?;
            let last_seen: String = r.get(9)?;
            let snooze_until: Option<String> = r.get(10)?;
            Ok(Some(IntakeRow {
                task_id: r.get(0)?,
                source_id: r.get(1)?,
                provider: r.get(2)?,
                run_id: r.get(3)?,
                triage_decision: r.get(4)?,
                duplicate_of: r.get(5)?,
                priority: r.get(6)?,
                state,
                first_seen: DateTime::parse_from_rfc3339(&first_seen)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, e.to_string().into()))?
                    .with_timezone(&Utc),
                last_seen: DateTime::parse_from_rfc3339(&last_seen)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, e.to_string().into()))?
                    .with_timezone(&Utc),
                snooze_until: snooze_until
                    .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
                    .transpose()
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, e.to_string().into()))?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Returns the run_id of an active duplicate run for the given task,
    /// if one exists. Used by Tier-1 PreFilter.
    pub fn lookup_active_run(&self, task_id: &str) -> rusqlite::Result<Option<String>> {
        let row = self.conn.query_row(
            "SELECT run_id FROM ticket_index \
             WHERE task_id = ?1 \
               AND run_id IS NOT NULL \
               AND state NOT IN ('Completed','Aborted','Skipped','Stale','TriagedDup','TriagedOOS')",
            params![task_id],
            |r| r.get::<_, Option<String>>(0),
        );
        match row {
            Ok(opt) => Ok(opt),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod repo_tests {
    use super::*;
    use rusqlite::Connection;

    fn db_with_schema() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);").unwrap();
        let sql = include_str!("../migrations/0007_ticket_index.sql");
        conn.execute_batch(sql).unwrap();
        conn
    }

    fn sample_row(task_id: &str, state: TicketState) -> IntakeRow {
        IntakeRow {
            task_id: task_id.into(),
            source_id: "linear:wsp1".into(),
            provider: "linear".into(),
            run_id: None,
            triage_decision: None,
            duplicate_of: None,
            priority: None,
            state,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            snooze_until: None,
        }
    }

    #[test]
    fn insert_then_fetch_roundtrip() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        let row = sample_row("linear:wsp1/ABC-1", TicketState::Seen);
        repo.insert(&row).unwrap();
        let fetched = repo.fetch("linear:wsp1/ABC-1").unwrap().unwrap();
        assert_eq!(fetched.state, TicketState::Seen);
        assert_eq!(fetched.task_id, "linear:wsp1/ABC-1");
    }

    #[test]
    fn lookup_active_run_returns_none_when_no_run_id() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/ABC-2", TicketState::Seen)).unwrap();
        let res = repo.lookup_active_run("linear:wsp1/ABC-2").unwrap();
        assert_eq!(res, None);
    }

    #[test]
    fn lookup_active_run_returns_run_id_when_active() {
        let conn = db_with_schema();
        conn.execute("INSERT INTO runs(id) VALUES ('run_abc')", []).unwrap();
        let repo = IntakeRepo::new(&conn);
        let mut row = sample_row("linear:wsp1/ABC-3", TicketState::Active);
        row.run_id = Some("run_abc".into());
        repo.insert(&row).unwrap();
        let res = repo.lookup_active_run("linear:wsp1/ABC-3").unwrap();
        assert_eq!(res, Some("run_abc".into()));
    }

    #[test]
    fn lookup_active_run_excludes_completed() {
        let conn = db_with_schema();
        conn.execute("INSERT INTO runs(id) VALUES ('run_done')", []).unwrap();
        let repo = IntakeRepo::new(&conn);
        let mut row = sample_row("linear:wsp1/ABC-4", TicketState::Completed);
        row.run_id = Some("run_done".into());
        repo.insert(&row).unwrap();
        let res = repo.lookup_active_run("linear:wsp1/ABC-4").unwrap();
        assert_eq!(res, None);
    }

    #[test]
    fn update_state_changes_row() {
        let conn = db_with_schema();
        let repo = IntakeRepo::new(&conn);
        repo.insert(&sample_row("linear:wsp1/ABC-5", TicketState::Seen)).unwrap();
        repo.update_state("linear:wsp1/ABC-5", TicketState::Triaged).unwrap();
        let fetched = repo.fetch("linear:wsp1/ABC-5").unwrap().unwrap();
        assert_eq!(fetched.state, TicketState::Triaged);
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p surge-persistence --lib intake::repo_tests
```

Expected: 5 passed.

- [ ] **Step 3: Tighten `Error` in `surge-intake`**

Now that `surge-persistence::Error` is reachable, replace the placeholder in `crates/surge-intake/src/error.rs`:

```rust
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("invalid task id: {0}")]
    InvalidTaskId(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    #[error("internal: {0}")]
    Internal(String),
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.to_string())
    }
}
```

(`surge-persistence` exposes a fuller `Error` that we may map later. For Plan A we accept `String` as the inner storage message.)

- [ ] **Step 4: Verify whole tree builds**

```bash
cargo build --workspace
```

Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-persistence/src/intake.rs crates/surge-intake/src/error.rs
git commit -m "feat(persistence): add IntakeRepo (insert, fetch, update_state, lookup_active_run)"
```

---

## Task 3.1 — `Tier1PreFilter` with `lookup_active_run`

**Files:**
- Modify: `crates/surge-intake/src/dedup.rs`

- [ ] **Step 1: Write the implementation**

Open `crates/surge-intake/src/dedup.rs`:

```rust
//! Tier-1 PreFilter: computational deduplication. No LLM, no network.
//!
//! MVP step: active-run lookup against `ticket_index`. Other steps
//! (embedding similarity for B/C in RFC-0010) are deferred to RFC-0014.

use crate::types::{TaskEvent, Tier1Decision};
use crate::{Error, Result};
use rusqlite::Connection;
use surge_persistence::intake::IntakeRepo;
use tracing::trace;

pub struct Tier1PreFilter<'a> {
    conn: &'a Connection,
}

impl<'a> Tier1PreFilter<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Decide whether the event represents a duplicate of an active run.
    pub fn check(&self, event: &TaskEvent) -> Result<Tier1Decision> {
        let repo = IntakeRepo::new(self.conn);
        let task_id = event.task_id.as_str();

        match repo.lookup_active_run(task_id) {
            Ok(Some(run_id)) => {
                trace!(?task_id, %run_id, "tier1 early-duplicate hit");
                Ok(Tier1Decision::EarlyDuplicate { run_id })
            }
            Ok(None) => {
                trace!(?task_id, "tier1 pass");
                Ok(Tier1Decision::Pass)
            }
            Err(e) => Err(Error::Storage(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TaskEventKind, TaskId};
    use chrono::Utc;
    use rusqlite::Connection;
    use surge_persistence::intake::{IntakeRow, TicketState};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);").unwrap();
        let sql = include_str!("../../surge-persistence/migrations/0007_ticket_index.sql");
        conn.execute_batch(sql).unwrap();
        conn
    }

    fn sample_event(task_id: &str) -> TaskEvent {
        TaskEvent {
            source_id: "linear:wsp1".into(),
            task_id: TaskId::try_new(task_id).unwrap(),
            kind: TaskEventKind::NewTask,
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        }
    }

    fn sample_row(task_id: &str, run_id: Option<&str>, state: TicketState) -> IntakeRow {
        IntakeRow {
            task_id: task_id.into(),
            source_id: "linear:wsp1".into(),
            provider: "linear".into(),
            run_id: run_id.map(String::from),
            triage_decision: None,
            duplicate_of: None,
            priority: None,
            state,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            snooze_until: None,
        }
    }

    #[test]
    fn pass_when_no_existing_row() {
        let conn = db();
        let f = Tier1PreFilter::new(&conn);
        let dec = f.check(&sample_event("linear:wsp1/A-1")).unwrap();
        assert_eq!(dec, Tier1Decision::Pass);
    }

    #[test]
    fn early_duplicate_when_active_run_exists() {
        let conn = db();
        conn.execute("INSERT INTO runs(id) VALUES ('run_x')", []).unwrap();
        IntakeRepo::new(&conn)
            .insert(&sample_row("linear:wsp1/A-2", Some("run_x"), TicketState::Active))
            .unwrap();
        let f = Tier1PreFilter::new(&conn);
        let dec = f.check(&sample_event("linear:wsp1/A-2")).unwrap();
        assert_eq!(
            dec,
            Tier1Decision::EarlyDuplicate {
                run_id: "run_x".into()
            }
        );
    }

    #[test]
    fn pass_when_existing_run_completed() {
        let conn = db();
        conn.execute("INSERT INTO runs(id) VALUES ('run_done')", []).unwrap();
        IntakeRepo::new(&conn)
            .insert(&sample_row("linear:wsp1/A-3", Some("run_done"), TicketState::Completed))
            .unwrap();
        let f = Tier1PreFilter::new(&conn);
        let dec = f.check(&sample_event("linear:wsp1/A-3")).unwrap();
        assert_eq!(dec, Tier1Decision::Pass);
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p surge-intake --lib dedup::tests
```

Expected: 3 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-intake/src/dedup.rs
git commit -m "feat(intake): add Tier1PreFilter (active-run lookup)"
```

---

## Task 3.2 — `candidates` module: keyword-overlap selection

**Files:**
- Modify: `crates/surge-intake/src/candidates.rs`

- [ ] **Step 1: Write the implementation**

Open `crates/surge-intake/src/candidates.rs`:

```rust
//! Computational selection of candidate tickets to feed Triage Author.
//!
//! In MVP we use Jaccard similarity over title+description tokens. RFC-0014
//! replaces this with embedding-based selection.

use crate::types::{TaskDetails, TaskSummary};
use std::collections::HashSet;

/// Return the top-`limit` items from `candidates` by Jaccard similarity to `target`.
///
/// Title and description of each item are tokenised (lowercased ASCII alphanumeric,
/// length >= 3) and compared.
pub fn top_by_keyword_overlap(
    target: &TaskDetails,
    candidates: &[CandidateInput],
    limit: usize,
) -> Vec<ScoredCandidate> {
    let target_tokens = tokenize(&target.title, &target.description);
    if target_tokens.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<ScoredCandidate> = candidates
        .iter()
        .filter(|c| c.task_id != target.task_id.as_str())
        .map(|c| {
            let tokens = tokenize(&c.title, &c.summary);
            let score = jaccard(&target_tokens, &tokens);
            ScoredCandidate {
                task_id: c.task_id.clone(),
                title: c.title.clone(),
                summary: c.summary.clone(),
                score,
            }
        })
        .filter(|c| c.score > 0.0)
        .collect();

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

fn tokenize(title: &str, body: &str) -> HashSet<String> {
    let combined = format!("{title} {body}");
    combined
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateInput {
    pub task_id: String,
    pub title: String,
    pub summary: String,
}

impl CandidateInput {
    pub fn from_summary(s: &TaskSummary) -> Self {
        Self {
            task_id: s.task_id.as_str().into(),
            title: s.title.clone(),
            summary: String::new(),
        }
    }

    pub fn from_details(d: &TaskDetails) -> Self {
        Self {
            task_id: d.task_id.as_str().into(),
            title: d.title.clone(),
            summary: d.description.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoredCandidate {
    pub task_id: String,
    pub title: String,
    pub summary: String,
    pub score: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskId;
    use chrono::Utc;

    fn details(task_id: &str, title: &str, body: &str) -> TaskDetails {
        TaskDetails {
            task_id: TaskId::try_new(task_id).unwrap(),
            source_id: "test".into(),
            title: title.into(),
            description: body.into(),
            status: "open".into(),
            labels: vec![],
            url: format!("https://example/{task_id}"),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            assignee: None,
            raw_payload: serde_json::json!({}),
        }
    }

    fn cand(task_id: &str, title: &str, summary: &str) -> CandidateInput {
        CandidateInput {
            task_id: task_id.into(),
            title: title.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn top_keeps_only_overlapping() {
        let target = details(
            "github:r#1",
            "Fix parser panic on nested objects",
            "Stack overflow when nesting exceeds 16",
        );
        let cs = vec![
            cand("github:r#2", "Parser crash with deep nesting", "stack overflow on 20+"),
            cand("github:r#3", "Add new logo", "ui design refresh"),
        ];
        let top = top_by_keyword_overlap(&target, &cs, 5);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].task_id, "github:r#2");
        assert!(top[0].score > 0.0);
    }

    #[test]
    fn excludes_self() {
        let target = details("github:r#1", "Fix bug", "important");
        let cs = vec![cand("github:r#1", "Fix bug", "important")];
        let top = top_by_keyword_overlap(&target, &cs, 5);
        assert!(top.is_empty());
    }

    #[test]
    fn respects_limit() {
        let target = details("github:r#1", "deep nesting parser", "stack overflow");
        let cs: Vec<_> = (10..30)
            .map(|i| cand(&format!("github:r#{i}"), "parser nesting stack overflow problem", "deep stack"))
            .collect();
        let top = top_by_keyword_overlap(&target, &cs, 5);
        assert_eq!(top.len(), 5);
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p surge-intake --lib candidates::tests
```

Expected: 3 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-intake/src/candidates.rs
git commit -m "feat(intake): add keyword-overlap candidate selector"
```

---

## Task 4.1 — `TaskRouter` skeleton + multiplexer

**Files:**
- Modify: `crates/surge-intake/src/router.rs`

- [ ] **Step 1: Write the router**

Open `crates/surge-intake/src/router.rs`:

```rust
//! `TaskRouter` — multiplex events from multiple `TaskSource`s into a single
//! channel, applying Tier-1 PreFilter on each event.
//!
//! Lives in `surge-intake` to keep storage + dedup + multiplexing close together.
//! `surge-daemon` instantiates this with the configured sources at startup.

use crate::dedup::Tier1PreFilter;
use crate::source::TaskSource;
use crate::types::{TaskEvent, Tier1Decision};
use crate::{Error, Result};
use futures::stream::{select_all, StreamExt};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

/// Output of the router for downstream consumers (Triage Author dispatcher).
#[derive(Debug)]
pub enum RouterOutput {
    /// Event should be triaged.
    Triage { event: TaskEvent },
    /// Tier-1 dedup said this is an early duplicate; downstream may post a comment.
    EarlyDuplicate { event: TaskEvent, run_id: String },
}

pub struct TaskRouter {
    sources: Vec<Arc<dyn TaskSource>>,
    conn: Arc<Mutex<Connection>>,
    out_tx: mpsc::Sender<RouterOutput>,
}

impl TaskRouter {
    pub fn new(
        sources: Vec<Arc<dyn TaskSource>>,
        conn: Arc<Mutex<Connection>>,
        out_tx: mpsc::Sender<RouterOutput>,
    ) -> Self {
        Self { sources, conn, out_tx }
    }

    /// Drive the router until all source streams finish or the output channel closes.
    /// In production the streams are infinite (polling loops), so this method runs
    /// until the daemon shuts down.
    pub async fn run(self) -> Result<()> {
        let streams = self.sources.iter().map(|s| s.watch_for_tasks()).collect::<Vec<_>>();
        let mut multiplex = select_all(streams);

        while let Some(item) = multiplex.next().await {
            match item {
                Ok(event) => {
                    let decision = {
                        let conn = self.conn.lock().await;
                        let pre = Tier1PreFilter::new(&*conn);
                        pre.check(&event)?
                    };

                    let out = match decision {
                        Tier1Decision::Pass => RouterOutput::Triage { event },
                        Tier1Decision::EarlyDuplicate { run_id } => {
                            RouterOutput::EarlyDuplicate { event, run_id }
                        }
                    };

                    if self.out_tx.send(out).await.is_err() {
                        info!("router output channel closed; stopping");
                        return Ok(());
                    }
                }
                Err(e) => {
                    warn!(error = %e, "task source emitted error; continuing");
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockTaskSource;
    use crate::types::{TaskDetails, TaskEvent, TaskEventKind, TaskId};
    use chrono::Utc;
    use rusqlite::Connection;
    use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);").unwrap();
        let sql = include_str!("../../surge-persistence/migrations/0007_ticket_index.sql");
        conn.execute_batch(sql).unwrap();
        conn
    }

    fn ev(task_id: &str) -> TaskEvent {
        TaskEvent {
            source_id: "mock:t".into(),
            task_id: TaskId::try_new(task_id).unwrap(),
            kind: TaskEventKind::NewTask,
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn passes_through_new_event_as_triage() {
        let conn = Arc::new(Mutex::new(db()));
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(ev("mock:t#1")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src], Arc::clone(&conn), tx);

        let handle = tokio::spawn(router.run());
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;
        let received = received.expect("did not receive within 1s").expect("channel closed");
        match received {
            RouterOutput::Triage { event } => assert_eq!(event.task_id.as_str(), "mock:t#1"),
            other => panic!("expected Triage, got {other:?}"),
        }
        // Drop receiver so router exits cleanly.
        drop(rx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn emits_early_duplicate_when_active_run_exists() {
        let conn = Arc::new(Mutex::new(db()));
        // Pre-seed an active run for task mock:t#9.
        {
            let c = conn.lock().await;
            c.execute("INSERT INTO runs(id) VALUES ('run_active')", []).unwrap();
            IntakeRepo::new(&*c)
                .insert(&IntakeRow {
                    task_id: "mock:t#9".into(),
                    source_id: "mock:t".into(),
                    provider: "mock".into(),
                    run_id: Some("run_active".into()),
                    triage_decision: None,
                    duplicate_of: None,
                    priority: None,
                    state: TicketState::Active,
                    first_seen: Utc::now(),
                    last_seen: Utc::now(),
                    snooze_until: None,
                })
                .unwrap();
        }
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(ev("mock:t#9")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src], Arc::clone(&conn), tx);

        let handle = tokio::spawn(router.run());
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match received {
            RouterOutput::EarlyDuplicate { run_id, .. } => assert_eq!(run_id, "run_active"),
            other => panic!("expected EarlyDuplicate, got {other:?}"),
        }
        drop(rx);
        let _ = handle.await;
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p surge-intake --lib router::tests
```

Expected: 2 passed.

- [ ] **Step 3: Verify whole crate**

```bash
cargo test -p surge-intake
cargo clippy -p surge-intake --all-targets -- -D warnings
```

Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-intake/src/router.rs
git commit -m "feat(intake): add TaskRouter (multiplex + Tier-1 dispatch)"
```

---

## Task 4.2 — `TaskRouter` integration test with two sources

**Files:**
- Create: `crates/surge-intake/tests/two_sources.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/surge-intake/tests/two_sources.rs`:

```rust
//! Integration test: two MockTaskSource instances feed the same router.
//! Both events should be observed.

use chrono::Utc;
use rusqlite::Connection;
use std::sync::Arc;
use std::time::Duration;
use surge_intake::router::{RouterOutput, TaskRouter};
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{TaskEvent, TaskEventKind, TaskId};
use surge_intake::TaskSource;
use tokio::sync::{mpsc, Mutex};

fn ev(source_id: &str, task_id: &str) -> TaskEvent {
    TaskEvent {
        source_id: source_id.into(),
        task_id: TaskId::try_new(task_id).unwrap(),
        kind: TaskEventKind::NewTask,
        seen_at: Utc::now(),
        raw_payload: serde_json::json!({}),
    }
}

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);").unwrap();
    let sql = include_str!("../../surge-persistence/migrations/0007_ticket_index.sql");
    conn.execute_batch(sql).unwrap();
    conn
}

#[tokio::test]
async fn two_sources_both_observed() {
    let conn = Arc::new(Mutex::new(db()));

    let src_a = Arc::new(MockTaskSource::new("mock:A", "mock"));
    let src_b = Arc::new(MockTaskSource::new("mock:B", "mock"));
    src_a.push_event(ev("mock:A", "mock:A#1")).await;
    src_b.push_event(ev("mock:B", "mock:B#1")).await;

    let (tx, mut rx) = mpsc::channel(8);
    let router = TaskRouter::new(
        vec![src_a as Arc<dyn TaskSource>, src_b as Arc<dyn TaskSource>],
        Arc::clone(&conn),
        tx,
    );
    let handle = tokio::spawn(router.run());

    let mut seen: Vec<String> = Vec::new();
    for _ in 0..2 {
        let item = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("router did not emit in time")
            .expect("channel closed");
        match item {
            RouterOutput::Triage { event } => seen.push(event.task_id.as_str().into()),
            other => panic!("unexpected: {other:?}"),
        }
    }

    seen.sort();
    assert_eq!(seen, vec!["mock:A#1".to_string(), "mock:B#1".to_string()]);

    drop(rx);
    let _ = handle.await;
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test -p surge-intake --test two_sources
```

Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-intake/tests/two_sources.rs
git commit -m "test(intake): integration test for two-source router"
```

---

## Plan A wrap-up

After Task 4.2 completes, run the full validation sequence:

- [ ] **Step 1: Workspace build**

```bash
cargo build --workspace
```

Expected: success.

- [ ] **Step 2: Workspace test**

```bash
cargo test --workspace
```

Expected: all existing tests still pass plus new `surge-intake` and `surge-persistence::intake` tests.

- [ ] **Step 3: Workspace clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Workspace fmt**

```bash
cargo fmt --all -- --check
```

If output is non-empty, run `cargo fmt --all` to fix and commit:

```bash
cargo fmt --all
git add -A
git commit -m "chore: cargo fmt"
```

- [ ] **Step 5: Document Plan A completion**

Append to a `PROGRESS-RFC-0010.md` (create if missing) or to `docs/03-ROADMAP.md`:

```markdown
## RFC-0010 — Plan A · Foundation ✅

- [x] M0 Crate scaffold (Tasks 0.1)
- [x] M1 Trait + types + MockTaskSource (Tasks 1.1–1.5)
- [x] M2 Persistence: ticket_index, task_source_state, IntakeRepo (Tasks 2.1–2.4)
- [x] M3 Tier-1 PreFilter + candidates (Tasks 3.1–3.2)
- [x] M4 TaskRouter (Tasks 4.1–4.2)

Plan B (Linear + GitHub adapters) and Plan C (Triage Author + notify + daemon + e2e) follow.
```

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "docs(rfc-0010): Plan A foundation complete"
```

---

## Plan A self-review

**Spec coverage:**

- Decision #7 (`surge-intake` crate) — covered by Task 0.1.
- Decision #22 (Tier-1 reads from SQLite) — covered by Tasks 2.1–2.4, 3.1.
- Decision #10 (Tier-1 step 1 only in MVP) — covered by Task 3.1; explicit comment that embedding steps are deferred.
- Decision #18 (pluggable architecture) — `trait TaskSource` (Task 1.4) takes no provider-specific concerns into the trait; future adapters slot in.
- `MockTaskSource` for testing — covered by Task 1.5.
- `ticket_index` schema (RFC components section) — covered by Task 2.1.
- `task_source_state` schema — covered by Task 2.2.
- `TicketState` enum / FSM (data flow section) — covered by Task 2.3.
- `IntakeRepo` query for active-run lookup — covered by Task 2.4.
- `Tier1PreFilter` step 1 — covered by Task 3.1.
- `surge-intake::candidates` module (added to spec during self-review) — covered by Task 3.2.
- `TaskRouter` (RFC components section) — covered by Tasks 4.1, 4.2.

**Out of scope for Plan A (covered in B and C):**

- Real `LinearTaskSource` and `GitHubIssuesTaskSource` (Plan B)
- Triage Author profile and bootstrap integration (Plan C)
- `surge-notify` `InboxCard` extension (Plan C)
- `surge-daemon` integration of `TaskRouter` and config wiring (Plan C)
- New `EventPayload` variants for tracker events (Plan C)
- End-to-end mock pipeline test (Plan C)
- Provider integration tests (Plan C)
- CLI commands (`surge tracker test`, etc.) (Plan C)
- Vertical-slice / token-budget enhancements (#25, #26 — RFC-0004 refactor, separate plan)

**Placeholder scan:** No TBD/TODO items remain. Each step has either explicit code or a verifiable command with expected output.

**Type consistency check:**

- `TaskId` is the same type throughout (created in Task 1.1, used in 1.3, 1.4, 1.5, 3.1, 3.2, 4.1, 4.2).
- `TicketState` (Task 2.3) is referenced as string values in 0007 migration's CHECK absence (no enum constraint at DB layer; we keep state validation in Rust). The state values used in `lookup_active_run` SQL match what `TicketState::as_str()` produces.
- `IntakeRow.run_id` is `Option<String>` — used consistently in tests in 2.4 and 3.1 and 4.1.
- `Tier1Decision` (Task 1.2) used in 3.1, 4.1.
- `RouterOutput` (Task 4.1) wraps `TaskEvent` and `String run_id` consistently.
- `MockTaskSource::push_event` (Task 1.5) used in 4.1 and 4.2 with the same signature.

No naming drift detected.
