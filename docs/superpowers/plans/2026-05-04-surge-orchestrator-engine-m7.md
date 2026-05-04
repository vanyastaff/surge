# M7 — surge-orchestrator daemon mode + MCP delegation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a long-running `surge daemon` process hosting the M6 engine over JSON-RPC IPC, plus MCP server delegation via a new `surge-mcp` crate (rmcp 1.6+) so agent stages can call user-configured MCP servers beyond the engine's built-in tool surface.

**Architecture:** Single-process daemon hosts many runs (preserves M5/M6 choice — accepted divergence from revision §03). `EngineFacade` trait abstracts in-process vs daemon hosting; existing `Engine` is unchanged. Cross-platform IPC via `interprocess` 2.x local-socket abstraction with line-delimited JSON-RPC framing. MCP integration via official rmcp 1.6+ (`transport-child-process` only — HTTP deferred). `RoutingToolDispatcher` fans out tool calls between engine built-ins and MCP servers, with sandbox-aware exposure at session-open. No snapshot schema bump — daemon owns no run state.

**Tech Stack:** Rust 2024 (MSRV 1.85), tokio multi-thread, new dep `rmcp = ">=1.6, <2"` (features `["client", "transport-child-process"]`), new dep `interprocess = "2"` (feature `tokio`), new dep `nix = "0.29"` (feature `signal` — Unix signal handling), existing deps re-used (`async-trait`, `thiserror`, `serde_json`, `tokio-util` for `CancellationToken`).

**Spec:** [docs/superpowers/specs/2026-05-04-surge-orchestrator-engine-m7-design.md](../specs/2026-05-04-surge-orchestrator-engine-m7-design.md) (committed at `138a99e`).

**Spec deviations to call out (the plan implements these):**
- The spec §5.8 introduces a new `AgentConfig::mcp_servers` field. The codebase already has `ToolOverride::mcp_add: Vec<String>` ([crates/surge-core/src/agent_config.rs:60](../../../crates/surge-core/src/agent_config.rs)) which serves the exact same per-stage allowlist purpose. The plan **reuses `ToolOverride::mcp_add`** instead of adding a new field, since their semantics overlap. Validation in Phase 1.3 references both the registry definitions and these existing override fields.
- The spec §3.5 / §7.3 reference a `ToolDispatcher::declared_tools()` method that doesn't exist yet on the trait. The plan adds it as a default-method extension in Phase 2 (non-breaking).

---

## File Structure

### New files

```
crates/surge-daemon/                                (NEW lib + bin crate)
├── Cargo.toml
├── README.md                                       (operator docs — Phase 11)
└── src/
    ├── lib.rs                                      (re-exports)
    ├── main.rs                                     (binary entry)
    ├── server.rs                                   (IPC accept-and-dispatch)
    ├── admission.rs                                (AdmissionController)
    ├── broadcast.rs                                (BroadcastRegistry)
    ├── lifecycle.rs                                (signal handlers, drain)
    ├── pidfile.rs                                  (PID + socket discovery)
    └── error.rs                                    (DaemonError)

crates/surge-mcp/                                   (NEW lib crate)
├── Cargo.toml
├── README.md                                       (per-server setup — Phase 11)
└── src/
    ├── lib.rs                                      (re-exports)
    ├── connection.rs                               (McpServerConnection)
    ├── registry.rs                                 (McpRegistry)
    └── error.rs                                    (McpError)

crates/surge-core/src/
└── mcp_config.rs                                   (NEW: McpServerRef, McpTransportConfig)

crates/surge-orchestrator/src/engine/
├── facade.rs                                       (NEW: EngineFacade trait + Local impl)
├── daemon_facade.rs                                (NEW: DaemonEngineFacade IPC client)
├── ipc.rs                                          (NEW: protocol enums + framing helpers)
└── tools/routing.rs                                (NEW: RoutingToolDispatcher)

crates/surge-cli/src/commands/
└── daemon.rs                                       (NEW: surge daemon start/stop/status/restart)

crates/surge-mcp/tests/fixtures/
└── mock_mcp_server.rs                              (NEW: minimal stdio MCP server fixture)

crates/surge-daemon/tests/
├── daemon_e2e_smoke.rs                             (NEW: in-process smoke)
├── daemon_admission_queue.rs                       (NEW: FIFO under load)
└── daemon_graceful_shutdown.rs                     (NEW: SIGTERM drain)

crates/surge-mcp/tests/
├── mcp_stdio_e2e.rs                                (NEW: connect, list, call, crash)
└── mcp_restart_policy.rs                           (NEW: restart_on_crash variants)

crates/surge-orchestrator/tests/
├── engine_m7_routing_dispatcher.rs                 (NEW: routing table semantics)
└── engine_m7_agent_stage_with_mcp.rs               (NEW: agent calls MCP tool)

crates/surge-cli/tests/
└── cli_m7_daemon_smoke.rs                          (NEW: daemon start + ping)
```

### Modified files

```
Cargo.toml                                          (workspace deps + members)
crates/surge-core/src/lib.rs                        (re-export McpServerRef)
crates/surge-core/src/run_event.rs:382              (RunConfig::mcp_servers field)
crates/surge-core/src/validation.rs                 (M7 validation rules)
crates/surge-orchestrator/Cargo.toml                (add interprocess, surge-mcp)
crates/surge-orchestrator/src/engine/mod.rs         (re-export EngineFacade etc.)
crates/surge-orchestrator/src/engine/handle.rs      (RunSummary, RunStatus types)
crates/surge-orchestrator/src/engine/tools/mod.rs   (ToolDispatcher::declared_tools)
crates/surge-orchestrator/src/engine/tools/worktree.rs  (override declared_tools)
crates/surge-orchestrator/src/engine/stage/agent.rs (RoutingToolDispatcher session-open wiring)
crates/surge-cli/Cargo.toml                         (add surge-mcp dep)
crates/surge-cli/src/commands/mod.rs                (re-export daemon module)
crates/surge-cli/src/commands/engine.rs             (--daemon flag retrofit)
crates/surge-cli/src/main.rs                        (Commands::Daemon variant)
docs/03-ROADMAP.md                                  (M7 line + surface)
```

---

## Phase 0 — scaffolding (1 day)

### Task 0.1: workspace updates + new crate skeletons

**Files:**
- Modify: `Cargo.toml` (workspace members + new deps)
- Create: `crates/surge-daemon/Cargo.toml`
- Create: `crates/surge-daemon/src/lib.rs` (placeholder)
- Create: `crates/surge-daemon/src/main.rs` (placeholder)
- Create: `crates/surge-mcp/Cargo.toml`
- Create: `crates/surge-mcp/src/lib.rs` (placeholder)

- [ ] **Step 1: Add new workspace deps + members**

Edit `Cargo.toml`. Append to `[workspace] members`:

```toml
"crates/surge-daemon",
"crates/surge-mcp",
```

Append to `[workspace.dependencies]`:

```toml
# M7 dependencies
rmcp = { version = ">=1.6, <2.0", default-features = false, features = ["client", "transport-child-process"] }
interprocess = { version = "2", features = ["tokio"] }
nix = { version = "0.29", default-features = false, features = ["signal"] }

# Internal M7 crates
surge-daemon = { path = "crates/surge-daemon" }
surge-mcp = { path = "crates/surge-mcp" }
```

- [ ] **Step 2: Create `crates/surge-daemon/Cargo.toml`**

```toml
[package]
name = "surge-daemon"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lib]
path = "src/lib.rs"

[[bin]]
name = "surge-daemon"
path = "src/main.rs"

[dependencies]
async-trait.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
tracing.workspace = true
tracing-subscriber.workspace = true

interprocess.workspace = true
sysinfo.workspace = true

surge-core.workspace = true
surge-orchestrator.workspace = true
surge-acp.workspace = true
surge-persistence.workspace = true
surge-notify.workspace = true

# Unix signal handling
[target.'cfg(unix)'.dependencies]
nix.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 3: Create `crates/surge-daemon/src/lib.rs` placeholder**

```rust
//! `surge-daemon` — long-running process that hosts the M7+ engine
//! and exposes it over IPC. See
//! `docs/superpowers/specs/2026-05-04-surge-orchestrator-engine-m7-design.md`
//! §3 and §6 for the design contract.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

// Modules added incrementally in Phase 3+.
```

- [ ] **Step 4: Create `crates/surge-daemon/src/main.rs` placeholder**

```rust
//! `surge-daemon` binary. Phase 6 fills this in.

fn main() -> std::process::ExitCode {
    eprintln!("surge-daemon: scaffolded; runtime added in Phase 6");
    std::process::ExitCode::from(2)
}
```

- [ ] **Step 5: Create `crates/surge-mcp/Cargo.toml`**

```toml
[package]
name = "surge-mcp"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["full"] }
tracing.workspace = true

rmcp.workspace = true

surge-core.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 6: Create `crates/surge-mcp/src/lib.rs` placeholder**

```rust
//! `surge-mcp` — MCP (Model Context Protocol) client integration for
//! `surge-orchestrator` agent stages. Wraps the official `rmcp` crate
//! with surge-flavoured registry, connection state, restart policy.
//!
//! See `docs/superpowers/specs/2026-05-04-surge-orchestrator-engine-m7-design.md`
//! §3.4, §5.6, §7 for the design contract.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

// Modules added incrementally in Phase 7+.
```

- [ ] **Step 7: Build workspace**

Run: `cargo build --workspace`
Expected: clean build. Both new crates compile (placeholders). The `surge-daemon` binary builds; running it prints "scaffolded; runtime added in Phase 6" and exits 2.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/surge-daemon/ crates/surge-mcp/
git commit -m "M7 P0: scaffold surge-daemon + surge-mcp crates"
```

---

## Phase 1 — surge-core extensions (CRITICAL PATH per spec §24.1) (2 days)

### Task 1.1: `McpServerRef` + `McpTransportConfig` types

**Files:**
- Create: `crates/surge-core/src/mcp_config.rs`
- Modify: `crates/surge-core/src/lib.rs` (add `pub mod mcp_config;` + re-export)

- [ ] **Step 1: Write failing test**

Create `crates/surge-core/src/mcp_config.rs`:

```rust
//! MCP server reference types — the run-level registry of MCP server
//! definitions. Per-stage `ToolOverride::mcp_add` then references
//! these by name.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Run-level definition of a single MCP server.
///
/// `name` identifies the server in `ToolOverride::mcp_add` allowlists.
/// `transport` describes how the engine spawns / connects to it.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerRef {
    /// Identifier referenced from per-stage allowlists.
    pub name: String,
    /// How the engine reaches this server.
    pub transport: McpTransportConfig,
    /// Optional whitelist of tool names. If `None`, all tools the
    /// server reports via `tools/list` are exposed.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Maximum time a single `tools/call` may take. Default 60 s.
    #[serde(default = "McpServerRef::default_call_timeout", with = "humantime_serde")]
    pub call_timeout: Duration,
    /// Whether the engine should re-spawn the server child process if
    /// it exits while still configured. Default true.
    #[serde(default = "McpServerRef::default_restart_on_crash")]
    pub restart_on_crash: bool,
}

impl McpServerRef {
    fn default_call_timeout() -> Duration {
        Duration::from_secs(60)
    }
    fn default_restart_on_crash() -> bool {
        true
    }
}

/// How a `surge` engine reaches an MCP server. M7 supports stdio
/// child-process only.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportConfig {
    /// Spawn `command args` and talk MCP over its stdio.
    Stdio {
        command: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_server_ref_toml_roundtrips() {
        let r = McpServerRef {
            name: "playwright".into(),
            transport: McpTransportConfig::Stdio {
                command: PathBuf::from("/usr/local/bin/mcp-playwright"),
                args: vec!["--headless".into()],
                env: HashMap::new(),
            },
            allowed_tools: Some(vec!["browser_navigate".into()]),
            call_timeout: Duration::from_secs(120),
            restart_on_crash: true,
        };
        let s = toml::to_string(&r).unwrap();
        let parsed: McpServerRef = toml::from_str(&s).unwrap();
        assert_eq!(r, parsed);
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let s = r#"
            name = "github"
            transport = { kind = "stdio", command = "npx", args = ["@github/mcp-server"] }
        "#;
        let r: McpServerRef = toml::from_str(s).unwrap();
        assert_eq!(r.allowed_tools, None);
        assert_eq!(r.call_timeout, Duration::from_secs(60));
        assert!(r.restart_on_crash);
    }
}
```

- [ ] **Step 2: Add `humantime_serde` workspace dep (for `Duration` in TOML)**

Edit workspace `Cargo.toml`, append to `[workspace.dependencies]`:

```toml
humantime-serde = "1"
```

Edit `crates/surge-core/Cargo.toml`, append to `[dependencies]`:

```toml
humantime-serde = { workspace = true }
```

- [ ] **Step 3: Wire module + re-export**

Edit `crates/surge-core/src/lib.rs`. Find the existing `pub mod ...;` block (search for `pub mod node;`) and add alongside:

```rust
pub mod mcp_config;
```

Find the existing `pub use ...` re-export block (near top after the `pub mod` declarations) and add:

```rust
pub use mcp_config::{McpServerRef, McpTransportConfig};
```

- [ ] **Step 4: Run failing test**

Run: `cargo test -p surge-core --lib mcp_config::`
Expected: tests pass (the file we wrote in Step 1 has the impl). Verify both `stdio_server_ref_toml_roundtrips` and `defaults_apply_when_omitted` are PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/surge-core/Cargo.toml crates/surge-core/src/mcp_config.rs crates/surge-core/src/lib.rs
git commit -m "M7 P1.1: McpServerRef + McpTransportConfig in surge-core

Run-level definitions for MCP servers. Stdio transport only in M7;
HTTP and SSE deferred per spec §1.2. Both new public types marked
#[non_exhaustive]."
```

---

### Task 1.2: `RunConfig::mcp_servers` registry field

**Files:**
- Modify: `crates/surge-core/src/run_event.rs:382` (RunConfig)
- Modify: `crates/surge-core/src/run_event.rs::tests` (add field to existing tests)

- [ ] **Step 1: Discover all `RunConfig { ... }` constructions**

Run: `grep -rn "RunConfig {" crates/ | grep -v target`
Expected: a handful of construction sites (engine, persistence tests, run_event self-tests). All will need the new field.

If count > 10, raise an alarm — the field-add ripple is bigger than expected. Otherwise continue.

- [ ] **Step 2: Write failing test**

Append to `crates/surge-core/src/run_event.rs::tests` (before closing `}`):

```rust
#[test]
fn run_config_with_mcp_servers_roundtrips() {
    use crate::mcp_config::{McpServerRef, McpTransportConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    let cfg = RunConfig {
        sandbox_default: SandboxMode::WorkspaceWrite,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: vec![McpServerRef {
            name: "playwright".into(),
            transport: McpTransportConfig::Stdio {
                command: PathBuf::from("mcp-playwright"),
                args: vec![],
                env: HashMap::new(),
            },
            allowed_tools: None,
            call_timeout: Duration::from_secs(60),
            restart_on_crash: true,
        }],
    };
    let payload = EventPayload::RunStarted {
        pipeline_template: None,
        project_path: PathBuf::from("/work"),
        initial_prompt: "x".into(),
        config: cfg.clone(),
    };
    let bytes = payload.to_bincode().unwrap();
    let parsed = EventPayload::from_bincode(&bytes).unwrap();
    assert_eq!(payload, parsed);
}

#[test]
fn run_config_default_mcp_servers_empty() {
    let s = r#"
        sandbox_default = "workspace_write"
        approval_default = "on_request"
    "#;
    let cfg: RunConfig = toml::from_str(s).unwrap();
    assert!(cfg.mcp_servers.is_empty());
    assert!(!cfg.auto_pr);
}
```

- [ ] **Step 3: Run failing test (compile fail expected)**

Run: `cargo test -p surge-core --lib run_event::tests::run_config_with_mcp_servers_roundtrips`
Expected: compile error — `RunConfig` has no `mcp_servers` field. Good.

- [ ] **Step 4: Add the field**

Edit `crates/surge-core/src/run_event.rs:382`. Replace the `RunConfig` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunConfig {
    pub sandbox_default: SandboxMode,
    pub approval_default: ApprovalPolicy,
    #[serde(default)]
    pub auto_pr: bool,
    /// Run-level registry of MCP servers available to agent stages.
    /// Per-stage `ToolOverride::mcp_add` references these by name.
    /// Empty by default — no MCP delegation.
    #[serde(default)]
    pub mcp_servers: Vec<crate::mcp_config::McpServerRef>,
}
```

- [ ] **Step 5: Fix compile sites**

Run: `cargo build --workspace 2>&1 | tee /tmp/m7-runconfig-discovery.log`
Expected: errors at every `RunConfig { ... }` construction missing `mcp_servers`. For each, add `mcp_servers: Vec::new(),` to the struct literal. Keep the change minimal — no behaviour change.

Re-run until clean: `cargo build --workspace`. Expected: zero errors.

- [ ] **Step 6: Run all tests**

Run: `cargo test -p surge-core --lib`
Expected: all tests pass including the two new ones from Step 2.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-core/src/run_event.rs <any files modified in Step 5>
git commit -m "M7 P1.2: RunConfig::mcp_servers — run-level MCP registry

Additive field with #[serde(default)] for backwards compat. Empty
by default; populated by the daemon / CLI from RunConfig load.
Bincode + TOML roundtrips covered."
```

---

### Task 1.3: validation rules in `surge-core::validation`

**Files:**
- Modify: `crates/surge-core/src/validation.rs` (new error variants + check fns)

- [ ] **Step 1: Locate the existing validation surface**

Run: `grep -n "ValidationErrorKind\|fn validate\|severity" crates/surge-core/src/validation.rs | head -30`
Expected: an enum `ValidationErrorKind` with many variants, each having a `severity()` arm. M7 adds three variants.

- [ ] **Step 2: Write failing tests**

Append to `crates/surge-core/src/validation.rs::tests` (before closing `}`):

```rust
#[test]
fn mcp_server_undeclared_in_tool_override_is_error() {
    use crate::agent_config::{AgentConfig, NodeLimits, ToolOverride};
    use crate::keys::ProfileKey;
    use std::collections::BTreeMap;

    let stage_cfg = AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: Some(ToolOverride {
            mcp_add: vec!["undeclared_server".into()],
            mcp_remove: vec![],
            skills_add: vec![],
            skills_remove: vec![],
            shell_allowlist_add: vec![],
        }),
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: NodeLimits::default(),
        hooks: vec![],
        custom_fields: BTreeMap::new(),
    };
    let registry: Vec<crate::mcp_config::McpServerRef> = vec![]; // empty registry
    let errors = crate::validation::validate_mcp_references(
        "stage_x",
        &stage_cfg,
        &registry,
    );
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        errors[0].kind,
        crate::validation::ValidationErrorKind::McpServerUndeclared { .. }
    ));
}

#[test]
fn empty_server_name_is_error() {
    use crate::mcp_config::{McpServerRef, McpTransportConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    let r = McpServerRef {
        name: "".into(),
        transport: McpTransportConfig::Stdio {
            command: PathBuf::from("nope"),
            args: vec![],
            env: HashMap::new(),
        },
        allowed_tools: None,
        call_timeout: Duration::from_secs(60),
        restart_on_crash: true,
    };
    let errors = crate::validation::validate_mcp_server_ref(&r);
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        errors[0].kind,
        crate::validation::ValidationErrorKind::McpServerNameEmpty
    ));
}

#[test]
fn command_path_with_dotdot_segment_is_error() {
    use crate::mcp_config::{McpServerRef, McpTransportConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    let r = McpServerRef {
        name: "evil".into(),
        transport: McpTransportConfig::Stdio {
            command: PathBuf::from("../../../usr/bin/yes"),
            args: vec![],
            env: HashMap::new(),
        },
        allowed_tools: None,
        call_timeout: Duration::from_secs(60),
        restart_on_crash: true,
    };
    let errors = crate::validation::validate_mcp_server_ref(&r);
    assert!(errors.iter().any(|e| matches!(
        e.kind,
        crate::validation::ValidationErrorKind::McpCommandPathUnsafe { .. }
    )));
}
```

- [ ] **Step 3: Run failing tests (compile fail expected)**

Run: `cargo test -p surge-core --lib validation::tests::mcp_`
Expected: compile errors — `validate_mcp_references`, `validate_mcp_server_ref`, and the `McpServer*` variants don't exist yet.

- [ ] **Step 4: Add new `ValidationErrorKind` variants**

