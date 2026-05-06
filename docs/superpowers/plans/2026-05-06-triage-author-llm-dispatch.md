# Triage Author LLM Dispatch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `Priority::Medium` placeholder in `surge-daemon` with a real ACP-driven Triage Author call that returns a typed `TriageDecision` and routes to one of four destinations (real InboxCard / duplicate-comment / out-of-scope-comment / Unclear notification).

**Architecture:** A standalone `dispatch_triage(...)` async function lives in `surge-orchestrator/src/triage.rs`, opens an ACP session via `BridgeFacade`, sends the system prompt + serialized `TriageInput`, awaits `OutcomeReported`, reads `triage_decision.json` + `inbox_summary.md` from a per-call scratch directory, and returns `TriageDecision`. Retry-up-to-3 with `Unclear` fallback keeps daemon match arms total. Daemon mapping replaces the `RouterOutput::Triage` arm in `crates/surge-daemon/src/main.rs`.

**Tech Stack:** Rust 2024, tokio, `agent-client-protocol`, `surge-acp::bridge::facade::BridgeFacade`, `surge-intake::types`, `serde_json`, `thiserror`, `ulid`, `tempfile` (test-only), `insta` (snapshot tests).

**Spec:** [docs/superpowers/specs/2026-05-06-triage-author-llm-dispatch-design.md](../specs/2026-05-06-triage-author-llm-dispatch-design.md)

---

## File structure

### Created

- `crates/surge-orchestrator/tests/triage_dispatch.rs` — six unit-test scenarios for `dispatch_triage` against `MockBridge`.
- `crates/surge-daemon/tests/triage_wiring.rs` — daemon-level smoke test wiring `dispatch_triage` to `RouterOutput::Triage` consumer.

### Modified

- `crates/surge-intake/src/candidates.rs` — add `build_for_task` async helper.
- `crates/surge-persistence/src/runs/storage.rs` — add `snapshot_active_runs` method returning a new `ActiveRunRow` row type.
- `crates/surge-persistence/src/runs/mod.rs` — re-export `ActiveRunRow`.
- `crates/surge-orchestrator/src/triage.rs` — extend with `TriageError`, `TriageOptions`, `dispatch_triage`, prompt rendering, scratch dir lifecycle, retry loop, claude-binary discovery helper, `ActiveRunSummary::from_row` mapping.
- `crates/surge-orchestrator/Cargo.toml` — add `tempfile` (dev), `insta` (dev), `ulid` to dev-dependencies if absent; add `_bootstrap_llm_test` feature.
- `crates/surge-orchestrator/tests/triage_llm.rs` — replace skeleton smoke with feature-gated real-LLM body.
- `crates/surge-daemon/src/main.rs` — extract `deliver_fallback_inbox` helper; replace the placeholder `RouterOutput::Triage` arm with `dispatch_triage` call + four-way decision routing.
- `crates/surge-daemon/Cargo.toml` — confirm `surge-orchestrator` already in deps.
- `docs/03-ROADMAP.md` — strike "Triage Author LLM dispatch via ACP" from Plan-C-polish remaining list; mark Plan-C-polish complete.

### File responsibilities

| File | Responsibility |
|---|---|
| `surge-intake/src/candidates.rs` | candidate-set assembly: `build_for_task` calls `source.list_open_tasks` and reduces via `top_by_keyword_overlap` to `Vec<TaskSummary>` |
| `surge-persistence/src/runs/storage.rs` | persistent run snapshot: `snapshot_active_runs` returns `Vec<ActiveRunRow>` for Triage's `active_runs` field |
| `surge-orchestrator/src/triage.rs` | LLM dispatch: types, errors, options, retry loop, scratch lifecycle, claude binary discovery, file-artifact reading |
| `surge-daemon/src/main.rs` | wiring: assemble `TriageInput`, call `dispatch_triage`, route four decisions; preserves existing fallback-inbox path on provider errors |

---

## Task 1: `surge-intake::candidates::build_for_task` helper

**Files:**
- Modify: `crates/surge-intake/src/candidates.rs`
- Test: `crates/surge-intake/src/candidates.rs` (in-module `#[cfg(test)] mod build_for_task_tests`)

- [ ] **Step 1: Read existing module to find the right insertion point**

```bash
sed -n '1,50p' crates/surge-intake/src/candidates.rs
```

You'll see `pub fn top_by_keyword_overlap(target, candidates, limit) -> Vec<ScoredCandidate>` and `pub struct CandidateInput { task_id, title, summary }` with `from_summary(s: &TaskSummary) -> Self`.

- [ ] **Step 2: Append the failing test at end of file**

Append to `crates/surge-intake/src/candidates.rs`:

```rust
#[cfg(test)]
mod build_for_task_tests {
    use super::*;
    use crate::testing::MockTaskSource;
    use crate::types::{TaskDetails, TaskId, TaskSummary};
    use chrono::Utc;
    use std::sync::Arc;

    fn td(id: &str, title: &str, body: &str) -> TaskDetails {
        TaskDetails {
            task_id: TaskId::try_new(id).unwrap(),
            source_id: "mock:t".into(),
            title: title.into(),
            description: body.into(),
            status: "open".into(),
            labels: vec![],
            url: format!("https://x/{id}"),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            assignee: None,
            raw_payload: serde_json::json!({}),
        }
    }

    fn ts(id: &str, title: &str) -> TaskSummary {
        TaskSummary {
            task_id: TaskId::try_new(id).unwrap(),
            title: title.into(),
            status: "open".into(),
            url: format!("https://x/{id}"),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn returns_top_n_by_jaccard_similarity() {
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        // Two candidates exist: one closely related, one unrelated.
        src.put_task(td("mock:t#10", "fix parser panic on nested objects", "stack overflow")).await;
        src.put_task(td("mock:t#20", "update readme typo", "docs only")).await;

        let target = td(
            "mock:t#100",
            "parser crashes with deeply nested json",
            "stack overflow when JSON has more than 16 nested levels",
        );

        let arc: Arc<dyn crate::TaskSource> = src.clone();
        let result = build_for_task(&arc, &target, 5).await.unwrap();

        // The semantically-close candidate should appear; readme typo should not
        // (Jaccard score = 0 against parser/json/nested tokens).
        let ids: Vec<&str> = result.iter().map(|s| s.task_id.as_str()).collect();
        assert!(ids.contains(&"mock:t#10"));
        assert!(!ids.contains(&"mock:t#20"));
    }

    #[tokio::test]
    async fn excludes_target_itself() {
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        // Insert the target as if it's also "open" — common in real trackers.
        src.put_task(td("mock:t#100", "parser crash", "nested json")).await;

        let target = td("mock:t#100", "parser crash", "nested json");

        let arc: Arc<dyn crate::TaskSource> = src.clone();
        let result = build_for_task(&arc, &target, 5).await.unwrap();
        assert!(result.iter().all(|s| s.task_id.as_str() != "mock:t#100"));
    }

    #[tokio::test]
    async fn empty_open_set_returns_empty() {
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        let target = td("mock:t#1", "anything", "");
        let arc: Arc<dyn crate::TaskSource> = src.clone();
        let result = build_for_task(&arc, &target, 5).await.unwrap();
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 3: Run the test — should fail to compile (function missing)**

```bash
cargo test -p surge-intake --lib candidates::build_for_task_tests 2>&1 | head -20
```

Expected: error[E0425]: cannot find function `build_for_task` in this scope.

- [ ] **Step 4: Implement `build_for_task` above the `#[cfg(test)] mod build_for_task_tests`**

Add right after the existing `CandidateInput::from_summary`/`from_details` impls in `candidates.rs`:

```rust
/// Build a top-`limit` candidate set for a given task by calling the
/// source's `list_open_tasks` and filtering via Jaccard similarity.
///
/// The source's full open-task list is bounded by the source impl
/// (typically ≤200 entries); this helper reduces to the top `limit`.
pub async fn build_for_task(
    source: &std::sync::Arc<dyn crate::TaskSource>,
    target: &crate::types::TaskDetails,
    limit: usize,
) -> crate::Result<Vec<crate::types::TaskSummary>> {
    let open = source.list_open_tasks().await?;
    let inputs: Vec<CandidateInput> = open.iter().map(CandidateInput::from_summary).collect();
    let scored = top_by_keyword_overlap(target, &inputs, limit);
    Ok(scored
        .into_iter()
        .filter_map(|s| {
            open.iter()
                .find(|t| t.task_id.as_str() == s.task_id)
                .cloned()
        })
        .collect())
}
```

- [ ] **Step 5: Run the test — should pass**

```bash
cargo test -p surge-intake --lib candidates::build_for_task_tests
```

Expected: `test result: ok. 3 passed`.

- [ ] **Step 6: Run full crate test to ensure no regression**

```bash
cargo test -p surge-intake
```

Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-intake/src/candidates.rs
git commit -m "$(cat <<'EOF'
feat(intake): add candidates::build_for_task helper for triage

Reduces source.list_open_tasks() to top-N by Jaccard similarity for
inclusion in TriageInput. Excludes the target task itself.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `Storage::snapshot_active_runs` in `surge-persistence`

**Files:**
- Modify: `crates/surge-persistence/src/runs/storage.rs`
- Modify: `crates/surge-persistence/src/runs/mod.rs` (re-export `ActiveRunRow`)
- Test: `crates/surge-persistence/src/runs/storage.rs` (in-file `#[cfg(test)] mod snapshot_active_runs_tests`)

- [ ] **Step 1: Locate `Storage::list_runs` to understand patterns**

```bash
grep -n "pub async fn list_runs\|pub async fn get_run" crates/surge-persistence/src/runs/storage.rs | head
```

The `list_runs(filter)` method exists. We add a sibling `snapshot_active_runs(limit)`.

- [ ] **Step 2: Write the failing test**

Append to `crates/surge-persistence/src/runs/storage.rs`:

```rust
#[cfg(test)]
mod snapshot_active_runs_tests {
    use super::*;
    use crate::runs::registry::{insert_run, RunStatus};
    use std::path::PathBuf;
    use surge_core::id::RunId;

    #[tokio::test]
    async fn snapshot_returns_active_runs_only() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        // Insert one Running, one Bootstrapping, one Completed.
        let pool = storage.registry_pool().clone();
        let conn = pool.get().unwrap();
        for (id, status) in [
            ("01HXX0000000000000000RUN1", RunStatus::Running),
            ("01HXX0000000000000000BTS1", RunStatus::Bootstrapping),
            ("01HXX0000000000000000DONE", RunStatus::Completed),
        ] {
            conn.execute(
                "INSERT INTO runs (id, project_path, pipeline_template, status, started_at_ms, ended_at_ms, daemon_pid)
                 VALUES (?1, ?2, NULL, ?3, ?4, NULL, NULL)",
                rusqlite::params![
                    id,
                    "/tmp/proj",
                    format!("{:?}", status),
                    1_700_000_000_000_i64,
                ],
            ).unwrap();
        }
        drop(conn);

        let snap = storage.snapshot_active_runs(32).await.unwrap();
        assert_eq!(snap.len(), 2, "only Running + Bootstrapping should appear");
        assert!(snap.iter().all(|r| matches!(
            r.status.as_str(), "Running" | "Bootstrapping"
        )));
    }
}
```

