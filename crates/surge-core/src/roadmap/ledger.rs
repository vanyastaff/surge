//! Ledger validation for [`RoadmapArtifact`]: dependency, mission and stage
//! checks, cycle detection and the diagnostics they raise.

use super::*;

impl RoadmapArtifact {
    /// Validate the task-ledger invariants of this roadmap.
    ///
    /// Pure structural checks, independent of any I/O or diagnostics
    /// plumbing: unique milestone/task ids, referential integrity of
    /// task-level `depends_on` / `discovered_from` and milestone-level
    /// `dependencies`, acyclic task dependencies, and — at schema v2 —
    /// a required `size` on every task. Returns an empty vector when the
    /// ledger is well-formed.
    #[must_use]
    pub fn validate_ledger(&self) -> Vec<RoadmapLedgerIssue> {
        let mut issues = Vec::new();

        let mut milestone_ids: HashSet<&str> = HashSet::new();
        for milestone in &self.milestones {
            if !milestone_ids.insert(milestone.id.as_str()) {
                issues.push(RoadmapLedgerIssue::DuplicateMilestoneId {
                    milestone: milestone.id.clone(),
                });
            }
        }

        let mut has_duplicate_task_ids = false;
        let mut task_ids: HashSet<&str> = HashSet::new();
        for task in self.tasks() {
            if !task_ids.insert(task.id.as_str()) {
                has_duplicate_task_ids = true;
                issues.push(RoadmapLedgerIssue::DuplicateTaskId {
                    task: task.id.clone(),
                });
            }
        }

        for task in self.tasks() {
            for dependency in &task.depends_on {
                if dependency == &task.id {
                    issues.push(RoadmapLedgerIssue::SelfDependency {
                        task: task.id.clone(),
                    });
                } else if !task_ids.contains(dependency.as_str()) {
                    issues.push(RoadmapLedgerIssue::UnknownDependsOn {
                        task: task.id.clone(),
                        missing: dependency.clone(),
                    });
                }
            }
            if let Some(origin) = &task.discovered_from {
                if origin == &task.id {
                    issues.push(RoadmapLedgerIssue::SelfDiscovery {
                        task: task.id.clone(),
                    });
                } else if !task_ids.contains(origin.as_str()) {
                    issues.push(RoadmapLedgerIssue::UnknownDiscoveredFrom {
                        task: task.id.clone(),
                        missing: origin.clone(),
                    });
                }
            }
            if self.schema_version >= SIZE_REQUIRED_FROM_VERSION && task.size.is_none() {
                issues.push(RoadmapLedgerIssue::MissingSize {
                    task: task.id.clone(),
                });
            }
        }

        for dependency in &self.dependencies {
            if dependency.from == dependency.to {
                issues.push(RoadmapLedgerIssue::MilestoneSelfDependency {
                    milestone: dependency.from.clone(),
                });
            }
            for milestone in [&dependency.from, &dependency.to] {
                if !milestone_ids.contains(milestone.as_str()) {
                    issues.push(RoadmapLedgerIssue::UnknownMilestoneDependency {
                        missing: milestone.clone(),
                    });
                }
            }
        }

        self.validate_missions(&milestone_ids, &mut issues);
        self.validate_stages(&milestone_ids, &mut issues);
        self.validate_parallel_groups(&mut issues);

        // Cycle detection is unreliable when duplicate task ids exist (the
        // HashMap in find_task_cycle uses last-write-wins, which can mask
        // edges). Skip it so we don't report a false-negative.
        if !has_duplicate_task_ids && let Some(cycle) = self.find_task_cycle() {
            issues.push(RoadmapLedgerIssue::DependencyCycle { cycle });
        }

        issues
    }

    /// The mission that owns `milestone_id`, if missions are declared.
    #[must_use]
    pub fn mission_of_milestone(&self, milestone_id: &str) -> Option<&RoadmapMission> {
        self.missions
            .iter()
            .find(|mission| mission.milestones.iter().any(|id| id == milestone_id))
    }

