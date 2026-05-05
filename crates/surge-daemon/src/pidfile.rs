//! PID + socket file discovery and stale-lock handling.
//!
//! Layout:
//! ```text
//! ~/.surge/daemon/
//! ├── daemon.pid          (text: PID of the running daemon)
//! ├── daemon.sock         (Unix socket; on Windows: holds the named pipe path)
//! └── version             (text: daemon binary version)
//! ```

use std::path::{Path, PathBuf};

/// Errors produced by PID-file operations.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum PidfileError {
    /// `dirs::home_dir()` returned `None`.
    #[error("home directory not found")]
    NoHome,
    /// Underlying I/O error reading or writing the daemon directory.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// PID file contents could not be parsed as `u32`.
    #[error("pid file is malformed: {0}")]
    Malformed(String),
    /// A live daemon process is already holding the lock.
    #[error("daemon already running (pid {0})")]
    AlreadyRunning(u32),
}

/// Returns the daemon directory path: `~/.surge/daemon/`.
pub fn daemon_dir() -> Result<PathBuf, PidfileError> {
    let home = dirs::home_dir().ok_or(PidfileError::NoHome)?;
    Ok(home.join(".surge").join("daemon"))
}

/// Returns the PID file path.
pub fn pid_path() -> Result<PathBuf, PidfileError> {
    Ok(daemon_dir()?.join("daemon.pid"))
}

/// Returns the socket marker path. On Unix this is the actual socket
/// path; on Windows the file holds the named-pipe path string.
pub fn socket_path() -> Result<PathBuf, PidfileError> {
    Ok(daemon_dir()?.join("daemon.sock"))
}

/// Returns the version-marker path.
pub fn version_path() -> Result<PathBuf, PidfileError> {
    Ok(daemon_dir()?.join("version"))
}

/// Read a stored PID. Returns `Ok(None)` if the file doesn't exist.
pub fn read_pid(path: &Path) -> Result<Option<u32>, PidfileError> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let trimmed = s.trim();
            trimmed
                .parse::<u32>()
                .map(Some)
                .map_err(|_| PidfileError::Malformed(trimmed.to_string()))
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PidfileError::Io(e)),
    }
}

/// Check whether a process with the given PID is currently alive.
/// Cross-platform via `sysinfo`.
#[must_use]
pub fn is_alive(pid: u32) -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}

/// Acquire the daemon lock by writing our PID. If a stale PID file
/// exists (process not alive), it is overwritten; if the PID is alive,
/// returns [`PidfileError::AlreadyRunning`].
///
/// Uses `OpenOptions::create_new` for the first attempt so that two
/// concurrent cold-starts cannot both write the file (atomic on all
/// major OSes including Windows). The stale-recovery fallback still
/// has a small race window, but that case (two processes racing after
/// a previous unclean exit) is rare and M7 documents single-user
/// operation as the constraint.
pub fn acquire_lock(pid: u32) -> Result<(), PidfileError> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let dir = daemon_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = pid_path()?;

    // Try atomic create_new first.
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut f) => {
            f.write_all(pid.to_string().as_bytes())?;
            Ok(())
        },
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // File exists — check if it's stale.
            match read_pid(&path)? {
                Some(existing) if is_alive(existing) => Err(PidfileError::AlreadyRunning(existing)),
                _ => {
                    // Stale — overwrite explicitly. Race window narrowed
                    // (only racing two stale-recovery attempts, which is
                    // far less common than two cold starts).
                    std::fs::write(&path, pid.to_string())?;
                    Ok(())
                },
            }
        },
        Err(e) => Err(PidfileError::Io(e)),
    }
}

/// Release the lock by removing the PID file. Best-effort.
pub fn release_lock() -> Result<(), PidfileError> {
    let path = pid_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PidfileError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_pid_handles_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nonexistent.pid");
        assert_eq!(read_pid(&path).unwrap(), None);
    }

    #[test]
    fn read_pid_parses_valid_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("d.pid");
        std::fs::write(&path, "12345\n").unwrap();
        assert_eq!(read_pid(&path).unwrap(), Some(12345));
    }

    #[test]
    fn read_pid_rejects_garbage() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("d.pid");
        std::fs::write(&path, "not-a-pid").unwrap();
        let err = read_pid(&path).unwrap_err();
        assert!(matches!(err, PidfileError::Malformed(_)));
    }

    #[test]
    fn is_alive_for_current_process_returns_true() {
        let me = std::process::id();
        assert!(is_alive(me));
    }
}
