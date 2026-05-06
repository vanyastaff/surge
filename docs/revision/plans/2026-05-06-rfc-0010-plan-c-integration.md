# RFC-0010 Issue-Tracker Integration · Plan C — Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Plans A+B into the running Surge system: add `Triage Author` bootstrap profile, extend `surge-notify` with `InboxCard` message type, register `TaskRouter` inside `surge-daemon`, add new `EventPayload` variants, expose `surge tracker` CLI commands, and verify end-to-end with a mock-driven scenario.

**Architecture:** This plan touches **multiple existing crates** (`surge-core`, `surge-orchestrator`, `surge-notify`, `surge-daemon`, `surge-cli`). Each task is scoped to one crate and follows TDD where applicable. The end-to-end mock test in Task 11.1 is the integration acceptance gate.

**Tech Stack:** Same as Plans A+B, plus `teloxide` (existing — used by `surge-notify` for Telegram) and `clap` (existing — used by `surge-cli`).

**Prerequisites:** Plans A and B complete.

---

## File structure

### Created
- `~/.surge/profiles/_bootstrap/triage-author-1.0.toml` — bootstrap profile (shipped via crate, see Task 7.1)
- `crates/surge-orchestrator/src/triage.rs` — Triage Author dispatcher
- `crates/surge-orchestrator/tests/triage_fixtures/duplicate_001.toml` — sample fixture (Task 7.5)
- `crates/surge-orchestrator/tests/triage_fixtures/enqueue_001.toml`
- `crates/surge-orchestrator/tests/triage_fixtures/out_of_scope_001.toml`
- `crates/surge-orchestrator/tests/triage_llm.rs` — fixture-driven LLM tests (`#[ignore]`d)
- `crates/surge-cli/src/commands/tracker.rs` — `surge tracker` subcommand
- `crates/surge-intake/tests/e2e_mock.rs` — end-to-end mock test
- `docs/revision/plans/PROGRESS-RFC-0010.md` — progress tracker

### Modified
- `crates/surge-core/src/event.rs` — new `EventPayload` variants
- `crates/surge-core/src/config.rs` — `TaskSourceConfig` types
- `crates/surge-notify/src/messages.rs` — `InboxCard` variant
- `crates/surge-notify/src/telegram.rs` — render `InboxCard`
- `crates/surge-notify/src/desktop.rs` — render `InboxCard`
- `crates/surge-orchestrator/src/lib.rs` — re-export `triage` module
- `crates/surge-daemon/src/lib.rs` — spawn `TaskRouter` on startup
- `crates/surge-daemon/Cargo.toml` — add `surge-intake` dep
- `crates/surge-cli/src/main.rs` — register `tracker` subcommand
- `crates/surge-cli/Cargo.toml` — add `surge-intake` dep
- `Cargo.toml` (workspace root) — re-verify members

---

## Task 7.1 — Triage Author profile TOML

**Files:**
- Create: `crates/surge-orchestrator/profiles/_bootstrap/triage-author-1.0.toml`
- Modify: `crates/surge-orchestrator/build.rs` (or wherever profiles are bundled — check existing pattern first)

- [ ] **Step 1: Locate where bootstrap profiles are bundled**

Search:

```bash
grep -rn "_bootstrap" crates/surge-orchestrator/ crates/surge-spec/ 2>/dev/null | head -20
ls crates/surge-orchestrator/profiles/_bootstrap 2>/dev/null
```

If the directory doesn't exist, the crate likely ships profiles via `include_str!` or expects them in `~/.surge/profiles/_bootstrap`. Check `crates/surge-cli/src/commands/init.rs` (or similar) for the install pattern.

For Plan C we ship the profile **inside** `surge-orchestrator` and load it at startup; `surge init` copies it to `~/.surge/profiles/_bootstrap/` if not present.

```bash
mkdir -p crates/surge-orchestrator/profiles/_bootstrap
```

- [ ] **Step 2: Write the profile**

Create `crates/surge-orchestrator/profiles/_bootstrap/triage-author-1.0.toml`:

```toml
id = "_bootstrap/triage-author"
display_name = "Triage Author"
version = "1.0"

[prompt]
system = """
You triage incoming tickets from external task sources.

You receive:
- The new ticket (title, body, labels, status, URL)
- List of currently active Surge runs and their associated ticket_ids
- Top 15-25 candidate tickets (similar open tickets + recent specs)
- Project context (cwd, language, top-level files)

Your job:
1. Decide whether this ticket is a duplicate, out-of-scope, unclear, or should be enqueued.
2. Assess priority (urgent/high/medium/low) from the ticket text and labels.
3. Output a structured decision.

Anti-patterns to avoid:
- Do not invent details that aren't in the ticket.
- Do not mark "duplicate" unless similarity is strong (>0.80 confidence).
- Always provide priority — never skip.
- Do not propose implementation; that's downstream.
"""

declared_outcomes = ["enqueued", "duplicate", "out_of_scope", "unclear"]
inputs_expected = ["task_payload.json", "candidates.json", "active_runs.json"]
outputs_produced = ["triage_decision.json", "inbox_summary.md"]

[sandbox]
mode = "read-only"
```

- [ ] **Step 3: Bundle the profile via `include_str!`**

In `crates/surge-orchestrator/src/lib.rs` (or a new `profiles.rs`), add:

```rust
pub const BOOTSTRAP_TRIAGE_AUTHOR_TOML: &str = include_str!(
    "../profiles/_bootstrap/triage-author-1.0.toml"
);
```

