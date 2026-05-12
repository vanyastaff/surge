//! Roadmap patch artifact validation, including cross-context reference checks.
//!
//! TODO(refactor): if this file grows further, consider splitting context
//! validation into its own submodule (e.g. `roadmap_patch/context.rs`).

use crate::roadmap::RoadmapArtifact;
use crate::roadmap_patch::{
    InsertionPoint, RoadmapItemRef, RoadmapPatch, RoadmapPatchItem, RoadmapPatchOperation,
    RoadmapPatchValidationCode, RoadmapPatchValidationIssue,
};

use super::super::contract::ArtifactKind;
use super::super::diagnostic::{
    ArtifactDiagnosticCode, ArtifactValidationDiagnostic, ArtifactValidationReport,
};
use super::super::parse::validate_toml_artifact;

pub(in crate::artifact_contract) fn validate_roadmap_patch(
    report: &mut ArtifactValidationReport,
    content: &str,
) {
    if validate_toml_artifact(report, content, &["id", "target", "operations"]).is_none() {
        return;
    }

    let Some(patch) = parse_roadmap_patch(report, content) else {
        return;
    };
    for issue in patch.validate_shape() {
        report.push(roadmap_patch_issue_to_diagnostic(issue));
    }
}

pub(in crate::artifact_contract) fn parse_roadmap_patch_for_context(
    report: &mut ArtifactValidationReport,
    content: &str,
) -> Option<RoadmapPatch> {
    if report.is_valid() {
        parse_roadmap_patch(report, content)
    } else {
        None
    }
}

pub(in crate::artifact_contract) fn validate_roadmap_patch_context(
    report: &mut ArtifactValidationReport,
    patch: &RoadmapPatch,
    roadmap: &RoadmapArtifact,
) {
    let introduced = introduced_refs(patch);
    for (index, operation) in patch.operations.iter().enumerate() {
        validate_operation_references(report, roadmap, &introduced, index, operation);
    }
    for (index, dependency) in patch.dependencies.iter().enumerate() {
        validate_context_ref(
            report,
            roadmap,
            &introduced,
            &dependency.from,
            format!("dependencies[{index}].from"),
        );
        validate_context_ref(
            report,
            roadmap,
            &introduced,
            &dependency.to,
            format!("dependencies[{index}].to"),
        );
    }
}

fn parse_roadmap_patch(
    report: &mut ArtifactValidationReport,
    content: &str,
) -> Option<RoadmapPatch> {
    match toml::from_str::<RoadmapPatch>(content) {
        Ok(patch) => Some(patch),
        Err(error) => {
            report.push(ArtifactValidationDiagnostic::error(
                ArtifactKind::RoadmapPatch,
                ArtifactDiagnosticCode::InvalidToml,
                None,
                format!("roadmap patch failed to parse: {error}"),
            ));
            None
        },
    }
}

fn roadmap_patch_issue_to_diagnostic(
    issue: RoadmapPatchValidationIssue,
) -> ArtifactValidationDiagnostic {
    ArtifactValidationDiagnostic::error(
        ArtifactKind::RoadmapPatch,
        roadmap_patch_code_to_artifact_code(issue.code),
        Some(issue.location),
        issue.message,
    )
}

const fn roadmap_patch_code_to_artifact_code(
    code: RoadmapPatchValidationCode,
) -> ArtifactDiagnosticCode {
    match code {
        RoadmapPatchValidationCode::UnsupportedSchemaVersion => {
            ArtifactDiagnosticCode::UnsupportedSchemaVersion
        },
        RoadmapPatchValidationCode::MissingOperation => ArtifactDiagnosticCode::MissingOperation,
        RoadmapPatchValidationCode::MissingInsertionPoint => {
            ArtifactDiagnosticCode::MissingInsertionPoint
        },
        RoadmapPatchValidationCode::MissingTargetReference
        | RoadmapPatchValidationCode::MissingTitle
        | RoadmapPatchValidationCode::MissingConflictMessage
        | RoadmapPatchValidationCode::MissingConflictChoice => ArtifactDiagnosticCode::MissingField,
    }
}

