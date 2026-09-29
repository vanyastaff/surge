//! `surge mcp serve` — Surge as an MCP *server* over stdio.
//!
//! Lets a "main agent" (Claude Code, Codex, any MCP client) drive Surge:
//! read the fleet inbox, inspect runs, steer them, answer the human gates the
//! operator surfaced, and start the idea → description → roadmap → flow
//! bootstrap journey. Every tool is a thin adapter over the same
//! `pub(crate)` functions the matching CLI command uses (`inbox`, `ready`,
//! `ledger`, `run report`, `steer`, `resolve`, `memory search`) — no query or
//! validation logic lives here.
//!
//! # Safety model
//!
//! - Read tools (`surge_inbox`, `surge_run_status`, `surge_ready_tasks`,
//!   `surge_ledger`, `surge_run_report`, `surge_memory_search`) are always
//!   available and never need the daemon.
//! - Mutating tools (`surge_steer`, `surge_resolve`, `surge_bootstrap_start`)
//!   are refused unless the server was started with `--allow-write`, and every
//!   accepted mutation is logged (to stderr, target
//!   `surge::mcp_serve::audit`) with the client name the caller announced in
//!   its MCP `initialize` request.
//! - `surge_resolve` only answers a run that currently sits in the inbox's
//!   NEEDS INPUT group, only with a decision the pending gate declares, and
//!   never answers a bootstrap-mode gate (description / roadmap / flow
//!   approval): those stay human decisions.
//! - stdout carries the MCP protocol only; diagnostics go to stderr.
//!
//! Failures are reported as MCP tool errors (`isError: true`), never panics.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::service::{Peer, RoleServer};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use surge_core::RunId;
use surge_core::bootstrap_operation::BootstrapIntent;
use surge_orchestrator::engine::daemon_facade::{BootstrapClientError, DaemonEngineFacade};
use surge_persistence::memory::MemoryStore;
use surge_persistence::runs::Storage;
use surge_persistence::task_ledger::TaskLedgerIndexRecord;
use tokio::sync::OnceCell;

use crate::commands::common::{connect_daemon_at, project_root, resolve_run_id};
use crate::commands::ledger::{LedgerArgs, query_records as query_ledger};
use crate::commands::ready::{ReadyArgs, query_records as query_ready};
use crate::commands::resolve::{PendingInput, build_answer, deliver_answer, inspect_pending};
use crate::commands::{bootstrap, inbox, memory, run, steer};

/// Default row cap for the inbox's Done tail and the ready backlog.
const DEFAULT_ROW_LIMIT: usize = 200;
/// Default row cap for `surge_ledger` (matches `surge ledger`).
const DEFAULT_LEDGER_LIMIT: usize = 500;
/// Hard ceiling on any caller-supplied row limit.
const MAX_ROW_LIMIT: usize = 5000;
/// Default and ceiling for `surge_memory_search` results per category.
const DEFAULT_MEMORY_LIMIT: usize = 10;
const MAX_MEMORY_LIMIT: usize = 50;
/// Client name recorded when the peer never announced one.
const UNKNOWN_CLIENT: &str = "unknown";

/// Everything a server instance needs, resolved once at startup so tool calls
/// never re-read process-global state such as `SURGE_HOME`.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Resolved surge home (`~/.surge` or `$SURGE_HOME`).
    pub home: PathBuf,
    /// Project root `surge_bootstrap_start` bootstraps (the enclosing git
    /// repository of the server's working directory, else the directory itself).
    pub project_root: PathBuf,
    /// Whether the mutating tools are enabled (`--allow-write`).
    pub allow_write: bool,
}

/// Entry point for `surge mcp serve`: serve MCP over this process's
/// stdin/stdout until the client disconnects.
///
/// # Errors
/// Returns an error if the home directory cannot be resolved or the MCP
/// transport fails.
pub async fn run(allow_write: bool) -> Result<()> {
    let home = crate::commands::common::surge_home_dir()?;
    let cwd = std::env::current_dir().context("resolve working directory")?;
    let options = ServeOptions {
        project_root: project_root(&cwd),
        home,
        allow_write,
    };
    tracing::info!(
        allow_write,
        home = %options.home.display(),
        "surge MCP server starting on stdio"
    );
    let outcome = serve(options, rmcp::transport::stdio()).await;
    // Tokio's stdin read cannot be cancelled: if the service ended while the
    // client still holds stdin open, dropping the runtime would block on it
    // forever. Same reasoning as `internal-stage-mcp` in `main.rs`.
    let code = match &outcome {
        Ok(()) => 0,
        Err(error) => {
            tracing::error!(error = %error, "surge MCP server stopped with an error");
            1
        },
    };
    std::process::exit(code)
}

