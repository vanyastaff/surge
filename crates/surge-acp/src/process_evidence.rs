//! Read-only local writer probes. Empty groups do not establish containment.
use surge_core::execution_recovery::process::{
    ProcessIdentity, WriterContainer, WriterCoverage, WriterLiveness,
};
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

/// Observation failed; callers must retain Unknown rather than infer cleanup.
#[derive(Debug, thiserror::Error)]
#[error("process identity observation failed: {0}")]
pub struct ObservationError(pub String);

/// Capture machine/boot/process-start identity without changing any process.
pub fn observe(pid: u32) -> Result<(ProcessIdentity, u32), ObservationError> {
    #[cfg(target_os = "linux")]
    {
        linux::observe(pid)
    }
    #[cfg(target_os = "macos")]
    {
        macos::observe(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Err(ObservationError(
            "platform process identity probe unavailable".into(),
        ))
    }
}
/// Record an isolated owned group. This does not upgrade group-only coverage.
pub fn observe_container(
    pid: u32,
    coverage: WriterCoverage,
) -> Result<WriterContainer, ObservationError> {
    let (identity, group) = observe(pid)?;
    if group != pid {
        return Err(ObservationError(
            "process is not its own isolated group leader".into(),
        ));
    }
    WriterContainer::new(identity, group, coverage)
        .map_err(|error| ObservationError(error.to_string()))
}
/// Prove old-boot absence only for the same captured machine and supported platform.
pub fn old_boot_gone(identity: &ProcessIdentity) -> Result<bool, ObservationError> {
    let (current, _) = observe(std::process::id())?;
    if current.platform() != identity.platform() || current.machine() != identity.machine() {
        return Err(ObservationError(
            "writer identity belongs to another platform or machine".into(),
        ));
    }
    if current.boot() == identity.boot() {
        return Ok(false);
    }
    Ok(true)
}
/// Inspect a covered group without signalling or killing its members.
/// Occupancy of a recorded process group, without coverage interpretation.
///
/// ADR-0021 accepts an empty `GroupOnly` group as best-effort cleanup; it never
/// upgrades coverage, so callers must not report this as confirmed closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupState {
    /// No process remains in the recorded group.
    Empty,
    /// The recorded leader identity is still running in its group.
    LeaderAlive,
    /// The group exists but its leader identity cannot be matched.
    Unmatched,
    /// The group could not be observed.
    Unknown(String),
}

/// Observe whether the recorded group still has members.
#[must_use]
pub fn group_state(container: &WriterContainer) -> GroupState {
    match old_boot_gone(container.identity()) {
        Ok(true) => return GroupState::Empty,
        Ok(false) => {},
        Err(error) => return GroupState::Unknown(error.to_string()),
    }
    #[cfg(unix)]
    {
        let Ok(group) = i32::try_from(container.group()) else {
            return GroupState::Unknown("group identity is out of range".into());
        };
        match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(group), None) {
            Ok(()) => match observe(container.identity().pid()) {
                Ok((current, current_group))
                    if &current == container.identity() && current_group == container.group() =>
                {
                    GroupState::LeaderAlive
                },
                _ => GroupState::Unmatched,
            },
            Err(nix::errno::Errno::ESRCH) => GroupState::Empty,
            Err(error) => GroupState::Unknown(format!("group observation failed: {error}")),
        }
    }
    #[cfg(not(unix))]
    {
        GroupState::Unknown("platform container probe unavailable".into())
    }
}

