//! Roadmap types — project-level planning across multiple specs.

use serde::{Deserialize, Serialize};

use crate::id::SpecId;
use crate::spec::Complexity;

/// A single item in a project roadmap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoadmapItem {
    /// Spec this item refers to.
    pub spec_id: SpecId,
    /// Human-readable title.
    pub title: String,
    /// Estimated complexity.
    pub complexity: Complexity,
    /// Priority for scheduling (higher = more important).
    #[serde(default)]
    pub priority: Priority,
    /// Specs that must complete before this one can start.
    #[serde(default)]
    pub depends_on: Vec<SpecId>,
    /// Current execution status.
    #[serde(default)]
    pub status: RoadmapStatus,
}

/// Priority level for roadmap scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Critical,
    High,
    #[default]
    Medium,
    Low,
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}

/// Execution status of a roadmap item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoadmapStatus {
    /// Not yet started.
    #[default]
    Pending,
    /// Currently being executed by the orchestrator.
    Running,
    /// Paused, waiting for human input or gate.
    Paused,
    /// Completed successfully.
    Completed,
    /// Failed — may be retried.
    Failed,
    /// Skipped (dependency failed or user cancelled).
    Skipped,
}

impl RoadmapStatus {
    /// Returns `true` if no further execution will happen.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Skipped)
    }
}

/// A project timeline — ordered batches of roadmap items.
///
/// Items within a batch are independent and can run in parallel.
/// Batches are executed sequentially.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Timeline {
    /// Ordered batches of roadmap items.
    pub batches: Vec<TimelineBatch>,
}

/// A single batch in a timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineBatch {
    /// Batch execution order (0-based).
    pub order: usize,
    /// Items in this batch (can run in parallel).
    pub items: Vec<RoadmapItem>,
    /// Why this batch is ordered this way.
    #[serde(default)]
    pub reason: String,
}

impl Timeline {
    /// Create a new empty timeline.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of items across all batches.
    #[must_use]
    pub fn total_items(&self) -> usize {
        self.batches.iter().map(|b| b.items.len()).sum()
    }

    /// Count items by status.
    #[must_use]
    pub fn count_by_status(&self, status: RoadmapStatus) -> usize {
        self.batches
            .iter()
            .flat_map(|b| &b.items)
            .filter(|item| item.status == status)
            .count()
    }

    /// Find a mutable reference to a roadmap item by spec ID.
    pub fn find_item_mut(&mut self, spec_id: SpecId) -> Option<&mut RoadmapItem> {
        self.batches
            .iter_mut()
            .flat_map(|b| &mut b.items)
            .find(|item| item.spec_id == spec_id)
    }

    /// Returns the next batch of pending items ready for execution.
    ///
    /// A batch is ready when all previous batches have completed.
    #[must_use]
    pub fn next_ready_batch(&self) -> Option<&TimelineBatch> {
        for batch in &self.batches {
            let all_terminal = batch.items.iter().all(|i| i.status.is_terminal());
            if all_terminal {
                continue;
            }
            let has_pending = batch
                .items
                .iter()
                .any(|i| i.status == RoadmapStatus::Pending);
            if has_pending {
                return Some(batch);
            }
            // Batch has running/paused items — wait for them.
            return None;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::SpecId;

    fn make_item(title: &str, status: RoadmapStatus) -> RoadmapItem {
        RoadmapItem {
            spec_id: SpecId::new(),
            title: title.to_string(),
            complexity: Complexity::Standard,
            priority: Priority::Medium,
            depends_on: vec![],
            status,
        }
    }

    #[test]
    fn test_timeline_total_items() {
        let timeline = Timeline {
            batches: vec![
                TimelineBatch {
                    order: 0,
                    items: vec![
                        make_item("A", RoadmapStatus::Pending),
                        make_item("B", RoadmapStatus::Pending),
                    ],
                    reason: String::new(),
                },
                TimelineBatch {
                    order: 1,
                    items: vec![make_item("C", RoadmapStatus::Pending)],
                    reason: String::new(),
                },
            ],
        };
        assert_eq!(timeline.total_items(), 3);
    }

    #[test]
    fn test_timeline_count_by_status() {
        let timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![
                    make_item("A", RoadmapStatus::Completed),
                    make_item("B", RoadmapStatus::Pending),
                    make_item("C", RoadmapStatus::Completed),
                ],
                reason: String::new(),
            }],
        };
        assert_eq!(timeline.count_by_status(RoadmapStatus::Completed), 2);
        assert_eq!(timeline.count_by_status(RoadmapStatus::Pending), 1);
    }

    #[test]
    fn test_timeline_next_ready_batch() {
        let timeline = Timeline {
            batches: vec![
                TimelineBatch {
                    order: 0,
                    items: vec![make_item("A", RoadmapStatus::Completed)],
                    reason: String::new(),
                },
                TimelineBatch {
                    order: 1,
                    items: vec![make_item("B", RoadmapStatus::Pending)],
                    reason: String::new(),
                },
            ],
        };
        let batch = timeline.next_ready_batch().unwrap();
        assert_eq!(batch.order, 1);
    }

    #[test]
    fn test_timeline_next_ready_batch_none_when_running() {
        let timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![make_item("A", RoadmapStatus::Running)],
                reason: String::new(),
            }],
        };
        assert!(timeline.next_ready_batch().is_none());
    }

    #[test]
    fn test_timeline_next_ready_batch_none_when_all_done() {
        let timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![make_item("A", RoadmapStatus::Completed)],
                reason: String::new(),
            }],
        };
        assert!(timeline.next_ready_batch().is_none());
    }

    #[test]
    fn test_roadmap_status_is_terminal() {
        assert!(RoadmapStatus::Completed.is_terminal());
        assert!(RoadmapStatus::Failed.is_terminal());
        assert!(RoadmapStatus::Skipped.is_terminal());
        assert!(!RoadmapStatus::Pending.is_terminal());
        assert!(!RoadmapStatus::Running.is_terminal());
        assert!(!RoadmapStatus::Paused.is_terminal());
    }

    #[test]
    fn test_priority_display() {
        assert_eq!(Priority::Critical.to_string(), "critical");
        assert_eq!(Priority::High.to_string(), "high");
        assert_eq!(Priority::Medium.to_string(), "medium");
        assert_eq!(Priority::Low.to_string(), "low");
    }

    #[test]
    fn test_roadmap_item_toml_roundtrip() {
        let item = make_item("Test item", RoadmapStatus::Pending);
        let toml_str = toml::to_string(&item).unwrap();
        let deserialized: RoadmapItem = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.title, "Test item");
        assert_eq!(deserialized.status, RoadmapStatus::Pending);
        assert_eq!(deserialized.priority, Priority::Medium);
    }

    #[test]
    fn test_find_item_mut() {
        let spec_id = SpecId::new();
        let mut timeline = Timeline {
            batches: vec![TimelineBatch {
                order: 0,
                items: vec![RoadmapItem {
                    spec_id,
                    title: "Find me".to_string(),
                    complexity: Complexity::Simple,
                    priority: Priority::High,
                    depends_on: vec![],
                    status: RoadmapStatus::Pending,
                }],
                reason: String::new(),
            }],
        };

        let item = timeline.find_item_mut(spec_id).unwrap();
        item.status = RoadmapStatus::Running;
        assert_eq!(timeline.batches[0].items[0].status, RoadmapStatus::Running);
    }
}
