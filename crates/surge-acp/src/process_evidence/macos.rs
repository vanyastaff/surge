use super::ObservationError;
use surge_core::{
    ContentHash,
    execution_recovery::process::{ProcessIdentity, ProcessPlatform},
};
fn machine() -> Result<ContentHash, ObservationError> {
    let mut uuid = [0u8; 16];
    let timeout = libc::timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    // SAFETY: uuid is a writable 16-byte UUID buffer and timeout is a live timespec.
    let result = unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &timeout) };
    if result != 0 {
        return Err(ObservationError(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(ContentHash::compute(&uuid))
}
fn boot() -> Result<String, ObservationError> {
    // Wall-clock-derived kern.boottime can change without a reboot. Only the
    // immutable kernel boot-session UUID can establish old-boot disappearance.
    let mut value = [0u8; 64];
    let mut size = value.len();
    // SAFETY: oldp points to a writable 64-byte buffer and oldlen reports its
    // exact capacity. The NUL-terminated name is static; no new value is supplied.
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            value.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || size != 37 || value[36] != 0 {
        return Err(ObservationError(
            "immutable kernel boot identity unavailable".into(),
        ));
    }
    std::str::from_utf8(&value[..36])
        .map(str::to_owned)
        .map_err(|error| ObservationError(error.to_string()))
}
pub(super) fn observe(pid: u32) -> Result<(ProcessIdentity, u32), ObservationError> {
    let pid_signed = i32::try_from(pid).map_err(|error| ObservationError(error.to_string()))?;
    if pid_signed <= 0 {
        return Err(ObservationError("process ID must be positive".into()));
    }
    let mut value = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .map_err(|error| ObservationError(error.to_string()))?;
    // SAFETY: buffer points to size writable bytes; proc_pidinfo is a read-only process query.
    let written = unsafe {
        libc::proc_pidinfo(
            pid_signed,
            libc::PROC_PIDTBSDINFO,
            0,
            value.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return Err(ObservationError(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    // SAFETY: a full-size successful proc_pidinfo initialized every field of proc_bsdinfo.
    let value = unsafe { value.assume_init() };
    if value.pbi_pid != pid || value.pbi_start_tvsec == 0 {
        return Err(ObservationError(
            "process identity changed during observation".into(),
        ));
    }
    let start = u128::from(value.pbi_start_tvsec) * 1_000_000 + u128::from(value.pbi_start_tvusec);
    let identity = ProcessIdentity::new(
        ProcessPlatform::MacOs,
        machine()?,
        boot()?,
        pid,
        start.to_string(),
    )
    .map_err(|error| ObservationError(error.to_string()))?;
    Ok((identity, value.pbi_pgid))
}