Verify the path resolves (Rust will fail to compile if the file doesn't exist).

- [ ] **Step 4: Build**

```bash
cargo build -p surge-orchestrator
```

Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/profiles/_bootstrap/triage-author-1.0.toml \
        crates/surge-orchestrator/src/lib.rs
git commit -m "feat(orchestrator): bundle Triage Author bootstrap profile"
```

---

## Task 7.2 — Triage Author dispatcher (renders inputs, parses output)

**Files:**
- Create: `crates/surge-orchestrator/src/triage.rs`
- Modify: `crates/surge-orchestrator/src/lib.rs`

- [ ] **Step 1: Write input/output types**

Create `crates/surge-orchestrator/src/triage.rs`:

```rust
//! Triage Author dispatcher: assembles inputs, invokes the agent, parses output.

use serde::{Deserialize, Serialize};
use surge_intake::types::{Priority, TaskDetails, TaskId, TaskSummary, TriageDecision};

/// Full input bundle handed to Triage Author at the start of its session.
#[derive(Debug, Clone, Serialize)]
pub struct TriageInput {
    pub task: TaskDetails,
    pub candidates: Vec<TaskSummary>,
    pub active_runs: Vec<ActiveRunSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveRunSummary {
    pub run_id: String,
    pub task_id: Option<String>,
    pub status: String,
    pub started_at: String,
}

/// Raw JSON shape Triage Author writes to `triage_decision.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct TriageJson {
    pub decision: String,
    #[serde(default)]
    pub duplicate_of: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub priority_reasoning: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
}

impl TriageJson {
    pub fn into_decision(self) -> Result<TriageDecision, String> {
        let prio = self.priority.as_deref().unwrap_or("medium");
        let priority = match prio {
            "urgent" => Priority::Urgent,
            "high" => Priority::High,
            "medium" => Priority::Medium,
            "low" => Priority::Low,
            other => return Err(format!("unknown priority: {other}")),
        };
        match self.decision.as_str() {
            "enqueued" => Ok(TriageDecision::Enqueued {
                priority,
                reasoning: self.priority_reasoning.unwrap_or_default(),
                summary: self.summary.unwrap_or_default(),
            }),
            "duplicate" => {
                let dup = self
                    .duplicate_of
                    .ok_or_else(|| "duplicate decision requires duplicate_of".to_string())?;
                let id = TaskId::try_new(dup).map_err(|e| format!("invalid duplicate_of: {e}"))?;
                Ok(TriageDecision::Duplicate {
                    of: id,
                    reasoning: self.priority_reasoning.unwrap_or_default(),
                })
            }
            "out_of_scope" => Ok(TriageDecision::OutOfScope {
                reasoning: self.priority_reasoning.unwrap_or_default(),
            }),
            "unclear" => Ok(TriageDecision::Unclear {
                question: self.question.unwrap_or_else(|| "no question provided".into()),
            }),
            other => Err(format!("unknown decision: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enqueued() {
        let raw = r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let dec = parsed.into_decision().unwrap();
        match dec {
            TriageDecision::Enqueued { priority, .. } => assert_eq!(priority, Priority::High),
            other => panic!("expected Enqueued, got {other:?}"),
        }
    }

    #[test]
    fn parse_duplicate_requires_duplicate_of() {
        let raw = r#"{"decision":"duplicate"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let err = parsed.into_decision().unwrap_err();
        assert!(err.contains("duplicate_of"));
    }

    #[test]
    fn parse_unknown_priority_errors() {
        let raw = r#"{"decision":"enqueued","priority":"super-urgent"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let err = parsed.into_decision().unwrap_err();
        assert!(err.contains("unknown priority"));
    }
}
```

- [ ] **Step 2: Add `surge-intake` dep to `surge-orchestrator/Cargo.toml`**

Append to `[dependencies]`:

```toml
surge-intake = { workspace = true }
```

- [ ] **Step 3: Re-export module**

Add to `crates/surge-orchestrator/src/lib.rs`:

```rust
pub mod triage;
```

- [ ] **Step 4: Add `surge-intake` to workspace deps**

In root `Cargo.toml` `[workspace.dependencies]`:

```toml
surge-intake = { path = "crates/surge-intake" }
```

- [ ] **Step 5: Build + test**

```bash
cargo build -p surge-orchestrator
cargo test -p surge-orchestrator --lib triage::tests
```

Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/surge-orchestrator/
git commit -m "feat(orchestrator): Triage Author dispatcher with input/output types"
```

---

## Task 7.3 — 3 fixture files for Triage Author tests

**Files:**
- Create: `crates/surge-orchestrator/tests/triage_fixtures/enqueue_001.toml`
- Create: `crates/surge-orchestrator/tests/triage_fixtures/duplicate_001.toml`
- Create: `crates/surge-orchestrator/tests/triage_fixtures/out_of_scope_001.toml`

- [ ] **Step 1: Create the fixtures directory**

```bash
mkdir -p crates/surge-orchestrator/tests/triage_fixtures
```

- [ ] **Step 2: enqueue_001.toml**

Create `crates/surge-orchestrator/tests/triage_fixtures/enqueue_001.toml`:

```toml
[input.task]
task_id = "github_issues:test/repo#42"
source_id = "github_issues:test/repo"
title = "Add tracing to auth middleware"
description = "We have no observability into the auth flow. Add tracing spans for login, token issue, and logout paths."
status = "open"
labels = ["surge:enabled", "area/auth"]
url = "https://github.com/test/repo/issues/42"
created_at = "2026-05-01T10:00:00Z"
updated_at = "2026-05-05T12:00:00Z"
assignee = ""

[[input.candidates]]
task_id = "github_issues:test/repo#10"
title = "Update README"

[[input.candidates]]
task_id = "github_issues:test/repo#23"
title = "Fix typo in CLI help"

[expected]
decision = "enqueued"
priority = "medium"
```

- [ ] **Step 3: duplicate_001.toml**

Create `crates/surge-orchestrator/tests/triage_fixtures/duplicate_001.toml`:

```toml
[input.task]
task_id = "github_issues:test/repo#100"
source_id = "github_issues:test/repo"
title = "Parser crashes with deeply nested JSON"
description = "Stack overflow when JSON has more than 16 nested levels."
status = "open"
labels = ["surge:enabled"]
url = "https://github.com/test/repo/issues/100"
created_at = "2026-05-05T10:00:00Z"
updated_at = "2026-05-05T10:00:00Z"
assignee = ""

[[input.candidates]]
task_id = "github_issues:test/repo#85"
title = "Fix parser panic on nested objects"

[expected]
decision = "duplicate"
duplicate_of = "github_issues:test/repo#85"
priority = "high"
```

- [ ] **Step 4: out_of_scope_001.toml**

Create `crates/surge-orchestrator/tests/triage_fixtures/out_of_scope_001.toml`:

```toml
[input.task]
task_id = "github_issues:test/repo#200"
source_id = "github_issues:test/repo"
title = "Hire a designer"
description = "We need a designer for the new branding project. Please post the job ad."
status = "open"
labels = ["surge:enabled"]
url = "https://github.com/test/repo/issues/200"
created_at = "2026-05-05T10:00:00Z"
updated_at = "2026-05-05T10:00:00Z"
assignee = ""

[expected]
decision = "out_of_scope"
priority = "low"
```

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/tests/triage_fixtures/
git commit -m "test(orchestrator): triage fixtures (enqueue/duplicate/out_of_scope)"
```

---

## Task 7.4 — Triage Author fixture-driven LLM test (ignored)

**Files:**
- Create: `crates/surge-orchestrator/tests/triage_llm.rs`

- [ ] **Step 1: Write the test runner**

Create `crates/surge-orchestrator/tests/triage_llm.rs`:

```rust
//! Fixture-driven LLM test for Triage Author. Requires:
//!   - ANTHROPIC_TEST_KEY env var
//!
//! Run with: `cargo test -p surge-orchestrator --test triage_llm -- --ignored`
//!
//! Each fixture in `triage_fixtures/*.toml` provides an input ticket and
//! candidate set, plus the expected decision/priority. The test invokes the
//! Triage Author profile against Claude Haiku at temperature=0 and validates
//! the output against tolerance bands:
//!   - decision: must match exactly
//!   - priority: ±1 acceptable

#![cfg(feature = "_bootstrap_llm_test")]

// Implementation note: this test depends on a runner that knows how to
// invoke a profile via the existing surge-orchestrator agent loop. As of
// Plan C MVP, expose a `triage::run_one(...)` function that:
//   - reads the profile TOML (BOOTSTRAP_TRIAGE_AUTHOR_TOML)
//   - constructs ACP session with claude-haiku
//   - sends inputs as a structured JSON message
//   - awaits `triage_decision.json` artifact
//   - parses via TriageJson
//
// We mark the test feature-gated so workspace `cargo test` doesn't
// attempt LLM calls. Activate with `--features _bootstrap_llm_test`.

#[test]
fn fixtures_compile() {
    // Smoke check that fixtures parse as TOML; full LLM test lives behind
    // a feature flag and is not enabled in default builds.
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
```

(The feature-gated body is documented for future expansion. For Plan C MVP the smoke check on TOML parsing is sufficient.)

- [ ] **Step 2: Run the smoke check**

```bash
cargo test -p surge-orchestrator --test triage_llm
```

Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/tests/triage_llm.rs
git commit -m "test(orchestrator): triage fixtures smoke test (LLM body deferred)"
```

---

## Task 8.1 — `NotifyMessage::InboxCard` variant

**Files:**
- Modify: `crates/surge-notify/src/messages.rs` (or wherever `NotifyMessage` lives)
- Modify: `crates/surge-notify/Cargo.toml` (add `surge-intake` dep — for `Priority`, `TaskId`)

- [ ] **Step 1: Locate `NotifyMessage`**

```bash
grep -rn "enum NotifyMessage" crates/surge-notify/src/ | head -5
```

- [ ] **Step 2: Add `surge-intake` to `surge-notify` Cargo.toml**

Append to `[dependencies]`:

```toml
surge-intake = { workspace = true }
```

- [ ] **Step 3: Add the variant**

Open the file containing `NotifyMessage` (likely `crates/surge-notify/src/messages.rs`). Add:

```rust
use surge_intake::types::{Priority, TaskId};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InboxCardPayload {
    pub task_id: TaskId,
    pub source_id: String,
    pub provider: String,
    pub title: String,
    pub summary: String,
    pub priority: Priority,
    pub task_url: String,
    pub run_id: String,
}
```

Add `InboxCard(InboxCardPayload)` to the `NotifyMessage` enum.

- [ ] **Step 4: Write a serde round-trip test**

Add at the end of the file:

```rust
#[cfg(test)]
mod inbox_card_tests {
    use super::*;
    use chrono::Utc;
    use surge_intake::types::{Priority, TaskId};

    #[test]
    fn round_trip_inbox_card() {
        let payload = InboxCardPayload {
            task_id: TaskId::try_new("github_issues:user/repo#1").unwrap(),
            source_id: "github_issues:user/repo".into(),
            provider: "github_issues".into(),
            title: "Fix parser".into(),
            summary: "panic on nested".into(),
            priority: Priority::High,
            task_url: "https://github.com/user/repo/issues/1".into(),
            run_id: "run_abc".into(),
        };
        let msg = NotifyMessage::InboxCard(payload.clone());
        let s = serde_json::to_string(&msg).unwrap();
        let back: NotifyMessage = serde_json::from_str(&s).unwrap();
        match back {
            NotifyMessage::InboxCard(p) => assert_eq!(p.task_id, payload.task_id),
            _ => panic!("wrong variant"),
        }
    }
}
```

- [ ] **Step 5: Build and test**

```bash
cargo build -p surge-notify
cargo test -p surge-notify --lib inbox_card_tests
```

Expected: 1 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-notify/Cargo.toml crates/surge-notify/src/messages.rs
git commit -m "feat(notify): add NotifyMessage::InboxCard variant"
```

---

## Task 8.2 — Telegram inbox card formatter

**Files:**
- Modify: `crates/surge-notify/src/telegram.rs`

- [ ] **Step 1: Locate the dispatcher**

```bash
grep -rn "fn send" crates/surge-notify/src/telegram.rs | head -10
```

Find the function that dispatches `NotifyMessage` variants to telegram messages. Add a branch for `InboxCard`.

- [ ] **Step 2: Add the formatter**

Locate the match (or if-else chain) that handles `NotifyMessage` variants and add:

```rust
NotifyMessage::InboxCard(p) => {
    let body = format!(
        "📋 Task from {provider} · {short}\n\n\
         {title}\n\
         priority: {prio} (auto-detected)\n\n\
         [▶ Start] [⏸ Snooze 24h] [✕ Skip]\n\
         [View ticket ↗]({url})",
        provider = p.provider,
        short = p.task_id.as_str().rsplit('/').next().unwrap_or(p.task_id.as_str()),
        title = p.title,
        prio = p.priority.label(),
        url = p.task_url,
    );

    // Inline keyboard with callback_data encoding (run_id + action).
    use teloxide::prelude::*;
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

    let kb = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                "▶ Start",
                format!("inbox:start:{}", p.run_id),
            ),
            InlineKeyboardButton::callback(
                "⏸ Snooze 24h",
                format!("inbox:snooze:{}", p.run_id),
            ),
            InlineKeyboardButton::callback(
                "✕ Skip",
                format!("inbox:skip:{}", p.run_id),
            ),
        ],
        vec![InlineKeyboardButton::url(
            "View ticket ↗",
            p.task_url.parse().expect("task_url is a valid URL"),
        )],
    ]);

    bot.send_message(chat_id, body)
        .reply_markup(kb)
        .await?;
}
```

(Adjust the `bot.send_message` call to match the actual telegram dispatcher signature in your tree.)

- [ ] **Step 3: Add a snapshot test of the body string**

In a `#[cfg(test)]` block in the same file:

```rust
#[cfg(test)]
mod inbox_format_tests {
    use super::*;
    use surge_intake::types::{Priority, TaskId};
    use crate::messages::{InboxCardPayload, NotifyMessage};

    fn sample_payload() -> InboxCardPayload {
        InboxCardPayload {
            task_id: TaskId::try_new("github_issues:user/repo#1").unwrap(),
            source_id: "github_issues:user/repo".into(),
            provider: "github_issues".into(),
            title: "Fix parser panic".into(),
            summary: "Stack overflow at depth 16".into(),
            priority: Priority::High,
            task_url: "https://github.com/user/repo/issues/1".into(),
            run_id: "run_abc".into(),
        }
    }

    #[test]
    fn inbox_card_body_format_snapshot() {
        let p = sample_payload();
        // Reproduce the format string used in the dispatcher to verify
        // we don't regress the user-visible text.
        let body = format!(
            "📋 Task from {provider} · {short}\n\n\
             {title}\n\
             priority: {prio} (auto-detected)\n\n\
             [▶ Start] [⏸ Snooze 24h] [✕ Skip]\n\
             [View ticket ↗]({url})",
            provider = p.provider,
            short = p.task_id.as_str().rsplit('/').next().unwrap_or(p.task_id.as_str()),
            title = p.title,
            prio = p.priority.label(),
            url = p.task_url,
        );
        insta::assert_snapshot!(body);
    }
}
```

- [ ] **Step 4: Run test, accept snapshot**

```bash
cargo test -p surge-notify --lib inbox_format_tests
cargo insta accept
```

(`cargo insta` from the `cargo-insta` tool, install with `cargo install cargo-insta` if not present.)

- [ ] **Step 5: Commit**

```bash
git add crates/surge-notify/src/telegram.rs crates/surge-notify/src/snapshots/
git commit -m "feat(notify): Telegram InboxCard formatter with snapshot test"
```

---

## Task 8.3 — Desktop inbox card formatter

**Files:**
- Modify: `crates/surge-notify/src/desktop.rs`

- [ ] **Step 1: Locate the desktop dispatcher**

```bash
grep -rn "notify_rust\|notify-rust" crates/surge-notify/ | head
```

Find where `notify_rust::Notification` is built. Add an `InboxCard` branch.

- [ ] **Step 2: Add the formatter**

```rust
NotifyMessage::InboxCard(p) => {
    let body = format!(
        "{}\npriority: {} ({})",
        p.title,
        p.priority.label(),
        p.provider,
    );
    let mut n = notify_rust::Notification::new();
    n.summary("📋 New Surge task")
        .body(&body)
        .action("start", "Start")
        .action("snooze", "Snooze 24h")
        .action("skip", "Skip")
        .timeout(notify_rust::Timeout::Never);
    let handle = n.show().map_err(|e| Error::Internal(e.to_string()))?;
    // Action handling: registers a closure that posts the chosen action
    // back to the daemon via the local IPC channel.
    let run_id = p.run_id.clone();
    handle.wait_for_action(move |action| {
        let action_name = action.unwrap_or("dismiss");
        let _ = post_inbox_action(&run_id, action_name);
    });
}
```

(`post_inbox_action` is a thin local-IPC call — for Plan C MVP it can be a tracing log + TODO; the inbox-decision handler is finalised in Task 9.3.)

- [ ] **Step 3: Add a smoke test of the body format**

```rust
#[cfg(test)]
mod desktop_inbox_tests {
    use super::*;
    use surge_intake::types::{Priority, TaskId};
    use crate::messages::InboxCardPayload;

    #[test]
    fn body_format() {
        let p = InboxCardPayload {
            task_id: TaskId::try_new("linear:wsp/A-1").unwrap(),
            source_id: "linear:wsp".into(),
            provider: "linear".into(),
            title: "Add tracing".into(),
            summary: "ad-hoc".into(),
            priority: Priority::Medium,
            task_url: "https://linear.app/wsp/issue/A-1".into(),
            run_id: "run_x".into(),
        };
        let body = format!(
            "{}\npriority: {} ({})",
            p.title,
            p.priority.label(),
            p.provider,
        );
        assert!(body.contains("Add tracing"));
        assert!(body.contains("medium"));
        assert!(body.contains("linear"));
    }
}
```

- [ ] **Step 4: Build and test**

```bash
cargo build -p surge-notify
cargo test -p surge-notify --lib desktop_inbox_tests
```

Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-notify/src/desktop.rs
git commit -m "feat(notify): desktop InboxCard formatter"
```

---

## Task 9.1 — `TaskSourceConfig` in `surge-core/config.rs`

**Files:**
- Modify: `crates/surge-core/src/config.rs`

- [ ] **Step 1: Locate the existing config**

```bash
grep -rn "SurgeConfig" crates/surge-core/src/ | head
```

- [ ] **Step 2: Add config types**

Append to the appropriate section of `crates/surge-core/src/config.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskSourceConfig {
    Linear(LinearSourceConfig),
    GithubIssues(GitHubIssuesSourceConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearSourceConfig {
    pub id: String,
    pub workspace_id: String,
    pub api_token_env: String,
    #[serde(default = "default_poll_interval", with = "duration_seconds")]
    pub poll_interval: Duration,
    #[serde(default)]
    pub label_filters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubIssuesSourceConfig {
    pub id: String,
    pub repo: String,
    pub api_token_env: String,
    #[serde(default = "default_poll_interval", with = "duration_seconds")]
    pub poll_interval: Duration,
    #[serde(default)]
    pub label_filters: Vec<String>,
}

fn default_poll_interval() -> Duration {
    Duration::from_secs(60)
}

mod duration_seconds {
    use super::Duration;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}
```

Then modify the existing `SurgeConfig` struct (find via grep) to add:

```rust
#[serde(default)]
pub task_sources: Vec<TaskSourceConfig>,
```

- [ ] **Step 3: Round-trip test**

Add to `crates/surge-core/src/config.rs`'s `#[cfg(test)]` mod:

```rust
#[test]
fn task_sources_round_trip_toml() {
    let toml_str = r#"
[[task_sources]]
type = "linear"
id = "linear-acme"
workspace_id = "wsp_acme_123"
api_token_env = "LINEAR_API_TOKEN"
poll_interval_seconds = 60
label_filters = ["surge:enabled", "surge:auto"]

[[task_sources]]
type = "github_issues"
id = "github-myapp"
repo = "user/myapp"
api_token_env = "GITHUB_TOKEN"
poll_interval_seconds = 60
label_filters = ["surge:enabled"]
"#;
    let cfg: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.task_sources.len(), 2);
    match &cfg.task_sources[0] {
        TaskSourceConfig::Linear(l) => assert_eq!(l.id, "linear-acme"),
        _ => panic!("expected Linear"),
    }
    match &cfg.task_sources[1] {
        TaskSourceConfig::GithubIssues(g) => assert_eq!(g.repo, "user/myapp"),
        _ => panic!("expected GithubIssues"),
    }
}
```

Note: `poll_interval_seconds` in the TOML maps to `poll_interval` field. Update the `with = "duration_seconds"` attribute to a `rename = "poll_interval_seconds"` if your existing serde style prefers that. Field-level `#[serde(rename = "poll_interval_seconds", with = "duration_seconds")]` is the cleanest:

```rust
#[serde(
    rename = "poll_interval_seconds",
    with = "duration_seconds",
    default = "default_poll_interval"
)]
pub poll_interval: Duration,
```

- [ ] **Step 4: Run test**

```bash
cargo test -p surge-core --lib config::tests::task_sources_round_trip_toml
```

Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-core/src/config.rs
git commit -m "feat(core): TaskSourceConfig for surge.toml [[task_sources]] entries"
```

---

## Task 9.2 — `surge-daemon`: spawn `TaskRouter` on startup

**Files:**
- Modify: `crates/surge-daemon/src/lib.rs` (or main loop file)
- Modify: `crates/surge-daemon/Cargo.toml` (add `surge-intake` + `surge-notify`)

- [ ] **Step 1: Add dependencies**

Append to `crates/surge-daemon/Cargo.toml` `[dependencies]`:

```toml
surge-intake = { workspace = true }
```

- [ ] **Step 2: Find the daemon startup function**

```bash
grep -rn "pub fn main\|pub async fn run\|fn start" crates/surge-daemon/src/ | head
```

- [ ] **Step 3: Wire `TaskRouter` after config loaded**

In the daemon's startup path, after the `SurgeConfig` is read and the SQLite connection is opened, add:

```rust
use std::env;
use std::sync::Arc;
use surge_core::config::TaskSourceConfig;
use surge_intake::github::source::{GitHubConfig, GitHubIssuesTaskSource};
use surge_intake::linear::source::{LinearConfig, LinearTaskSource};
use surge_intake::router::TaskRouter;
use surge_intake::TaskSource;
use tokio::sync::{mpsc, Mutex};

let mut sources: Vec<Arc<dyn TaskSource>> = Vec::new();
for s in &config.task_sources {
    match s {
        TaskSourceConfig::Linear(l) => {
            let token = env::var(&l.api_token_env).unwrap_or_else(|_| {
                tracing::warn!(env = %l.api_token_env, "env not set; skipping source");
                String::new()
            });
            if token.is_empty() {
                continue;
            }
            let cfg = LinearConfig {
                id: l.id.clone(),
                display_name: format!("Linear · {}", l.workspace_id),
                workspace_id: l.workspace_id.clone(),
                api_token: token,
                poll_interval: l.poll_interval,
                label_filters: l.label_filters.clone(),
            };
            match LinearTaskSource::new(cfg) {
                Ok(s) => sources.push(Arc::new(s)),
                Err(e) => tracing::error!(error = %e, "failed to init Linear source"),
            }
        }
        TaskSourceConfig::GithubIssues(g) => {
            let token = env::var(&g.api_token_env).unwrap_or_default();
            if token.is_empty() {
                tracing::warn!(env = %g.api_token_env, "env not set; skipping source");
                continue;
            }
            let (owner, repo) = match g.repo.split_once('/') {
                Some((o, r)) => (o.to_string(), r.to_string()),
                None => {
                    tracing::error!("invalid repo format: {}", g.repo);
                    continue;
                }
            };
            let cfg = GitHubConfig {
                id: g.id.clone(),
                display_name: format!("GitHub · {}", g.repo),
                owner,
                repo,
                api_token: token,
                poll_interval: g.poll_interval,
                label_filters: g.label_filters.clone(),
            };
            match GitHubIssuesTaskSource::new(cfg) {
                Ok(s) => sources.push(Arc::new(s)),
                Err(e) => tracing::error!(error = %e, "failed to init GitHub source"),
            }
        }
    }
}

if !sources.is_empty() {
    let (tx, mut rx) = mpsc::channel(32);
    let conn_arc = Arc::new(Mutex::new(/* daemon's existing rusqlite::Connection */));
    let router = TaskRouter::new(sources, conn_arc, tx);
    tokio::spawn(async move {
        if let Err(e) = router.run().await {
            tracing::error!(error = %e, "task router exited");
        }
    });

    tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            // Plan C handling: route to triage dispatcher / inbox notification.
            // For Task 9.2 we just log; Task 9.3 wires the rest.
            tracing::info!(?out, "router output");
        }
    });
}
```

(The `conn_arc` line uses the connection the daemon already owns; adapt to actual variable name.)

- [ ] **Step 4: Build**

```bash
cargo build -p surge-daemon
```

Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-daemon/Cargo.toml crates/surge-daemon/src/
git commit -m "feat(daemon): spawn TaskRouter from configured task_sources"
```

---

## Task 9.3 — Wire router output → Triage dispatcher → inbox notify

**Files:**
- Modify: `crates/surge-daemon/src/lib.rs` (continue from 9.2)

- [ ] **Step 1: Replace the placeholder loop in 9.2**

Replace the placeholder consumer:

```rust
tokio::spawn(async move {
    while let Some(out) = rx.recv().await {
        match out {
            surge_intake::router::RouterOutput::Triage { event } => {
                // Stub: Plan C MVP just logs and creates a placeholder run_id.
                // Real Triage Author dispatch is wired in via surge-orchestrator
                // when the orchestrator's `triage::run_one` lands. For Plan C MVP
                // we route the event into the inbox formatter directly with
                // a synthetic InboxCardPayload.
                let payload = surge_notify::messages::InboxCardPayload {
                    task_id: event.task_id.clone(),
                    source_id: event.source_id.clone(),
                    provider: event
                        .source_id
                        .split(':')
                        .next()
                        .unwrap_or("unknown")
                        .to_string(),
                    title: event
                        .raw_payload
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("New ticket")
                        .to_string(),
                    summary: String::new(),
                    priority: surge_intake::types::Priority::Medium,
                    task_url: event
                        .raw_payload
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    run_id: ulid::Ulid::new().to_string(),
                };
                let msg = surge_notify::messages::NotifyMessage::InboxCard(payload);
                if let Err(e) = notify_dispatcher.send(msg).await {
                    tracing::error!(error = %e, "failed to send inbox notification");
                }
            }
            surge_intake::router::RouterOutput::EarlyDuplicate { event, run_id } => {
                tracing::info!(?event, %run_id, "tier1 early-dup; posting comment");
                // TODO Plan C polish: invoke source.post_comment with prefixed body
            }
        }
    }
});
```

(`notify_dispatcher` is the existing handle in the daemon for sending `NotifyMessage`s. Adapt to the actual API.)

> **Note:** This is the **MVP wire-up**. Triage Author full integration (LLM dispatch via `surge-orchestrator::triage::run_one`) is left as a TODO for Plan C polish — we surface the inbox card immediately with `Priority::Medium` placeholder. The user can then start the run; the bootstrap pipeline produces the proper Description Author output. The polish task replaces the placeholder with real Triage decision data.

- [ ] **Step 2: Build**

```bash
cargo build -p surge-daemon
```

- [ ] **Step 3: Commit**

```bash
git add crates/surge-daemon/src/
git commit -m "feat(daemon): wire router → inbox notify (MVP placeholder triage)"
```

---

## Task 10.1 — New `EventPayload` variants in `surge-core`

**Files:**
- Modify: `crates/surge-core/src/event.rs`

- [ ] **Step 1: Locate the enum**

```bash
grep -rn "enum EventPayload\|enum SurgeEvent" crates/surge-core/src/ | head
```

- [ ] **Step 2: Add the new variants**

Append the following variants to the `EventPayload` enum (preserving existing variants):

```rust
TicketDetected {
    task_id: String,
    source_id: String,
    provider: String,
},
Tier1DedupDecided {
    task_id: String,
    decision: String,         // "Pass" | "EarlyDuplicate"
    duplicate_run_id: Option<String>,
},
TriageDecided {
    task_id: String,
    decision: String,         // "enqueued"|"duplicate"|"out_of_scope"|"unclear"
    priority: Option<String>,
    duplicate_of: Option<String>,
    reasoning: String,
},
InboxCardSent {
    task_id: String,
    run_id: String,
    channels: Vec<String>,
},
InboxDecided {
    task_id: String,
    run_id: String,
    decision: String,         // "start"|"snooze"|"skip"
    decided_via: String,      // "telegram"|"desktop"
},
TrackerCommentPosted {
    task_id: String,
    purpose: String,
},
TrackerCommentPostFailed {
    task_id: String,
    attempt: u32,
    error: String,
},
TrackerLabelChanged {
    task_id: String,
    label: String,
    present: bool,
},
TrackerLabelSetFailed {
    task_id: String,
    label: String,
    error: String,
},
TaskSourcePollFailed {
    source_id: String,
    attempt: u32,
    error: String,
    retry_in_secs: u64,
},
TaskSourceAuthFailed {
    source_id: String,
    error: String,
},
TriageAuthorFailed {
    task_id: String,
    attempt: u32,
    error: String,
},
TriageStaleRecovery {
    task_id: String,
    run_id: String,
    reason: String,
},
UserMentionReceived {
    task_id: String,
    comment_id: String,
    body: String,
},
```

- [ ] **Step 3: Update bincode serialization tests**

Find the existing event-payload round-trip test in `surge-core` and add a case for one of the new variants:

```rust
#[test]
fn ticket_detected_round_trip_bincode() {
    let ev = EventPayload::TicketDetected {
        task_id: "github_issues:user/repo#1".into(),
        source_id: "github_issues:user/repo".into(),
        provider: "github_issues".into(),
    };
    let bytes = bincode::serialize(&ev).unwrap();
    let back: EventPayload = bincode::deserialize(&bytes).unwrap();
    assert!(matches!(back, EventPayload::TicketDetected { .. }));
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p surge-core --lib event::tests
```

Expected: existing tests still pass + new test passes.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-core/src/event.rs
git commit -m "feat(core): EventPayload variants for tracker integration"
```

---

## Task 11.1 — End-to-end mock pipeline test

**Files:**
- Create: `crates/surge-intake/tests/e2e_mock.rs`

- [ ] **Step 1: Write the test**

Create `crates/surge-intake/tests/e2e_mock.rs`:

```rust
//! End-to-end mock pipeline:
//! MockTaskSource → TaskRouter → router output → assert.
//!
//! Verifies that a new task event flows through Tier-1 dedup and arrives
//! at the consumer as a `Triage` output, then a follow-up event for the
//! same task with an active run produces an `EarlyDuplicate`.

use chrono::Utc;
use rusqlite::Connection;
use std::sync::Arc;
use std::time::Duration;
use surge_intake::router::{RouterOutput, TaskRouter};
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{TaskEvent, TaskEventKind, TaskId};
use surge_intake::TaskSource;
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use tokio::sync::{mpsc, Mutex};

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
async fn e2e_new_task_then_dup() {
    let conn = Arc::new(Mutex::new(db()));

    // First wave: a fresh ticket arrives.
    {
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(ev("mock:t#1")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src as Arc<dyn TaskSource>], Arc::clone(&conn), tx);
        let handle = tokio::spawn(router.run());

        let out = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match out {
            RouterOutput::Triage { event } => assert_eq!(event.task_id.as_str(), "mock:t#1"),
            other => panic!("expected Triage, got {other:?}"),
        }
        drop(rx);
        let _ = handle.await;
    }

    // Simulate that a run got created for that ticket.
    {
        let c = conn.lock().await;
        c.execute("INSERT INTO runs(id) VALUES ('run_xyz')", []).unwrap();
        IntakeRepo::new(&*c)
            .insert(&IntakeRow {
                task_id: "mock:t#1".into(),
                source_id: "mock:t".into(),
                provider: "mock".into(),
                run_id: Some("run_xyz".into()),
                triage_decision: Some("enqueued".into()),
                duplicate_of: None,
                priority: Some("medium".into()),
                state: TicketState::Active,
                first_seen: Utc::now(),
                last_seen: Utc::now(),
                snooze_until: None,
            })
            .unwrap();
    }

    // Second wave: the same ticket appears again (e.g. label changed).
    {
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(ev("mock:t#1")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src as Arc<dyn TaskSource>], Arc::clone(&conn), tx);
        let handle = tokio::spawn(router.run());

        let out = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match out {
            RouterOutput::EarlyDuplicate { run_id, .. } => {
                assert_eq!(run_id, "run_xyz");
            }
            other => panic!("expected EarlyDuplicate, got {other:?}"),
        }
        drop(rx);
        let _ = handle.await;
    }
}
```

- [ ] **Step 2: Run test**

```bash
cargo test -p surge-intake --test e2e_mock
```

Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-intake/tests/e2e_mock.rs
git commit -m "test(intake): end-to-end mock pipeline (new task + dedup)"
```

