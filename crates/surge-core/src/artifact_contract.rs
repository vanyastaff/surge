//! Artifact contract metadata.
//!
//! This module is intentionally pure: it defines canonical artifact names,
//! schema ownership, and validator identifiers without reading files or
//! invoking runtime services. CLI and orchestrator code compose these contracts
//! with I/O, logging, and engine-specific validation.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// Current schema version used by Surge-owned artifact contracts.
pub const ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// Role artifact families that Surge validates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    /// Bootstrap description artifact.
    Description,
    /// Product requirements artifact.
    Requirements,
    /// Roadmap Planner artifact.
    Roadmap,
    /// Spec Author artifact.
    Spec,
    /// Architect decision artifact.
    Adr,
    /// Long-form subtask story artifact.
    Story,
    /// Implementation plan artifact.
    Plan,
    /// Executable `flow.toml` graph artifact.
    Flow,
}

impl ArtifactKind {
    /// Stable kebab-case identifier used in profile metadata and CLI flags.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Description => "description",
            Self::Requirements => "requirements",
            Self::Roadmap => "roadmap",
            Self::Spec => "spec",
            Self::Adr => "adr",
            Self::Story => "story",
            Self::Plan => "plan",
            Self::Flow => "flow",
        }
    }

    /// Return the contract metadata for this kind.
    #[must_use]
    pub const fn contract(self) -> ArtifactContract {
        contract_for(self)
    }
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse an artifact kind from a stable CLI/profile identifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown artifact kind {input:?}")]
pub struct ParseArtifactKindError {
    input: String,
}

impl FromStr for ArtifactKind {
    type Err = ParseArtifactKindError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "description" | "description-md" => Ok(Self::Description),
            "requirements" | "requirements-md" => Ok(Self::Requirements),
            "roadmap" | "roadmap-md" | "roadmap-toml" => Ok(Self::Roadmap),
            "spec" | "spec-md" | "spec-toml" => Ok(Self::Spec),
            "adr" | "architecture-decision-record" => Ok(Self::Adr),
            "story" | "story-file" => Ok(Self::Story),
            "plan" | "implementation-plan" => Ok(Self::Plan),
            "flow" | "flow-toml" => Ok(Self::Flow),
            _ => Err(ParseArtifactKindError {
                input: input.to_owned(),
            }),
        }
    }
}

/// Primary serialization format for an artifact contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactFormat {
    /// Markdown with required headings and optional structured sections.
    Markdown,
    /// Human-authored TOML parsed into typed Rust structures.
    Toml,
    /// `flow.toml` graph parsed as [`crate::graph::Graph`].
    FlowToml,
}

/// Component that owns the schema-version field for a contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaVersionOwner {
    /// Generic artifact contract version from [`ARTIFACT_SCHEMA_VERSION`].
    ArtifactContract,
    /// The graph schema version in [`crate::graph::SCHEMA_VERSION`].
    Graph,
    /// No machine-readable schema-version field is required.
    HumanReadable,
}

/// Stable reference embedded in profiles and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactContractRef {
    /// Artifact family.
    pub kind: ArtifactKind,
    /// Expected schema version for this artifact family.
    pub schema_version: u32,
}

impl ArtifactContractRef {
    /// Create a reference for the current version of `kind`.
    #[must_use]
    pub const fn current(kind: ArtifactKind) -> Self {
        Self {
            kind,
            schema_version: ARTIFACT_SCHEMA_VERSION,
        }
    }
}

/// Severity for artifact validation diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactDiagnosticSeverity {
    /// Validation cannot accept the artifact.
    Error,
    /// Validation accepts the artifact but found a compatibility issue.
    Warning,
}