    /// Mission-level invariants; a no-op when no missions are declared,
    /// except that a task claiming `fulfills` then references nothing.
    ///
    /// Structure: unique non-empty missions whose milestone lists name
    /// existing milestones, each milestone in exactly one mission, in
    /// roadmap order. Contract: every mission carries at least one
    /// assertion, assertion ids are unique, and each assertion is fulfilled
    /// by exactly one task inside its own mission (Factory's coverage gate —
    /// no orphans, no duplicates).
    fn validate_missions(
        &self,
        milestone_ids: &HashSet<&str>,
        issues: &mut Vec<RoadmapLedgerIssue>,
    ) {
        let mut mission_ids: HashSet<&str> = HashSet::new();
        let mut owner: HashMap<&str, &str> = HashMap::new();
        let mut structure_ok = true;
        for mission in &self.missions {
            if !mission_ids.insert(mission.id.as_str()) {
                issues.push(RoadmapLedgerIssue::DuplicateMissionId {
                    mission: mission.id.clone(),
                });
            }
            if mission.milestones.is_empty() {
                structure_ok = false;
                issues.push(RoadmapLedgerIssue::EmptyMission {
                    mission: mission.id.clone(),
                });
            }
            if mission.validation_contract.is_empty() {
                issues.push(RoadmapLedgerIssue::EmptyValidationContract {
                    mission: mission.id.clone(),
                });
            }
            for milestone in &mission.milestones {
                if !milestone_ids.contains(milestone.as_str()) {
                    structure_ok = false;
                    issues.push(RoadmapLedgerIssue::UnknownMissionMilestone {
                        mission: mission.id.clone(),
                        milestone: milestone.clone(),
                    });
                } else if owner
                    .insert(milestone.as_str(), mission.id.as_str())
                    .is_some()
                {
                    structure_ok = false;
                    issues.push(RoadmapLedgerIssue::MilestoneInSeveralMissions {
                        milestone: milestone.clone(),
                    });
                }
            }
        }
        if !self.missions.is_empty() {
            for milestone in &self.milestones {
                if !owner.contains_key(milestone.id.as_str()) {
                    structure_ok = false;
                    issues.push(RoadmapLedgerIssue::MilestoneWithoutMission {
                        milestone: milestone.id.clone(),
                    });
                }
            }
        }
        // Order only means something once membership is a clean partition.
        if structure_ok {
            let declared = self
                .missions
                .iter()
                .flat_map(|mission| mission.milestones.iter());
            let mismatch = declared
                .zip(&self.milestones)
                .find(|(declared, actual)| **declared != actual.id);
            if let Some((_, actual)) = mismatch {
                issues.push(RoadmapLedgerIssue::MissionOrderMismatch {
                    milestone: actual.id.clone(),
                });
            }
        }

        // Assertion id -> owning mission.
        let mut assertion_mission: HashMap<&str, &str> = HashMap::new();
        for mission in &self.missions {
            for assertion in &mission.validation_contract {
                if assertion_mission
                    .insert(assertion.id.as_str(), mission.id.as_str())
                    .is_some()
                {
                    issues.push(RoadmapLedgerIssue::DuplicateAssertionId {
                        assertion: assertion.id.clone(),
                    });
                }
            }
        }

        let mut claims: HashMap<&str, Vec<String>> = HashMap::new();
        let fulfilled = self.milestones.iter().flat_map(|milestone| {
            let task_mission = owner.get(milestone.id.as_str()).copied();
            milestone.tasks.iter().flat_map(move |task| {
                task.fulfills
                    .iter()
                    .map(move |assertion| (task_mission, task, assertion))
            })
        });
        for (task_mission, task, assertion) in fulfilled {
            match assertion_mission.get(assertion.as_str()) {
                None => issues.push(RoadmapLedgerIssue::UnknownFulfills {
                    task: task.id.clone(),
                    assertion: assertion.clone(),
                }),
                Some(mission) if Some(*mission) != task_mission => {
                    issues.push(RoadmapLedgerIssue::FulfillsOutsideMission {
                        task: task.id.clone(),
                        assertion: assertion.clone(),
                        mission: (*mission).to_owned(),
                    });
                },
                Some(_) => claims
                    .entry(assertion.as_str())
                    .or_default()
                    .push(task.id.clone()),
            }
        }
        for mission in &self.missions {
            for assertion in &mission.validation_contract {
                match claims.get(assertion.id.as_str()).map(Vec::as_slice) {
                    None | Some([]) => issues.push(RoadmapLedgerIssue::UnclaimedAssertion {
                        mission: mission.id.clone(),
                        assertion: assertion.id.clone(),
                    }),
                    Some([_]) => {},
                    Some(tasks) => issues.push(RoadmapLedgerIssue::AssertionClaimedTwice {
                        assertion: assertion.id.clone(),
                        tasks: tasks.to_vec(),
                    }),
                }
            }
        }
    }

