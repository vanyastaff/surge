//! Legacy scheduling model: flat items grouped into parallel batches.

use super::*;

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