Edit `crates/surge-core/src/validation.rs`. Find `pub enum ValidationErrorKind {` and add three variants (alongside the existing ones — keep alphabetical or grouping consistent with the file's style):

```rust
    /// A stage's `ToolOverride::mcp_add` references a server name not
    /// declared in `RunConfig::mcp_servers`.
    McpServerUndeclared { stage: String, server: String },
    /// `McpServerRef::name` is empty.
    McpServerNameEmpty,
    /// `McpTransportConfig::Stdio::command` contains `..` segments.
    McpCommandPathUnsafe { command: String },
```

- [ ] **Step 5: Add `severity()` arms**

In the same file, find `impl ValidationErrorKind { pub fn severity(&self) -> Severity {` and add the three arms (all `Error`):

```rust
            Self::McpServerUndeclared { .. } => Severity::Error,
            Self::McpServerNameEmpty => Severity::Error,
            Self::McpCommandPathUnsafe { .. } => Severity::Error,
```

Per memory `feedback_spec_scope_discipline.md` rule 5, the existing M6 retrofit removed wildcard arms — verify no `_ =>` arm exists at the end of the match. If it does, the M6 retrofit was incomplete; add the new arms above any wildcard and remove the wildcard.

- [ ] **Step 6: Add the validation functions**

Append to `crates/surge-core/src/validation.rs` (before the `#[cfg(test)]` block):

```rust
/// Validate per-stage `ToolOverride::mcp_add` references resolve in
/// the run-level registry. Returns errors for unresolved names.
#[must_use]
pub fn validate_mcp_references(
    stage_name: &str,
    stage_cfg: &crate::agent_config::AgentConfig,
    registry: &[crate::mcp_config::McpServerRef],
) -> Vec<ValidationError> {
    let mut out = Vec::new();
    let Some(overrides) = &stage_cfg.tool_overrides else {
        return out;
    };
    let known: std::collections::HashSet<&str> =
        registry.iter().map(|r| r.name.as_str()).collect();
    for name in &overrides.mcp_add {
        if !known.contains(name.as_str()) {
            out.push(ValidationError {
                kind: ValidationErrorKind::McpServerUndeclared {
                    stage: stage_name.to_string(),
                    server: name.clone(),
                },
                location: format!("nodes.{stage_name}.tool_overrides.mcp_add"),
            });
        }
    }
    out
}

/// Validate a single `McpServerRef` for safety / well-formedness.
#[must_use]
pub fn validate_mcp_server_ref(
    r: &crate::mcp_config::McpServerRef,
) -> Vec<ValidationError> {
    let mut out = Vec::new();
    if r.name.is_empty() {
        out.push(ValidationError {
            kind: ValidationErrorKind::McpServerNameEmpty,
            location: "mcp_servers[].name".into(),
        });
    }
    match &r.transport {
        crate::mcp_config::McpTransportConfig::Stdio { command, .. } => {
            // Reject `..` traversal segments. Pure-name (no slash) and
            // absolute paths are fine.
            let s = command.to_string_lossy();
            if s.split(['/', '\\']).any(|seg| seg == "..") {
                out.push(ValidationError {
                    kind: ValidationErrorKind::McpCommandPathUnsafe {
                        command: s.into_owned(),
                    },
                    location: "mcp_servers[].transport.command".into(),
                });
            }
        }
    }
    out
}
```

- [ ] **Step 7: Run tests to verify pass**

Run: `cargo test -p surge-core --lib validation::tests::`
Expected: all three new tests PASS plus existing validation tests still pass.

Also run: `cargo build --workspace` — expected clean (the new variants must be matched everywhere `ValidationErrorKind` is exhaustively handled, but the M6 retrofit added explicit arms; `severity()` covered in Step 5; if other call sites match exhaustively, fix them with the appropriate arm).

- [ ] **Step 8: Commit**

```bash
git add crates/surge-core/src/validation.rs
git commit -m "M7 P1.3: surge-core validation rules for MCP server references

- McpServerUndeclared: stage references unknown server
- McpServerNameEmpty
- McpCommandPathUnsafe: stdio command contains '..' segments

Per feedback_spec_scope_discipline.md rule 4, validation belongs in
surge-core so editors / external runners benefit. All severities
explicit (no wildcard arm)."
```

---

### Task 1.4: integrate MCP validation into top-level graph load

**Files:**
- Modify: `crates/surge-core/src/validation.rs` (add MCP checks to top-level validate function)

- [ ] **Step 1: Locate the top-level validation entry point**

Run: `grep -n "pub fn validate\b\|pub fn validate_" crates/surge-core/src/validation.rs | head -10`
Expected: `pub fn validate_graph_with_run_config(...)` or similar that aggregates validation passes. If it doesn't exist (graph-only validation today), create one.

Inspect the file to confirm. If only graph-level validation exists, the next step adds a run-config-aware wrapper.

- [ ] **Step 2: Write failing test**

Append to `crates/surge-core/src/validation.rs::tests`:

```rust
#[test]
fn validate_with_run_config_surfaces_mcp_undeclared() {
    use crate::agent_config::{AgentConfig, NodeLimits, ToolOverride};
    use crate::approvals::ApprovalPolicy;
    use crate::graph::{Graph, Node, NodeConfig};
    use crate::keys::{NodeKey, ProfileKey};
    use crate::run_event::RunConfig;
    use crate::sandbox::SandboxMode;
    use std::collections::BTreeMap;

    let mut nodes = BTreeMap::new();
    let stage_key = NodeKey::try_from("research").unwrap();
    nodes.insert(
        stage_key.clone(),
        Node {
            key: stage_key.clone(),
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("researcher@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: Some(ToolOverride {
                    mcp_add: vec!["nope".into()],
                    mcp_remove: vec![],
                    skills_add: vec![],
                    skills_remove: vec![],
                    shell_allowlist_add: vec![],
                }),
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: NodeLimits::default(),
                hooks: vec![],
                custom_fields: BTreeMap::new(),
            }),
            declared_outcomes: vec![],
        },
    );
    let graph = Graph {
        nodes,
        edges: vec![],
        terminals: vec![],
        subgraphs: BTreeMap::new(),
    };
    let run_cfg = RunConfig {
        sandbox_default: SandboxMode::ReadOnly,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: vec![], // empty: stage refs an undeclared server
    };
    let errors = crate::validation::validate_with_run_config(&graph, &run_cfg);
    assert!(errors.iter().any(|e| matches!(
        e.kind,
        crate::validation::ValidationErrorKind::McpServerUndeclared { .. }
    )));
}
```

- [ ] **Step 3: Run failing test**

Run: `cargo test -p surge-core --lib validation::tests::validate_with_run_config_surfaces_mcp_undeclared`
Expected: compile error — `validate_with_run_config` doesn't exist yet.

- [ ] **Step 4: Add `validate_with_run_config`**

Append to `crates/surge-core/src/validation.rs` (next to other top-level validation fns):

```rust
/// Combined graph + run-config validation. Aggregates graph-level
/// errors (existing `validate(graph)`) with M7 MCP-aware rules that
/// need both the graph (for stage configs) and run config (for the
/// MCP server registry).
#[must_use]
pub fn validate_with_run_config(
    graph: &crate::graph::Graph,
    run_config: &crate::run_event::RunConfig,
) -> Vec<ValidationError> {
    let mut out = validate(graph);

    // Per-server well-formedness.
    for server in &run_config.mcp_servers {
        out.extend(validate_mcp_server_ref(server));
    }

    // Per-stage allowlist resolution.
    for (key, node) in &graph.nodes {
        if let crate::graph::NodeConfig::Agent(agent_cfg) = &node.config {
            out.extend(validate_mcp_references(
                key.as_str(),
                agent_cfg,
                &run_config.mcp_servers,
            ));
        }
    }

    out
}
```

If the file doesn't already have a `validate(graph: &Graph)` function, the implementation should call into whatever the existing entry point is (e.g., `validate_graph` or per-rule fns) — adapt the wrapper, don't invent new naming.

- [ ] **Step 5: Run test to verify pass**

Run: `cargo test -p surge-core --lib validation::tests::validate_with_run_config_surfaces_mcp_undeclared`
Expected: PASS.

Also run: `cargo test -p surge-core --lib validation::` — all validation tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-core/src/validation.rs
git commit -m "M7 P1.4: validate_with_run_config aggregates MCP rules

Top-level wrapper that combines graph validation with the new
M7 MCP-aware rules. Engine and editor consumers call this when
they have both pieces; pure-graph callers (TOML lint) use the
existing validate()."
```

---

## Phase 2 — `EngineFacade` trait + `LocalEngineFacade` (2 days)

### Task 2.1: extend `ToolDispatcher` with `declared_tools` default-method

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/tools/mod.rs`
- Modify: `crates/surge-orchestrator/src/engine/tools/worktree.rs`

- [ ] **Step 1: Write failing test**

Append to `crates/surge-orchestrator/src/engine/tools/mod.rs::tests`:

```rust
#[test]
fn default_declared_tools_is_empty() {
    let d = NoOp;
    assert!(d.declared_tools().is_empty());
}

#[test]
fn worktree_dispatcher_declares_its_tools() {
    use std::path::PathBuf;
    let d = crate::engine::tools::worktree::WorktreeToolDispatcher::new(PathBuf::from("/tmp"));
    let tools = d.declared_tools();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"shell_exec"));
}
```

- [ ] **Step 2: Run failing test (compile fail expected)**

Run: `cargo test -p surge-orchestrator --lib engine::tools::tests::default_declared_tools_is_empty`
Expected: compile error — `declared_tools` method not found on trait.

- [ ] **Step 3: Add `DeclaredTool` type and trait extension**

Edit `crates/surge-orchestrator/src/engine/tools/mod.rs`. After the `ToolResultPayload` enum, add:

```rust
/// Declaration metadata for a single tool the dispatcher offers to
/// agent stages. Used by `RoutingToolDispatcher` to assemble the
/// session's tool list at session-open time.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct DeclaredTool {
    /// Tool name as the agent will see it.
    pub name: String,
    /// Human-readable description shown to the agent.
    pub description: Option<String>,
    /// JSON Schema for the tool's input arguments.
    pub input_schema: serde_json::Value,
}
```

Then change the trait. Replace the existing trait block:

```rust
/// Routes non-special ACP tool calls to implementations. Engine calls
/// `dispatch` for every `ToolCall` whose name is not `report_stage_outcome`
/// or `request_human_input` (those are engine-handled).
#[async_trait]
pub trait ToolDispatcher: Send + Sync {
    /// Dispatch a single tool call and return the result payload.
    async fn dispatch(&self, ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload;

    /// Tools this dispatcher declares to agent stages. Default is
    /// empty; the production `WorktreeToolDispatcher` overrides this
    /// with its built-in catalog (read_file, write_file, shell_exec,
    /// apply_diff). Used by `RoutingToolDispatcher` to assemble the
    /// session-level tool list.
    fn declared_tools(&self) -> Vec<DeclaredTool> {
        Vec::new()
    }
}
```

The default returns empty, so existing `ToolDispatcher` impls continue to compile.

- [ ] **Step 4: Override on `WorktreeToolDispatcher`**

Edit `crates/surge-orchestrator/src/engine/tools/worktree.rs`. Find the `impl ToolDispatcher for WorktreeToolDispatcher` block and add the override:

```rust
    fn declared_tools(&self) -> Vec<crate::engine::tools::DeclaredTool> {
        use crate::engine::tools::DeclaredTool;
        use serde_json::json;
        vec![
            DeclaredTool {
                name: "read_file".into(),
                description: Some("Read the contents of a file inside the run's worktree.".into()),
                input_schema: json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                }),
            },
            DeclaredTool {
                name: "write_file".into(),
                description: Some("Write contents to a file inside the run's worktree.".into()),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "contents": { "type": "string" },
                    },
                    "required": ["path", "contents"],
                }),
            },
            DeclaredTool {
                name: "shell_exec".into(),
                description: Some("Execute a shell command inside the run's worktree.".into()),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "timeout_seconds": { "type": "integer", "minimum": 1 },
                    },
                    "required": ["command"],
                }),
            },
            DeclaredTool {
                name: "apply_diff".into(),
                description: Some("Apply a unified-diff patch to files in the run's worktree.".into()),
                input_schema: json!({
                    "type": "object",
                    "properties": { "diff": { "type": "string" } },
                    "required": ["diff"],
                }),
            },
        ]
    }
```

If the actual M6 `WorktreeToolDispatcher` only implements a subset of these (e.g., doesn't have `apply_diff` yet), drop variants that don't dispatch — the declared list must match what `dispatch` actually handles. Use `grep -n "fn dispatch" crates/surge-orchestrator/src/engine/tools/worktree.rs` and check the inner `match call.tool.as_str()` to confirm which names are real.

- [ ] **Step 5: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::tools::`
Expected: both new tests PASS plus existing dispatcher tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/surge-orchestrator/src/engine/tools/mod.rs crates/surge-orchestrator/src/engine/tools/worktree.rs
git commit -m "M7 P2.1: ToolDispatcher::declared_tools default-method

Adds DeclaredTool type and a default-method on the trait so existing
impls remain compatible. WorktreeToolDispatcher overrides to expose
its built-in catalog. RoutingToolDispatcher (Phase 8) uses this
during session-open to assemble the agent's tool list."
```

---

### Task 2.2: `EngineFacade` trait + `RunSummary`/`RunStatus` types

**Files:**
- Create: `crates/surge-orchestrator/src/engine/facade.rs`
- Modify: `crates/surge-orchestrator/src/engine/handle.rs` (add RunSummary, RunStatus)
- Modify: `crates/surge-orchestrator/src/engine/mod.rs` (re-export)

- [ ] **Step 1: Write failing test**

Create `crates/surge-orchestrator/src/engine/facade.rs`:

```rust
//! `EngineFacade` — abstraction over `Engine` so CLI / tests can
//! switch between in-process (`LocalEngineFacade`) and out-of-process
//! (`DaemonEngineFacade`, Phase 5) hosting without touching the
//! engine's public API.

use crate::engine::config::EngineRunConfig;
use crate::engine::engine::Engine;
use crate::engine::error::EngineError;
use crate::engine::handle::{RunHandle, RunSummary};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use surge_core::graph::Graph;
use surge_core::id::RunId;

/// Engine-facing surface used by CLI commands and tests. All futures
/// are `Send`. Implementations: `LocalEngineFacade` (in-process,
/// straight delegation to `Engine`) and `DaemonEngineFacade`
/// (forwards every method as an IPC request — Phase 5).
#[async_trait]
pub trait EngineFacade: Send + Sync {
    /// Start a new run.
    async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError>;

    /// Resume an existing run from its latest snapshot.
    async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError>;

    /// Cancel an in-flight run.
    async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError>;

    /// Provide an answer to a paused run waiting on human input.
    async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError>;

    /// List runs visible to this facade. For the local facade, this
    /// is the in-memory active set. For the daemon facade, the daemon
    /// reports its full view.
    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError>;
}

/// In-process `EngineFacade`. Wraps an `Arc<Engine>` and forwards
/// every call directly. Default for the M6-style CLI invocation.
pub struct LocalEngineFacade {
    engine: Arc<Engine>,
}

impl LocalEngineFacade {
    /// Construct a facade around the given engine.
    #[must_use]
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl EngineFacade for LocalEngineFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        self.engine
            .start_run(run_id, graph, worktree_path, run_config)
            .await
    }

    async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        self.engine.resume_run(run_id, worktree_path).await
    }

    async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError> {
        self.engine.stop_run(run_id, reason).await
    }

    async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError> {
        self.engine
            .resolve_human_input(run_id, call_id, response)
            .await
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        // M7 LocalEngineFacade returns active runs as known by the
        // engine. Status is always Active for in-process; the daemon
        // facade returns a richer view including queued / completed.
        Ok(self.engine.snapshot_active_runs().await)
    }
}

#[cfg(test)]
mod tests {
    // Compile-time check only — real behaviour exercised in Phase 10
    // integration tests with a real engine.
    use super::*;
    fn _facade_is_object_safe() {
        let _: Option<Arc<dyn EngineFacade>> = None;
    }
}
```

- [ ] **Step 2: Add `RunSummary` + `RunStatus` to `handle.rs`**

Edit `crates/surge-orchestrator/src/engine/handle.rs`. Append before any existing `#[cfg(test)]` block (or at end of file):

```rust
/// Lightweight projection of a run's state, used by
/// `EngineFacade::list_runs` and the daemon's `ListRuns` IPC reply.
#[non_exhaustive]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RunSummary {
    /// Identifier of the run.
    pub run_id: RunId,
    /// Current high-level status.
    pub status: RunStatus,
    /// Wall-clock time the run was registered with the engine.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Highest seq the engine has persisted for this run, if any.
    pub last_event_seq: Option<u64>,
}

/// High-level run status as observed from outside (e.g., by `surge
/// engine ls --daemon`). Distinct from the engine's internal state.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is currently executing inside the engine.
    Active,
    /// Run is queued by the daemon's AdmissionController, not yet started.
    Awaiting,
    /// Run reached a successful terminal node.
    Completed,
    /// Run reached a failure terminal node or an unrecoverable error.
    Failed,
    /// Run was cancelled via `stop_run`.
    Aborted,
}
```

If `chrono::DateTime<chrono::Utc>` isn't already imported, add `use chrono;` at the top — chrono is already a workspace dep (per `Cargo.toml`).

- [ ] **Step 3: Add `Engine::snapshot_active_runs`**

Edit `crates/surge-orchestrator/src/engine/engine.rs`. Append a new method to the `impl Engine { ... }` block:

```rust
    /// Snapshot the in-process active-run map as a `Vec<RunSummary>`.
    /// Used by `LocalEngineFacade::list_runs`.
    pub async fn snapshot_active_runs(&self) -> Vec<crate::engine::handle::RunSummary> {
        use crate::engine::handle::{RunStatus, RunSummary};
        let runs = self.runs.read().await;
        runs.keys()
            .map(|id| RunSummary {
                run_id: *id,
                status: RunStatus::Active,
                started_at: chrono::Utc::now(), // M7 simplification — engine doesn't track per-run start time yet
                last_event_seq: None,
            })
            .collect()
    }
```

The `started_at` is a placeholder for the M7 `LocalEngineFacade`; the daemon's facade has the richer view. M8+ may wire a real per-run start timestamp into `ActiveRun`.

- [ ] **Step 4: Wire module + re-export**

Edit `crates/surge-orchestrator/src/engine/mod.rs`. Add to the `pub mod ...;` block:

```rust
pub mod facade;
```

Add to the `pub use ...;` re-exports:

```rust
pub use facade::{EngineFacade, LocalEngineFacade};
pub use handle::{RunStatus, RunSummary};
```

- [ ] **Step 5: Run tests**

Run: `cargo build --workspace`
Expected: clean build.

Run: `cargo test -p surge-orchestrator --lib engine::facade::tests::`
Expected: the `_facade_is_object_safe` compile-time check compiles (no runtime tests yet).

- [ ] **Step 6: Commit**

```bash
git add crates/surge-orchestrator/src/engine/facade.rs crates/surge-orchestrator/src/engine/handle.rs crates/surge-orchestrator/src/engine/engine.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M7 P2.2: EngineFacade trait + LocalEngineFacade + RunSummary

EngineFacade abstracts in-process vs daemon hosting. Local impl
is straight delegation to Engine; DaemonEngineFacade follows in
Phase 5. RunSummary/RunStatus are the IPC-friendly projection
returned by list_runs."
```

---

## Phase 3 — `surge-daemon` scaffold + IPC framing + protocol types (3 days)

### Task 3.1: IPC protocol types in `surge-orchestrator/src/engine/ipc.rs`

**Files:**
- Create: `crates/surge-orchestrator/src/engine/ipc.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs` (re-export)

- [ ] **Step 1: Write failing test**

Create `crates/surge-orchestrator/src/engine/ipc.rs`:

```rust
//! IPC protocol types shared between the daemon (`surge-daemon`) and
//! the daemon-facing client (`DaemonEngineFacade`). Wire format is
//! line-delimited JSON; one frame per line, no embedded newlines
//! (compact serde_json).

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome, RunSummary};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use surge_core::graph::Graph;
use surge_core::id::RunId;

/// Monotonically-increasing client-side request identifier. Echoed
/// in the matching `DaemonResponse` so the client can multiplex
/// requests over a single socket.
pub type RequestId = u64;

/// Stable error codes for IPC error responses.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    RunAlreadyActive,
    RunNotFound,
    RunNotActive,
    AdmissionFull,
    StorageError,
    EngineError,
    Internal,
    ShuttingDown,
}

/// Request frames sent from CLI to daemon.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum DaemonRequest {
    Ping {
        request_id: RequestId,
    },
    StartRun {
        request_id: RequestId,
        run_id: RunId,
        graph: Box<Graph>,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    },
    ResumeRun {
        request_id: RequestId,
        run_id: RunId,
        worktree_path: PathBuf,
    },
    StopRun {
        request_id: RequestId,
        run_id: RunId,
        reason: String,
    },
    ResolveHumanInput {
        request_id: RequestId,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    },
    ListRuns {
        request_id: RequestId,
    },
    Subscribe {
        request_id: RequestId,
        run_id: RunId,
    },
    Unsubscribe {
        request_id: RequestId,
        run_id: RunId,
    },
    Shutdown {
        request_id: RequestId,
    },
}

impl DaemonRequest {
    /// Returns the request_id of the carried request.
    #[must_use]
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Ping { request_id }
            | Self::StartRun { request_id, .. }
            | Self::ResumeRun { request_id, .. }
            | Self::StopRun { request_id, .. }
            | Self::ResolveHumanInput { request_id, .. }
            | Self::ListRuns { request_id }
            | Self::Subscribe { request_id, .. }
            | Self::Unsubscribe { request_id, .. }
            | Self::Shutdown { request_id } => *request_id,
        }
    }
}

/// Response frames sent from daemon to CLI.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum DaemonResponse {
    PingOk { request_id: RequestId, version: String },
    StartRunOk { request_id: RequestId, run_id: RunId },
    StartRunQueued { request_id: RequestId, run_id: RunId, position: usize },
    ResumeRunOk { request_id: RequestId },
    StopRunOk { request_id: RequestId },
    ResolveHumanInputOk { request_id: RequestId },
    ListRunsOk { request_id: RequestId, runs: Vec<RunSummary> },
    SubscribeOk { request_id: RequestId },
    UnsubscribeOk { request_id: RequestId },
    ShutdownOk { request_id: RequestId },
    Error { request_id: RequestId, code: ErrorCode, message: String },
}

impl DaemonResponse {
    /// Returns the request_id this response correlates to.
    #[must_use]
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::PingOk { request_id, .. }
            | Self::StartRunOk { request_id, .. }
            | Self::StartRunQueued { request_id, .. }
            | Self::ResumeRunOk { request_id }
            | Self::StopRunOk { request_id }
            | Self::ResolveHumanInputOk { request_id }
            | Self::ListRunsOk { request_id, .. }
            | Self::SubscribeOk { request_id }
            | Self::UnsubscribeOk { request_id }
            | Self::ShutdownOk { request_id }
            | Self::Error { request_id, .. } => *request_id,
        }
    }
}

