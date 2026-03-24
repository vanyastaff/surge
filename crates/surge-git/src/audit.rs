//! Audit logging for cleanup operations.
//!
//! Provides [`CleanupAudit`] which logs all cleanup operations to a file
//! for debugging and verification of the zero-garbage guarantee.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::worktree::GitError;

/// Type of cleanup event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupEventType {
    /// Worktree was removed.
    WorktreeRemoved,
    /// Branch was deleted.
    BranchDeleted,
    /// Orphaned worktree was detected.
    OrphanDetected,
    /// Merged branch was detected.
    MergedBranchDetected,
}

/// A single cleanup event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupEvent {
    /// Timestamp when the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Type of cleanup event.
    pub event_type: CleanupEventType,
    /// Identifier of the resource (spec_id or branch name).
    pub resource_id: String,
    /// Optional additional context or reason.
    pub context: Option<String>,
}

impl CleanupEvent {
    /// Create a new cleanup event.
    pub fn new(event_type: CleanupEventType, resource_id: String, context: Option<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            event_type,
            resource_id,
            context,
        }
    }

    /// Format event as a log line (JSON).
    fn to_log_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                r#"{{"timestamp":"{}","event_type":"unknown","resource_id":"{}"}}"#,
                self.timestamp.to_rfc3339(),
                self.resource_id
            )
        })
    }
}

/// File-based audit logger for cleanup operations.
pub struct CleanupAudit {
    log_path: PathBuf,
}

impl CleanupAudit {
    /// Create a new `CleanupAudit` that logs to the specified path.
    ///
    /// Creates parent directories if they don't exist.
    pub fn new<P: AsRef<Path>>(log_path: P) -> Result<Self, GitError> {
        let log_path = log_path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }

        Ok(Self { log_path })
    }

    /// Log a cleanup event to the audit file.
    ///
    /// Appends a JSON line to the log file. Creates the file if it doesn't exist.
    pub fn log(&self, event: CleanupEvent) -> Result<(), GitError> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;

        writeln!(file, "{}", event.to_log_line())?;

        Ok(())
    }

    /// Log a worktree removal event.
    pub fn log_worktree_removed(
        &self,
        spec_id: &str,
        context: Option<String>,
    ) -> Result<(), GitError> {
        let event = CleanupEvent::new(
            CleanupEventType::WorktreeRemoved,
            spec_id.to_string(),
            context,
        );
        self.log(event)
    }

    /// Log a branch deletion event.
    pub fn log_branch_deleted(
        &self,
        branch_name: &str,
        context: Option<String>,
    ) -> Result<(), GitError> {
        let event = CleanupEvent::new(
            CleanupEventType::BranchDeleted,
            branch_name.to_string(),
            context,
        );
        self.log(event)
    }

    /// Log an orphaned worktree detection event.
    pub fn log_orphan_detected(
        &self,
        spec_id: &str,
        context: Option<String>,
    ) -> Result<(), GitError> {
        let event = CleanupEvent::new(
            CleanupEventType::OrphanDetected,
            spec_id.to_string(),
            context,
        );
        self.log(event)
    }

    /// Log a merged branch detection event.
    pub fn log_merged_branch_detected(
        &self,
        branch_name: &str,
        context: Option<String>,
    ) -> Result<(), GitError> {
        let event = CleanupEvent::new(
            CleanupEventType::MergedBranchDetected,
            branch_name.to_string(),
            context,
        );
        self.log(event)
    }

    /// Read all events from the audit log.
    ///
    /// Returns an empty vector if the log file doesn't exist.
    pub fn read_events(&self) -> Result<Vec<CleanupEvent>, GitError> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let contents = fs::read_to_string(&self.log_path)?;

        let events: Vec<CleanupEvent> = contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_audit() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("cleanup.log");

        let audit = CleanupAudit::new(&log_path).unwrap();
        assert_eq!(audit.log_path, log_path);
    }

    #[test]
    fn test_log_worktree_removed() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("cleanup.log");
        let audit = CleanupAudit::new(&log_path).unwrap();

        audit
            .log_worktree_removed("test-spec", Some("cleanup test".to_string()))
            .unwrap();

        let events = audit.read_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, CleanupEventType::WorktreeRemoved);
        assert_eq!(events[0].resource_id, "test-spec");
        assert_eq!(events[0].context, Some("cleanup test".to_string()));
    }

    #[test]
    fn test_log_branch_deleted() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("cleanup.log");
        let audit = CleanupAudit::new(&log_path).unwrap();

        audit.log_branch_deleted("surge/test-branch", None).unwrap();

        let events = audit.read_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, CleanupEventType::BranchDeleted);
        assert_eq!(events[0].resource_id, "surge/test-branch");
        assert_eq!(events[0].context, None);
    }

    #[test]
    fn test_log_multiple_events() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("cleanup.log");
        let audit = CleanupAudit::new(&log_path).unwrap();

        audit.log_orphan_detected("orphan-1", None).unwrap();
        audit
            .log_worktree_removed("orphan-1", Some("cleaned up".to_string()))
            .unwrap();
        audit
            .log_merged_branch_detected("surge/old-branch", None)
            .unwrap();
        audit.log_branch_deleted("surge/old-branch", None).unwrap();

        let events = audit.read_events().unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event_type, CleanupEventType::OrphanDetected);
        assert_eq!(events[1].event_type, CleanupEventType::WorktreeRemoved);
        assert_eq!(events[2].event_type, CleanupEventType::MergedBranchDetected);
        assert_eq!(events[3].event_type, CleanupEventType::BranchDeleted);
    }

    #[test]
    fn test_read_empty_log() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("nonexistent.log");
        let audit = CleanupAudit::new(&log_path).unwrap();

        let events = audit.read_events().unwrap();
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn test_create_nested_directory() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join(".surge").join("logs").join("cleanup.log");

        let audit = CleanupAudit::new(&log_path).unwrap();
        audit.log_worktree_removed("test", None).unwrap();

        assert!(log_path.exists());
        let events = audit.read_events().unwrap();
        assert_eq!(events.len(), 1);
    }
}
