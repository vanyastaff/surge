//! Cross-process advisory lock around per-run events.sqlite.lock.

#[cfg(not(windows))]
use std::fs::{File, OpenOptions};
#[cfg(not(windows))]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::runs::error::OpenError;

/// Advisory file lock held by the live writer for cross-process exclusion.
///
/// On Unix uses `flock(LOCK_EX | LOCK_NB)` semantics (advisory, cooperative);
/// on Windows uses `LockFileEx` with
/// `LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY`. Either way, only
/// cooperating processes that take the same lock will be blocked —
/// non-cooperating tools (Windows Explorer reading the file) are not.
///
/// The owned file closes on every failed acquisition and on final lease drop.
pub struct FileLock {
    path: std::path::PathBuf,
    #[cfg(not(windows))]
    file: File,
    #[cfg(windows)]
    native: crate::state_home::NativeJournalLock,
}

impl FileLock {
    pub(crate) fn identity(&self) -> Result<Vec<u8>, OpenError> {
        #[cfg(windows)]
        {
            self.native
                .identity(
                    self.path
                        .file_name()
                        .ok_or_else(|| OpenError::Authority("lock filename missing".into()))?,
                )
                .map_err(Into::into)
        }
        #[cfg(not(windows))]
        {
            let held = self.file.metadata()?;
            let linked = std::fs::symlink_metadata(&self.path)?;
            if !held.is_file()
                || held.nlink() != 1
                || linked.file_type().is_symlink()
                || held.dev() != linked.dev()
                || held.ino() != linked.ino()
            {
                return Err(OpenError::Authority(
                    "original journal lock identity changed".into(),
                ));
            }
            Ok([held.dev().to_le_bytes(), held.ino().to_le_bytes()].concat())
        }
    }
    /// Obtain the normal run writer lock, creating its file when necessary.
    #[cfg(test)]
    pub fn try_acquire(lock_path: &Path, run_id: surge_core::RunId) -> Result<Self, OpenError> {
        Self::acquire(lock_path, run_id, true)
    }

    /// Informational recovery may only use an already-existing original journal lock.
    pub(crate) fn try_acquire_existing(
        lock_path: &Path,
        run_id: surge_core::RunId,
    ) -> Result<Self, OpenError> {
        Self::acquire(lock_path, run_id, false)
    }

    fn acquire(
        lock_path: &Path,
        run_id: surge_core::RunId,
        create: bool,
    ) -> Result<Self, OpenError> {
        #[cfg(windows)]
        {
            use crate::state_home::NativeError;
            use windows::Win32::Foundation::{
                STATUS_OBJECT_NAME_NOT_FOUND, STATUS_OBJECT_PATH_NOT_FOUND,
            };
            let native = crate::state_home::NativeJournalLock::acquire(lock_path, create).map_err(
                |error| match error {
                    NativeError::Busy => OpenError::WriterAlreadyHeld { run_id },
                    NativeError::Open(
                        STATUS_OBJECT_NAME_NOT_FOUND | STATUS_OBJECT_PATH_NOT_FOUND,
                    ) => OpenError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "journal lock is missing",
                    )),
                    other => other.into(),
                },
            )?;
            Ok(Self {
                native,
                path: lock_path.to_owned(),
            })
        }
        #[cfg(not(windows))]
        {
            if create && let Some(parent) = lock_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(create)
                .read(true)
                .write(true)
                .truncate(false)
                .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK)
                .open(lock_path)?;
            file.try_lock().map_err(|error| match error {
                std::fs::TryLockError::WouldBlock => OpenError::WriterAlreadyHeld { run_id },
                std::fs::TryLockError::Error(error) => OpenError::Io(error),
            })?;
            let owner = Self {
                file,
                path: lock_path.to_owned(),
            };
            owner.identity()?;
            Ok(owner)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::RunId;
    use tempfile::TempDir;

    #[test]
    fn second_acquire_in_same_process_fails() {
        let tmp = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let lock_path = tmp.path().join("test.lock");

            let _l1 = FileLock::try_acquire(&lock_path, RunId::new()).unwrap();
            let l2 = FileLock::try_acquire(&lock_path, RunId::new());
            assert!(l2.is_err(), "second lock acquire should fail");
        }
        tmp.close().unwrap();
    }

    #[test]
    fn release_after_drop_allows_reacquire() {
        let tmp = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let lock_path = tmp.path().join("test.lock");

            let l1 = FileLock::try_acquire(&lock_path, RunId::new()).unwrap();
            drop(l1);
            let l2 = FileLock::try_acquire(&lock_path, RunId::new());
            assert!(l2.is_ok(), "lock should be reacquirable after drop");
        }
        tmp.close().unwrap();
    }

    #[test]
    fn existing_only_refuses_missing_without_creating_anything() {
        let tmp = TempDir::new().unwrap();
        let lock_path = tmp.path().join("missing").join("events.sqlite.lock");
        assert!(matches!(
            FileLock::try_acquire_existing(&lock_path, RunId::new()),
            Err(OpenError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound
        ));
        assert!(!tmp.path().join("missing").exists());
    }

    #[test]
    fn lock_probe() {
        let Some(path) = std::env::var_os("SURGE_WRITER_LOCK_PROBE") else {
            return;
        };
        let result = FileLock::try_acquire_existing(Path::new(&path), RunId::new());
        if std::env::var_os("SURGE_WRITER_LOCK_EXPECT_BUSY").is_some() {
            assert!(matches!(result, Err(OpenError::WriterAlreadyHeld { .. })));
        } else {
            let _held = result.unwrap();
            std::fs::write(
                std::env::var_os("SURGE_WRITER_LOCK_READY").unwrap(),
                b"held",
            )
            .unwrap();
            let mut byte = [0];
            std::io::Read::read_exact(&mut std::io::stdin(), &mut byte).unwrap();
        }
    }

    #[test]
    fn actual_second_process_exclusion_and_death_release() {
        let tmp = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let path = tmp.path().join("events.sqlite.lock");
            let ready = tmp.path().join("ready");
            let initial = FileLock::try_acquire(&path, RunId::new()).unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runs::file_lock::tests::lock_probe",
                    "--nocapture",
                ])
                .env("SURGE_WRITER_LOCK_PROBE", &path)
                .env("SURGE_WRITER_LOCK_EXPECT_BUSY", "1")
                .status()
                .unwrap();
            assert!(status.success());
            drop(initial);
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runs::file_lock::tests::lock_probe",
                    "--nocapture",
                ])
                .env("SURGE_WRITER_LOCK_PROBE", &path)
                .env("SURGE_WRITER_LOCK_READY", &ready)
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !ready.exists() && std::time::Instant::now() < deadline {
                assert!(child.try_wait().unwrap().is_none());
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let observed_ready = ready.exists();
            let excluded = matches!(
                FileLock::try_acquire_existing(&path, RunId::new()),
                Err(OpenError::WriterAlreadyHeld { .. })
            );
            child.kill().unwrap();
            child.wait().unwrap();
            assert!(observed_ready);
            assert!(excluded);
            let _reacquired = FileLock::try_acquire_existing(&path, RunId::new()).unwrap();
        }
        tmp.close().unwrap();
    }
}