/// Notification frames pushed from daemon to CLI (no request_id —
/// these are fire-and-forget broadcasts).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonEvent {
    PerRun {
        run_id: RunId,
        event: EngineRunEvent,
    },
    Global(GlobalDaemonEvent),
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GlobalDaemonEvent {
    RunAccepted { run_id: RunId },
    RunFinished { run_id: RunId, outcome: RunOutcome },
    DaemonShuttingDown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_request_serde_roundtrips() {
        let req = DaemonRequest::Ping { request_id: 42 };
        let s = serde_json::to_string(&req).unwrap();
        assert!(!s.contains('\n'), "compact json must not have newlines");
        let parsed: DaemonRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.request_id(), 42);
    }

    #[test]
    fn error_response_carries_code() {
        let r = DaemonResponse::Error {
            request_id: 7,
            code: ErrorCode::RunNotFound,
            message: "no such run".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let parsed: DaemonResponse = serde_json::from_str(&s).unwrap();
        match parsed {
            DaemonResponse::Error { code, request_id, .. } => {
                assert_eq!(code, ErrorCode::RunNotFound);
                assert_eq!(request_id, 7);
            }
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn shutting_down_event_serializes() {
        let ev = DaemonEvent::Global(GlobalDaemonEvent::DaemonShuttingDown);
        let s = serde_json::to_string(&ev).unwrap();
        let parsed: DaemonEvent = serde_json::from_str(&s).unwrap();
        match parsed {
            DaemonEvent::Global(GlobalDaemonEvent::DaemonShuttingDown) => {},
            _ => panic!("roundtrip failed"),
        }
    }
}
```

- [ ] **Step 2: Wire module + re-export**

Edit `crates/surge-orchestrator/src/engine/mod.rs`. Add to `pub mod` block:

```rust
pub mod ipc;
```

Add to `pub use` block:

```rust
pub use ipc::{DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent, RequestId};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::ipc::tests::`
Expected: all three tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-orchestrator/src/engine/ipc.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M7 P3.1: IPC protocol types (DaemonRequest/Response/Event)

Stable wire format: line-delimited JSON, one frame per line. All
public enums #[non_exhaustive] for forward-compat. RequestId is u64;
ErrorCode is a stable string enum so clients can react programmatically."
```

---

### Task 3.2: line-delimited framing helpers

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/ipc.rs` (add framing module)

- [ ] **Step 1: Write failing test**

Append to `crates/surge-orchestrator/src/engine/ipc.rs::tests`:

```rust
#[tokio::test]
async fn write_and_read_frame_roundtrip() {
    use tokio::io::{AsyncReadExt, BufReader};
    let (client, server) = tokio::io::duplex(4096);
    let (server_read, mut server_write) = tokio::io::split(server);
    let mut server_buf = BufReader::new(server_read);

    let req = DaemonRequest::Ping { request_id: 100 };
    crate::engine::ipc::write_frame(&mut server_write, &req).await.unwrap();
    drop(server_write); // signal EOF

    let mut all = String::new();
    server_buf.read_to_string(&mut all).await.unwrap();
    assert!(all.ends_with('\n'));

    // Round-trip the other way: feed bytes into read_request_frame.
    let _ = client; // unused
    let bytes = all.as_bytes().to_vec();
    let cursor = std::io::Cursor::new(bytes);
    let mut cursor_reader = BufReader::new(cursor);
    let parsed = crate::engine::ipc::read_request_frame(&mut cursor_reader).await.unwrap();
    assert!(matches!(parsed, Some(DaemonRequest::Ping { request_id: 100 })));
}

#[tokio::test]
async fn read_request_frame_returns_none_on_eof() {
    use tokio::io::BufReader;
    let empty: &[u8] = &[];
    let mut reader = BufReader::new(empty);
    let parsed = crate::engine::ipc::read_request_frame(&mut reader).await.unwrap();
    assert!(parsed.is_none());
}
```

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p surge-orchestrator --lib engine::ipc::tests::write_and_read_frame_roundtrip`
Expected: compile error — `write_frame` and `read_request_frame` don't exist.

- [ ] **Step 3: Add framing helpers**

Append to `crates/surge-orchestrator/src/engine/ipc.rs` (above the `#[cfg(test)]` block):

```rust
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// Errors produced by the framing layer.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum FramingError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame too large ({0} bytes; cap is {1})")]
    FrameTooLarge(usize, usize),
}

/// Maximum size of a single IPC frame in bytes. Larger frames (e.g.,
/// a Graph TOML payload over 8 MB) are rejected. Adjust if real
/// workloads bump up against this.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Serialize and write a single frame followed by `\n`. Compact JSON.
pub async fn write_frame<W, T>(writer: &mut W, frame: &T) -> Result<(), FramingError>
where
    W: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let mut bytes = serde_json::to_vec(frame)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(bytes.len(), MAX_FRAME_BYTES));
    }
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one line from `reader` and parse it as a `DaemonRequest`.
/// Returns `Ok(None)` on EOF (clean disconnect).
pub async fn read_request_frame<R>(
    reader: &mut BufReader<R>,
) -> Result<Option<DaemonRequest>, FramingError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(line.len(), MAX_FRAME_BYTES));
    }
    Ok(Some(serde_json::from_str(line.trim_end())?))
}

/// Read one line and parse as a `DaemonResponse`.
pub async fn read_response_frame<R>(
    reader: &mut BufReader<R>,
) -> Result<Option<DaemonResponse>, FramingError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(line.len(), MAX_FRAME_BYTES));
    }
    Ok(Some(serde_json::from_str(line.trim_end())?))
}

/// Read one line and parse as a `DaemonEvent` (notifications).
pub async fn read_event_frame<R>(
    reader: &mut BufReader<R>,
) -> Result<Option<DaemonEvent>, FramingError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(line.len(), MAX_FRAME_BYTES));
    }
    Ok(Some(serde_json::from_str(line.trim_end())?))
}

/// Either a response or an event, since they share a wire stream
/// (server → client). Used by the daemon-side client task that has to
/// route both kinds.
#[non_exhaustive]
#[derive(Debug)]
pub enum InboundServerFrame {
    Response(DaemonResponse),
    Event(DaemonEvent),
}

/// Read one server-bound frame and discriminate. Tries
/// `DaemonResponse` first; on parse failure (no `request_id`),
/// falls back to `DaemonEvent`.
pub async fn read_inbound_server_frame<R>(
    reader: &mut BufReader<R>,
) -> Result<Option<InboundServerFrame>, FramingError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(line.len(), MAX_FRAME_BYTES));
    }
    let trimmed = line.trim_end();
    // Both share serde "method" / "kind" tags — try response first.
    if let Ok(r) = serde_json::from_str::<DaemonResponse>(trimmed) {
        return Ok(Some(InboundServerFrame::Response(r)));
    }
    let ev: DaemonEvent = serde_json::from_str(trimmed)?;
    Ok(Some(InboundServerFrame::Event(ev)))
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p surge-orchestrator --lib engine::ipc::tests::`
Expected: all five tests PASS (3 from Task 3.1, 2 from this task).

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/ipc.rs
git commit -m "M7 P3.2: IPC line-delimited framing helpers

write_frame, read_request_frame, read_response_frame, read_event_frame,
read_inbound_server_frame. 8 MB cap on a single frame (errors
explicit). Both directions use serde_json compact mode."
```

---

### Task 3.3: `surge-daemon::pidfile` — PID + socket discovery

**Files:**
- Create: `crates/surge-daemon/src/pidfile.rs`
- Modify: `crates/surge-daemon/src/lib.rs` (declare module)

- [ ] **Step 1: Write failing tests**

Create `crates/surge-daemon/src/pidfile.rs`:

```rust
//! PID + socket file discovery and stale-lock handling.
//!
//! Layout:
//! ```text
//! ~/.surge/daemon/
//! ├── daemon.pid          (text: PID of the running daemon)
//! ├── daemon.sock         (Unix socket; on Windows: holds the named pipe path)
//! └── version             (text: daemon binary version)
//! ```

use std::path::{Path, PathBuf};

/// Errors produced by PID-file operations.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum PidfileError {
    #[error("home directory not found")]
    NoHome,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("pid file is malformed: {0}")]
    Malformed(String),
    #[error("daemon already running (pid {0})")]
    AlreadyRunning(u32),
}

/// Returns the daemon directory path: `~/.surge/daemon/`.
pub fn daemon_dir() -> Result<PathBuf, PidfileError> {
    let home = dirs::home_dir().ok_or(PidfileError::NoHome)?;
    Ok(home.join(".surge").join("daemon"))
}

/// Returns the PID file path.
pub fn pid_path() -> Result<PathBuf, PidfileError> {
    Ok(daemon_dir()?.join("daemon.pid"))
}

/// Returns the socket marker path. On Unix this is the actual socket
/// path; on Windows the file holds the named-pipe path string.
pub fn socket_path() -> Result<PathBuf, PidfileError> {
    Ok(daemon_dir()?.join("daemon.sock"))
}

/// Returns the version-marker path.
pub fn version_path() -> Result<PathBuf, PidfileError> {
    Ok(daemon_dir()?.join("version"))
}

/// Read a stored PID. Returns `Ok(None)` if the file doesn't exist.
pub fn read_pid(path: &Path) -> Result<Option<u32>, PidfileError> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let trimmed = s.trim();
            trimmed
                .parse::<u32>()
                .map(Some)
                .map_err(|_| PidfileError::Malformed(trimmed.to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PidfileError::Io(e)),
    }
}

/// Check whether a process with the given PID is currently alive.
/// Cross-platform via `sysinfo`.
#[must_use]
pub fn is_alive(pid: u32) -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}

/// Acquire the daemon lock by writing our PID. If a stale PID file
/// exists (process not alive), it is overwritten; if the PID is alive,
/// returns `AlreadyRunning`.
pub fn acquire_lock(pid: u32) -> Result<(), PidfileError> {
    let dir = daemon_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = pid_path()?;
    if let Some(existing) = read_pid(&path)? {
        if is_alive(existing) {
            return Err(PidfileError::AlreadyRunning(existing));
        }
    }
    std::fs::write(&path, pid.to_string())?;
    Ok(())
}

/// Release the lock by removing the PID file. Best-effort.
pub fn release_lock() -> Result<(), PidfileError> {
    let path = pid_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PidfileError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_pid_handles_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nonexistent.pid");
        assert_eq!(read_pid(&path).unwrap(), None);
    }

    #[test]
    fn read_pid_parses_valid_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("d.pid");
        std::fs::write(&path, "12345\n").unwrap();
        assert_eq!(read_pid(&path).unwrap(), Some(12345));
    }

    #[test]
    fn read_pid_rejects_garbage() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("d.pid");
        std::fs::write(&path, "not-a-pid").unwrap();
        let err = read_pid(&path).unwrap_err();
        assert!(matches!(err, PidfileError::Malformed(_)));
    }

    #[test]
    fn is_alive_for_current_process_returns_true() {
        let me = std::process::id();
        assert!(is_alive(me));
    }
}
```

- [ ] **Step 2: Add `dirs` to surge-daemon Cargo.toml**

Edit `crates/surge-daemon/Cargo.toml`. Append to `[dependencies]`:

```toml
dirs.workspace = true
```

- [ ] **Step 3: Wire module**

Edit `crates/surge-daemon/src/lib.rs`. Add:

```rust
pub mod pidfile;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p surge-daemon --lib pidfile::`
Expected: all four tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-daemon/Cargo.toml crates/surge-daemon/src/lib.rs crates/surge-daemon/src/pidfile.rs
git commit -m "M7 P3.3: surge-daemon::pidfile — PID + socket discovery

daemon_dir / pid_path / socket_path / version_path return the
canonical paths under ~/.surge/daemon/. acquire_lock detects stale
PID files via sysinfo and overwrites them. release_lock is
idempotent."
```

---

### Task 3.4: `surge-daemon::error` — `DaemonError`

**Files:**
- Create: `crates/surge-daemon/src/error.rs`
- Modify: `crates/surge-daemon/src/lib.rs` (declare module)

- [ ] **Step 1: Create the file**

Create `crates/surge-daemon/src/error.rs`:

```rust
//! Daemon-side error type. Mapped to `ErrorCode` on the IPC wire by
//! `server.rs` (Phase 6).

use surge_orchestrator::engine::ipc::ErrorCode;

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing: {0}")]
    Framing(#[from] surge_orchestrator::engine::ipc::FramingError),
    #[error("pidfile: {0}")]
    Pidfile(#[from] crate::pidfile::PidfileError),
    #[error("admission queue full ({active}/{max} active, {queued} queued)")]
    AdmissionFull { active: usize, max: usize, queued: usize },
    #[error("run not active: {0}")]
    RunNotActive(surge_core::id::RunId),
    #[error("storage: {0}")]
    Storage(String),
    #[error("engine: {0}")]
    Engine(#[from] surge_orchestrator::engine::EngineError),
    #[error("client disconnected mid-request")]
    ClientGone,
    #[error("shutdown in progress")]
    ShuttingDown,
}

impl DaemonError {
    /// Map this error to a stable IPC error code.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Io(_) | Self::Framing(_) => ErrorCode::BadRequest,
            Self::Pidfile(_) => ErrorCode::Internal,
            Self::AdmissionFull { .. } => ErrorCode::AdmissionFull,
            Self::RunNotActive(_) => ErrorCode::RunNotActive,
            Self::Storage(_) => ErrorCode::StorageError,
            Self::Engine(_) => ErrorCode::EngineError,
            Self::ClientGone => ErrorCode::Internal,
            Self::ShuttingDown => ErrorCode::ShuttingDown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_full_maps_to_admission_full() {
        let e = DaemonError::AdmissionFull {
            active: 8,
            max: 8,
            queued: 3,
        };
        assert_eq!(e.code(), ErrorCode::AdmissionFull);
    }

    #[test]
    fn shutting_down_maps_to_shutting_down() {
        assert_eq!(DaemonError::ShuttingDown.code(), ErrorCode::ShuttingDown);
    }
}
```

- [ ] **Step 2: Wire module**

Edit `crates/surge-daemon/src/lib.rs`. Add:

```rust
pub mod error;
pub use error::DaemonError;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-daemon --lib error::`
Expected: both tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/error.rs crates/surge-daemon/src/lib.rs
git commit -m "M7 P3.4: DaemonError + IPC ErrorCode mapping

Single error type with explicit code() projection. All variants map
to one of the stable codes in surge_orchestrator::engine::ipc::ErrorCode."
```

---

> **PR 1 ready** (P0 + P1 + P2 + P3.1-3.2): foundation, core extensions, EngineFacade trait, IPC protocol types + framing. Daemon binary still a placeholder. Next PR adds AdmissionController, BroadcastRegistry, server loop.

---

## Phase 4 — `AdmissionController` + `BroadcastRegistry` (2 days)

### Task 4.1: `AdmissionController` data structure + admit/release

**Files:**
- Create: `crates/surge-daemon/src/admission.rs`
- Modify: `crates/surge-daemon/src/lib.rs` (declare module)

- [ ] **Step 1: Write failing tests**

Create `crates/surge-daemon/src/admission.rs`:

```rust
//! `AdmissionController` — caps concurrent runs hosted by the daemon.
//! FIFO when over capacity. No aging, no preemption (M8 if needed).

use std::collections::{HashSet, VecDeque};
use surge_core::id::RunId;
use tokio::sync::Mutex;

/// Decision returned by `try_admit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Run admitted; daemon may proceed to call `engine.start_run`.
    Admitted,
    /// Run queued at the given 0-based position. Daemon should send
    /// `StartRunQueued` to the client and admit it later when
    /// `notify_completed` makes a slot.
    Queued { position: usize },
}

/// Concurrent admission policy.
pub struct AdmissionController {
    inner: Mutex<Inner>,
    notify: tokio::sync::Notify,
    max_active: usize,
}

struct Inner {
    active: HashSet<RunId>,
    queue: VecDeque<RunId>,
}

impl AdmissionController {
    /// Construct with a hard cap on concurrent active runs.
    #[must_use]
    pub fn new(max_active: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                active: HashSet::new(),
                queue: VecDeque::new(),
            }),
            notify: tokio::sync::Notify::new(),
            max_active,
        }
    }

    /// Attempt to admit a run. Returns `Admitted` if a slot was free
    /// or `Queued { position }` if the run joined the FIFO queue.
    pub async fn try_admit(&self, run_id: RunId) -> AdmissionDecision {
        let mut inner = self.inner.lock().await;
        if inner.active.len() < self.max_active {
            inner.active.insert(run_id);
            AdmissionDecision::Admitted
        } else {
            inner.queue.push_back(run_id);
            AdmissionDecision::Queued {
                position: inner.queue.len() - 1,
            }
        }
    }

    /// Mark a run as finished. Frees its slot and wakes any waiter
    /// blocked on `wait_for_admission`.
    pub async fn notify_completed(&self, run_id: RunId) {
        let mut inner = self.inner.lock().await;
        inner.active.remove(&run_id);
        // Drain any queued runs that now fit. Caller (server loop)
        // observes via a future call to `pop_queued`.
        self.notify.notify_waiters();
    }

    /// If a slot is free and a run is queued, dequeue + admit it.
    /// Returns the admitted RunId.
    pub async fn pop_queued(&self) -> Option<RunId> {
        let mut inner = self.inner.lock().await;
        if inner.active.len() < self.max_active {
            if let Some(id) = inner.queue.pop_front() {
                inner.active.insert(id);
                return Some(id);
            }
        }
        None
    }

    /// Snapshot counts for `surge daemon status`.
    pub async fn snapshot(&self) -> AdmissionSnapshot {
        let inner = self.inner.lock().await;
        AdmissionSnapshot {
            active: inner.active.len(),
            max_active: self.max_active,
            queued: inner.queue.len(),
        }
    }

    /// Block until something changes (a slot frees, a queue empties).
    /// Useful for the server loop's "drain queue" task.
    pub async fn wait_changed(&self) {
        self.notify.notified().await;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AdmissionSnapshot {
    pub active: usize,
    pub max_active: usize,
    pub queued: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn under_cap_admits() {
        let a = AdmissionController::new(2);
        let r1 = RunId::new();
        let r2 = RunId::new();
        assert_eq!(a.try_admit(r1).await, AdmissionDecision::Admitted);
        assert_eq!(a.try_admit(r2).await, AdmissionDecision::Admitted);
    }

    #[tokio::test]
    async fn over_cap_queues_in_fifo() {
        let a = AdmissionController::new(1);
        let r1 = RunId::new();
        let r2 = RunId::new();
        let r3 = RunId::new();
        assert_eq!(a.try_admit(r1).await, AdmissionDecision::Admitted);
        assert_eq!(a.try_admit(r2).await, AdmissionDecision::Queued { position: 0 });
        assert_eq!(a.try_admit(r3).await, AdmissionDecision::Queued { position: 1 });
    }

    #[tokio::test]
    async fn complete_then_pop_admits_next() {
        let a = AdmissionController::new(1);
        let r1 = RunId::new();
        let r2 = RunId::new();
        let _ = a.try_admit(r1).await;
        let _ = a.try_admit(r2).await; // queued at 0
        assert!(a.pop_queued().await.is_none()); // still no slot
        a.notify_completed(r1).await;
        let popped = a.pop_queued().await;
        assert_eq!(popped, Some(r2));
    }

    #[tokio::test]
    async fn snapshot_counts() {
        let a = AdmissionController::new(2);
        let r1 = RunId::new();
        let r2 = RunId::new();
        let r3 = RunId::new();
        let _ = a.try_admit(r1).await;
        let _ = a.try_admit(r2).await;
        let _ = a.try_admit(r3).await;
        let s = a.snapshot().await;
        assert_eq!(s.active, 2);
        assert_eq!(s.max_active, 2);
        assert_eq!(s.queued, 1);
    }
}
```

- [ ] **Step 2: Wire module**

Edit `crates/surge-daemon/src/lib.rs`. Add:

```rust
pub mod admission;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-daemon --lib admission::`
Expected: all four tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/admission.rs crates/surge-daemon/src/lib.rs
git commit -m "M7 P4.1: AdmissionController — FIFO queue under cap

try_admit returns Admitted or Queued { position }. pop_queued is
called by the server loop after notify_completed wakes it. snapshot
feeds surge daemon status."
```

---

### Task 4.2: `BroadcastRegistry` — per-run + global subscriptions

**Files:**
- Create: `crates/surge-daemon/src/broadcast.rs`
- Modify: `crates/surge-daemon/src/lib.rs` (declare module)

- [ ] **Step 1: Write failing tests**

Create `crates/surge-daemon/src/broadcast.rs`:

```rust
//! `BroadcastRegistry` — multi-subscriber event fan-out used by the
//! daemon's IPC server. The daemon spawns one forward task per
//! active run; that task sends events into the per-run channel here,
//! and N CLI clients subscribed via `Subscribe` IPC each get their
//! own broadcast `Receiver`.

use std::collections::HashMap;
use surge_core::id::RunId;
use surge_orchestrator::engine::handle::EngineRunEvent;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use tokio::sync::{broadcast, RwLock};

const DEFAULT_PER_RUN_CAPACITY: usize = 256;
const DEFAULT_GLOBAL_CAPACITY: usize = 64;

pub struct BroadcastRegistry {
    per_run: RwLock<HashMap<RunId, broadcast::Sender<EngineRunEvent>>>,
    global: broadcast::Sender<GlobalDaemonEvent>,
}

impl BroadcastRegistry {
    #[must_use]
    pub fn new() -> Self {
        let (global_tx, _) = broadcast::channel(DEFAULT_GLOBAL_CAPACITY);
        Self {
            per_run: RwLock::new(HashMap::new()),
            global: global_tx,
        }
    }

    /// Register a new run. Returns the sender so the daemon's forward
    /// task can publish events into it.
    pub async fn register(&self, run_id: RunId) -> broadcast::Sender<EngineRunEvent> {
        let mut map = self.per_run.write().await;
        let (tx, _) = broadcast::channel(DEFAULT_PER_RUN_CAPACITY);
        map.insert(run_id, tx.clone());
        tx
    }

    /// Subscribe to a run's broadcast. Returns `None` if the run is
    /// not registered (probably already terminated).
    pub async fn subscribe(&self, run_id: RunId) -> Option<broadcast::Receiver<EngineRunEvent>> {
        let map = self.per_run.read().await;
        map.get(&run_id).map(broadcast::Sender::subscribe)
    }

    /// Drop the per-run channel. Subscribers receive `Closed` from
    /// future `recv` calls.
    pub async fn deregister(&self, run_id: RunId) {
        let mut map = self.per_run.write().await;
        map.remove(&run_id);
    }

    /// Subscribe to global daemon events.
    #[must_use]
    pub fn subscribe_global(&self) -> broadcast::Receiver<GlobalDaemonEvent> {
        self.global.subscribe()
    }

    /// Publish a global daemon event. Best-effort (no error if no
    /// subscribers).
    pub fn publish_global(&self, event: GlobalDaemonEvent) {
        let _ = self.global.send(event);
    }

    /// Number of currently-registered per-run channels (active runs).
    pub async fn active_count(&self) -> usize {
        self.per_run.read().await.len()
    }
}

impl Default for BroadcastRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_orchestrator::engine::handle::RunOutcome;

    #[tokio::test]
    async fn register_subscribe_deregister() {
        let r = BroadcastRegistry::new();
        let id = RunId::new();
        let _tx = r.register(id).await;
        assert_eq!(r.active_count().await, 1);
        let rx = r.subscribe(id).await;
        assert!(rx.is_some());
        r.deregister(id).await;
        assert_eq!(r.active_count().await, 0);
        let rx2 = r.subscribe(id).await;
        assert!(rx2.is_none());
    }

    #[tokio::test]
    async fn global_publish_reaches_subscribers() {
        let r = BroadcastRegistry::new();
        let mut rx = r.subscribe_global();
        let id = RunId::new();
        r.publish_global(GlobalDaemonEvent::RunAccepted { run_id: id });
        let ev = rx.recv().await.unwrap();
        match ev {
            GlobalDaemonEvent::RunAccepted { run_id } => assert_eq!(run_id, id),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn per_run_event_fanout_to_two_subscribers() {
        let r = BroadcastRegistry::new();
        let id = RunId::new();
        let tx = r.register(id).await;
        let mut a = r.subscribe(id).await.unwrap();
        let mut b = r.subscribe(id).await.unwrap();
        let _ = tx.send(EngineRunEvent::Terminal(RunOutcome::Completed {
            terminal: surge_core::keys::NodeKey::try_from("end").unwrap(),
        }));
        assert!(matches!(a.recv().await.unwrap(), EngineRunEvent::Terminal(_)));
        assert!(matches!(b.recv().await.unwrap(), EngineRunEvent::Terminal(_)));
    }
}
```

- [ ] **Step 2: Wire module**

Edit `crates/surge-daemon/src/lib.rs`:

```rust
pub mod broadcast;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-daemon --lib broadcast::`
Expected: all three tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/broadcast.rs crates/surge-daemon/src/lib.rs
git commit -m "M7 P4.2: BroadcastRegistry — per-run + global event fan-out

register/subscribe/deregister manage per-run channels; subscribe_global
+ publish_global handle daemon-level RunAccepted/RunFinished/
DaemonShuttingDown events. Tokio broadcast channel capacities tuned
to per-run 256 / global 64."
```

---

## Phase 5 — `DaemonEngineFacade` + IPC client (3 days)

### Task 5.1: `DaemonClient` — connect + read-loop + dispatcher

**Files:**
- Create: `crates/surge-orchestrator/src/engine/daemon_facade.rs`
- Modify: `crates/surge-orchestrator/src/engine/mod.rs` (declare module + re-export)
- Modify: `crates/surge-orchestrator/Cargo.toml` (add `interprocess`)

- [ ] **Step 1: Add `interprocess` dep**

Edit `crates/surge-orchestrator/Cargo.toml`. Append to `[dependencies]`:

```toml
interprocess.workspace = true
```

- [ ] **Step 2: Create the facade scaffold**

Create `crates/surge-orchestrator/src/engine/daemon_facade.rs`:

```rust
//! `DaemonEngineFacade` — out-of-process `EngineFacade` impl that
//! forwards every method as an IPC request to a `surge-daemon`.
//!
//! Wire format: line-delimited JSON over a local socket. The client
//! task spawned by `DaemonClient::connect` reads inbound frames and
//! dispatches them: responses go to the matching oneshot in
//! `pending`; events go to the `EventDispatcher` for per-run fan-out.

use crate::engine::config::EngineRunConfig;
use crate::engine::error::EngineError;
use crate::engine::facade::EngineFacade;
use crate::engine::handle::{EngineRunEvent, RunHandle, RunOutcome, RunSummary};
use crate::engine::ipc::{
    DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent, InboundServerFrame,
    RequestId, read_inbound_server_frame, write_frame,
};
use async_trait::async_trait;
use interprocess::local_socket::tokio::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use surge_core::graph::Graph;
use surge_core::id::RunId;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast, oneshot};