/// Stop a recorded group whose leader identity still matches (ADR-0021).
///
/// Sends SIGTERM to the group, waits up to `grace`, then SIGKILL and waits up to
/// `grace` again. A group whose leader identity cannot be matched is never
/// signalled, so a reused group id cannot be hit. Blocking: run off async workers.
#[must_use]
pub fn stop_group(container: &WriterContainer, grace: std::time::Duration) -> GroupState {
    let state = group_state(container);
    if state != GroupState::LeaderAlive {
        return state;
    }
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, killpg};
        let Ok(group) = i32::try_from(container.group()) else {
            return GroupState::Unknown("group identity is out of range".into());
        };
        let group = nix::unistd::Pid::from_raw(group);
        for signal in [Signal::SIGTERM, Signal::SIGKILL] {
            if group_state(container) != GroupState::LeaderAlive {
                break;
            }
            match killpg(group, signal) {
                Ok(()) | Err(nix::errno::Errno::ESRCH) => {},
                Err(error) => {
                    return GroupState::Unknown(format!("group {signal} failed: {error}"));
                },
            }
            let deadline = std::time::Instant::now() + grace;
            while std::time::Instant::now() < deadline {
                if group_state(container) == GroupState::Empty {
                    return GroupState::Empty;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        group_state(container)
    }
    #[cfg(not(unix))]
    {
        let _ = grace;
        state
    }
}

pub fn probe(container: &WriterContainer) -> WriterLiveness {
    match old_boot_gone(container.identity()) {
        Ok(true) => return WriterLiveness::Gone,
        Ok(false) => {},
        Err(error) => return WriterLiveness::Unknown(error.to_string()),
    }
    #[cfg(unix)]
    {
        let Ok(group) = i32::try_from(container.group()) else {
            return WriterLiveness::Unknown("group identity is out of range".into());
        };
        match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(group),None) {
            Ok(())=>match observe(container.identity().pid()) {
                Ok((current,current_group)) if &current==container.identity() && current_group==container.group()=>WriterLiveness::Alive,
                _=>WriterLiveness::Unknown("group exists but leader identity cannot be matched; descendants or a reused identity may exist".into()),
            },
            Err(nix::errno::Errno::ESRCH) if container.coverage()==WriterCoverage::CoveredDomain=>WriterLiveness::Gone,
            Err(nix::errno::Errno::ESRCH)=>WriterLiveness::Unknown("group is empty but unrestricted descendant/effect coverage is incomplete".into()),
            Err(error)=>WriterLiveness::Unknown(format!("group observation failed: {error}")),
        }
    }
    #[cfg(not(unix))]
    {
        WriterLiveness::Unknown("platform container probe unavailable".into())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    fn spawn_group(script: &str) -> std::process::Child {
        std::process::Command::new("/bin/sh")
            .args(["-c", script])
            .process_group(0)
            .spawn()
            .expect("spawn owned group leader")
    }

    #[test]
    fn stop_group_escalates_to_kill_and_reports_an_empty_group() {
        // The leader ignores SIGTERM, so only the SIGKILL step can empty the group.
        let ready = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let script = format!(
            "exec /usr/bin/python3 -c 'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); open(\"{}\", \"w\").close(); time.sleep(60)'",
            ready.display()
        );
        std::fs::remove_file(&ready).unwrap();
        let mut child = spawn_group(&script);
        while !ready.exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let container = observe_container(child.id(), WriterCoverage::GroupOnly).unwrap();
        assert_eq!(group_state(&container), GroupState::LeaderAlive);
        let reaper = std::thread::spawn(move || child.wait());
        let grace = std::time::Duration::from_millis(300);
        let started = std::time::Instant::now();
        let state = stop_group(&container, grace);
        let _ = reaper.join();
        assert_eq!(state, GroupState::Empty);
        assert!(
            started.elapsed() >= grace,
            "SIGTERM alone emptied a TERM-ignoring group"
        );
    }

    #[test]
    fn stop_group_never_signals_an_already_empty_group() {
        let mut child = spawn_group("exit 0");
        let container = observe_container(child.id(), WriterCoverage::GroupOnly);
        let _ = child.wait();
        // Observation can lose the race with a fast exit; both outcomes stay unsignalled.
        if let Ok(container) = container {
            assert_eq!(
                stop_group(&container, std::time::Duration::from_millis(50)),
                GroupState::Empty
            );
        }
    }
}