    /// Stage-level invariants; a no-op when no stages are declared.
    ///
    /// Unique stage ids, every stage non-empty (no ceremonial stages) and
    /// naming existing milestones, each milestone in exactly one stage, and
    /// the concatenated stage milestone lists reproducing the roadmap
    /// milestone order. The number of stages is never constrained: one is
    /// valid.
    fn validate_stages(&self, milestone_ids: &HashSet<&str>, issues: &mut Vec<RoadmapLedgerIssue>) {
        if self.stages.is_empty() {
            return;
        }
        let mut stage_ids: HashSet<&str> = HashSet::new();
        let mut owner: HashSet<&str> = HashSet::new();
        let mut structure_ok = true;
        for stage in &self.stages {
            if !stage_ids.insert(stage.id.as_str()) {
                issues.push(RoadmapLedgerIssue::DuplicateStageId {
                    stage: stage.id.clone(),
                });
            }
            if stage.milestones.is_empty() {
                structure_ok = false;
                issues.push(RoadmapLedgerIssue::EmptyStage {
                    stage: stage.id.clone(),
                });
            }
            for milestone in &stage.milestones {
                if !milestone_ids.contains(milestone.as_str()) {
                    structure_ok = false;
                    issues.push(RoadmapLedgerIssue::UnknownStageMilestone {
                        stage: stage.id.clone(),
                        milestone: milestone.clone(),
                    });
                } else if !owner.insert(milestone.as_str()) {
                    structure_ok = false;
                    issues.push(RoadmapLedgerIssue::MilestoneInSeveralStages {
                        milestone: milestone.clone(),
                    });
                }
            }
        }
        for milestone in &self.milestones {
            if !owner.contains(milestone.id.as_str()) {
                structure_ok = false;
                issues.push(RoadmapLedgerIssue::MilestoneWithoutStage {
                    milestone: milestone.id.clone(),
                });
            }
        }
        // Order only means something once membership is a clean partition.
        if structure_ok {
            let mismatch = self
                .stages
                .iter()
                .flat_map(|stage| stage.milestones.iter())
                .zip(&self.milestones)
                .find(|(declared, actual)| **declared != actual.id);
            if let Some((_, actual)) = mismatch {
                issues.push(RoadmapLedgerIssue::StageOrderMismatch {
                    milestone: actual.id.clone(),
                });
            }
        }
    }

    /// Reject blank group names and groups whose members depend on each other.
    ///
    /// A dependency counts whether direct or transitive (through tasks in any
    /// group), because either way the two members cannot run concurrently.
    fn validate_parallel_groups(&self, issues: &mut Vec<RoadmapLedgerIssue>) {
        let dependencies: HashMap<&str, &[String]> = self
            .tasks()
            .map(|task| (task.id.as_str(), task.depends_on.as_slice()))
            .collect();
        let group_of: HashMap<&str, &str> = self
            .tasks()
            .filter_map(|task| Some((task.id.as_str(), task.parallel_group.as_deref()?)))
            .collect();

        for task in self.tasks() {
            let Some(group) = task.parallel_group.as_deref() else {
                continue;
            };
            if group.trim().is_empty() {
                issues.push(RoadmapLedgerIssue::EmptyParallelGroup {
                    task: task.id.clone(),
                });
                continue;
            }
            let mut visited: HashSet<&str> = HashSet::new();
            let mut pending: Vec<&str> = task.depends_on.iter().map(String::as_str).collect();
            while let Some(candidate) = pending.pop() {
                if !visited.insert(candidate) {
                    continue;
                }
                if candidate != task.id && group_of.get(candidate) == Some(&group) {
                    issues.push(RoadmapLedgerIssue::ParallelGroupDependency {
                        group: group.to_owned(),
                        task: task.id.clone(),
                        depends_on: candidate.to_owned(),
                    });
                }
                if let Some(next) = dependencies.get(candidate) {
                    pending.extend(next.iter().map(String::as_str));
                }
            }
        }
    }

