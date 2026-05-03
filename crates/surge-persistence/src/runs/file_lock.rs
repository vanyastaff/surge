//! Cross-process advisory lock around per-run events.sqlite.lock.

use std::fs::{File, OpenOptions};
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
/// Implementation note: the underlying `fd_lock::RwLock` is intentionally
/// `Box::leak`-ed onto the heap so the guard's `'static` lifetime is real
/// rather than synthesized via unsafe. The leak is ~24 bytes per `FileLock`
/// instance (one `RwLock<File>` plus heap allocation overhead). Since
/// `RunWriter` instances are infrequent and bounded by run count for the
/// process lifetime, the leak is acceptable. The OS releases the underlying
/// file descriptor when the process exits.
pub struct FileLock {
    _guard: fd_lock::RwLockWriteGuard<'static, File>,
}

impl FileLock {
    /// Try to acquire the exclusive lock for `lock_path`.
    ///
    /// Returns `OpenError::WriterAlreadyHeld { run_id }` if the lock is
    /// already held by another process (or another in-process holder, on
    /// platforms where `fd-lock` is per-handle).
    pub fn try_acquire(lock_path: &Path, run_id: surge_core::RunId) -> Result<Self, OpenError> {
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)?;

        // Box + leak: the guard borrows from the leaked RwLock with a real
        // 'static lifetime. No unsafe required. The 24-byte leak per acquire
        // is acceptable (RunWriter creations are rare, leaks end at process
        // exit).
        let lock_box: &'static mut fd_lock::RwLock<File> =
            Box::leak(Box::new(fd_lock::RwLock::new(file)));
        let guard = lock_box
            .try_write()
            .map_err(|_| OpenError::WriterAlreadyHeld { run_id })?;

        Ok(Self { _guard: guard })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::RunId;
    use tempfile::TempDir;

    #[test]
    fn second_acquire_in_same_process_fails() {
        let tmp = TempDir::new().unwrap();
        let lock_path = tmp.path().join("test.lock");

        let _l1 = FileLock::try_acquire(&lock_path, RunId::new()).unwrap();
        let l2 = FileLock::try_acquire(&lock_path, RunId::new());
        assert!(l2.is_err(), "second lock acquire should fail");
    }

    #[test]
    fn release_after_drop_allows_reacquire() {
        let tmp = TempDir::new().unwrap();
        let lock_path = tmp.path().join("test.lock");

        let l1 = FileLock::try_acquire(&lock_path, RunId::new()).unwrap();
        drop(l1);
        let l2 = FileLock::try_acquire(&lock_path, RunId::new());
        assert!(l2.is_ok(), "lock should be reacquirable after drop");
    }
}