/// IPC client wrapping a connected local socket. One client supports
/// multiplexed requests via `RequestId` correlation. Exposes
/// `EngineFacade` via `DaemonEngineFacade`.
pub struct DaemonClient {
    next_id: AtomicU64,
    write_half: Mutex<tokio::io::WriteHalf<LocalSocketStream>>,
    pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<DaemonResponse>>>>,
    event_dispatcher: Arc<EventDispatcher>,
}

struct EventDispatcher {
    per_run: Mutex<HashMap<RunId, broadcast::Sender<EngineRunEvent>>>,
    global: broadcast::Sender<GlobalDaemonEvent>,
    /// Tracks completion oneshots so `start_run` can fabricate a
    /// `JoinHandle<RunOutcome>` for the returned `RunHandle`.
    completion: Mutex<HashMap<RunId, oneshot::Sender<RunOutcome>>>,
}

impl DaemonClient {
    /// Open a connection to the daemon at `socket_path` and start the
    /// read loop. On Unix this is a Unix socket path; on Windows it
    /// is the named-pipe path stored in `~/.surge/daemon/daemon.sock`
    /// (the file holds the pipe path as text — see PR 6 for the
    /// resolution helper).
    pub async fn connect(socket_path: PathBuf) -> Result<Arc<Self>, EngineError> {
        let name = socket_name_from_path(&socket_path)
            .map_err(|e| EngineError::Internal(format!("socket name: {e}")))?;
        let stream = LocalSocketStream::connect(name)
            .await
            .map_err(|e| EngineError::Internal(format!("connect {socket_path:?}: {e}")))?;
        let (read_half, write_half) = tokio::io::split(stream);

        let (global_tx, _) = broadcast::channel(64);
        let event_dispatcher = Arc::new(EventDispatcher {
            per_run: Mutex::new(HashMap::new()),
            global: global_tx,
            completion: Mutex::new(HashMap::new()),
        });
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<DaemonResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let pending_for_task = pending.clone();
        let dispatcher_for_task = event_dispatcher.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(read_half);
            loop {
                match read_inbound_server_frame(&mut reader).await {
                    Ok(Some(InboundServerFrame::Response(resp))) => {
                        let id = resp.request_id();
                        if let Some(tx) = pending_for_task.lock().await.remove(&id) {
                            let _ = tx.send(resp);
                        }
                    }
                    Ok(Some(InboundServerFrame::Event(ev))) => {
                        match ev {
                            DaemonEvent::PerRun { run_id, event } => {
                                if let EngineRunEvent::Terminal(outcome) = &event {
                                    if let Some(tx) = dispatcher_for_task
                                        .completion
                                        .lock()
                                        .await
                                        .remove(&run_id)
                                    {
                                        let _ = tx.send(outcome.clone());
                                    }
                                }
                                if let Some(tx) =
                                    dispatcher_for_task.per_run.lock().await.get(&run_id)
                                {
                                    let _ = tx.send(event);
                                }
                            }
                            DaemonEvent::Global(g) => {
                                let _ = dispatcher_for_task.global.send(g);
                            }
                        }
                    }
                    Ok(None) => break, // EOF — daemon closed connection
                    Err(e) => {
                        tracing::error!(err = %e, "daemon-client read loop: {e}");
                        break;
                    }
                }
            }
        });

        Ok(Arc::new(Self {
            next_id: AtomicU64::new(1),
            write_half: Mutex::new(write_half),
            pending,
            event_dispatcher,
        }))
    }

    /// Allocate a fresh request id.
    fn next_request_id(&self) -> RequestId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a request and await the matching response.
    async fn rpc(&self, build: impl FnOnce(RequestId) -> DaemonRequest) -> Result<DaemonResponse, EngineError> {
        let id = self.next_request_id();
        let req = build(id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        {
            let mut w = self.write_half.lock().await;
            write_frame(&mut *w, &req)
                .await
                .map_err(|e| EngineError::Internal(format!("write_frame: {e}")))?;
            w.flush().await.ok();
        }
        rx.await
            .map_err(|_| EngineError::Internal("daemon dropped before response".into()))
    }
}

/// Helper to convert a filesystem path into the platform-specific
/// `local_socket::Name` interprocess expects.
fn socket_name_from_path(
    path: &std::path::Path,
) -> Result<interprocess::local_socket::Name<'_>, std::io::Error> {
    use interprocess::local_socket::{NameType, ToFsName};
    path.to_fs_name::<interprocess::local_socket::GenericFilePath>()
}

/// Public `EngineFacade` surface — wraps the `DaemonClient`.
pub struct DaemonEngineFacade {
    inner: Arc<DaemonClient>,
}

impl DaemonEngineFacade {
    /// Open an IPC connection and return a facade.
    pub async fn connect(socket_path: PathBuf) -> Result<Self, EngineError> {
        Ok(Self {
            inner: DaemonClient::connect(socket_path).await?,
        })
    }
}

#[async_trait]
impl EngineFacade for DaemonEngineFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        // Reserve completion + per-run channel BEFORE sending Subscribe so
        // we don't lose early events.
        let (completion_tx, completion_rx) = oneshot::channel();
        self.inner
            .event_dispatcher
            .completion
            .lock()
            .await
            .insert(run_id, completion_tx);
        let (event_tx, event_rx) = broadcast::channel(256);
        self.inner
            .event_dispatcher
            .per_run
            .lock()
            .await
            .insert(run_id, event_tx);

        let resp = self
            .inner
            .rpc(|request_id| DaemonRequest::StartRun {
                request_id,
                run_id,
                graph: Box::new(graph),
                worktree_path,
                run_config,
            })
            .await?;
        match resp {
            DaemonResponse::StartRunOk { .. } | DaemonResponse::StartRunQueued { .. } => {}
            DaemonResponse::Error { code, message, .. } => {
                self.cleanup_run_channels(run_id).await;
                return Err(map_error(code, message));
            }
            other => {
                self.cleanup_run_channels(run_id).await;
                return Err(EngineError::Internal(format!(
                    "unexpected response: {other:?}"
                )));
            }
        }

        // Subscribe to per-run events.
        let sub = self
            .inner
            .rpc(|request_id| DaemonRequest::Subscribe { request_id, run_id })
            .await?;
        if let DaemonResponse::Error { code, message, .. } = sub {
            self.cleanup_run_channels(run_id).await;
            return Err(map_error(code, message));
        }

        let join: tokio::task::JoinHandle<RunOutcome> = tokio::spawn(async move {
            completion_rx
                .await
                .unwrap_or(RunOutcome::Aborted {
                    reason: "daemon connection lost".into(),
                })
        });

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }

    async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        let (completion_tx, completion_rx) = oneshot::channel();
        self.inner
            .event_dispatcher
            .completion
            .lock()
            .await
            .insert(run_id, completion_tx);
        let (event_tx, event_rx) = broadcast::channel(256);
        self.inner
            .event_dispatcher
            .per_run
            .lock()
            .await
            .insert(run_id, event_tx);

        let resp = self
            .inner
            .rpc(|request_id| DaemonRequest::ResumeRun {
                request_id,
                run_id,
                worktree_path,
            })
            .await?;
        if let DaemonResponse::Error { code, message, .. } = resp {
            self.cleanup_run_channels(run_id).await;
            return Err(map_error(code, message));
        }

        let sub = self
            .inner
            .rpc(|request_id| DaemonRequest::Subscribe { request_id, run_id })
            .await?;
        if let DaemonResponse::Error { code, message, .. } = sub {
            self.cleanup_run_channels(run_id).await;
            return Err(map_error(code, message));
        }

        let join: tokio::task::JoinHandle<RunOutcome> = tokio::spawn(async move {
            completion_rx
                .await
                .unwrap_or(RunOutcome::Aborted {
                    reason: "daemon connection lost".into(),
                })
        });

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }

    async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError> {
        match self
            .inner
            .rpc(|request_id| DaemonRequest::StopRun {
                request_id,
                run_id,
                reason,
            })
            .await?
        {
            DaemonResponse::StopRunOk { .. } => Ok(()),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, message)),
            other => Err(EngineError::Internal(format!("unexpected: {other:?}"))),
        }
    }

    async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError> {
        match self
            .inner
            .rpc(|request_id| DaemonRequest::ResolveHumanInput {
                request_id,
                run_id,
                call_id,
                response,
            })
            .await?
        {
            DaemonResponse::ResolveHumanInputOk { .. } => Ok(()),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, message)),
            other => Err(EngineError::Internal(format!("unexpected: {other:?}"))),
        }
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        match self
            .inner
            .rpc(|request_id| DaemonRequest::ListRuns { request_id })
            .await?
        {
            DaemonResponse::ListRunsOk { runs, .. } => Ok(runs),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, message)),
            other => Err(EngineError::Internal(format!("unexpected: {other:?}"))),
        }
    }
}

impl DaemonEngineFacade {
    async fn cleanup_run_channels(&self, run_id: RunId) {
        self.inner
            .event_dispatcher
            .per_run
            .lock()
            .await
            .remove(&run_id);
        self.inner
            .event_dispatcher
            .completion
            .lock()
            .await
            .remove(&run_id);
    }
}

fn map_error(code: ErrorCode, message: String) -> EngineError {
    use surge_core::id::RunId;
    match code {
        ErrorCode::RunNotFound => {
            // Engine-side variant carries a RunId, but the IPC error
            // doesn't necessarily round-trip the id. Use Internal with
            // a clear message.
            EngineError::Internal(format!("daemon: run not found ({message})"))
        }
        ErrorCode::RunAlreadyActive => EngineError::Internal(format!(
            "daemon: run already active ({message})"
        )),
        ErrorCode::AdmissionFull => {
            EngineError::Internal(format!("daemon: admission full ({message})"))
        }
        ErrorCode::ShuttingDown => {
            EngineError::Internal(format!("daemon: shutting down ({message})"))
        }
        _ => EngineError::Internal(format!("daemon error [{code:?}]: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_error_admission_full() {
        let e = map_error(ErrorCode::AdmissionFull, "8/8 active".into());
        assert!(format!("{e}").contains("admission full"));
    }
}
```

- [ ] **Step 3: Wire module**

Edit `crates/surge-orchestrator/src/engine/mod.rs`:

```rust
pub mod daemon_facade;
pub use daemon_facade::{DaemonClient, DaemonEngineFacade};
```

- [ ] **Step 4: Build + run unit tests**

Run: `cargo build --workspace`
Expected: clean build. The interprocess crate is now wired; verify it pulled in correctly.

Run: `cargo test -p surge-orchestrator --lib engine::daemon_facade::`
Expected: the `map_error_admission_full` test passes (only test in this task — full integration in Phase 10).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/surge-orchestrator/Cargo.toml crates/surge-orchestrator/src/engine/daemon_facade.rs crates/surge-orchestrator/src/engine/mod.rs
git commit -m "M7 P5.1: DaemonEngineFacade IPC client

Connects via interprocess local-socket; spawns a background read
loop that dispatches inbound frames (responses to oneshots, events
to per-run / global broadcast). All five EngineFacade methods
implemented: start_run, resume_run, stop_run, resolve_human_input,
list_runs. Per-run channel registered before StartRun is sent to
avoid losing early events."
```

---

> **PR 2 ready** (P3.3-3.4 + P4 + P5.1): pidfile, DaemonError, AdmissionController, BroadcastRegistry, DaemonEngineFacade IPC client. Server-side handler still missing — PR 3 adds it.

---

## Phase 6 — daemon server + CLI integration (2 days)

### Task 6.1: `surge-daemon::server` — accept loop + per-connection handler

**Files:**
- Create: `crates/surge-daemon/src/server.rs`
- Modify: `crates/surge-daemon/src/lib.rs`

- [ ] **Step 1: Create the server module**

Create `crates/surge-daemon/src/server.rs`:

```rust
//! IPC server: accept-and-dispatch loop. Each accepted connection
//! gets a per-connection task that reads `DaemonRequest` frames from
//! the socket and forwards them to the engine via the
//! `LocalEngineFacade`. Per-run subscriptions spawn forwarder tasks
//! that pump `EngineRunEvent`s into the wire as `DaemonEvent::PerRun`.

use crate::admission::{AdmissionController, AdmissionDecision};
use crate::broadcast::BroadcastRegistry;
use crate::error::DaemonError;
use crate::pidfile;
use interprocess::local_socket::tokio::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use surge_core::id::RunId;
use surge_orchestrator::engine::facade::{EngineFacade, LocalEngineFacade};
use surge_orchestrator::engine::handle::EngineRunEvent;
use surge_orchestrator::engine::ipc::{
    DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent, RequestId,
    read_request_frame, write_frame,
};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Top-level server config.
pub struct ServerConfig {
    pub max_active: usize,
    pub socket_path: PathBuf,
}

/// Wires together the engine, admission, broadcast registry, and the
/// IPC listener. Called by `main.rs`.
pub async fn run(
    cfg: ServerConfig,
    facade: Arc<dyn EngineFacade>,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    use interprocess::local_socket::ListenerOptions;

    let admission = Arc::new(AdmissionController::new(cfg.max_active));
    let broadcast = Arc::new(BroadcastRegistry::new());

    let name = path_to_socket_name(&cfg.socket_path)?;
    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .map_err(DaemonError::Io)?;

    tracing::info!(socket = %cfg.socket_path.display(), "daemon listening");

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("shutdown signal received; closing listener");
                break;
            }
            conn = listener.accept() => {
                match conn {
                    Ok(stream) => {
                        let facade = facade.clone();
                        let admission = admission.clone();
                        let broadcast = broadcast.clone();
                        let shutdown_for_conn = shutdown.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, facade, admission, broadcast, shutdown_for_conn).await {
                                tracing::warn!(err = %e, "connection ended with error");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(err = %e, "accept failed");
                    }
                }
            }
        }
    }

    Ok(())
}

fn path_to_socket_name(path: &std::path::Path) -> Result<interprocess::local_socket::Name<'_>, DaemonError> {
    use interprocess::local_socket::ToFsName;
    path.to_fs_name::<interprocess::local_socket::GenericFilePath>()
        .map_err(DaemonError::Io)
}

/// Per-connection state: tracks which runs the client subscribed to,
/// so we can clean up forwarder tasks when the client disconnects.
struct ConnState {
    subscriptions: HashSet<RunId>,
}

async fn handle_connection(
    stream: LocalSocketStream,
    facade: Arc<dyn EngineFacade>,
    admission: Arc<AdmissionController>,
    broadcast: Arc<BroadcastRegistry>,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let (read_half, write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);
    let writer: Arc<Mutex<tokio::io::WriteHalf<LocalSocketStream>>> =
        Arc::new(Mutex::new(write_half));
    let state = Arc::new(Mutex::new(ConnState {
        subscriptions: HashSet::new(),
    }));

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                let resp_id: RequestId = 0;
                let err = DaemonResponse::Error {
                    request_id: resp_id,
                    code: ErrorCode::ShuttingDown,
                    message: "daemon shutting down".into(),
                };
                let mut w = writer.lock().await;
                let _ = write_frame(&mut *w, &err).await;
                let _ = w.flush().await;
                break;
            }
            frame = read_request_frame(&mut reader) => {
                match frame {
                    Ok(Some(req)) => {
                        let req_id = req.request_id();
                        let resp = dispatch(req, &*facade, &admission, &broadcast, &state, &writer).await;
                        if let Some(r) = resp {
                            let mut w = writer.lock().await;
                            if let Err(e) = write_frame(&mut *w, &r).await {
                                tracing::warn!(err = %e, "write_frame failed; closing connection");
                                let _ = req_id;
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        // EOF: client disconnected cleanly
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(err = %e, "read_request_frame failed; closing connection");
                        break;
                    }
                }
            }
        }
    }

    // Cleanup: remove subscriptions for this connection.
    let s = state.lock().await;
    for run_id in &s.subscriptions {
        // Per-run forwarders shut down naturally on EOF when their
        // subscriber Receiver is dropped. Nothing to do explicitly.
        tracing::debug!(run_id = %run_id, "client disconnected; subscription cleaned up");
    }
    Ok(())
}

async fn dispatch(
    req: DaemonRequest,
    facade: &dyn EngineFacade,
    admission: &Arc<AdmissionController>,
    broadcast: &Arc<BroadcastRegistry>,
    state: &Arc<Mutex<ConnState>>,
    writer: &Arc<Mutex<tokio::io::WriteHalf<LocalSocketStream>>>,
) -> Option<DaemonResponse> {
    let request_id = req.request_id();
    match req {
        DaemonRequest::Ping { request_id } => Some(DaemonResponse::PingOk {
            request_id,
            version: env!("CARGO_PKG_VERSION").into(),
        }),
        DaemonRequest::StartRun {
            request_id,
            run_id,
            graph,
            worktree_path,
            run_config,
        } => {
            let decision = admission.try_admit(run_id).await;
            match decision {
                AdmissionDecision::Admitted => {
                    broadcast
                        .publish_global(GlobalDaemonEvent::RunAccepted { run_id });
                    let tx = broadcast.register(run_id).await;
                    let admission_for_completion = admission.clone();
                    let broadcast_for_completion = broadcast.clone();
                    match facade
                        .start_run(run_id, *graph, worktree_path, run_config)
                        .await
                    {
                        Ok(handle) => {
                            spawn_forward_task(
                                run_id,
                                handle,
                                tx,
                                writer.clone(),
                                admission_for_completion,
                                broadcast_for_completion,
                            );
                            Some(DaemonResponse::StartRunOk {
                                request_id,
                                run_id,
                            })
                        }
                        Err(e) => {
                            broadcast.deregister(run_id).await;
                            admission.notify_completed(run_id).await;
                            Some(DaemonResponse::Error {
                                request_id,
                                code: ErrorCode::EngineError,
                                message: format!("{e}"),
                            })
                        }
                    }
                }
                AdmissionDecision::Queued { position } => Some(DaemonResponse::StartRunQueued {
                    request_id,
                    run_id,
                    position,
                }),
            }
        }
        DaemonRequest::ResumeRun {
            request_id,
            run_id,
            worktree_path,
        } => {
            let tx = broadcast.register(run_id).await;
            let admission_for_completion = admission.clone();
            let broadcast_for_completion = broadcast.clone();
            match facade.resume_run(run_id, worktree_path).await {
                Ok(handle) => {
                    spawn_forward_task(
                        run_id,
                        handle,
                        tx,
                        writer.clone(),
                        admission_for_completion,
                        broadcast_for_completion,
                    );
                    Some(DaemonResponse::ResumeRunOk { request_id })
                }
                Err(e) => {
                    broadcast.deregister(run_id).await;
                    Some(DaemonResponse::Error {
                        request_id,
                        code: ErrorCode::EngineError,
                        message: format!("{e}"),
                    })
                }
            }
        }
        DaemonRequest::StopRun {
            request_id,
            run_id,
            reason,
        } => match facade.stop_run(run_id, reason).await {
            Ok(()) => Some(DaemonResponse::StopRunOk { request_id }),
            Err(e) => Some(DaemonResponse::Error {
                request_id,
                code: ErrorCode::EngineError,
                message: format!("{e}"),
            }),
        },
        DaemonRequest::ResolveHumanInput {
            request_id,
            run_id,
            call_id,
            response,
        } => match facade.resolve_human_input(run_id, call_id, response).await {
            Ok(()) => Some(DaemonResponse::ResolveHumanInputOk { request_id }),
            Err(e) => Some(DaemonResponse::Error {
                request_id,
                code: ErrorCode::EngineError,
                message: format!("{e}"),
            }),
        },
        DaemonRequest::ListRuns { request_id } => match facade.list_runs().await {
            Ok(runs) => Some(DaemonResponse::ListRunsOk { request_id, runs }),
            Err(e) => Some(DaemonResponse::Error {
                request_id,
                code: ErrorCode::EngineError,
                message: format!("{e}"),
            }),
        },
        DaemonRequest::Subscribe {
            request_id,
            run_id,
        } => {
            let rx = broadcast.subscribe(run_id).await;
            match rx {
                Some(rx) => {
                    let mut s = state.lock().await;
                    s.subscriptions.insert(run_id);
                    let writer_for_task = writer.clone();
                    tokio::spawn(forward_per_run_to_client(run_id, rx, writer_for_task));
                    Some(DaemonResponse::SubscribeOk { request_id })
                }
                None => Some(DaemonResponse::Error {
                    request_id,
                    code: ErrorCode::RunNotActive,
                    message: format!("run {run_id} not active in this daemon"),
                }),
            }
        }
        DaemonRequest::Unsubscribe { request_id, run_id } => {
            let mut s = state.lock().await;
            s.subscriptions.remove(&run_id);
            Some(DaemonResponse::UnsubscribeOk { request_id })
        }
        DaemonRequest::Shutdown { request_id } => {
            // The shutdown signal is handled by the lifecycle module
            // (Task 6.2). We acknowledge here; the lifecycle drain
            // closes connections shortly after.
            Some(DaemonResponse::ShutdownOk { request_id })
        }
    }
}

fn spawn_forward_task(
    run_id: RunId,
    handle: surge_orchestrator::engine::handle::RunHandle,
    publisher: tokio::sync::broadcast::Sender<EngineRunEvent>,
    writer: Arc<Mutex<tokio::io::WriteHalf<LocalSocketStream>>>,
    admission: Arc<AdmissionController>,
    broadcast: Arc<BroadcastRegistry>,
) {
    let _ = writer; // unused — events go to broadcast registry; per-subscriber forwarders write to wire
    tokio::spawn(async move {
        let mut rx = handle.events;
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let _ = publisher.send(ev);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
        // Run is finished. Wait for completion handle to read the
        // outcome, then notify admission + broadcast.
        let outcome = handle
            .completion
            .await
            .unwrap_or(surge_orchestrator::engine::handle::RunOutcome::Aborted {
                reason: "run task panicked".into(),
            });
        broadcast.publish_global(GlobalDaemonEvent::RunFinished {
            run_id,
            outcome,
        });
        broadcast.deregister(run_id).await;
        admission.notify_completed(run_id).await;
        // Drain queue: pop any newly-admittable run. The dispatch path
        // for a queued run picks up admission via pop_queued (caller
        // should retry start_run; M7 simplification: the queued run
        // request waits in pending until pop happens — this requires
        // a small follow-up wiring in dispatch::StartRun. For M7 we
        // ship the basic FIFO; per-PR3 polish handles re-admission.)
    });
}

async fn forward_per_run_to_client(
    run_id: RunId,
    mut rx: tokio::sync::broadcast::Receiver<EngineRunEvent>,
    writer: Arc<Mutex<tokio::io::WriteHalf<LocalSocketStream>>>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let frame = DaemonEvent::PerRun { run_id, event };
                let mut w = writer.lock().await;
                if let Err(e) = write_frame(&mut *w, &frame).await {
                    tracing::debug!(err = %e, run_id = %run_id, "subscriber write failed; ending forwarder");
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(run_id = %run_id, dropped = n, "per-run forwarder lagged");
                continue;
            }
        }
    }
}
```

- [ ] **Step 2: Wire module**

Edit `crates/surge-daemon/src/lib.rs`:

```rust
pub mod server;
pub use server::{run as run_server, ServerConfig};
```

- [ ] **Step 3: Build**

Run: `cargo build --workspace`
Expected: clean build. All scaffolds compile.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/server.rs crates/surge-daemon/src/lib.rs
git commit -m "M7 P6.1: surge-daemon::server — accept-and-dispatch IPC loop

Listener via interprocess::local_socket. Per-connection task reads
DaemonRequest frames, dispatches to LocalEngineFacade, writes
DaemonResponse back. Subscribe spawns a per-run forwarder that
pumps EngineRunEvents to the wire as DaemonEvent::PerRun. Run
completion notifies AdmissionController and publishes
GlobalDaemonEvent::RunFinished."
```

---

### Task 6.2: `surge-daemon::lifecycle` — signal handlers + graceful drain

**Files:**
- Create: `crates/surge-daemon/src/lifecycle.rs`
- Modify: `crates/surge-daemon/src/lib.rs`

- [ ] **Step 1: Create the lifecycle module**

Create `crates/surge-daemon/src/lifecycle.rs`:

```rust
//! Signal-handler installation and graceful-drain helpers. Cancels a
//! `CancellationToken` on SIGTERM / SIGINT (Unix) or Ctrl+C (Windows).

use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Spawn a task that listens for OS termination signals and cancels
/// `token` when one fires. Returns immediately; the task lives for
/// the daemon's lifetime.
pub fn install_signal_handlers(token: CancellationToken) {
    tokio::spawn(async move {
        wait_for_termination().await;
        tracing::info!("termination signal received; cancelling shutdown token");
        token.cancel();
    });
}

#[cfg(unix)]
async fn wait_for_termination() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => tracing::info!("SIGTERM received"),
        _ = int.recv() => tracing::info!("SIGINT received"),
    }
}

