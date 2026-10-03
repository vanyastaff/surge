//! Serial, leased terminal-comment delivery, independent of completion ingestion.

use std::{collections::HashMap, sync::Arc, time::Duration};

use rusqlite::Connection;
use surge_intake::{TaskSource, types::TaskId};
use surge_persistence::{intake_outbox, runs::Clock};
use tokio::sync::{Mutex, watch};
use tracing::warn;

pub(crate) async fn run(
    sources: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            break;
        }
        deliver_one(&sources, &conn, clock.as_ref()).await;
        tokio::select! {
            _ = shutdown.changed() => break,
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
    // A finite final drain never waits for backoff or an ever-growing backlog.
    let final_drain = async {
        for _ in 0..32 {
            if !deliver_one(&sources, &conn, clock.as_ref()).await {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(6), final_drain).await;
}

async fn deliver_one(
    sources: &HashMap<String, Arc<dyn TaskSource>>,
    conn: &Mutex<Connection>,
    clock: &dyn Clock,
) -> bool {
    let claimed = {
        let mut guard = conn.lock().await;
        intake_outbox::claim(&mut guard, clock.now_ms())
    };
    let comment = match claimed {
        Ok(Some(comment)) => comment,
        Ok(None) => return false,
        Err(error) => {
            warn!(%error, "terminal outbox claim failed");
            return false;
        },
    };
    let result = match TaskId::try_new(comment.task_id().to_string()) {
        Ok(task_id) => match sources.get(comment.source_id()) {
            Some(source) => {
                crate::intake_completion::post_comment_bounded(
                    source.as_ref(),
                    &task_id,
                    comment.body(),
                )
                .await
            },
            None => Err(format!(
                "tracker source '{}' is not registered; restore source configuration",
                comment.source_id()
            )),
        },
        Err(error) => Err(format!("invalid queued task identity: {error}")),
    };
    // Lease eligibility is checked at completion time, not before network I/O.
    let mut guard = conn.lock().await;
    let now_ms = clock.now_ms();
    let settled = match result {
        Ok(()) => intake_outbox::acknowledge(&mut guard, &comment, now_ms),
        Err(error) => {
            warn!(%error, task_id = %comment.task_id(), run_id = %comment.run_id(), "terminal comment retained for retry");
            intake_outbox::retry(&guard, &comment, now_ms, &error)
        },
    };
    match settled {
        Ok(true) => {},
        Ok(false) => {
            warn!(run_id = %comment.run_id(), "terminal outbox lease expired or replaced before settlement");
        },
        Err(error) => warn!(%error, "terminal outbox settlement failed; lease will expire"),
    }
    true
}
