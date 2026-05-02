//! Process tracking and PID file management.
//!
//! Provides [`ProcessTracker`] which writes PID files for spawned agent processes
//! and ensures they are cleaned up on shutdown. Critical for preventing zombie
//! processes, especially on Windows with CREATE_NO_WINDOW spawned processes.
//!
//! PID files are written to `.surge/pids/<agent-name>.pid` and removed when the
//! agent shuts down or the tracker is dropped.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use surge_core::SurgeError;
use tracing::{debug, warn};

/// Process ID type.
pub type Pid = u32;

/// Tracks spawned agent processes via PID files.
///
/// Writes PID files to `.surge/pids/` when processes are spawned and removes
/// them on shutdown. Used by [`crate::pool::AgentPool`] to ensure zero zombie
/// processes across graceful and ungraceful shutdowns.
pub struct ProcessTracker {
    /// Directory where PID files are stored.
    pid_dir: PathBuf,
}

impl ProcessTracker {
    /// Create a new `ProcessTracker` that stores PID files in the specified directory.
    ///
    /// Creates the directory if it doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns error if directory creation fails.
    pub fn new<P: AsRef<Path>>(pid_dir: P) -> Result<Self, SurgeError> {
        let pid_dir = pid_dir.as_ref().to_path_buf();

        // Ensure PID directory exists
        fs::create_dir_all(&pid_dir)?;

        debug!("ProcessTracker initialized at {}", pid_dir.display());

        Ok(Self { pid_dir })
    }

    /// Record a spawned process by writing its PID to a file.
    ///
    /// Creates `<pid_dir>/<agent_name>.pid` containing the process ID.
    ///
    /// # Errors
    ///
    /// Returns error if file write fails.
    pub fn track(&self, agent_name: &str, pid: Pid) -> Result<(), SurgeError> {
        let pid_file = self.pid_file_path(agent_name);

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&pid_file)?;

        writeln!(file, "{}", pid)?;

        debug!(
            agent = agent_name,
            pid = pid,
            "Tracked process in {}",
            pid_file.display()
        );

        Ok(())
    }

    /// Stop tracking a process and remove its PID file.
    ///
    /// Returns `Ok(())` even if the PID file doesn't exist — untracking is idempotent.
    pub fn untrack(&self, agent_name: &str) -> Result<(), SurgeError> {
        let pid_file = self.pid_file_path(agent_name);

        match fs::remove_file(&pid_file) {
            Ok(()) => {
                debug!(
                    agent = agent_name,
                    "Removed PID file {}",
                    pid_file.display()
                );
                Ok(())
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Already removed or never existed — not an error
                Ok(())
            },
            Err(e) => {
                warn!(
                    agent = agent_name,
                    "Failed to remove PID file {}: {}",
                    pid_file.display(),
                    e
                );
                // Don't propagate error — best effort cleanup
                Ok(())
            },
        }
    }

    /// Read the PID from a tracked process.
    ///
    /// Returns `None` if the PID file doesn't exist or contains invalid data.
    pub fn read_pid(&self, agent_name: &str) -> Option<Pid> {
        let pid_file = self.pid_file_path(agent_name);

        let content = fs::read_to_string(&pid_file).ok()?;
        content.trim().parse::<Pid>().ok()
    }

    /// Check if a process is currently being tracked.
    #[must_use]
    pub fn is_tracked(&self, agent_name: &str) -> bool {
        self.pid_file_path(agent_name).exists()
    }

    /// List all tracked processes.
    ///
    /// Returns a vector of `(agent_name, pid)` tuples for all PID files found.
    pub fn list_tracked(&self) -> Vec<(String, Pid)> {
        let mut tracked = Vec::new();

        let entries = match fs::read_dir(&self.pid_dir) {
            Ok(entries) => entries,
            Err(e) => {
                warn!("Failed to read PID directory: {}", e);
                return tracked;
            },
        };

        for entry in entries.flatten() {
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("pid") {
                continue;
            }

            let Some(agent_name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            if let Some(pid) = self.read_pid(agent_name) {
                tracked.push((agent_name.to_string(), pid));
            }
        }

        tracked
    }

    /// Verify that a tracked process is actually running.
    ///
    /// Platform-specific: uses Windows-compatible process checking.
    #[must_use]
    pub fn is_running(&self, agent_name: &str) -> bool {
        let Some(pid) = self.read_pid(agent_name) else {
            return false;
        };

        self.is_pid_alive(pid)
    }

    /// Check if a process ID is currently running.
    ///
    /// Platform-specific implementation.
    #[must_use]
    fn is_pid_alive(&self, pid: Pid) -> bool {
        #[cfg(unix)]
        {
            // On Unix, send signal 0 to check if process exists
            // Returns 0 if process exists, -1 if not
            unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
        }

        #[cfg(windows)]
        {
            // On Windows, try to open the process handle
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            };

            unsafe {
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);

                if let Ok(handle) = handle
                    && !handle.is_invalid()
                {
                    let _ = CloseHandle(handle);
                    return true;
                }

                false
            }
        }

        #[cfg(not(any(unix, windows)))]
        {
            // Fallback: assume process is running if PID file exists
            warn!(
                pid = pid,
                "Process existence check not implemented for this platform"
            );
            true
        }
    }

    /// Remove all PID files (cleanup on shutdown).
    ///
    /// Best-effort operation — logs warnings for any failures but doesn't propagate errors.
    pub fn cleanup_all(&self) {
        let tracked = self.list_tracked();

        debug!("Cleaning up {} tracked processes", tracked.len());

        for (agent_name, _pid) in tracked {
            let _ = self.untrack(&agent_name);
        }
    }

    /// Get the path to the PID file for a given agent.
    fn pid_file_path(&self, agent_name: &str) -> PathBuf {
        self.pid_dir.join(format!("{}.pid", agent_name))
    }
}