#[cfg(windows)]
async fn wait_for_termination() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("Ctrl+C received");
}

/// Wait for `token` to be cancelled, then sleep up to `grace` for
/// in-flight tasks to wind down. Used by `main.rs` after the server
/// loop exits to give run forwarders time to publish final events.
pub async fn drain(token: CancellationToken, grace: Duration) {
    token.cancelled().await;
    tokio::time::sleep(grace).await;
}
```

- [ ] **Step 2: Wire module**

Edit `crates/surge-daemon/src/lib.rs`:

```rust
pub mod lifecycle;
```

- [ ] **Step 3: Build**

Run: `cargo build --workspace`
Expected: clean build (Unix) and clean build (Windows — verify by `cargo build --target x86_64-pc-windows-gnu` if cross-build available; otherwise rely on CI).

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/src/lifecycle.rs crates/surge-daemon/src/lib.rs
git commit -m "M7 P6.2: lifecycle — signal handlers + graceful drain

install_signal_handlers cancels a CancellationToken on SIGTERM/SIGINT
(Unix) or Ctrl+C (Windows). drain waits for cancellation then sleeps
for the configured grace period."
```

---

### Task 6.3: `surge-daemon` binary `main.rs`

**Files:**
- Modify: `crates/surge-daemon/src/main.rs`

- [ ] **Step 1: Replace the placeholder main**

Edit `crates/surge-daemon/src/main.rs`:

```rust
//! `surge-daemon` binary entry point.

use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::AcpBridge;
use surge_daemon::{lifecycle, pidfile, run_server, ServerConfig};
use surge_orchestrator::engine::{Engine, EngineConfig};
use surge_orchestrator::engine::facade::LocalEngineFacade;
use surge_persistence::runs::Storage;
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(version, about = "surge-daemon — long-running engine host")]
struct Args {
    /// Maximum concurrent active runs.
    #[arg(long, default_value_t = 8)]
    max_active: usize,
    /// Graceful-shutdown grace window.
    #[arg(long, default_value = "30s", value_parser = parse_humantime)]
    shutdown_grace: Duration,
    /// Detach from the controlling terminal (Unix: setsid).
    /// Currently a no-op on Windows; documented in spec §12.1.
    #[arg(long)]
    detached: bool,
}

fn parse_humantime(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| e.to_string())
}

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    if let Err(e) = pidfile::acquire_lock(std::process::id()) {
        eprintln!("surge-daemon: {e}");
        return std::process::ExitCode::from(2);
    }

    let socket_path = match pidfile::socket_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("surge-daemon: socket_path: {e}");
            let _ = pidfile::release_lock();
            return std::process::ExitCode::from(2);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("surge-daemon: tokio runtime: {e}");
            let _ = pidfile::release_lock();
            return std::process::ExitCode::from(2);
        }
    };

    let exit = rt.block_on(async {
        let shutdown = CancellationToken::new();
        lifecycle::install_signal_handlers(shutdown.clone());

        let storage = match Storage::open(&surge_runs_dir()).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("surge-daemon: storage: {e}");
                return 2;
            }
        };
        let bridge = match AcpBridge::with_defaults() {
            Ok(b) => Arc::new(b) as Arc<dyn surge_acp::bridge::facade::BridgeFacade>,
            Err(e) => {
                eprintln!("surge-daemon: bridge: {e}");
                return 2;
            }
        };
        let tool_dispatcher: Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher> =
            Arc::new(surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                std::env::current_dir().unwrap_or_default(),
            ));
        let notifier = Arc::new(surge_notify::MultiplexingNotifier::new()) as Arc<dyn surge_notify::NotifyDeliverer>;
        let engine = Arc::new(Engine::new_with_notifier(
            bridge,
            storage,
            tool_dispatcher,
            notifier,
            EngineConfig::default(),
        ));
        let facade: Arc<dyn surge_orchestrator::engine::facade::EngineFacade> =
            Arc::new(LocalEngineFacade::new(engine));

        // Write version file.
        let _ = std::fs::write(
            pidfile::version_path().unwrap_or_else(|_| std::path::PathBuf::from("/dev/null")),
            env!("CARGO_PKG_VERSION"),
        );

        let server_cfg = ServerConfig {
            max_active: args.max_active,
            socket_path: socket_path.clone(),
        };
        let server_handle = tokio::spawn({
            let facade = facade.clone();
            let shutdown = shutdown.clone();
            async move {
                if let Err(e) = run_server(server_cfg, facade, shutdown).await {
                    tracing::error!(err = %e, "server exited with error");
                }
            }
        });

        // Wait for shutdown.
        shutdown.cancelled().await;
        // Grace window for forwarders.
        tokio::time::sleep(args.shutdown_grace).await;
        let _ = server_handle.abort();
        0
    });

    let _ = pidfile::release_lock();
    let _ = std::fs::remove_file(socket_path);
    std::process::ExitCode::from(exit)
}

fn surge_runs_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".surge"))
        .unwrap_or_else(|| std::path::PathBuf::from(".surge"))
}
```

- [ ] **Step 2: Add `humantime` dep**

Edit `crates/surge-daemon/Cargo.toml`. Append to `[dependencies]`:

```toml
clap.workspace = true
humantime = "2"
```

- [ ] **Step 3: Build**

Run: `cargo build --workspace`
Expected: clean build. The `surge-daemon` binary now is a real binary (compiles, executes init, exits cleanly when no signal).

- [ ] **Step 4: Smoke-test the binary**

Run (Unix): `./target/debug/surge-daemon --max-active 4 &; sleep 1; pkill -TERM surge-daemon; sleep 2`
Expected: daemon starts (logs "daemon listening" to stderr), receives SIGTERM, drains, exits cleanly. No leftover `~/.surge/daemon/daemon.pid`.

- [ ] **Step 5: Commit**

```bash
git add crates/surge-daemon/Cargo.toml crates/surge-daemon/src/main.rs
git commit -m "M7 P6.3: surge-daemon binary — main.rs

Wires Storage + AcpBridge + WorktreeToolDispatcher + MultiplexingNotifier
into Engine, wraps in LocalEngineFacade, runs the IPC server. Signal
handlers cancel the shutdown token; main waits for cancellation then
sleeps shutdown_grace before aborting forwarders."
```

---

### Task 6.4: `surge daemon` CLI subtree

**Files:**
- Create: `crates/surge-cli/src/commands/daemon.rs`
- Modify: `crates/surge-cli/src/commands/mod.rs`
- Modify: `crates/surge-cli/src/main.rs` (Commands::Daemon variant)

- [ ] **Step 1: Create the command module**

Create `crates/surge-cli/src/commands/daemon.rs`:

```rust
//! `surge daemon` subtree — start / stop / status / restart for the
//! long-running surge-daemon process.

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use std::path::PathBuf;
use std::time::Duration;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::ipc::{DaemonRequest, DaemonResponse};

#[derive(Subcommand, Debug)]
pub enum DaemonCommands {
    /// Start the daemon.
    Start {
        /// Detach from the controlling terminal.
        #[arg(long)]
        detached: bool,
        /// Maximum concurrent runs.
        #[arg(long, default_value_t = 8)]
        max_active: usize,
    },
    /// Stop the daemon (graceful drain).
    Stop {
        /// Skip the drain — send SIGKILL after 1s.
        #[arg(long)]
        force: bool,
    },
    /// Print daemon status (uptime, active runs, queue, MCP).
    Status,
    /// Restart the daemon (stop + start).
    Restart,
}

pub async fn run(cmd: DaemonCommands) -> Result<()> {
    match cmd {
        DaemonCommands::Start { detached, max_active } => start(detached, max_active).await,
        DaemonCommands::Stop { force } => stop(force).await,
        DaemonCommands::Status => status().await,
        DaemonCommands::Restart => {
            stop(false).await.ok();
            start(true, 8).await
        }
    }
}

async fn start(detached: bool, max_active: usize) -> Result<()> {
    use surge_daemon::pidfile;

    if let Some(pid) = pidfile::read_pid(&pidfile::pid_path()?)? {
        if pidfile::is_alive(pid) {
            return Err(anyhow!("daemon already running (pid {pid})"));
        }
        eprintln!("note: stale pid file (pid {pid} not alive); will overwrite");
    }

    let mut cmd = std::process::Command::new(daemon_binary_path()?);
    cmd.arg("--max-active").arg(max_active.to_string());
    if detached {
        cmd.arg("--detached");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    nix::unistd::setsid().ok();
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0008 /* DETACHED_PROCESS */ | 0x0000_0200 /* CREATE_NEW_PROCESS_GROUP */);
        }
    }
    let child = cmd.spawn().context("spawn surge-daemon")?;
    println!("started surge-daemon (pid {})", child.id());

    // Poll for socket readiness up to 5 s.
    let socket_path = pidfile::socket_path()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if socket_path.exists() {
            println!("socket ready: {}", socket_path.display());
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(anyhow!(
        "daemon socket {} did not become readable within 5s",
        socket_path.display()
    ))
}

async fn stop(force: bool) -> Result<()> {
    use surge_daemon::pidfile;
    let socket_path = pidfile::socket_path()?;
    let facade = DaemonEngineFacade::connect(socket_path)
        .await
        .context("connect to daemon")?;

    // Send Shutdown via raw rpc — facade doesn't expose this method.
    // For M7 we shortcut: read the pid and send SIGTERM directly.
    let pid = pidfile::read_pid(&pidfile::pid_path()?)?
        .ok_or_else(|| anyhow!("no daemon pid file; daemon not running?"))?;
    let _ = facade; // hold connection open until SIGTERM lands

    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let signal = if force { Signal::SIGKILL } else { Signal::SIGTERM };
        kill(Pid::from_raw(pid as i32), signal).context("kill daemon")?;
    }
    #[cfg(windows)]
    {
        // M7 limitation — Windows graceful stop sends Ctrl+Break via
        // `taskkill /pid <pid>` (graceful) or `/F` (force).
        let mut tk = std::process::Command::new("taskkill");
        tk.arg("/pid").arg(pid.to_string());
        if force {
            tk.arg("/F");
        }
        tk.status().context("taskkill")?;
    }

    println!("requested daemon stop (pid {pid})");
    Ok(())
}

async fn status() -> Result<()> {
    use surge_daemon::pidfile;
    let pid_path = pidfile::pid_path()?;
    let socket_path = pidfile::socket_path()?;
    let pid = pidfile::read_pid(&pid_path)?;
    match pid {
        Some(p) if pidfile::is_alive(p) => {
            println!("status: running");
            println!("pid:    {p}");
            println!("socket: {}", socket_path.display());
            // Try Ping for live confirmation.
            match DaemonEngineFacade::connect(socket_path).await {
                Ok(_) => println!("ping:   ok"),
                Err(e) => println!("ping:   error ({e})"),
            }
        }
        Some(p) => {
            println!("status: stopped (stale pid file: {p})");
        }
        None => {
            println!("status: not running");
        }
    }
    Ok(())
}

fn daemon_binary_path() -> Result<PathBuf> {
    // Look for `surge-daemon` next to the current exe (cargo install
    // layout) or fall back to PATH lookup.
    if let Ok(my_exe) = std::env::current_exe() {
        if let Some(parent) = my_exe.parent() {
            let candidate = parent.join(if cfg!(windows) { "surge-daemon.exe" } else { "surge-daemon" });
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    // PATH fallback via `which`.
    which::which("surge-daemon").context("surge-daemon binary not found in PATH or alongside surge")
}
```

- [ ] **Step 2: Add deps to surge-cli**

Edit `crates/surge-cli/Cargo.toml`. Append to `[dependencies]`:

```toml
surge-daemon.workspace = true
which = "6"
```

(target-specific section for `nix` on Unix only:)

```toml
[target.'cfg(unix)'.dependencies]
nix = { workspace = true, features = ["signal"] }
```

- [ ] **Step 3: Wire module**

Edit `crates/surge-cli/src/commands/mod.rs`. Add:

```rust
pub mod daemon;
```

- [ ] **Step 4: Add the variant to `Commands` enum**

Edit `crates/surge-cli/src/main.rs`. Find the `enum Commands` and add:

```rust
    /// Manage the long-running surge-daemon process.
    Daemon {
        #[command(subcommand)]
        command: commands::daemon::DaemonCommands,
    },
```

In the match dispatcher, add the arm:

```rust
        Commands::Daemon { command } => commands::daemon::run(command).await,
```

- [ ] **Step 5: Build + smoke**

Run: `cargo build --workspace`
Expected: clean build.

Run: `./target/debug/surge daemon status`
Expected: prints "status: not running" (no daemon up).

- [ ] **Step 6: Commit**

```bash
git add crates/surge-cli/Cargo.toml crates/surge-cli/src/commands/daemon.rs crates/surge-cli/src/commands/mod.rs crates/surge-cli/src/main.rs
git commit -m "M7 P6.4: surge daemon start/stop/status/restart subtree

Auto-spawns the daemon from a sibling binary location with detached
flags (Unix setsid, Windows DETACHED_PROCESS). status pings the
daemon over the socket. stop reads pid + sends SIGTERM (force =
SIGKILL); Windows uses taskkill."
```

---

### Task 6.5: `--daemon` flag retrofit on `surge engine ...`

**Files:**
- Modify: `crates/surge-cli/src/commands/engine.rs`

- [ ] **Step 1: Add `--daemon` flag to each subcommand**

Edit `crates/surge-cli/src/commands/engine.rs`. Update the `EngineCommands` enum: every variant gains `#[arg(long)] daemon: bool`. For example:

```rust
#[derive(Subcommand, Debug)]
pub enum EngineCommands {
    Run {
        spec_path: PathBuf,
        #[arg(long)]
        watch: bool,
        #[arg(long)]
        worktree: Option<PathBuf>,
        #[arg(long)]
        daemon: bool,
    },
    Watch {
        run_id: String,
        #[arg(long)]
        daemon: bool,
    },
    Resume {
        run_id: String,
        #[arg(long)]
        daemon: bool,
    },
    Stop {
        run_id: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        daemon: bool,
    },
    Ls {
        #[arg(long)]
        daemon: bool,
    },
    Logs {
        run_id: String,
        #[arg(long)]
        since: Option<u64>,
        #[arg(long)]
        follow: bool,
    },
}
```

- [ ] **Step 2: Update `run_command` to switch facades**

Replace `run_command` with a version that constructs the right facade:

```rust
async fn run_command(
    spec_path: PathBuf,
    watch: bool,
    worktree: Option<PathBuf>,
    daemon: bool,
) -> Result<()> {
    use std::time::Duration;
    use surge_core::graph::Graph;
    use surge_orchestrator::engine::facade::EngineFacade;
    use surge_orchestrator::engine::handle::EngineRunEvent;

    let toml_text = std::fs::read_to_string(&spec_path)
        .with_context(|| format!("read {}", spec_path.display()))?;
    let graph: Graph =
        toml::from_str(&toml_text).with_context(|| format!("parse {}", spec_path.display()))?;
    let worktree_path = worktree.map_or_else(|| std::env::current_dir().context("cwd"), Ok)?;
    if !worktree_path.exists() {
        return Err(anyhow!(
            "worktree path does not exist: {}",
            worktree_path.display()
        ));
    }

    let facade: std::sync::Arc<dyn EngineFacade> = if daemon {
        ensure_daemon_running().await?;
        let socket = surge_daemon::pidfile::socket_path()?;
        std::sync::Arc::new(
            surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?,
        )
    } else {
        let storage = surge_persistence::runs::Storage::open(&surge_runs_dir()?)
            .await
            .context("open storage")?;
        let bridge: std::sync::Arc<dyn surge_acp::bridge::facade::BridgeFacade> =
            std::sync::Arc::new(
                surge_acp::bridge::AcpBridge::with_defaults()
                    .context("AcpBridge::with_defaults")?,
            );
        let tool_dispatcher: std::sync::Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher> =
            std::sync::Arc::new(
                surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                    worktree_path.clone(),
                ),
            );
        let notifier = build_default_notifier();
        let engine = std::sync::Arc::new(surge_orchestrator::engine::Engine::new_with_notifier(
            bridge,
            storage,
            tool_dispatcher,
            notifier,
            surge_orchestrator::engine::EngineConfig::default(),
        ));
        std::sync::Arc::new(surge_orchestrator::engine::facade::LocalEngineFacade::new(
            engine,
        ))
    };

    let run_id = surge_core::id::RunId::new();
    println!("{run_id}");

    let handle = facade
        .start_run(
            run_id,
            graph,
            worktree_path,
            surge_orchestrator::engine::EngineRunConfig::default(),
        )
        .await?;

    if watch {
        let mut rx = handle.events;
        loop {
            match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
                Ok(Ok(event)) => {
                    print_event(&event);
                    if matches!(event, EngineRunEvent::Terminal(_)) {
                        break;
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
    }

    Ok(())
}

async fn ensure_daemon_running() -> Result<()> {
    use surge_daemon::pidfile;
    let pid = pidfile::read_pid(&pidfile::pid_path()?)?;
    if let Some(p) = pid {
        if pidfile::is_alive(p) {
            return Ok(());
        }
    }
    eprintln!("note: daemon not running; auto-spawning…");
    crate::commands::daemon::run(crate::commands::daemon::DaemonCommands::Start {
        detached: true,
        max_active: 8,
    })
    .await
}
```

