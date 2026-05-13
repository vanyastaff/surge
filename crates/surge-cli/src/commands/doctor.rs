//! `surge doctor` — runtime diagnostics for ACP agents and sandbox matrix.
//!
//! Surfaces:
//! - detected agents on `PATH` (via `surge-acp` registry + discovery);
//! - their version vs. the declared
//!   [`surge_core::runtime::RuntimeVersionPolicy`];
//! - bundled sandbox matrix as JSON / TOML / text.
//!
//! Build the `DoctorReport` data shape from `surge_core::doctor` so every
//! consumer (CLI text output, JSON for tooling, UI surface) agrees on the
//! contract.

use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;
use surge_acp::Registry;
use surge_core::doctor::{DoctorEntry, DoctorReport, MatrixCell, MatrixCellStatus, VersionStatus};
use surge_core::runtime::{RuntimeKind, RuntimeVersionPolicy, version_policy};
use surge_core::sandbox::SandboxMode;
use surge_core::sandbox_matrix::{RuntimeSandboxMatrix, RuntimeSandboxRow};
use surge_orchestrator::engine::version_probe::{ProbeError, probe_version};
use tracing::{debug, info, warn};

/// `surge doctor` subcommand surface.
#[derive(Subcommand, Debug)]
pub enum DoctorCommands {
    /// Full diagnostic report: detected agents, version policy compliance,
    /// per-runtime sandbox-matrix status.
    Report {
        /// Output format.
        #[arg(short, long, default_value = "text")]
        format: DoctorFormat,
    },
    /// Print the bundled sandbox matrix without probing.
    Matrix {
        /// Output format.
        #[arg(short, long, default_value = "text")]
        format: DoctorFormat,
    },
    /// Run a smoke session against a named agent. The mock smoke runs in
    /// CI; the real smoke is gated by `SURGE_DOCTOR_REAL=1`.
    Agent {
        /// Agent id from `surge.toml` or the builtin registry.
        name: String,
    },
}

/// Output format selector for `report` and `matrix` subcommands.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DoctorFormat {
    /// Human-friendly aligned text (default).
    Text,
    /// Machine-readable JSON.
    Json,
    /// TOML mirror of the bundled matrix shape.
    Toml,
}

/// Dispatch entry point — wired from `main.rs`'s `Commands::Doctor` arm.
pub async fn run(command: DoctorCommands) -> Result<()> {
    info!(target: "surge_cli.doctor", ?command, "running surge doctor");
    match command {
        DoctorCommands::Report { format } => run_report(format).await,
        DoctorCommands::Matrix { format } => run_matrix(format),
        DoctorCommands::Agent { name } => run_agent_smoke(name).await,
    }
}

async fn run_report(format: DoctorFormat) -> Result<()> {
    debug!(target: "surge_cli.doctor", ?format, "building DoctorReport");
    let registry = Registry::builtin();
    let detected = registry.detect_installed_with_paths();
    let matrix = surge_core::default_matrix();
    info!(
        target: "surge_cli.doctor",
        registry_count = registry.list().len(),
        detected_count = detected.len(),
        "registry detection complete"
    );

    // Index detections by agent id so every registry entry can find its
    // command_path (when present). Reports are stable across machines:
    // every declared agent gets a row, with `binary_path = None` and
    // `version_status = ProbeFailed` for the ones not on PATH.
    let detected_by_id: std::collections::HashMap<String, surge_acp::DetectedAgent> = detected
        .into_iter()
        .map(|d| (d.entry.id.clone(), d))
        .collect();

    let mut report = DoctorReport::new();
    for entry in registry.list() {
        let command_path = detected_by_id
            .get(&entry.id)
            .and_then(|d| d.command_path.clone());
        let doctor_entry = build_doctor_entry(entry.clone(), command_path, &matrix).await;
        report.entries.push(doctor_entry);
    }

    render(&report, format)?;
    Ok(())
}

fn run_matrix(format: DoctorFormat) -> Result<()> {
    debug!(target: "surge_cli.doctor", ?format, "rendering bundled matrix");
    let matrix = surge_core::default_matrix();
    match format {
        DoctorFormat::Text => render_matrix_text(&matrix),
        DoctorFormat::Json => {
            let v = serde_json::to_string_pretty(&matrix)
                .map_err(|e| anyhow::anyhow!("json render failed: {e}"))?;
            println!("{v}");
        },
        DoctorFormat::Toml => {
            #[derive(serde::Serialize)]
            struct Doc<'a> {
                rows: &'a [RuntimeSandboxRow],
            }
            let v = toml::to_string(&Doc {
                rows: matrix.rows(),
            })
            .map_err(|e| anyhow::anyhow!("toml render failed: {e}"))?;
            println!("{v}");
        },
    }
    Ok(())
}