impl Drop for ProcessTracker {
    fn drop(&mut self) {
        debug!("ProcessTracker dropped, cleaning up PID files");
        self.cleanup_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ProcessTracker) {
        let tmp = TempDir::new().unwrap();
        let pid_dir = tmp.path().join("pids");
        let tracker = ProcessTracker::new(&pid_dir).unwrap();
        (tmp, tracker)
    }

    #[test]
    fn test_new_creates_directory() {
        let tmp = TempDir::new().unwrap();
        let pid_dir = tmp.path().join("pids");

        assert!(!pid_dir.exists());

        let _tracker = ProcessTracker::new(&pid_dir).unwrap();

        assert!(pid_dir.exists());
        assert!(pid_dir.is_dir());
    }

    #[test]
    fn test_track_writes_pid_file() {
        let (_tmp, tracker) = setup();

        tracker.track("test-agent", 12345).unwrap();

        let pid_file = tracker.pid_file_path("test-agent");
        assert!(pid_file.exists());

        let content = fs::read_to_string(&pid_file).unwrap();
        assert_eq!(content.trim(), "12345");
    }

    #[test]
    fn test_track_overwrites_existing_pid() {
        let (_tmp, tracker) = setup();

        tracker.track("test-agent", 12345).unwrap();
        tracker.track("test-agent", 67890).unwrap();

        let pid = tracker.read_pid("test-agent").unwrap();
        assert_eq!(pid, 67890);
    }

    #[test]
    fn test_untrack_removes_pid_file() {
        let (_tmp, tracker) = setup();

        tracker.track("test-agent", 12345).unwrap();
        assert!(tracker.is_tracked("test-agent"));

        tracker.untrack("test-agent").unwrap();
        assert!(!tracker.is_tracked("test-agent"));
    }

    #[test]
    fn test_untrack_idempotent() {
        let (_tmp, tracker) = setup();

        // Untracking a non-existent process should succeed
        tracker.untrack("nonexistent").unwrap();
        tracker.untrack("nonexistent").unwrap();
    }

    #[test]
    fn test_read_pid() {
        let (_tmp, tracker) = setup();

        tracker.track("test-agent", 99999).unwrap();

        let pid = tracker.read_pid("test-agent").unwrap();
        assert_eq!(pid, 99999);
    }

    #[test]
    fn test_read_pid_nonexistent() {
        let (_tmp, tracker) = setup();

        let pid = tracker.read_pid("nonexistent");
        assert!(pid.is_none());
    }

    #[test]
    fn test_is_tracked() {
        let (_tmp, tracker) = setup();

        assert!(!tracker.is_tracked("test-agent"));

        tracker.track("test-agent", 12345).unwrap();
        assert!(tracker.is_tracked("test-agent"));

        tracker.untrack("test-agent").unwrap();
        assert!(!tracker.is_tracked("test-agent"));
    }

    #[test]
    fn test_list_tracked() {
        let (_tmp, tracker) = setup();

        tracker.track("agent-1", 111).unwrap();
        tracker.track("agent-2", 222).unwrap();
        tracker.track("agent-3", 333).unwrap();

        let tracked = tracker.list_tracked();
        assert_eq!(tracked.len(), 3);

        // Check all agents are present (order may vary)
        let names: Vec<String> = tracked.iter().map(|(name, _)| name.clone()).collect();
        assert!(names.contains(&"agent-1".to_string()));
        assert!(names.contains(&"agent-2".to_string()));
        assert!(names.contains(&"agent-3".to_string()));
    }

    #[test]
    fn test_cleanup_all() {
        let (_tmp, tracker) = setup();

        tracker.track("agent-1", 111).unwrap();
        tracker.track("agent-2", 222).unwrap();

        assert_eq!(tracker.list_tracked().len(), 2);

        tracker.cleanup_all();

        assert_eq!(tracker.list_tracked().len(), 0);
    }

    #[test]
    fn test_drop_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let pid_dir = tmp.path().join("pids");

        {
            let tracker = ProcessTracker::new(&pid_dir).unwrap();
            tracker.track("agent-1", 111).unwrap();
            tracker.track("agent-2", 222).unwrap();

            assert_eq!(tracker.list_tracked().len(), 2);
        } // tracker dropped here

        // Create new tracker to check if files were cleaned up
        let tracker = ProcessTracker::new(&pid_dir).unwrap();
        assert_eq!(tracker.list_tracked().len(), 0);
    }

    #[test]
    fn test_is_running_self() {
        let (_tmp, tracker) = setup();

        // Current process should always be running
        let self_pid = std::process::id();
        assert!(tracker.is_pid_alive(self_pid));
    }

    #[test]
    fn test_is_running_invalid_pid() {
        let (_tmp, tracker) = setup();

        // PID 0 and very high PIDs are unlikely to exist
        assert!(!tracker.is_pid_alive(0));
        assert!(!tracker.is_pid_alive(u32::MAX));
    }

    #[test]
    fn test_is_running_tracked() {
        let (_tmp, tracker) = setup();

        let self_pid = std::process::id();
        tracker.track("self", self_pid).unwrap();

        assert!(tracker.is_running("self"));
    }

    #[test]
    fn test_is_running_untracked() {
        let (_tmp, tracker) = setup();

        assert!(!tracker.is_running("nonexistent"));
    }
}
