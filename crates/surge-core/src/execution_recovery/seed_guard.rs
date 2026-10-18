//! One incremental startup-marker checker. Metadata never hydrates a seed.
use crate::{ContentHash, EventPayload, RunEvent, RunId, work_item::WorkItemBinding};

/// Unsupported or contradictory semantic startup history.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SeedHistoryError {
    #[error("invalid event position: sequence zero")]
    InvalidPosition,
    #[error("duplicate continuation seed marker")]
    Duplicate,
    #[error("continuation seed marker appears after executable history")]
    Late,
    #[error("continuation seed marker contradicts its own startup binding")]
    Binding,
    #[error("semantic history unavailable: continuation seed is not hydrated")]
    Unavailable,
}
/// Constant-size, incremental structural context; conveys no launch/read capability.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedStartupGuard {
    run: Option<RunId>,
    binding: Option<WorkItemBinding>,
    graph_hash: Option<ContentHash>,
    executable: bool,
    marker: bool,
}
impl SeedStartupGuard {
    /// Check before projection mutation. Call finish to enforce unavailable input.
    pub fn observe(&mut self, event: &RunEvent) -> Result<(), SeedHistoryError> {
        if event.seq == 0 {
            return Err(SeedHistoryError::InvalidPosition);
        }
        match &event.payload {
            EventPayload::RunStarted { .. } => self.run = Some(event.run_id),
            EventPayload::WorkItemAttemptBound { context } => {
                self.binding = Some(context.binding().clone())
            },
            EventPayload::PipelineMaterialized { graph_hash, .. } => {
                self.graph_hash = Some(*graph_hash)
            },
            EventPayload::ContinuationSeedBound { manifest } => {
                if self.marker {
                    return Err(SeedHistoryError::Duplicate);
                }
                if self.executable {
                    return Err(SeedHistoryError::Late);
                }
                let fields = manifest.fields();
                if fields.run != event.run_id
                    || self.run.is_some_and(|run| run != fields.run)
                    || self.binding.as_ref() != Some(&fields.binding)
                    || self.graph_hash != Some(fields.graph_hash)
                {
                    return Err(SeedHistoryError::Binding);
                }
                self.marker = true;
            },
            EventPayload::OwnedFlowInputsBound { .. }
            | EventPayload::ArtifactProduced { .. }
            | EventPayload::VerificationSubjectObserved { .. } => {},
            _ => self.executable = true,
        }
        Ok(())
    }
    /// Refuse observed seed metadata until authenticated semantic hydration exists.
    pub fn finish(&self) -> Result<(), SeedHistoryError> {
        if self.marker {
            Err(SeedHistoryError::Unavailable)
        } else {
            Ok(())
        }
    }
}
/// Validate the complete input before publishing any semantic projection.
pub fn preflight_unhydrated(events: &[RunEvent]) -> Result<(), SeedHistoryError> {
    let mut guard = SeedStartupGuard::default();
    for event in events {
        guard.observe(event)?;
        guard.finish()?;
    }
    guard.finish()
}