    /// Group tasks into dependency-respecting waves for concurrent execution.
    ///
    /// Wave `n` holds every task whose `depends_on` tasks all sit in waves
    /// `< n`, so the tasks inside one wave can run in parallel. Within a wave
    /// tasks are ordered by priority (`p0` first, unprioritised last), then by
    /// their milestone's priority, then by declaration order. Unknown
    /// dependency ids are ignored; tasks on a dependency cycle never become
    /// ready and are omitted (they are reported by [`Self::validate_ledger`]).
    /// The plan covers every task regardless of status.
    #[must_use]
    pub fn ready_batches(&self) -> Vec<Vec<&RoadmapTask>> {
        fn placed_before(entries: &[Entry<'_>], index: usize, wave: usize) -> bool {
            entries[index].wave.is_some_and(|placed| placed < wave)
        }
        struct Entry<'a> {
            task: &'a RoadmapTask,
            milestone_priority: Option<TaskPriority>,
            wave: Option<usize>,
        }
        let mut entries: Vec<Entry<'_>> = self
            .milestones
            .iter()
            .flat_map(|milestone| {
                milestone.tasks.iter().map(|task| Entry {
                    task,
                    milestone_priority: milestone.priority,
                    wave: None,
                })
            })
            .collect();
        let mut index_of: HashMap<&str, usize> = HashMap::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            index_of.entry(entry.task.id.as_str()).or_insert(index);
        }

        let mut batches: Vec<Vec<&RoadmapTask>> = Vec::new();
        loop {
            let wave = batches.len();
            // A dependency is settled when it is unknown or already placed in
            // an earlier wave; a self edge never settles.
            let is_settled = |index: usize, dependency: &String| {
                index_of
                    .get(dependency.as_str())
                    .is_none_or(|&target| target != index && placed_before(&entries, target, wave))
            };
            let ready: Vec<usize> = (0..entries.len())
                .filter(|&index| entries[index].wave.is_none())
                .filter(|&index| {
                    let dependencies = &entries[index].task.depends_on;
                    dependencies
                        .iter()
                        .all(|dependency| is_settled(index, dependency))
                })
                .collect();
            if ready.is_empty() {
                break;
            }
            for &index in &ready {
                entries[index].wave = Some(wave);
            }
            let mut batch: Vec<usize> = ready;
            // Stable sort keeps declaration order among equal priorities.
            batch.sort_by_key(|&index| {
                (
                    entries[index].task.priority.map_or(u8::MAX, |p| p as u8),
                    entries[index]
                        .milestone_priority
                        .map_or(u8::MAX, |p| p as u8),
                )
            });
            batches.push(batch.into_iter().map(|index| entries[index].task).collect());
        }
        batches
    }

    /// Iterate every task across all milestones in declaration order.
    pub fn tasks(&self) -> impl Iterator<Item = &RoadmapTask> {
        self.milestones
            .iter()
            .flat_map(|milestone| milestone.tasks.iter())
    }

    /// Find one cycle in the task-level `depends_on` graph, if any.
    ///
    /// Deterministic: tasks are visited in declaration order, and each task's
    /// dependencies in their declared order, so the same roadmap always
    /// reports the same cycle. Unknown dependency ids are ignored here — they
    /// are reported separately as [`RoadmapLedgerIssue::UnknownDependsOn`].
    ///
    /// Uses an explicit-stack iterative DFS (not recursion): a linear chain of
    /// tens of thousands of tasks would blow a recursive call stack, and a
    /// depth cap would silently miss deep cycles.
    fn find_task_cycle(&self) -> Option<Vec<String>> {
        let dependencies: HashMap<&str, &[String]> = self
            .tasks()
            .map(|task| (task.id.as_str(), task.depends_on.as_slice()))
            .collect();

        let mut marks: HashMap<&str, CycleMark> = HashMap::new();

        for task in self.tasks() {
            if let Some(cycle) = find_cycle_from(task.id.as_str(), &dependencies, &mut marks) {
                return Some(cycle);
            }
        }
        None
    }
}

