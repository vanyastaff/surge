//! Cross-process advisory lock around per-run events.sqlite.lock.

use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::runs::error::OpenError;

/// Advisory file lock held by the live writer for cross-process exclusion.
///
/// On Unix uses `flock(LOCK_EX | LOCK_NB)` semantics (advisory, cooperative);
/// on Windows uses `LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY`.
/// Either way, only cooperating processes that take the same lock will be
/// blocked — non-cooperating tools (Windows Explorer reading the file) are not.
pub struct FileLock {
    // Box+raw-pointer dance to obtain a 'static guard while the underlying
    // RwLock is owned by the same struct.
    _lock: Box<fd_lock::RwLock<File>>,
    _guard: fd_lock::RwLockWriteGuard<'static, File>,
}

impl FileLock {
    /// Try to acquire the exclusive lock for `lock_path`.
    ///
    /// Returns `OpenError::WriterAlreadyHeld` (with `run_id` propagated by the
    /// caller) if the lock is already held by another process.
    pub fn try_acquire(lock_path: &Path, run_id: surge_core::RunId) -> Result<Self, OpenError> {
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;

        let mut lock = Box::new(fd_lock::RwLock::new(file));

        // SAFETY: the Box keeps the RwLock alive in the same struct; the guard
        // borrows from it. Extending the borrow lifetime to 'static is sound
        // as long as the Box is dropped AFTER the guard. Since they're stored
        // as fields in the same struct and Drop runs in field-declaration
        // order, _guard drops before _lock. We use a raw pointer to bypass
        // the borrow checker for that one extension.
        let lock_ref: &'static mut fd_lock::RwLock<File> = unsafe {
            &mut *std::ptr::from_mut(Box::as_mut(&mut lock))
        };
        let guard = lock_ref
            .try_write()
            .map_err(|_| OpenError::WriterAlreadyHeld { run_id })?;

        Ok(Self {
            _lock: lock,
            _guard: guard,
        })
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