fn introduced_refs(patch: &RoadmapPatch) -> Vec<RoadmapItemRef> {
    let mut refs = Vec::new();
    for operation in &patch.operations {
        match operation {
            RoadmapPatchOperation::AddMilestone { milestone, .. } => {
                refs.push(RoadmapItemRef::Milestone {
                    milestone_id: milestone.id.clone(),
                });
                refs.extend(milestone.tasks.iter().map(|task| RoadmapItemRef::Task {
                    milestone_id: milestone.id.clone(),
                    task_id: task.id.clone(),
                }));
            },
            RoadmapPatchOperation::AddTask {
                milestone_id, task, ..
            } => refs.push(RoadmapItemRef::Task {
                milestone_id: milestone_id.clone(),
                task_id: task.id.clone(),
            }),
            RoadmapPatchOperation::ReplaceDraftItem {
                target,
                replacement,
                ..
            } => {
                push_replacement_ref(&mut refs, target, replacement);
            },
        }
    }
    refs
}

fn push_replacement_ref(
    refs: &mut Vec<RoadmapItemRef>,
    target: &RoadmapItemRef,
    replacement: &RoadmapPatchItem,
) {
    match (target, replacement) {
        (_, RoadmapPatchItem::Milestone { milestone }) => refs.push(RoadmapItemRef::Milestone {
            milestone_id: milestone.id.clone(),
        }),
        (
            RoadmapItemRef::Task {
                milestone_id,
                task_id: _,
            },
            RoadmapPatchItem::Task { task },
        ) => refs.push(RoadmapItemRef::Task {
            milestone_id: milestone_id.clone(),
            task_id: task.id.clone(),
        }),
        _ => {},
    }
}

fn validate_operation_references(
    report: &mut ArtifactValidationReport,
    roadmap: &RoadmapArtifact,
    introduced: &[RoadmapItemRef],
    index: usize,
    operation: &RoadmapPatchOperation,
) {
    match operation {
        RoadmapPatchOperation::AddMilestone { insertion, .. }
        | RoadmapPatchOperation::AddTask { insertion, .. } => {
            if let Some(insertion) = insertion {
                validate_insertion_context(report, roadmap, introduced, index, insertion);
            }
        },
        RoadmapPatchOperation::ReplaceDraftItem { target, .. } => {
            validate_context_ref(
                report,
                roadmap,
                introduced,
                target,
                format!("operations[{index}].target"),
            );
        },
    }
}

fn validate_insertion_context(
    report: &mut ArtifactValidationReport,
    roadmap: &RoadmapArtifact,
    introduced: &[RoadmapItemRef],
    index: usize,
    insertion: &InsertionPoint,
) {
    match insertion {
        InsertionPoint::AppendToRoadmap => {},
        InsertionPoint::BeforeMilestone { milestone_id }
        | InsertionPoint::AfterMilestone { milestone_id }
        | InsertionPoint::AppendToMilestone { milestone_id } => {
            let reference = RoadmapItemRef::Milestone {
                milestone_id: milestone_id.clone(),
            };
            validate_context_ref(
                report,
                roadmap,
                introduced,
                &reference,
                format!("operations[{index}].insertion"),
            );
        },
        InsertionPoint::BeforeTask {
            milestone_id,
            task_id,
        }
        | InsertionPoint::AfterTask {
            milestone_id,
            task_id,
        } => {
            let reference = RoadmapItemRef::Task {
                milestone_id: milestone_id.clone(),
                task_id: task_id.clone(),
            };
            validate_context_ref(
                report,
                roadmap,
                introduced,
                &reference,
                format!("operations[{index}].insertion"),
            );
        },
    }
}