#[derive(Clone, Copy, PartialEq)]
enum CycleMark {
    Visiting,
    Done,
}

/// One entry in the explicit DFS stack: a node and the index of the next
/// child (dependency) to visit from it.
struct CycleFrame<'a> {
    node: &'a str,
    next_child: usize,
}

/// Iterative depth-first search from `start` looking for a back edge into the
/// active path. Returns the cycle path (first == last) on the first one found.
///
/// Mirrors a recursive coloring DFS: a node on the active path is `Visiting`,
/// a fully-explored node is `Done`. Encountering a `Visiting` node means the
/// active path plus that node closes a cycle.
fn find_cycle_from<'a>(
    start: &'a str,
    dependencies: &HashMap<&'a str, &'a [String]>,
    marks: &mut HashMap<&'a str, CycleMark>,
) -> Option<Vec<String>> {
    if marks.get(start) == Some(&CycleMark::Done) {
        return None;
    }

    let mut path: Vec<CycleFrame<'a>> = vec![CycleFrame {
        node: start,
        next_child: 0,
    }];
    marks.insert(start, CycleMark::Visiting);

    while let Some(frame) = path.last_mut() {
        let node = frame.node;
        let children = dependencies.get(node).copied().unwrap_or_default();

        if frame.next_child < children.len() {
            let target = children[frame.next_child].as_str();
            frame.next_child += 1;

            // Only follow edges to known tasks; unknown ids are a separate
            // diagnostic and must not appear in the reported cycle.
            if !dependencies.contains_key(target) {
                continue;
            }
            match marks.get(target) {
                Some(CycleMark::Done) => continue,
                Some(CycleMark::Visiting) => {
                    // Back edge: close the cycle at `target`.
                    let start_idx = path.iter().position(|f| f.node == target)?;
                    let mut cycle: Vec<String> = path[start_idx..]
                        .iter()
                        .map(|f| f.node.to_owned())
                        .collect();
                    cycle.push(target.to_owned());
                    return Some(cycle);
                },
                None => {
                    marks.insert(target, CycleMark::Visiting);
                    path.push(CycleFrame {
                        node: target,
                        next_child: 0,
                    });
                },
            }
        } else {
            marks.insert(node, CycleMark::Done);
            path.pop();
        }
    }
    None
}