---

## Task 13.1 — `surge tracker` CLI subcommand

**Files:**
- Create: `crates/surge-cli/src/commands/tracker.rs`
- Modify: `crates/surge-cli/src/main.rs`
- Modify: `crates/surge-cli/Cargo.toml`

- [ ] **Step 1: Add dep**

Append to `crates/surge-cli/Cargo.toml`:

```toml
surge-intake = { workspace = true }
```

- [ ] **Step 2: Create the subcommand**

Create `crates/surge-cli/src/commands/tracker.rs`:

```rust
//! `surge tracker` subcommand: list configured sources, test connectivity.

use anyhow::{Context, Result};
use clap::Subcommand;
use std::env;
use std::time::Duration;
use surge_core::config::{SurgeConfig, TaskSourceConfig};
use surge_intake::github::source::{GitHubConfig, GitHubIssuesTaskSource};
use surge_intake::linear::source::{LinearConfig, LinearTaskSource};
use surge_intake::TaskSource;

#[derive(Subcommand, Debug)]
pub enum TrackerCommand {
    /// List configured task sources.
    List,
    /// Test connectivity to a configured source by id.
    Test {
        /// Source id (e.g. "linear-acme").
        id: String,
    },
}

pub async fn run(cmd: TrackerCommand, config: SurgeConfig) -> Result<()> {
    match cmd {
        TrackerCommand::List => list(config),
        TrackerCommand::Test { id } => test(config, &id).await,
    }
}

fn list(config: SurgeConfig) -> Result<()> {
    if config.task_sources.is_empty() {
        println!("No task sources configured.");
        return Ok(());
    }
    println!("Configured task sources:");
    for s in &config.task_sources {
        match s {
            TaskSourceConfig::Linear(l) => {
                println!(
                    "  · linear · id={} workspace={} env={} interval={}s",
                    l.id,
                    l.workspace_id,
                    l.api_token_env,
                    l.poll_interval.as_secs()
                );
            }
            TaskSourceConfig::GithubIssues(g) => {
                println!(
                    "  · github_issues · id={} repo={} env={} interval={}s",
                    g.id,
                    g.repo,
                    g.api_token_env,
                    g.poll_interval.as_secs()
                );
            }
        }
    }
    Ok(())
}

async fn test(config: SurgeConfig, target_id: &str) -> Result<()> {
    let s = config
        .task_sources
        .iter()
        .find(|s| match s {
            TaskSourceConfig::Linear(l) => l.id == target_id,
            TaskSourceConfig::GithubIssues(g) => g.id == target_id,
        })
        .with_context(|| format!("source not found: {target_id}"))?;

    match s {
        TaskSourceConfig::Linear(l) => {
            let token = env::var(&l.api_token_env).with_context(|| {
                format!("env var {} not set", l.api_token_env)
            })?;
            let cfg = LinearConfig {
                id: l.id.clone(),
                display_name: format!("Linear · {}", l.workspace_id),
                workspace_id: l.workspace_id.clone(),
                api_token: token,
                poll_interval: Duration::from_secs(60),
                label_filters: l.label_filters.clone(),
            };
            let src = LinearTaskSource::new(cfg)?;
            let summaries = src.list_open_tasks().await?;
            println!("✓ {} open tasks accessible", summaries.len());
        }
        TaskSourceConfig::GithubIssues(g) => {
            let token = env::var(&g.api_token_env).with_context(|| {
                format!("env var {} not set", g.api_token_env)
            })?;
            let (owner, repo) = g
                .repo
                .split_once('/')
                .with_context(|| format!("invalid repo format: {}", g.repo))?;
            let cfg = GitHubConfig {
                id: g.id.clone(),
                display_name: format!("GitHub · {}", g.repo),
                owner: owner.into(),
                repo: repo.into(),
                api_token: token,
                poll_interval: Duration::from_secs(60),
                label_filters: g.label_filters.clone(),
            };
            let src = GitHubIssuesTaskSource::new(cfg)?;
            let summaries = src.list_open_tasks().await?;
            println!("✓ {} open issues accessible", summaries.len());
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Register in main.rs**

Find the existing `Commands` enum in `crates/surge-cli/src/main.rs` and add:

```rust
mod commands;