/// Serve MCP over `transport` until the peer disconnects. Split from [`run`]
/// so tests can drive it over an in-memory duplex stream.
///
/// # Errors
/// Returns an error if the MCP handshake or transport fails.
pub async fn serve<T, E, A>(options: ServeOptions, transport: T) -> Result<()>
where
    T: rmcp::transport::IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let service = SurgeMcpServer::new(options)
        .serve(transport)
        .await
        .context("MCP handshake failed")?;
    service.waiting().await.context("MCP session failed")?;
    Ok(())
}

/// A tool call's failure, reported to the client as an MCP tool error.
#[derive(Debug, thiserror::Error)]
enum ToolError {
    /// A mutating tool was called on a read-only server.
    #[error(
        "`{0}` mutates run state and this server is read-only; restart it with `surge mcp serve --allow-write` to enable it"
    )]
    WriteDisabled(&'static str),
    /// The tool needs the daemon and none is reachable.
    #[error("daemon not running — start it with `surge daemon start` ({0})")]
    DaemonNotRunning(String),
    /// The request is understood but refused (bad state, invalid decision).
    #[error("{0}")]
    Rejected(String),
    /// Anything else (storage, IO, malformed identifiers).
    #[error("{0:#}")]
    Failed(#[from] anyhow::Error),
}

impl ToolError {
    fn kind(&self) -> &'static str {
        match self {
            Self::WriteDisabled(_) => "write_disabled",
            Self::DaemonNotRunning(_) => "daemon_not_running",
            Self::Rejected(_) => "rejected",
            Self::Failed(_) => "failed",
        }
    }
}

/// A successful tool result: a short human summary plus structured JSON
/// (always a JSON object, as MCP requires of `structuredContent`).
struct ToolOutput {
    summary: String,
    data: Value,
}

type ToolResult = Result<ToolOutput, ToolError>;

fn into_call_result(result: ToolResult) -> CallToolResult {
    match result {
        Ok(ToolOutput { summary, data }) => {
            let mut call = CallToolResult::success(vec![Content::text(summary)]);
            call.structured_content = Some(data);
            call
        },
        Err(error) => {
            let message = error.to_string();
            let mut call = CallToolResult::error(vec![Content::text(message.clone())]);
            call.structured_content = Some(json!({
                "error": { "kind": error.kind(), "message": message }
            }));
            call
        },
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Value, ToolError> {
    serde_json::to_value(value)
        .map_err(|e| ToolError::Failed(anyhow::Error::new(e).context("serialize tool output")))
}

/// Arguments of `surge_inbox`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct InboxParams {
    /// List the Done group in full (default: only its count).
    #[serde(default)]
    pub include_done: bool,
    /// Maximum Done runs considered (default 200); NEEDS INPUT / WORKING /
    /// WAITING runs are never truncated.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Arguments of `surge_run_status`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunIdParams {
    /// Run id: the full ULID or a unique suffix of at least 6 characters, as
    /// shown by `surge_inbox`.
    pub run_id: String,
}

/// Arguments of `surge_ready_tasks`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ReadyParams {
    /// Only tasks with this exact status (e.g. `pending`,
    /// `ready_for_verification`, `failed_verification`).
    #[serde(default)]
    pub status: Option<String>,
    /// Only tasks discovered mid-run (with a `discovered_from` edge).
    #[serde(default)]
    pub discovered: bool,
    /// Only tasks belonging to this run.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Maximum rows (default 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Arguments of `surge_ledger`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct LedgerParams {
    /// Scope to one run; omit for every project's ledger (per-project scoping
    /// is not available yet — same as `surge ledger`).
    #[serde(default)]
    pub run_id: Option<String>,
    /// Maximum rows (default 500).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Text rendering `surge_run_report` returns alongside the structured report.
#[derive(Debug, Default, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReportFormat {
    /// One-line summary as text (default); the full report is structured.
    #[default]
    Json,
    /// The full Markdown report as text.
    Md,
}

