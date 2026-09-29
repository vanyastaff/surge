//! `surge mcp serve` — Surge as an MCP *server* over stdio.
//!
//! Lets a "main agent" (Claude Code, Codex, any MCP client) drive Surge:
//! read the fleet inbox, inspect runs, steer them, answer the human gates the
//! operator surfaced, and start the idea → description → roadmap → flow
//! bootstrap journey. Every tool is a thin adapter over the same services the
//! matching CLI command uses — `surge_orchestrator::operator` (inbox, pending
//! input, run report/trace) and the `ready`, `ledger`, `steer` and `memory
//! search` command modules — no query or validation logic lives here.
//!
//! # Safety model
//!
//! - Read tools (`surge_inbox`, `surge_run_status`, `surge_ready_tasks`,
//!   `surge_ledger`, `surge_run_report`, `surge_memory_search`) are always
//!   available and never need the daemon.
//! - Mutating tools (`surge_steer`, `surge_resolve`, `surge_bootstrap_start`;
//!   the [`TOOLS`] table is the single list) all start with
//!   [`SurgeMcpServer::begin_mutation`]: it refuses the call unless the server
//!   was started with `--allow-write` and hands back an [`Audit`] whose
//!   `record` writes the audit line (stderr, target `surge::mcp_serve::audit`,
//!   with the client name announced in the MCP `initialize` request). A new
//!   mutating tool cannot forget either step without skipping that one call.
//! - `surge_resolve`'s whole policy lives in the pure [`authorize_resolution`]:
//!   the run must sit in the inbox's NEEDS INPUT group, a bootstrap-mode gate
//!   (description / roadmap / flow approval) is never answered, the caller must
//!   name the node it was shown, and the decision must be one the pending gate
//!   declares.
//! - stdout carries the MCP protocol only; diagnostics go to stderr.
//!
//! Failures are reported as MCP tool errors (`isError: true`), never panics.

use std::fmt::Display;
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
use surge_core::bootstrap_operation::BootstrapIntent;
use surge_core::{RoadmapStatus, RunId, RunState};
use surge_orchestrator::engine::daemon_facade::{BootstrapClientError, DaemonEngineFacade};
use surge_orchestrator::operator::{
    self, AttentionGroup, LedgerQuery, MemoryQuery, OperatorAnswer, OperatorError,
    OperatorErrorKind, PendingInput, PendingKind, ReadyQuery, classify, collect_entries,
    compile_report, compile_trace, deliver_answer, fold_run_state, inspect_pending, query_ledger,
    query_memory, query_ready, queue_steer,
};
use surge_persistence::memory::MemoryStore;
use surge_persistence::runs::Storage;
use surge_persistence::runs::registry::RunSummary;
use surge_persistence::task_ledger::TaskLedgerIndexRecord;
use tokio::sync::OnceCell;

use crate::commands::bootstrap;
use crate::commands::common::{connect_daemon_at, project_root};

/// Default row cap for the inbox's Done tail and the ready backlog.
const DEFAULT_ROW_LIMIT: usize = 200;
/// Default row cap for `surge_ledger` (matches `surge ledger`).
const DEFAULT_LEDGER_LIMIT: usize = 500;
/// Largest accepted caller-supplied row limit. Mirrored by the `range` in the
/// params' JSON schema (attributes take literals); a test keeps them equal.
const MAX_ROW_LIMIT: usize = 5000;
/// Default and largest accepted `surge_memory_search` results per category.
const DEFAULT_MEMORY_LIMIT: usize = 10;
const MAX_MEMORY_LIMIT: usize = 50;
/// How many rows a listing fetches before applying the caller's `limit`, so
/// `total` can be reported truthfully; beyond it `total` saturates.
const SCAN_CEILING: usize = 1_000_000;
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

/// One tool of this server and whether it mutates run state.
struct ToolSpec {
    name: &'static str,
    mutating: bool,
}