- [ ] **Step 3: Run the test — should fail to compile**

```bash
cargo test -p surge-persistence --lib runs::storage::snapshot_active_runs_tests 2>&1 | head -20
```

Expected: errors about `snapshot_active_runs` and `ActiveRunRow` missing.

- [ ] **Step 4: Add the row type at the top of `crates/surge-persistence/src/runs/storage.rs`**

After existing imports (around line 1-30), add:

```rust
/// Lightweight active-run row for Triage Author's `active_runs` input.
///
/// Returned by [`Storage::snapshot_active_runs`]. Carries only fields
/// useful for dedup hints — full run metadata stays inside `RunSummary`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveRunRow {
    pub run_id: String,
    pub task_id: Option<String>,
    pub status: String,
    pub started_at_ms: i64,
}
```

- [ ] **Step 5: Implement `snapshot_active_runs` on `impl Storage`**

In the same file, inside `impl Storage`, add (after `list_runs`):

```rust
/// Snapshot of currently active runs (status Running or Bootstrapping).
///
/// Bounded by `limit` rows. Used by Triage Author to reason about
/// dedup against in-flight work.
///
/// Layer 1 leaves `task_id` as `None` for all rows because the
/// `ticket_index` join would require resolving cross-table foreign
/// keys not yet materialised in this code path. Layer 2's engine
/// integration will populate it.
pub async fn snapshot_active_runs(
    &self,
    limit: usize,
) -> Result<Vec<ActiveRunRow>, crate::runs::error::StorageError> {
    let conn = self
        .registry_pool
        .get()
        .map_err(|e| crate::runs::error::StorageError::Other(e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, status, started_at_ms FROM runs
             WHERE status IN ('Running', 'Bootstrapping')
             ORDER BY started_at_ms DESC
             LIMIT ?1",
        )
        .map_err(|e| crate::runs::error::StorageError::Other(e.to_string()))?;
    let rows = stmt
        .query_map([limit as i64], |row| {
            Ok(ActiveRunRow {
                run_id: row.get::<_, String>(0)?,
                task_id: None,
                status: row.get::<_, String>(1)?,
                started_at_ms: row.get::<_, i64>(2)?,
            })
        })
        .map_err(|e| crate::runs::error::StorageError::Other(e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| crate::runs::error::StorageError::Other(e.to_string()))?);
    }
    Ok(out)
}
```

- [ ] **Step 6: Re-export `ActiveRunRow`**

Edit `crates/surge-persistence/src/runs/mod.rs`:

```rust
pub use storage::{ActiveRunRow, Storage};
```

(Adjust to match the existing re-export style — likely already `pub use storage::Storage;`. Add `ActiveRunRow` to the export.)

- [ ] **Step 7: Run test**

```bash
cargo test -p surge-persistence --lib runs::storage::snapshot_active_runs_tests
```

Expected: `test result: ok. 1 passed`.

- [ ] **Step 8: Run full crate test**

```bash
cargo test -p surge-persistence
```

Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add crates/surge-persistence/src/runs/storage.rs crates/surge-persistence/src/runs/mod.rs
git commit -m "$(cat <<'EOF'
feat(persistence): add Storage::snapshot_active_runs accessor

Returns currently active (Running/Bootstrapping) runs as a lightweight
ActiveRunRow snapshot for Triage Author's active_runs input. task_id
is None at Layer 1; Layer 2 will populate from ticket_index join.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `TriageError` and `TriageOptions` types

**Files:**
- Modify: `crates/surge-orchestrator/src/triage.rs`
- Modify: `crates/surge-orchestrator/Cargo.toml` (add `tempfile` to dev, ensure `ulid` is available)

- [ ] **Step 1: Check current `triage.rs` imports and end-of-file**

```bash
sed -n '1,15p' crates/surge-orchestrator/src/triage.rs
```

You'll see the existing `use surge_intake::types::...` imports. We'll add `std::path::PathBuf`, `std::sync::Arc`, `std::time::Duration`, and the `BridgeFacade` import.

- [ ] **Step 2: Add new types above the existing `TriageInput` struct**

In `crates/surge-orchestrator/src/triage.rs`, after the doc-comment block at top (around line 7), insert new imports and types:

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::facade::BridgeFacade;
use surge_intake::types::{Priority, TaskDetails, TaskId, TaskSummary, TriageDecision};
```

Keep the existing `use serde::{Deserialize, Serialize};` and `use surge_intake::types::...` lines if already present — collapse duplicates.

Then add (after existing struct defs, before `#[cfg(test)] mod tests`):

```rust
/// Tunable parameters for [`dispatch_triage`].
///
/// Construct via [`Self::with_scratch_root`] for the typical case of
/// passing only the per-task scratch root and Claude binary path.
#[derive(Debug, Clone)]
pub struct TriageOptions {
    /// Resolved Claude binary path. If `None`, the dispatcher
    /// returns `Ok(TriageDecision::Unclear)` immediately on the
    /// first attempt with a configuration-hint message.
    pub claude_binary: Option<PathBuf>,
    /// Per-attempt timeout. Default: 5 min (matches RFC-0010 §"Bootstrap stage failures").
    pub attempt_timeout: Duration,
    /// Maximum attempts before falling back to `Unclear`. Default: 3.
    pub max_attempts: u32,
    /// Root directory for per-call scratch dirs.
    pub scratch_root: PathBuf,
    /// Whether to keep scratch on Unclear / TriageError for post-mortem.
    pub keep_scratch_on_failure: bool,
}

impl TriageOptions {
    /// Build options with sensible defaults given a scratch root and
    /// (optional) Claude binary path.
    #[must_use]
    pub fn with_scratch_root(scratch_root: PathBuf, claude_binary: Option<PathBuf>) -> Self {
        Self {
            claude_binary,
            attempt_timeout: Duration::from_secs(300),
            max_attempts: 3,
            scratch_root,
            keep_scratch_on_failure: true,
        }
    }
}

/// Errors returned from [`dispatch_triage`] for invariant violations.
///
/// Note: retry-eligible failures (timeout, agent crash, malformed JSON)
/// are NOT surfaced as `TriageError` — they retry up to
/// `opts.max_attempts` times and on exhaustion become
/// `Ok(TriageDecision::Unclear { question })`. `TriageError` is
/// reserved for failures that prevent any forward progress.
#[derive(Debug, thiserror::Error)]
pub enum TriageError {
    #[error("scratch dir setup failed: {0}")]
    Scratch(#[from] std::io::Error),
    #[error("acp bridge: {0}")]
    Bridge(String),
}
```

- [ ] **Step 3: Confirm dev-dependencies**

Open `crates/surge-orchestrator/Cargo.toml`. Confirm `tempfile` is in `[dev-dependencies]` (it already is per current state) and add `ulid` to runtime dependencies if absent:

```bash
grep -n "ulid\|tempfile" crates/surge-orchestrator/Cargo.toml
```

If `ulid` is missing from `[dependencies]`, add:

```toml
ulid = { workspace = true }
```

- [ ] **Step 4: Build to verify types compile**

```bash
cargo build -p surge-orchestrator
```

Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/triage.rs crates/surge-orchestrator/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(orchestrator): add TriageOptions and TriageError types

Foundation for dispatch_triage. TriageError covers invariant
violations only; retry-eligible failures fall back to Unclear.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `dispatch_triage` Enqueued happy path

**Files:**
- Modify: `crates/surge-orchestrator/src/triage.rs`
- Test: `crates/surge-orchestrator/tests/triage_dispatch.rs` (new)
- Modify: `crates/surge-orchestrator/tests/fixtures/mod.rs` (re-export `mock_bridge` if needed)

- [ ] **Step 1: Confirm fixtures module re-exports mock_bridge**

```bash
cat crates/surge-orchestrator/tests/fixtures/mod.rs
```

If `pub mod mock_bridge;` is present, no change needed. Otherwise add it.

- [ ] **Step 2: Write the failing happy-path test**

Create `crates/surge-orchestrator/tests/triage_dispatch.rs`:

```rust
//! Unit tests for `triage::dispatch_triage` against `MockBridge`.

#[path = "fixtures/mod.rs"]
mod fixtures;

use chrono::Utc;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::event::{BridgeEvent, SessionEndReason};
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::{OutcomeKey, SessionId};
use surge_intake::types::{Priority, TaskDetails, TaskId, TriageDecision};
use surge_orchestrator::triage::{dispatch_triage, TriageInput, TriageOptions};
use tempfile::TempDir;

fn task_details(id: &str) -> TaskDetails {
    TaskDetails {
        task_id: TaskId::try_new(id).unwrap(),
        source_id: "mock:t".into(),
        title: "Fix parser panic".into(),
        description: "Stack overflow on nested JSON".into(),
        status: "open".into(),
        labels: vec!["surge:enabled".into()],
        url: format!("https://x/{id}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    }
}

fn input() -> TriageInput {
    TriageInput {
        task: task_details("mock:t#1"),
        candidates: vec![],
        active_runs: vec![],
    }
}

/// Test harness: drives the mock bridge by writing the artifact files
/// into the scratch dir and emitting the OutcomeReported event.
async fn drive_enqueued(
    bridge: Arc<fixtures::mock_bridge::MockBridge>,
    scratch: &std::path::Path,
    session: SessionId,
    decision_json: &str,
    summary_md: &str,
) {
    // Write artifacts as if the agent had done so.
    std::fs::write(scratch.join("triage_decision.json"), decision_json).unwrap();
    std::fs::write(scratch.join("inbox_summary.md"), summary_md).unwrap();
    bridge
        .enqueue_event(BridgeEvent::OutcomeReported {
            session,
            outcome: OutcomeKey::from_str("enqueued").unwrap(),
            summary: "agent picked enqueued".into(),
            artifacts_produced: vec![
                "triage_decision.json".into(),
                "inbox_summary.md".into(),
            ],
        })
        .await;
}

#[tokio::test]
async fn enqueued_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        // Use a non-None binary so dispatcher proceeds to open_session.
        // MockBridge ignores the actual path.
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    // Spawn a side task that drives the mock bridge once it has subscribers.
    // We need the dispatcher to subscribe before enqueueing; do this by
    // sleeping briefly then writing artifacts + pumping events.
    let scratch_root = tmp.path().to_path_buf();
    let bridge_for_drive = Arc::clone(&bridge);
    let drive = tokio::spawn(async move {
        // Wait long enough for dispatcher to call subscribe + open_session.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The dispatcher creates one scratch subdir per call; find it.
        let entries: Vec<_> = std::fs::read_dir(&scratch_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        let scratch = entries
            .iter()
            .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .expect("dispatcher should have created scratch subdir")
            .path();

        let decision = r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#;
        let summary = "## Fix parser panic\n\nStack overflow at depth 16.";
        drive_enqueued(bridge_for_drive, &scratch, session, decision, summary).await;
        // Pump after artifacts are in place so dispatcher reads them post-event.
        let bridge_pump = Arc::clone(&fixtures::mock_bridge::Arc_clone_workaround(/* see below */));
    });

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .expect("dispatch_triage should succeed");

    drive.await.unwrap();

    match result {
        TriageDecision::Enqueued { priority, .. } => {
            assert_eq!(priority, Priority::High);
        }
        other => panic!("expected Enqueued, got {other:?}"),
    }
}
```