/// Arguments of `surge_run_report`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunReportParams {
    /// Run id (full ULID or unique suffix).
    pub run_id: String,
    /// Text rendering to return next to the structured report.
    #[serde(default)]
    pub format: ReportFormat,
}

/// Arguments of `surge_steer`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SteerParams {
    /// Run id (full ULID or unique suffix). Must be active in the daemon.
    pub run_id: String,
    /// Guidance for the run's agent. Delivered at the next stage boundary and
    /// prepended to that stage's prompt; it does not interrupt the current
    /// agent turn.
    pub message: String,
}

/// Arguments of `surge_resolve`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ResolveParams {
    /// Run id (full ULID or unique suffix). Must be in `surge_inbox`'s
    /// `needs_input` group.
    pub run_id: String,
    /// For a HumanGate: one of the outcome keys listed in the run's
    /// `pending_input.options` (see `surge_run_status`). For a free-form
    /// tool-driven question: the answer text.
    pub decision: String,
    /// Optional operator comment attached to a HumanGate decision.
    #[serde(default)]
    pub note: Option<String>,
    /// Optional guard: the gate node you were shown. The call is refused if
    /// the run has since moved to a different pending node.
    #[serde(default)]
    pub expected_node: Option<String>,
}

/// Arguments of `surge_bootstrap_start`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BootstrapStartParams {
    /// The idea to turn into a description, roadmap and flow.
    pub idea: String,
}

/// Arguments of `surge_memory_search`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MemorySearchParams {
    /// Full-text query over the project memory (discoveries, patterns,
    /// gotchas, file contexts).
    pub query: String,
    /// Keep only entries carrying at least one of these tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Maximum results per category (default 10, max 50).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Shared, immutable server state.
struct Inner {
    options: ServeOptions,
    storage: OnceCell<Arc<Storage>>,
}

/// The MCP server handler exposing Surge's operator surface as tools.
#[derive(Clone)]
pub struct SurgeMcpServer {
    inner: Arc<Inner>,
    // Built once; read by the `#[tool_handler]`-generated `call_tool` / `list_tools`.
    tool_router: ToolRouter<Self>,
}

