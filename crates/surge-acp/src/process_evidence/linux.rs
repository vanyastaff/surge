use super::ObservationError;
use surge_core::{
    ContentHash,
    execution_recovery::process::{ProcessIdentity, ProcessPlatform},
};
pub(super) fn observe(pid: u32) -> Result<(ProcessIdentity, u32), ObservationError> {
    let machine =
        std::fs::read("/etc/machine-id").map_err(|error| ObservationError(error.to_string()))?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|error| ObservationError(error.to_string()))?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|error| ObservationError(error.to_string()))?;
    let (_, tail) = stat
        .rsplit_once(") ")
        .ok_or_else(|| ObservationError("invalid process stat record".into()))?;
    let fields: Vec<_> = tail.split_whitespace().collect();
    let group = fields
        .get(2)
        .ok_or_else(|| ObservationError("process group missing".into()))?
        .parse::<u32>()
        .map_err(|error| ObservationError(error.to_string()))?;
    let start = fields
        .get(19)
        .ok_or_else(|| ObservationError("process creation identity missing".into()))?;
    let identity = ProcessIdentity::new(
        ProcessPlatform::Linux,
        ContentHash::compute(&machine),
        boot.trim().into(),
        pid,
        (*start).into(),
    )
    .map_err(|error| ObservationError(error.to_string()))?;
    Ok((identity, group))
}