Note the placeholder `Arc_clone_workaround` — the test is written before the real code; you'll iterate. The cleaner pattern is to call `bridge.pump_scripted_events()` from the same scope that owns `Arc<MockBridge>`. Refactor:

```rust
// Replace the previous `drive` task with:
let scratch_root = tmp.path().to_path_buf();
let bridge_for_drive = Arc::clone(&bridge);
let drive = tokio::spawn(async move {
    tokio::time::sleep(Duration::from_millis(50)).await;
    let entries: Vec<_> = std::fs::read_dir(&scratch_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let scratch = entries
        .iter()
        .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .expect("dispatcher should have created scratch subdir")
        .path();

    let decision = r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#;
    let summary = "## Fix parser panic\n\nStack overflow at depth 16.";
    std::fs::write(scratch.join("triage_decision.json"), decision).unwrap();
    std::fs::write(scratch.join("inbox_summary.md"), summary).unwrap();
    bridge_for_drive
        .enqueue_event(BridgeEvent::OutcomeReported {
            session,
            outcome: OutcomeKey::from_str("enqueued").unwrap(),
            summary: "agent picked enqueued".into(),
            artifacts_produced: vec!["triage_decision.json".into(), "inbox_summary.md".into()],
        })
        .await;
    bridge_for_drive.pump_scripted_events().await;
});
```

Remove the helper `drive_enqueued` and the `Arc_clone_workaround` reference. Final test body uses the inline `drive` task.

- [ ] **Step 3: Run the test — should fail (function not implemented)**

```bash
cargo test -p surge-orchestrator --test triage_dispatch enqueued_happy_path 2>&1 | head -20
```

Expected: error[E0432]: unresolved import `surge_orchestrator::triage::dispatch_triage`.

- [ ] **Step 4: Implement `dispatch_triage` in `triage.rs`**

Add after `TriageError` definition:

```rust
/// Dispatch a Triage Author session against the supplied bridge.
///
/// Returns `Ok(TriageDecision)` even on retry exhaustion (materialises
/// as `Unclear` with a diagnostic question). `Err(TriageError)` is
/// reserved for invariant violations that prevent any forward progress.
///
/// # Errors
/// - [`TriageError::Scratch`] if the per-call scratch directory
///   cannot be created.
/// - [`TriageError::Bridge`] for facade-level dead-bridge or
///   handshake failures.
pub async fn dispatch_triage(
    bridge: Arc<dyn BridgeFacade>,
    input: TriageInput,
    opts: TriageOptions,
) -> Result<TriageDecision, TriageError> {
    // Short-circuit if claude binary is not configured.
    let Some(claude_binary) = opts.claude_binary.clone() else {
        return Ok(TriageDecision::Unclear {
            question: "Claude binary not configured (set SURGE_CLAUDE_BINARY or install \
                       claude-code); install to enable LLM-driven triage"
                .into(),
        });
    };

    // Build a fresh scratch dir for this top-level call.
    let scratch_dir = opts
        .scratch_root
        .join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(&scratch_dir)?;

    let mut last_err: Option<String> = None;

    for attempt in 1..=opts.max_attempts {
        match try_one_attempt(
            Arc::clone(&bridge),
            &input,
            &claude_binary,
            &scratch_dir,
            opts.attempt_timeout,
            attempt,
            last_err.as_deref(),
        )
        .await
        {
            Ok(decision) => {
                if !opts.keep_scratch_on_failure
                    && !matches!(decision, TriageDecision::Unclear { .. })
                {
                    let _ = std::fs::remove_dir_all(&scratch_dir);
                }
                return Ok(decision);
            }
            Err(AttemptError::Bridge(e)) => return Err(TriageError::Bridge(e)),
            Err(AttemptError::Retryable(msg)) => {
                tracing::warn!(attempt, error = %msg, "triage attempt failed; will retry");
                last_err = Some(msg);
            }
        }
    }

    let question = format!(
        "Triage failed after {} attempts: {}",
        opts.max_attempts,
        last_err.as_deref().unwrap_or("unknown error")
    );
    Ok(TriageDecision::Unclear { question })
}

/// Per-attempt error type — distinguishes retryable from fatal.
enum AttemptError {
    /// Bridge facade itself failed (open/send/close); fatal.
    Bridge(String),
    /// Retryable: timeout, agent crash, malformed artifact.
    Retryable(String),
}

/// Bridge-level sandbox for sessions where Surge delegates isolation
/// to the agent itself (per Vision-2026 §"Sandbox-delegated").
///
/// Returns `AlwaysAllowSandbox` — the bridge applies no tool filtering,
/// because each ACP-conformant agent (Claude Code, Codex CLI, etc.)
/// already has its own native sandbox enforcement. The profile's
/// `[sandbox] mode = ...` field is a semantic marker the agent reads,
/// not a directive the bridge enforces.
///
/// Globalising this convention (toggle in `SurgeConfig`, removal of
/// `DenyListSandbox` surface) is the RFC-0006 refactor; for Layer 1
/// we lock in the convention via this helper.
fn delegated_sandbox() -> Box<dyn surge_acp::bridge::sandbox::Sandbox> {
    Box::new(surge_acp::bridge::sandbox::AlwaysAllowSandbox)
}

async fn try_one_attempt(
    bridge: Arc<dyn BridgeFacade>,
    input: &TriageInput,
    claude_binary: &PathBuf,
    scratch_dir: &std::path::Path,
    attempt_timeout: Duration,
    attempt: u32,
    feedback: Option<&str>,
) -> Result<TriageDecision, AttemptError> {
    use std::collections::BTreeMap;
    use surge_acp::bridge::session::{AgentKind, MessageContent, SessionConfig};
    use surge_acp::client::PermissionPolicy;

    let prompt_text = render_prompt(input, feedback);

    let declared_outcomes = vec![
        OutcomeKey::try_from("enqueued").map_err(|e| AttemptError::Bridge(e.to_string()))?,
        OutcomeKey::try_from("duplicate").map_err(|e| AttemptError::Bridge(e.to_string()))?,
        OutcomeKey::try_from("out_of_scope").map_err(|e| AttemptError::Bridge(e.to_string()))?,
        OutcomeKey::try_from("unclear").map_err(|e| AttemptError::Bridge(e.to_string()))?,
    ];

    let mut bindings = BTreeMap::new();
    bindings.insert("intake.task_id".into(), input.task.task_id.as_str().to_string());
    bindings.insert("intake.attempt".into(), attempt.to_string());

    let cfg = SessionConfig {
        agent_kind: AgentKind::ClaudeCode {
            binary: claude_binary.clone(),
            extra_args: vec![],
        },
        working_dir: scratch_dir.to_path_buf(),
        system_prompt: prompt_text.clone(),
        declared_outcomes,
        allows_escalation: false,
        tools: vec![],
        sandbox: delegated_sandbox(),
        permission_policy: PermissionPolicy::default(),
        bindings,
    };

    let mut events = bridge.subscribe();

    let session_id = bridge
        .open_session(cfg)
        .await
        .map_err(|e| AttemptError::Bridge(format!("open_session: {e}")))?;

    bridge
        .send_message(session_id, MessageContent::Text(prompt_text))
        .await
        .map_err(|e| AttemptError::Bridge(format!("send_message: {e}")))?;

    // Drive the event loop with a single timeout.
    let outcome_result = tokio::time::timeout(attempt_timeout, async {
        loop {
            match events.recv().await {
                Ok(ev) => {
                    if event_session_id(&ev) != Some(session_id) {
                        continue;
                    }
                    match ev {
                        BridgeEvent::OutcomeReported { outcome, .. } => return Ok(outcome),
                        BridgeEvent::SessionEnded { reason, .. } => match reason {
                            SessionEndReason::AgentCrashed { .. } => {
                                return Err(format!("agent crashed: {reason:?}"))
                            }
                            SessionEndReason::Timeout { .. } => {
                                return Err("session timeout".into())
                            }
                            _ => continue,
                        },
                        _ => continue,
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err("event stream closed".into())
                }
            }
        }
    })
    .await;

    let _ = bridge.close_session(session_id).await;

    let outcome = match outcome_result {
        Err(_elapsed) => return Err(AttemptError::Retryable("timeout".into())),
        Ok(Err(msg)) => return Err(AttemptError::Retryable(msg)),
        Ok(Ok(outcome)) => outcome,
    };

    // Read the artifact and parse.
    let json_path = scratch_dir.join("triage_decision.json");
    let raw = std::fs::read(&json_path).map_err(|e| {
        AttemptError::Retryable(format!("triage_decision.json missing: {e}"))
    })?;
    let parsed: TriageJson = serde_json::from_slice(&raw).map_err(|e| {
        AttemptError::Retryable(format!("triage_decision.json malformed: {e}"))
    })?;
    let mut decision = parsed
        .into_decision()
        .map_err(|e| AttemptError::Retryable(format!("decision rejected: {e}")))?;

    // For Enqueued, splice in the inbox_summary.md if present.
    if let TriageDecision::Enqueued { summary, .. } = &mut decision {
        if summary.is_empty() {
            if let Ok(md) = std::fs::read_to_string(scratch_dir.join("inbox_summary.md")) {
                *summary = md;
            }
        }
    }

    // Sanity-check against the agent's reported outcome string.
    let outcome_str = outcome.as_str();
    let decision_kind = match &decision {
        TriageDecision::Enqueued { .. } => "enqueued",
        TriageDecision::Duplicate { .. } => "duplicate",
        TriageDecision::OutOfScope { .. } => "out_of_scope",
        TriageDecision::Unclear { .. } => "unclear",
    };
    if outcome_str != decision_kind {
        return Err(AttemptError::Retryable(format!(
            "outcome ({outcome_str}) mismatch with decision ({decision_kind})"
        )));
    }

    Ok(decision)
}

fn render_prompt(input: &TriageInput, feedback: Option<&str>) -> String {
    let json = serde_json::to_string_pretty(input)
        .unwrap_or_else(|_| "{}".into());
    let mut out = String::new();
    out.push_str(crate::BOOTSTRAP_TRIAGE_AUTHOR_TOML);
    out.push_str("\n\n# Inputs\n\nThe triage input is encoded as JSON. The shape is:\n");
    out.push_str("- task: TaskDetails\n- candidates: TaskSummary[]\n- active_runs: ActiveRunSummary[]\n\n");
    out.push_str("The literal JSON follows:\n\n");
    out.push_str(&json);
    out.push_str("\n\n# Task\n\nDecide whether this ticket is a duplicate, out-of-scope, unclear, \
                  or should be enqueued. Then, in your working directory:\n\n\
                  1. Write your structured decision to `triage_decision.json` (schema below).\n\
                  2. Write a 3-5 line markdown blurb to `inbox_summary.md` (used as the body of \
                     the inbox card on `enqueued`; safe to omit otherwise).\n\
                  3. Call `report_stage_outcome` with the matching `outcome` and \
                     `artifacts_produced = [\"triage_decision.json\", \"inbox_summary.md\"]`.\n\n");
    out.push_str("triage_decision.json schema:\n\
                  - decision: \"enqueued\" | \"duplicate\" | \"out_of_scope\" | \"unclear\"\n\
                  - duplicate_of: string (task id) or null\n\
                  - priority: \"urgent\" | \"high\" | \"medium\" | \"low\"\n\
                  - priority_reasoning: one sentence\n\
                  - summary: one sentence\n\
                  - question: string (only when decision = \"unclear\")\n");
    if let Some(fb) = feedback {
        out.push_str("\n# Feedback from previous attempt\n\n");
        out.push_str(fb);
        out.push('\n');
    }
    out
}

