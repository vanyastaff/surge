//! Read-only project history. Acceptance, execution and verification stay separate.
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use surge_core::{RunId, roadmap::RoadmapArtifact, task_execution::TaskExecutionView};
use surge_persistence::runs::{
    RunSummary, bootstrap_operations::BootstrapOperationRecord, registry::RunSummaryCursor,
};

mod loading;
mod model;
mod planning_input;
pub(crate) use loading::{load_registry, load_run};
#[cfg(test)]
pub(crate) use model::CurrentWork;
pub(crate) use model::{HomeOperation, HomeProjection, OperationKey, origin_text};
pub(crate) use planning_input::load_planning_inputs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Availability {
    Unobserved,
    Available,
    Absent,
    Unavailable(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Freshness {
    Loading,
    Observed,
    Retained,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coverage {
    Unknown,
    Partial,
    Complete,
}
#[derive(Clone, Debug)]
pub(crate) struct SourceObservation {
    pub(crate) availability: Availability,
    pub(crate) freshness: Freshness,
    pub(crate) coverage: Coverage,
    pub(crate) through: Option<u64>,
}
impl Default for SourceObservation {
    fn default() -> Self {
        Self {
            availability: Availability::Unobserved,
            freshness: Freshness::Retained,
            coverage: Coverage::Unknown,
            through: None,
        }
    }
}
impl SourceObservation {
    pub(crate) fn label(&self) -> String {
        let availability = match &self.availability {
            Availability::Unobserved => "Not loaded".to_owned(),
            Availability::Available => "History available".into(),
            Availability::Absent => "No saved history".into(),
            Availability::Unavailable(error) => format!("History unavailable: {error}"),
        };
        let freshness = match self.freshness {
            Freshness::Loading => "refreshing",
            Freshness::Observed => "last observed",
            Freshness::Retained => "last known",
        };
        let coverage = match self.coverage {
            Coverage::Unknown => "coverage unknown",
            Coverage::Partial => "more history available",
            Coverage::Complete => "loaded to observed end",
        };
        format!("{availability} · {freshness} · {coverage}")
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RunOwnership {
    Modern {
        project: surge_core::id::WorkItemProjectId,
        binding: surge_core::work_item::WorkItemBinding,
        graph: Option<surge_core::ContentHash>,
    },
    Bootstrap {
        operation: RunId,
        planning: bool,
    },
    /// Registry path matches, but startup journal ownership still needs inspection.
    CandidateProject,
    RecordedProject,
    Unknown,
}
#[derive(Clone)]
pub(crate) struct ProjectRun {
    pub(crate) run: RunId,
    pub(crate) ownership: RunOwnership,
    pub(crate) requested: RunOwnership,
    pub(crate) source: SourceObservation,
    pub(crate) events: Arc<Vec<surge_core::RunEvent>>,
    pub(crate) execution: Option<Arc<TaskExecutionView>>,
    pub(crate) mission: Option<Arc<crate::mission::Mission>>,
    pub(crate) decisions: Arc<Vec<surge_orchestrator::operator::PendingDecision>>,
}
#[derive(Clone)]
pub(crate) struct LegacyOperation {
    pub(crate) record: BootstrapOperationRecord,
    pub(crate) roadmap: Option<Arc<RoadmapArtifact>>,
    pub(crate) roadmap_source: SourceObservation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryScope {
    pub(crate) generation: u64,
    pub(crate) root: PathBuf,
    pub(crate) repository: Option<PathBuf>,
    pub(crate) home: PathBuf,
}
#[derive(Clone)]
pub(crate) struct PlanningInput {
    pub(crate) path: PathBuf,
    pub(crate) source: SourceObservation,
    pub(crate) roadmap: Option<Arc<RoadmapArtifact>>,
}
#[derive(Clone, Default)]
pub(crate) struct ProjectHistory {
    pub(crate) scope: Option<HistoryScope>,
    pub(crate) source: SourceObservation,
    pub(crate) planning_inputs: Arc<Vec<PlanningInput>>,
    /// UI snapshot of attached live streams; never populated by file inspection.
    pub(crate) live_observed: HashMap<RunId, u64>,
    pub(crate) summaries: Vec<RunSummary>,
    pub(crate) operations: HashMap<RunId, Arc<LegacyOperation>>,
    pub(crate) runs: HashMap<RunId, Arc<ProjectRun>>,
    pub(crate) next_cursor: Option<RunSummaryCursor>,
    generation: u64,
    pub(crate) loading: bool,
}
impl ProjectHistory {
    pub(crate) fn begin(
        &mut self,
        root: PathBuf,
        repository: Option<PathBuf>,
        home: PathBuf,
        append: bool,
    ) -> HistoryScope {
        self.generation = self.generation.saturating_add(1);
        let same = self.scope.as_ref().is_some_and(|old| {
            old.root == root && old.repository == repository && old.home == home
        });
        if !same {
            self.live_observed.clear();
            self.planning_inputs = Arc::default();
            self.summaries.clear();
            self.operations.clear();
            self.runs.clear();
            self.next_cursor = None;
        }
        if !append {
            self.next_cursor = None;
        }
        let scope = HistoryScope {
            generation: self.generation,
            root,
            repository,
            home,
        };
        self.scope = Some(scope.clone());
        self.loading = true;
        self.source.freshness = Freshness::Loading;
        scope
    }
    pub(crate) fn accepts(&self, scope: &HistoryScope) -> bool {
        self.scope.as_ref() == Some(scope)
    }
    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.scope = None;
        self.live_observed.clear();
        self.planning_inputs = Arc::default();
        self.summaries.clear();
        self.operations.clear();
        self.runs.clear();
        self.next_cursor = None;
        self.loading = false;
        self.source = SourceObservation::default();
    }
    pub(crate) fn fail(&mut self, scope: &HistoryScope, error: String) {
        if self.accepts(scope) {
            self.loading = false;
            self.source.availability = Availability::Unavailable(error);
            self.source.freshness = Freshness::Retained;
        }
    }
}

#[cfg(test)]
mod tests;
