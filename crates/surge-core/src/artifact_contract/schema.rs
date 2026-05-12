//! Self-describing contract introspection: markdown outlines, JSON Schema
//! export, and contract summaries.

use serde::Serialize;

use crate::roadmap::RoadmapArtifact;
use crate::roadmap_patch::RoadmapPatch;
use crate::spec::SpecArtifact;

use super::contract::{
    ARTIFACT_SCHEMA_VERSION, ArtifactFormat, ArtifactKind, contract_for, schema_version_for_kind,
};

/// Required markdown headings (level-2) for markdown-format artifacts.
///
/// Returns `Some(&[...])` for kinds whose primary format is Markdown — the
/// returned slice is the same set of `## <Section>` headings the validator
/// requires. Returns `None` for kinds whose canonical contract is *not*
/// a markdown outline:
///
/// - `spec`, `roadmap`, `roadmap-patch`: the contract is captured by the
///   JSON Schema returned from [`json_schema_for`].
/// - `flow`: there is currently no exported JSON Schema; the contract is
///   enforced by [`crate::graph::Graph`] in Rust plus engine-level
///   validation. Schema export for flow is tracked separately.
#[must_use]
pub const fn markdown_outline(kind: ArtifactKind) -> Option<&'static [&'static str]> {
    match kind {
        ArtifactKind::Description => Some(&["Goal", "Context", "Requirements", "Out of Scope"]),
        ArtifactKind::Requirements => Some(&[
            "Overview",
            "User Stories",
            "Functional Requirements",
            "Success Criteria",
            "Out of Scope",
        ]),
        ArtifactKind::Adr => Some(&[
            "Status",
            "Context",
            "Decision",
            "Alternatives considered",
            "Consequences",
        ]),
        ArtifactKind::Story => Some(&[
            "Context",
            "What needs to be done",
            "Architecture decisions",
            "Files to modify",
            "Acceptance criteria",
            "Out of scope",
        ]),
        ArtifactKind::Plan => Some(&["Settings", "Tasks"]),
        ArtifactKind::Roadmap
        | ArtifactKind::RoadmapPatch
        | ArtifactKind::Spec
        | ArtifactKind::Flow => None,
    }
}

/// Stable `$id` prefix for exported JSON Schemas.
const SCHEMA_ID_PREFIX: &str = "https://surge.dev/schema/v1";

/// Export the JSON Schema (draft 2020-12) for `kind`, when one is available.
///
/// Returns `Some(schema)` for kinds whose primary format is TOML and which
/// Surge owns as a typed artifact contract (`spec`, `roadmap`, `roadmap-patch`),
/// plus an inline schema for the ADR TOML frontmatter. Returns `None` for
/// markdown-only kinds (`description`, `requirements`, `story`, `plan`) and
/// for `flow` — the latter is currently described by [`crate::graph::Graph`]
/// in Rust and tracked separately for schema export.
#[must_use]
pub fn json_schema_for(kind: ArtifactKind) -> Option<serde_json::Value> {
    match kind {
        ArtifactKind::Spec => Some(schema_value::<SpecArtifact>("spec.json")),
        ArtifactKind::Roadmap => Some(schema_value::<RoadmapArtifact>("roadmap.json")),
        ArtifactKind::RoadmapPatch => Some(schema_value::<RoadmapPatch>("roadmap-patch.json")),
        ArtifactKind::Adr => Some(adr_frontmatter_schema()),
        ArtifactKind::Flow
        | ArtifactKind::Description
        | ArtifactKind::Requirements
        | ArtifactKind::Story
        | ArtifactKind::Plan => None,
    }
}

fn schema_value<T: schemars::JsonSchema>(filename: &str) -> serde_json::Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema)
        .expect("schemars::Schema is JSON-serializable by construction");
    if let Some(object) = value.as_object_mut() {
        ensure_schema_version_required(object);
        object.insert(
            "$id".to_string(),
            serde_json::Value::String(format!("{SCHEMA_ID_PREFIX}/{filename}")),
        );
        object.insert(
            "x-surge-schema-version".to_string(),
            serde_json::Value::Number(ARTIFACT_SCHEMA_VERSION.into()),
        );
    }
    value
}

/// Force `schema_version` into the root `required` array.
///
/// Surge-owned TOML artifacts (`spec`, `roadmap`, `roadmap-patch`) keep
/// `#[serde(default = "...")]` on `schema_version` so that legacy in-memory
/// constructors do not have to thread the constant — but the on-disk
/// contract treats a missing `schema_version` as an error (see
/// [`super::parse::validate_schema_version`]). Without this post-processing
/// step, schemars would mark the field optional and external validators would
/// accept artifacts that the orchestrator later rejects.
fn ensure_schema_version_required(object: &mut serde_json::Map<String, serde_json::Value>) {
    let has_property = object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|properties| properties.contains_key("schema_version"));
    if !has_property {
        return;
    }
    let required = object
        .entry("required".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let Some(array) = required.as_array_mut() else {
        return;
    };
    let already_present = array
        .iter()
        .any(|value| value.as_str() == Some("schema_version"));
    if !already_present {
        array.insert(0, serde_json::Value::String("schema_version".to_string()));
    }
}

fn adr_frontmatter_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": format!("{SCHEMA_ID_PREFIX}/adr-frontmatter.json"),
        "title": "AdrFrontmatter",
        "description": "Required TOML frontmatter fields for an ADR markdown file. The full ADR body is markdown-only; see `markdown_outline(ArtifactKind::Adr)` for the required body sections.",
        "type": "object",
        "required": ["status", "deciders", "date"],
        "properties": {
            "status": {
                "type": "string",
                "description": "ADR lifecycle status, for example `proposed`, `accepted`, `superseded`."
            },
            "deciders": {
                "type": "array",
                "description": "Humans or teams who own this decision.",
                "items": { "type": "string", "minLength": 1 },
                "minItems": 1
            },
            "date": {
                "type": "string",
                "description": "ISO-8601 calendar date the decision was recorded (`YYYY-MM-DD`).",
                "pattern": r"^\d{4}-\d{2}-\d{2}$"
            }
        },
        "additionalProperties": true,
        "x-surge-schema-version": ARTIFACT_SCHEMA_VERSION
    })
}

/// Self-describing summary for one artifact contract.
///
/// Convenience aggregate for prompt generators and external tooling that wants
/// a single call to introspect a kind: canonical path, primary format,
/// required markdown sections (when applicable), and the JSON Schema (when
/// available).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContractSummary {
    /// Artifact family.
    pub kind: ArtifactKind,
    /// Canonical path or path pattern relative to the run worktree.
    pub canonical_path: &'static str,
    /// Primary representation agents should produce.
    pub primary_format: ArtifactFormat,
    /// Markdown sections that must be present, when applicable.
    pub required_markdown_sections: Option<&'static [&'static str]>,
    /// JSON Schema describing the on-disk format, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<serde_json::Value>,
    /// Current `schema_version` for the kind.
    pub schema_version: u32,
}

/// Build the [`ContractSummary`] for `kind`.
#[must_use]
pub fn contract_summary(kind: ArtifactKind) -> ContractSummary {
    let contract = contract_for(kind);
    ContractSummary {
        kind,
        canonical_path: contract.canonical_path,
        primary_format: contract.primary_format,
        required_markdown_sections: markdown_outline(kind),
        json_schema: json_schema_for(kind),
        schema_version: schema_version_for_kind(kind),
    }
}