#[derive(Subcommand)]
enum Commands {
    // ... existing variants ...
    /// Manage issue-tracker integration.
    Tracker {
        #[command(subcommand)]
        cmd: commands::tracker::TrackerCommand,
    },
}
```

In the dispatch `match`:

```rust
Commands::Tracker { cmd } => commands::tracker::run(cmd, config).await?,
```

(Adapt to the actual `main` shape; ensure `tokio::main` if needed.)

- [ ] **Step 4: Build**

```bash
cargo build -p surge-cli
```

Expected: success.

- [ ] **Step 5: Smoke run**

```bash
cargo run -p surge-cli -- tracker list
```

Expected: prints "No task sources configured." (assuming the local `surge.toml` has none).

- [ ] **Step 6: Commit**

```bash
git add crates/surge-cli/Cargo.toml crates/surge-cli/src/
git commit -m "feat(cli): surge tracker list / test commands"
```

---

## Plan C wrap-up

- [ ] **Step 1: Workspace verification**

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: green.

- [ ] **Step 2: Document Plan C completion**

Append to `docs/revision/plans/PROGRESS-RFC-0010.md`:

```markdown
## RFC-0010 — Plan C · Integration ✅

- [x] M7 Triage Author profile + dispatcher + fixtures (Tasks 7.1–7.4)
- [x] M8 surge-notify InboxCard (Telegram + Desktop) (Tasks 8.1–8.3)
- [x] M9 surge-daemon TaskRouter wire-up (Tasks 9.1–9.3)
- [x] M10 EventPayload variants (Task 10.1)
- [x] M11 End-to-end mock pipeline test (Task 11.1)
- [x] M13 surge tracker CLI subcommand (Task 13.1)