/// Stable diagnostic code emitted by artifact validators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactDiagnosticCode {
    /// Artifact path/name does not match the selected contract.
    InvalidArtifactPath,
    /// Required markdown section is missing.
    MissingSection,
    /// Required machine-readable field is missing.
    MissingField,
    /// `schema_version` is missing.
    MissingSchemaVersion,
    /// `schema_version` is present but unsupported.
    UnsupportedSchemaVersion,
    /// TOML content failed to parse.
    InvalidToml,
    /// Markdown frontmatter is missing or malformed.
    InvalidFrontmatter,
    /// Acceptance criteria are missing.
    MissingAcceptanceCriteria,
    /// Artifact kind is not supported by this validator.
    UnsupportedArtifactKind,
    /// Flow graph parsed as TOML but failed engine-level graph validation.
    GraphValidationFailed,
}

impl ArtifactDiagnosticCode {
    /// Stable snake-case code for CLI JSON and tests.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArtifactPath => "invalid_artifact_path",
            Self::MissingSection => "missing_section",
            Self::MissingField => "missing_field",
            Self::MissingSchemaVersion => "missing_schema_version",
            Self::UnsupportedSchemaVersion => "unsupported_schema_version",
            Self::InvalidToml => "invalid_toml",
            Self::InvalidFrontmatter => "invalid_frontmatter",
            Self::MissingAcceptanceCriteria => "missing_acceptance_criteria",
            Self::UnsupportedArtifactKind => "unsupported_artifact_kind",
            Self::GraphValidationFailed => "graph_validation_failed",
        }
    }
}

impl fmt::Display for ArtifactDiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One validation diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactValidationDiagnostic {
    /// Stable machine-readable code.
    pub code: ArtifactDiagnosticCode,
    /// Severity of the finding.
    pub severity: ArtifactDiagnosticSeverity,
    /// Artifact family being validated.
    pub kind: ArtifactKind,
    /// Optional path or logical field location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Short user-safe message. Do not include full artifact contents.
    pub message: String,
}

impl ArtifactValidationDiagnostic {
    /// Construct an error diagnostic.
    #[must_use]
    pub fn error(
        kind: ArtifactKind,
        code: ArtifactDiagnosticCode,
        location: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: ArtifactDiagnosticSeverity::Error,
            kind,
            location,
            message: message.into(),
        }
    }

    /// Construct a warning diagnostic.
    #[must_use]
    pub fn warning(
        kind: ArtifactKind,
        code: ArtifactDiagnosticCode,
        location: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: ArtifactDiagnosticSeverity::Warning,
            kind,
            location,
            message: message.into(),
        }
    }
}

/// Validation report for one artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactValidationReport {
    /// Artifact kind selected by the caller.
    pub kind: ArtifactKind,
    /// Diagnostics emitted by the validator.
    #[serde(default)]
    pub diagnostics: Vec<ArtifactValidationDiagnostic>,
}

impl ArtifactValidationReport {
    /// Start an empty report for `kind`.
    #[must_use]
    pub const fn new(kind: ArtifactKind) -> Self {
        Self {
            kind,
            diagnostics: Vec::new(),
        }
    }

    /// Append a diagnostic.
    pub fn push(&mut self, diagnostic: ArtifactValidationDiagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Return true when no error-severity diagnostics are present.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.error_count() == 0
    }

    /// Count error-severity diagnostics.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == ArtifactDiagnosticSeverity::Error)
            .count()
    }

    /// Count warning-severity diagnostics.
    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == ArtifactDiagnosticSeverity::Warning)
            .count()
    }

    /// Convert this report into a result that fails when errors exist.
    ///
    /// # Errors
    /// Returns [`ArtifactValidationError`] with the full report when any
    /// error-severity diagnostic is present.
    pub fn into_result(self) -> Result<(), ArtifactValidationError> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(ArtifactValidationError { report: self })
        }
    }
}

/// Error wrapper for invalid artifacts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{} artifact validation error(s) for {}",
    .report.error_count(),
    .report.kind
)]
pub struct ArtifactValidationError {
    /// Full validation report.
    pub report: ArtifactValidationReport,
}