/// Every tool this server exposes. `mutating` drives the `--allow-write` guard
/// ([`SurgeMcpServer::begin_mutation`]); a test checks the table against the
/// registered router (names and `read_only_hint`), so a tool cannot be added
/// to one and not the other.
const TOOLS: [ToolSpec; 10] = [
    ToolSpec {
        name: "surge_inbox",
        mutating: false,
    },
    ToolSpec {
        name: "surge_run_status",
        mutating: false,
    },
    ToolSpec {
        name: "surge_ready_tasks",
        mutating: false,
    },
    ToolSpec {
        name: "surge_ledger",
        mutating: false,
    },
    ToolSpec {
        name: "surge_run_report",
        mutating: false,
    },
    ToolSpec {
        name: "surge_run_trace",
        mutating: false,
    },
    ToolSpec {
        name: "surge_steer",
        mutating: true,
    },
    ToolSpec {
        name: "surge_resolve",
        mutating: true,
    },
    ToolSpec {
        name: "surge_bootstrap_start",
        mutating: true,
    },
    ToolSpec {
        name: "surge_memory_search",
        mutating: false,
    },
];

/// A tool call's failure, reported to the client as an MCP tool error whose
/// structured `error.kind` is a stable, documented code (see `docs/mcp.md`).
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
    /// The run id is well formed but names no run.
    #[error("{0}")]
    RunNotFound(String),
    /// The run id is malformed, too short, or matches several runs.
    #[error("{0}")]
    InvalidRunId(String),
    /// An argument is blank or out of range.
    #[error("{0}")]
    InvalidArgument(String),
    /// The run is not blocked on an answer this server may give.
    #[error("{0}")]
    NotAwaitingInput(String),
    /// The pending gate is a bootstrap approval: a human decision.
    #[error("{0}")]
    HumanOnlyGate(String),
    /// The run is blocked at a different node than the caller was shown.
    #[error(
        "run is now blocked at @{current_node}, not @{expected_node}; re-read `surge_run_status`"
    )]
    StaleGate {
        expected_node: String,
        current_node: String,
    },
    /// The decision is not one the pending gate declares.
    #[error("{message}")]
    InvalidDecision {
        message: String,
        valid_decisions: Vec<String>,
    },
    /// The daemon (or the bootstrap supervisor) declined a well-formed request.
    #[error("{0}")]
    Rejected(String),
    /// An internal fault: storage, IO, serialization.
    #[error("{0:#}")]
    Failed(#[from] anyhow::Error),
}

impl From<OperatorError> for ToolError {
    fn from(error: OperatorError) -> Self {
        Self::Failed(anyhow::Error::new(error))
    }
}

impl ToolError {
    /// The stable machine-readable code.
    fn kind(&self) -> &'static str {
        match self {
            Self::WriteDisabled(_) => "write_disabled",
            Self::DaemonNotRunning(_) => "daemon_not_running",
            Self::RunNotFound(_) => "run_not_found",
            Self::InvalidRunId(_) => "invalid_run_id",
            Self::InvalidArgument(_) => "invalid_argument",
            Self::NotAwaitingInput(_) => "not_awaiting_input",
            Self::HumanOnlyGate(_) => "human_only_gate",
            Self::StaleGate { .. } => "stale_gate",
            Self::InvalidDecision { .. } => "invalid_decision",
            Self::Rejected(_) => "rejected",
            Self::Failed(_) => "failed",
        }
    }

    /// Machine-readable detail a caller can act on without parsing the message.
    fn data(&self) -> Option<Value> {
        match self {
            Self::StaleGate {
                expected_node,
                current_node,
            } => Some(json!({ "expected_node": expected_node, "current_node": current_node })),
            Self::InvalidDecision {
                valid_decisions, ..
            } => Some(json!({ "valid_decisions": valid_decisions })),
            _ => None,
        }
    }
}

/// A successful tool result: a short human summary plus structured JSON
/// (always a JSON object, as MCP requires of `structuredContent`). The
/// structured form is the machine-readable one; the summary is prose.
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
            let mut body = json!({ "kind": error.kind(), "message": message });
            if let Some(data) = error.data() {
                body["data"] = data;
            }
            let mut call = CallToolResult::error(vec![Content::text(message)]);
            call.structured_content = Some(json!({ "error": body }));
            call
        },
    }
}

/// Handle returned by [`SurgeMcpServer::begin_mutation`]; proof the write guard
/// passed. Consuming it with [`Audit::record`] writes the audit line, so the
/// two steps of a mutation stay together.
#[must_use = "call `record` right before the mutation is carried out"]
struct Audit {
    client: String,
    tool: &'static str,
}

