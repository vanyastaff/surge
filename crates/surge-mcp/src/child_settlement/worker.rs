//! Only this thread polls or kills the retained plain standard child.
use super::{
    ChildParcel, panic_boundary,
    tracker::{self, Completion, Start},
};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

pub(super) fn run(receive: &mpsc::Receiver<Start>, control: &AtomicU8, completion: &Completion) {
    // Actual parcel remains outside every caught operation and is never a sole
    // by-value channel payload. Unexpected panic aborts without unwinding it.
    let mut owned = None;
    panic_boundary::protected(|| {
        match receive.recv() {
            Ok(Start::NoChild | Start::Settled) => {},
            Ok(Start::Child(slot, taken)) => {
                owned = tracker::lock(&slot).take();
                if owned.is_some() {
                    if taken.send(()).is_err() {
                        control.store(tracker::IMMEDIATE, Ordering::SeqCst);
                    }
                    settle_owned(&mut owned, control);
                } else {
                    match receive.recv() {
                        Ok(Start::Settled) => {},
                        _ => std::process::abort(),
                    }
                }
            },
            Err(_) => std::process::abort(),
        }
        completion.finish();
    });
}

pub(super) fn settle_owned(owned: &mut Option<ChildParcel>, control: &AtomicU8) {
    let mut grace = None;
    let mut killed = false;
    let mut ambiguous = false;
    let mut warned = None;
    loop {
        let Some(parcel) = owned.as_mut() else {
            std::process::abort();
        };
        let request = control.load(Ordering::SeqCst);
        if request >= tracker::GRACEFUL && grace.is_none() {
            grace = Some(Instant::now());
        }
        let immediate = request >= tracker::IMMEDIATE
            || grace.is_some_and(|started: Instant| started.elapsed() >= Duration::from_secs(3));
        if immediate && !killed && !ambiguous {
            killed = true;
            let _ = panic_boundary::protected(|| parcel.child.kill());
        }
        match panic_boundary::protected(|| parcel.child.try_wait()) {
            Ok(Some(_status)) => {
                // Direct leader exit alone is not descendant containment proof.
                panic_boundary::protected(|| drop(owned.take()));
                return;
            },
            Ok(None) => {},
            Err(_) => {
                ambiguous = true;
                if warned.is_none_or(|last: Instant| last.elapsed() >= Duration::from_secs(60)) {
                    panic_boundary::protected(
                        || tracing::warn!(target:"mcp::supervisor",reason="mcp_child_wait_uncertain"),
                    );
                    warned = Some(Instant::now());
                }
            },
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
