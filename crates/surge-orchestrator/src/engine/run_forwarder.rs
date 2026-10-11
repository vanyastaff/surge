//! Finite internal event forwarding under the writer's original read-drain lease.

use surge_persistence::runs::{EventSeq, ReadEvent, RunEventFeed, WriterLifecycle};
use tokio::sync::broadcast;

use super::{event_tap::RunEventTap, handle::RunSettlementFailure};

pub(crate) type ForwarderResult = Result<EventSeq, RunSettlementFailure>;

pub(crate) fn spawn(
    feed: RunEventFeed,
    tap: broadcast::Sender<RunEventTap>,
) -> tokio::task::JoinHandle<ForwarderResult> {
    tokio::spawn(forward(feed, tap))
}

async fn forward(mut feed: RunEventFeed, tap: broadcast::Sender<RunEventTap>) -> ForwarderResult {
    let mut after = EventSeq::ZERO;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(
        surge_persistence::runs::subscribe::POLL_INTERVAL_MS,
    ));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let before = feed.current_lifecycle();
        let upper = upper_bound(&before)?;
        if matches!(before, WriterLifecycle::Sealed(_)) && after == upper {
            return Ok(after);
        }
        if after > upper {
            return Err(RunSettlementFailure::ForwarderSequence);
        }
        if matches!(before, WriterLifecycle::Running) {
            tokio::select! {
                changed = feed.changed() => {
                    changed.map_err(|_| RunSettlementFailure::WriterInterrupted)?;
                    continue;
                },
                _ = tick.tick() => {},
            }
        }
        // The actual blocking closure retains the reader AND original writer lease.
        // Do not select away this join: cancellation never manufactures SQL settlement.
        let lease = feed.read_lease();
        let batch = tokio::task::spawn_blocking(move || lease.read_batch(after, upper))
            .await
            .map_err(|_| RunSettlementFailure::ForwarderJoin)?
            .map_err(|_| RunSettlementFailure::ForwarderRead)?;
        let observed = feed.current_lifecycle();
        let final_upper = upper_bound(&observed)?;
        if after > final_upper {
            return Err(RunSettlementFailure::ForwarderSequence);
        }
        // A read begun while Running may predate the final append and seal.
        // Only an actual read under the fixed sealed bound can establish a gap.
        if batch.is_empty() && matches!(before, WriterLifecycle::Sealed(_)) && after < final_upper {
            return Err(RunSettlementFailure::ForwarderSequence);
        }
        after = offer_batch(*feed.run_id(), &tap, after, final_upper, batch)?;
    }
}

fn upper_bound(state: &WriterLifecycle) -> Result<EventSeq, RunSettlementFailure> {
    match state {
        WriterLifecycle::Running => Ok(EventSeq(i64::MAX as u64)),
        WriterLifecycle::Sealed(prefix) => Ok(*prefix),
        WriterLifecycle::Failed(_) => Err(RunSettlementFailure::WriterInterrupted),
    }
}

fn offer_batch(
    run_id: surge_core::RunId,
    tap: &broadcast::Sender<RunEventTap>,
    mut after: EventSeq,
    upper: EventSeq,
    batch: Vec<ReadEvent>,
) -> ForwarderResult {
    for event in batch {
        if after.0.checked_add(1) != Some(event.seq.0) || event.seq > upper {
            return Err(RunSettlementFailure::ForwarderSequence);
        }
        after = event.seq;
        let _ = tap.send(RunEventTap { run_id, event });
    }
    Ok(after)
}

#[cfg(test)]
mod tests;
