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
