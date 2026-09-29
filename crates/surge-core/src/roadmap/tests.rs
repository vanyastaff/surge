use super::*;
use crate::id::SpecId;

/// Guards [`RoadmapStatus::ALL`]: this match has no wildcard arm, so a
/// ninth `RoadmapStatus` variant fails to compile here (and only here)
/// until it is added both to this match and to `ALL` alongside it — see
/// `ALL`'s own doc comment for why that pairing is deliberate.
#[test]
fn all_variants_are_named() {
    fn name(status: RoadmapStatus) -> &'static str {
        match status {
            RoadmapStatus::Pending => "pending",
            RoadmapStatus::Running => "running",
            RoadmapStatus::Paused => "paused",
            RoadmapStatus::ReadyForVerification => "ready_for_verification",
            RoadmapStatus::FailedVerification => "failed_verification",
            RoadmapStatus::Completed => "completed",
            RoadmapStatus::Failed => "failed",
            RoadmapStatus::Skipped => "skipped",
        }
    }
    assert_eq!(RoadmapStatus::ALL.len(), 8);
    for status in RoadmapStatus::ALL {
        assert!(!name(status).is_empty());
    }
}

#[test]
fn verification_report_defaults_to_v1_not_roadmap_v2() {
    // A verifier writes verification-report.toml with no schema_version.
    // It must default to ARTIFACT_SCHEMA_VERSION (1) — the version its
    // contract validator requires — not the roadmap's bumped v2.
    let toml = "task_id = \"m1-t1\"\noutcome = \"passed\"\nsummary = \"all green\"\n";
    let report: VerificationReportArtifact = toml::from_str(toml).unwrap();
    assert_eq!(report.schema_version, ARTIFACT_SCHEMA_VERSION);
    assert_ne!(report.schema_version, ROADMAP_SCHEMA_VERSION);
}

fn make_item(title: &str, status: RoadmapStatus) -> RoadmapItem {
    RoadmapItem {
        spec_id: SpecId::new(),
        title: title.to_string(),
        complexity: Complexity::Standard,
        priority: Priority::Medium,
        depends_on: vec![],
        status,
    }
}

#[test]
fn test_timeline_total_items() {
    let timeline = Timeline {
        batches: vec![
            TimelineBatch {
                order: 0,
                items: vec![
                    make_item("A", RoadmapStatus::Pending),
                    make_item("B", RoadmapStatus::Pending),
                ],
                reason: String::new(),
            },
            TimelineBatch {
                order: 1,
                items: vec![make_item("C", RoadmapStatus::Pending)],
                reason: String::new(),
            },
        ],
    };
    assert_eq!(timeline.total_items(), 3);
}

#[test]
fn test_timeline_count_by_status() {
    let timeline = Timeline {
        batches: vec![TimelineBatch {
            order: 0,
            items: vec![
                make_item("A", RoadmapStatus::Completed),
                make_item("B", RoadmapStatus::Pending),
                make_item("C", RoadmapStatus::Completed),
            ],
            reason: String::new(),
        }],
    };
    assert_eq!(timeline.count_by_status(RoadmapStatus::Completed), 2);
    assert_eq!(timeline.count_by_status(RoadmapStatus::Pending), 1);
}

#[test]
fn test_timeline_next_ready_batch() {
    let timeline = Timeline {
        batches: vec![
            TimelineBatch {
                order: 0,
                items: vec![make_item("A", RoadmapStatus::Completed)],
                reason: String::new(),
            },
            TimelineBatch {
                order: 1,
                items: vec![make_item("B", RoadmapStatus::Pending)],
                reason: String::new(),
            },
        ],
    };
    let batch = timeline.next_ready_batch().unwrap();
    assert_eq!(batch.order, 1);
}

#[test]
fn test_timeline_next_ready_batch_none_when_running() {
    let timeline = Timeline {
        batches: vec![TimelineBatch {
            order: 0,
            items: vec![make_item("A", RoadmapStatus::Running)],
            reason: String::new(),
        }],
    };
    assert!(timeline.next_ready_batch().is_none());
}

#[test]
fn test_timeline_next_ready_batch_none_when_all_done() {
    let timeline = Timeline {
        batches: vec![TimelineBatch {
            order: 0,
            items: vec![make_item("A", RoadmapStatus::Completed)],
            reason: String::new(),
        }],
    };
    assert!(timeline.next_ready_batch().is_none());
}

