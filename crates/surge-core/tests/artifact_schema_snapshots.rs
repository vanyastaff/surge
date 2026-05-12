//! Snapshot tests for JSON Schemas exported by the artifact contract module.
//!
//! Snapshots capture the current canonical shape of every TOML-format artifact
//! contract that Surge exposes. Any breaking schema change will surface as a
//! diff here, so external prompt profiles and validators can be updated in
//! lockstep.
//!
//! Run: `cargo test -p surge-core --test artifact_schema_snapshots`
//! Accept new snapshots after intentional changes: `cargo insta accept`.

use surge_core::{ArtifactKind, contract_summary, json_schema_for, markdown_outline};

#[test]
fn spec_json_schema_is_stable() {
    let schema = json_schema_for(ArtifactKind::Spec).expect("spec exports a JSON schema");
    insta::assert_json_snapshot!("spec_json_schema", schema);
}

#[test]
fn roadmap_json_schema_is_stable() {
    let schema = json_schema_for(ArtifactKind::Roadmap).expect("roadmap exports a JSON schema");
    insta::assert_json_snapshot!("roadmap_json_schema", schema);
}

#[test]
fn roadmap_patch_json_schema_is_stable() {
    let schema =
        json_schema_for(ArtifactKind::RoadmapPatch).expect("roadmap-patch exports a JSON schema");
    insta::assert_json_snapshot!("roadmap_patch_json_schema", schema);
}

#[test]
fn adr_frontmatter_schema_is_stable() {
    let schema = json_schema_for(ArtifactKind::Adr).expect("ADR exports a frontmatter schema");
    insta::assert_json_snapshot!("adr_frontmatter_json_schema", schema);
}

#[test]
fn markdown_only_kinds_have_no_json_schema() {
    for kind in [
        ArtifactKind::Description,
        ArtifactKind::Requirements,
        ArtifactKind::Story,
        ArtifactKind::Plan,
    ] {
        assert!(
            json_schema_for(kind).is_none(),
            "{kind} should not expose a JSON schema (markdown-only contract)"
        );
        assert!(
            markdown_outline(kind).is_some(),
            "{kind} should expose a markdown outline"
        );
    }
}

#[test]
fn flow_currently_has_no_exported_schema() {
    // Flow is described by [`surge_core::Graph`] in Rust. Schema export for
    // flow is tracked separately; until then `json_schema_for` returns None
    // and `markdown_outline` returns None (flow has no markdown body).
    assert!(json_schema_for(ArtifactKind::Flow).is_none());
    assert!(markdown_outline(ArtifactKind::Flow).is_none());
}

#[test]
fn contract_summary_combines_schema_and_outline() {
    let spec = contract_summary(ArtifactKind::Spec);
    assert_eq!(spec.kind, ArtifactKind::Spec);
    assert_eq!(spec.canonical_path, "spec.toml");
    assert!(spec.json_schema.is_some());
    assert!(spec.required_markdown_sections.is_none());

    let plan = contract_summary(ArtifactKind::Plan);
    assert_eq!(plan.kind, ArtifactKind::Plan);
    assert!(plan.json_schema.is_none());
    let sections = plan
        .required_markdown_sections
        .expect("plan must expose markdown outline");
    assert_eq!(sections, &["Settings", "Tasks"]);
}

#[test]
fn exported_schemas_carry_surge_id_and_version() {
    for kind in [
        ArtifactKind::Spec,
        ArtifactKind::Roadmap,
        ArtifactKind::RoadmapPatch,
        ArtifactKind::Adr,
    ] {
        let schema = json_schema_for(kind).expect("kind exports a schema");
        let object = schema.as_object().expect("schema is a JSON object");
        let id = object
            .get("$id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{kind} schema missing $id"));
        assert!(
            id.starts_with("https://surge.dev/schema/v1/"),
            "{kind} $id should start with the canonical Surge namespace, got {id}"
        );
        let version = object
            .get("x-surge-schema-version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_else(|| panic!("{kind} schema missing x-surge-schema-version"));
        assert_eq!(version, u64::from(surge_core::ARTIFACT_SCHEMA_VERSION));
    }
}
