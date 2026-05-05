//! Engine-wide registry of MCP server connections.

use crate::connection::McpServerConnection;
use crate::error::McpError;
use std::collections::HashMap;
use std::sync::Arc;
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
}

impl McpRegistry {
    /// Build a registry from a slice of [`McpServerRef`]. Connections
    /// are not eagerly opened — first use of each server triggers
    /// the spawn via [`McpServerConnection::list_tools`] /
    /// [`McpServerConnection::call_tool`].
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
    /// `RoutingToolDispatcher` at session-open to assemble the
    /// agent's tool catalog.
    ///
    /// Servers are queried in sorted order by name, and the final list
    /// is additionally sorted by `(server, tool)` to guarantee
    /// deterministic output regardless of `HashMap` iteration order or
    /// per-server `tools/list` ordering.
    pub async fn list_all_tools(&self) -> Result<Vec<McpToolEntry>, McpError> {
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
        let r = McpRegistry::from_config(&[]);
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
        let r = McpRegistry::from_config(&refs);
        assert_eq!(r.servers.len(), 1);
        assert!(r.servers.contains_key("echo"));
    }
}