/// Canonical metadata for one artifact family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactContract {
    /// Artifact family.
    pub kind: ArtifactKind,
    /// Canonical path or path pattern relative to the run worktree.
    pub canonical_path: &'static str,
    /// Primary representation agents should produce.
    pub primary_format: ArtifactFormat,
    /// Optional compatibility artifact when humans also need Markdown.
    pub markdown_compatibility: Option<&'static str>,
    /// Schema-version ownership.
    pub schema_version_owner: SchemaVersionOwner,
    /// Stable validator id used by CLI flags and diagnostics.
    pub validator_kind: &'static str,
    /// Accepted historical or ergonomic aliases for the artifact path/name.
    pub aliases: &'static [&'static str],
}

impl ArtifactContract {
    /// Return a versioned reference to this contract.
    #[must_use]
    pub const fn reference(self) -> ArtifactContractRef {
        ArtifactContractRef::current(self.kind)
    }

    /// True if `path` matches the canonical path or one of the accepted aliases.
    #[must_use]
    pub fn accepts_path(self, path: &Path) -> bool {
        let normalized = normalize_path(path);
        if normalized == self.canonical_path {
            return true;
        }
        self.aliases.iter().any(|alias| normalized == *alias)
            || self.accepts_pattern(normalized.as_str())
    }

    fn accepts_pattern(self, normalized: &str) -> bool {
        match self.kind {
            ArtifactKind::Adr => is_adr_path(normalized),
            ArtifactKind::Story => is_story_path(normalized),
            _ => false,
        }
    }
}

/// Return every canonical artifact contract in stable order.
#[must_use]
pub const fn all_contracts() -> &'static [ArtifactContract] {
    &CONTRACTS
}

/// Return the contract metadata for `kind`.
#[must_use]
pub const fn contract_for(kind: ArtifactKind) -> ArtifactContract {
    match kind {
        ArtifactKind::Description => DESCRIPTION_CONTRACT,
        ArtifactKind::Requirements => REQUIREMENTS_CONTRACT,
        ArtifactKind::Roadmap => ROADMAP_CONTRACT,
        ArtifactKind::Spec => SPEC_CONTRACT,
        ArtifactKind::Adr => ADR_CONTRACT,
        ArtifactKind::Story => STORY_CONTRACT,
        ArtifactKind::Plan => PLAN_CONTRACT,
        ArtifactKind::Flow => FLOW_CONTRACT,
    }
}

/// Validate an artifact path and contents against the selected contract.
#[must_use]
pub fn validate_artifact(
    kind: ArtifactKind,
    path: Option<&Path>,
    content: &str,
) -> ArtifactValidationReport {
    let mut report = ArtifactValidationReport::new(kind);
    if let Some(path) = path {
        validate_artifact_path_into(&mut report, path);
    }
    validate_artifact_text_into(&mut report, path, content);
    report
}

/// Validate an artifact path against the selected contract.
#[must_use]
pub fn validate_artifact_path(kind: ArtifactKind, path: &Path) -> ArtifactValidationReport {
    let mut report = ArtifactValidationReport::new(kind);
    validate_artifact_path_into(&mut report, path);
    report
}

/// Validate artifact contents against the selected contract's primary format.
#[must_use]
pub fn validate_artifact_text(kind: ArtifactKind, content: &str) -> ArtifactValidationReport {
    let mut report = ArtifactValidationReport::new(kind);
    validate_artifact_text_into(&mut report, None, content);
    report
}

fn validate_artifact_path_into(report: &mut ArtifactValidationReport, path: &Path) {
    let contract = contract_for(report.kind);
    if contract.accepts_path(path) {
        return;
    }

    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::InvalidArtifactPath,
        Some(normalize_path(path)),
        format!(
            "expected artifact path matching {} for {}",
            contract.canonical_path, report.kind
        ),
    ));
}

fn validate_artifact_text_into(
    report: &mut ArtifactValidationReport,
    path: Option<&Path>,
    content: &str,
) {
    match report.kind {
        ArtifactKind::Description => validate_description_markdown(report, content),
        ArtifactKind::Requirements => validate_requirements_markdown(report, content),
        ArtifactKind::Roadmap => validate_roadmap(report, path, content),
        ArtifactKind::Spec => validate_spec(report, path, content),
        ArtifactKind::Adr => validate_adr_markdown(report, content),
        ArtifactKind::Story => validate_story_markdown(report, content),
        ArtifactKind::Plan => validate_plan_markdown(report, content),
        ArtifactKind::Flow => validate_flow_toml(report, content),
    }
}