/// One structural problem found by [`RoadmapArtifact::validate_ledger`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RoadmapLedgerIssue {
    /// Two milestones share the same id.
    DuplicateMilestoneId {
        /// The duplicated milestone id.
        milestone: String,
    },
    /// Two tasks share the same id (across all milestones).
    DuplicateTaskId {
        /// The duplicated task id.
        task: String,
    },
    /// A task depends on itself.
    SelfDependency {
        /// The task id.
        task: String,
    },
    /// A task depends on a task id that does not exist.
    UnknownDependsOn {
        /// The task declaring the dependency.
        task: String,
        /// The missing task id.
        missing: String,
    },
    /// A task claims to be discovered from itself.
    SelfDiscovery {
        /// The task id.
        task: String,
    },
    /// A task's `discovered_from` references a task id that does not exist.
    UnknownDiscoveredFrom {
        /// The task declaring the origin.
        task: String,
        /// The missing task id.
        missing: String,
    },
    /// A milestone-level dependency references a missing milestone id.
    UnknownMilestoneDependency {
        /// The missing milestone id.
        missing: String,
    },
    /// A milestone dependency references itself.
    MilestoneSelfDependency {
        /// The milestone id.
        milestone: String,
    },
    /// The task-level `depends_on` graph contains a cycle.
    DependencyCycle {
        /// The cycle as a task-id path; first and last entries are equal.
        cycle: Vec<String>,
    },
    /// A schema-v2 task is missing its required `size`.
    MissingSize {
        /// The task id.
        task: String,
    },
    /// Two missions share the same id.
    DuplicateMissionId {
        /// The duplicated mission id.
        mission: String,
    },
    /// A mission lists no milestones.
    EmptyMission {
        /// The mission id.
        mission: String,
    },
    /// A mission has no validation contract assertions.
    EmptyValidationContract {
        /// The mission id.
        mission: String,
    },
    /// A mission lists a milestone id that does not exist.
    UnknownMissionMilestone {
        /// The mission id.
        mission: String,
        /// The missing milestone id.
        milestone: String,
    },
    /// A milestone is listed by more than one mission.
    MilestoneInSeveralMissions {
        /// The milestone id.
        milestone: String,
    },
    /// Missions are declared but this milestone belongs to none of them.
    MilestoneWithoutMission {
        /// The milestone id.
        milestone: String,
    },
    /// Concatenated mission milestone lists diverge from the milestone order
    /// at this milestone.
    MissionOrderMismatch {
        /// First milestone found out of mission order.
        milestone: String,
    },
    /// Two assertions (in any missions) share the same id.
    DuplicateAssertionId {
        /// The duplicated assertion id.
        assertion: String,
    },
    /// A task's `fulfills` names an assertion no mission declares.
    UnknownFulfills {
        /// The task id.
        task: String,
        /// The unknown assertion id.
        assertion: String,
    },
    /// A task fulfills an assertion owned by a mission other than its own.
    FulfillsOutsideMission {
        /// The task id.
        task: String,
        /// The assertion id.
        assertion: String,
        /// Mission that owns the assertion.
        mission: String,
    },
    /// No task in the mission fulfills this assertion.
    UnclaimedAssertion {
        /// The mission id.
        mission: String,
        /// The unclaimed assertion id.
        assertion: String,
    },
    /// More than one task claims to fulfill the same assertion.
    AssertionClaimedTwice {
        /// The assertion id.
        assertion: String,
        /// Every task claiming it.
        tasks: Vec<String>,
    },
    /// Two stages share the same id.
    DuplicateStageId {
        /// The duplicated stage id.
        stage: String,
    },
    /// A stage lists no milestones; empty stages are ceremony, not structure.
    EmptyStage {
        /// The stage id.
        stage: String,
    },
    /// A stage lists a milestone id that does not exist.
    UnknownStageMilestone {
        /// The stage id.
        stage: String,
        /// The missing milestone id.
        milestone: String,
    },
    /// A milestone is listed by more than one stage.
    MilestoneInSeveralStages {
        /// The milestone id.
        milestone: String,
    },
    /// Stages are declared but this milestone belongs to none of them.
    MilestoneWithoutStage {
        /// The milestone id.
        milestone: String,
    },
    /// Concatenated stage milestone lists diverge from the milestone order at
    /// this milestone.
    StageOrderMismatch {
        /// First milestone found out of stage order.
        milestone: String,
    },
    /// A task's `parallel_group` is blank.
    EmptyParallelGroup {
        /// The task id.
        task: String,
    },
    /// A task depends (directly or transitively) on another member of its
    /// own `parallel_group`, so the two cannot run concurrently.
    ParallelGroupDependency {
        /// The group name.
        group: String,
        /// The dependent task.
        task: String,
        /// The group member it depends on.
        depends_on: String,
    },
}

