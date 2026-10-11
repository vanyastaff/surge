//! Opt-in, run-bound test controls for actual event-reader ownership and pool failures.

use surge_core::RunId;
use surge_persistence::runs::{FeedPoolProbe, FeedReadGate, RunEventFeed};
use tokio::sync::oneshot;

use super::{Engine, EngineError};

/// Which actual worker boundary the next matching feed stops at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedProbeKind {
    /// Stop before actual checkout; expose an opaque real-pool exhaustion controller.
    BeforeCheckout,
    /// Stop after actual checkout; controller retains only barrier channels.
    AfterCheckout,
}

/// Typed controls transferred from the actual run feed to its fixture.
pub enum FeedProbe {
    /// Pool exhaustion controls intentionally retain a real pool/lease until dropped.
    BeforeCheckout {
        /// Channel-only pre-checkout barrier.
        gate: FeedReadGate,
        /// Opaque owner of the actual reader pool and writer lifecycle observation.
        pool: FeedPoolProbe,
    },
    /// This controller owns no reader, pool, connection, or writer lease.
    AfterCheckout {
        /// Channel-only barrier after the worker acquired its real connection.
        gate: FeedReadGate,
    },
}

pub(super) struct FeedProbeRequest {
    run: RunId,
    kind: FeedProbeKind,
    reply: oneshot::Sender<Result<FeedProbe, EngineError>>,
}

impl Engine {
    /// Install one typed, run-bound probe before starting or resuming that run.
    ///
    /// Duplicate installation and poisoned state fail without replacing a pending probe.
    /// The actual feed consumes the request once, including on a run-ID mismatch.
    pub fn install_feed_probe_for_test(
        &self,
        run: RunId,
        kind: FeedProbeKind,
    ) -> Result<oneshot::Receiver<Result<FeedProbe, EngineError>>, EngineError> {
        let mut pending = self
            .next_feed_probe
            .lock()
            .map_err(|_| EngineError::Internal("feed test probe state poisoned".into()))?;
        if pending.is_some() {
            return Err(EngineError::Internal(
                "feed test probe already installed".into(),
            ));
        }
        let (reply, result) = oneshot::channel();
        *pending = Some(FeedProbeRequest { run, kind, reply });
        Ok(result)
    }

    pub(super) fn configure_feed_for_test(
        &self,
        mut feed: RunEventFeed,
    ) -> Result<RunEventFeed, EngineError> {
        let request = self
            .next_feed_probe
            .lock()
            .map_err(|_| EngineError::Internal("feed test probe state poisoned".into()))?
            .take();
        let Some(request) = request else {
            return Ok(feed);
        };
        let installed = if request.run == *feed.run_id() {
            match request.kind {
                FeedProbeKind::BeforeCheckout => feed
                    .install_pool_probe_for_test()
                    .map(|(gate, pool)| FeedProbe::BeforeCheckout { gate, pool }),
                FeedProbeKind::AfterCheckout => feed
                    .install_read_gate_for_test()
                    .map(|gate| FeedProbe::AfterCheckout { gate }),
            }
            .map_err(|error| error.to_string())
        } else {
            Err("feed test probe run identity mismatch".to_owned())
        };
        match installed {
            Ok(probe) => request.reply.send(Ok(probe)).map_err(|_undelivered| {
                // Dropping the unsent controller releases its installed barrier.
                EngineError::Internal("feed test probe receiver was dropped".into())
            })?,
            Err(diagnostic) => {
                let _ = request
                    .reply
                    .send(Err(EngineError::Storage(diagnostic.clone())));
                return Err(EngineError::Storage(diagnostic));
            },
        }
        Ok(feed)
    }
}
