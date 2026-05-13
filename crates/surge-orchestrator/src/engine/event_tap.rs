//! Engine-level broadcast tap of run-event log appends.
//!
//! Subscribers receive every [`ReadEvent`] appended to any active run as a
//! [`RunEventTap`] message carrying the originating [`RunId`]. The
//! [`Engine`](super::engine::Engine) owns a single
//! [`broadcast::Sender`](tokio::sync::broadcast::Sender) and fans incoming
//! events out from each active run's SQL-backed `subscribe_events()` stream
//! via a per-run forwarder task spawned in
//! [`Engine::start_run`](super::engine::Engine::start_run).
//!
//! Buffer size is fixed at [`TAP_BUFFER_SIZE`]. Subscribers that fall behind
//! receive [`RecvError::Lagged`](tokio::sync::broadcast::error::RecvError::Lagged)
//! from their [`Receiver`](tokio::sync::broadcast::Receiver) and are expected
//! to trigger a full reconcile against the persisted event log rather than
//! attempting to recover the dropped events.
//!
//! See [ADR 0011](../../../../docs/adr/0011-telegram-card-lifecycle.md) and
//! the Telegram cockpit milestone plan for the recovery contract.

use surge_core::id::RunId;
use surge_persistence::runs::ReadEvent;

/// Capacity of the engine-level broadcast channel.
///
/// Sized for the cockpit's normal subscriber latency profile: a single
/// in-flight run typically emits dozens of events per minute, and a buffer
/// of 1024 absorbs short bursts (e.g. a snapshot or a bootstrap stage
/// transition) without forcing a lag-and-reconcile cycle on the cockpit.
pub const TAP_BUFFER_SIZE: usize = 1024;

/// One run-event broadcast onto [`Engine::subscribe_tap`](super::engine::Engine::subscribe_tap).
///
/// `event` carries the persisted [`ReadEvent`] as produced by
/// `RunWriter::subscribe_events`. `run_id` identifies the originating run so a
/// single subscriber can route incoming events across multiple concurrent
/// runs without needing per-run receivers.
#[derive(Debug, Clone)]
pub struct RunEventTap {
    /// The run that appended the event.
    pub run_id: RunId,
    /// The persisted event payload as read back from the storage layer.
    pub event: ReadEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::RunId;
    use surge_core::migrations::MAX_SUPPORTED_VERSION;
    use surge_core::run_event::{EventPayload, VersionedEventPayload};
    use surge_persistence::runs::EventSeq;
    use tokio::sync::broadcast::error::RecvError;

    fn dummy_event(seq: u64) -> ReadEvent {
        ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms: 0,
            kind: "test".to_owned(),
            payload: VersionedEventPayload {
                schema_version: MAX_SUPPORTED_VERSION,
                payload: EventPayload::RunCompleted {
                    terminal_node: surge_core::keys::NodeKey::try_new("end")
                        .expect("valid node key"),
                },
            },
        }
    }

    #[tokio::test]
    async fn two_subscribers_both_receive_appended_event() {
        let (tx, _initial) = tokio::sync::broadcast::channel(TAP_BUFFER_SIZE);
        let mut sub_a = tx.subscribe();
        let mut sub_b = tx.subscribe();

        let run_id = RunId::new();
        let tap = RunEventTap {
            run_id,
            event: dummy_event(1),
        };
        tx.send(tap.clone()).expect("at least one receiver");

        let received_a = sub_a.recv().await.expect("subscriber A receives the tap");
        let received_b = sub_b.recv().await.expect("subscriber B receives the tap");

        assert_eq!(received_a.run_id, run_id);
        assert_eq!(received_b.run_id, run_id);
        assert_eq!(received_a.event.seq, tap.event.seq);
        assert_eq!(received_b.event.seq, tap.event.seq);
    }

    #[tokio::test]
    async fn slow_subscriber_receives_lagged_rather_than_blocks_writer() {
        // Capacity 4 keeps the test fast while still exercising the same
        // broadcast::channel semantics we rely on for TAP_BUFFER_SIZE.
        let (tx, _initial) = tokio::sync::broadcast::channel::<RunEventTap>(4);
        let mut slow = tx.subscribe();

        let run_id = RunId::new();
        // Send 10 events while the slow subscriber drains nothing: capacity 4
        // means events 1..=6 are evicted before any recv() call.
        for seq in 1..=10 {
            let send_result = tx.send(RunEventTap {
                run_id,
                event: dummy_event(seq),
            });
            // The send must succeed — overflow does NOT propagate back to the
            // writer. Lag surfaces on the slow receiver, not on send().
            assert!(send_result.is_ok(), "send must not block or fail on lag");
        }

        // First recv() must surface Lagged with the count of dropped events.
        let lag = slow
            .recv()
            .await
            .expect_err("slow subscriber must observe Lagged");
        match lag {
            RecvError::Lagged(missed) => assert!(
                missed >= 1,
                "Lagged count must report at least one dropped event"
            ),
            RecvError::Closed => panic!("channel must not close while sender is alive"),
        }

        // After absorbing the Lagged signal the receiver continues from the
        // first event still in the buffer.
        let next = slow.recv().await.expect("subscriber resumes after Lagged");
        assert!(
            next.event.seq.0 >= 7,
            "expected next event seq >= 7 after lag, got {}",
            next.event.seq.0
        );
    }
}
