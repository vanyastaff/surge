//! Engine-wide registry of MCP server connections.

use crate::connection::McpServerConnection;
use crate::error::McpError;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use surge_core::mcp_config::McpServerRef;

/// Single-server tool listing entry, returned by [`McpRegistry::list_all_tools`].
#[non_exhaustive]
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

/// One selected server's catalog outcome, returned by
/// [`McpRegistry::list_tools_per_server`].
#[non_exhaustive]
#[derive(Debug)]
pub struct McpServerCatalog {
    /// Configured server name.
    pub server: String,
    /// The server's tools, or why its catalog could not be built
    /// (startup timeout, `tools/list` timeout, transport failure, …).
    pub tools: Result<Vec<McpToolEntry>, McpError>,
}

/// Result of a single MCP call, surge-flavoured (decoupled from
/// rmcp's exact types so callers don't need to depend on rmcp).
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct McpToolResult {
    /// Parsed content blocks returned by the server.
    pub content: Vec<McpContent>,
    /// Whether the server flagged this result as an error.
    pub is_error: bool,
}

/// One content block in a [`McpToolResult`]. M7 supports `Text`
/// directly; non-text content (image, resource, etc.) is summarised
/// as `Other`.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum McpContent {
    /// Plain text content.
    Text(String),
    /// Non-text content type — agents see a stub summary.
    Other {
        /// Raw discriminant of the content variant (e.g., "Image").
        kind: String,
        /// Debug-style summary of the original content.
        summary: String,
    },
}

impl McpToolEntry {
    /// Construct an entry. Prefer this over a struct literal so the
    /// type can grow new fields with `#[non_exhaustive]` without
    /// breaking external constructors.
    #[must_use]
    pub fn new(
        server: String,
        tool: String,
        description: Option<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            server,
            tool,
            description,
            input_schema,
        }
    }
}

#[derive(Clone, Copy)]
enum MonitorPolicy {
    LegacyProactive,
    OnDemand,
}

/// Engine-wide registry of MCP server connections. Holds one
/// [`McpServerConnection`] per configured server. Connections are
/// constructed in `Disconnected` state — first use of each server
/// triggers the spawn.
pub struct McpRegistry {
    servers: HashMap<String, Arc<McpServerConnection>>,
    /// Cancelled by [`shutdown`](Self::shutdown). The U11 per-connection
    /// health monitors bind to a clone of this so they stop when the
    /// run terminates — the seam exists from U3 so the monitor task is
    /// never born without a cancellation source.
    cancel_token: tokio_util::sync::CancellationToken,
    /// Guards one-time lazy spawn of the U11 health monitors (started on
    /// first async use, when a Tokio runtime is guaranteed present —
    /// `from_config` is sync and may be called outside a runtime).
    monitors_started: AtomicBool,
    monitor_policy: MonitorPolicy,
}

impl McpRegistry {
    /// Immutable server contracts for creating an independent run-owned registry.
    #[must_use]
    pub fn configured_servers(&self) -> Vec<McpServerRef> {
        let mut configs: Vec<_> = self
            .servers
            .values()
            .map(|server| server.configuration())
            .collect();
        configs.sort_by(|left, right| left.name.cmp(&right.name));
        configs
    }
    /// Build a registry from a slice of [`McpServerRef`]. Connections
    /// are not eagerly opened — first use of each server triggers
    /// the spawn via [`McpServerConnection::list_tools`] /
    /// [`McpServerConnection::call_tool`].
    ///
    /// `cwd` pins every child process's working directory and roots its
    /// captured-stderr file. Run-scoped callers pass the run worktree;
    /// daemon diagnostic probes pass `None`.
    #[must_use]
    pub fn from_config(refs: &[McpServerRef], cwd: Option<&Path>) -> Self {
        let mut servers = HashMap::new();
        for r in refs {
            servers.insert(
                r.name.clone(),
                Arc::new(McpServerConnection::new(
                    r.clone(),
                    cwd.map(Path::to_path_buf),
                )),
            );
        }
        Self {
            servers,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            monitors_started: AtomicBool::new(false),
            monitor_policy: MonitorPolicy::LegacyProactive,
        }
    }

    /// Build lazy run-owned connections whose launches are durably observed.
    #[must_use]
    pub fn from_config_owned(
        refs: &[McpServerRef],
        cwd: Option<&Path>,
        observer: &Arc<dyn crate::writer_observer::HostWriterObserver>,
    ) -> Self {
        Self::owned_registry(refs, cwd, observer, MonitorPolicy::LegacyProactive)
    }