fn validate_description_markdown(report: &mut ArtifactValidationReport, content: &str) {
    require_markdown_sections(
        report,
        content,
        &["Goal", "Context", "Requirements", "Out of Scope"],
    );
}

fn validate_requirements_markdown(report: &mut ArtifactValidationReport, content: &str) {
    require_markdown_sections(
        report,
        content,
        &[
            "Overview",
            "User Stories",
            "Functional Requirements",
            "Success Criteria",
            "Out of Scope",
        ],
    );
}

fn validate_roadmap(report: &mut ArtifactValidationReport, path: Option<&Path>, content: &str) {
    if is_markdown_artifact(path, content) {
        require_markdown_sections(report, content, &["Milestones", "Dependencies", "Risks"]);
    } else {
        validate_toml_artifact(report, content, &["milestones"]);
    }
}

fn validate_spec(report: &mut ArtifactValidationReport, path: Option<&Path>, content: &str) {
    if is_markdown_artifact(path, content) {
        require_markdown_sections(
            report,
            content,
            &["Goal", "Subtasks", "Acceptance Criteria"],
        );
        require_acceptance_criteria(report, content);
    } else {
        validate_toml_artifact(report, content, &["spec"]);
        validate_spec_toml_acceptance(report, content);
    }
}

fn validate_story_markdown(report: &mut ArtifactValidationReport, content: &str) {
    require_markdown_sections(
        report,
        content,
        &[
            "Context",
            "What needs to be done",
            "Architecture decisions",
            "Files to modify",
            "Acceptance criteria",
            "Out of scope",
        ],
    );
    require_acceptance_criteria(report, content);
}

fn validate_plan_markdown(report: &mut ArtifactValidationReport, content: &str) {
    require_markdown_sections(report, content, &["Settings", "Tasks"]);
}

fn validate_adr_markdown(report: &mut ArtifactValidationReport, content: &str) {
    validate_adr_frontmatter(report, content);
    require_markdown_sections(
        report,
        content,
        &[
            "Status",
            "Context",
            "Decision",
            "Alternatives considered",
            "Consequences",
        ],
    );
}

fn validate_flow_toml(report: &mut ArtifactValidationReport, content: &str) {
    let Some(value) = parse_toml_value(report, content) else {
        return;
    };
    validate_schema_version(report, &value, crate::graph::SCHEMA_VERSION);
    require_toml_fields(report, &value, &["metadata", "start", "nodes", "edges"]);
}

fn validate_toml_artifact(
    report: &mut ArtifactValidationReport,
    content: &str,
    required_fields: &[&str],
) {
    let Some(value) = parse_toml_value(report, content) else {
        return;
    };
    validate_schema_version(report, &value, ARTIFACT_SCHEMA_VERSION);
    require_toml_fields(report, &value, required_fields);
}

fn validate_spec_toml_acceptance(report: &mut ArtifactValidationReport, content: &str) {
    let Some(value) = content.parse::<toml::Value>().ok() else {
        return;
    };
    if spec_toml_has_acceptance_criteria(&value) {
        return;
    }
    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::MissingAcceptanceCriteria,
        Some("spec.subtasks.acceptance_criteria".to_string()),
        "every spec subtask must include machine-readable acceptance criteria",
    ));
}

fn parse_toml_value(report: &mut ArtifactValidationReport, content: &str) -> Option<toml::Value> {
    match content.parse::<toml::Value>() {
        Ok(value) => Some(value),
        Err(error) => {
            report.push(ArtifactValidationDiagnostic::error(
                report.kind,
                ArtifactDiagnosticCode::InvalidToml,
                None,
                format!("artifact TOML failed to parse: {error}"),
            ));
            None
        },
    }
}

