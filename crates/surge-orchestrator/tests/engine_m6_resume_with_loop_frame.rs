//! M6 placeholder: crash-resume e2e with an active LoopFrame in the snapshot.
//! Full crash-resume requires daemon kill semantics (kill the process mid-loop,
//! restart, verify LoopFrame restored at correct current_index).
//! Snapshot v1→v2 reader and replay event handling are covered at unit level
//! in PR-1 (engine::snapshot + engine::replay tests).

#[test]
#[ignore = "M6 crash-resume inside loop: requires daemon-mode kill semantics (M7)"]
fn resume_after_crash_inside_loop_restores_loop_frame() {
    // M7: start a multi-iteration loop, kill the engine process mid-iteration,
    // restart, call engine.resume_run(), verify:
    // (a) LoopFrame.current_index continues from where it was.
    // (b) No LoopIterationStarted events are repeated for already-completed iters.
}
