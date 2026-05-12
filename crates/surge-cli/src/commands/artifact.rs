//! `surge artifact` commands.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use surge_core::{
    ArtifactDiagnosticCode, ArtifactKind, ArtifactValidationDiagnostic, ArtifactValidationReport,
    Graph, all_contracts, json_schema_for, markdown_outline, validate_artifact,
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
    /// Export the JSON Schema (draft 2020-12) describing one artifact contract.
    ///
    /// Use `--all` to emit a single JSON object keyed by artifact kind. For
    /// markdown-only kinds the command lists required `## <Section>` headings
    /// instead of a schema.
    Schema {
        /// Artifact contract kind. Omit when using `--all`.
        kind: Option<ArtifactKind>,
        /// Emit every kind as one JSON object.
        #[arg(long, conflicts_with = "kind")]
        all: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = SchemaOutputFormat::Pretty)]
        format: SchemaOutputFormat,
        /// Optional path to write the schema to instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

/// JSON output style for `surge artifact schema`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SchemaOutputFormat {
    /// Pretty-printed JSON (multi-line, indented).
    Pretty,
    /// Compact single-line JSON.
    Json,
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
        ArtifactCommands::Schema {
            kind,
            all,
            format,
            output,
        } => schema_command(kind, all, format, output.as_deref()),
    }
}

fn schema_command(
    kind: Option<ArtifactKind>,
    all: bool,
    format: SchemaOutputFormat,
    output: Option<&Path>,
) -> Result<()> {
    let value = if all {
        all_schemas_value()
    } else {
        let kind = kind.context(
            "specify an artifact kind (for example `surge artifact schema spec`) or pass --all",
        )?;
        single_schema_value(kind)?
    };

    let rendered = match format {
        SchemaOutputFormat::Pretty => serde_json::to_string_pretty(&value)?,
        SchemaOutputFormat::Json => serde_json::to_string(&value)?,
    };

    if let Some(path) = output {
        std::fs::write(path, &rendered).with_context(|| format!("write {}", path.display()))?;
    } else {
        println!("{rendered}");
    }
    Ok(())
}

fn single_schema_value(kind: ArtifactKind) -> Result<serde_json::Value> {
    match json_schema_for(kind) {
        Some(schema) => Ok(schema),
        None => bail!(
            "no JSON schema for {kind}; primary format is {} and the contract is described by required markdown sections instead: {}",
            describe_primary_format(kind),
            describe_outline(kind)
        ),
    }
}

fn all_schemas_value() -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for contract in all_contracts() {
        let key = contract.kind.as_str().to_string();
        let entry = match json_schema_for(contract.kind) {
            Some(schema) => schema,
            None => serde_json::json!({
                "x-surge-no-json-schema": true,
                "primary_format": describe_primary_format(contract.kind),
                "required_markdown_sections": markdown_outline(contract.kind),
            }),
        };
        map.insert(key, entry);
    }
    serde_json::Value::Object(map)
}

fn describe_primary_format(kind: ArtifactKind) -> &'static str {
    match surge_core::contract_for(kind).primary_format {
        surge_core::ArtifactFormat::Markdown => "markdown",
        surge_core::ArtifactFormat::Toml => "toml",
        surge_core::ArtifactFormat::FlowToml => "flow-toml",
    }
}

fn describe_outline(kind: ArtifactKind) -> String {
    match markdown_outline(kind) {
        Some(sections) => sections
            .iter()
            .map(|section| format!("## {section}"))
            .collect::<Vec<_>>()
            .join(", "),
        None => "(no markdown outline registered; this kind currently lacks a JSON schema export)"
            .to_string(),
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
    if kind == ArtifactKind::Flow {
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
            ArtifactDiagnosticCode::GraphParseFailed,
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

    #[test]
    fn validates_flow_reports_parse_errors_separately() {
        let report = validate_loaded_artifact(
            ArtifactKind::Flow,
            Path::new("flow.toml"),
            "schema_version = 1\nstart = [",
        );

        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == ArtifactDiagnosticCode::InvalidToml)
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == ArtifactDiagnosticCode::GraphParseFailed)
        );
    }

    #[test]
    fn validates_flow_reports_graph_errors_with_contract_errors() {
        let report = validate_loaded_artifact(
            ArtifactKind::Flow,
            Path::new("flow.toml"),
            r#"schema_version = 2
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
                .any(|diagnostic| diagnostic.code
                    == ArtifactDiagnosticCode::UnsupportedSchemaVersion)
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == ArtifactDiagnosticCode::GraphValidationFailed)
        );
    }
}