fn validate_context_ref(
    report: &mut ArtifactValidationReport,
    roadmap: &RoadmapArtifact,
    introduced: &[RoadmapItemRef],
    reference: &RoadmapItemRef,
    location: String,
) {
    if roadmap_contains_ref(roadmap, reference) || introduced.contains(reference) {
        return;
    }

    report.push(ArtifactValidationDiagnostic::error(
        ArtifactKind::RoadmapPatch,
        ArtifactDiagnosticCode::InvalidReference,
        Some(location),
        "roadmap patch references an item not present in the supplied roadmap context",
    ));
}

fn roadmap_contains_ref(roadmap: &RoadmapArtifact, reference: &RoadmapItemRef) -> bool {
    match reference {
        RoadmapItemRef::Milestone { milestone_id } => roadmap
            .milestones
            .iter()
            .any(|milestone| milestone.id == *milestone_id),
        RoadmapItemRef::Task {
            milestone_id,
            task_id,
        } => roadmap.milestones.iter().any(|milestone| {
            milestone.id == *milestone_id && milestone.tasks.iter().any(|task| task.id == *task_id)
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::roadmap::RoadmapArtifact;

    use super::super::super::contract::ArtifactKind;
    use super::super::super::diagnostic::ArtifactDiagnosticCode;
    use super::super::super::{validate_artifact, validate_roadmap_patch_text_with_context};

    #[test]
    fn validates_roadmap_patch_toml_shape() {
        let report = validate_artifact(
            ArtifactKind::RoadmapPatch,
            Some(Path::new("roadmap-patch.toml")),
            r#"schema_version = 1
id = "rpatch-demo"
rationale = "Add follow-up feature work."
status = "drafted"

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"

[[operations]]
op = "add_milestone"

[operations.milestone]
id = "m2"
title = "Follow-up feature"

[operations.insertion]
kind = "append_to_roadmap"
"#,
        );

        assert!(report.is_valid(), "{report:#?}");
    }

    #[test]
    fn rejects_roadmap_patch_without_operations() {
        let report = validate_artifact(
            ArtifactKind::RoadmapPatch,
            Some(Path::new("roadmap-patch.toml")),
            r#"schema_version = 1
id = "rpatch-empty"
operations = []

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"
"#,
        );

        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code == ArtifactDiagnosticCode::MissingOperation })
        );
    }

    #[test]
    fn validates_roadmap_patch_references_against_context() {
        let roadmap = RoadmapArtifact::new(vec![crate::roadmap::RoadmapMilestone::new(
            "m1", "Existing",
        )]);
        let report = validate_roadmap_patch_text_with_context(
            r#"schema_version = 1
id = "rpatch-context"

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"

[[operations]]
op = "add_task"
milestone_id = "missing"

[operations.task]
id = "m1-t2"
title = "New task"

[operations.insertion]
kind = "append_to_milestone"
milestone_id = "missing"
"#,
            &roadmap,
        );

        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code == ArtifactDiagnosticCode::InvalidReference })
        );
    }

    #[test]
    fn replacement_task_introduces_ref_with_target_milestone_context() {
        let mut milestone = crate::roadmap::RoadmapMilestone::new("m1", "Existing");
        milestone
            .tasks
            .push(crate::roadmap::RoadmapTask::new("m1-t1", "Old task"));
        let roadmap = RoadmapArtifact::new(vec![milestone]);

        let report = validate_roadmap_patch_text_with_context(
            r#"schema_version = 1
id = "rpatch-replace-ref"

[target]
kind = "project_roadmap"
roadmap_path = ".ai-factory/ROADMAP.md"

[[operations]]
op = "replace_draft_item"
reason = "rename draft task"

[operations.target]
kind = "task"
milestone_id = "m1"
task_id = "m1-t1"

[operations.replacement]
kind = "task"

[operations.replacement.task]
id = "m1-t2"
title = "New task"

[[dependencies]]
reason = "new task depends on old milestone"

[dependencies.from]
kind = "task"
milestone_id = "m1"
task_id = "m1-t2"

[dependencies.to]
kind = "milestone"
milestone_id = "m1"
"#,
            &roadmap,
        );

        assert!(report.is_valid(), "{report:#?}");
    }
}