fn validate_schema_version(
    report: &mut ArtifactValidationReport,
    value: &toml::Value,
    expected: u32,
) {
    let Some(schema_version) = value.get("schema_version") else {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingSchemaVersion,
            Some("schema_version".to_string()),
            "artifact is missing schema_version",
        ));
        return;
    };

    if schema_version.as_integer() == Some(i64::from(expected)) {
        return;
    }

    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::UnsupportedSchemaVersion,
        Some("schema_version".to_string()),
        format!("expected schema_version {expected}"),
    ));
}

fn require_toml_fields(
    report: &mut ArtifactValidationReport,
    value: &toml::Value,
    required_fields: &[&str],
) {
    for field in required_fields {
        if toml_field(value, field).is_some() {
            continue;
        }
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingField,
            Some((*field).to_string()),
            format!("artifact is missing required field {field}"),
        ));
    }
}

fn toml_field<'a>(value: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    path.split('.')
        .try_fold(value, |current, segment| current.get(segment))
}

fn spec_toml_has_acceptance_criteria(value: &toml::Value) -> bool {
    let spec = value.get("spec").unwrap_or(value);
    let Some(subtasks) = spec.get("subtasks").and_then(toml::Value::as_array) else {
        return false;
    };

    !subtasks.is_empty()
        && subtasks
            .iter()
            .all(|subtask| toml_array_is_non_empty(subtask, "acceptance_criteria"))
}

fn toml_array_is_non_empty(value: &toml::Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(toml::Value::as_array)
        .is_some_and(|items| !items.is_empty())
}

fn validate_adr_frontmatter(report: &mut ArtifactValidationReport, content: &str) {
    let Some(frontmatter) = parse_toml_frontmatter(report, content) else {
        return;
    };
    require_toml_fields(report, &frontmatter, &["status", "deciders", "date"]);
}

fn parse_toml_frontmatter(
    report: &mut ArtifactValidationReport,
    content: &str,
) -> Option<toml::Value> {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("+++") {
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::InvalidFrontmatter,
            Some("frontmatter".to_string()),
            "ADR must start with TOML frontmatter delimited by +++",
        ));
        return None;
    }

    let mut frontmatter = Vec::new();
    for line in lines {
        if line.trim() == "+++" {
            return parse_toml_value(report, &frontmatter.join("\n"));
        }
        frontmatter.push(line);
    }

    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::InvalidFrontmatter,
        Some("frontmatter".to_string()),
        "ADR TOML frontmatter is missing the closing +++ delimiter",
    ));
    None
}

fn require_markdown_sections(
    report: &mut ArtifactValidationReport,
    content: &str,
    required_sections: &[&str],
) {
    for section in required_sections {
        if has_markdown_heading(content, section) {
            continue;
        }
        report.push(ArtifactValidationDiagnostic::error(
            report.kind,
            ArtifactDiagnosticCode::MissingSection,
            Some(format!("## {section}")),
            format!("artifact is missing required section ## {section}"),
        ));
    }
}

fn require_acceptance_criteria(report: &mut ArtifactValidationReport, content: &str) {
    if has_markdown_heading(content, "Acceptance Criteria")
        || content.to_ascii_lowercase().contains("acceptance criteria")
    {
        return;
    }
    report.push(ArtifactValidationDiagnostic::error(
        report.kind,
        ArtifactDiagnosticCode::MissingAcceptanceCriteria,
        Some("Acceptance Criteria".to_string()),
        "artifact must include acceptance criteria",
    ));
}

fn has_markdown_heading(content: &str, expected: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            return false;
        }
        let heading = trimmed.trim_start_matches('#').trim();
        strip_trailing_heading_marker(heading).eq_ignore_ascii_case(expected)
    })
}

fn strip_trailing_heading_marker(heading: &str) -> &str {
    heading.trim_end_matches('#').trim_end()
}

fn is_markdown_artifact(path: Option<&Path>, content: &str) -> bool {
    path.is_some_and(|path| path.extension().is_some_and(|ext| ext == "md"))
        || content.trim_start().starts_with('#')
        || content.trim_start().starts_with("+++")
}