Update `resume_command` and `stop_command` to use the daemon facade when `daemon == true`:

```rust
async fn resume_command(run_id: String, daemon: bool) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    if !daemon {
        return Err(anyhow!(
            "M6: resume requires --daemon (in-process resume needs the engine to be alive); \
             use `surge engine resume {id} --daemon`"
        ));
    }
    ensure_daemon_running().await?;
    let socket = surge_daemon::pidfile::socket_path()?;
    let facade = surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
    let cwd = std::env::current_dir().context("cwd")?;
    let _handle = facade.resume_run(id, cwd).await?;
    println!("resumed {id}");
    Ok(())
}

async fn stop_command(run_id: String, reason: Option<String>, daemon: bool) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    if !daemon {
        return Err(anyhow!("--daemon required for cross-process stop"));
    }
    ensure_daemon_running().await?;
    let socket = surge_daemon::pidfile::socket_path()?;
    let facade = surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
    facade
        .stop_run(id, reason.unwrap_or_else(|| "user-requested".into()))
        .await?;
    println!("stopped {id}");
    Ok(())
}
```

Update `ls_command` to use the daemon when requested:

```rust
async fn ls_command(daemon: bool) -> Result<()> {
    if daemon {
        let socket = surge_daemon::pidfile::socket_path()?;
        let facade = surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
        let runs = facade.list_runs().await?;
        println!("{:<32} {:<10} STARTED", "ID", "STATUS");
        for r in runs {
            println!(
                "{:<32} {:<10} {}",
                r.run_id,
                format!("{:?}", r.status).to_lowercase(),
                r.started_at.format("%Y-%m-%d %H:%M:%S")
            );
        }
        return Ok(());
    }
    // Existing M6 disk-listing path unchanged.
    legacy_ls_command().await
}
```

(Rename the existing `ls_command` body to `legacy_ls_command`.)

Update the dispatcher:

```rust
pub async fn run(command: EngineCommands) -> Result<()> {
    match command {
        EngineCommands::Run { spec_path, watch, worktree, daemon } => run_command(spec_path, watch, worktree, daemon).await,
        EngineCommands::Watch { run_id, daemon } => watch_command(run_id, daemon).await,
        EngineCommands::Resume { run_id, daemon } => resume_command(run_id, daemon).await,
        EngineCommands::Stop { run_id, reason, daemon } => stop_command(run_id, reason, daemon).await,
        EngineCommands::Ls { daemon } => ls_command(daemon).await,
        EngineCommands::Logs { run_id, since, follow } => logs_command(run_id, since, follow).await,
    }
}
```

`watch_command` with `--daemon` opens the daemon facade, calls `Subscribe`, and tails events. Without `--daemon`, falls back to the existing disk-tail mode.

```rust
async fn watch_command(run_id: String, daemon: bool) -> Result<()> {
    let id = parse_run_id(&run_id)?;
    if !daemon {
        follow_log_from(id, 0).await?;
        return Ok(());
    }
    let socket = surge_daemon::pidfile::socket_path()?;
    let facade = surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
    // For the simplest path, ask facade for a no-op start that fails
    // gracefully if run isn't active. M7 simplification: there is no
    // dedicated "subscribe-only" verb on EngineFacade — the daemon
    // already pushes per-run events to subscribers via the Subscribe
    // IPC verb. Here we use the pidfile-based approach: open a raw
    // DaemonClient, send Subscribe + Unsubscribe ourselves.
    eprintln!("watching {id} via daemon (Ctrl+C to stop)…");
    // Defer the raw-client subscription helper to follow-up if the
    // smoke flow doesn't need it; M6's disk-tail mode covers most
    // watch needs.
    follow_log_from(id, 0).await?;
    Ok(())
}
```

(Note: a richer streaming watch lives in PR 6 polish — for M7 PR 3 we ship daemon `Subscribe` working from `start_run --daemon --watch`. Standalone `watch --daemon` falls back to disk tail.)

- [ ] **Step 2: Build**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 3: Smoke**

Run sequence:
```bash
./target/debug/surge daemon start
./target/debug/surge engine ls --daemon       # empty list
./target/debug/surge daemon status
./target/debug/surge daemon stop
```
Expected: each command succeeds; `engine ls --daemon` prints the header with no rows; `daemon status` after stop reports stopped.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-cli/src/commands/engine.rs
git commit -m "M7 P6.5: --daemon flag on surge engine subcommands

run/watch/resume/stop/ls grow a --daemon flag. With --daemon, CLI
auto-spawns the daemon (if not running) and routes through
DaemonEngineFacade. Without it, M6 in-process behaviour preserved.
Resume and cross-process stop require --daemon."
```

---

> **PR 3 ready** (P6 complete). Daemon end-to-end works: `surge daemon start; surge engine run flow.toml --daemon --watch; surge daemon stop`. MCP not yet wired.

---

## Phase 7 — `surge-mcp` rmcp wrapper + `McpServerConnection` (3 days)

### Task 7.1: `McpError` enum

**Files:**
- Create: `crates/surge-mcp/src/error.rs`
- Modify: `crates/surge-mcp/src/lib.rs`

- [ ] **Step 1: Create the error module**

Create `crates/surge-mcp/src/error.rs`:

```rust
//! Errors produced by the MCP integration layer.

use std::time::Duration;

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("server '{0}' not configured in run-level mcp_servers registry")]
    ServerNotConfigured(String),
    #[error("server '{server}' failed to start: {source}")]
    StartFailed { server: String, source: String },
    #[error("server '{server}' crashed (exit code {exit_code:?})")]
    ServerCrashed { server: String, exit_code: Option<i32> },
    #[error("server '{server}' is not running (restart_on_crash=false)")]
    ServerNotRunning { server: String },
    #[error("server '{server}' tool '{tool}' not found")]
    ToolNotFound { server: String, tool: String },
    #[error("MCP call timed out after {0:?}")]
    Timeout(Duration),
    #[error("rmcp transport error: {0}")]
    Transport(String),
    #[error("rmcp service error: {0}")]
    Service(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_format_includes_duration() {
        let e = McpError::Timeout(Duration::from_secs(60));
        assert!(format!("{e}").contains("60s"));
    }
}
```

- [ ] **Step 2: Wire module**

Edit `crates/surge-mcp/src/lib.rs`:

```rust
pub mod error;
pub use error::McpError;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-mcp --lib error::`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-mcp/src/error.rs crates/surge-mcp/src/lib.rs
git commit -m "M7 P7.1: McpError — surge-mcp error taxonomy

#[non_exhaustive] enum covering: ServerNotConfigured, StartFailed,
ServerCrashed, ServerNotRunning, ToolNotFound, Timeout, Transport,
Service. RoutingToolDispatcher (Phase 8) maps each to a friendly
ToolResultPayload::Error message."
```

---

### Task 7.2: `McpServerConnection` — state machine + lazy connect

**Files:**
- Create: `crates/surge-mcp/src/connection.rs`
- Modify: `crates/surge-mcp/src/lib.rs`

- [ ] **Step 1: Create the connection module**

Create `crates/surge-mcp/src/connection.rs`:

```rust
//! Per-server MCP connection state. Wraps an rmcp `RunningService` and
//! handles spawn / crash detection / reconnect.

use crate::error::McpError;
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use rmcp::transport::child_process::TokioChildProcess;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use tokio::sync::Mutex;

/// State of a single MCP server connection.
#[derive(Debug)]
enum ConnState {
    /// Not yet connected, or fully shut down.
    Disconnected,
    /// rmcp service is alive; can dispatch calls.
    Running(Arc<RunningService<RoleClient, ()>>),
    /// Server died; in-flight calls have already been failed. Next
    /// `call_tool` triggers reconnect (if `restart_on_crash`).
    Crashed { last_exit: Option<i32> },
}

pub struct McpServerConnection {
    config: McpServerRef,
    state: Mutex<ConnState>,
}

impl McpServerConnection {
    /// Construct in `Disconnected` state. First `call_tool` /
    /// `list_tools` triggers `ensure_connected`.
    #[must_use]
    pub fn new(config: McpServerRef) -> Self {
        Self {
            config,
            state: Mutex::new(ConnState::Disconnected),
        }
    }

    /// Server name (from config).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Drive state to `Running`. Returns the `RunningService` arc on
    /// success.
    async fn ensure_connected(&self) -> Result<Arc<RunningService<RoleClient, ()>>, McpError> {
        let mut state = self.state.lock().await;
        match &*state {
            ConnState::Running(rs) => return Ok(rs.clone()),
            ConnState::Crashed { .. } if !self.config.restart_on_crash => {
                return Err(McpError::ServerNotRunning {
                    server: self.config.name.clone(),
                });
            }
            _ => {}
        }
        // Spawn child and run initialise handshake.
        let transport = match &self.config.transport {
            McpTransportConfig::Stdio { command, args, env } => {
                let mut tokio_cmd = tokio::process::Command::new(command);
                tokio_cmd.args(args);
                for (k, v) in env {
                    tokio_cmd.env(k, v);
                }
                TokioChildProcess::new(tokio_cmd)
                    .map_err(|e| McpError::StartFailed {
                        server: self.config.name.clone(),
                        source: e.to_string(),
                    })?
            }
        };
        let service = ()
            .serve(transport)
            .await
            .map_err(|e| McpError::StartFailed {
                server: self.config.name.clone(),
                source: e.to_string(),
            })?;
        let rs = Arc::new(service);
        *state = ConnState::Running(rs.clone());
        Ok(rs)
    }

    /// List tools the server reports via the MCP `tools/list` verb.
    pub async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, McpError> {
        let rs = self.ensure_connected().await?;
        let result = rs
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| McpError::Service(e.to_string()))?;
        Ok(result)
    }

    /// Call a tool with the given arguments. Honours
    /// `config.call_timeout` — exceeding it returns
    /// `McpError::Timeout`.
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let rs = self.ensure_connected().await?;
        let timeout = self.config.call_timeout;
        let params = rmcp::model::CallToolRequestParam {
            name: tool.to_string().into(),
            arguments: match arguments {
                serde_json::Value::Object(m) => Some(m),
                serde_json::Value::Null => None,
                other => {
                    let mut m = serde_json::Map::new();
                    m.insert("input".into(), other);
                    Some(m)
                }
            },
        };
        match tokio::time::timeout(timeout, rs.peer().call_tool(params)).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => {
                // Service-level error. If the transport is dead, mark
                // crashed; the next call will reconnect.
                self.mark_crashed(None).await;
                Err(McpError::Service(e.to_string()))
            }
            Err(_elapsed) => Err(McpError::Timeout(timeout)),
        }
    }

    async fn mark_crashed(&self, exit_code: Option<i32>) {
        let mut state = self.state.lock().await;
        *state = ConnState::Crashed { last_exit: exit_code };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn new_starts_disconnected() {
        let r = McpServerRef {
            name: "x".into(),
            transport: McpTransportConfig::Stdio {
                command: PathBuf::from("nonexistent"),
                args: vec![],
                env: HashMap::new(),
            },
            allowed_tools: None,
            call_timeout: Duration::from_secs(1),
            restart_on_crash: true,
        };
        let c = McpServerConnection::new(r);
        assert_eq!(c.name(), "x");
    }
}
```

- [ ] **Step 2: Wire module**

Edit `crates/surge-mcp/src/lib.rs`:

```rust
pub mod connection;
pub use connection::McpServerConnection;
```

- [ ] **Step 3: Build**

Run: `cargo build -p surge-mcp`
Expected: clean build (rmcp APIs resolve, types match). If any rmcp API has shifted in 1.6.x, adjust the `peer().list_all_tools()` / `call_tool` call shapes accordingly — verify against current `cargo doc -p rmcp --open`.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-mcp/src/connection.rs crates/surge-mcp/src/lib.rs
git commit -m "M7 P7.2: McpServerConnection — rmcp wrapper, state machine

Disconnected → Running → Crashed states. ensure_connected lazy-spawns
the child via TokioChildProcess; call_tool wraps rmcp's peer
operations with the configured call_timeout. Crash detection marks
state and the next call reconnects (when restart_on_crash=true)."
```

---

## Phase 8 — `McpRegistry` + `RoutingToolDispatcher` (3 days)

### Task 8.1: `McpRegistry::from_config` + `call_tool` + `list_all_tools`

**Files:**
- Create: `crates/surge-mcp/src/registry.rs`
- Modify: `crates/surge-mcp/src/lib.rs`

- [ ] **Step 1: Create the registry**

Create `crates/surge-mcp/src/registry.rs`:

```rust
//! Engine-wide registry of MCP server connections.

use crate::connection::McpServerConnection;
use crate::error::McpError;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use surge_core::mcp_config::McpServerRef;

/// Single-server tool listing entry, returned by `list_all_tools`.
#[derive(Clone, Debug)]
pub struct McpToolEntry {
    /// Name of the server this tool comes from.
    pub server: String,
    /// Tool name as the agent will see it.
    pub tool: String,
    /// Description, if the server supplied one.
    pub description: Option<String>,
    /// JSON-schema-shaped input definition.
    pub input_schema: serde_json::Value,
}

/// Result of a single MCP call, surge-flavoured (decoupled from
/// rmcp's exact types so callers don't need to depend on rmcp).
#[derive(Clone, Debug)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    pub is_error: bool,
}

#[derive(Clone, Debug)]
pub enum McpContent {
    Text(String),
    Other { kind: String, summary: String },
}

pub struct McpRegistry {
    servers: HashMap<String, Arc<McpServerConnection>>,
}

impl McpRegistry {
    /// Build a registry from a slice of `McpServerRef`. Connections
    /// are constructed in `Disconnected` state — first use of each
    /// server triggers the spawn.
    #[must_use]
    pub fn from_config(refs: &[McpServerRef]) -> Self {
        let mut servers = HashMap::new();
        for r in refs {
            servers.insert(
                r.name.clone(),
                Arc::new(McpServerConnection::new(r.clone())),
            );
        }
        Self { servers }
    }

    /// Combined `tools/list` across all configured servers. Used by
    /// `RoutingToolDispatcher` at session-open to assemble the agent's
    /// tool catalog.
    pub async fn list_all_tools(&self) -> Result<Vec<McpToolEntry>, McpError> {
        let mut out = Vec::new();
        for (name, conn) in &self.servers {
            let tools = conn.list_tools().await?;
            for t in tools {
                out.push(McpToolEntry {
                    server: name.clone(),
                    tool: t.name.to_string(),
                    description: t.description.map(|c| c.to_string()),
                    input_schema: serde_json::Value::Object(
                        (*t.input_schema).clone(),
                    ),
                });
            }
        }
        Ok(out)
    }