    /// Build durably observed run-owned connections without background probes.
    /// Each catalog or tool operation reconnects only on explicit demand.
    #[must_use]
    pub fn from_config_owned_on_demand(
        refs: &[McpServerRef],
        cwd: Option<&Path>,
        observer: &Arc<dyn crate::writer_observer::HostWriterObserver>,
    ) -> Self {
        Self::owned_registry(refs, cwd, observer, MonitorPolicy::OnDemand)
    }

    fn owned_registry(
        refs: &[McpServerRef],
        cwd: Option<&Path>,
        observer: &Arc<dyn crate::writer_observer::HostWriterObserver>,
        monitor_policy: MonitorPolicy,
    ) -> Self {
        let servers = refs
            .iter()
            .map(|config| {
                (
                    config.name.clone(),
                    Arc::new(McpServerConnection::new_owned(
                        config.clone(),
                        cwd.map(Path::to_path_buf),
                        observer.clone(),
                    )),
                )
            })
            .collect();
        Self {
            servers,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            monitors_started: AtomicBool::new(false),
            monitor_policy,
        }
    }

    /// Lazily spawn the U11 health monitors exactly once, bound to the
    /// registry cancellation token (the U3 seam). Called from the async
    /// entry points (`list_all_tools` / `call_tool`) so a Tokio runtime
    /// is guaranteed present; idempotent.
    fn ensure_monitors_started(&self) {
        if matches!(self.monitor_policy, MonitorPolicy::OnDemand) {
            return;
        }
        if self.monitors_started.swap(true, Ordering::AcqRel) {
            return;
        }
        for conn in self.servers.values() {
            // Fire-and-forget by design: the task is cancelled via the
            // registry CancellationToken on shutdown, not by holding
            // the handle (a `let _ =` here would trip
            // clippy::let_underscore_future on the JoinHandle).
            conn.spawn_health_monitor(self.cancel_token.clone());
        }
    }