#[test]
fn test_roadmap_status_is_terminal() {
    assert!(RoadmapStatus::Completed.is_terminal());
    assert!(RoadmapStatus::Failed.is_terminal());
    assert!(RoadmapStatus::Skipped.is_terminal());
    assert!(!RoadmapStatus::Pending.is_terminal());
    assert!(!RoadmapStatus::Running.is_terminal());
    assert!(!RoadmapStatus::Paused.is_terminal());
}

#[test]
fn test_priority_display() {
    assert_eq!(Priority::Critical.to_string(), "critical");
    assert_eq!(Priority::High.to_string(), "high");
    assert_eq!(Priority::Medium.to_string(), "medium");
    assert_eq!(Priority::Low.to_string(), "low");
}

#[test]
fn test_roadmap_item_toml_roundtrip() {
    let item = make_item("Test item", RoadmapStatus::Pending);
    let toml_str = toml::to_string(&item).unwrap();
    let deserialized: RoadmapItem = toml::from_str(&toml_str).unwrap();
    assert_eq!(deserialized.title, "Test item");
    assert_eq!(deserialized.status, RoadmapStatus::Pending);
    assert_eq!(deserialized.priority, Priority::Medium);
}

#[test]
fn test_roadmap_artifact_toml_roundtrip() {
    let mut milestone = RoadmapMilestone::new("m1", "Artifact contracts");
    let mut task = RoadmapTask::new("m1-t1", "Define schema");
    task.acceptance_criteria
        .push("schema_version is present".to_string());
    milestone.tasks.push(task);

    let mut artifact = RoadmapArtifact::new(vec![milestone]);
    artifact.dependencies.push(RoadmapDependency {
        from: "m1".into(),
        to: "m2".into(),
        reason: "contracts unblock validators".to_string(),
    });
    artifact.risks.push(RoadmapRisk {
        description: "legacy markdown drift".to_string(),
        mitigation: Some("keep compatibility docs".to_string()),
    });

    let toml_str = toml::to_string(&artifact).unwrap();
    let deserialized: RoadmapArtifact = toml::from_str(&toml_str).unwrap();

    assert_eq!(deserialized.schema_version, ROADMAP_SCHEMA_VERSION);
    assert_eq!(deserialized.milestones[0].id, "m1");
    assert_eq!(deserialized.milestones[0].tasks[0].id, "m1-t1");
    assert_eq!(deserialized.dependencies[0].to, "m2");
    assert_eq!(
        deserialized.risks[0].mitigation.as_deref(),
        Some("keep compatibility docs")
    );
}

#[test]
fn test_find_item_mut() {
    let spec_id = SpecId::new();
    let mut timeline = Timeline {
        batches: vec![TimelineBatch {
            order: 0,
            items: vec![RoadmapItem {
                spec_id,
                title: "Find me".to_string(),
                complexity: Complexity::Simple,
                priority: Priority::High,
                depends_on: vec![],
                status: RoadmapStatus::Pending,
            }],
            reason: String::new(),
        }],
    };

    let item = timeline.find_item_mut(spec_id).unwrap();
    item.status = RoadmapStatus::Running;
    assert_eq!(timeline.batches[0].items[0].status, RoadmapStatus::Running);
}

fn ledger_roadmap(tasks: Vec<RoadmapTask>) -> RoadmapArtifact {
    let mut milestone = RoadmapMilestone::new("m1", "Ledger");
    milestone.tasks = tasks;
    RoadmapArtifact::new(vec![milestone])
}

fn sized_task(id: &str, depends_on: &[&str]) -> RoadmapTask {
    let mut task = RoadmapTask::new(id, id);
    task.size = Some(TaskSize::M);
    task.depends_on = depends_on.iter().map(|&id| id.into()).collect();
    task
}

#[test]
fn validate_ledger_accepts_well_formed_v2() {
    let roadmap = ledger_roadmap(vec![
        sized_task("t1", &[]),
        sized_task("t2", &["t1"]),
        sized_task("t3", &["t1", "t2"]),
    ]);
    assert_eq!(roadmap.validate_ledger(), Vec::new());
}