impl Audit {
    /// Log the accepted mutation (stderr, target `surge::mcp_serve::audit`).
    /// `subject` is the run / operation acted on; `detail` must not carry
    /// sensitive text (log lengths, not content).
    fn record(self, subject: impl Display, detail: impl Display) {
        tracing::info!(
            target: "surge::mcp_serve::audit",
            client = ?self.client,
            tool = self.tool,
            %subject,
            %detail,
            "MCP mutation"
        );
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
    /// Maximum Done runs listed (1-5000, default 200); NEEDS INPUT / WORKING /
    /// WAITING runs are never truncated. Out-of-range values are rejected.
    #[serde(default)]
    #[schemars(range(min = 1, max = 5000))]
    pub limit: Option<i64>,
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
    /// Maximum rows (1-5000, default 200). Out-of-range values are rejected.
    #[serde(default)]
    #[schemars(range(min = 1, max = 5000))]
    pub limit: Option<i64>,
}

/// Arguments of `surge_ledger`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct LedgerParams {
    /// Scope to one run; omit for every project's ledger (per-project scoping
    /// is not available yet — same as `surge ledger`).
    #[serde(default)]
    pub run_id: Option<String>,
    /// Maximum rows (1-5000, default 500). Out-of-range values are rejected.
    #[serde(default)]
    #[schemars(range(min = 1, max = 5000))]
    pub limit: Option<i64>,
}

/// Text rendering `surge_run_report` returns. The full report is always in
/// `structuredContent`, whatever the format.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReportFormat {
    /// One-line prose summary as text (default).
    #[default]
    Summary,
    /// The full Markdown report as text.
    Markdown,
}

/// Arguments of `surge_run_report`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunReportParams {
    /// Run id (full ULID or unique suffix).
    pub run_id: String,
    /// Text rendering of the result. `structuredContent` always carries the
    /// full machine-readable report regardless.
    #[serde(default)]
    pub format: ReportFormat,
}

