//! Each terminal barrier runs in its own actual host process.
use super::*;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

struct Owner(Arc<AtomicBool>);
struct TerminalBarrier;
impl Drop for TerminalBarrier {
    fn drop(&mut self) {
        shutdown_children_and_join();
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl HostWriterObserver for Owner {
    async fn before_child(
        &self,
        _: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, crate::writer_observer::WriterObservationError>
    {
        Ok(surge_core::id::ExecutionWriterId::new())
    }
    async fn child_started(
        &self,
        _: surge_core::id::ExecutionWriterId,
        _: Option<u32>,
    ) -> Result<(), crate::writer_observer::WriterObservationError> {
        Ok(())
    }
}

fn run_isolated(case: &str) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_settlement::tests::terminal_host_probe",
            "--nocapture",
        ])
        .env("SURGE_SETTLEMENT_TEST_CASE", case)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "isolated terminal settlement case failed: {case}"
            );
            return;
        }
        if started.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated terminal settlement case timed out: {case}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn unpolled_close_and_retained_send_settle_after_runtime_drop() {
    run_isolated("close");
    run_isolated("send");
}

#[test]
fn concurrent_terminal_barriers_hold_pending_reservations_and_close_admission() {
    run_isolated("barriers");
}

#[test]
fn owner_capacity_and_failed_pipe_preparation_keep_real_settlement() {
    run_isolated("capacity");
    run_isolated("pipes");
}

#[test]
fn terminal_host_probe() {
    let Ok(case) = std::env::var("SURGE_SETTLEMENT_TEST_CASE") else {
        return;
    };
    if case == "barriers" {
        pending_barriers();
        return;
    }
    if case == "capacity" {
        bounded_capacity();
        return;
    }
    if case == "pipes" {
        failed_pipes();
        return;
    }
    let _terminal = TerminalBarrier;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let (mut transport, stderr) = {
        let _entered = runtime.enter();
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "exec sleep 60"]);
        ObservedTransport::spawn(command, Some(Arc::new(Owner(dropped.clone()))), "fixture")
            .unwrap()
    };
    let pid = transport.id();
    drop(stderr);
    assert!(!dropped.load(Ordering::SeqCst));
    if case == "close" {
        let requested = Instant::now();
        drop(transport.close());
        drop(runtime);
        while !dropped.load(Ordering::SeqCst) && requested.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let graceful_settled = dropped.load(Ordering::SeqCst);
        let before_barrier = std::process::Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .unwrap();
        drop(transport);
        shutdown_children_and_join();
        assert!(
            graceful_settled,
            "never-polled close failed to request settlement while transport stayed alive"
        );
        assert!(
            before_barrier.stdout.is_empty(),
            "graceful completion did not reap the actual child before transport drop/barrier"
        );
        assert!(
            requested.elapsed() >= Duration::from_secs(3),
            "fixture exited without exercising graceful timeout"
        );
    } else {
        let message =
            serde_json::from_value(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
                .unwrap();
        let pending_send = transport.send(message);
        drop(runtime);
        drop(transport);
        shutdown_children_and_join();
        drop(pending_send);
    }
    assert!(
        dropped.load(Ordering::SeqCst),
        "original owner was not dropped after observed exit"
    );
    let probe = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    assert!(
        probe.stdout.is_empty(),
        "actual child still exists after terminal settlement"
    );
    assert!(
        tracker::reserve().is_err(),
        "terminal host reopened admission"
    );
}

fn bounded_capacity() {
    let mut admitted = Vec::new();
    for _ in 0..128 {
        admitted.push(tracker::reserve().unwrap());
    }
    assert!(
        tracker::reserve().is_err(),
        "host admitted more than the fixed owner limit"
    );
    for (send, ticket) in admitted {
        send.send(tracker::Start::NoChild).unwrap();
        ticket.completion.wait();
    }
    let start = Instant::now();
    let (send, ticket) = loop {
        if let Ok(reservation) = tracker::reserve() {
            break reservation;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "finished owner handles were not reaped"
        );
        std::thread::yield_now();
    };
    send.send(tracker::Start::NoChild).unwrap();
    ticket.completion.wait();
    shutdown_children_and_join();
    assert!(tracker::reserve().is_err());
}

fn failed_pipes() {
    let _terminal = TerminalBarrier;
    let (send, ticket) = tracker::reserve().unwrap();
    let reservation = Reservation {
        send,
        ticket,
        child_spawned: true,
    };
    let dropped = Arc::new(AtomicBool::new(false));
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", "exec sleep 2"])
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let slot = Arc::new(Mutex::new(Some(ChildParcel {
        child,
        _owner: Some(Arc::new(Owner(dropped.clone()))),
    })));
    let failure = pipes(&slot);
    let start = Instant::now();
    fallback(&slot, &reservation);
    reservation.ticket.completion.wait();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "failed pipe conversion did not immediately settle the actual child"
    );
    assert!(failure.is_err());
    assert!(dropped.load(Ordering::SeqCst));
    assert!(tracker::lock(&slot).is_none());
    let probe = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    assert!(
        probe.stdout.is_empty(),
        "fallback released owner before observed child exit"
    );
}

fn pending_barriers() {
    let (send, ticket) = tracker::reserve().unwrap();
    let first_done = Arc::new(AtomicBool::new(false));
    let first_flag = first_done.clone();
    let first = std::thread::spawn(move || {
        shutdown_children_and_join();
        first_flag.store(true, Ordering::SeqCst);
    });
    let start = Instant::now();
    while ticket.control.load(Ordering::SeqCst) != tracker::IMMEDIATE {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
    let second_done = Arc::new(AtomicBool::new(false));
    let second_flag = second_done.clone();
    let second = std::thread::spawn(move || {
        shutdown_children_and_join();
        second_flag.store(true, Ordering::SeqCst);
    });
    let entered = Instant::now();
    while tracker::waiting_barriers() == 0 {
        assert!(entered.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
    assert!(!first_done.load(Ordering::SeqCst));
    assert!(!second_done.load(Ordering::SeqCst));
    send.send(tracker::Start::NoChild).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    assert!(first_done.load(Ordering::SeqCst) && second_done.load(Ordering::SeqCst));
    assert!(tracker::reserve().is_err());
}