#[test]
fn validate_ledger_flags_duplicate_ids() {
    let mut roadmap = ledger_roadmap(vec![sized_task("t1", &[]), sized_task("t1", &[])]);
    roadmap
        .milestones
        .push(RoadmapMilestone::new("m1", "Duplicate"));

    let issues = roadmap.validate_ledger();

    assert!(issues.contains(&RoadmapLedgerIssue::DuplicateTaskId { task: "t1".into() }));
    assert!(issues.contains(&RoadmapLedgerIssue::DuplicateMilestoneId {
        milestone: "m1".into()
    }));
}

#[test]
fn validate_ledger_flags_unknown_and_self_references() {
    let mut discovered = sized_task("t2", &["missing"]);
    discovered.discovered_from = Some("t2".into());
    let mut self_dep = sized_task("t1", &["t1"]);
    self_dep.discovered_from = Some("ghost".into());
    let roadmap = ledger_roadmap(vec![self_dep, discovered]);

    let issues = roadmap.validate_ledger();

    assert!(issues.contains(&RoadmapLedgerIssue::SelfDependency { task: "t1".into() }));
    assert!(issues.contains(&RoadmapLedgerIssue::UnknownDependsOn {
        task: "t2".into(),
        missing: "missing".into()
    }));
    assert!(issues.contains(&RoadmapLedgerIssue::SelfDiscovery { task: "t2".into() }));
    assert!(issues.contains(&RoadmapLedgerIssue::UnknownDiscoveredFrom {
        task: "t1".into(),
        missing: "ghost".into()
    }));
}

#[test]
fn validate_ledger_reports_cycle_deterministically() {
    let roadmap = ledger_roadmap(vec![
        sized_task("t1", &["t3"]),
        sized_task("t2", &["t1"]),
        sized_task("t3", &["t2"]),
    ]);

    let issues = roadmap.validate_ledger();

    assert_eq!(
        issues,
        vec![RoadmapLedgerIssue::DependencyCycle {
            cycle: vec!["t1".into(), "t3".into(), "t2".into(), "t1".into()]
        }]
    );
}

#[test]
fn validate_ledger_detects_deep_cycle_without_stack_overflow() {
    // A long linear chain t0 -> t1 -> ... -> t_{n-1} -> t0 forms one big
    // cycle far deeper than any recursion cap would allow. The iterative
    // DFS must both avoid a stack overflow and still report the cycle.
    let n = 20_000;
    let tasks: Vec<RoadmapTask> = (0..n)
        .map(|i| {
            let next = (i + 1) % n;
            let mut task = RoadmapTask::new(format!("t{i}"), format!("Task {i}"));
            task.size = Some(TaskSize::M);
            task.depends_on = vec![format!("t{next}").into()];
            task
        })
        .collect();
    let roadmap = ledger_roadmap(tasks);

    let issues = roadmap.validate_ledger();
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, RoadmapLedgerIssue::DependencyCycle { .. })),
        "deep cycle must be reported, got {issues:?}"
    );
}

#[test]
fn validate_ledger_requires_size_only_at_v2() {
    let mut roadmap = ledger_roadmap(vec![RoadmapTask::new("t1", "No size")]);
    assert_eq!(
        roadmap.validate_ledger(),
        vec![RoadmapLedgerIssue::MissingSize { task: "t1".into() }]
    );

    roadmap.schema_version = 1;
    assert_eq!(roadmap.validate_ledger(), Vec::new());
}

#[test]
fn validate_ledger_flags_unknown_milestone_dependency() {
    let mut roadmap = ledger_roadmap(vec![sized_task("t1", &[])]);
    roadmap.dependencies.push(RoadmapDependency {
        from: "m1".into(),
        to: "m9".into(),
        reason: String::new(),
    });

    assert_eq!(
        roadmap.validate_ledger(),
        vec![RoadmapLedgerIssue::UnknownMilestoneDependency {
            missing: "m9".into()
        }]
    );
}

#[test]
fn validate_ledger_flags_self_referencing_milestone_dependency() {
    let mut roadmap = ledger_roadmap(vec![sized_task("t1", &[])]);
    roadmap.dependencies.push(RoadmapDependency {
        from: "m1".into(),
        to: "m1".into(),
        reason: String::new(),
    });

    let issues = roadmap.validate_ledger();
    assert!(
        issues.contains(&RoadmapLedgerIssue::MilestoneSelfDependency {
            milestone: "m1".into()
        })
    );
}