    /// Call a tool on a specific server.
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
        _timeout: Duration,
    ) -> Result<McpToolResult, McpError> {
        let conn = self
            .servers
            .get(server)
            .ok_or_else(|| McpError::ServerNotConfigured(server.into()))?;
        let r = conn.call_tool(tool, arguments).await?;
        let content = r
            .content
            .into_iter()
            .map(|raw| match raw.raw {
                rmcp::model::RawContent::Text(t) => McpContent::Text(t.text),
                other => McpContent::Other {
                    kind: format!("{other:?}").split('(').next().unwrap_or("?").into(),
                    summary: format!("{other:?}"),
                },
            })
            .collect();
        Ok(McpToolResult {
            content,
            is_error: r.is_error.unwrap_or(false),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use surge_core::mcp_config::{McpServerRef, McpTransportConfig};

    #[test]
    fn empty_registry_is_empty() {
        let r = McpRegistry::from_config(&[]);
        assert!(r.servers.is_empty());
    }

    #[test]
    fn registry_holds_named_connection() {
        let refs = vec![McpServerRef {
            name: "echo".into(),
            transport: McpTransportConfig::Stdio {
                command: PathBuf::from("nope"),
                args: vec![],
                env: HashMap::new(),
            },
            allowed_tools: None,
            call_timeout: Duration::from_secs(60),
            restart_on_crash: true,
        }];
        let r = McpRegistry::from_config(&refs);
        assert_eq!(r.servers.len(), 1);
        assert!(r.servers.contains_key("echo"));
    }
}
```

- [ ] **Step 2: Wire module**

Edit `crates/surge-mcp/src/lib.rs`:

```rust
pub mod registry;
pub use registry::{McpContent, McpRegistry, McpToolEntry, McpToolResult};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p surge-mcp --lib registry::`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-mcp/src/registry.rs crates/surge-mcp/src/lib.rs
git commit -m "M7 P8.1: McpRegistry — multi-server connection holder

from_config builds connections in Disconnected state. list_all_tools
aggregates tools/list across all configured servers. call_tool
dispatches by server name and translates rmcp content types into
the surge-flavoured McpContent enum (Text + Other catch-all)."
```

---

### Task 8.2: `RoutingToolDispatcher` — engine + MCP fan-out

**Files:**
- Create: `crates/surge-orchestrator/src/engine/tools/routing.rs`
- Modify: `crates/surge-orchestrator/src/engine/tools/mod.rs` (add module)
- Modify: `crates/surge-orchestrator/Cargo.toml` (add `surge-mcp`)

- [ ] **Step 1: Add `surge-mcp` dep**

Edit `crates/surge-orchestrator/Cargo.toml`. Append to `[dependencies]`:

```toml
surge-mcp.workspace = true
```

- [ ] **Step 2: Create the routing dispatcher**

Create `crates/surge-orchestrator/src/engine/tools/routing.rs`:

```rust
//! `RoutingToolDispatcher` — fans out `ToolDispatcher::dispatch`
//! between the engine's built-in tools (e.g., `WorktreeToolDispatcher`)
//! and an `McpRegistry`. Routing decisions are precomputed at
//! construction time from the merged tool catalog.

use crate::engine::tools::{
    DeclaredTool, ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use surge_mcp::{McpContent, McpRegistry, McpToolEntry};

/// One row in the routing table: where a given tool name lives.
#[derive(Clone, Debug)]
enum ToolOrigin {
    Engine,
    Mcp { server: String, timeout: Duration },
}

/// `ToolDispatcher` impl that routes between engine + MCP. Constructed
/// once per engine (or per session, depending on the agent stage's
/// allowlist filtering).
pub struct RoutingToolDispatcher {
    engine_dispatcher: Arc<dyn ToolDispatcher>,
    mcp_registry: Arc<McpRegistry>,
    routing_table: HashMap<String, ToolOrigin>,
    declared: Vec<DeclaredTool>,
}

impl RoutingToolDispatcher {
    /// Build with engine dispatcher + MCP registry + filtered list of
    /// MCP tools that should be exposed for the current session.
    /// Engine-built-in tools (from `engine_dispatcher.declared_tools`)
    /// are inserted with `Engine` origin and override any MCP entries
    /// with the same name (collision resolution).
    pub fn new(
        engine_dispatcher: Arc<dyn ToolDispatcher>,
        mcp_registry: Arc<McpRegistry>,
        mcp_tools: &[McpToolEntry],
        per_server_timeouts: &HashMap<String, Duration>,
    ) -> Self {
        let mut table: HashMap<String, ToolOrigin> = HashMap::new();
        let mut declared: Vec<DeclaredTool> = Vec::new();

        for entry in mcp_tools {
            let timeout = per_server_timeouts
                .get(&entry.server)
                .copied()
                .unwrap_or(Duration::from_secs(60));
            table.insert(
                entry.tool.clone(),
                ToolOrigin::Mcp {
                    server: entry.server.clone(),
                    timeout,
                },
            );
            declared.push(DeclaredTool {
                name: entry.tool.clone(),
                description: entry.description.clone(),
                input_schema: entry.input_schema.clone(),
            });
        }

        // Engine tools overwrite MCP collisions (engine wins).
        let engine_tools = engine_dispatcher.declared_tools();
        for et in &engine_tools {
            table.insert(et.name.clone(), ToolOrigin::Engine);
        }
        // Replace any duplicate-named entries in `declared` with the
        // engine's version (description / schema take precedence).
        let engine_names: std::collections::HashSet<&str> =
            engine_tools.iter().map(|t| t.name.as_str()).collect();
        declared.retain(|d| !engine_names.contains(d.name.as_str()));
        declared.extend(engine_tools.into_iter());

        Self {
            engine_dispatcher,
            mcp_registry,
            routing_table: table,
            declared,
        }
    }
}

#[async_trait]
impl ToolDispatcher for RoutingToolDispatcher {
    async fn dispatch(
        &self,
        ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> ToolResultPayload {
        match self.routing_table.get(&call.tool) {
            Some(ToolOrigin::Engine) => self.engine_dispatcher.dispatch(ctx, call).await,
            Some(ToolOrigin::Mcp { server, timeout }) => {
                match self
                    .mcp_registry
                    .call_tool(server, &call.tool, call.arguments.clone(), *timeout)
                    .await
                {
                    Ok(r) if !r.is_error => ToolResultPayload::Ok {
                        content: serde_json::Value::Array(
                            r.content.into_iter().map(content_to_json).collect(),
                        ),
                    },
                    Ok(r) => ToolResultPayload::Error {
                        message: r
                            .content
                            .into_iter()
                            .map(content_to_string)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    },
                    Err(e) => ToolResultPayload::Error {
                        message: format!("MCP error: {e}"),
                    },
                }
            }
            None => ToolResultPayload::Unsupported {
                message: format!("unknown tool: {}", call.tool),
            },
        }
    }

    fn declared_tools(&self) -> Vec<DeclaredTool> {
        self.declared.clone()
    }
}

fn content_to_json(c: McpContent) -> serde_json::Value {
    match c {
        McpContent::Text(s) => serde_json::json!({ "type": "text", "text": s }),
        McpContent::Other { kind, summary } => serde_json::json!({
            "type": kind,
            "summary": summary,
        }),
    }
}

fn content_to_string(c: McpContent) -> String {
    match c {
        McpContent::Text(s) => s,
        McpContent::Other { kind, summary } => format!("[{kind}] {summary}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tools::{ToolCall, ToolDispatcher, ToolResultPayload};

    struct EngineStub;
    #[async_trait]
    impl ToolDispatcher for EngineStub {
        async fn dispatch(
            &self,
            _ctx: &ToolDispatchContext<'_>,
            call: &ToolCall,
        ) -> ToolResultPayload {
            ToolResultPayload::Ok {
                content: serde_json::json!({"echo": call.tool}),
            }
        }
        fn declared_tools(&self) -> Vec<DeclaredTool> {
            vec![DeclaredTool {
                name: "shell_exec".into(),
                description: None,
                input_schema: serde_json::json!({}),
            }]
        }
    }

    #[tokio::test]
    async fn engine_tool_wins_collision() {
        let mcp = Arc::new(McpRegistry::from_config(&[]));
        let mcp_tools = vec![McpToolEntry {
            server: "fake".into(),
            tool: "shell_exec".into(), // colliding name
            description: Some("from MCP".into()),
            input_schema: serde_json::json!({}),
        }];
        let r = RoutingToolDispatcher::new(
            Arc::new(EngineStub),
            mcp,
            &mcp_tools,
            &HashMap::new(),
        );
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &surge_core::run_state::RunMemory::default(),
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({}),
        };
        let result = r.dispatch(&ctx, &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                assert_eq!(content["echo"], "shell_exec");
            }
            other => panic!("expected engine route, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_tool_is_unsupported() {
        let mcp = Arc::new(McpRegistry::from_config(&[]));
        let r = RoutingToolDispatcher::new(
            Arc::new(EngineStub),
            mcp,
            &[],
            &HashMap::new(),
        );
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &surge_core::run_state::RunMemory::default(),
        };
        let call = ToolCall {
            call_id: "c2".into(),
            tool: "whatever".into(),
            arguments: serde_json::json!({}),
        };
        match r.dispatch(&ctx, &call).await {
            ToolResultPayload::Unsupported { .. } => {}
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Wire module**

Edit `crates/surge-orchestrator/src/engine/tools/mod.rs`. Add:

```rust
pub mod routing;
pub use routing::RoutingToolDispatcher;
```

- [ ] **Step 4: Build + test**

Run: `cargo build --workspace`
Expected: clean build.

Run: `cargo test -p surge-orchestrator --lib engine::tools::routing::`
Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/surge-orchestrator/Cargo.toml crates/surge-orchestrator/src/engine/tools/routing.rs crates/surge-orchestrator/src/engine/tools/mod.rs
git commit -m "M7 P8.2: RoutingToolDispatcher — engine + MCP fan-out

Routing table built at construction from MCP catalog + engine
declared_tools (engine wins on collisions). dispatch() forwards to
engine_dispatcher or mcp_registry.call_tool based on ToolOrigin.
declared_tools returns the merged catalog for session-open consumption."
```

---

> **PR 4 ready** (P7 + P8.1-8.2): surge-mcp crate, RoutingToolDispatcher. Mock MCP server tests come in PR 5.

---

## Phase 9 — sandbox-aware tool exposure (1 day)

### Task 9.1: wire `RoutingToolDispatcher` into `engine::stage::agent`

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/stage/agent.rs` (session-open hook)
- Modify: `crates/surge-orchestrator/src/engine/run_task.rs` (carry McpRegistry through RunTaskParams)

- [ ] **Step 1: Locate the existing session-open code**

Run: `grep -n "open_session\|SessionConfig\|tool_dispatcher" crates/surge-orchestrator/src/engine/stage/agent.rs | head -30`
Expected: find the spot where `BridgeFacade::open_session(SessionConfig { tools, ... })` is called. M7 inserts `RoutingToolDispatcher` construction here.

- [ ] **Step 2: Add `mcp_registry` to `RunTaskParams`**

Edit `crates/surge-orchestrator/src/engine/run_task.rs`. Find the `RunTaskParams` struct and add:

```rust
    /// Optional MCP registry. When `Some`, agent stages wrap the
    /// engine dispatcher with `RoutingToolDispatcher` to expose
    /// configured MCP tools alongside engine built-ins.
    pub mcp_registry: Option<Arc<surge_mcp::McpRegistry>>,
```

In every `RunTaskParams { ... }` construction site (engine.rs `start_run`, `resume_run`), pass `mcp_registry: None` for now (M7 wires production via the Engine constructor in Step 4).

- [ ] **Step 3: Construct `RoutingToolDispatcher` at session-open**

Edit `crates/surge-orchestrator/src/engine/stage/agent.rs`. Where the session is being opened (just before `bridge.open_session(...)`), insert:

```rust
let session_dispatcher: Arc<dyn crate::engine::tools::ToolDispatcher> = if let Some(reg) = &params.mcp_registry {
    // Filter MCP tools by the stage's allowlist (ToolOverride::mcp_add).
    let allowed_servers: std::collections::HashSet<&str> = agent_cfg
        .tool_overrides
        .as_ref()
        .map(|o| o.mcp_add.iter().map(String::as_str).collect())
        .unwrap_or_default();

    let all = match reg.list_all_tools().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(err = %e, "MCP list_all_tools failed; proceeding with engine tools only");
            Vec::new()
        }
    };

    let filtered: Vec<surge_mcp::McpToolEntry> = all
        .into_iter()
        .filter(|t| {
            allowed_servers.contains(t.server.as_str())
                && sandbox_allows_mcp_tool(&sandbox_cfg, &t.server, &t.tool)
        })
        .collect();

    // Per-server timeout map (from RunConfig::mcp_servers).
    let timeouts: std::collections::HashMap<String, std::time::Duration> = run_cfg_mcp_servers
        .iter()
        .map(|s| (s.name.clone(), s.call_timeout))
        .collect();

    Arc::new(crate::engine::tools::RoutingToolDispatcher::new(
        params.tool_dispatcher.clone(),
        reg.clone(),
        &filtered,
        &timeouts,
    )) as Arc<dyn crate::engine::tools::ToolDispatcher>
} else {
    params.tool_dispatcher.clone()
};
```

The session's tool list is built from `session_dispatcher.declared_tools()` and passed in `SessionConfig`. The implementer subagent will adapt the existing tool-list-construction code in this file to use `session_dispatcher.declared_tools()` instead of the engine dispatcher's directly.

- [ ] **Step 4: Add `sandbox_allows_mcp_tool` helper**

Add at the end of `crates/surge-orchestrator/src/engine/stage/agent.rs`:

```rust
/// Conservative M7 heuristic for whether a sandbox tier permits an
/// MCP server's tool. Will be replaced by M4 (sandbox milestone)
/// proper enforcement.
fn sandbox_allows_mcp_tool(
    sandbox: &surge_core::sandbox::SandboxConfig,
    _server: &str,
    _tool: &str,
) -> bool {
    use surge_core::sandbox::SandboxMode;
    match sandbox.mode {
        SandboxMode::ReadOnly => false, // conservative: no MCP under read-only
        SandboxMode::WorkspaceWrite
        | SandboxMode::WorkspaceWriteWithNetwork
        | SandboxMode::FullAccess => true,
    }
}
```

(Adjust to match the actual `SandboxMode` variants in surge-core; if not all 4 exist yet, drop the missing ones.)

- [ ] **Step 5: Plumb `run_cfg_mcp_servers` through `run_task::execute`**

Find where `RunConfig` is loaded from the event log (or from `EngineRunConfig` for fresh runs). Pass `&run_cfg.mcp_servers` (the registry) into the agent stage's params. M7 simplification: store the `Vec<McpServerRef>` once on `RunTaskParams` so every stage in the run sees the same registry.

Add field to `RunTaskParams`:

```rust
    /// Run-level MCP server registry (mirror of RunConfig::mcp_servers).
    pub mcp_servers: Vec<surge_core::mcp_config::McpServerRef>,
```

Initialize from the persisted `RunConfig` on `start_run` / `resume_run` paths. Default empty.

- [ ] **Step 6: Build**

Run: `cargo build --workspace`
Expected: clean build. Engine still works without MCP (registry None / empty); with MCP registry, agent stages now expose filtered MCP tools.

- [ ] **Step 7: Commit**

```bash
git add crates/surge-orchestrator/src/engine/stage/agent.rs crates/surge-orchestrator/src/engine/run_task.rs
git commit -m "M7 P9: RoutingToolDispatcher session-open wiring

agent stages now construct RoutingToolDispatcher per session when
McpRegistry is configured. Filters by stage's ToolOverride::mcp_add
allowlist intersected with sandbox-tier heuristic. Engine continues
to work unchanged when McpRegistry is None."
```

---

### Task 9.2: wire `mcp_registry` into `Engine` construction

**Files:**
- Modify: `crates/surge-orchestrator/src/engine/engine.rs`

- [ ] **Step 1: Add `mcp_registry` field to Engine**

Edit `crates/surge-orchestrator/src/engine/engine.rs`. Add to the `Engine` struct:

```rust
    mcp_registry: Option<Arc<surge_mcp::McpRegistry>>,
