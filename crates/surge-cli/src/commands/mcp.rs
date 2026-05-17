//! `surge mcp` — operator surface for configured MCP servers.
//!
//! Request-scoped validation (ADR-0014): `list` / `start` ask the
//! daemon to spawn → handshake → `tools/list` → tear down per server;
//! `logs` tails the bounded, redacted, daemon-probe-scoped stderr file;
//! `stop` is an explicit idempotent ack — under per-run MCP isolation
//! there is no persistent daemon-held child to stop (to halt MCP
//! activity in a live run, abort the run).

use anyhow::{Result, anyhow};
use clap::{Subcommand, ValueEnum};
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;

/// `surge mcp` subcommand surface.
#[derive(Subcommand, Debug)]
pub enum McpCommands {
    /// List configured MCP servers with a live probe of each.
    List {
        /// Output format.
        #[arg(short, long, default_value = "text")]
        format: McpFormat,
    },
    /// Probe a single configured server (spawn → handshake →
    /// `tools/list` → tear down) to validate its configuration.
    Start {
        /// Server name from `surge.toml` `[[mcp_servers]]`.
        name: String,
        /// Output format.
        #[arg(short, long, default_value = "text")]
        format: McpFormat,
    },
    /// No-op acknowledgement: under per-run MCP isolation there is no
    /// persistent daemon-held child to stop (see ADR-0014).
    Stop {
        /// Server name (accepted for symmetry; nothing is terminated).
        name: String,
    },
    /// Tail the captured (redacted) stderr from the most recent probe
    /// of `name`.
    Logs {
        /// Server name from `surge.toml` `[[mcp_servers]]`.
        name: String,
        /// Max trailing lines to show.
        #[arg(long)]
        tail: Option<usize>,
    },
}

/// Output format selector.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum McpFormat {
    /// Human-friendly aligned text (default).
    Text,
    /// Machine-readable JSON.
    Json,
}

/// Top-level dispatcher for `surge mcp` invocations.
pub async fn run(cmd: McpCommands) -> Result<()> {
    match cmd {
        McpCommands::Stop { name } => {
            // Explicit idempotent ack (ADR-0014 / Alternative A1
            // Option D): per-run isolation means no persistent child.
            println!(
                "mcp stop '{name}': no-op — MCP servers are per-run scoped, \
                 there is no persistent daemon-held child to stop. To halt \
                 MCP activity in a live run, abort the run (`surge` run controls)."
            );
            Ok(())
        },
        McpCommands::List { format } => {
            let facade = connect().await?;
            let servers = facade
                .mcp_probe(None)
                .await
                .map_err(|e| anyhow!("mcp probe failed: {e}"))?;
            match format {
                McpFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&servers)?);
                },
                McpFormat::Text => render_probe_text(&servers),
            }
            Ok(())
        },
        McpCommands::Start { name, format } => {
            let facade = connect().await?;
            let servers = facade
                .mcp_probe(Some(name.clone()))
                .await
                .map_err(|e| anyhow!("mcp probe failed: {e}"))?;
            if servers.is_empty() {
                return Err(anyhow!(
                    "no MCP server named '{name}' in surge.toml [[mcp_servers]]"
                ));
            }
            match format {
                McpFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&servers)?);
                },
                McpFormat::Text => render_probe_text(&servers),
            }
            Ok(())
        },
        McpCommands::Logs { name, tail } => {
            let facade = connect().await?;
            let (server, scope, lines) = facade
                .mcp_logs(name, tail)
                .await
                .map_err(|e| anyhow!("mcp logs failed: {e}"))?;
            if lines.is_empty() {
                println!(
                    "no captured stderr for '{server}' (scope: {scope}). \
                     Run `surge mcp start {server}` to probe it first."
                );
            } else {
                for line in lines {
                    println!("{line}");
                }
            }
            Ok(())
        },
    }
}

/// Connect to the daemon, mapping a connection failure to an
/// actionable message (mirrors `surge daemon` UX).
async fn connect() -> Result<DaemonEngineFacade> {
    let socket_path = surge_daemon::pidfile::socket_path()
        .map_err(|e| anyhow!("could not resolve daemon socket path: {e}"))?;
    DaemonEngineFacade::connect(socket_path)
        .await
        .map_err(|_| anyhow!("daemon not reachable — start it with `surge daemon start`"))
}

fn render_probe_text(servers: &[surge_orchestrator::engine::ipc::McpProbeReport]) {
    if servers.is_empty() {
        println!("no MCP servers configured in surge.toml [[mcp_servers]]");
        return;
    }
    let name_w = servers
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    println!(
        "{:<name_w$}  {:<12}  {:<6}  DETAIL",
        "NAME", "STATUS", "TOOLS"
    );
    for s in servers {
        let tools = s
            .tool_count
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".into());
        let detail = s.error.as_deref().unwrap_or("");
        println!(
            "{:<name_w$}  {:<12}  {:<6}  {}",
            s.name, s.status, tools, detail
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        cmd: McpCommands,
    }

    fn parse(args: &[&str]) -> McpCommands {
        TestCli::parse_from(args).cmd
    }

    #[test]
    fn list_defaults_to_text_format() {
        match parse(&["surge-mcp", "list"]) {
            McpCommands::List { format } => assert!(matches!(format, McpFormat::Text)),
            other => panic!("expected List, got {other:?}"),
        }
        match parse(&["surge-mcp", "list", "--format", "json"]) {
            McpCommands::List { format } => assert!(matches!(format, McpFormat::Json)),
            other => panic!("expected List json, got {other:?}"),
        }
    }

    #[test]
    fn start_takes_name_and_logs_takes_optional_tail() {
        match parse(&["surge-mcp", "start", "filesystem"]) {
            McpCommands::Start { name, .. } => assert_eq!(name, "filesystem"),
            other => panic!("expected Start, got {other:?}"),
        }
        match parse(&["surge-mcp", "logs", "fs", "--tail", "50"]) {
            McpCommands::Logs { name, tail } => {
                assert_eq!(name, "fs");
                assert_eq!(tail, Some(50));
            },
            other => panic!("expected Logs, got {other:?}"),
        }
        match parse(&["surge-mcp", "logs", "fs"]) {
            McpCommands::Logs { tail, .. } => assert_eq!(tail, None),
            other => panic!("expected Logs, got {other:?}"),
        }
    }

    #[test]
    fn stop_parses_and_is_a_pure_ack() {
        // `stop` must parse (it's a real subcommand) and carry the name;
        // its runtime behaviour is a no-op ack with no IPC (ADR-0014).
        match parse(&["surge-mcp", "stop", "fs"]) {
            McpCommands::Stop { name } => assert_eq!(name, "fs"),
            other => panic!("expected Stop, got {other:?}"),
        }
    }
}