fn event_session_id(event: &BridgeEvent) -> Option<SessionId> {
    match event {
        BridgeEvent::SessionEstablished { session, .. }
        | BridgeEvent::AgentMessage { session, .. }
        | BridgeEvent::TokenUsage { session, .. }
        | BridgeEvent::ToolCall { session, .. }
        | BridgeEvent::ToolResult { session, .. }
        | BridgeEvent::OutcomeReported { session, .. }
        | BridgeEvent::HumanInputRequested { session, .. }
        | BridgeEvent::SessionEnded { session, .. } => Some(*session),
        BridgeEvent::Error { session, .. } => *session,
    }
}
```

Add the `BridgeEvent` and `OutcomeKey` imports at top of file:

```rust
use surge_acp::bridge::event::{BridgeEvent, SessionEndReason};
use surge_core::{OutcomeKey, SessionId};
```

- [ ] **Step 5: Run the test — should pass**

```bash
cargo test -p surge-orchestrator --test triage_dispatch enqueued_happy_path
```

Expected: `test result: ok. 1 passed`.

If it hangs: the test scratch-dir polling is racy. Increase the `tokio::time::sleep(Duration::from_millis(50))` to 200ms, or add an explicit synchronisation pattern via `tokio::sync::Notify` between the dispatcher's subscribe and the drive task's pump.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-orchestrator/src/triage.rs crates/surge-orchestrator/tests/triage_dispatch.rs
git commit -m "$(cat <<'EOF'
feat(orchestrator): dispatch_triage Enqueued happy path

Adds the core dispatch loop: open ACP session, send prompt, await
OutcomeReported, read triage_decision.json + inbox_summary.md from
scratch dir, parse via TriageJson::into_decision. Layer 1 of the
two-layer plan in 2026-05-06-triage-author-llm-dispatch-design.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Decision-variant tests (Duplicate / OutOfScope / Unclear)

**Files:**
- Modify: `crates/surge-orchestrator/tests/triage_dispatch.rs`

- [ ] **Step 1: Extract a shared `drive_one_attempt` helper at the top of the test file**

Add right after the `input()` helper:

```rust
/// Reusable driver: writes JSON + summary into the scratch sub-dir
/// the dispatcher created, then enqueues an OutcomeReported event
/// matching `outcome_key`.
async fn drive_one_attempt(
    bridge: Arc<fixtures::mock_bridge::MockBridge>,
    scratch_root: std::path::PathBuf,
    session: SessionId,
    outcome_key: &str,
    decision_json: &str,
    summary_md: &str,
) {
    tokio::time::sleep(Duration::from_millis(80)).await;
    let scratch = std::fs::read_dir(&scratch_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .expect("dispatcher created scratch subdir")
        .path();

    std::fs::write(scratch.join("triage_decision.json"), decision_json).unwrap();
    if !summary_md.is_empty() {
        std::fs::write(scratch.join("inbox_summary.md"), summary_md).unwrap();
    }
    bridge
        .enqueue_event(BridgeEvent::OutcomeReported {
            session,
            outcome: OutcomeKey::from_str(outcome_key).unwrap(),
            summary: format!("agent picked {outcome_key}"),
            artifacts_produced: vec!["triage_decision.json".into(), "inbox_summary.md".into()],
        })
        .await;
    bridge.pump_scripted_events().await;
}
```

Refactor `enqueued_happy_path` to use `drive_one_attempt`. Then add the next three tests:

- [ ] **Step 2: Add `duplicate_happy_path`**

```rust
#[tokio::test]
async fn duplicate_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "duplicate",
        r#"{"decision":"duplicate","duplicate_of":"mock:t#42","priority":"high","priority_reasoning":"same code path"}"#,
        "",
    ));

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Duplicate { of, .. } => {
            assert_eq!(of.as_str(), "mock:t#42");
        }
        other => panic!("expected Duplicate, got {other:?}"),
    }
}
```

- [ ] **Step 3: Add `out_of_scope_happy_path`**

```rust
#[tokio::test]
async fn out_of_scope_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "out_of_scope",
        r#"{"decision":"out_of_scope","priority":"low","priority_reasoning":"hiring task"}"#,
        "",
    ));

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .unwrap();
    drive.await.unwrap();

    assert!(matches!(result, TriageDecision::OutOfScope { .. }));
}
```

- [ ] **Step 4: Add `unclear_happy_path`**

```rust
#[tokio::test]
async fn unclear_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "unclear",
        r#"{"decision":"unclear","priority":"medium","question":"What does X mean here?"}"#,
        "",
    ));

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(question.contains("What does X mean here"));
        }
        other => panic!("expected Unclear, got {other:?}"),
    }
}
```

- [ ] **Step 5: Run all four tests**

```bash
cargo test -p surge-orchestrator --test triage_dispatch
```

Expected: `4 passed`.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-orchestrator/tests/triage_dispatch.rs
git commit -m "$(cat <<'EOF'
test(orchestrator): triage decision variants Duplicate/OOS/Unclear

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Retry mechanics — bad JSON recovers; exhaustion → Unclear

**Files:**
- Modify: `crates/surge-orchestrator/tests/triage_dispatch.rs`

- [ ] **Step 1: Add helper for multi-attempt scripting**

We need to drive multiple attempts. Each attempt opens a new session (the dispatcher closes between retries). So the helper writes to whichever scratch sub-dir exists most recently and emits one event per attempt.

Add to `triage_dispatch.rs`:

```rust
/// Drive `n_attempts` sequential attempts. For each, write the
/// supplied (decision_json, summary_md) into the latest scratch
/// sub-dir and emit OutcomeReported for `outcome_keys[i]`.
async fn drive_n_attempts(
    bridge: Arc<fixtures::mock_bridge::MockBridge>,
    scratch_root: std::path::PathBuf,
    sessions: Vec<SessionId>,
    outcomes_and_artifacts: Vec<(&'static str, &'static str)>,
) {
    assert_eq!(sessions.len(), outcomes_and_artifacts.len());
    for (i, (outcome_key, decision_json)) in outcomes_and_artifacts.iter().enumerate() {
        // Wait for dispatcher to create a new scratch dir for this attempt
        // (only true for the first attempt — later attempts reuse the
        // top-level scratch dir; the inner attempt logic runs against
        // the same scratch_dir).
        tokio::time::sleep(Duration::from_millis(80)).await;
        let scratch = std::fs::read_dir(&scratch_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .expect("dispatcher created scratch subdir")
            .path();
        std::fs::write(scratch.join("triage_decision.json"), decision_json).unwrap();
        bridge
            .enqueue_event(BridgeEvent::OutcomeReported {
                session: sessions[i],
                outcome: OutcomeKey::from_str(outcome_key).unwrap(),
                summary: format!("attempt {} → {outcome_key}", i + 1),
                artifacts_produced: vec!["triage_decision.json".into()],
            })
            .await;
        bridge.pump_scripted_events().await;
    }
}
```

Note: per the implementation in Task 4, the scratch dir is created **once per `dispatch_triage` call** (top-level), and reused across retries — see the `for attempt in 1..=opts.max_attempts` loop. So we only need one scratch sub-dir, not one per attempt.

- [ ] **Step 2: Pin a sequence of session ids on MockBridge**

`MockBridge::pin_next_session_id` consumes one id per call. To pin a sequence, extend `MockBridge` with `pin_session_ids(Vec<SessionId>)` that pops from a queue.

Modify `crates/surge-orchestrator/tests/fixtures/mock_bridge.rs`:

```rust
// Replace `pinned_session_id: Mutex<Option<SessionId>>` with:
pinned_session_ids: Mutex<VecDeque<SessionId>>,
```

Update `MockBridge::new`:
```rust
pinned_session_ids: Mutex::new(VecDeque::new()),
```

Replace `pin_next_session_id`:
```rust
#[allow(dead_code)]
pub async fn pin_next_session_id(&self, id: SessionId) {
    self.pinned_session_ids.lock().await.push_back(id);
}

#[allow(dead_code)]
pub async fn pin_session_ids(&self, ids: Vec<SessionId>) {
    let mut q = self.pinned_session_ids.lock().await;
    for id in ids {
        q.push_back(id);
    }
}
```

Update the `open_session` impl in the same file:
```rust
async fn open_session(&self, _: SessionConfig) -> Result<SessionId, OpenSessionError> {
    self.recorded_calls.lock().await.push(RecordedCall::OpenSession);
    let id = self
        .pinned_session_ids
        .lock()
        .await
        .pop_front()
        .unwrap_or_else(SessionId::new);
    Ok(id)
}
```

Run the existing orchestrator tests to make sure nothing else broke:

```bash
cargo test -p surge-orchestrator --tests 2>&1 | tail -20
```

Expected: existing tests still pass. (The single-session `pin_next_session_id` is now backed by the queue; behaviour is identical for tests that pin only one.)

- [ ] **Step 3: Write `bad_json_then_recovers`**

Add to `triage_dispatch.rs`:

```rust
#[tokio::test]
async fn bad_json_then_recovers() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session_a = SessionId::new();
    let session_b = SessionId::new();
    bridge.pin_session_ids(vec![session_a, session_b]).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 3,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_n_attempts(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        vec![session_a, session_b],
        vec![
            ("enqueued", "{ this is not valid json"),
            ("enqueued", r#"{"decision":"enqueued","priority":"medium","priority_reasoning":"ok","summary":"fixed"}"#),
        ],
    ));

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .unwrap();
    drive.await.unwrap();

    assert!(matches!(result, TriageDecision::Enqueued { .. }));
}
```

- [ ] **Step 4: Write `exhaust_retries_yields_unclear`**

```rust
#[tokio::test]
async fn exhaust_retries_yields_unclear() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let s1 = SessionId::new();
    let s2 = SessionId::new();
    let s3 = SessionId::new();
    bridge.pin_session_ids(vec![s1, s2, s3]).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 3,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: true,
    };

    let drive = tokio::spawn(drive_n_attempts(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        vec![s1, s2, s3],
        vec![
            ("enqueued", "{ bad 1"),
            ("enqueued", "{ bad 2"),
            ("enqueued", "{ bad 3"),
        ],
    ));

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(question.contains("Triage failed after 3 attempts"));
        }
        other => panic!("expected Unclear, got {other:?}"),
    }
}
```

- [ ] **Step 5: Run the new tests**

```bash
cargo test -p surge-orchestrator --test triage_dispatch bad_json_then_recovers exhaust_retries_yields_unclear
```

Expected: `2 passed`.

- [ ] **Step 6: Run full triage_dispatch suite**

```bash
cargo test -p surge-orchestrator --test triage_dispatch
```

Expected: `6 passed`.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-orchestrator/tests/triage_dispatch.rs crates/surge-orchestrator/tests/fixtures/mock_bridge.rs
git commit -m "$(cat <<'EOF'
test(orchestrator): triage retry mechanics — bad JSON + exhaustion

Adds two retry tests: malformed JSON on attempt 1 recovers on attempt 2;
three malformed attempts yield Unclear with diagnostic question.
Extends MockBridge to support pinning a sequence of session ids.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Timeout and AgentCrashed retry tests

**Files:**
- Modify: `crates/surge-orchestrator/tests/triage_dispatch.rs`

- [ ] **Step 1: Write `timeout_yields_unclear`**

Append to `triage_dispatch.rs`:

```rust
#[tokio::test]
async fn timeout_yields_unclear() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let s = SessionId::new();
    bridge.pin_session_ids(vec![s]).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_millis(150),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: true,
    };

    // No drive task — bridge never emits OutcomeReported, so the
    // dispatcher hits its tokio::time::timeout.
    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .unwrap();

    match result {
        TriageDecision::Unclear { question } => assert!(question.contains("timeout")),
        other => panic!("expected Unclear, got {other:?}"),
    }
}
```

- [ ] **Step 2: Write `agent_crashed_yields_unclear`**

```rust
#[tokio::test]
async fn agent_crashed_yields_unclear() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let s = SessionId::new();
    bridge.pin_session_ids(vec![s]).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: true,
    };

    let drive = {
        let bridge = Arc::clone(&bridge);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            bridge
                .enqueue_event(BridgeEvent::SessionEnded {
                    session: s,
                    reason: SessionEndReason::AgentCrashed {
                        exit_code: Some(137),
                        stderr_tail: "killed".into(),
                    },
                })
                .await;
            bridge.pump_scripted_events().await;
        })
    };

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(question.contains("agent crashed") || question.contains("AgentCrashed"));
        }
        other => panic!("expected Unclear, got {other:?}"),
    }
}
```

- [ ] **Step 3: Write `binary_missing_short_circuits`**

```rust
#[tokio::test]
async fn binary_missing_short_circuits() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: None, // explicit
        attempt_timeout: Duration::from_secs(5),
        max_attempts: 3,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(question.to_lowercase().contains("claude binary"));
        }
        other => panic!("expected Unclear, got {other:?}"),
    }

    // The bridge should NEVER have been called — the function returned
    // before any open_session.
    let calls = bridge.recorded_calls.lock().await;
    assert!(calls.is_empty(), "no bridge calls expected, got {calls:?}");
}
```

- [ ] **Step 4: Run the three tests**

```bash
cargo test -p surge-orchestrator --test triage_dispatch timeout_yields_unclear agent_crashed_yields_unclear binary_missing_short_circuits
```

Expected: `3 passed`.

- [ ] **Step 5: Run full triage_dispatch suite**

```bash
cargo test -p surge-orchestrator --test triage_dispatch
```

Expected: `9 passed`.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-orchestrator/tests/triage_dispatch.rs
git commit -m "$(cat <<'EOF'
test(orchestrator): triage timeout/agent-crash/binary-missing paths

Validates that exhausted timeout/crash retries yield Unclear, and
that a None claude_binary short-circuits without touching the bridge.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Initial-message snapshot test

**Files:**
- Modify: `crates/surge-orchestrator/src/triage.rs`
- Confirm: `insta` is in dev-dependencies (it should be — used elsewhere in workspace)

- [ ] **Step 1: Confirm `insta` is available**

```bash
grep -n "insta" crates/surge-orchestrator/Cargo.toml
```

If not present in `[dev-dependencies]`, add it:

```toml
insta = { workspace = true }
```

- [ ] **Step 2: Add the snapshot test in the existing `tests` module**

Append inside the existing `#[cfg(test)] mod tests { ... }` block (the parser tests at the bottom of `triage.rs`):

