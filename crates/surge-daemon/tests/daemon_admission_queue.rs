//! [`AdmissionController`] FIFO order under concurrent admission. Two
//! tests cover the FIFO contract and the cap-at-8 / queue-the-rest
//! behaviour.

use surge_core::id::RunId;
use surge_daemon::admission::{AdmissionController, AdmissionDecision};

#[tokio::test]
async fn fifo_queue_preserves_order() {
    let a = AdmissionController::new(1);
    let r1 = RunId::new();
    let r2 = RunId::new();
    let r3 = RunId::new();

    assert_eq!(a.try_admit(r1).await, AdmissionDecision::Admitted);
    assert_eq!(
        a.try_admit(r2).await,
        AdmissionDecision::Queued { position: 0 }
    );
    assert_eq!(
        a.try_admit(r3).await,
        AdmissionDecision::Queued { position: 1 }
    );

    a.notify_completed(r1).await;
    assert_eq!(a.pop_queued().await, Some(r2));
    a.notify_completed(r2).await;
    assert_eq!(a.pop_queued().await, Some(r3));
}

#[tokio::test]
async fn cap_8_admits_first_8_queues_rest() {
    let a = AdmissionController::new(8);
    let mut admitted = 0;
    let mut queued = 0;
    for _ in 0..12 {
        match a.try_admit(RunId::new()).await {
            AdmissionDecision::Admitted => admitted += 1,
            AdmissionDecision::Queued { .. } => queued += 1,
        }
    }
    assert_eq!(admitted, 8);
    assert_eq!(queued, 4);
    let s = a.snapshot().await;
    assert_eq!(s.active, 8);
    assert_eq!(s.queued, 4);
}