/// Arguments of `surge_run_trace`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunTraceParams {
    /// Run id (full ULID or unique suffix).
    pub run_id: String,
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
    /// What the run is blocked on decides what this is (`pending_input.kind`
    /// in `surge_run_status`): for `gate`, one of the outcome keys in
    /// `pending_input.options`; for `tool_call`, the free-form answer text.
    /// A `bootstrap_approval` is never accepted.
    pub decision: String,
    /// Optional operator comment. Only a `gate` decision carries one; it is
    /// rejected for a `tool_call` answer, where it would be ignored.
    #[serde(default)]
    pub note: Option<String>,
    /// The `pending_input.node` you were shown by `surge_run_status`. The call
    /// is refused (`stale_gate`) if the run is blocked at a different node now.
    pub expected_node: String,
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
    /// Maximum results per category (1-50, default 10). Out-of-range values
    /// are rejected.
    #[serde(default)]
    #[schemars(range(min = 1, max = 50))]
    pub limit: Option<i64>,
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

    /// The single entry to every mutating tool: refuse unless `--allow-write`,
    /// then hand back the [`Audit`] that must record the mutation. Every tool
    /// with `mutating: true` in [`TOOLS`] starts here.
    fn begin_mutation(&self, tool: &'static str, client: &str) -> Result<Audit, ToolError> {
        debug_assert!(
            TOOLS.iter().any(|spec| spec.name == tool && spec.mutating),
            "`{tool}` is not registered as mutating in TOOLS"
        );
        if !self.inner.options.allow_write {
            return Err(ToolError::WriteDisabled(tool));
        }
        Ok(Audit {
            client: client.to_owned(),
            tool,
        })
    }

    /// Resolve `value` (full ULID or unique suffix) and load the run's registry
    /// row, telling a malformed/ambiguous id from a missing run from a fault.
    async fn load_run(&self, value: &str) -> Result<RunSummary, ToolError> {
        let storage = self.storage().await?;
        let run_id = operator::resolve_run_id(storage, value)
            .await
            .map_err(classify_run_id_error)?;
        match storage.get_run(&run_id).await {
            Ok(Some(summary)) => Ok(summary),
            Ok(None) => Err(ToolError::RunNotFound(format!("no run {run_id}"))),
            Err(e) => Err(ToolError::Failed(
                anyhow::Error::new(e).context("read run registry"),
            )),
        }
    }

    async fn existing_run(&self, value: &str) -> Result<RunId, ToolError> {
        Ok(self.load_run(value).await?.id)
    }

    /// What the run is blocked on. [`PendingState::Nothing`] unless the run is
    /// in the inbox's NEEDS INPUT group; real read failures are propagated, not
    /// reported as a bootstrap approval.
    async fn load_pending(
        &self,
        run_id: RunId,
        attention: AttentionGroup,
    ) -> Result<PendingState, ToolError> {
        if attention != AttentionGroup::NeedsInput {
            return Ok(PendingState::Nothing);
        }
        let storage = self.storage().await?;
        let inspect_error = match inspect_pending(storage, run_id).await {
            Ok(pending) => return Ok(PendingState::Input(pending)),
            Err(e) => e,
        };
        // `inspect_pending` fails both for a run blocked outside a pipeline
        // gate and for a genuine read failure; re-fold to tell them apart.
        let reader = storage
            .open_run_reader(run_id)
            .await
            .map_err(|e| ToolError::Failed(anyhow::Error::new(e).context("open run")))?;
        match fold_run_state(&reader, run_id).await? {
            RunState::Bootstrapping { .. } => Ok(PendingState::BootstrapApproval),
            _ => Err(ToolError::from(inspect_error)),
        }
    }

    async fn inbox_impl(&self, params: InboxParams) -> ToolResult {
        let limit = validated_limit(params.limit, DEFAULT_ROW_LIMIT, MAX_ROW_LIMIT)?;
        let storage = self.storage().await?;
        // Every run is classified anyway; cap the Done listing here so its
        // `total` is real.
        let entries = collect_entries(storage, None, usize::MAX).await?;

        let mut needs_input = Vec::new();
        let mut working = Vec::new();
        let mut waiting = Vec::new();
        let mut done = Vec::new();
        let mut done_total = 0_usize;
        for entry in &entries {
            match entry.attention {
                AttentionGroup::NeedsInput => needs_input.push(to_json(entry)?),
                AttentionGroup::Working => working.push(to_json(entry)?),
                AttentionGroup::Waiting => waiting.push(to_json(entry)?),
                AttentionGroup::Done => {
                    done_total += 1;
                    if params.include_done && done.len() < limit {
                        done.push(to_json(entry)?);
                    }
                },
            }
        }
        let summary = format!(
            "{} need input, {} working, {} waiting, {done_total} done",
            needs_input.len(),
            working.len(),
            waiting.len(),
        );
        let mut done_group = json!({
            "count": done.len(),
            "total": done_total,
            "truncated": params.include_done && done_total > done.len(),
        });
        if params.include_done {
            done_group["runs"] = Value::Array(done);
        }
        Ok(ToolOutput {
            summary,
            data: json!({
                "needs_input": needs_input,
                "working": working,
                "waiting": waiting,
                "done": done_group,
            }),
        })
    }

    async fn run_status_impl(&self, params: RunIdParams) -> ToolResult {
        let summary = self.load_run(&params.run_id).await?;
        let run_id = summary.id;
        let storage = self.storage().await?;
        let entry = classify(storage, &summary).await?;
        let pending = self.load_pending(run_id, entry.attention).await?;
        Ok(ToolOutput {
            summary: format!(
                "run {run_id}: {} ({})",
                entry.attention.as_str(),
                summary.status.as_str()
            ),
            data: json!({
                "run": to_json(&entry)?,
                "registry_status": to_json(&summary.status)?,
                "pending_input": pending_input_json(&pending),
            }),
        })
    }

    async fn ready_impl(&self, params: ReadyParams) -> ToolResult {
        let limit = validated_limit(params.limit, DEFAULT_ROW_LIMIT, MAX_ROW_LIMIT)?;
        let storage = self.storage().await?;
        let run_id = self.optional_run_id(params.run_id.as_deref()).await?;
        let status = params
            .status
            .as_deref()
            .map(str::parse::<RoadmapStatus>)
            .transpose()
            .map_err(|e| ToolError::InvalidArgument(format!("invalid status: {e}")))?;
        let query = ReadyQuery {
            status,
            discovered_only: params.discovered,
            run_id,
            limit: SCAN_CEILING,
        };
        let mut tasks = normalized(query_ready(storage, &query)?);
        let total = tasks.len();
        tasks.truncate(limit);
        Ok(ToolOutput {
            summary: format!("{} of {total} actionable task(s)", tasks.len()),
            data: json!({
                "count": tasks.len(),
                "total": total,
                "truncated": total > tasks.len(),
                "tasks": to_json(&tasks)?,
            }),
        })
    }

    async fn ledger_impl(&self, params: LedgerParams) -> ToolResult {
        let limit = validated_limit(params.limit, DEFAULT_LEDGER_LIMIT, MAX_ROW_LIMIT)?;
        let storage = self.storage().await?;
        let run_id = self.optional_run_id(params.run_id.as_deref()).await?;
        let query = LedgerQuery {
            run_id,
            limit: SCAN_CEILING,
        };
        let mut tasks = normalized(query_ledger(storage, &query)?);
        let total = tasks.len();
        tasks.truncate(limit);
        let verified = tasks.iter().filter(|t| t.is_evidence_backed()).count();
        Ok(ToolOutput {
            summary: format!("{} of {total} task(s), {verified} verified", tasks.len()),
            data: json!({
                "count": tasks.len(),
                "total": total,
                "truncated": total > tasks.len(),
                "verified": verified,
                "tasks": to_json(&tasks)?,
            }),
        })
    }

    /// Resolve an optional run reference to the full-ULID string the ledger
    /// queries expect.
    async fn optional_run_id(&self, value: Option<&str>) -> Result<Option<RunId>, ToolError> {
        match value {
            Some(value) => Ok(Some(self.existing_run(value).await?)),
            None => Ok(None),
        }
    }

    async fn run_report_impl(&self, params: RunReportParams) -> ToolResult {
        let run_id = self.existing_run(&params.run_id).await?;
        let storage = self.storage().await?;
        let report = compile_report(storage, &run_id.to_string()).await?;
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
            ReportFormat::Summary => one_line,
            ReportFormat::Markdown => surge_core::run_report::render_markdown(&report),
        };
        Ok(ToolOutput {
            summary,
            data: json!({ "report": to_json(&report)? }),
        })
    }

    async fn run_trace_impl(&self, params: RunTraceParams) -> ToolResult {
        let run_id = self.existing_run(&params.run_id).await?;
        let storage = self.storage().await?;
        let rendered = compile_trace(storage, &run_id.to_string()).await?;
        let trace: Value = serde_json::from_str(&rendered)
            .map_err(|e| ToolError::Failed(anyhow::Error::new(e).context("parse trace JSON")))?;
        let spans = trace
            .pointer("/resourceSpans/0/scopeSpans/0/spans")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        Ok(ToolOutput {
            summary: format!("run {run_id}: OTLP trace with {spans} span(s)"),
            data: json!({ "trace": trace }),
        })
    }

    async fn steer_impl(&self, client: &str, params: SteerParams) -> ToolResult {
        let audit = self.begin_mutation("surge_steer", client)?;
        let run_id = self.existing_run(&params.run_id).await?;
        if params.message.trim().is_empty() {
            return Err(ToolError::InvalidArgument(
                "steer message must not be blank".into(),
            ));
        }
        let daemon = self.daemon().await?;
        // The message itself is not logged: it may carry sensitive guidance.
        audit.record(
            run_id,
            format_args!("queueing steer, message_len={}", params.message.len()),
        );
        let steer_id = queue_steer(&daemon, run_id, &params.message)
            .await
            .map_err(daemon_rejection)?;
        Ok(ToolOutput {
            summary: format!("steer {steer_id} queued for run {run_id}; applies at the next stage"),
            data: json!({ "run_id": run_id.to_string(), "steer_id": steer_id }),
        })
    }

    async fn resolve_impl(&self, client: &str, params: ResolveParams) -> ToolResult {
        let audit = self.begin_mutation("surge_resolve", client)?;
        let summary = self.load_run(&params.run_id).await?;
        let run_id = summary.id;
        let storage = self.storage().await?;
        // The very classifier `surge inbox` uses decides whether the run was
        // surfaced to an operator at all.
        let attention = classify(storage, &summary).await?.attention;
        let pending = self.load_pending(run_id, attention).await?;

        let authorized = authorize_resolution(
            attention,
            &pending,
            &ResolveRequest {
                expected_node: &params.expected_node,
                decision: &params.decision,
                note: params.note.as_deref(),
            },
        )?;
        let answer = match &authorized.answer {
            Answer::FreeForm(text) => OperatorAnswer::Text(text.clone()),
            Answer::Outcome(outcome) => OperatorAnswer::Outcome {
                key: outcome.clone(),
                comment: authorized.note.map(ToOwned::to_owned),
            },
        };
        let validated = authorized.pending.build_answer(answer).map_err(|e| {
            ToolError::Failed(
                anyhow::Error::new(e).context("build answer for an authorized resolution"),
            )
        })?;

        let daemon = self.daemon().await?;
        let node = authorized.pending.node.to_string();
        audit.record(
            run_id,
            format_args!(
                "resolving @{node} with {}",
                match &authorized.answer {
                    Answer::Outcome(outcome) => outcome.as_str(),
                    Answer::FreeForm(_) => "<free-form answer>",
                }
            ),
        );
        deliver_answer(&daemon, run_id, validated)
            .await
            .map_err(daemon_rejection)?;
        Ok(ToolOutput {
            summary: format!("resolved run {run_id} at @{node}"),
            data: json!({
                "run_id": run_id.to_string(),
                "node": node,
                "decision": authorized.answer.to_json(),
            }),
        })
    }

    async fn bootstrap_start_impl(&self, client: &str, params: BootstrapStartParams) -> ToolResult {
        let audit = self.begin_mutation("surge_bootstrap_start", client)?;
        let idea = params.idea.trim();
        if idea.is_empty() {
            return Err(ToolError::InvalidArgument("idea must not be blank".into()));
        }
        let daemon = self.daemon().await?;
        let project_root = self.inner.options.project_root.clone();
        let config = bootstrap::load_project_config_at(&project_root)?;
        let intent = BootstrapIntent::new(
            project_root.clone(),
            idea.to_owned(),
            config.analytics.budget_guard(),
        )
        .map_err(|e| ToolError::InvalidArgument(e.to_string()))?;

        let operation_id = RunId::new();
        audit.record(
            operation_id,
            format_args!(
                "starting bootstrap, project={}, idea_len={}",
                project_root.display(),
                idea.len()
            ),
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
            }),
        })
    }

    async fn memory_search_impl(&self, params: MemorySearchParams) -> ToolResult {
        let limit = validated_limit(params.limit, DEFAULT_MEMORY_LIMIT, MAX_MEMORY_LIMIT)?;
        let query = params.query.trim().to_owned();
        if query.is_empty() {
            return Err(ToolError::InvalidArgument("query must not be blank".into()));
        }
        let store_path = MemoryStore::path_in(&self.inner.options.home);
        if !store_path.exists() {
            return Ok(ToolOutput {
                summary: "no project memory recorded yet".into(),
                data: json!({
                    "count": 0,
                    "total": 0,
                    "truncated": false,
                    "results": { "discoveries": [], "patterns": [], "gotchas": [], "file_contexts": [] },
                }),
            });
        }
        let memory_query = MemoryQuery {
            text: query,
            spec_id: None,
            tags: params.tags,
            limit: SCAN_CEILING,
        };
        // rusqlite is synchronous; keep it off the async workers. Fetch past
        // the caller's limit so `total` is real and the tag filter sees every
        // hit, then cut each category down to `limit`.
        let mut results =
            tokio::task::spawn_blocking(move || query_memory(&store_path, &memory_query))
                .await
                .map_err(|e| {
                    ToolError::Failed(anyhow::Error::new(e).context("memory search task"))
                })??;
        let total = results.total_count();
        results.discoveries.truncate(limit);
        results.patterns.truncate(limit);
        results.gotchas.truncate(limit);
        results.file_contexts.truncate(limit);
        let count = results.total_count();
        Ok(ToolOutput {
            summary: format!("{count} of {total} memory result(s)"),
            data: json!({
                "count": count,
                "total": total,
                "truncated": total > count,
                "results": to_json(&results)?,
            }),
        })
    }
}