```rust
#[test]
fn render_prompt_snapshot() {
    use chrono::TimeZone;
    let task = TaskDetails {
        task_id: TaskId::try_new("github_issues:test/repo#42").unwrap(),
        source_id: "github_issues:test/repo".into(),
        title: "Add tracing to auth middleware".into(),
        description: "We have no observability into the auth flow.".into(),
        status: "open".into(),
        labels: vec!["surge:enabled".into(), "area/auth".into()],
        url: "https://github.com/test/repo/issues/42".into(),
        created_at: chrono::Utc.with_ymd_and_hms(2026, 5, 1, 10, 0, 0).unwrap(),
        updated_at: chrono::Utc.with_ymd_and_hms(2026, 5, 5, 12, 0, 0).unwrap(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    };
    let candidates = vec![
        TaskSummary {
            task_id: TaskId::try_new("github_issues:test/repo#10").unwrap(),
            title: "Update README".into(),
            status: "open".into(),
            url: "https://github.com/test/repo/issues/10".into(),
            updated_at: chrono::Utc.with_ymd_and_hms(2026, 4, 28, 9, 0, 0).unwrap(),
        },
    ];
    let active_runs = vec![ActiveRunSummary {
        run_id: "01HXX0000000000000000RUN1".into(),
        task_id: Some("github_issues:test/repo#9".into()),
        status: "Running".into(),
        started_at: "2026-05-06T10:00:00Z".into(),
    }];
    let input = TriageInput {
        task,
        candidates,
        active_runs,
    };
    let rendered = super::render_prompt(&input, None);
    insta::assert_snapshot!("triage_initial_prompt", rendered);
}

#[test]
fn render_prompt_with_feedback_includes_feedback_block() {
    let task = TaskDetails {
        task_id: TaskId::try_new("github_issues:t/r#1").unwrap(),
        source_id: "github_issues:t/r".into(),
        title: "x".into(),
        description: "y".into(),
        status: "open".into(),
        labels: vec![],
        url: "https://x".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    };
    let input = TriageInput {
        task,
        candidates: vec![],
        active_runs: vec![],
    };
    let rendered = super::render_prompt(&input, Some("previous attempt malformed JSON"));
    assert!(rendered.contains("# Feedback from previous attempt"));
    assert!(rendered.contains("previous attempt malformed JSON"));
}
```

- [ ] **Step 3: Run the test, accept the snapshot**

```bash
cargo test -p surge-orchestrator --lib triage::tests::render_prompt_snapshot
```

Expected on first run: snapshot review prompted (test fails or pending).

```bash
cargo insta accept --package surge-orchestrator
```

Then re-run:

```bash
cargo test -p surge-orchestrator --lib triage::tests::render_prompt_snapshot
```

Expected: pass.

- [ ] **Step 4: Run the feedback-block test**

```bash
cargo test -p surge-orchestrator --lib triage::tests::render_prompt_with_feedback_includes_feedback_block
```

Expected: pass.

- [ ] **Step 5: Commit (include the snapshot file)**

```bash
git add crates/surge-orchestrator/src/triage.rs crates/surge-orchestrator/src/snapshots/
git commit -m "$(cat <<'EOF'
test(orchestrator): snapshot test for triage prompt rendering

Catches prompt regressions at PR review time. Also asserts feedback
block is appended on retry attempts.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Claude-binary discovery helper

**Files:**
- Modify: `crates/surge-orchestrator/src/triage.rs`

- [ ] **Step 1: Write the failing helper test**

Append to the existing `#[cfg(test)] mod tests` in `triage.rs`:

```rust
#[test]
fn find_claude_binary_returns_env_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = tmp.path().join("claude-fake");
    std::fs::write(&fake, b"#!/bin/sh\necho fake\n").unwrap();
    // SAFETY: setting an env var in a single-threaded test is fine.
    // Tests in the same crate that touch this var should be #[serial].
    unsafe { std::env::set_var("SURGE_CLAUDE_BINARY", &fake); }
    let result = super::find_claude_binary();
    unsafe { std::env::remove_var("SURGE_CLAUDE_BINARY"); }
    assert_eq!(result, Some(fake));
}

#[test]
fn find_claude_binary_returns_none_when_not_found() {
    unsafe { std::env::remove_var("SURGE_CLAUDE_BINARY"); }
    unsafe { std::env::remove_var("CLAUDE_PATH"); }
    // We can't 100% guarantee `claude` isn't on PATH; relax the assertion to
    // "function returns Some-or-None without panicking and the result is a
    // valid file path if Some".
    if let Some(p) = super::find_claude_binary() {
        assert!(p.exists(), "discovery returned a non-existent path: {p:?}");
    }
}
```

