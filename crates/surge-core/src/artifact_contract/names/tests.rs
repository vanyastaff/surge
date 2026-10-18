use std::path::PathBuf;

use super::*;
use crate::content_hash::ContentHash;
use crate::keys::NodeKey;

#[test]
fn every_producer_spelling_resolves_to_its_kind_and_format() {
    use ArtifactFormat::{FlowToml, Markdown, Toml};
    use ArtifactKind::{Description, Flow, Roadmap, RoadmapPatch};
    let table: &[(&str, ArtifactKind, Option<ArtifactFormat>)] = &[
        ("roadmap", Roadmap, None),
        ("roadmap.toml", Roadmap, Some(Toml)),
        ("roadmap-toml", Roadmap, Some(Toml)),
        ("roadmap_toml", Roadmap, Some(Toml)),
        ("ROADMAP_TOML", Roadmap, Some(Toml)),
        ("roadmap.md", Roadmap, Some(Markdown)),
        ("roadmap-md", Roadmap, Some(Markdown)),
        ("roadmap_md", Roadmap, Some(Markdown)),
        ("roadmap_patch", RoadmapPatch, None),
        ("roadmap-patch-toml", RoadmapPatch, Some(Toml)),
        ("roadmap_patch.toml", RoadmapPatch, Some(Toml)),
        ("description", Description, None),
        ("description_md", Description, Some(Markdown)),
        ("flow", Flow, None),
        ("flow.toml", Flow, Some(FlowToml)),
        ("flow_toml", Flow, Some(FlowToml)),
    ];
    for (name, kind, format) in table {
        assert_eq!(
            artifact_name(name),
            Some(ArtifactName {
                kind: *kind,
                format: *format
            }),
            "{name}"
        );
    }
    for unknown in ["user_prompt", "project_context", "roadmapx", "notes.md"] {
        assert_eq!(artifact_name(unknown), None, "{unknown}");
    }
}

#[test]
fn artifact_kind_parsing_delegates_to_the_name_authority() {
    for name in ["roadmap_toml", "roadmap-toml", "roadmap.toml", "roadmap"] {
        assert_eq!(name.parse::<ArtifactKind>().unwrap(), ArtifactKind::Roadmap);
    }
    assert!("roadmapx".parse::<ArtifactKind>().is_err());
}

fn artifact(name: &str, seq: u64, content: &str) -> ArtifactRef {
    ArtifactRef {
        hash: ContentHash::compute(content.as_bytes()),
        path: PathBuf::from(format!("{name}.out")),
        name: name.into(),
        produced_by: NodeKey::try_from("roadmap_planner").unwrap(),
        produced_at_seq: Some(
            crate::execution_recovery::semantic_position::SemanticPosition::local(seq).unwrap(),
        ),
    }
}

#[test]
fn selection_takes_the_latest_of_one_spelling_and_separates_formats() {
    let refs = [
        artifact("roadmap_toml", 3, "old"),
        artifact("roadmap_toml", 9, "new"),
        artifact("roadmap_md", 9, "human"),
    ];
    let toml = select_canonical(&refs, ArtifactKind::Roadmap, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(
        toml.produced_at_seq,
        Some(crate::execution_recovery::semantic_position::SemanticPosition::local(9).unwrap())
    );
    assert_eq!(toml.name, "roadmap_toml");
    let markdown = select_canonical(
        &refs,
        ArtifactKind::Roadmap,
        Some(ArtifactFormat::Markdown),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(markdown.name, "roadmap_md");
    assert_eq!(
        select_canonical(&refs, ArtifactKind::Description, None, None).unwrap(),
        None
    );
}

#[test]
fn competing_spellings_and_equal_sequences_are_ambiguous() {
    let competing = [
        artifact("roadmap", 4, "a"),
        artifact("roadmap-toml", 5, "b"),
    ];
    let error = select_canonical(&competing, ArtifactKind::Roadmap, None, None).unwrap_err();
    let ArtifactSelectionError::Ambiguous(error) = error else {
        panic!("expected name ambiguity")
    };
    assert_eq!(error.names, vec!["roadmap", "roadmap-toml"]);
    let same_seq = [artifact("roadmap", 4, "a"), artifact("roadmap", 4, "b")];
    assert!(select_canonical(&same_seq, ArtifactKind::Roadmap, None, None).is_err());
    // The identical event read twice is not a conflict.
    let duplicate = [artifact("roadmap", 4, "a"), artifact("roadmap", 4, "a")];
    assert!(select_canonical(&duplicate, ArtifactKind::Roadmap, None, None).is_ok());
}

#[test]
fn seed_paths_come_from_the_contract_not_the_content() {
    assert_eq!(
        canonical_seed_path(ArtifactKind::Roadmap, None),
        Some(PathBuf::from("roadmap.toml"))
    );
    assert_eq!(
        canonical_seed_path(ArtifactKind::Roadmap, Some(ArtifactFormat::Markdown)),
        Some(PathBuf::from("roadmap.md"))
    );
    assert_eq!(
        canonical_seed_path(ArtifactKind::Description, None),
        Some(PathBuf::from("description.md"))
    );
    assert_eq!(
        canonical_seed_path(ArtifactKind::Flow, Some(ArtifactFormat::FlowToml)),
        Some(PathBuf::from("flow.toml"))
    );
    assert_eq!(canonical_seed_path(ArtifactKind::Adr, None), None);
}
