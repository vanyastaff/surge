//! Retained Windows PID ownership; never infer staleness from an API failure.
use super::PidfileError;
use std::path::{Path, PathBuf};
use surge_persistence::{RuntimeControlFile, RuntimeDirectory, RuntimeHomeOwner};

/// Own the original PID object, its exclusive lock and protected namespace.
pub struct PidfileGuard {
    file: Option<RuntimeControlFile>,
    owner: RuntimeHomeOwner,
    home: PathBuf,
}

impl PidfileGuard {
    /// Acquire the protected daemon PID file under the canonical selected home.
    ///
    /// # Errors
    /// Refuses unsafe paths/security, live or uncertain PID owners and contention.
    pub fn acquire(pid: u32) -> Result<Self, PidfileError> {
        let home = surge_core::home::surge_home_dir().ok_or(PidfileError::NoHome)?;
        Self::acquire_in(&home, pid)
    }

    fn acquire_in(home: &Path, pid: u32) -> Result<Self, PidfileError> {
        if pid == 0 {
            return Err(PidfileError::Malformed("zero process id".into()));
        }
        let owner = RuntimeHomeOwner::prepare(home)?;
        let directory = owner.directory(RuntimeDirectory::Daemon)?;
        let mut file = directory.open_control("daemon.pid".as_ref())?;
        if !file.try_lock_exclusive()? {
            return Err(PidfileError::Busy);
        }
        let bytes = file.read_bounded(64)?;
        if file.was_created() {
            if !bytes.is_empty() {
                return Err(PidfileError::Malformed("new PID file is not empty".into()));
            }
        } else {
            let stored = parse_pid(&bytes)?;
            if is_alive(stored)? {
                return Err(PidfileError::AlreadyRunning(stored));
            }
        }
        file.replace_bytes(pid.to_string().as_bytes())?;
        // Arm deletion only after the complete own-PID publication succeeds.
        Ok(Self {
            file: Some(file),
            owner,
            home: home.to_owned(),
        })
    }

    /// The immutable home selected at acquisition.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Publish the version through a protected, exclusively owned control file.
    ///
    /// # Errors
    /// Refuses unsafe existing markers, contention and native write/flush failures.
    pub fn write_version(&self, version: &str) -> Result<(), PidfileError> {
        let directory = self.owner.directory(RuntimeDirectory::Daemon)?;
        let mut file = directory.open_control("version".as_ref())?;
        if !file.try_lock_exclusive()? {
            return Err(PidfileError::Busy);
        }
        file.replace_bytes(version.as_bytes())?;
        Ok(())
    }

    /// Remove only the original locked PID object after runtime shutdown.
    ///
    /// # Errors
    /// Returns descriptor deletion or complete parent flush failures.
    pub fn release(mut self) -> Result<(), PidfileError> {
        if let Some(file) = self.file.take() {
            file.remove()?;
        }
        Ok(())
    }
}

impl Drop for PidfileGuard {
    fn drop(&mut self) {
        if let Some(file) = self.file.take()
            && let Err(error) = file.remove()
        {
            tracing::error!(%error, "failed to remove owned daemon PID file");
        }
    }
}

fn parse_pid(bytes: &[u8]) -> Result<u32, PidfileError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| PidfileError::Malformed("invalid UTF-8".into()))?
        .trim();
    value
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid != 0)
        .ok_or_else(|| PidfileError::Malformed("expected nonzero process id".into()))
}

// ProcessTracker's private bool API deliberately collapses unknown to live.
// PID replacement needs a typed error, so observe the retained native handle here.
pub(super) fn is_alive(pid: u32) -> Result<bool, PidfileError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    if pid == 0 {
        return Err(PidfileError::Malformed("zero process id".into()));
    }
    // SAFETY: scalar PID/access inputs; successful OpenProcess returns an owned handle.
    let handle = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(handle) => handle,
        Err(error)
            if error.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) =>
        {
            return Ok(false);
        },
        Err(error) => return Err(std::io::Error::other(error).into()),
    };
    // SAFETY: transfer the one newly acquired process handle into its sole owner.
    let owned = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    // SAFETY: owned retains SYNCHRONIZE access through this nonblocking observation.
    match unsafe { WaitForSingleObject(HANDLE(owned.as_raw_handle()), 0) } {
        WAIT_OBJECT_0 => Ok(false),
        WAIT_TIMEOUT => Ok(true),
        WAIT_FAILED => Err(std::io::Error::last_os_error().into()),
        status => Err(std::io::Error::other(format!(
            "unexpected process wait status {}",
            status.0
        ))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));

    fn seed(home: &Path, bytes: &[u8]) {
        let owner = RuntimeHomeOwner::prepare(home).unwrap();
        let directory = owner.directory(RuntimeDirectory::Daemon).unwrap();
        let mut file = directory.open_control("daemon.pid".as_ref()).unwrap();
        assert!(file.try_lock_exclusive().unwrap());
        file.replace_bytes(bytes).unwrap();
    }

    #[test]
    fn guard_blocks_second_owner_then_releases_original_file() {
        let home = FixtureHome::new().unwrap();
        let pid = std::process::id();
        let guard = PidfileGuard::acquire_in(home.path(), pid).unwrap();
        assert_eq!(
            std::fs::read(home.path().join("daemon/daemon.pid")).unwrap(),
            pid.to_string().as_bytes()
        );
        let second = PidfileGuard::acquire_in(home.path(), pid);
        assert!(matches!(
            second,
            Err(PidfileError::Busy
                | PidfileError::Ownership(surge_persistence::PersistenceError::OwnershipBusy))
        ));
        guard.release().unwrap();
        assert!(!home.path().join("daemon/daemon.pid").exists());
        PidfileGuard::acquire_in(home.path(), pid)
            .unwrap()
            .release()
            .unwrap();
        home.close().unwrap();
    }

    #[test]
    fn current_process_pid_refuses_without_deleting_existing_file() {
        let home = FixtureHome::new().unwrap();
        let bytes = std::process::id().to_string().into_bytes();
        seed(home.path(), &bytes);
        assert!(
            matches!(PidfileGuard::acquire_in(home.path(), std::process::id()), Err(PidfileError::AlreadyRunning(pid)) if pid == std::process::id())
        );
        assert_eq!(
            std::fs::read(home.path().join("daemon/daemon.pid")).unwrap(),
            bytes
        );
        home.close().unwrap();
    }

    #[test]
    fn empty_zero_malformed_and_oversized_existing_pid_remain_unchanged() {
        for bytes in [vec![], b"0".to_vec(), b"not-a-pid".to_vec(), vec![b'1'; 65]] {
            let home = FixtureHome::new().unwrap();
            seed(home.path(), &bytes);
            assert!(PidfileGuard::acquire_in(home.path(), std::process::id()).is_err());
            assert_eq!(
                std::fs::read(home.path().join("daemon/daemon.pid")).unwrap(),
                bytes
            );
            home.close().unwrap();
        }
    }

    #[test]
    fn terminated_process_with_exit_259_is_stale_even_with_retained_handle() {
        let home = FixtureHome::new().unwrap();
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "exit 259"])
            .spawn()
            .unwrap();
        let old_pid = child.id();
        assert_eq!(child.wait().unwrap().code(), Some(259));
        seed(home.path(), old_pid.to_string().as_bytes());
        let guard = PidfileGuard::acquire_in(home.path(), std::process::id()).unwrap();
        assert_eq!(
            std::fs::read(home.path().join("daemon/daemon.pid")).unwrap(),
            std::process::id().to_string().as_bytes()
        );
        guard.release().unwrap();
        drop(child);
        home.close().unwrap();
    }
}