#[test]
fn task_ledger_fields_roundtrip_via_toml() {
    let mut task = sized_task("t2", &["t1"]);
    task.discovered_from = Some("t1".into());
    task.verified = true;
    task.status = RoadmapStatus::ReadyForVerification;
    let roadmap = ledger_roadmap(vec![sized_task("t1", &[]), task]);

    let toml_str = toml::to_string(&roadmap).unwrap();
    let parsed: RoadmapArtifact = toml::from_str(&toml_str).unwrap();

    assert_eq!(parsed, roadmap);
    assert!(toml_str.contains("ready_for_verification"));
    assert!(toml_str.contains("size = \"m\""));
}

#[test]
fn v1_roadmap_toml_still_parses_with_defaulted_ledger_fields() {
    let parsed: RoadmapArtifact = toml::from_str(
        r#"
schema_version = 1

[[milestones]]
id = "m1"
title = "Legacy"

[[milestones.tasks]]
id = "t1"
title = "Old task"
"#,
    )
    .unwrap();

    let task = &parsed.milestones[0].tasks[0];
    assert_eq!(parsed.schema_version, 1);
    assert!(task.depends_on.is_empty());
    assert!(task.discovered_from.is_none());
    assert!(task.size.is_none());
    assert!(!task.verified);
    assert_eq!(parsed.validate_ledger(), Vec::new());
}

#[test]
fn to_markdown_preserves_running_checkbox_marker() {
    let mut milestone = RoadmapMilestone::new("m1", "Active");
    milestone
        .tasks
        .push(RoadmapTask::new("m1-t1", "Currently running"));
    milestone.tasks[0].status = RoadmapStatus::Running;
    let roadmap = RoadmapArtifact::new(vec![milestone]);

    let markdown = roadmap.to_markdown();

    assert!(markdown.contains("- [~] m1-t1: Currently running (running)"));
}

fn assertion(id: &str) -> ValidationAssertion {
    ValidationAssertion {
        id: id.into(),
        title: id.into(),
        pass_condition: format!("{id} observably holds"),
        evidence: Vec::new(),
    }
}

fn fulfilling(id: &str, fulfills: &[&str]) -> RoadmapTask {
    let mut task = sized_task(id, &[]);
    task.fulfills = fulfills.iter().map(|&id| id.into()).collect();
    task
}

/// Two missions over three milestones, every assertion claimed once.
fn mission_roadmap() -> RoadmapArtifact {
    let mut m1 = RoadmapMilestone::new("m1", "Accounts");
    m1.tasks = vec![
        fulfilling("m1-t1", &[]),
        fulfilling("m1-t2", &["VAL-AUTH-001"]),
    ];
    let mut m2 = RoadmapMilestone::new("m2", "Profiles");
    m2.tasks = vec![fulfilling("m2-t1", &["VAL-AUTH-002"])];
    let mut m3 = RoadmapMilestone::new("m3", "Billing");
    m3.tasks = vec![fulfilling("m3-t1", &["VAL-PAY-001"])];

    let mut accounts = RoadmapMission::new("mission-1", "Accounts", "Users can sign in");
    accounts.milestones = vec!["m1".into(), "m2".into()];
    accounts.validation_contract = vec![assertion("VAL-AUTH-001"), assertion("VAL-AUTH-002")];
    let mut billing = RoadmapMission::new("mission-2", "Billing", "Users can pay");
    billing.milestones = vec!["m3".into()];
    billing.validation_contract = vec![assertion("VAL-PAY-001")];

    let mut roadmap = RoadmapArtifact::new(vec![m1, m2, m3]);
    roadmap.missions = vec![accounts, billing];
    roadmap
}

#[test]
fn well_formed_missions_validate_and_round_trip() {
    let roadmap = mission_roadmap();
    assert_eq!(roadmap.validate_ledger(), Vec::new());

    let text = toml::to_string(&roadmap).unwrap();
    let parsed: RoadmapArtifact = toml::from_str(&text).unwrap();
    assert_eq!(parsed, roadmap);
    assert_eq!(
        parsed
            .mission_of_milestone(&"m2".into())
            .map(|m| m.id.as_str()),
        Some("mission-1")
    );
}

#[test]
fn roadmap_without_missions_serializes_without_the_field() {
    let roadmap = ledger_roadmap(vec![sized_task("t1", &[])]);
    let text = toml::to_string(&roadmap).unwrap();
    assert!(!text.contains("missions"), "{text}");
    assert!(!text.contains("fulfills"), "{text}");
}