fn normalize_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn is_adr_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("docs/adr/") else {
        return false;
    };
    if !rest.ends_with(".md") {
        return false;
    }
    let stem = rest.trim_end_matches(".md");
    let Some((number, slug)) = stem.split_once('-') else {
        return false;
    };
    number.len() == 4 && number.chars().all(|ch| ch.is_ascii_digit()) && !slug.is_empty()
}

fn is_story_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("stories/story-") else {
        return false;
    };
    let Some(number) = rest.strip_suffix(".md") else {
        return false;
    };
    number.len() == 3 && number.chars().all(|ch| ch.is_ascii_digit())
}

const DESCRIPTION_ALIASES: &[&str] = &[];
const REQUIREMENTS_ALIASES: &[&str] = &["requirements.md"];
const ROADMAP_ALIASES: &[&str] = &["roadmap.md"];
const SPEC_ALIASES: &[&str] = &["spec.md"];
const ADR_ALIASES: &[&str] = &["adr.md"];
const STORY_ALIASES: &[&str] = &[];
const PLAN_ALIASES: &[&str] = &["plan.md"];
const FLOW_ALIASES: &[&str] = &[];

const DESCRIPTION_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Description,
    canonical_path: "description.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "description",
    aliases: DESCRIPTION_ALIASES,
};

const REQUIREMENTS_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Requirements,
    canonical_path: "requirements.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "requirements",
    aliases: REQUIREMENTS_ALIASES,
};

const ROADMAP_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Roadmap,
    canonical_path: "roadmap.toml",
    primary_format: ArtifactFormat::Toml,
    markdown_compatibility: Some("roadmap.md"),
    schema_version_owner: SchemaVersionOwner::ArtifactContract,
    validator_kind: "roadmap",
    aliases: ROADMAP_ALIASES,
};

const SPEC_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Spec,
    canonical_path: "spec.toml",
    primary_format: ArtifactFormat::Toml,
    markdown_compatibility: Some("spec.md"),
    schema_version_owner: SchemaVersionOwner::ArtifactContract,
    validator_kind: "spec",
    aliases: SPEC_ALIASES,
};

const ADR_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Adr,
    canonical_path: "docs/adr/<NNNN>-<slug>.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "adr",
    aliases: ADR_ALIASES,
};

const STORY_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Story,
    canonical_path: "stories/story-NNN.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "story",
    aliases: STORY_ALIASES,
};

const PLAN_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Plan,
    canonical_path: "plan.md",
    primary_format: ArtifactFormat::Markdown,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::HumanReadable,
    validator_kind: "plan",
    aliases: PLAN_ALIASES,
};

const FLOW_CONTRACT: ArtifactContract = ArtifactContract {
    kind: ArtifactKind::Flow,
    canonical_path: "flow.toml",
    primary_format: ArtifactFormat::FlowToml,
    markdown_compatibility: None,
    schema_version_owner: SchemaVersionOwner::Graph,
    validator_kind: "flow",
    aliases: FLOW_ALIASES,
};