    /// A clone of the registry-lifetime cancellation token. U11's
    /// health monitors `select!` on this so they exit when
    /// [`shutdown`](Self::shutdown) is called.
    #[must_use]
    pub fn cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancel_token.clone()
    }

    /// Per-server health snapshot, sorted by server name for
    /// deterministic output (`surge mcp list`, daemon status).
    pub async fn statuses(&self) -> Vec<(String, crate::McpHealth)> {
        let mut names: Vec<&String> = self.servers.keys().collect();
        names.sort();
        let mut out = Vec::with_capacity(names.len());
        for n in names {
            let conn = self.servers.get(n).expect("just collected from this map");
            out.push((n.clone(), conn.status().await));
        }
        out
    }

    /// Deterministically tear down every connection and cancel the
    /// health-monitor token. Called by the engine on run terminal
    /// outcome (before the `Arc<McpRegistry>` drops). Every connection is
    /// bounded and every unconfirmed service result is returned to the host.
    /// Host process/effect disappearance requires separate writer evidence.
    pub async fn shutdown(&self) -> Result<(), crate::cleanup::RegistryCleanupError> {
        // Stop U11 health monitors first so they don't race a reconnect
        // against the teardown.
        self.cancel_token.cancel();
        let per_conn = Duration::from_secs(5);
        let mut handles = Vec::with_capacity(self.servers.len());
        for conn in self.servers.values() {
            let c = conn.clone();
            handles.push(tokio::spawn(async move {
                match tokio::time::timeout(per_conn, c.shutdown()).await {
                    Ok(result) => result,
                    Err(_) => Err(crate::cleanup::CleanupError::Timeout {
                        server: c.name().to_owned(),
                    }),
                }
            }));
        }
        let mut failures = Vec::new();
        for h in handles {
            match h.await {
                Ok(Ok(())) => {},
                Ok(Err(error)) => failures.push(error),
                Err(_error) => failures.push(crate::cleanup::CleanupError::WorkerJoin {
                    reason: "mcp_cleanup_worker_join_failed".into(),
                }),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(crate::cleanup::RegistryCleanupError { failures })
        }
    }

    /// Combined `tools/list` across all configured servers. Used by
    /// `RoutingToolDispatcher` at session-open to assemble the
    /// agent's tool catalog.
    ///
    /// Servers are queried in sorted order by name, and the final list
    /// is additionally sorted by `(server, tool)` to guarantee
    /// deterministic output regardless of `HashMap` iteration order or
    /// per-server `tools/list` ordering.
    pub async fn list_all_tools(&self) -> Result<Vec<McpToolEntry>, McpError> {
        self.ensure_monitors_started();
        let mut server_names: Vec<String> = self.servers.keys().cloned().collect();
        server_names.sort();
        self.query_tools(server_names).await
    }

    /// Query only configured selected servers without starting health monitors.
    /// All names are validated before any query; duplicates are queried once.
    /// All-or-nothing: the first failing server fails the whole listing (see
    /// [`Self::list_tools_per_server`] for per-server outcomes).
    pub async fn list_tools_for_servers(
        &self,
        names: &[String],
    ) -> Result<Vec<McpToolEntry>, McpError> {
        let selected = self.selected_servers(names)?;
        self.query_tools(selected).await
    }

    /// Query configured selected servers and report each server's catalog
    /// outcome separately, so one failing server neither hides the others'
    /// tools nor loses its own identity. Does not start health monitors.
    ///
    /// Outcomes are in sorted server order, duplicates queried once; each
    /// server's tools are sorted by name.
    ///
    /// # Errors
    /// [`McpError::ServerNotConfigured`] when any name is not configured;
    /// names are validated before any server is queried.
    pub async fn list_tools_per_server(
        &self,
        names: &[String],
    ) -> Result<Vec<McpServerCatalog>, McpError> {
        let selected = self.selected_servers(names)?;
        let mut out = Vec::with_capacity(selected.len());
        for server in selected {
            let tools = self.server_tools(&server).await.map(|mut tools| {
                tools.sort_by(|a, b| a.tool.cmp(&b.tool));
                tools
            });
            out.push(McpServerCatalog { server, tools });
        }
        Ok(out)
    }

    fn selected_servers(&self, names: &[String]) -> Result<Vec<String>, McpError> {
        let mut selected = names.to_vec();
        selected.sort();
        selected.dedup();
        for name in &selected {
            if !self.servers.contains_key(name) {
                return Err(McpError::ServerNotConfigured(name.clone()));
            }
        }
        Ok(selected)
    }

    async fn query_tools(&self, server_names: Vec<String>) -> Result<Vec<McpToolEntry>, McpError> {
        let mut out = Vec::new();
        for name in server_names {
            out.extend(self.server_tools(&name).await?);
        }
        // Final sort by (server, tool) to be doubly safe — server-side
        // tools/list ordering is implementation-defined.
        out.sort_by(|a, b| a.server.cmp(&b.server).then_with(|| a.tool.cmp(&b.tool)));
        Ok(out)
    }

    async fn server_tools(&self, name: &str) -> Result<Vec<McpToolEntry>, McpError> {
        let conn = self
            .servers
            .get(name)
            .ok_or_else(|| McpError::ServerNotConfigured(name.to_owned()))?;
        let tools = conn.list_tools().await?;
        Ok(tools
            .into_iter()
            .map(|t| {
                // Call `schema_as_json_value()` first (borrows `t`) before
                // moving any fields out of `t`. Then extract owned fields.
                // `schema_as_json_value()` returns
                // `Value::Object(self.input_schema.as_ref().clone())`.
                let input_schema = t.schema_as_json_value();
                McpToolEntry {
                    server: name.to_owned(),
                    tool: t.name.to_string(),
                    description: t.description.map(|c| c.to_string()),
                    input_schema,
                }
            })
            .collect())
    }

    /// Call a tool on a specific server.
    ///
    /// `timeout` bounds the RPC: the effective bound is
    /// `min(timeout, server_config_timeout)` and expiry returns
    /// [`McpError::Timeout`]. A lazy (re)connect before the RPC is bounded
    /// by the server's startup deadline instead (see
    /// [`McpServerConnection::call_tool_within`]).
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> Result<McpToolResult, McpError> {
        self.ensure_monitors_started();
        let conn = self
            .servers
            .get(server)
            .ok_or_else(|| McpError::ServerNotConfigured(server.into()))?;
        let r = conn.call_tool_within(tool, arguments, timeout).await?;
        let r = crate::connection::opaque_error_result(r);
        // `r.content: Vec<Content>` where `Content = Annotated<RawContent>`.
        // `Annotated<T>` exposes the inner value as `pub raw: T`.
        // `RawContent::Text(t)` carries a `RawTextContent` with field `t.text: String`.
        let content = r
            .content
            .into_iter()
            .map(|annotated| match annotated.raw {
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
    use std::collections::HashMap as Map;
    use std::path::PathBuf;

    #[test]
    fn empty_registry_is_empty() {
        let r = McpRegistry::from_config(&[], None);
        assert!(r.servers.is_empty());
    }

    #[test]
    fn registry_holds_named_connection() {
        let refs = vec![McpServerRef::new(
            "echo".into(),
            surge_core::mcp_config::McpTransportConfig::stdio(
                PathBuf::from("nope"),
                vec![],
                Map::new(),
            ),
            None,
            Duration::from_secs(60),
            true,
        )];
        let r = McpRegistry::from_config(&refs, None);
        assert_eq!(r.servers.len(), 1);
        assert!(r.servers.contains_key("echo"));
    }
}