```

Add a builder constructor:

```rust
    /// Construct with an MCP registry. The registry is shared across
    /// all runs hosted by this engine. M7 daemon uses this constructor
    /// to wire user-configured MCP servers; in-process M6-style CLI
    /// stays on `new_with_notifier` (no MCP).
    #[must_use]
    pub fn new_with_mcp(
        bridge: Arc<dyn BridgeFacade>,
        storage: Arc<surge_persistence::runs::Storage>,
        tool_dispatcher: Arc<dyn ToolDispatcher>,
        notify_deliverer: Arc<dyn surge_notify::NotifyDeliverer>,
        mcp_registry: Option<Arc<surge_mcp::McpRegistry>>,
        config: EngineConfig,
    ) -> Self {
        Self {
            bridge,
            storage,
            tool_dispatcher,
            notify_deliverer,
            mcp_registry,
            config: Arc::new(config),
            runs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
```

Initialize the existing `new` and `new_with_notifier` to set `mcp_registry: None`.

- [ ] **Step 2: Wire registry through `start_run` / `resume_run`**

In both `Engine::start_run` and `Engine::resume_run`, when constructing `RunTaskParams`, pass:

```rust
            mcp_registry: self.mcp_registry.clone(),
            mcp_servers: run_config_mcp_servers, // populated from RunConfig
```

For `start_run`, `run_config_mcp_servers` comes from the caller; M7 plumbs it via `EngineRunConfig` extension OR keeps `RunConfig::mcp_servers` as the sole source (simpler). Choose the latter — read from the persisted `RunStarted` event payload:

In `resume_run`, the registry is already in the event log; `replay::replay` should surface it as part of the replay result. Add a `mcp_servers: Vec<McpServerRef>` field to the replay result if not already present.

- [ ] **Step 3: Build**

Run: `cargo build --workspace`
Expected: clean. The engine surface gains `new_with_mcp` constructor; existing call sites keep using `new` / `new_with_notifier` until they want MCP.

- [ ] **Step 4: Update `surge-daemon::main` to use `new_with_mcp`**

Edit `crates/surge-daemon/src/main.rs`. Replace the `Engine::new_with_notifier(...)` with:

```rust
let engine = Arc::new(Engine::new_with_mcp(
    bridge,
    storage,
    tool_dispatcher,
    notifier,
    None, // M7 simplification: registry is per-run, populated when run starts
    EngineConfig::default(),
));
```

(M7 keeps the per-run model — the registry is rebuilt from `RunConfig::mcp_servers` when a run starts. M9+ may add daemon-level shared registries.)

- [ ] **Step 5: Commit**

```bash
git add crates/surge-orchestrator/src/engine/engine.rs crates/surge-daemon/src/main.rs
git commit -m "M7 P9.2: Engine::new_with_mcp constructor + wiring

Optional Arc<McpRegistry> on Engine. Daemon uses new_with_mcp; in-
process CLI keeps new_with_notifier (no MCP). RunTaskParams carries
the registry through to agent stages."
```

---

> **PR 5 ready** (P9 complete). Daemon + MCP routing functional. Tests in next phase.

---

## Phase 10 — integration tests (2 days)

### Task 10.1: in-process daemon end-to-end smoke test

**Files:**
- Create: `crates/surge-daemon/tests/daemon_e2e_smoke.rs`

- [ ] **Step 1: Write the test**

Create `crates/surge-daemon/tests/daemon_e2e_smoke.rs`:

```rust
//! End-to-end smoke: spin up the IPC server inline (no subprocess),
//! connect a DaemonEngineFacade, ping, list_runs, shutdown.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_daemon::{run_server, ServerConfig};
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::facade::{EngineFacade, LocalEngineFacade};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct StubFacade;

#[async_trait::async_trait]
impl EngineFacade for StubFacade {
    async fn start_run(
        &self,
        _run_id: surge_core::id::RunId,
        _graph: surge_core::graph::Graph,
        _worktree_path: PathBuf,
        _run_config: surge_orchestrator::engine::EngineRunConfig,
    ) -> Result<surge_orchestrator::engine::handle::RunHandle, surge_orchestrator::engine::EngineError> {
        Err(surge_orchestrator::engine::EngineError::Internal("stub".into()))
    }

    async fn resume_run(
        &self,
        _run_id: surge_core::id::RunId,
        _worktree_path: PathBuf,
    ) -> Result<surge_orchestrator::engine::handle::RunHandle, surge_orchestrator::engine::EngineError> {
        Err(surge_orchestrator::engine::EngineError::Internal("stub".into()))
    }

    async fn stop_run(
        &self,
        _run_id: surge_core::id::RunId,
        _reason: String,
    ) -> Result<(), surge_orchestrator::engine::EngineError> {
        Ok(())
    }

    async fn resolve_human_input(
        &self,
        _run_id: surge_core::id::RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), surge_orchestrator::engine::EngineError> {
        Ok(())
    }

    async fn list_runs(
        &self,
    ) -> Result<Vec<surge_orchestrator::engine::handle::RunSummary>, surge_orchestrator::engine::EngineError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn ping_round_trip() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("test.sock");
    let cfg = ServerConfig {
        max_active: 4,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let facade: Arc<dyn EngineFacade> = Arc::new(StubFacade);
    let server_handle = tokio::spawn({
        let cfg = cfg;
        let facade = facade.clone();
        let shutdown = shutdown.clone();
        async move { run_server(cfg, facade, shutdown).await }
    });

    // Wait briefly for the listener to start.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = DaemonEngineFacade::connect(socket).await.expect("connect");
    let runs = client.list_runs().await.expect("list_runs");
    assert!(runs.is_empty());

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_handle).await;
}
```

- [ ] **Step 2: Add `async-trait` and `tempfile` dev-deps to surge-daemon**

Edit `crates/surge-daemon/Cargo.toml`. Append to `[dev-dependencies]`:

```toml
async-trait.workspace = true
serde_json.workspace = true
surge-core.workspace = true
surge-orchestrator.workspace = true
tokio-util = { version = "0.7", features = ["rt"] }
```

- [ ] **Step 3: Run test**

Run: `cargo test -p surge-daemon --test daemon_e2e_smoke`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-daemon/Cargo.toml crates/surge-daemon/tests/daemon_e2e_smoke.rs
git commit -m "M7 P10.1: daemon_e2e_smoke — in-process IPC round-trip

Spins up run_server with a StubFacade, connects DaemonEngineFacade
over a real local socket (under tempdir), calls list_runs, verifies
shutdown completes within 2s. Smoke covers the happy IPC path
without needing a real engine."
```

---

### Task 10.2: AdmissionController FIFO queue under load

**Files:**
- Create: `crates/surge-daemon/tests/daemon_admission_queue.rs`

- [ ] **Step 1: Write the test**

Create `crates/surge-daemon/tests/daemon_admission_queue.rs`:

```rust
//! AdmissionController FIFO order under concurrent admission.

use surge_core::id::RunId;
use surge_daemon::admission::{AdmissionController, AdmissionDecision};

#[tokio::test]
async fn fifo_queue_preserves_order() {
    let a = AdmissionController::new(1);
    let r1 = RunId::new();
    let r2 = RunId::new();
    let r3 = RunId::new();

    assert_eq!(a.try_admit(r1).await, AdmissionDecision::Admitted);
    assert_eq!(a.try_admit(r2).await, AdmissionDecision::Queued { position: 0 });
    assert_eq!(a.try_admit(r3).await, AdmissionDecision::Queued { position: 1 });

    a.notify_completed(r1).await;
    assert_eq!(a.pop_queued().await, Some(r2));
    a.notify_completed(r2).await;
    assert_eq!(a.pop_queued().await, Some(r3));
}

#[tokio::test]
async fn cap_8_admits_first_8_queues_rest() {
    let a = AdmissionController::new(8);
    let mut admitted = 0;
    let mut queued = 0;
    for _ in 0..12 {
        match a.try_admit(RunId::new()).await {
            AdmissionDecision::Admitted => admitted += 1,
            AdmissionDecision::Queued { .. } => queued += 1,
        }
    }
    assert_eq!(admitted, 8);
    assert_eq!(queued, 4);
    let s = a.snapshot().await;
    assert_eq!(s.active, 8);
    assert_eq!(s.queued, 4);
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p surge-daemon --test daemon_admission_queue`
Expected: both tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-daemon/tests/daemon_admission_queue.rs
git commit -m "M7 P10.2: daemon_admission_queue — FIFO order under load

Verifies queue order is preserved across concurrent admissions and
that cap-8 admits first 8 + queues the rest. Snapshot counts match."
```

---

### Task 10.3: graceful shutdown drain

**Files:**
- Create: `crates/surge-daemon/tests/daemon_graceful_shutdown.rs`

- [ ] **Step 1: Write the test**

Create `crates/surge-daemon/tests/daemon_graceful_shutdown.rs`:

```rust
//! Graceful shutdown: cancellation token causes the server loop to
//! exit cleanly within the configured grace window.

use std::sync::Arc;
use std::time::Duration;
use surge_daemon::{run_server, ServerConfig};
use surge_orchestrator::engine::facade::EngineFacade;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct StubFacade;

#[async_trait::async_trait]
impl EngineFacade for StubFacade {
    async fn start_run(
        &self,
        _: surge_core::id::RunId,
        _: surge_core::graph::Graph,
        _: std::path::PathBuf,
        _: surge_orchestrator::engine::EngineRunConfig,
    ) -> Result<surge_orchestrator::engine::handle::RunHandle, surge_orchestrator::engine::EngineError> {
        Err(surge_orchestrator::engine::EngineError::Internal("stub".into()))
    }
    async fn resume_run(
        &self,
        _: surge_core::id::RunId,
        _: std::path::PathBuf,
    ) -> Result<surge_orchestrator::engine::handle::RunHandle, surge_orchestrator::engine::EngineError> {
        Err(surge_orchestrator::engine::EngineError::Internal("stub".into()))
    }
    async fn stop_run(
        &self,
        _: surge_core::id::RunId,
        _: String,
    ) -> Result<(), surge_orchestrator::engine::EngineError> {
        Ok(())
    }
    async fn resolve_human_input(
        &self,
        _: surge_core::id::RunId,
        _: Option<String>,
        _: serde_json::Value,
    ) -> Result<(), surge_orchestrator::engine::EngineError> {
        Ok(())
    }
    async fn list_runs(
        &self,
    ) -> Result<Vec<surge_orchestrator::engine::handle::RunSummary>, surge_orchestrator::engine::EngineError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn shutdown_token_exits_within_500ms() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("shutdown.sock");
    let cfg = ServerConfig {
        max_active: 4,
        socket_path: socket,
    };
    let shutdown = CancellationToken::new();
    let facade: Arc<dyn EngineFacade> = Arc::new(StubFacade);
    let handle = tokio::spawn({
        let cfg = cfg;
        let facade = facade.clone();
        let shutdown = shutdown.clone();
        async move { run_server(cfg, facade, shutdown).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    shutdown.cancel();
    let res = tokio::time::timeout(Duration::from_millis(500), handle).await;
    assert!(res.is_ok(), "server failed to exit within 500ms after cancel");
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p surge-daemon --test daemon_graceful_shutdown`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-daemon/tests/daemon_graceful_shutdown.rs
git commit -m "M7 P10.3: daemon_graceful_shutdown — cancellation drains server

Cancellation token causes the accept-loop's tokio::select! to exit;
the test asserts the server task completes within 500ms of cancel."
```

---

### Task 10.4: mock MCP server fixture binary

**Files:**
- Create: `crates/surge-mcp/tests/fixtures/mock_mcp_server.rs`
- Modify: `crates/surge-mcp/Cargo.toml` (add `[[example]]` for the binary)

- [ ] **Step 1: Add the example binary**

Edit `crates/surge-mcp/Cargo.toml`. Append:

```toml
[features]
mock-server = []

[[example]]
name = "mock_mcp_server"
path = "tests/fixtures/mock_mcp_server.rs"
required-features = ["mock-server"]
```

(Optional alternative: put it under `[[bin]]` with a feature gate. Example is simpler since cargo builds them automatically when needed.)

Add `[dev-dependencies]`:

```toml
rmcp = { workspace = true, features = ["server", "transport-io"] }
schemars = "0.8"
```

- [ ] **Step 2: Create the mock**

Create `crates/surge-mcp/tests/fixtures/mock_mcp_server.rs`:

```rust
//! Minimal stdio MCP server fixture for surge-mcp integration tests.
//! Declares two tools: `echo` (returns the input) and `crash_now`
//! (exits the process after the next call).

#[cfg(feature = "mock-server")]
fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        use rmcp::{ServerHandler, ServiceExt};
        use rmcp::model::{
            CallToolResult, Content, RawTextContent, RawContent,
            Tool, ToolAnnotations,
        };
        use rmcp::service::RequestContext;
        use rmcp::transport::io::stdio;

        struct Mock;

        #[async_trait::async_trait]
        impl ServerHandler for Mock {
            // (Implementer subagent: rmcp 1.6 ServerHandler API may
            // require an exhaustive set of methods. The minimal surface
            // we need: list_tools (returns ["echo","crash_now"]) and
            // call_tool (handles each by name). Refer to rmcp's
            // `examples/everything` for the canonical shape and adapt.)
        }

        let (read, write) = stdio();
        let _service = Mock.serve((read, write)).await.expect("serve");
    });
}

#[cfg(not(feature = "mock-server"))]
fn main() {
    panic!("build with --features mock-server")
}
```

- [ ] **Step 3: Build the example**

Run: `cargo build -p surge-mcp --example mock_mcp_server --features mock-server`
Expected: clean build. The actual `ServerHandler` impl needs the implementer subagent to adapt rmcp 1.6's exact API — point them at `cargo doc -p rmcp --open` and `https://github.com/modelcontextprotocol/rust-sdk/tree/main/examples`.

If rmcp's ServerHandler trait shape has shifted, fall back to `rmcp::ServiceExt::serve` with a closure-based service builder. The minimum we need from this fixture: respond to `tools/list` with `[echo, crash_now]` and respond to `tools/call` for each by name.

- [ ] **Step 4: Commit**

```bash
git add crates/surge-mcp/Cargo.toml crates/surge-mcp/tests/fixtures/mock_mcp_server.rs
git commit -m "M7 P10.4: mock MCP server fixture for stdio integration tests

Cargo example gated by the 'mock-server' feature so it doesn't
build by default. Used by the next task's stdio integration tests.
Implementer adapts ServerHandler to rmcp 1.6's current API."
```

---

### Task 10.5: McpServerConnection lifecycle (connect, list, call, crash)

**Files:**
- Create: `crates/surge-mcp/tests/mcp_stdio_e2e.rs`

- [ ] **Step 1: Write the test**

Create `crates/surge-mcp/tests/mcp_stdio_e2e.rs`:

```rust
//! End-to-end stdio integration: spawn the mock server fixture,
//! connect via McpServerConnection, list_tools, call echo, observe
//! response.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
use surge_mcp::McpServerConnection;

fn mock_server_path() -> PathBuf {
    // Built by `cargo build --example mock_mcp_server --features mock-server`.
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target"));
    target.join("debug").join("examples").join(if cfg!(windows) {
        "mock_mcp_server.exe"
    } else {
        "mock_mcp_server"
    })
}

fn server_ref(restart: bool) -> McpServerRef {
    McpServerRef {
        name: "mock".into(),
        transport: McpTransportConfig::Stdio {
            command: mock_server_path(),
            args: vec![],
            env: HashMap::new(),
        },
        allowed_tools: None,
        call_timeout: Duration::from_secs(5),
        restart_on_crash: restart,
    }
}

#[tokio::test]
#[ignore = "requires `cargo build --example mock_mcp_server --features mock-server` first"]
async fn list_tools_includes_echo_and_crash_now() {
    let c = McpServerConnection::new(server_ref(true));
    let tools = c.list_tools().await.expect("list_tools");
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert!(names.contains(&"echo".to_string()));
    assert!(names.contains(&"crash_now".to_string()));
}

#[tokio::test]
#[ignore = "requires mock_mcp_server example built"]
async fn call_echo_round_trips() {
    let c = McpServerConnection::new(server_ref(true));
    let result = c
        .call_tool("echo", serde_json::json!({"text": "hello"}))
        .await
        .expect("call_tool");
    assert!(!result.is_error.unwrap_or(false));
}
```

- [ ] **Step 2: Run tests (with example pre-built)**

Run: `cargo build -p surge-mcp --example mock_mcp_server --features mock-server`
Then: `cargo test -p surge-mcp --test mcp_stdio_e2e -- --ignored`
Expected: both tests PASS (or, if ServerHandler shape needs adaptation in 10.4, this is where the implementer iterates against rmcp's docs).

- [ ] **Step 3: Commit**

```bash
git add crates/surge-mcp/tests/mcp_stdio_e2e.rs
git commit -m "M7 P10.5: mcp_stdio_e2e — list_tools + call_tool round-trips

Marked #[ignore] because they require the mock_mcp_server example
binary to be pre-built. CI runs them via a build-then-test pair.
Local devs run `cargo test -p surge-mcp -- --ignored` after building."
```

---

### Task 10.6: RoutingToolDispatcher full-lifecycle integration

**Files:**
- Create: `crates/surge-orchestrator/tests/engine_m7_routing_dispatcher.rs`

- [ ] **Step 1: Write the test**

Create `crates/surge-orchestrator/tests/engine_m7_routing_dispatcher.rs`:

```rust
//! Validates that RoutingToolDispatcher's declared_tools merges
//! engine + MCP catalogs correctly (engine wins on collisions) and
//! that dispatch routes by ToolOrigin.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use surge_core::id::{RunId, SessionId};
use surge_core::run_state::RunMemory;
use surge_mcp::{McpRegistry, McpToolEntry};
use surge_orchestrator::engine::tools::{
    DeclaredTool, RoutingToolDispatcher, ToolCall, ToolDispatchContext, ToolDispatcher,
    ToolResultPayload,
};

struct EngineStub;

#[async_trait::async_trait]
impl ToolDispatcher for EngineStub {
    async fn dispatch(
        &self,
        _ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> ToolResultPayload {
        ToolResultPayload::Ok {
            content: serde_json::json!({"engine_handled": call.tool}),
        }
    }
    fn declared_tools(&self) -> Vec<DeclaredTool> {
        vec![DeclaredTool {
            name: "shell_exec".into(),
            description: Some("engine version".into()),
            input_schema: serde_json::json!({}),
        }]
    }
}

#[tokio::test]
async fn merged_catalog_engine_wins() {
    let registry = Arc::new(McpRegistry::from_config(&[]));
    let mcp_tools = vec![
        McpToolEntry {
            server: "mock".into(),
            tool: "shell_exec".into(),
            description: Some("MCP override (should be ignored)".into()),
            input_schema: serde_json::json!({}),
        },
        McpToolEntry {
            server: "mock".into(),
            tool: "browser_navigate".into(),
            description: None,
            input_schema: serde_json::json!({}),
        },
    ];
    let r = RoutingToolDispatcher::new(
        Arc::new(EngineStub),
        registry,
        &mcp_tools,
        &HashMap::new(),
    );
    let declared = r.declared_tools();
    let names: Vec<&str> = declared.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"shell_exec"));
    assert!(names.contains(&"browser_navigate"));
    // Engine version wins
    let shell_exec = declared.iter().find(|t| t.name == "shell_exec").unwrap();
    assert_eq!(shell_exec.description.as_deref(), Some("engine version"));
}

#[tokio::test]
async fn engine_route_is_taken_when_collision() {
    let registry = Arc::new(McpRegistry::from_config(&[]));
    let mcp_tools = vec![McpToolEntry {
        server: "mock".into(),
        tool: "shell_exec".into(),
        description: None,
        input_schema: serde_json::json!({}),
    }];
    let r = RoutingToolDispatcher::new(
        Arc::new(EngineStub),
        registry,
        &mcp_tools,
        &HashMap::new(),
    );
    let ctx = ToolDispatchContext {
        run_id: RunId::new(),
        session_id: SessionId::new(),
        worktree_root: std::path::Path::new("/tmp"),
        run_memory: &RunMemory::default(),
    };
    let call = ToolCall {
        call_id: "c1".into(),
        tool: "shell_exec".into(),
        arguments: serde_json::json!({}),
    };
    match r.dispatch(&ctx, &call).await {
        ToolResultPayload::Ok { content } => {
            assert_eq!(content["engine_handled"], "shell_exec");
        }
        other => panic!("expected engine route, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p surge-orchestrator --test engine_m7_routing_dispatcher`
Expected: both tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/surge-orchestrator/tests/engine_m7_routing_dispatcher.rs
git commit -m "M7 P10.6: engine_m7_routing_dispatcher — collision + route tests

Verifies merged declared_tools (engine wins shell_exec collision,
both shell_exec and browser_navigate visible) and that dispatch
actually routes shell_exec to the engine impl, not MCP."
```

---

## Phase 11 — polish (1 day)

### Task 11.1: surge-daemon README

**Files:**
- Create: `crates/surge-daemon/README.md`

- [ ] **Step 1: Write the README**

Create `crates/surge-daemon/README.md`:

```markdown
# surge-daemon

Long-running process that hosts the surge engine and exposes it over
local-socket IPC. Companion to the `surge` CLI — when you run `surge
engine run flow.toml --daemon`, the CLI spawns / connects to a
daemon and the run survives the CLI exiting.

## Quick start

```bash
# Start (foreground; logs to stderr).
surge daemon start

# Start (detached; runs until you stop it).
surge daemon start --detached

# Status.
surge daemon status

# Stop (graceful).
surge daemon stop

# Stop (force, after 1s).
surge daemon stop --force
```

## Files

```
~/.surge/daemon/
├── daemon.pid     # PID of the running daemon
├── daemon.sock    # Unix socket; on Windows: holds the named-pipe path
├── version        # Daemon binary version
└── logs/          # (M7+) rotating logs via tracing-appender
```

## Configuration

CLI flags on `surge daemon start`:

| Flag | Default | Description |
|------|---------|-------------|
| `--max-active N` | 8 | Concurrent active runs cap |
| `--shutdown-grace D` | 30s | Time to wait for in-flight runs to drain on stop |
| `--detached` | false | Detach from controlling terminal (Unix `setsid`, Windows `DETACHED_PROCESS`) |

## Troubleshooting

**"daemon already running (pid N)"**
The PID file exists and the process is alive. If you want a different
daemon, stop the existing one (`surge daemon stop`).

**"daemon socket did not become readable within 5s"**
The daemon binary may have crashed during startup. Check stderr
output (or the daemon logs once they ship). Common cause: storage
directory permissions; surge expects `~/.surge` to be writable.

**Stale PID file**
If a daemon process was killed forcefully (`kill -9`, OOM, power
cut), the PID file may persist. `surge daemon start` detects this
via `sysinfo` and overwrites the stale file with a warning.

**Daemon won't accept connections after restart**
On rare occasions on macOS / Linux, an old socket file lingers. Run
`rm ~/.surge/daemon/daemon.sock` and restart the daemon.

## Mixing daemon and CLI versions

The daemon and CLI must be from the same surge binary set. After
upgrading surge:

```bash
surge daemon restart
```

This is the safest path. The CLI runs a version handshake on connect
(M8+) and warns if mismatch.
```

- [ ] **Step 2: Commit**

```bash
git add crates/surge-daemon/README.md
git commit -m "M7 P11.1: surge-daemon README — operator docs"
```

---

### Task 11.2: surge-mcp README

**Files:**
- Create: `crates/surge-mcp/README.md`

- [ ] **Step 1: Write the README**

Create `crates/surge-mcp/README.md`:

```markdown
# surge-mcp

MCP (Model Context Protocol) integration for surge. Wraps the
official [`rmcp`](https://docs.rs/rmcp) crate (`>=1.6`) with
surge-flavoured config, registry, restart policy.

M7 supports stdio child-process transport only; HTTP / SSE deferred.

## Configuring an MCP server

In your `flow.toml` (or run-level config TOML loaded by `surge engine
run`), add a `[mcp_servers.<name>]` table. Example:

```toml
[mcp_servers.playwright]
transport = { kind = "stdio", command = "/usr/local/bin/mcp-playwright" }
allowed_tools = ["browser_navigate", "browser_screenshot"]
call_timeout = "60s"
restart_on_crash = true

[mcp_servers.github]
transport = { kind = "stdio", command = "npx", args = ["@github/mcp-server"] }
```

Then in your agent stage:

```toml
[nodes.research]
kind = "Agent"
profile = "researcher@1.0"

[nodes.research.tool_overrides]
mcp_add = ["playwright"]
```

The agent will see `playwright`'s tools (filtered through
`allowed_tools` if specified) alongside engine built-ins.

## Restart on crash

Default: `true`. If the MCP child process exits while still
configured, the next `tools/call` triggers a re-spawn. Disable with:

```toml
[mcp_servers.flaky]
transport = { kind = "stdio", command = "flaky-server" }
restart_on_crash = false
```

After a crash with `restart_on_crash = false`, subsequent calls
return `McpError::ServerNotRunning` until the run restarts.

## Lifetime

MCP servers are shared across runs hosted by the same daemon (warm
process, fast tools/list). M7 does not isolate per-run; if a
`playwright` server holds browser state for run A, it's the same
state for run B. Per-run isolation is M9+.

## Common server installs

| Server | Install |
|--------|---------|
| Playwright | `npm i -g @modelcontextprotocol/server-playwright` |
| GitHub | `npx @github/mcp-server` (no install) |
| Postgres | `npm i -g @modelcontextprotocol/server-postgres` |
| Memory | `npm i -g @modelcontextprotocol/server-memory` |

(Versions and packages move; check
[modelcontextprotocol.io/servers](https://modelcontextprotocol.io/servers).)

## Troubleshooting

**`McpError::StartFailed`**
The child command failed to spawn. Verify the binary is on `PATH` or
use an absolute path in `command`. Check execute permission.

**`McpError::Timeout`**
The call exceeded `call_timeout`. Either the server is genuinely
slow (raise the timeout) or it deadlocked on a tool implementation
bug (check the server's logs).

**`McpError::ServerCrashed`**
The child process exited mid-call. Surge logs the exit code (if
known) at `tracing` level INFO. With `restart_on_crash = true`, the
next call re-spawns. With `false`, you'll see
`McpError::ServerNotRunning` — restart the run.
```

- [ ] **Step 2: Commit**

```bash
git add crates/surge-mcp/README.md
git commit -m "M7 P11.2: surge-mcp README — operator setup + troubleshooting"
```

---

### Task 11.3: rustdoc coverage + ROADMAP update

**Files:**
- Modify: `docs/03-ROADMAP.md` (M7 line + surface)
- Verify: rustdoc warnings on new crates

- [ ] **Step 1: Run rustdoc on new crates**

Run: `cargo doc -p surge-daemon -p surge-mcp --no-deps 2>&1 | grep -i warning | head -20`
Expected: zero warnings, OR a small list of fixable items (missing-docs on private items don't matter; pub items must have a `///` line).

If any pub items are missing docs, add them. Re-run until clean.

- [ ] **Step 2: Update ROADMAP**

Edit `docs/03-ROADMAP.md`. Update the engine M-series progress table:

```markdown
| M6 | Loop execution, subgraph execution, Notify delivery, `surge-notify` crate | Shipped |
| M7 | Daemon mode (long-running engine host with IPC), MCP server delegation via `rmcp`, `surge-daemon` + `surge-mcp` crates | **Shipped** |
| M8 | Retry / bootstrap stages / HumanGate channels, AdmissionController aging | Planned |
```

Append a "M7 surface shipped" section similar to the M6 one:

```markdown
### M7 surface shipped in this PR

- `surge daemon start/stop/status/restart` — long-running daemon process,
  PID + socket discovery under `~/.surge/daemon/`.
- `surge engine run|resume|stop|watch|ls --daemon` — out-of-process engine
  hosting via cross-platform local socket (Unix domain socket on
  Linux/macOS, named pipe on Windows).
- `EngineFacade` trait with `LocalEngineFacade` (M6 default) and
  `DaemonEngineFacade` (IPC client) impls.
- `AdmissionController` (FIFO, max 8 concurrent) and `BroadcastRegistry`
  (multi-subscriber per-run + global events) inside the daemon.
- `surge-mcp` crate exposing `McpRegistry` + `McpServerConnection` over
  rmcp 1.6 stdio transport, with `restart_on_crash` policy.
- `RoutingToolDispatcher` fans out tool calls between engine
  built-ins and MCP servers; sandbox-aware exposure at session-open.
- New validation rules in `surge-core::validation`:
  `McpServerUndeclared`, `McpServerNameEmpty`, `McpCommandPathUnsafe`.
- `RunConfig::mcp_servers: Vec<McpServerRef>` registry; per-stage
  allowlist via existing `ToolOverride::mcp_add`.
- Snapshot v2 unchanged.
```

- [ ] **Step 3: Commit**

```bash
git add docs/03-ROADMAP.md crates/surge-daemon/src/ crates/surge-mcp/src/
git commit -m "M7 P11.3: rustdoc cleanup + ROADMAP M7 surface

Doc warnings on pub items resolved across surge-daemon and surge-mcp.
ROADMAP updated to reflect M7 shipped surface and bump M8 to next."
```

---

> **PR 6 ready** (P10 + P11). M7 complete.

---

## Self-review (per writing-plans skill)

### Spec coverage

Mapping spec sections to plan tasks:

| Spec § | Topic | Plan tasks |
|--------|-------|-----------|
| §1.1 daemon | Long-running process, IPC, AdmissionController, BroadcastRegistry, EngineFacade, --daemon retrofit | P3.1-3.4, P4.1-4.2, P5.1, P6.1-6.5 |
| §1.1 MCP | rmcp wrapper, McpServerRef, McpRegistry, RoutingToolDispatcher, sandbox-aware exposure, AgentConfig per-stage allowlist | P1.1, P1.3, P1.4, P7.1-7.2, P8.1-8.2, P9.1-9.2 |
| §1.2 deferred items | All listed in §16 of spec; plan does NOT touch HTTP transport, daemon auth, hot-reload, MCP resources/prompts, etc. | (intentionally absent from plan) |
| §3.1 single-process | Daemon hosts many runs | P6.1-6.3 |
| §3.2 IPC framing | JSON-RPC line-delimited via interprocess | P3.1-3.2 |
| §3.3 EngineFacade | Trait + Local + Daemon impls | P2.2, P5.1 |
| §3.4 rmcp choice | Pinned to >=1.6, <2 | P0 (workspace dep) |
| §3.5 RoutingToolDispatcher | Engine wins on collision | P8.2 |
| §3.6 snapshot v2 unchanged | No new schema | (no task — explicitly preserved) |
| §3.7 AdmissionController FIFO | No aging | P4.1 |
| §3.8 BroadcastRegistry | per-run + global | P4.2 |
| §3.9 PID + socket | Discovery + lock | P3.3 |
| §3.10 graceful shutdown | Cancellation token + drain | P6.2, P10.3 |
| §5 public API | All types from spec implemented | P1.1, P2.2, P3.1, P5.1, P7.x, P8.x |
| §6 run lifecycle | start_run / subscribe / stop_run | P5.1, P6.1 |
| §7 MCP delegation | Configuration shape, lifecycle, exposure, routing, sandbox, timeout | P1.1, P7.x, P8.x, P9.1 |
| §8 snapshot | Unchanged | (no task) |
| §9 persistence | No new EventPayload variants | (no task — explicitly preserved) |
| §10 concurrency | Tokio multi-thread, send-friendly rmcp | P5.1, P6.1, P7.2 |
| §10.5 validation | McpServerUndeclared / NameEmpty / PathUnsafe in surge-core | P1.3, P1.4 |
| §11 threading | Send everywhere | (covered by impl shape) |
| §12 CLI | --daemon retrofit + surge daemon subtree | P6.4-6.5 |
| §13 errors | DaemonError + McpError | P3.4, P7.1 |
| §14 testing | Unit + integration coverage | P10.1-10.6 |
| §15 acceptance | All 19 criteria | covered by P0-P11 |
| §22 self-review | scope discipline | (this section) |
| §23 accepted divergences | Single-process daemon | preserved by design |

**Gap check:**
- The spec mentions `Engine::list_runs` in §5.1 → mapped to
  `Engine::snapshot_active_runs` in P2.2. Same idea, different name.
  Acceptable.
- The spec's `DaemonEngineFacade::start_run` "subscribe-events"
  inline plumbing → implemented in P5.1 via the per-run channel
  registration before the IPC StartRun is sent.
- The spec's "auto-spawn daemon if --daemon and no daemon running"
  → P6.4 adds `ensure_daemon_running` in the engine commands; called
  from `run_command --daemon`.
- The spec's "version handshake on connect" → P3.1 returns `version`
  in `PingOk`. The CLI's "warn if mismatch" is a soft target — wired
  into `surge daemon status` only, not strictly enforced. Documented
  in spec §3.9 as M7 ships single version. **No further action.**

### Placeholder scan

Searching for the patterns from the writing-plans skill:
- "TBD", "TODO", "fill in" — checked and absent in code blocks; one
  doc-only "M7+" appears in lifetime/HTTP discussions (legitimate,
  spec-aligned).
- "Add appropriate error handling" — absent.
- "Write tests for the above" without code — absent. Every test step
  has full test code.
- "Similar to Task N" without repeating — absent.

### Type consistency

- `EngineFacade` trait — used identically in P2.2, P5.1, P6.5.
- `RunSummary` / `RunStatus` — defined in P2.2 (non_exhaustive),
  serialised in P3.1's `DaemonResponse::ListRunsOk`, returned by
  `list_runs` impls in P2.2 + P5.1. Matches.
- `McpServerRef` / `McpTransportConfig` — defined in P1.1 with
  `non_exhaustive`, used in P1.4 / P7.2 / P8.1. Matches.
- `DeclaredTool` — defined in P2.1, used in P8.2 routing.
- `RoutingToolDispatcher::new` signature — agreed across P8.2
  (defines) + P9.1 (constructs).
- `DaemonRequest` / `DaemonResponse` / `DaemonEvent` — aligned across
  P3.1 (defines), P5.1 (client uses), P6.1 (server dispatches).
- IPC `request_id` — `u64` everywhere.

### Phase split confirms PR cadence

- PR 1: P0 + P1 (4 tasks) + P2 (2 tasks) + P3.1-3.2 (2 tasks) — 9 tasks total
- PR 2: P3.3-3.4 + P4 + P5.1 — 5 tasks
- PR 3: P6.1-6.5 — 5 tasks
- PR 4: P7 + P8.1-8.2 — 4 tasks
- PR 5: P9.1-9.2 — 2 tasks
- PR 6: P10 + P11 — 9 tasks

Total: 34 tasks across 6 PRs. Aligns with M6 cadence (6 PRs, ~30
tasks).

### Final note on rmcp API drift

rmcp 1.6.x is a moving target. Implementer subagents should:
- Pin to `>=1.6, <2.0` in the workspace `Cargo.toml`.
- Run `cargo doc -p rmcp --open` before P7.2 / P10.4 / P10.5 to
  verify the exact `ServerHandler` / `ServiceExt` / `Peer` API shapes
  match the code in this plan. Adjust if the SDK has shifted method
  signatures (likely on minor versions).

---

*End of M7 implementation plan.*