const CONTRACTS: [ArtifactContract; 8] = [
    DESCRIPTION_CONTRACT,
    REQUIREMENTS_CONTRACT,
    ROADMAP_CONTRACT,
    SPEC_CONTRACT,
    ADR_CONTRACT,
    STORY_CONTRACT,
    PLAN_CONTRACT,
    FLOW_CONTRACT,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_are_in_stable_order() {
        let kinds: Vec<_> = all_contracts()
            .iter()
            .map(|contract| contract.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                ArtifactKind::Description,
                ArtifactKind::Requirements,
                ArtifactKind::Roadmap,
                ArtifactKind::Spec,
                ArtifactKind::Adr,
                ArtifactKind::Story,
                ArtifactKind::Plan,
                ArtifactKind::Flow,
            ]
        );
    }

    #[test]
    fn artifact_kind_parses_aliases() {
        assert_eq!(
            "flow-toml".parse::<ArtifactKind>().unwrap(),
            ArtifactKind::Flow
        );
        assert_eq!(
            "spec-md".parse::<ArtifactKind>().unwrap(),
            ArtifactKind::Spec
        );
        assert!("unknown".parse::<ArtifactKind>().is_err());
    }

    #[test]
    fn validation_report_counts_errors_and_warnings() {
        let mut report = ArtifactValidationReport::new(ArtifactKind::Description);
        report.push(ArtifactValidationDiagnostic::warning(
            ArtifactKind::Description,
            ArtifactDiagnosticCode::InvalidArtifactPath,
            Some("description.txt".to_string()),
            "non-canonical path",
        ));
        report.push(ArtifactValidationDiagnostic::error(
            ArtifactKind::Description,
            ArtifactDiagnosticCode::MissingSection,
            Some("## Goal".to_string()),
            "missing section",
        ));

        assert!(!report.is_valid());
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.warning_count(), 1);
        assert!(report.into_result().is_err());
    }

    #[test]
    fn validates_description_markdown_sections() {
        let report = validate_artifact_text(
            ArtifactKind::Description,
            r#"# Description

## Goal
Ship the feature.

## Context
Existing project context.

## Requirements
- Must be testable.

## Out of Scope
- Deployment.
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn reports_missing_markdown_sections() {
        let report = validate_artifact_text(ArtifactKind::Description, "## Goal\nOnly goal.");

        assert!(!report.is_valid());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == ArtifactDiagnosticCode::MissingSection)
        );
    }

    #[test]
    fn validates_adr_toml_frontmatter() {
        let report = validate_artifact(
            ArtifactKind::Adr,
            Some(Path::new("docs/adr/0001-record-choice.md")),
            r#"+++
status = "accepted"
deciders = ["core team"]
date = "2026-05-11"
+++

# ADR 0001: Record choice

## Status
Accepted.

## Context
We need a decision.

## Decision
Use the contract module.

## Alternatives considered
- Keep conventions implicit.

## Consequences
- Validators can share diagnostics.
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn validates_flow_schema_version_without_graph_validation() {
        let report = validate_artifact_text(
            ArtifactKind::Flow,
            r#"schema_version = 2
start = "missing"
edges = []

[metadata]
name = "Invalid"

[nodes]
"#,
        );

        assert_eq!(report.error_count(), 1);
        assert_eq!(
            report.diagnostics[0].code,
            ArtifactDiagnosticCode::UnsupportedSchemaVersion
        );
    }

    #[test]
    fn validates_spec_markdown_acceptance_criteria() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.md")),
            r#"# Spec

## Goal
Build a validator.

## Subtasks
- Add the pure core module.

## Acceptance Criteria
- [ ] Invalid schema versions produce diagnostics.
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn validates_spec_toml_when_every_subtask_has_acceptance_criteria() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1

[spec]
subtasks = [
  { id = "one", acceptance_criteria = ["first check passes"] },
  { id = "two", acceptance_criteria = ["second check passes"] },
]
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn rejects_spec_toml_when_any_subtask_lacks_acceptance_criteria() {
        let report = validate_artifact(
            ArtifactKind::Spec,
            Some(Path::new("spec.toml")),
            r#"schema_version = 1
acceptance_criteria = ["global criteria must not mask per-subtask gaps"]

[spec]
subtasks = [
  { id = "one", acceptance_criteria = ["first check passes"] },
  { id = "two" },
]
"#,
        );

        assert!(!report.is_valid());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == ArtifactDiagnosticCode::MissingAcceptanceCriteria
                && diagnostic.location.as_deref() == Some("spec.subtasks.acceptance_criteria")
        }));
    }

    #[test]
    fn path_patterns_accept_expected_locations() {
        assert!(contract_for(ArtifactKind::Adr).accepts_path(Path::new("docs/adr/0001-choice.md")));
        assert!(contract_for(ArtifactKind::Story).accepts_path(Path::new("stories/story-001.md")));
        assert!(!contract_for(ArtifactKind::Story).accepts_path(Path::new("stories/story-1.md")));
    }
}