impl std::fmt::Display for RoadmapLedgerIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateMilestoneId { milestone } => {
                write!(formatter, "duplicate milestone id {milestone:?}")
            },
            Self::DuplicateTaskId { task } => write!(formatter, "duplicate task id {task:?}"),
            Self::MilestoneSelfDependency { milestone } => {
                write!(formatter, "milestone {milestone:?} depends on itself")
            },
            Self::SelfDependency { task } => {
                write!(formatter, "task {task:?} depends on itself")
            },
            Self::UnknownDependsOn { task, missing } => write!(
                formatter,
                "task {task:?} depends on unknown task {missing:?}"
            ),
            Self::SelfDiscovery { task } => {
                write!(formatter, "task {task:?} is discovered from itself")
            },
            Self::UnknownDiscoveredFrom { task, missing } => write!(
                formatter,
                "task {task:?} is discovered from unknown task {missing:?}"
            ),
            Self::UnknownMilestoneDependency { missing } => write!(
                formatter,
                "milestone dependency references unknown milestone {missing:?}"
            ),
            Self::DependencyCycle { cycle } => {
                write!(formatter, "task dependency cycle: {}", cycle.join(" -> "))
            },
            Self::MissingSize { task } => write!(
                formatter,
                "task {task:?} is missing its required size (schema v2)"
            ),
            Self::DuplicateMissionId { mission } => {
                write!(formatter, "duplicate mission id {mission:?}")
            },
            Self::EmptyMission { mission } => {
                write!(formatter, "mission {mission:?} lists no milestones")
            },
            Self::EmptyValidationContract { mission } => write!(
                formatter,
                "mission {mission:?} has an empty validation_contract; write the assertions that define done before splitting work"
            ),
            Self::UnknownMissionMilestone { mission, milestone } => write!(
                formatter,
                "mission {mission:?} lists unknown milestone {milestone:?}"
            ),
            Self::MilestoneInSeveralMissions { milestone } => write!(
                formatter,
                "milestone {milestone:?} is listed by more than one mission"
            ),
            Self::MilestoneWithoutMission { milestone } => write!(
                formatter,
                "milestone {milestone:?} belongs to no mission; when missions are declared every milestone needs one"
            ),
            Self::MissionOrderMismatch { milestone } => write!(
                formatter,
                "milestone {milestone:?} is out of mission order; list missions and their milestones in roadmap order"
            ),
            Self::DuplicateAssertionId { assertion } => {
                write!(formatter, "duplicate validation assertion id {assertion:?}")
            },
            Self::UnknownFulfills { task, assertion } => write!(
                formatter,
                "task {task:?} fulfills unknown assertion {assertion:?}"
            ),
            Self::FulfillsOutsideMission {
                task,
                assertion,
                mission,
            } => write!(
                formatter,
                "task {task:?} fulfills {assertion:?}, which belongs to another mission ({mission:?})"
            ),
            Self::UnclaimedAssertion { mission, assertion } => write!(
                formatter,
                "assertion {assertion:?} in mission {mission:?} is fulfilled by no task"
            ),
            Self::AssertionClaimedTwice { assertion, tasks } => write!(
                formatter,
                "assertion {assertion:?} is fulfilled by several tasks ({}); exactly one leaf task must claim it",
                tasks.join(", ")
            ),
            Self::DuplicateStageId { stage } => write!(formatter, "duplicate stage id {stage:?}"),
            Self::EmptyStage { stage } => write!(
                formatter,
                "stage {stage:?} lists no milestones; drop it or give it work (a stage marks a real release boundary)"
            ),
            Self::UnknownStageMilestone { stage, milestone } => write!(
                formatter,
                "stage {stage:?} lists unknown milestone {milestone:?}"
            ),
            Self::MilestoneInSeveralStages { milestone } => write!(
                formatter,
                "milestone {milestone:?} is listed by more than one stage"
            ),
            Self::MilestoneWithoutStage { milestone } => write!(
                formatter,
                "milestone {milestone:?} belongs to no stage; when stages are declared every milestone needs one"
            ),
            Self::StageOrderMismatch { milestone } => write!(
                formatter,
                "milestone {milestone:?} is out of stage order; list stages and their milestones in roadmap order"
            ),
            Self::EmptyParallelGroup { task } => {
                write!(formatter, "task {task:?} has a blank parallel_group")
            },
            Self::ParallelGroupDependency {
                group,
                task,
                depends_on,
            } => write!(
                formatter,
                "task {task:?} depends on {depends_on:?} but both are in parallel group {group:?}; tasks in one group must be independent"
            ),
        }
    }
}