#[test]
fn missions_must_partition_milestones_in_order() {
    let mut roadmap = mission_roadmap();
    roadmap.missions[1].milestones.clear();
    let issues = roadmap.validate_ledger();
    assert!(issues.contains(&RoadmapLedgerIssue::EmptyMission {
        mission: "mission-2".into()
    }));
    assert!(
        issues.contains(&RoadmapLedgerIssue::MilestoneWithoutMission {
            milestone: "m3".into()
        })
    );

    let mut roadmap = mission_roadmap();
    roadmap.missions[1].milestones.push("m2".into());
    assert!(
        roadmap
            .validate_ledger()
            .contains(&RoadmapLedgerIssue::MilestoneInSeveralMissions {
                milestone: "m2".into()
            })
    );

    let mut roadmap = mission_roadmap();
    roadmap.missions[1].milestones.push("m9".into());
    assert!(
        roadmap
            .validate_ledger()
            .contains(&RoadmapLedgerIssue::UnknownMissionMilestone {
                mission: "mission-2".into(),
                milestone: "m9".into()
            })
    );

    let mut roadmap = mission_roadmap();
    roadmap.missions[0].milestones = vec!["m2".into(), "m1".into()];
    assert_eq!(
        roadmap.validate_ledger(),
        vec![RoadmapLedgerIssue::MissionOrderMismatch {
            milestone: "m1".into()
        }]
    );
}

#[test]
fn validation_contract_is_required_and_claimed_exactly_once() {
    let mut roadmap = mission_roadmap();
    roadmap.missions[1].validation_contract.clear();
    let issues = roadmap.validate_ledger();
    assert!(
        issues.contains(&RoadmapLedgerIssue::EmptyValidationContract {
            mission: "mission-2".into()
        })
    );
    assert!(issues.contains(&RoadmapLedgerIssue::UnknownFulfills {
        task: "m3-t1".into(),
        assertion: "VAL-PAY-001".into()
    }));

    let mut roadmap = mission_roadmap();
    roadmap.milestones[0].tasks[1].fulfills.clear();
    assert_eq!(
        roadmap.validate_ledger(),
        vec![RoadmapLedgerIssue::UnclaimedAssertion {
            mission: "mission-1".into(),
            assertion: "VAL-AUTH-001".into()
        }]
    );

    let mut roadmap = mission_roadmap();
    roadmap.milestones[0].tasks[0].fulfills = vec!["VAL-AUTH-001".into()];
    assert_eq!(
        roadmap.validate_ledger(),
        vec![RoadmapLedgerIssue::AssertionClaimedTwice {
            assertion: "VAL-AUTH-001".into(),
            tasks: vec!["m1-t1".into(), "m1-t2".into()]
        }]
    );

    let mut roadmap = mission_roadmap();
    roadmap.missions[1]
        .validation_contract
        .push(assertion("VAL-AUTH-001"));
    assert!(
        roadmap
            .validate_ledger()
            .contains(&RoadmapLedgerIssue::DuplicateAssertionId {
                assertion: "VAL-AUTH-001".into()
            })
    );
}

#[test]
fn a_task_cannot_fulfill_another_missions_assertion() {
    let mut roadmap = mission_roadmap();
    roadmap.milestones[2].tasks[0].fulfills = vec!["VAL-AUTH-002".into()];
    roadmap.milestones[1].tasks[0].fulfills = vec!["VAL-PAY-001".into()];
    let issues = roadmap.validate_ledger();
    assert!(
        issues.contains(&RoadmapLedgerIssue::FulfillsOutsideMission {
            task: "m3-t1".into(),
            assertion: "VAL-AUTH-002".into(),
            mission: "mission-1".into()
        })
    );
    assert!(issues.contains(&RoadmapLedgerIssue::UnclaimedAssertion {
        mission: "mission-2".into(),
        assertion: "VAL-PAY-001".into()
    }));
}

#[test]
fn markdown_lists_missions_contracts_and_fulfills() {
    let markdown = mission_roadmap().to_markdown();
    assert!(
        markdown.contains("### mission-1: Accounts (m1, m2)"),
        "{markdown}"
    );
    assert!(markdown.contains("Goal: Users can sign in"), "{markdown}");
    assert!(
        markdown.contains("- VAL-PAY-001: VAL-PAY-001"),
        "{markdown}"
    );
    assert!(
        markdown.contains("  - Fulfills: VAL-AUTH-001"),
        "{markdown}"
    );
}

