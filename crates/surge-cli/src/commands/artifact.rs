//! `surge artifact` commands.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use surge_core::{
    ArtifactDiagnosticCode, ArtifactKind, ArtifactValidationDiagnostic, ArtifactValidationReport,
    Graph, validate_artifact,
};

/// Subcommands under `surge artifact`.
#[derive(Subcommand, Debug)]
pub enum ArtifactCommands {
    /// Validate an artifact against a Surge artifact contract.
    Validate {
        /// Artifact contract kind, for example `description`, `roadmap`, `spec`, `adr`, or `flow`.
        #[arg(long)]
        kind: ArtifactKind,
        /// Artifact path to read and validate.
        path: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ArtifactOutputFormat::Human)]
        format: ArtifactOutputFormat,
    },
}

/// Validation output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ArtifactOutputFormat {
    /// Human-readable diagnostics.
    Human,
    /// Pretty-printed JSON report.
    Json,
}

/// Top-level dispatcher for `surge artifact`.
pub fn run(command: ArtifactCommands) -> Result<()> {
    match command {
        ArtifactCommands::Validate { kind, path, format } => validate_command(kind, &path, format),
    }
}

fn validate_command(kind: ArtifactKind, path: &Path, format: ArtifactOutputFormat) -> Result<()> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let report = validate_loaded_artifact(kind, path, &content);
    write_report(path, &report, format)?;

    if report.is_valid() {
        Ok(())
    } else {
        bail!(
            "artifact validation failed with {} error(s)",
            report.error_count()
        )
    }
}

fn validate_loaded_artifact(
    kind: ArtifactKind,
    path: &Path,
    content: &str,
) -> ArtifactValidationReport {
    let mut report = validate_artifact(kind, Some(path), content);
    if kind == ArtifactKind::Flow && report.is_valid() {
        validate_flow_graph(content, &mut report);
    }
    report
}

fn validate_flow_graph(content: &str, report: &mut ArtifactValidationReport) {
    match toml::from_str::<Graph>(content) {
        Ok(graph) => {
            if let Err(error) = surge_orchestrator::engine::validate::validate_for_m6(&graph) {
                report.push(ArtifactValidationDiagnostic::error(
                    ArtifactKind::Flow,
                    ArtifactDiagnosticCode::GraphValidationFailed,
                    None,
                    format!("flow graph validation failed: {error}"),
                ));
            }
        },
        Err(error) => report.push(ArtifactValidationDiagnostic::error(
            ArtifactKind::Flow,
            ArtifactDiagnosticCode::GraphValidationFailed,
            None,
            format!("flow graph parse failed: {error}"),
        )),
    }
}

fn write_report(
    path: &Path,
    report: &ArtifactValidationReport,
    format: ArtifactOutputFormat,
) -> Result<()> {
    match format {
        ArtifactOutputFormat::Human => write_human_report(path, report),
        ArtifactOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report)?);
            Ok(())
        },
    }
}

fn write_human_report(path: &Path, report: &ArtifactValidationReport) -> Result<()> {
    if report.is_valid() {
        println!("OK {} {}", report.kind, path.display());
        return Ok(());
    }

    eprintln!(
        "INVALID {} {} ({} error(s), {} warning(s))",
        report.kind,
        path.display(),
        report.error_count(),
        report.warning_count()
    );
    for diagnostic in &report.diagnostics {
        let location = diagnostic
            .location
            .as_deref()
            .map_or(String::new(), |location| format!(" at {location}"));
        eprintln!(
            "- {:?} [{}]{} {}",
            diagnostic.severity, diagnostic.code, location, diagnostic.message
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_flow_with_engine_errors() {
        let report = validate_loaded_artifact(
            ArtifactKind::Flow,
            Path::new("flow.toml"),
            r#"schema_version = 1
start = "missing"
edges = []

[metadata]
name = "broken"
created_at = "2026-05-11T00:00:00Z"

[nodes]
"#,
        );

        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == ArtifactDiagnosticCode::GraphValidationFailed)
        );
    }
}
