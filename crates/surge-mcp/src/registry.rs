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
}

impl McpRegistry {
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
        }
    }

    /// Lazily spawn the U11 health monitors exactly once, bound to the
    /// registry cancellation token (the U3 seam). Called from the async
    /// entry points (`list_all_tools` / `call_tool`) so a Tokio runtime
    /// is guaranteed present; idempotent.
    fn ensure_monitors_started(&self) {
        if self
            .monitors_started
            .swap(true, Ordering::AcqRel)
        {
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
    /// outcome (before the `Arc<McpRegistry>` drops) so no MCP child is
    /// orphaned. Each connection's teardown is time-bounded so a hung
    /// child cannot wedge the registry; idempotent.
    pub async fn shutdown(&self) {
        // Stop U11 health monitors first so they don't race a reconnect
        // against the teardown.
        self.cancel_token.cancel();
        let per_conn = Duration::from_secs(5);
        let mut handles = Vec::with_capacity(self.servers.len());
        for conn in self.servers.values() {
            let c = conn.clone();
            handles.push(tokio::spawn(async move {
                if tokio::time::timeout(per_conn, c.shutdown()).await.is_err() {
                    tracing::warn!(
                        target: "mcp::supervisor",
                        server = %c.name(),
                        "mcp shutdown exceeded per-connection budget; abandoning child to RAII"
                    );
                }
            }));
        }
        for h in handles {
            if let Err(e) = h.await {
                tracing::warn!(
                    target: "mcp::supervisor",
                    error = %e,
                    "MCP per-connection shutdown task panicked"
                );
            }
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
        let mut out = Vec::new();
        // Iterate servers in sorted order for deterministic output.
        let mut server_names: Vec<&String> = self.servers.keys().collect();
        server_names.sort();
        for name in server_names {
            let conn = self
                .servers
                .get(name)
                .expect("just collected from this map");
            let tools = conn.list_tools().await?;
            for t in tools {
                // Call `schema_as_json_value()` first (borrows `t`) before
                // moving any fields out of `t`. Then extract owned fields.
                // `schema_as_json_value()` returns
                // `Value::Object(self.input_schema.as_ref().clone())`.
                let input_schema = t.schema_as_json_value();
                let tool_name = t.name.to_string();
                let description = t.description.map(|c| c.to_string());
                out.push(McpToolEntry {
                    server: name.clone(),
                    tool: tool_name,
                    description,
                    input_schema,
                });
            }
        }
        // Final sort by (server, tool) to be doubly safe — server-side
        // tools/list ordering is implementation-defined.
        out.sort_by(|a, b| a.server.cmp(&b.server).then_with(|| a.tool.cmp(&b.tool)));
        Ok(out)
    }

    /// Call a tool on a specific server.
    ///
    /// `timeout` is enforced as a hard deadline via
    /// [`tokio::time::timeout`]. The effective bound is
    /// `min(timeout, server_config_timeout)` — whichever fires first
    /// wins. On caller-timeout expiry this returns
    /// [`McpError::Timeout`]; the server-config timeout is enforced
    /// independently by [`McpServerConnection::call_tool`].
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
        let r = match tokio::time::timeout(timeout, conn.call_tool(tool, arguments)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(e),
            Err(_elapsed) => return Err(McpError::Timeout(timeout)),
        };
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