/// Validate a caller-supplied row limit: `1..=max`, else `invalid_argument`.
/// Never clamps silently.
fn validated_limit(requested: Option<i64>, default: usize, max: usize) -> Result<usize, ToolError> {
    let Some(requested) = requested else {
        return Ok(default);
    };
    usize::try_from(requested)
        .ok()
        .filter(|limit| (1..=max).contains(limit))
        .ok_or_else(|| {
            ToolError::InvalidArgument(format!(
                "limit must be between 1 and {max}, got {requested}"
            ))
        })
}

/// A daemon that declined a steer or an answer, with the full cause chain and
/// what an MCP client can do about it.
fn daemon_rejection(error: OperatorError) -> ToolError {
    ToolError::Rejected(format!(
        "{:#}. The run must be active in a running daemon.",
        anyhow::Error::new(error)
    ))
}

/// Map `resolve_run_id`'s failure onto the stable codes: an id the caller got
/// wrong is `run_not_found` or `invalid_run_id`, anything else is a fault.
fn classify_run_id_error(error: OperatorError) -> ToolError {
    match error.kind() {
        OperatorErrorKind::NotFound => ToolError::RunNotFound(error.to_string()),
        OperatorErrorKind::InvalidInput => ToolError::InvalidRunId(error.to_string()),
        OperatorErrorKind::NotAwaitingInput | OperatorErrorKind::Fault => ToolError::from(error),
    }
}

