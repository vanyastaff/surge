//! Request bookkeeping, independent of rendering and I/O.
use crate::project_lifecycle::{ProjectRun, RunOwnership};
use std::collections::HashSet;
use surge_core::RunId;

#[derive(Default)]
pub(super) struct RunReads {
    active: HashSet<(u64, RunId)>,
}
impl RunReads {
    pub(super) fn start_epoch(&mut self, epoch: u64) {
        self.active.retain(|(observed, _)| *observed == epoch);
    }
    pub(super) fn clear(&mut self) {
        self.active.clear();
    }
    pub(super) fn begin(&mut self, epoch: u64, run: RunId) -> bool {
        self.active.insert((epoch, run))
    }
    pub(super) fn finish(&mut self, epoch: u64, run: RunId) {
        self.active.remove(&(epoch, run));
    }
    pub(super) fn contains(&self, epoch: u64, run: RunId) -> bool {
        self.active.contains(&(epoch, run))
    }
    pub(super) fn count(&self, epoch: u64) -> usize {
        self.active
            .iter()
            .filter(|(observed, _)| *observed == epoch)
            .count()
    }
}
#[derive(Default)]
pub(super) struct ModelRebuild {
    generation: u64,
    active: Option<u64>,
    pending: bool,
}
impl ModelRebuild {
    pub(super) fn request(&mut self) -> Option<u64> {
        self.generation = self.generation.saturating_add(1);
        if self.active.is_some() {
            self.pending = true;
            None
        } else {
            self.active = Some(self.generation);
            Some(self.generation)
        }
    }
    pub(super) fn complete(&mut self, generation: u64) -> (bool, Option<u64>) {
        if self.active != Some(generation) {
            return (false, None);
        }
        self.active = None;
        let publish = generation == self.generation;
        let next = if std::mem::take(&mut self.pending) {
            self.active = Some(self.generation);
            Some(self.generation)
        } else {
            None
        };
        (publish, next)
    }
}
pub(super) fn needs_run_refresh(
    stored: Option<&ProjectRun>,
    owner: &RunOwnership,
    known_seq: Option<u64>,
) -> bool {
    let Some(stored) = stored else {
        return true;
    };
    if stored.requested != *owner {
        return true;
    }
    if stored.source.availability == crate::project_lifecycle::Availability::Unobserved {
        return true;
    }
    if stored.source.coverage == crate::project_lifecycle::Coverage::Partial
        || stored.source.availability != crate::project_lifecycle::Availability::Available
    {
        return false;
    }
    known_seq.is_some_and(|seq| stored.source.through.is_none_or(|through| seq > through))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_lifecycle::*;
    #[test]
    fn repeated_notifications_coalesce_into_one_active_and_one_latest_rebuild() {
        let mut rebuild = ModelRebuild::default();
        let first = rebuild.request().unwrap();
        let mut jobs = 1;
        for _ in 0..1000 {
            jobs += usize::from(rebuild.request().is_some());
        }
        assert_eq!(
            jobs, 1,
            "large retained history must not be snapshotted for every notification"
        );
        let (publish, next) = rebuild.complete(first);
        assert!(!publish, "obsolete result cannot publish");
        let latest = next.expect("one rebuild of the latest inputs remains");
        assert_eq!(rebuild.complete(latest), (true, None));
    }
    #[tokio::test]
    async fn held_old_read_cannot_block_or_remove_a_same_project_new_epoch_read() {
        let run = RunId::new();
        let mut reads = RunReads::default();
        reads.start_epoch(1);
        assert!(reads.begin(1, run));
        let (reply, held) = tokio::sync::oneshot::channel();
        reads.start_epoch(2);
        assert!(
            reads.begin(2, run),
            "refresh must be able to read the current epoch"
        );
        reply.send(()).unwrap();
        held.await.unwrap();
        reads.finish(1, run);
        assert!(
            reads.contains(2, run),
            "late old response must not release the current request"
        );
        reads.finish(2, run);
        assert_eq!(reads.count(2), 0);
    }
    #[test]
    fn a_partial_1024_event_prefix_waits_for_explicit_continuation_without_hot_looping() {
        let run = RunId::new();
        let owner = RunOwnership::RecordedProject;
        let stored = ProjectRun {
            run,
            ownership: owner.clone(),
            requested: owner.clone(),
            source: SourceObservation {
                availability: Availability::Available,
                freshness: Freshness::Observed,
                coverage: Coverage::Partial,
                through: Some(1024),
            },
            events: std::sync::Arc::default(),
            execution: None,
            mission: None,
            decisions: std::sync::Arc::default(),
        };
        let requests = (0..1000)
            .filter(|_| needs_run_refresh(Some(&stored), &owner, Some(2048)))
            .count();
        assert_eq!(
            requests, 0,
            "a bounded prefix must not be reread from zero on every notification"
        );
    }
    #[test]
    fn unavailable_run_waits_for_explicit_refresh_instead_of_notification_retry_storm() {
        let run = RunId::new();
        let owner = RunOwnership::RecordedProject;
        let stored = ProjectRun {
            run,
            ownership: RunOwnership::Unknown,
            requested: owner.clone(),
            source: SourceObservation {
                availability: Availability::Unavailable("corrupt journal".into()),
                freshness: Freshness::Retained,
                coverage: Coverage::Unknown,
                through: None,
            },
            events: std::sync::Arc::default(),
            execution: None,
            mission: None,
            decisions: std::sync::Arc::default(),
        };
        assert!(!(0..1000).any(|_| needs_run_refresh(Some(&stored), &owner, Some(1200))));
    }
}