impl SurgeMcpServer {
    /// Build a server for `options`. Storage is opened lazily on first use.
    #[must_use]
    pub fn new(options: ServeOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                options,
                storage: OnceCell::new(),
            }),
            tool_router: Self::tool_router(),
        }
    }

    async fn storage(&self) -> Result<&Arc<Storage>, ToolError> {
        self.inner
            .storage
            .get_or_try_init(|| async {
                Storage::open(&self.inner.options.home)
                    .await
                    .context("open surge storage")
            })
            .await
            .map_err(ToolError::Failed)
    }

    async fn daemon(&self) -> Result<DaemonEngineFacade, ToolError> {
        let socket = surge_daemon::pidfile::socket_path_in(&self.inner.options.home);
        connect_daemon_at(socket)
            .await
            .map_err(|e| ToolError::DaemonNotRunning(e.to_string()))
    }

    fn require_write(&self, tool: &'static str) -> Result<(), ToolError> {
        if self.inner.options.allow_write {
            Ok(())
        } else {
            Err(ToolError::WriteDisabled(tool))
        }
    }

    /// Resolve `value` (full ULID or unique suffix) and require the run to
    /// exist in the registry.
    async fn existing_run(&self, value: &str) -> Result<RunId, ToolError> {
        let storage = self.storage().await?;
        let run_id = resolve_run_id(storage, value).await?;
        match storage.get_run(&run_id).await {
            Ok(Some(_)) => Ok(run_id),
            Ok(None) => Err(ToolError::Rejected(format!("no run {run_id}"))),
            Err(e) => Err(ToolError::Failed(
                anyhow::Error::new(e).context("read run registry"),
            )),
        }
    }

    async fn inbox_impl(&self, params: InboxParams) -> ToolResult {
        let storage = self.storage().await?;
        let limit = clamp_limit(params.limit, DEFAULT_ROW_LIMIT);
        let entries = inbox::collect_entries(storage, None, limit).await?;

        let mut groups: [Vec<Value>; 4] = Default::default();
        for entry in &entries {
            let slot = match entry.attention() {
                "needs_input" => 0,
                "working" => 1,
                "waiting" => 2,
                _ => 3,
            };
            groups[slot].push(to_json(entry)?);
        }
        let [needs_input, working, waiting, done] = groups;
        let summary = format!(
            "{} need input, {} working, {} waiting, {} done",
            needs_input.len(),
            working.len(),
            waiting.len(),
            done.len()
        );
        let done_count = done.len();
        Ok(ToolOutput {
            summary,
            data: json!({
                "needs_input": needs_input,
                "working": working,
                "waiting": waiting,
                "done": {
                    "count": done_count,
                    "runs": if params.include_done { done } else { Vec::new() },
                },
            }),
        })
    }

    async fn run_status_impl(&self, params: RunIdParams) -> ToolResult {
        let run_id = self.existing_run(&params.run_id).await?;
        let storage = self.storage().await?;
        let summary = storage
            .get_run(&run_id)
            .await
            .map_err(|e| ToolError::Failed(anyhow::Error::new(e).context("read run registry")))?
            .ok_or_else(|| ToolError::Rejected(format!("no run {run_id}")))?;
        let entry = inbox::classify(storage, &summary).await?;

        let mut pending_input = Value::Null;
        let mut note = Value::Null;
        if entry.is_needs_input() {
            match inspect_pending(storage, run_id).await {
                Ok(pending) => pending_input = pending_to_json(&pending),
                Err(_) => {
                    note = json!(
                        "the run awaits an approval that is not a pipeline gate (bootstrap \
                         approval); answer it in the desktop app, Telegram, or `surge bootstrap`"
                    );
                },
            }
        }
        let text = format!(
            "run {run_id}: {} ({})",
            entry.attention(),
            serde_json::to_value(summary.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        );
        Ok(ToolOutput {
            summary: text,
            data: json!({
                "run": to_json(&entry)?,
                "registry_status": to_json(&summary.status)?,
                "pending_input": pending_input,
                "note": note,
            }),
        })
    }

    async fn ready_impl(&self, params: ReadyParams) -> ToolResult {
        let storage = self.storage().await?;
        let run_id = self.optional_run_id(params.run_id.as_deref()).await?;
        let args = ReadyArgs {
            status: params.status,
            discovered: params.discovered,
            run_id,
            all_projects: false,
            limit: clamp_limit(params.limit, DEFAULT_ROW_LIMIT),
            json: true,
        };
        let records = query_ready(storage, &args)?;
        let tasks = normalized(records);
        Ok(ToolOutput {
            summary: format!("{} actionable task(s)", tasks.len()),
            data: json!({ "count": tasks.len(), "tasks": to_json(&tasks)? }),
        })
    }

    async fn ledger_impl(&self, params: LedgerParams) -> ToolResult {
        let storage = self.storage().await?;
        let run_id = self.optional_run_id(params.run_id.as_deref()).await?;
        let args = LedgerArgs {
            run_id,
            all_projects: false,
            limit: clamp_limit(params.limit, DEFAULT_LEDGER_LIMIT),
            json: true,
        };
        let tasks = normalized(query_ledger(storage, &args)?);
        let verified = tasks.iter().filter(|t| t.is_evidence_backed()).count();
        Ok(ToolOutput {
            summary: format!("{} task(s), {verified} verified", tasks.len()),
            data: json!({
                "count": tasks.len(),
                "verified": verified,
                "tasks": to_json(&tasks)?,
            }),
        })
    }

    /// Resolve an optional run reference to the full-ULID string the ledger
    /// queries expect.
    async fn optional_run_id(&self, value: Option<&str>) -> Result<Option<String>, ToolError> {
        match value {
            Some(value) => Ok(Some(self.existing_run(value).await?.to_string())),
            None => Ok(None),
        }
    }

    async fn run_report_impl(&self, params: RunReportParams) -> ToolResult {
        let storage = self.storage().await?;
        let report = run::compile_report(storage, &params.run_id).await?;
        let completion = to_json(&report.completion)?;
        let one_line = format!(
            "run {}: {} — {} node(s), {} verdict(s), evidence_backed={}",
            report.run_id,
            completion
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            report.nodes.len(),
            report.verdicts.len(),
            report
                .evidence_backed
                .map_or_else(|| "n/a".to_owned(), |v| v.to_string()),
        );
        let summary = match params.format {
            ReportFormat::Json => one_line,
            ReportFormat::Md => surge_core::run_report::render_markdown(&report),
        };
        Ok(ToolOutput {
            summary,
            data: json!({ "report": to_json(&report)? }),
        })
    }

    async fn steer_impl(&self, client: &str, params: SteerParams) -> ToolResult {
        self.require_write("surge_steer")?;
        let run_id = self.existing_run(&params.run_id).await?;
        if params.message.trim().is_empty() {
            return Err(ToolError::Rejected(
                "steer message must not be blank".into(),
            ));
        }
        let daemon = self.daemon().await?;
        // The message itself is not logged: it may carry sensitive guidance.
        tracing::info!(
            target: "surge::mcp_serve::audit",
            client = ?client,
            tool = "surge_steer",
            %run_id,
            message_len = params.message.len(),
            "MCP mutation: queueing steer"
        );
        let steer_id = steer::queue_steer(&daemon, run_id, &params.message).await?;
        Ok(ToolOutput {
            summary: format!("steer {steer_id} queued for run {run_id}; applies at the next stage"),
            data: json!({ "run_id": run_id.to_string(), "steer_id": steer_id }),
        })
    }

    async fn resolve_impl(&self, client: &str, params: ResolveParams) -> ToolResult {
        self.require_write("surge_resolve")?;
        let run_id = self.existing_run(&params.run_id).await?;
        let storage = self.storage().await?;

        // Gate 1: only what the inbox surfaces. The run must be in NEEDS INPUT
        // right now, judged by the very classifier `surge inbox` uses.
        let summary = storage
            .get_run(&run_id)
            .await
            .map_err(|e| ToolError::Failed(anyhow::Error::new(e).context("read run registry")))?
            .ok_or_else(|| ToolError::Rejected(format!("no run {run_id}")))?;
        let entry = inbox::classify(storage, &summary).await?;
        if !entry.is_needs_input() {
            return Err(ToolError::Rejected(format!(
                "run {run_id} is not in the inbox's needs_input group (it is {}); only a gate \
                 surfaced by `surge_inbox` can be resolved",
                entry.attention()
            )));
        }
        let pending = inspect_pending(storage, run_id)
            .await
            .map_err(|e| ToolError::Rejected(format!("{e:#}")))?;

        // Gate 2: bootstrap approvals are human decisions, never an agent's.
        if pending.is_bootstrap_gate {
            return Err(ToolError::Rejected(format!(
                "run {run_id} is blocked at a bootstrap approval (@{}); description, roadmap and \
                 flow approvals must be given by a human in the desktop app, Telegram, or \
                 `surge bootstrap`",
                pending.node
            )));
        }
        // Gate 3: the caller must be answering the gate it was shown.
        if let Some(expected) = params.expected_node.as_deref()
            && expected != pending.node.as_ref() as &str
        {
            return Err(ToolError::Rejected(format!(
                "run {run_id} is now blocked at @{}, not @{expected}; re-read `surge_run_status`",
                pending.node
            )));
        }
        // Gate 4: a HumanGate must declare its options so the decision can be
        // validated; refusing beats forwarding an unchecked outcome.
        if !pending.is_tool_call && pending.gate_options.is_empty() {
            return Err(ToolError::Rejected(format!(
                "the gate at @{} declares no outcome options, so a decision cannot be validated",
                pending.node
            )));
        }

        let decision = params.decision.trim();
        let (_, response) = if pending.is_tool_call {
            build_answer(
                true,
                pending.call_id.clone(),
                &pending.gate_options,
                None,
                None,
                Some(decision),
                None,
            )
        } else {
            build_answer(
                false,
                pending.call_id.clone(),
                &pending.gate_options,
                Some(decision),
                params.note.as_deref(),
                None,
                None,
            )
        }
        .map_err(|e| ToolError::Rejected(format!("{e:#}")))?;

        let daemon = self.daemon().await?;
        tracing::info!(
            target: "surge::mcp_serve::audit",
            client = ?client,
            tool = "surge_resolve",
            %run_id,
            node = %pending.node,
            decision = if pending.is_tool_call { "<free-form answer>" } else { decision },
            "MCP mutation: resolving human input"
        );
        deliver_answer(&daemon, run_id, &pending, response).await?;
        Ok(ToolOutput {
            summary: format!("resolved run {run_id} at @{}", pending.node),
            data: json!({ "run_id": run_id.to_string(), "node": pending.node.to_string() }),
        })
    }

    async fn bootstrap_start_impl(&self, client: &str, params: BootstrapStartParams) -> ToolResult {
        self.require_write("surge_bootstrap_start")?;
        let idea = params.idea.trim();
        if idea.is_empty() {
            return Err(ToolError::Rejected("idea must not be blank".into()));
        }
        let daemon = self.daemon().await?;
        let project_root = self.inner.options.project_root.clone();
        let config = bootstrap::load_project_config_at(&project_root)?;
        let intent = BootstrapIntent::new(
            project_root.clone(),
            idea.to_owned(),
            config.analytics.budget_guard(),
        )
        .map_err(|e| ToolError::Rejected(e.to_string()))?;

        let operation_id = RunId::new();
        tracing::info!(
            target: "surge::mcp_serve::audit",
            client = ?client,
            tool = "surge_bootstrap_start",
            %operation_id,
            project = %project_root.display(),
            idea_len = idea.len(),
            "MCP mutation: starting bootstrap"
        );
        let status = daemon
            .start_bootstrap(operation_id, intent)
            .await
            .map_err(|e| match e {
                BootstrapClientError::NotReady => ToolError::Rejected(
                    "the daemon has no durable bootstrap supervisor available".into(),
                ),
                other => ToolError::Rejected(other.to_string()),
            })?;
        Ok(ToolOutput {
            summary: format!(
                "bootstrap operation {operation_id} accepted (planning run {}). It will stop at \
                 the description, roadmap and flow approval gates for a human to decide.",
                status.planning_run
            ),
            data: json!({
                "operation_id": operation_id.to_string(),
                "planning_run": status.planning_run.to_string(),
                "implementation_run": status.implementation_run.to_string(),
                "status": to_json(&status)?,
                "human_gates": "description, roadmap and flow approvals are answered by a human \
                                (desktop app, Telegram, `surge bootstrap`); this server cannot \
                                approve them",
            }),
        })
    }

    async fn memory_search_impl(&self, params: MemorySearchParams) -> ToolResult {
        let query = params.query.trim().to_owned();
        if query.is_empty() {
            return Err(ToolError::Rejected("query must not be blank".into()));
        }
        let store_path = MemoryStore::path_in(&self.inner.options.home);
        if !store_path.exists() {
            return Ok(ToolOutput {
                summary: "no project memory recorded yet".into(),
                data: json!({
                    "total": 0,
                    "results": { "discoveries": [], "patterns": [], "gotchas": [], "file_contexts": [] },
                }),
            });
        }
        let limit = params
            .limit
            .unwrap_or(DEFAULT_MEMORY_LIMIT)
            .clamp(1, MAX_MEMORY_LIMIT);
        let tags = params.tags;
        // rusqlite is synchronous; keep it off the async workers.
        let results = tokio::task::spawn_blocking(move || {
            let store = MemoryStore::open(&store_path)?;
            memory::query_memory(&store, &query, None, &tags, limit)
        })
        .await
        .map_err(|e| ToolError::Failed(anyhow::Error::new(e).context("memory search task")))??;
        Ok(ToolOutput {
            summary: format!("{} memory result(s)", results.total_count()),
            data: json!({ "total": results.total_count(), "results": to_json(&results)? }),
        })
    }
}

