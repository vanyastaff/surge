//! `surge trust` — inspect and pin repo-resident `.surge/` files (ADR-0020).
//!
//! A fresh clone's `.surge/` files are executable context that the operator
//! has not seen. `surge trust list` shows what is pinned and what drifted;
//! `surge trust accept <path>` pins the current content, which is what
//! unblocks a run after an `UntrustedProjectFile` escalation.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use surge_core::artifact_contract::RelPath;

use crate::commands::common::{project_root, surge_home_dir};
use surge_persistence::trust_store::{TrustStore, repo_id};

/// `surge trust` subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum TrustCommands {
    /// List pinned files for this repository and their drift state.
    List(TrustListArgs),
    /// Pin a file's current content.
    Accept(TrustAcceptArgs),
}

/// Arguments for `surge trust list`.
#[derive(Debug, Clone, Args)]
pub struct TrustListArgs {
    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `surge trust accept`.
#[derive(Debug, Clone, Args)]
pub struct TrustAcceptArgs {
    /// Project-relative path (e.g. `.surge/flows/bug-fix-1.0.toml`).
    /// Optional when `--all` is given.
    pub path: Option<String>,
    /// Pin every gated project file under `.surge/` at its current content.
    #[arg(long)]
    pub all: bool,
}

/// Run `surge trust`.
///
/// # Errors
/// Storage/IO failures; an unknown path or a path outside the project.
pub async fn run(command: TrustCommands) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = project_root(&cwd);
    let store = open_store(&root)?;
    match command {
        TrustCommands::List(args) => list(&root, &store, args),
        TrustCommands::Accept(args) => accept(&root, store, args),
    }
}

fn open_store(root: &std::path::Path) -> Result<TrustStore> {
    let home = surge_home_dir()?;
    let id = repo_id(remote_url(root).as_deref(), root);
    TrustStore::open(&home, &id).map_err(|e| anyhow!("{e}"))
}

/// The repository's `origin` URL, when it has one.
fn remote_url(root: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// Every `.surge/` file in the repository (recursively), project-relative.
fn project_files(root: &std::path::Path) -> Result<Vec<RelPath>> {
    let layer_dir = root.join(".surge");
    if !layer_dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    collect(&layer_dir, root, &mut files)?;
    files.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Ok(files)
}

fn collect(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<RelPath>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "worktrees" || name == "runs" {
            continue;
        }
        if path.is_dir() {
            collect(&path, root, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| anyhow!("{} is outside the project", path.display()))?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if let Ok(rel_path) = RelPath::new(rel_str) {
            out.push(rel_path);
        }
    }
    Ok(())
}

fn list(root: &std::path::Path, store: &TrustStore, args: TrustListArgs) -> Result<()> {
    let files = project_files(root)?;
    let mut rows = Vec::new();
    for path in files
        .iter()
        .filter(|path| surge_persistence::trust_store::is_trust_gated_path(path.as_str()))
    {
        let absolute = path.join_onto(root);
        let content =
            std::fs::read(&absolute).with_context(|| format!("read {}", absolute.display()))?;
        let state = match store.check(path, &content).map_err(|e| anyhow!("{e}"))? {
            None => "pinned",
            Some(untrusted) if untrusted.pinned.is_some() => "changed",
            Some(_) => "unpinned",
        };
        rows.push((path.as_str().to_string(), state));
    }
    if args.json {
        let payload: Vec<_> = rows
            .iter()
            .map(|(path, state)| serde_json::json!({ "path": path, "state": state }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("No project files under .surge/.");
        return Ok(());
    }
    println!("{:<44} STATE", "PATH");
    for (path, state) in &rows {
        println!("{path:<44} {state}");
    }
    let untrusted = rows.iter().filter(|(_, s)| *s != "pinned").count();
    if untrusted > 0 {
        println!("\n{untrusted} file(s) need review: `surge trust accept <path>` (or --all).");
    }
    Ok(())
}

fn accept(root: &std::path::Path, mut store: TrustStore, args: TrustAcceptArgs) -> Result<()> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let targets: Vec<RelPath> = if args.all {
        project_files(root)?
            .into_iter()
            .filter(|path| surge_persistence::trust_store::is_trust_gated_path(path.as_str()))
            .collect()
    } else {
        let raw = args
            .path
            .ok_or_else(|| anyhow!("provide a path, or --all to pin every .surge/ file"))?;
        let path =
            RelPath::new(raw.trim_start_matches("./")).map_err(|e| anyhow!("invalid path: {e}"))?;
        vec![path]
    };
    if targets.is_empty() {
        bail!("no files to pin under .surge/");
    }
    for path in &targets {
        let absolute = path.join_onto(root);
        if !absolute.exists() {
            bail!("{} does not exist", absolute.display());
        }
        let hash = store
            .accept_file(root, path, now_ms)
            .map_err(|e| anyhow!("{e}"))?;
        println!("pinned {} ({hash})", path.as_str());
    }
    Ok(())
}