- [ ] **Step 2: Run — should fail to compile**

```bash
cargo test -p surge-orchestrator --lib triage::tests::find_claude_binary 2>&1 | head -10
```

Expected: function `find_claude_binary` not found.

- [ ] **Step 3: Implement `find_claude_binary` in `triage.rs`**

Add at module scope (sibling of `dispatch_triage`):

```rust
/// Best-effort discovery of the Claude binary path.
///
/// Probe order:
/// 1. `SURGE_CLAUDE_BINARY` env var (must point to an existing file).
/// 2. `CLAUDE_PATH` env var (existing convention from `surge-acp::discovery`).
/// 3. Standard install locations for the current platform: e.g.
///    `/usr/local/bin/claude`, `/opt/homebrew/bin/claude`, `~/.local/bin/claude`,
///    `%USERPROFILE%\AppData\Local\Programs\claude\claude.exe`.
///
/// Returns `None` if no candidate exists. The dispatcher then surfaces
/// a configuration-hint Unclear notification.
#[must_use]
pub fn find_claude_binary() -> Option<PathBuf> {
    for var in ["SURGE_CLAUDE_BINARY", "CLAUDE_PATH"] {
        if let Ok(v) = std::env::var(var) {
            let p = PathBuf::from(v);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // Standard paths probe via surge_acp::discovery::Platform.
    use surge_acp::discovery::Platform;
    let bin_name = if cfg!(windows) { "claude.exe" } else { "claude" };
    for base in Platform::current().standard_paths() {
        let candidate = base.join(bin_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p surge-orchestrator --lib triage::tests::find_claude_binary
```

Expected: `2 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/triage.rs
git commit -m "$(cat <<'EOF'
feat(orchestrator): triage::find_claude_binary helper

Probes SURGE_CLAUDE_BINARY, CLAUDE_PATH, then platform-standard
install locations. Returns None for missing — dispatcher then
short-circuits to Unclear with a configuration hint.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Daemon helper extraction — `deliver_fallback_inbox`

**Files:**
- Modify: `crates/surge-daemon/src/main.rs`

- [ ] **Step 1: Open `crates/surge-daemon/src/main.rs` and locate the existing `RouterOutput::Triage` arm (around lines 311–394)**

```bash
grep -n "RouterOutput::Triage\|Priority::Medium" crates/surge-daemon/src/main.rs
```

- [ ] **Step 2: Extract the InboxCard-building + delivery logic into a helper at module scope**

After the existing `surge_runs_dir()` helper near the top of `main.rs`, add:

```rust
/// Deliver a Medium-priority placeholder InboxCard for `event`.
///
/// Used as the fallback path when (a) the source registry doesn't
/// know `event.source_id`, (b) `source.fetch_task` fails, or (c)
/// `dispatch_triage` returns `TriageError`. Preserves Plan-C-MVP
/// behaviour for unrecoverable provider errors.
async fn deliver_fallback_inbox(
    notifier: &Arc<dyn surge_notify::NotifyDeliverer>,
    event: &surge_intake::types::TaskEvent,
) {
    let title = event
        .raw_payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("New ticket")
        .to_string();
    let task_url = event
        .raw_payload
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let provider = event
        .task_id
        .as_str()
        .split(':')
        .next()
        .unwrap_or("unknown")
        .to_string();
    let run_id_str = ulid::Ulid::new().to_string();
    let payload = surge_notify::messages::InboxCardPayload {
        task_id: event.task_id.clone(),
        source_id: event.source_id.clone(),
        provider,
        title,
        summary: String::new(),
        priority: surge_intake::types::Priority::Medium,
        task_url,
        run_id: run_id_str.clone(),
    };
    let rendered_desktop = surge_notify::desktop::format_inbox_card_desktop(&payload);
    let rendered = surge_notify::RenderedNotification {
        severity: surge_core::notify_config::NotifySeverity::Info,
        title: rendered_desktop.title.clone(),
        body: rendered_desktop.body.clone(),
        artifact_paths: vec![],
    };
    let run_id = match run_id_str.parse::<surge_core::id::RunId>() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse run_id; skipping fallback delivery");
            return;
        }
    };
    let node_key = match surge_core::keys::NodeKey::try_new("intake") {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!(error = %e, "failed to construct intake NodeKey");
            return;
        }
    };
    let channel = surge_core::notify_config::NotifyChannel::Desktop;
    let ctx = surge_notify::NotifyDeliveryContext {
        run_id,
        node: &node_key,
    };
    match notifier.deliver(&ctx, &channel, &rendered).await {
        Ok(()) => tracing::info!(task_id = %event.task_id, "fallback InboxCard delivered"),
        Err(surge_notify::NotifyError::ChannelNotConfigured) => {
            tracing::debug!(task_id = %event.task_id, "Desktop channel not configured")
        }
        Err(e) => tracing::warn!(error = %e, task_id = %event.task_id, "fallback delivery failed"),
    }
}
```

- [ ] **Step 3: Build to confirm helper compiles in isolation**

```bash
cargo build -p surge-daemon
```

Expected: success (the function isn't called yet — that's Task 11).

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/main.rs
git commit -m "$(cat <<'EOF'
refactor(daemon): extract deliver_fallback_inbox helper

Pulls Medium-priority placeholder InboxCard logic out of the
RouterOutput::Triage arm. Reused in the next commit for two
distinct error paths (no source / fetch_task fail).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Daemon — replace placeholder with `dispatch_triage`

**Files:**
- Modify: `crates/surge-daemon/src/main.rs`

- [ ] **Step 1: Locate the exact `RouterOutput::Triage { event } => { ... }` arm**

```bash
grep -n "RouterOutput::Triage\|RouterOutput::EarlyDuplicate" crates/surge-daemon/src/main.rs
```

You'll see the arm starts around line 311 inside `tokio::spawn(async move { while let Some(out) = rx.recv().await { match out { ... } } })`.

- [ ] **Step 2: Determine what the consumer task captures**

Currently it captures `notifier`, `source_map_for_consumer`. We need to also capture `bridge` and `storage` (both are `Arc<...>` — clone them into the spawn block).

In `spawn_task_router`, accept `bridge` and `storage` as parameters and clone into the closure. Update the function signature:

```rust
async fn spawn_task_router(
    sources: Vec<Arc<dyn TaskSource>>,
    source_map: HashMap<String, Arc<dyn TaskSource>>,
    notifier: Arc<dyn surge_notify::NotifyDeliverer>,
    storage: Arc<Storage>,
    bridge: Arc<dyn surge_acp::bridge::facade::BridgeFacade>,
) {
```

And inside, when spawning the consumer:

```rust
let source_map_for_consumer = Arc::new(source_map);
let bridge_for_consumer = Arc::clone(&bridge);
let storage_for_consumer = Arc::clone(&storage);
let notifier_for_consumer = Arc::clone(&notifier);
tokio::spawn(async move {
    while let Some(out) = rx.recv().await {
        match out {
            // ... new Triage arm below ...
        }
    }
});
```

In `main`, update the call:

```rust
spawn_task_router(
    sources,
    source_map,
    Arc::clone(&notifier),
    Arc::clone(&storage),
    Arc::clone(&bridge),
).await;
```

- [ ] **Step 3: Replace the `RouterOutput::Triage` arm body**

Replace the existing block (lines 311–394) with:

```rust
surge_intake::router::RouterOutput::Triage { event } => {
    let Some(source) = source_map_for_consumer.get(&event.source_id).cloned() else {
        tracing::warn!(
            source_id = %event.source_id,
            "no source registered for triage event; falling back"
        );
        deliver_fallback_inbox(&notifier_for_consumer, &event).await;
        continue;
    };

    // Fetch full task details — raw_payload alone is too thin for Triage.
    let task_details = match source.fetch_task(&event.task_id).await {
        Ok(td) => td,
        Err(e) => {
            tracing::warn!(error = %e, task_id = %event.task_id, "fetch_task failed; falling back");
            deliver_fallback_inbox(&notifier_for_consumer, &event).await;
            continue;
        }
    };

    let candidates = surge_intake::candidates::build_for_task(&source, &task_details, 15)
        .await
        .unwrap_or_default();
    let active_run_rows = storage_for_consumer
        .snapshot_active_runs(32)
        .await
        .unwrap_or_default();
    let active_runs: Vec<surge_orchestrator::triage::ActiveRunSummary> = active_run_rows
        .into_iter()
        .map(|r| surge_orchestrator::triage::ActiveRunSummary {
            run_id: r.run_id,
            task_id: r.task_id,
            status: r.status,
            started_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(r.started_at_ms)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        })
        .collect();

    let input = surge_orchestrator::triage::TriageInput {
        task: task_details.clone(),
        candidates,
        active_runs,
    };
    let scratch_root = surge_runs_dir().join("intake").join("triage");
    let opts = surge_orchestrator::triage::TriageOptions::with_scratch_root(
        scratch_root,
        surge_orchestrator::triage::find_claude_binary(),
    );

    match surge_orchestrator::triage::dispatch_triage(
        Arc::clone(&bridge_for_consumer),
        input,
        opts,
    )
    .await
    {
        Err(e) => {
            tracing::warn!(error = %e, task_id = %event.task_id, "triage invariant failure; falling back");
            deliver_fallback_inbox(&notifier_for_consumer, &event).await;
        }
        Ok(surge_intake::types::TriageDecision::Enqueued { priority, summary, .. }) => {
            // Build a real InboxCardPayload using LLM-derived priority + summary.
            let provider = event
                .task_id
                .as_str()
                .split(':')
                .next()
                .unwrap_or("unknown")
                .to_string();
            let run_id_str = ulid::Ulid::new().to_string();
            let payload = surge_notify::messages::InboxCardPayload {
                task_id: event.task_id.clone(),
                source_id: event.source_id.clone(),
                provider,
                title: task_details.title.clone(),
                summary,
                priority,
                task_url: task_details.url.clone(),
                run_id: run_id_str.clone(),
            };
            let rendered_desktop = surge_notify::desktop::format_inbox_card_desktop(&payload);
            let rendered = surge_notify::RenderedNotification {
                severity: surge_core::notify_config::NotifySeverity::Info,
                title: rendered_desktop.title,
                body: rendered_desktop.body,
                artifact_paths: vec![],
            };
            let run_id = match run_id_str.parse::<surge_core::id::RunId>() {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(error = %e, "skipping delivery: bad run_id");
                    continue;
                }
            };
            let node_key = match surge_core::keys::NodeKey::try_new("intake") {
                Ok(k) => k,
                Err(e) => {
                    tracing::warn!(error = %e, "skipping delivery: bad NodeKey");
                    continue;
                }
            };
            let channel = surge_core::notify_config::NotifyChannel::Desktop;
            let ctx = surge_notify::NotifyDeliveryContext {
                run_id,
                node: &node_key,
            };
            match notifier_for_consumer.deliver(&ctx, &channel, &rendered).await {
                Ok(()) => tracing::info!(task_id = %event.task_id, priority = ?payload.priority, "InboxCard delivered (LLM-derived)"),
                Err(surge_notify::NotifyError::ChannelNotConfigured) => {
                    tracing::debug!(task_id = %event.task_id, "Desktop channel not configured")
                }
                Err(e) => tracing::warn!(error = %e, task_id = %event.task_id, "InboxCard delivery failed"),
            }
        }
        Ok(surge_intake::types::TriageDecision::Duplicate { of, reasoning }) => {
            let body = format!(
                "Surge: detected duplicate of {}. {}",
                of.as_str(),
                reasoning
            );
            if let Err(e) = source.post_comment(&event.task_id, &body).await {
                tracing::warn!(error = %e, task_id = %event.task_id, "duplicate comment post failed");
            } else {
                tracing::info!(task_id = %event.task_id, duplicate_of = %of, "duplicate comment posted");
            }
        }
        Ok(surge_intake::types::TriageDecision::OutOfScope { reasoning }) => {
            let body = format!("Surge: out of scope. {}", reasoning);
            if let Err(e) = source.post_comment(&event.task_id, &body).await {
                tracing::warn!(error = %e, task_id = %event.task_id, "out_of_scope comment post failed");
            } else {
                tracing::info!(task_id = %event.task_id, "out_of_scope comment posted");
            }
        }
        Ok(surge_intake::types::TriageDecision::Unclear { question }) => {
            let rendered = surge_notify::RenderedNotification {
                severity: surge_core::notify_config::NotifySeverity::Warn,
                title: format!("Triage unclear · {}", event.task_id.as_str()),
                body: question,
                artifact_paths: vec![],
            };
            let run_id_str = ulid::Ulid::new().to_string();
            let run_id = match run_id_str.parse::<surge_core::id::RunId>() {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(error = %e, "skipping unclear delivery: bad run_id");
                    continue;
                }
            };
            let node_key = match surge_core::keys::NodeKey::try_new("intake") {
                Ok(k) => k,
                Err(e) => {
                    tracing::warn!(error = %e, "skipping unclear delivery: bad NodeKey");
                    continue;
                }
            };
            let channel = surge_core::notify_config::NotifyChannel::Desktop;
            let ctx = surge_notify::NotifyDeliveryContext {
                run_id,
                node: &node_key,
            };
            match notifier_for_consumer.deliver(&ctx, &channel, &rendered).await {
                Ok(()) => tracing::info!(task_id = %event.task_id, "Unclear notification delivered"),
                Err(surge_notify::NotifyError::ChannelNotConfigured) => {
                    tracing::debug!(task_id = %event.task_id, "Desktop channel not configured")
                }
                Err(e) => tracing::warn!(error = %e, task_id = %event.task_id, "Unclear delivery failed"),
            }
        }
    }
}
```

Keep the existing `surge_intake::router::RouterOutput::EarlyDuplicate { event, run_id }` arm unchanged.

- [ ] **Step 4: Build the daemon**

```bash
cargo build -p surge-daemon
```

Expected: success.

If `Storage` import is missing, add `use surge_persistence::runs::Storage;` near top of main.rs.

- [ ] **Step 5: Run the existing daemon tests to confirm no regression**

```bash
cargo test -p surge-daemon
```

Expected: all green (existing tests don't exercise the Triage arm).

- [ ] **Step 6: Commit**

```bash
git add crates/surge-daemon/src/main.rs
git commit -m "$(cat <<'EOF'
feat(daemon): wire dispatch_triage to RouterOutput::Triage