fn clamp_limit(requested: Option<usize>, default: usize) -> usize {
    requested.unwrap_or(default).clamp(1, MAX_ROW_LIMIT)
}

/// Apply spec §10/R30's single "evidence-backed" rule to the JSON `verified`
/// field, exactly as `surge ready --json` / `surge ledger --json` do.
fn normalized(records: Vec<TaskLedgerIndexRecord>) -> Vec<TaskLedgerIndexRecord> {
    records
        .into_iter()
        .map(TaskLedgerIndexRecord::with_verified_normalized)
        .collect()
}

fn pending_to_json(pending: &PendingInput) -> Value {
    json!({
        "node": pending.node.to_string(),
        "prompt": pending.prompt.trim(),
        "kind": if pending.is_tool_call { "tool_call" } else { "gate" },
        "options": pending
            .gate_options
            .iter()
            .map(|(outcome, label)| json!({ "outcome": outcome, "label": label }))
            .collect::<Vec<_>>(),
        "bootstrap_gate": pending.is_bootstrap_gate,
    })
}

/// The MCP client's self-reported name from its `initialize` request, as
/// stated by the caller (not verified).
fn client_name(peer: &Peer<RoleServer>) -> String {
    peer.peer_info().map_or_else(
        || UNKNOWN_CLIENT.to_owned(),
        |info| info.client_info.name.clone(),
    )
}

