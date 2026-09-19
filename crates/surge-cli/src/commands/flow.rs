//! `surge flow` — inspect the flow catalog (ADR-0020).
//!
//! The catalog is the set of templates a queued task can run: the
//! repository's `.surge/flows/`, the operator's `SURGE_HOME/flows/`, and the
//! bundled archetypes. Precedence is project → home → bundled; the listing
//! names the winning lane so a shadowed template is never a surprise.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use surge_core::{FlowCatalog, FlowRef, ProjectLayer};

use crate::commands::common::{project_root, surge_home_dir};

/// `surge flow` subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum FlowCommands {
    /// List every template visible to this repository.
    List(FlowListArgs),
    /// Show one template: its metadata, lane, path and node/edge counts.
    Show(FlowShowArgs),
}

/// Arguments for `surge flow list`.
#[derive(Debug, Clone, Args)]
pub struct FlowListArgs {
    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `surge flow show`.
#[derive(Debug, Clone, Args)]
pub struct FlowShowArgs {
    /// Flow reference (`bug-fix@1` or `bug-fix@1.0`).
    pub reference: String,
}

/// Run `surge flow`.
///
/// # Errors
/// Catalog scan failures; an unknown reference.
pub async fn run(command: FlowCommands) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = project_root(&cwd);
    let layer = ProjectLayer::for_project(&root);
    let home = surge_home_dir()?.join("flows");
    let catalog = FlowCatalog::scan(&layer, Some(&home)).map_err(|e| anyhow::anyhow!("{e}"))?;

    match command {
        FlowCommands::List(args) => {
            if args.json {
                let rows: Vec<_> = catalog
                    .entries()
                    .iter()
                    .map(|entry| {
                        serde_json::json!({
                            "reference": entry.reference.to_string(),
                            "layer": entry.layer.as_str(),
                            "path": entry.path.as_ref().map(|p| p.display().to_string()),
                            "when_to_use": entry.when_to_use,
                            "archetype": entry.archetype,
                            "autonomy": entry.autonomy,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print_catalog_table(&mut std::io::stdout().lock(), &catalog);
            }
            Ok(())
        },
        FlowCommands::Show(args) => {
            let reference: FlowRef = args.reference.parse().map_err(|e| anyhow::anyhow!("{e}"))?;
            let entry = catalog
                .resolve(&reference)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("reference:   {}", entry.reference);
            println!("layer:       {}", entry.layer);
            if let Some(path) = &entry.path {
                println!("path:        {}", path.display());
            }
            println!(
                "when_to_use: {}",
                entry.when_to_use.as_deref().unwrap_or("(not declared)")
            );
            println!(
                "archetype:   {}",
                entry.archetype.as_deref().unwrap_or("(none)")
            );
            println!("nodes:       {}", entry.graph.nodes.len());
            println!("edges:       {}", entry.graph.edges.len());
            println!("start:       {}", entry.graph.start);
            Ok(())
        },
    }
}

fn print_catalog_table(out: &mut impl std::io::Write, catalog: &FlowCatalog) {
    let entries = catalog.entries();
    if entries.is_empty() {
        let _ = writeln!(out, "No flow templates found.");
        return;
    }
    let _ = writeln!(out, "{:<26} {:<9} WHEN TO USE", "TEMPLATE", "LAYER");
    for entry in entries {
        let _ = writeln!(
            out,
            "{:<26} {:<9} {}",
            entry.reference,
            entry.layer.as_str(),
            entry.when_to_use.as_deref().unwrap_or("(not declared)")
        );
    }
    let _ = writeln!(out, "\n{} template(s).", entries.len());
}
