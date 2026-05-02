//! Verify that legacy types still behave identically after M1 additions.
//!
//! This is acceptance criterion 8: pure addition means existing code paths
//! must be unaffected.

use surge_core::{
    Spec, Subtask, SubtaskState, SurgeConfig, SurgeError, SurgeEvent, TaskState,
    VersionedEvent,
};

#[test]
fn task_state_terminal_classification_unchanged() {
    assert!(TaskState::Completed.is_terminal());
    assert!(TaskState::Cancelled.is_terminal());
    assert!(TaskState::Failed { reason: "x".into() }.is_terminal());
    assert!(!TaskState::Draft.is_terminal());
    assert!(!TaskState::Planning.is_terminal());
}

#[test]
fn task_state_active_classification_unchanged() {
    assert!(TaskState::Planning.is_active());
    assert!(TaskState::Executing { completed: 1, total: 3 }.is_active());
    assert!(!TaskState::Draft.is_active());
}

#[test]
fn surge_event_versioned_event_constructible() {
    let e = SurgeEvent::AgentConnected { agent_name: "claude".into() };
    let v = VersionedEvent::new(e, 0);
    assert_eq!(v.version, 1);
}

#[test]
fn legacy_subtask_state_terminal() {
    assert!(SubtaskState::Completed.is_terminal());
    assert!(!SubtaskState::Pending.is_terminal());
}

#[test]
fn surge_error_io_from_works() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let _surge: SurgeError = io_err.into();
}

#[test]
fn legacy_types_still_re_exported() {
    // Compile-test: these types must remain reachable from crate root.
    let _ = std::marker::PhantomData::<SurgeConfig>;
    let _ = std::marker::PhantomData::<Spec>;
    let _ = std::marker::PhantomData::<Subtask>;
}