/// Apply spec §10/R30's single "evidence-backed" rule to the JSON `verified`
/// field, exactly as `surge ready --json` / `surge ledger --json` do.
fn normalized(records: Vec<TaskLedgerIndexRecord>) -> Vec<TaskLedgerIndexRecord> {
    records
        .into_iter()
        .map(TaskLedgerIndexRecord::with_verified_normalized)
        .collect()
}

/// What a run is blocked on, as far as this server may act on it.
#[derive(Debug)]
enum PendingState {
    /// The run is not in the inbox's NEEDS INPUT group.
    Nothing,
    /// A bootstrap-stage approval outside any pipeline gate (no node to name).
    BootstrapApproval,
    /// A pipeline `HumanGate` or tool-driven question.
    Input(PendingInput),
}

/// `surge_run_status`'s `pending_input`: null when the run is not blocked,
/// else a value tagged by `kind`.
fn pending_input_json(state: &PendingState) -> Value {
    match state {
        PendingState::Nothing => Value::Null,
        PendingState::BootstrapApproval => json!({ "kind": "bootstrap_approval" }),
        PendingState::Input(pending) => {
            let node = pending.node.to_string();
            let prompt = pending.prompt.trim();
            match &pending.kind {
                PendingKind::BootstrapGate => json!({
                    "kind": "bootstrap_approval",
                    "node": node,
                    "prompt": prompt,
                }),
                PendingKind::ToolCall { .. } => json!({
                    "kind": "tool_call",
                    "node": node,
                    "prompt": prompt,
                }),
                PendingKind::Gate { options, .. } => json!({
                    "kind": "gate",
                    "node": node,
                    "prompt": prompt,
                    "options": options
                        .iter()
                        .map(|o| json!({ "outcome": o.outcome, "label": o.label }))
                        .collect::<Vec<_>>(),
                }),
            }
        },
    }
}