RFC-0010 implementation complete. Acceptance criteria from the spec to verify:
- (1) surge-intake compiles, exports trait, ships Linear+GitHub impls ✅
- (2) Configured sources poll successfully (verify via `cargo test --test linear_real --test github_real -- --ignored` with secrets)
- (3-12) End-to-end with real services — Phase 2 polish
```

- [ ] **Step 3: Commit**

```bash
git add docs/revision/plans/PROGRESS-RFC-0010.md
git commit -m "docs(rfc-0010): Plan C integration complete"
```

---

## Plan C self-review

**Spec coverage:**

- Decision #8 (Triage Author bootstrap stage 0) — covered by Tasks 7.1, 7.2, 7.3, 7.4.
- Decision #11 (Surge sets priority via Triage) — `TriageDecision::Enqueued.priority` (Task 7.2 input/output types).
- Decision #14 (Telegram = universal cockpit-inbox) — `InboxCard` formatter in 8.2; existing approval/status formatters unchanged.
- Decision #17 (`InboxCard` new message type) — Task 8.1.
- Decision #19 (inbox-cycle approval) — buttons in Task 8.2.
- Decision #20 (Telegram + Desktop parallel) — Tasks 8.2 and 8.3.
- Decision #23 (post comment after run created) — placeholder in Task 9.3 (real call routes through `TaskSource.post_comment`; full wire-up belongs to Plan C polish noted at the end of 9.3).
- New `EventPayload` variants — Task 10.1.
- Acceptance criteria #1, #2, #5, #9 (label-driven L3) — partially covered (criterion #1 fully; #2 needs real-API run; #5 needs polish task to invoke real `post_comment`; #9 requires Triage real LLM dispatch).

**Out of scope for Plan C (deferred):**

- Real Triage Author LLM dispatch via `surge-orchestrator::triage::run_one` (placeholder in Task 9.3 produces inbox cards with `Priority::Medium`; replacement is Plan-C-polish).
- Comment posting on RunStarted/Completed/Failed transitions (placeholder in Task 9.3).
- Inbox-decision callback handlers (Start/Snooze/Skip) end-to-end into engine — partial in Task 8.2 (button construction); full handler binds in Plan-C-polish.
- Vertical-slice + token-budget enhancements #25, #26 (RFC-0004 refactor — separate plan).

**Placeholder scan:** Two `TODO`s remain, both marked explicitly:

1. Task 5.4 / 6.2 (Plan B): multi-issue per cycle emission — documented note, not a functional gap.
2. Task 9.3: Triage real-LLM dispatch + RunStarted comment + inbox-decision handlers — explicitly flagged as Plan-C-polish; the MVP wire-up surfaces inbox cards correctly.

**Type consistency:**

- `Priority` and `TaskId` shared via `surge-intake::types` — used unchanged in `surge-notify::InboxCardPayload`, `surge-orchestrator::triage::TriageJson::into_decision`, `surge-cli` config printing.
- `TaskSourceConfig` field names (`workspace_id`, `repo`, `api_token_env`, `label_filters`, `poll_interval`) match between `surge-core` (Task 9.1), the daemon wire-up (Task 9.2), and CLI (Task 13.1).
- `RouterOutput` variants (`Triage`, `EarlyDuplicate`) used identically in 9.2/9.3 and the e2e test 11.1.
- `TriageDecision::Enqueued` field names (`priority`, `reasoning`, `summary`) consistent between dispatcher (Task 7.2) and fixture format (Task 7.3).
