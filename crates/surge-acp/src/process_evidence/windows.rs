//! Native Windows identity and conservative named Job Object recovery probes.
use super::{GroupState, ObservationError};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use surge_core::execution_recovery::process::{
    ProcessIdentity, ProcessPlatform, WindowsJobIdentity, WriterContainer, WriterCoverage,
    WriterLiveness,
};
use surge_process::windows_job::Job;
use windows::Win32::Foundation::{FILETIME, HANDLE};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::w;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: u32,
        information: *mut std::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

fn failure(error: impl std::fmt::Display) -> ObservationError {
    ObservationError(error.to_string())
}

fn machine() -> Result<surge_core::ContentHash, ObservationError> {
    let mut buffer = [0_u16; 256];
    let mut bytes = u32::try_from(std::mem::size_of_val(&buffer)).map_err(failure)?;
    // SAFETY: the fixed UTF-16 output buffer is writable for its advertised byte
    // size; all input strings are static NUL-terminated strings. The registry
    // view is explicitly 64-bit so 32-bit and 64-bit hosts share machine identity.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Cryptography"),
            w!("MachineGuid"),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
        .ok()
        .map_err(failure)?;
    }
    let length = buffer.iter().position(|value| *value == 0).ok_or_else(|| {
        ObservationError("machine identity registry value is not terminated".into())
    })?;
    let guid = String::from_utf16(&buffer[..length]).map_err(failure)?;
    let guid = guid.to_ascii_lowercase();
    if guid.len() != 36
        || !guid.chars().enumerate().all(|(index, ch)| {
            if [8, 13, 18, 23].contains(&index) {
                ch == '-'
            } else {
                ch.is_ascii_hexdigit()
            }
        })
    {
        return Err(ObservationError(
            "machine registry identity is invalid".into(),
        ));
    }
    Ok(surge_core::ContentHash::compute(guid.as_bytes()))
}

// The documented PROCESS_TELEMETRY_ID_INFORMATION header is 96 bytes, with
// HeaderSize at offset 0 and BootId at offset 60. Unlike the BCD boot-entry GUID,
// this is the actual boot ID, frozen by the kernel rather than derived from time.
// https://learn.microsoft.com/en-us/windows/win32/devnotes/process_telemetry_id_information_type
// https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntqueryinformationprocess
fn parse_boot(buffer: &[u64], returned: u32) -> Result<String, ObservationError> {
    let capacity = std::mem::size_of_val(buffer);
    let returned = usize::try_from(returned).map_err(failure)?;
    if returned < 96 || returned > capacity {
        return Err(ObservationError(
            "boot telemetry buffer length is invalid".into(),
        ));
    }
    let first = buffer
        .first()
        .ok_or_else(|| ObservationError("missing telemetry header".into()))?
        .to_le_bytes();
    let header = u32::from_le_bytes([first[0], first[1], first[2], first[3]]);
    if header < 96 || usize::try_from(header).map_err(failure)? > returned {
        return Err(ObservationError(
            "boot telemetry header length is invalid".into(),
        ));
    }
    let word = buffer
        .get(7)
        .ok_or_else(|| ObservationError("missing boot ID".into()))?
        .to_le_bytes();
    let boot_id = u32::from_le_bytes([word[4], word[5], word[6], word[7]]);
    // BootId zero is valid; widen before adding one to satisfy the positive
    // durable token contract without overflowing at the maximum DWORD value.
    Ok((u64::from(boot_id) + 1).to_string())
}

fn boot() -> Result<String, ObservationError> {
    // Native telemetry can include bounded variable strings after its header.
    // u64 storage guarantees alignment and one MiB bounds any allocation.
    let mut buffer = vec![0_u64; 131_072];
    let length = u32::try_from(std::mem::size_of_val(buffer.as_slice())).map_err(failure)?;
    let mut returned = 0;
    // SAFETY: GetCurrentProcess returns a valid borrowed pseudo-handle. Class 64
    // requests the documented telemetry structure; the output pointer references
    // aligned writable storage for exactly length bytes throughout the call.
    let status = unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            64,
            buffer.as_mut_ptr().cast(),
            length,
            &raw mut returned,
        )
    };
    if status < 0 {
        return Err(ObservationError(format!(
            "boot telemetry query failed: NTSTATUS {status:#x}"
        )));
    }
    parse_boot(&buffer, returned)
}