#[tool_router]
impl SurgeMcpServer {
    #[tool(
        description = "Fleet inbox: every run grouped by what it needs from the operator \
                       (needs_input / working / waiting / done), blocked-first. Runs in needs_input \
                       carry the prompt they are blocked on.",
        annotations(read_only_hint = true)
    )]
    async fn surge_inbox(&self, Parameters(params): Parameters<InboxParams>) -> CallToolResult {
        into_call_result(self.inbox_impl(params).await)
    }

    #[tool(
        description = "Status of one run: inbox classification, registry status, and — when the run \
                       is blocked on a pipeline gate — the pending question with its valid decisions.",
        annotations(read_only_hint = true)
    )]
    async fn surge_run_status(
        &self,
        Parameters(params): Parameters<RunIdParams>,
    ) -> CallToolResult {
        into_call_result(self.run_status_impl(params).await)
    }

    #[tool(
        description = "Actionable task backlog from the cross-run task ledger (tasks not yet \
                       completed, failed or skipped).",
        annotations(read_only_hint = true)
    )]
    async fn surge_ready_tasks(
        &self,
        Parameters(params): Parameters<ReadyParams>,
    ) -> CallToolResult {
        into_call_result(self.ready_impl(params).await)
    }

    #[tool(
        description = "Full task ledger (settled tasks included) for one run, or for all runs, with \
                       evidence-backed verification status.",
        annotations(read_only_hint = true)
    )]
    async fn surge_ledger(&self, Parameters(params): Parameters<LedgerParams>) -> CallToolResult {
        into_call_result(self.ledger_impl(params).await)
    }

    #[tool(
        description = "The compiled Run Report for a run: nodes, outcomes, verifier verdicts, \
                       evidence, cost, skills, steers and approvals, rebuilt from the event log.",
        annotations(read_only_hint = true)
    )]
    async fn surge_run_report(
        &self,
        Parameters(params): Parameters<RunReportParams>,
    ) -> CallToolResult {
        into_call_result(self.run_report_impl(params).await)
    }

    #[tool(
        description = "Queue guidance for a live run. Delivered at the next stage boundary; does \
                       not interrupt the current agent. Requires the daemon and `--allow-write`.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn surge_steer(
        &self,
        peer: Peer<RoleServer>,
        Parameters(params): Parameters<SteerParams>,
    ) -> CallToolResult {
        into_call_result(self.steer_impl(&client_name(&peer), params).await)
    }

    #[tool(
        description = "Answer a run blocked on human input. Only runs listed in `surge_inbox` \
                       needs_input can be resolved, only with a decision the gate declares, and \
                       bootstrap approvals (description / roadmap / flow) are never accepted. \
                       Requires the daemon and `--allow-write`.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn surge_resolve(
        &self,
        peer: Peer<RoleServer>,
        Parameters(params): Parameters<ResolveParams>,
    ) -> CallToolResult {
        into_call_result(self.resolve_impl(&client_name(&peer), params).await)
    }

    #[tool(
        description = "Start the idea -> description -> roadmap -> flow bootstrap journey in the \
                       daemon. It halts at each human approval gate; this server never approves \
                       them. Requires the daemon and `--allow-write`.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn surge_bootstrap_start(
        &self,
        peer: Peer<RoleServer>,
        Parameters(params): Parameters<BootstrapStartParams>,
    ) -> CallToolResult {
        into_call_result(self.bootstrap_start_impl(&client_name(&peer), params).await)
    }

    #[tool(
        description = "Full-text search over Surge's project memory (discoveries, patterns, \
                       gotchas, file contexts).",
        annotations(read_only_hint = true)
    )]
    async fn surge_memory_search(
        &self,
        Parameters(params): Parameters<MemorySearchParams>,
    ) -> CallToolResult {
        into_call_result(self.memory_search_impl(params).await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SurgeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("surge", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Surge orchestrator. Start with surge_inbox to see what needs attention. Read \
                 tools are always available; steer, resolve and bootstrap_start need the server \
                 to run with --allow-write and a running daemon. Bootstrap approval gates are \
                 human-only.",
            )
    }
}

#[cfg(test)]
mod tests;