Replaces the Priority::Medium placeholder with a real ACP-driven
Triage Author call. Routes the four TriageDecision variants:
- Enqueued → InboxCard with LLM-derived priority + summary
- Duplicate → tracker comment, no inbox
- OutOfScope → tracker comment, no inbox
- Unclear → Warn-severity desktop notification

Falls back to deliver_fallback_inbox on (a) unknown source_id, (b)
fetch_task failure, (c) TriageError invariant violation. Preserves
Plan-C-MVP behaviour for unrecoverable provider errors.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Daemon smoke test

**Files:**
- Create: `crates/surge-daemon/tests/triage_wiring.rs`
- Confirm: `crates/surge-daemon/Cargo.toml` `[dev-dependencies]` has `surge-orchestrator` (likely missing — add)

- [ ] **Step 1: Add surge-orchestrator and tempfile to daemon dev-deps**

```bash
grep -n "surge-orchestrator\|tempfile\|surge-intake" crates/surge-daemon/Cargo.toml
```

Add under `[dev-dependencies]`:

```toml
surge-orchestrator = { workspace = true }
tempfile = { workspace = true }
```

(if any are absent — be additive, don't replace).

- [ ] **Step 2: Write the test**

Create `crates/surge-daemon/tests/triage_wiring.rs`:

```rust
//! Daemon smoke test: end-to-end wiring of dispatch_triage into the
//! `RouterOutput::Triage` consumer. Uses MockBridge + MockTaskSource
//! to validate the InboxCardPayload constructed for an Enqueued
//! decision carries a non-Medium (LLM-derived) priority.
//!
//! This test does NOT exercise the consumer task in `main.rs`
//! directly (it's tightly coupled to daemon lifecycle); instead we
//! reproduce the four-arm match logic against an in-process channel.
//! The cost is some duplication of branching, but it catches
//! signature drift between dispatch_triage and the daemon's expected
//! inputs.

use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::{OutcomeKey, SessionId};
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{TaskDetails, TaskId, Priority, TriageDecision};
use tempfile::TempDir;
use std::str::FromStr;

#[path = "../../surge-orchestrator/tests/fixtures/mod.rs"]
mod fixtures;

#[tokio::test]
async fn triage_enqueued_yields_real_priority() {
    // Arrange: mock task source with one task.
    let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
    let task = TaskDetails {
        task_id: TaskId::try_new("mock:t#1").unwrap(),
        source_id: "mock:t".into(),
        title: "Fix parser panic".into(),
        description: "Stack overflow".into(),
        status: "open".into(),
        labels: vec!["surge:enabled".into()],
        url: "https://x/1".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    };
    src.put_task(task.clone()).await;

    // Mock bridge scripted to return Enqueued{priority: Urgent}.
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let scratch_root = tmp.path().to_path_buf();
    let bridge_drive = Arc::clone(&bridge);
    let drive = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        let scratch = std::fs::read_dir(&scratch_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .unwrap()
            .path();
        std::fs::write(
            scratch.join("triage_decision.json"),
            r#"{"decision":"enqueued","priority":"urgent","priority_reasoning":"prod crash","summary":"hot fix"}"#,
        ).unwrap();
        bridge_drive
            .enqueue_event(BridgeEvent::OutcomeReported {
                session,
                outcome: OutcomeKey::from_str("enqueued").unwrap(),
                summary: "agent".into(),
                artifacts_produced: vec!["triage_decision.json".into()],
            })
            .await;
        bridge_drive.pump_scripted_events().await;
    });

    let opts = surge_orchestrator::triage::TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };
    let input = surge_orchestrator::triage::TriageInput {
        task: task.clone(),
        candidates: vec![],
        active_runs: vec![],
    };

    let result = surge_orchestrator::triage::dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input,
        opts,
    )
    .await
    .unwrap();
    drive.await.unwrap();

    // Assert: not Medium, and priority round-trips into payload.
    match result {
        TriageDecision::Enqueued { priority, .. } => {
            assert_eq!(priority, Priority::Urgent);
            assert_ne!(priority, Priority::Medium, "must NOT regress to Medium placeholder");
        }
        other => panic!("expected Enqueued, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run the test**

```bash
cargo test -p surge-daemon --test triage_wiring
```

Expected: `1 passed`.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/Cargo.toml crates/surge-daemon/tests/triage_wiring.rs
git commit -m "$(cat <<'EOF'
test(daemon): smoke test for triage Enqueued → real priority

Validates the wiring without daemon lifecycle: MockTaskSource +
MockBridge scripted to Enqueued{Urgent} → dispatch_triage returns
Urgent, asserting we no longer regress to the Medium placeholder.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Feature-gated LLM E2E test

**Files:**
- Modify: `crates/surge-orchestrator/Cargo.toml`
- Modify: `crates/surge-orchestrator/tests/triage_llm.rs`

- [ ] **Step 1: Confirm feature flag exists**

```bash
grep -n "_bootstrap_llm_test\|\\[features\\]" crates/surge-orchestrator/Cargo.toml
```

If absent, add to `[features]`:

```toml
[features]
_bootstrap_llm_test = []
```

- [ ] **Step 2: Replace `triage_llm.rs` body with feature-gated real-LLM test**

Open `crates/surge-orchestrator/tests/triage_llm.rs`. The file currently has a smoke that confirms fixtures parse. Keep that smoke (it runs in default builds) and add a feature-gated module that runs the real LLM:

```rust
//! Fixture-driven test for Triage Author. Two flavours:
//!
//! 1. Default: smoke check that fixtures parse as TOML.
//! 2. `--features _bootstrap_llm_test`: dispatch real Claude Haiku
//!    against the fixtures and assert decision matches.
//!
//! Run feature-gated:
//!   ANTHROPIC_TEST_KEY=sk-... cargo test -p surge-orchestrator \
//!     --test triage_llm --features _bootstrap_llm_test -- --ignored

#[test]
fn fixtures_compile() {
    use std::fs;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/triage_fixtures");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("fixtures dir") {
        let p = entry.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let contents = fs::read_to_string(&p).unwrap();
        let _: toml::Value = toml::from_str(&contents).expect("valid TOML");
        count += 1;
    }
    assert!(count >= 3, "expected at least 3 fixtures, found {count}");
}

#[cfg(feature = "_bootstrap_llm_test")]
mod llm {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use surge_acp::bridge::acp_bridge::AcpBridge;
    use surge_acp::bridge::facade::BridgeFacade;
    use surge_intake::types::{Priority, TaskDetails, TaskId, TaskSummary, TriageDecision};
    use surge_orchestrator::triage::{dispatch_triage, TriageInput, TriageOptions};