/// What the caller of `surge_resolve` asks for.
struct ResolveRequest<'a> {
    expected_node: &'a str,
    decision: &'a str,
    note: Option<&'a str>,
}

/// The accepted answer.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// A `gate` outcome key.
    Outcome(String),
    /// A `tool_call` free-form answer.
    FreeForm(String),
}

impl Answer {
    fn to_json(&self) -> Value {
        match self {
            Self::Outcome(outcome) => json!({ "kind": "outcome", "value": outcome }),
            Self::FreeForm(text) => json!({ "kind": "free_form", "value": text }),
        }
    }
}

/// A resolution that passed every gate of [`authorize_resolution`].
#[derive(Debug)]
struct Authorized<'a> {
    pending: &'a PendingInput,
    answer: Answer,
    /// The operator comment (gate decisions only).
    note: Option<&'a str>,
}

/// The whole safety policy of `surge_resolve`, pure and decided in this order:
///
/// 1. the run must be in the inbox's NEEDS INPUT group (`not_awaiting_input`);
/// 2. a bootstrap approval is a human decision (`human_only_gate`);
/// 3. the caller must be answering the node it was shown (`stale_gate`);
/// 4. the decision must be one the pending request accepts
///    (`invalid_decision` / `invalid_argument`).
fn authorize_resolution<'a>(
    attention: AttentionGroup,
    pending: &'a PendingState,
    request: &ResolveRequest<'a>,
) -> Result<Authorized<'a>, ToolError> {
    if attention != AttentionGroup::NeedsInput {
        return Err(ToolError::NotAwaitingInput(format!(
            "run is not in the inbox's needs_input group (it is {}); only a request surfaced by \
             `surge_inbox` can be resolved",
            attention.as_str()
        )));
    }
    let human_only = |node: Option<&str>| {
        ToolError::HumanOnlyGate(format!(
            "run is blocked at a bootstrap approval{}; description, roadmap and flow approvals \
             must be given by a human in the desktop app, Telegram, or `surge bootstrap`",
            node.map_or_else(String::new, |node| format!(" (@{node})"))
        ))
    };
    let pending = match pending {
        PendingState::Nothing => {
            return Err(ToolError::Failed(anyhow::anyhow!(
                "run is in needs_input but no pending request could be found"
            )));
        },
        PendingState::BootstrapApproval => return Err(human_only(None)),
        PendingState::Input(pending) => pending,
    };
    if matches!(pending.kind, PendingKind::BootstrapGate) {
        return Err(human_only(Some(pending.node.as_ref())));
    }
    if request.expected_node != pending.node.as_ref() as &str {
        return Err(ToolError::StaleGate {
            expected_node: request.expected_node.to_owned(),
            current_node: pending.node.to_string(),
        });
    }

    let decision = request.decision.trim();
    if decision.is_empty() {
        return Err(ToolError::InvalidArgument(
            "decision must not be blank".into(),
        ));
    }
    let options = match &pending.kind {
        PendingKind::Gate { options, .. } => options.as_slice(),
        PendingKind::ToolCall { .. } | PendingKind::BootstrapGate => &[],
    };
    if matches!(pending.kind, PendingKind::ToolCall { .. }) {
        if request.note.is_some() {
            return Err(ToolError::InvalidArgument(
                "`note` is only accepted for a gate decision; a tool_call answer is the \
                 `decision` text alone"
                    .into(),
            ));
        }
        return Ok(Authorized {
            pending,
            answer: Answer::FreeForm(decision.to_owned()),
            note: None,
        });
    }
    let valid_decisions: Vec<String> = options.iter().map(|o| o.outcome.clone()).collect();
    if valid_decisions.is_empty() {
        return Err(ToolError::InvalidDecision {
            message: format!(
                "the gate at @{} declares no outcome options, so a decision cannot be validated",
                pending.node
            ),
            valid_decisions,
        });
    }
    if !valid_decisions.iter().any(|valid| valid == decision) {
        return Err(ToolError::InvalidDecision {
            message: format!(
                "decision {decision:?} is not valid for the gate at @{}; valid: {}",
                pending.node,
                valid_decisions.join(", ")
            ),
            valid_decisions,
        });
    }
    Ok(Authorized {
        pending,
        answer: Answer::Outcome(decision.to_owned()),
        note: request.note,
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
        description = "The run as an OpenTelemetry trace in OTLP/JSON (structuredContent.trace): a \
                       surge.run span with a child span per stage attempt and span events for \
                       outcomes, hook rejections, verified tasks and tool calls. Post it to any \
                       collector's /v1/traces.",
        annotations(read_only_hint = true)
    )]
    async fn surge_run_trace(
        &self,
        Parameters(params): Parameters<RunTraceParams>,
    ) -> CallToolResult {
        into_call_result(self.run_trace_impl(params).await)
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