/// Task 11 entry point. The mock smoke path is verified in CI; the real
/// smoke path is gated behind `SURGE_DOCTOR_REAL=1`. Until the agent-stage
/// integration lands, the command surface is wired so users see the
/// intended behavior and the test suite can probe it.
async fn run_agent_smoke(name: String) -> Result<()> {
    let real = std::env::var("SURGE_DOCTOR_REAL").is_ok();
    let registry = Registry::builtin();
    let entry = registry
        .list()
        .iter()
        .find(|e| e.id == name)
        .ok_or_else(|| anyhow::anyhow!("agent `{name}` not in builtin registry"))?
        .clone();
    let matrix = surge_core::default_matrix();
    let runtime = entry.runtime;

    println!("agent: {} ({})", entry.id, entry.display_name);
    if let Some(rt) = runtime {
        println!("runtime: {rt}");
        let matrix_dry_run = matrix_dry_run(rt, &matrix);
        println!("matrix dry-run:");
        for cell in matrix_dry_run {
            println!(
                "  {:?} -> {}{}",
                cell.mode,
                format_status(cell.status),
                if cell.flags.is_empty() {
                    String::new()
                } else {
                    format!(" flags={:?}", cell.flags)
                }
            );
        }
    } else {
        warn!(target: "surge_cli.doctor", agent = %name, "no runtime mapping in registry entry");
        println!("runtime: <unmapped; matrix lookup skipped>");
    }

    if real {
        // Real smoke: open ACP session, send canned prompt, close.
        // Deferred to Task 11's full implementation — placeholder for now.
        warn!(
            target: "surge_cli.doctor",
            "real smoke session not yet wired; rerun with SURGE_DOCTOR_REAL unset for matrix-only dry-run"
        );
        println!("real smoke session: NOT YET IMPLEMENTED — Task 11 follow-up");
    } else {
        println!("real smoke session: skipped (set SURGE_DOCTOR_REAL=1 to enable)");
    }
    Ok(())
}

async fn build_doctor_entry(
    entry: surge_acp::RegistryEntry,
    command_path: Option<String>,
    matrix: &RuntimeSandboxMatrix,
) -> DoctorEntry {
    let runtime = entry.runtime;
    let policy = runtime.and_then(version_policy);

    // Probe the binary (if available); fold into VersionStatus.
    let (detected_version, version_status) = match command_path.as_deref() {
        None => (None, VersionStatus::ProbeFailed),
        Some(path) => probe_one(path, policy.as_ref()).await,
    };

    let matrix_cells = match runtime {
        Some(rt) => matrix_dry_run(rt, matrix),
        None => Vec::new(),
    };

    let mut e = DoctorEntry::new(entry.id.clone());
    e.runtime = runtime;
    e.binary_path = command_path;
    e.detected_version = detected_version;
    e.policy = policy;
    e.version_status = version_status;
    e.matrix = matrix_cells;
    e
}

async fn probe_one(
    binary: &str,
    policy: Option<&RuntimeVersionPolicy>,
) -> (Option<String>, VersionStatus) {
    let path = PathBuf::from(binary);
    match probe_version(&path).await {
        Ok(version) => {
            let detected_str = version.to_string();
            match policy {
                Some(p) if !p.min_version.matches(&version) => {
                    warn!(
                        target: "surge_cli.doctor",
                        binary,
                        found = %detected_str,
                        min = %p.min_version,
                        "runtime version below declared minimum"
                    );
                    (Some(detected_str), VersionStatus::BelowMinimum)
                },
                Some(_) => (Some(detected_str), VersionStatus::Ok),
                None => (Some(detected_str), VersionStatus::NotApplicable),
            }
        },
        Err(err) => {
            warn!(target: "surge_cli.doctor", binary, error = %err, "version probe failed");
            (None, probe_error_to_status(&err))
        },
    }
}

fn probe_error_to_status(err: &ProbeError) -> VersionStatus {
    let _ = err; // Errors all fold to the same surface for the report.
    VersionStatus::ProbeFailed
}

/// Render every `(runtime, mode)` matrix cell for a specific runtime.
fn matrix_dry_run(runtime: RuntimeKind, matrix: &RuntimeSandboxMatrix) -> Vec<MatrixCell> {
    let modes = [
        SandboxMode::ReadOnly,
        SandboxMode::WorkspaceWrite,
        SandboxMode::WorkspaceNetwork,
        SandboxMode::FullAccess,
    ];
    modes
        .iter()
        .map(|&mode| match matrix.lookup(runtime, mode) {
            None => MatrixCell::new(mode, MatrixCellStatus::Unsupported),
            Some(row) => {
                let status = if row.verified {
                    MatrixCellStatus::Verified
                } else {
                    MatrixCellStatus::DeclaredUnverified
                };
                MatrixCell::new(mode, status)
                    .with_flags(row.flags.iter().cloned())
                    .with_note(row.note.clone())
            },
        })
        .collect()
}