fn milestone_with(id: &str, tasks: Vec<RoadmapTask>) -> RoadmapMilestone {
    let mut milestone = RoadmapMilestone::new(id, id);
    milestone.tasks = tasks;
    milestone
}

fn stage_roadmap() -> RoadmapArtifact {
    let mut roadmap = RoadmapArtifact::new(vec![
        milestone_with("m1", vec![sized_task("m1-t1", &[])]),
        milestone_with("m2", vec![sized_task("m2-t1", &[])]),
        milestone_with("m3", vec![sized_task("m3-t1", &[])]),
    ]);
    let mut first = RoadmapStage::new("stage-1", "MVP", "Usable core");
    first.milestones = vec!["m1".into(), "m2".into()];
    first.exit_criteria = vec!["Core flow demoable".into()];
    let mut second = RoadmapStage::new("stage-2", "Public beta", "");
    second.milestones = vec!["m3".into()];
    roadmap.stages = vec![first, second];
    roadmap
}

#[test]
fn single_stage_roadmap_is_valid() {
    let mut roadmap = stage_roadmap();
    roadmap.stages.truncate(1);
    roadmap.stages[0].milestones = vec!["m1".into(), "m2".into(), "m3".into()];
    roadmap.stages_rationale = Some("A timer ships in one release.".into());
    assert_eq!(roadmap.validate_ledger(), Vec::new());
    assert!(
        roadmap
            .to_markdown()
            .contains("Stages: A timer ships in one release."),
    );
}

#[test]
fn roadmap_without_stages_is_valid() {
    let mut roadmap = stage_roadmap();
    roadmap.stages.clear();
    assert_eq!(roadmap.validate_ledger(), Vec::new());
    assert!(!roadmap.to_markdown().contains("# Stage"));
}

#[test]
fn ceremonial_empty_stage_is_rejected() {
    let mut roadmap = stage_roadmap();
    roadmap
        .stages
        .push(RoadmapStage::new("stage-3", "Production", "Launch"));
    assert!(
        roadmap
            .validate_ledger()
            .contains(&RoadmapLedgerIssue::EmptyStage {
                stage: "stage-3".into()
            })
    );
}

#[test]
fn well_formed_stages_validate_and_render_as_headings() {
    let roadmap = stage_roadmap();
    assert_eq!(roadmap.validate_ledger(), Vec::new());
    let markdown = roadmap.to_markdown();
    let first = markdown.find("# Stage stage-1: MVP").expect("first stage");
    let m2 = markdown.find("## m2: m2").expect("m2");
    let second = markdown
        .find("# Stage stage-2: Public beta")
        .expect("second stage");
    assert!(first < m2 && m2 < second, "{markdown}");
    assert!(
        markdown.contains("- Exit: Core flow demoable"),
        "{markdown}"
    );
}

#[test]
fn stage_partition_violations_are_reported() {
    let mut roadmap = stage_roadmap();
    roadmap.stages[1].milestones = vec!["m2".into(), "ghost".into()];
    let issues = roadmap.validate_ledger();
    assert!(
        issues.contains(&RoadmapLedgerIssue::MilestoneInSeveralStages {
            milestone: "m2".into()
        })
    );
    assert!(issues.contains(&RoadmapLedgerIssue::UnknownStageMilestone {
        stage: "stage-2".into(),
        milestone: "ghost".into()
    }));
    assert!(issues.contains(&RoadmapLedgerIssue::MilestoneWithoutStage {
        milestone: "m3".into()
    }));
}

#[test]
fn stage_ids_and_milestone_order_are_enforced() {
    let mut roadmap = stage_roadmap();
    roadmap.stages[1].id = "stage-1".into();
    assert!(
        roadmap
            .validate_ledger()
            .contains(&RoadmapLedgerIssue::DuplicateStageId {
                stage: "stage-1".into()
            })
    );

    let mut roadmap = stage_roadmap();
    roadmap.stages[0].milestones = vec!["m2".into(), "m1".into()];
    assert!(
        roadmap
            .validate_ledger()
            .contains(&RoadmapLedgerIssue::StageOrderMismatch {
                milestone: "m1".into()
            })
    );
}