fn creation(pid: u32) -> Result<u64, ObservationError> {
    // SAFETY: a non-inheritable process query handle is requested for the supplied
    // PID. On success the SDK returns one owned valid handle.
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map_err(failure)?;
    // SAFETY: ownership of the newly opened handle is transferred exactly once
    // to OwnedHandle, which closes it on every return path below.
    let owned = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the retained process handle grants query access and four distinct
    // initialized output FILETIMEs remain writable for the synchronous call.
    unsafe {
        GetProcessTimes(
            HANDLE(owned.as_raw_handle()),
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .map_err(failure)?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

pub(super) fn observe(pid: u32) -> Result<(ProcessIdentity, u32), ObservationError> {
    let identity = ProcessIdentity::new(
        ProcessPlatform::Windows,
        machine()?,
        boot()?,
        pid,
        creation(pid)?.to_string(),
    )
    .map_err(failure)?;
    // This compatibility value is not a Windows group capability. Only
    // observe_named_container can establish actual Job Object authority.
    Ok((identity, pid))
}

/// Capture a leader only after validating its actual named Job Object membership.
pub fn observe_named_container(
    pid: u32,
    job_name: &str,
    coverage: WriterCoverage,
) -> Result<WriterContainer, ObservationError> {
    let (identity, _) = observe(pid)?;
    let metadata = WindowsJobIdentity::new(job_name.into(), identity.clone()).map_err(failure)?;
    let job = Job::open(metadata.name())
        .map_err(failure)?
        .ok_or_else(|| ObservationError("launched named Job Object is missing".into()))?;
    let created = identity.start().parse::<u64>().map_err(failure)?;
    if !job.contains_process(pid, created).map_err(failure)? {
        return Err(ObservationError(
            "leader is not in its launched Job Object".into(),
        ));
    }
    WriterContainer::with_windows_job(identity, pid, coverage, metadata).map_err(failure)
}

/// Open once and retain the verified job across validation and any later signal.
fn matched_job(container: &WriterContainer) -> Result<Job, GroupState> {
    let metadata = container.windows_job().ok_or_else(|| {
        GroupState::Unknown("legacy Windows container has no named Job Object evidence".into())
    })?;
    match super::old_boot_gone(container.identity()) {
        Ok(true) => return Err(GroupState::Empty),
        Ok(false) => {},
        Err(error) => return Err(GroupState::Unknown(error.to_string())),
    }
    let job = match Job::open(metadata.name()) {
        Ok(Some(job)) => job,
        Ok(None) => return Err(GroupState::Empty),
        Err(error) => return Err(GroupState::Unknown(error.to_string())),
    };
    let start = container
        .identity()
        .start()
        .parse::<u64>()
        .map_err(|error| {
            GroupState::Unknown(format!(
                "Windows creation identity cannot be represented: {error}"
            ))
        })?;
    match job.contains_process(container.identity().pid(), start) {
        Ok(true) => Ok(job),
        Ok(false) => Err(GroupState::Unmatched),
        Err(error) => Err(GroupState::Unknown(error.to_string())),
    }
}

pub(super) fn group_state(container: &WriterContainer) -> GroupState {
    match matched_job(container) {
        Ok(job) => match job.active_processes() {
            Ok(0) => GroupState::Empty,
            Ok(_) => GroupState::LeaderAlive,
            Err(error) => GroupState::Unknown(error.to_string()),
        },
        Err(state) => state,
    }
}

pub(super) fn stop_group(container: &WriterContainer, grace: std::time::Duration) -> GroupState {
    let job = match matched_job(container) {
        Ok(job) => job,
        Err(state) => return state,
    };
    if let Err(error) = job.terminate() {
        return GroupState::Unknown(error.to_string());
    }
    let Some(deadline) = std::time::Instant::now().checked_add(grace) else {
        return GroupState::Unknown("job cleanup grace exceeds clock range".into());
    };
    loop {
        match job.active_processes() {
            Ok(0) => return GroupState::Empty,
            Ok(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            },
            Ok(_) => return GroupState::Unknown("job cleanup grace expired".into()),
            Err(error) => return GroupState::Unknown(error.to_string()),
        }
    }
}

pub(super) fn probe(container: &WriterContainer) -> WriterLiveness {
    match group_state(container) {
        GroupState::LeaderAlive => WriterLiveness::Alive,
        GroupState::Empty if container.coverage() == WriterCoverage::CoveredDomain => {
            WriterLiveness::Gone
        },
        GroupState::Empty => WriterLiveness::Unknown(
            "job is empty but unrestricted descendant/effect coverage is incomplete".into(),
        ),
        GroupState::Unmatched => WriterLiveness::Unknown("job leader identity is unmatched".into()),
        GroupState::Unknown(reason) => WriterLiveness::Unknown(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_boot_parser_checks_header_bounds_and_widens_counter() {
        // Independent byte positions from the Microsoft ABI: HeaderSize=0,
        // BootId=60, fixed header extent=96; no Rust repr(C) oracle duplication.
        for counter in [0_u32, u32::MAX] {
            let mut bytes = [0_u8; 96];
            bytes[..4].copy_from_slice(&96_u32.to_le_bytes());
            bytes[60..64].copy_from_slice(&counter.to_le_bytes());
            let words = bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|chunk| u64::from_le_bytes(*chunk))
                .collect::<Vec<_>>();
            assert_eq!(
                parse_boot(&words, 96).unwrap(),
                (u64::from(counter) + 1).to_string()
            );
            assert!(parse_boot(&words, 95).is_err());
            assert!(parse_boot(&words, 97).is_err());
            let mut invalid = words.clone();
            invalid[0] = 95;
            assert!(parse_boot(&invalid, 96).is_err());
            invalid[0] = 97;
            assert!(parse_boot(&invalid, 96).is_err());
        }
        assert!(parse_boot(&[], 0).is_err());
    }

    #[test]
    fn native_identity_has_stable_machine_boot_and_precise_creation() {
        let (first, _) = observe(std::process::id()).unwrap();
        let (second, _) = observe(std::process::id()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.platform(), ProcessPlatform::Windows);
        assert!(first.start().parse::<u64>().unwrap() > 0);
        assert!(observe_container_without_named_job().is_err());
    }

    fn observe_container_without_named_job() -> Result<WriterContainer, ObservationError> {
        super::super::observe_container(std::process::id(), WriterCoverage::GroupOnly)
    }

    #[test]
    fn legacy_old_boot_and_foreign_machine_tokens_never_authorize_cleanup() {
        let (current, _) = observe(std::process::id()).unwrap();
        for machine in [
            *current.machine(),
            surge_core::ContentHash::compute(b"foreign"),
        ] {
            let identity = ProcessIdentity::new(
                ProcessPlatform::Windows,
                machine,
                "1".into(),
                current.pid(),
                current.start().into(),
            )
            .unwrap();
            let legacy =
                WriterContainer::new(identity, current.pid(), WriterCoverage::GroupOnly).unwrap();
            assert!(matches!(group_state(&legacy), GroupState::Unknown(_)));
            assert!(matches!(
                stop_group(&legacy, std::time::Duration::ZERO),
                GroupState::Unknown(_)
            ));
            assert!(matches!(probe(&legacy), WriterLiveness::Unknown(_)));
        }
    }

    #[test]
    fn named_foreign_machine_is_unknown_and_old_boot_is_group_only_empty() {
        let (current, _) = observe(std::process::id()).unwrap();
        let different_boot = if current.boot() == "1" { "2" } else { "1" };
        for (machine, expected) in [
            (*current.machine(), GroupState::Empty),
            (
                surge_core::ContentHash::compute(b"foreign"),
                GroupState::Unknown(String::new()),
            ),
        ] {
            let identity = ProcessIdentity::new(
                ProcessPlatform::Windows,
                machine,
                different_boot.into(),
                current.pid(),
                current.start().into(),
            )
            .unwrap();
            let job = WindowsJobIdentity::new(
                format!("Global\\SurgeMcp-{}", "a".repeat(64)),
                identity.clone(),
            )
            .unwrap();
            let container = WriterContainer::with_windows_job(
                identity,
                current.pid(),
                WriterCoverage::GroupOnly,
                job,
            )
            .unwrap();
            let observed = stop_group(&container, std::time::Duration::ZERO);
            match expected {
                GroupState::Empty => assert_eq!(observed, GroupState::Empty),
                _ => assert!(matches!(observed, GroupState::Unknown(_))),
            }
            assert!(matches!(probe(&container), WriterLiveness::Unknown(_)));
        }
    }

    #[test]
    fn authenticated_job_rejects_reused_creation_then_stops_exact_leader() {
        use surge_process::windows_job::{LaunchSpec, NativeChild};
        let mut environment = std::env::vars_os().collect::<Vec<_>>();
        environment.push(("SURGE_ACP_OWNED_JOB_TEST_CHILD".into(), "1".into()));
        let mut child = NativeChild::spawn(&LaunchSpec {
            program: std::env::current_exe().unwrap().into_os_string(),
            args: [
                "--ignored",
                "--exact",
                "process_evidence::windows::tests::controlled_job_child",
                "--nocapture",
            ]
            .into_iter()
            .map(Into::into)
            .collect(),
            environment,
            current_dir: None,
        })
        .unwrap();
        let observed =
            observe_named_container(child.id(), child.job_name(), WriterCoverage::GroupOnly)
                .unwrap();
        assert_eq!(group_state(&observed), GroupState::LeaderAlive);
        let identity = observed.identity();
        let forged_start = identity
            .start()
            .parse::<u64>()
            .unwrap()
            .checked_add(1)
            .unwrap();
        let forged_identity = ProcessIdentity::new(
            ProcessPlatform::Windows,
            *identity.machine(),
            identity.boot().into(),
            identity.pid(),
            forged_start.to_string(),
        )
        .unwrap();
        let forged_job =
            WindowsJobIdentity::new(child.job_name().into(), forged_identity.clone()).unwrap();
        let forged = WriterContainer::with_windows_job(
            forged_identity,
            child.id(),
            WriterCoverage::GroupOnly,
            forged_job,
        )
        .unwrap();
        assert_eq!(group_state(&forged), GroupState::Unmatched);
        assert_eq!(
            stop_group(&forged, std::time::Duration::from_secs(2)),
            GroupState::Unmatched
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "forged cleanup terminated the live child"
        );
        assert_eq!(group_state(&observed), GroupState::LeaderAlive);
        assert_eq!(
            stop_group(&observed, std::time::Duration::from_secs(5)),
            GroupState::Empty
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native child did not settle after empty job"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            matches!(probe(&observed), WriterLiveness::Unknown(_)),
            "group-only stop cannot prove unrestricted effect closure"
        );
    }

    #[test]
    #[ignore = "controlled subprocess-only fixture; parent owns and terminates the native job"]
    fn controlled_job_child() {
        assert_eq!(
            std::env::var("SURGE_ACP_OWNED_JOB_TEST_CHILD").as_deref(),
            Ok("1")
        );
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    #[test]
    fn missing_named_job_only_proves_group_absence() {
        let (identity, _) = observe(std::process::id()).unwrap();
        let name = format!(
            "Global\\SurgeMcp-{:032x}{:032x}",
            ulid::Ulid::new().0,
            ulid::Ulid::new().0
        );
        let job = WindowsJobIdentity::new(name, identity.clone()).unwrap();
        let container = WriterContainer::with_windows_job(
            identity.clone(),
            identity.pid(),
            WriterCoverage::GroupOnly,
            job,
        )
        .unwrap();
        assert_eq!(group_state(&container), GroupState::Empty);
        assert_eq!(
            stop_group(&container, std::time::Duration::ZERO),
            GroupState::Empty
        );
        assert!(matches!(probe(&container), WriterLiveness::Unknown(_)));
    }
}