fn render(report: &DoctorReport, format: DoctorFormat) -> Result<()> {
    match format {
        DoctorFormat::Text => render_report_text(report),
        DoctorFormat::Json => {
            let s = serde_json::to_string_pretty(report)
                .map_err(|e| anyhow::anyhow!("json render failed: {e}"))?;
            println!("{s}");
        },
        DoctorFormat::Toml => {
            let s =
                toml::to_string(report).map_err(|e| anyhow::anyhow!("toml render failed: {e}"))?;
            println!("{s}");
        },
    }
    Ok(())
}

fn render_report_text(report: &DoctorReport) {
    println!("# surge doctor — agent diagnostic report\n");
    if report.entries.is_empty() {
        println!("No ACP agents detected on PATH.");
        println!("Tip: install one of the bundled agents (claude, codex, gemini) and re-run.");
        return;
    }
    for (i, entry) in report.entries.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("## {}", entry.agent_name);
        match &entry.runtime {
            Some(rt) => println!("  runtime:           {rt}"),
            None => println!("  runtime:           <unmapped>"),
        }
        match &entry.binary_path {
            Some(path) => println!("  binary:            {path}"),
            None => println!("  binary:            <not detected on PATH>"),
        }
        match &entry.detected_version {
            Some(v) => println!("  detected version:  {v}"),
            None => println!("  detected version:  <unknown>"),
        }
        if let Some(p) = &entry.policy {
            println!("  declared minimum:  {}", p.min_version);
        }
        println!(
            "  status:            {}",
            format_version_status(entry.version_status)
        );
        if !entry.matrix.is_empty() {
            println!("  sandbox matrix:");
            for cell in &entry.matrix {
                println!(
                    "    {:18} {}{}",
                    format!("{:?}", cell.mode),
                    format_status(cell.status),
                    if cell.flags.is_empty() {
                        String::new()
                    } else {
                        format!("  flags={:?}", cell.flags)
                    },
                );
            }
        }
    }
}

fn render_matrix_text(matrix: &RuntimeSandboxMatrix) {
    println!("# surge sandbox delegation matrix\n");
    let header_runtime = "runtime";
    let header_mode = "mode";
    let header_status = "status";
    let header_flags = "flags";
    println!("{header_runtime:<14} {header_mode:<18} {header_status:<22} {header_flags}");
    let separator = "-".repeat(80);
    println!("{separator}");
    for row in matrix.rows() {
        let status = if row.verified {
            "verified"
        } else if row.flags.is_empty() {
            "declared-unverified"
        } else {
            "declared (flags set)"
        };
        let flags = if row.flags.is_empty() {
            "—".to_string()
        } else {
            format!("{:?}", row.flags)
        };
        println!(
            "{:<14} {:<18} {:<22} {}",
            format!("{}", row.runtime),
            format!("{:?}", row.mode),
            status,
            flags,
        );
    }
}

fn format_version_status(status: VersionStatus) -> &'static str {
    match status {
        VersionStatus::NotApplicable => "no policy declared",
        VersionStatus::Ok => "OK",
        VersionStatus::BelowMinimum => "BELOW MINIMUM (warn-only)",
        VersionStatus::ProbeFailed => "probe failed",
        _ => "unknown",
    }
}

fn format_status(status: MatrixCellStatus) -> &'static str {
    match status {
        MatrixCellStatus::Verified => "verified",
        MatrixCellStatus::DeclaredUnverified => "declared-unverified",
        MatrixCellStatus::Unsupported => "unsupported",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_dry_run_covers_four_modes() {
        let matrix = surge_core::default_matrix();
        let cells = matrix_dry_run(RuntimeKind::ClaudeCode, &matrix);
        assert_eq!(cells.len(), 4);
        let modes: Vec<_> = cells.iter().map(|c| c.mode).collect();
        assert_eq!(
            modes,
            vec![
                SandboxMode::ReadOnly,
                SandboxMode::WorkspaceWrite,
                SandboxMode::WorkspaceNetwork,
                SandboxMode::FullAccess,
            ]
        );
    }

    #[test]
    fn matrix_dry_run_for_claude_marks_verified_cells() {
        let matrix = surge_core::default_matrix();
        let cells = matrix_dry_run(RuntimeKind::ClaudeCode, &matrix);
        // The bundled matrix has Claude Code verified across all four modes.
        for cell in cells {
            assert_eq!(cell.status, MatrixCellStatus::Verified, "{cell:?}");
            assert!(!cell.flags.is_empty());
        }
    }

    #[test]
    fn matrix_dry_run_for_gemini_marks_unverified_non_full_access() {
        let matrix = surge_core::default_matrix();
        let cells = matrix_dry_run(RuntimeKind::Gemini, &matrix);
        // Only full-access is verified for Gemini (others are docker-only gaps).
        for cell in cells {
            match cell.mode {
                SandboxMode::FullAccess => {
                    assert_eq!(cell.status, MatrixCellStatus::Verified);
                },
                _ => {
                    assert_eq!(cell.status, MatrixCellStatus::DeclaredUnverified);
                },
            }
        }
    }
}