    fn priority_distance(a: Priority, b: Priority) -> u32 {
        let rank = |p: Priority| -> u32 {
            match p {
                Priority::Low => 0,
                Priority::Medium => 1,
                Priority::High => 2,
                Priority::Urgent => 3,
            }
        };
        rank(a).abs_diff(rank(b))
    }

    #[derive(serde::Deserialize)]
    struct Fixture {
        input: FixtureInput,
        expected: FixtureExpected,
    }
    #[derive(serde::Deserialize)]
    struct FixtureInput {
        task: TaskDetails,
        #[serde(default)]
        candidates: Vec<TaskSummary>,
    }
    #[derive(serde::Deserialize)]
    struct FixtureExpected {
        decision: String,
        #[serde(default)]
        priority: Option<String>,
        #[serde(default)]
        duplicate_of: Option<String>,
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires ANTHROPIC_TEST_KEY and a real claude binary"]
    async fn fixtures_against_real_haiku() {
        let bridge = Arc::new(AcpBridge::with_defaults().expect("AcpBridge"));
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/triage_fixtures");

        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).unwrap();
            let fixture: Fixture = toml::from_str(&raw).unwrap();

            let input = TriageInput {
                task: fixture.input.task,
                candidates: fixture.input.candidates,
                active_runs: vec![],
            };
            let tmp = tempfile::tempdir().unwrap();
            let opts = TriageOptions {
                claude_binary: surge_orchestrator::triage::find_claude_binary(),
                attempt_timeout: Duration::from_secs(180),
                max_attempts: 1,
                scratch_root: tmp.path().to_path_buf(),
                keep_scratch_on_failure: true,
            };
            let result = dispatch_triage(
                Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
                input,
                opts,
            )
            .await
            .expect("dispatch_triage");

            let actual_decision = match &result {
                TriageDecision::Enqueued { .. } => "enqueued",
                TriageDecision::Duplicate { .. } => "duplicate",
                TriageDecision::OutOfScope { .. } => "out_of_scope",
                TriageDecision::Unclear { .. } => "unclear",
            };
            assert_eq!(
                actual_decision, fixture.expected.decision,
                "fixture {:?}: decision mismatch", path
            );

            if let (Some(exp_p), TriageDecision::Enqueued { priority, .. }) =
                (fixture.expected.priority.as_deref(), &result)
            {
                let exp = match exp_p {
                    "urgent" => Priority::Urgent,
                    "high" => Priority::High,
                    "medium" => Priority::Medium,
                    "low" => Priority::Low,
                    other => panic!("fixture has unknown priority: {other}"),
                };
                let dist = priority_distance(exp, *priority);
                assert!(
                    dist <= 1,
                    "fixture {:?}: priority {:?} too far from expected {:?}",
                    path, priority, exp
                );
            }

            if let (Some(exp_dup), TriageDecision::Duplicate { of, .. }) =
                (fixture.expected.duplicate_of.as_deref(), &result)
            {
                assert_eq!(of.as_str(), exp_dup, "fixture {:?}: duplicate_of mismatch", path);
            }
        }
    }
}
```

- [ ] **Step 3: Verify default build still passes**

```bash
cargo test -p surge-orchestrator --test triage_llm
```

Expected: smoke `fixtures_compile` passes; LLM module not compiled.

- [ ] **Step 4: Verify feature build compiles (without running)**

```bash
cargo build -p surge-orchestrator --tests --features _bootstrap_llm_test
```

Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/Cargo.toml crates/surge-orchestrator/tests/triage_llm.rs
git commit -m "$(cat <<'EOF'
test(orchestrator): feature-gated LLM E2E for triage fixtures

Adds --features _bootstrap_llm_test that runs real Claude Haiku
against the three triage fixtures. Decision must match exactly;
priority is allowed ±1 step. Default workspace test still runs the
fixtures-parse smoke check only — no API key required.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Final QA — clippy, fmt, roadmap

**Files:**
- Modify: `docs/03-ROADMAP.md`

- [ ] **Step 1: Run workspace build**

```bash
cargo build --workspace
```

Expected: success.

- [ ] **Step 2: Run workspace clippy with `-D warnings`**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: no warnings.

If clippy fires on the new code, fix inline. Common likely culprits:
- `clippy::needless_pass_by_value` — change `Arc<dyn …>` parameter to `&Arc<dyn …>` in helpers if flagged.
- `clippy::missing_errors_doc` — add `# Errors` doc to public functions returning `Result`.

- [ ] **Step 3: Run full workspace test**

```bash
cargo test --workspace
```

Expected: all green.

- [ ] **Step 4: Run fmt**

```bash
task fmt 2>/dev/null || cargo +nightly fmt --all || cargo fmt --all
```

If `task fmt` requires nightly rustfmt and isn't available, document this in the commit message and fall back to `cargo fmt --all` (stable).

- [ ] **Step 5: Update `docs/03-ROADMAP.md`**

Open `docs/03-ROADMAP.md` and locate the Plan-C-polish section (around the line with "5 of 6"). Strike the deferred line and update the status:

```bash
grep -n "5 of 6\|Triage Author LLM dispatch\|Plan-C-polish" docs/03-ROADMAP.md | head
```

Edit:

- Change `## RFC-0010 — Plan-C-polish ✅ (5 of 6)` → `## RFC-0010 — Plan-C-polish ✅ (6 of 6)`.
- Move the "Triage Author LLM dispatch via ACP" item from "Remaining (deferred to its own session)" to the completed list with a checkbox `[x]` and a one-line note linking to the design spec and this plan.

```markdown
- [x] **Triage Author LLM dispatch via ACP** — Replaces the
      `Priority::Medium` placeholder in surge-daemon with a real
      ACP-driven Triage Author call. File-artifact return path
      (`triage_decision.json` + `inbox_summary.md`) matching the
      Description Author pattern. Layer 1 of two-layer plan; Layer 2
      promotes Triage to a graph node (separate RFC).
      See: [docs/superpowers/specs/2026-05-06-triage-author-llm-dispatch-design.md](superpowers/specs/2026-05-06-triage-author-llm-dispatch-design.md).
```

- [ ] **Step 6: Commit**

```bash
git add docs/03-ROADMAP.md
git commit -m "$(cat <<'EOF'
docs(rfc-0010): mark Plan-C-polish #6 (Triage LLM dispatch) complete

All 6 Plan-C-polish items now shipped. RFC-0010 acceptance criterion
#3 fully passes (priority is LLM-derived). Layer 2 (Triage as graph
node) tracked separately as a future RFC.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification

- [ ] **Step 1: Verify everything green**

```bash
cargo build --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo test --workspace
```

Expected: all green.

- [ ] **Step 2: Verify the LLM-gated path compiles independently**

```bash
cargo build -p surge-orchestrator --tests --features _bootstrap_llm_test
```

Expected: success.

- [ ] **Step 3: Print the diff summary**

```bash
git log --oneline main..HEAD
git diff --stat main..HEAD
```

Expected: ~14 commits, ~640 LOC across `surge-intake`, `surge-persistence`, `surge-orchestrator`, `surge-daemon`, plus the docs update.

---

## Plan self-review

**1. Spec coverage:**

| Spec section | Implemented in |
|---|---|
| §1.1 dispatch_triage async function | T3 (types), T4 (skeleton + happy path) |
| §1.1 file-artifact return path | T4 (read scratch_dir/{decision.json,summary.md}) |
| §1.1 retry semantics | T6 (bad JSON), T7 (timeout, agent crash, binary missing) |
| §1.1 daemon mapping (4 arms) | T11 (Enqueued/Duplicate/OutOfScope/Unclear) |
| §1.1 candidate-set assembly | T1 (`candidates::build_for_task`) |
| §1.1 active-runs snapshot | T2 (`Storage::snapshot_active_runs`) |
| §1.1 explicit handling missing claude binary | T9 (`find_claude_binary`) + T7 (binary_missing test) |
| §1.1 feature-gated LLM E2E | T13 |
| §3.5 deliver_fallback_inbox helper | T10 (extraction) + T11 (use sites) |
| §4.1 prompt rendering | T4 (`render_prompt` impl) |
| §4.3 event loop | T4 (try_one_attempt impl) |
| §5.1 retry table | T6 + T7 (covers timeout/crash/bad-json/missing-file) |
| §5.2 final fallback (Unclear) | T6 (exhaustion test), T7 (timeout/crash tests) |
| §5.3 scratch dir lifecycle | T4 (cleanup on success), T7 (keep_on_failure verification) |
| §5.4 daemon-side fallback | T10 + T11 (deliver_fallback_inbox calls on errors) |
| §6.1 unit tests on mock | T4, T5, T6, T7 (9 tests total) |
| §6.2 prompt snapshot test | T8 |
| §6.3 LLM E2E feature-gated | T13 |
| §6.4 daemon smoke | T12 |
| §10 acceptance criteria | T14 (clippy + fmt + roadmap) |

All spec sections covered. **Forward-compat (§7)** is naturally preserved by file artifacts + standard `report_stage_outcome` flow — no specific task needed.

**2. Placeholder scan:** No "TBD", "TODO", or "fill in details" in the plan. Each step has actual code. The one place where I write "iterate" is in T4 step 2's harness skeleton — it's annotated with the cleaner pattern immediately below, so the engineer reading it sees both the rough shape and the concrete refactor target.

**3. Type consistency:**
- `dispatch_triage(bridge: Arc<dyn BridgeFacade>, input: TriageInput, opts: TriageOptions) -> Result<TriageDecision, TriageError>` — consistent across T3 (signature), T4 (impl), T11 (call site), T12 (smoke), T13 (E2E).
- `TriageOptions::with_scratch_root(scratch_root, claude_binary)` — used in T11.
- `find_claude_binary() -> Option<PathBuf>` — defined T9, used T11, T13.
- `ActiveRunRow { run_id, task_id, status, started_at_ms }` — defined T2, mapped to `ActiveRunSummary` in T11 via `from_timestamp_millis(...).to_rfc3339()`.
- `MockBridge::pin_session_ids(Vec<SessionId>)` — added T6, used T6/T7.
- `deliver_fallback_inbox(notifier, event)` — defined T10, called T11 (three sites).

All names match across tasks.

**4. Scope check:** Layer 1 only. Layer 2 (Triage as graph node) is explicitly out of scope per spec §1.2. Plan does not touch `RunState`, `EventPayload`, or engine state machine.

---