#[test]
fn legacy_roadmap_without_stages_or_priorities_still_parses() {
    let roadmap: RoadmapArtifact = toml::from_str(
        "schema_version = 1\n[[milestones]]\nid = \"m1\"\ntitle = \"M\"\n[[milestones.tasks]]\nid = \"t1\"\ntitle = \"T\"\n",
    )
    .expect("legacy roadmap parses");
    assert!(roadmap.stages.is_empty());
    assert_eq!(roadmap.milestones[0].priority, None);
    assert_eq!(roadmap.milestones[0].tasks[0].parallel_group, None);
    let serialized = toml::to_string(&roadmap).expect("serialize");
    assert!(!serialized.contains("stages"), "{serialized}");
    assert!(!serialized.contains("priority"), "{serialized}");
}

#[test]
fn priority_and_parallel_group_round_trip() {
    let mut task = sized_task("t1", &[]);
    task.priority = Some(TaskPriority::P0);
    task.parallel_group = Some("api".into());
    let roadmap = RoadmapArtifact::new(vec![milestone_with("m1", vec![task])]);
    let serialized = toml::to_string(&roadmap).expect("serialize");
    assert!(serialized.contains("priority = \"p0\""), "{serialized}");
    let parsed: RoadmapArtifact = toml::from_str(&serialized).expect("parse");
    assert_eq!(parsed, roadmap);
    assert!(
        roadmap
            .to_markdown()
            .contains("- [ ] t1: t1 [p0] [parallel: api]"),
        "{}",
        roadmap.to_markdown()
    );
}

#[test]
fn parallel_group_rejects_direct_and_transitive_dependencies() {
    let mut a = sized_task("a", &[]);
    let mut b = sized_task("b", &["a"]);
    let mut c = sized_task("c", &["b"]);
    let mut outsider = sized_task("x", &[]);
    for task in [&mut a, &mut b, &mut c] {
        task.parallel_group = Some("g".into());
    }
    outsider.parallel_group = Some("h".into());
    let roadmap = RoadmapArtifact::new(vec![milestone_with("m1", vec![a, b, c, outsider])]);
    let issues = roadmap.validate_ledger();
    assert!(
        issues.contains(&RoadmapLedgerIssue::ParallelGroupDependency {
            group: "g".into(),
            task: "b".into(),
            depends_on: "a".into()
        })
    );
    assert!(
        issues.contains(&RoadmapLedgerIssue::ParallelGroupDependency {
            group: "g".into(),
            task: "c".into(),
            depends_on: "a".into()
        })
    );
    assert_eq!(issues.len(), 3, "{issues:?}");
}

#[test]
fn independent_group_members_and_blank_groups() {
    let mut a = sized_task("a", &[]);
    let mut b = sized_task("b", &[]);
    a.parallel_group = Some("g".into());
    b.parallel_group = Some("  ".into());
    let roadmap = RoadmapArtifact::new(vec![milestone_with("m1", vec![a, b])]);
    assert_eq!(
        roadmap.validate_ledger(),
        vec![RoadmapLedgerIssue::EmptyParallelGroup { task: "b".into() }]
    );
}

#[test]
fn ready_batches_are_dependency_waves_ordered_by_priority() {
    let mut low = sized_task("low", &[]);
    low.priority = Some(TaskPriority::P3);
    let mut urgent = sized_task("urgent", &[]);
    urgent.priority = Some(TaskPriority::P0);
    let unranked = sized_task("unranked", &[]);
    let after = sized_task("after", &["low", "urgent"]);
    let mut second_milestone_urgent = sized_task("late", &["ghost"]);
    second_milestone_urgent.priority = Some(TaskPriority::P0);
    let roadmap = RoadmapArtifact::new(vec![
        milestone_with("m1", vec![low, unranked, urgent, after]),
        milestone_with("m2", vec![second_milestone_urgent]),
    ]);
    let ids: Vec<Vec<&str>> = roadmap
        .ready_batches()
        .iter()
        .map(|batch| batch.iter().map(|task| task.id.as_str()).collect())
        .collect();
    assert_eq!(
        ids,
        vec![vec!["urgent", "late", "low", "unranked"], vec!["after"]]
    );
}

#[test]
fn ready_batches_omit_cycles() {
    let roadmap = RoadmapArtifact::new(vec![milestone_with(
        "m1",
        vec![
            sized_task("a", &["b"]),
            sized_task("b", &["a"]),
            sized_task("c", &[]),
        ],
    )]);
    let batches = roadmap.ready_batches();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0][0].id, "c");
}
