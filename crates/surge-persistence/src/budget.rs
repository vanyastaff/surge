//! Budget tracking and alerting for token usage costs.
//!
//! Provides budget monitoring capabilities by querying aggregated token
//! usage from the store and comparing against configured budget thresholds.

use crate::store::Store;
use crate::Result;
use std::time::SystemTime;

/// Budget status for a specific time period.
///
/// Represents the current spending status relative to a configured budget,
/// including warning levels based on threshold percentages.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetStatus {
    /// Budget limit in USD for this period.
    pub budget_limit_usd: f64,

    /// Actual spending in USD for this period.
    pub actual_spending_usd: f64,

    /// Percentage of budget used (0-100+).
    pub usage_percentage: f64,

    /// Budget warning level based on threshold.
    pub warning_level: BudgetWarningLevel,

    /// Start of the time period (Unix timestamp in milliseconds).
    pub period_start_ms: u64,

    /// End of the time period (Unix timestamp in milliseconds).
    pub period_end_ms: u64,
}

impl BudgetStatus {
    /// Check if the budget has been exceeded.
    #[must_use]
    pub fn is_exceeded(&self) -> bool {
        self.actual_spending_usd > self.budget_limit_usd
    }

    /// Check if the budget is within acceptable limits.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self.warning_level, BudgetWarningLevel::Ok)
    }

    /// Get remaining budget in USD.
    #[must_use]
    pub fn remaining_usd(&self) -> f64 {
        (self.budget_limit_usd - self.actual_spending_usd).max(0.0)
    }
}

/// Warning level for budget usage.
///
/// Indicates the severity of budget usage based on configured thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetWarningLevel {
    /// Budget usage is within acceptable limits.
    Ok,

    /// Budget usage is approaching the threshold (warning level).
    Warning,

    /// Budget usage is near or exceeds the limit (critical level).
    Critical,
}

/// Budget tracker for monitoring spending against configured limits.
///
/// Queries the store for time-based cost aggregations and compares them
/// against daily and weekly budget thresholds.
pub struct BudgetTracker {
    /// Warning threshold percentage (0-100).
    /// When spending exceeds this percentage, status changes to Warning.
    warn_threshold: u8,

    /// Critical threshold percentage (0-100).
    /// When spending exceeds this percentage, status changes to Critical.
    critical_threshold: u8,
}

impl BudgetTracker {
    /// Create a new budget tracker with the given warning threshold.
    ///
    /// # Arguments
    ///
    /// * `warn_threshold` - Percentage (0-100) at which to trigger warnings
    ///
    /// # Example
    ///
    /// ```
    /// use surge_persistence::budget::BudgetTracker;
    ///
    /// // Warn at 80%, critical at 95%
    /// let tracker = BudgetTracker::new(80);
    /// ```
    #[must_use]
    pub fn new(warn_threshold: u8) -> Self {
        Self {
            warn_threshold: warn_threshold.min(100),
            critical_threshold: 95,
        }
    }

    /// Create a new budget tracker with custom warning and critical thresholds.
    ///
    /// # Arguments
    ///
    /// * `warn_threshold` - Percentage (0-100) at which to trigger warnings
    /// * `critical_threshold` - Percentage (0-100) at which to trigger critical alerts
    #[must_use]
    pub fn with_thresholds(warn_threshold: u8, critical_threshold: u8) -> Self {
        Self {
            warn_threshold: warn_threshold.min(100),
            critical_threshold: critical_threshold.min(100),
        }
    }

    /// Check daily budget status.
    ///
    /// Queries the store for all spending in the current day (UTC) and
    /// compares it against the daily budget limit.
    ///
    /// # Arguments
    ///
    /// * `store` - Reference to the storage backend
    /// * `daily_budget_usd` - Daily budget limit in USD
    ///
    /// # Errors
    ///
    /// Returns an error if querying the store fails.
    pub fn check_daily_budget(&self, store: &Store, daily_budget_usd: f64) -> Result<BudgetStatus> {
        let (start_ms, end_ms) = Self::get_current_day_range();
        let actual_spending = self.get_spending_in_range(store, start_ms, end_ms)?;

        Ok(self.calculate_budget_status(
            daily_budget_usd,
            actual_spending,
            start_ms,
            end_ms,
        ))
    }

    /// Check weekly budget status.
    ///
    /// Queries the store for all spending in the current week (Monday-Sunday, UTC)
    /// and compares it against the weekly budget limit.
    ///
    /// # Arguments
    ///
    /// * `store` - Reference to the storage backend
    /// * `weekly_budget_usd` - Weekly budget limit in USD
    ///
    /// # Errors
    ///
    /// Returns an error if querying the store fails.
    pub fn check_weekly_budget(&self, store: &Store, weekly_budget_usd: f64) -> Result<BudgetStatus> {
        let (start_ms, end_ms) = Self::get_current_week_range();
        let actual_spending = self.get_spending_in_range(store, start_ms, end_ms)?;

        Ok(self.calculate_budget_status(
            weekly_budget_usd,
            actual_spending,
            start_ms,
            end_ms,
        ))
    }

    /// Get budget status for a custom time range.
    ///
    /// # Arguments
    ///
    /// * `store` - Reference to the storage backend
    /// * `budget_usd` - Budget limit for this time range
    /// * `start_ms` - Start of time range (Unix timestamp in milliseconds)
    /// * `end_ms` - End of time range (Unix timestamp in milliseconds)
    ///
    /// # Errors
    ///
    /// Returns an error if querying the store fails.
    pub fn get_budget_status(
        &self,
        store: &Store,
        budget_usd: f64,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<BudgetStatus> {
        let actual_spending = self.get_spending_in_range(store, start_ms, end_ms)?;

        Ok(self.calculate_budget_status(
            budget_usd,
            actual_spending,
            start_ms,
            end_ms,
        ))
    }

    // ── Internal Helpers ────────────────────────────────────────────────

    /// Query the store for total spending in a time range.
    ///
    /// Sums up all `estimated_cost_usd` values from sessions within the
    /// specified time range.
    fn get_spending_in_range(&self, store: &Store, start_ms: u64, end_ms: u64) -> Result<f64> {
        store.get_cost_in_time_range(start_ms, end_ms)
    }

    /// Calculate budget status from spending and limits.
    fn calculate_budget_status(
        &self,
        budget_limit_usd: f64,
        actual_spending_usd: f64,
        period_start_ms: u64,
        period_end_ms: u64,
    ) -> BudgetStatus {
        let usage_percentage = if budget_limit_usd > 0.0 {
            (actual_spending_usd / budget_limit_usd) * 100.0
        } else {
            0.0
        };

        let warning_level = if usage_percentage >= self.critical_threshold as f64 {
            BudgetWarningLevel::Critical
        } else if usage_percentage >= self.warn_threshold as f64 {
            BudgetWarningLevel::Warning
        } else {
            BudgetWarningLevel::Ok
        };

        BudgetStatus {
            budget_limit_usd,
            actual_spending_usd,
            usage_percentage,
            warning_level,
            period_start_ms,
            period_end_ms,
        }
    }

    /// Get the start and end timestamps for the current day (UTC).
    ///
    /// Returns (start_of_day_ms, end_of_day_ms).
    fn get_current_day_range() -> (u64, u64) {
        let now = SystemTime::now();
        let now_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Calculate start of day (00:00:00 UTC)
        // Use integer division to get whole days, then multiply back
        let days_since_epoch = now_ms / (86400 * 1000);
        let start_of_day_ms = days_since_epoch * 86400 * 1000;
        let end_of_day_ms = start_of_day_ms + (86400 * 1000); // +24 hours

        (start_of_day_ms, end_of_day_ms)
    }

    /// Get the start and end timestamps for the current week (Monday-Sunday, UTC).
    ///
    /// Returns (start_of_week_ms, end_of_week_ms).
    fn get_current_week_range() -> (u64, u64) {
        let now = SystemTime::now();
        let now_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Calculate days since Unix epoch (Thursday, Jan 1, 1970)
        let days_since_epoch = now_ms / (86400 * 1000);

        // Thursday = 3 (0=Sunday, 1=Monday, ..., 6=Saturday)
        // We want Monday as week start
        let day_of_week = (days_since_epoch + 3) % 7; // 0=Monday, 1=Tuesday, ...

        // Calculate start of week (Monday 00:00:00 UTC)
        let start_of_week_ms = now_ms - (day_of_week * 86400 * 1000)
            - (now_ms % (86400 * 1000));
        let end_of_week_ms = start_of_week_ms + (7 * 86400 * 1000); // +7 days

        (start_of_week_ms, end_of_week_ms)
    }
}

impl Default for BudgetTracker {
    fn default() -> Self {
        Self::new(80)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SessionUsage;
    use surge_core::id::{SpecId, SubtaskId, TaskId};

    fn create_test_store() -> Store {
        Store::in_memory().expect("Failed to create in-memory store")
    }

    fn create_test_session(timestamp_ms: u64, cost_usd: f64) -> SessionUsage {
        SessionUsage {
            session_id: format!("sess-{}", timestamp_ms),
            agent_name: "test-agent".to_string(),
            task_id: TaskId::new(),
            subtask_id: Some(SubtaskId::new()),
            spec_id: SpecId::new(),
            timestamp_ms,
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: Some(100),
            cached_read_tokens: None,
            cached_write_tokens: None,
            estimated_cost_usd: Some(cost_usd),
        }
    }

    #[test]
    fn test_budget_tracker_new() {
        let tracker = BudgetTracker::new(75);
        assert_eq!(tracker.warn_threshold, 75);
        assert_eq!(tracker.critical_threshold, 95);
    }

    #[test]
    fn test_budget_tracker_with_thresholds() {
        let tracker = BudgetTracker::with_thresholds(70, 90);
        assert_eq!(tracker.warn_threshold, 70);
        assert_eq!(tracker.critical_threshold, 90);
    }

    #[test]
    fn test_budget_status_ok() {
        let status = BudgetStatus {
            budget_limit_usd: 100.0,
            actual_spending_usd: 50.0,
            usage_percentage: 50.0,
            warning_level: BudgetWarningLevel::Ok,
            period_start_ms: 0,
            period_end_ms: 1000,
        };

        assert!(!status.is_exceeded());
        assert!(status.is_ok());
        assert_eq!(status.remaining_usd(), 50.0);
    }

    #[test]
    fn test_budget_status_exceeded() {
        let status = BudgetStatus {
            budget_limit_usd: 100.0,
            actual_spending_usd: 120.0,
            usage_percentage: 120.0,
            warning_level: BudgetWarningLevel::Critical,
            period_start_ms: 0,
            period_end_ms: 1000,
        };

        assert!(status.is_exceeded());
        assert!(!status.is_ok());
        assert_eq!(status.remaining_usd(), 0.0);
    }

    #[test]
    fn test_calculate_budget_status_ok() {
        let tracker = BudgetTracker::new(80);
        let status = tracker.calculate_budget_status(100.0, 50.0, 0, 1000);

        assert_eq!(status.budget_limit_usd, 100.0);
        assert_eq!(status.actual_spending_usd, 50.0);
        assert_eq!(status.usage_percentage, 50.0);
        assert_eq!(status.warning_level, BudgetWarningLevel::Ok);
    }

    #[test]
    fn test_calculate_budget_status_warning() {
        let tracker = BudgetTracker::new(80);
        let status = tracker.calculate_budget_status(100.0, 85.0, 0, 1000);

        assert_eq!(status.usage_percentage, 85.0);
        assert_eq!(status.warning_level, BudgetWarningLevel::Warning);
    }

    #[test]
    fn test_calculate_budget_status_critical() {
        let tracker = BudgetTracker::new(80);
        let status = tracker.calculate_budget_status(100.0, 96.0, 0, 1000);

        assert_eq!(status.usage_percentage, 96.0);
        assert_eq!(status.warning_level, BudgetWarningLevel::Critical);
    }

    #[test]
    fn test_get_current_day_range() {
        let (start_ms, end_ms) = BudgetTracker::get_current_day_range();

        // Should be exactly 24 hours apart
        assert_eq!(end_ms - start_ms, 86400 * 1000);

        // Start should be divisible by 86400000 (one day in milliseconds)
        assert_eq!(start_ms % (86400 * 1000), 0);
    }

    #[test]
    fn test_get_current_week_range() {
        let (start_ms, end_ms) = BudgetTracker::get_current_week_range();

        // Should be exactly 7 days apart
        assert_eq!(end_ms - start_ms, 7 * 86400 * 1000);

        // Start should be at midnight (divisible by one day)
        assert_eq!(start_ms % (86400 * 1000), 0);
    }

    #[test]
    fn test_check_budget_with_no_spending() {
        let store = create_test_store();
        let tracker = BudgetTracker::new(80);

        // No sessions added, should have zero spending
        let status = tracker
            .get_budget_status(&store, 100.0, 0, i64::MAX as u64)
            .expect("get_budget_status failed");

        assert_eq!(status.actual_spending_usd, 0.0);
        assert_eq!(status.usage_percentage, 0.0);
        assert_eq!(status.warning_level, BudgetWarningLevel::Ok);
    }

    #[test]
    fn test_check_budget_with_spending() {
        let mut store = create_test_store();
        let tracker = BudgetTracker::new(80);

        // Add sessions with known costs
        let session1 = create_test_session(1000, 10.0);
        let session2 = create_test_session(2000, 25.0);
        let session3 = create_test_session(3000, 15.0);

        store.insert_session(&session1).expect("insert failed");
        store.insert_session(&session2).expect("insert failed");
        store.insert_session(&session3).expect("insert failed");

        // Query for all sessions (total: 50.0)
        let status = tracker
            .get_budget_status(&store, 100.0, 0, i64::MAX as u64)
            .expect("get_budget_status failed");

        assert_eq!(status.actual_spending_usd, 50.0);
        assert_eq!(status.usage_percentage, 50.0);
        assert_eq!(status.warning_level, BudgetWarningLevel::Ok);
    }

    #[test]
    fn test_check_budget_exceeds_warning() {
        let mut store = create_test_store();
        let tracker = BudgetTracker::new(80);

        // Add sessions that exceed warning threshold
        let session = create_test_session(1000, 85.0);
        store.insert_session(&session).expect("insert failed");

        let status = tracker
            .get_budget_status(&store, 100.0, 0, i64::MAX as u64)
            .expect("get_budget_status failed");

        assert_eq!(status.actual_spending_usd, 85.0);
        assert_eq!(status.usage_percentage, 85.0);
        assert_eq!(status.warning_level, BudgetWarningLevel::Warning);
    }

    #[test]
    fn test_check_budget_exceeds_critical() {
        let mut store = create_test_store();
        let tracker = BudgetTracker::new(80);

        // Add sessions that exceed critical threshold
        let session = create_test_session(1000, 97.0);
        store.insert_session(&session).expect("insert failed");

        let status = tracker
            .get_budget_status(&store, 100.0, 0, i64::MAX as u64)
            .expect("get_budget_status failed");

        assert_eq!(status.actual_spending_usd, 97.0);
        assert_eq!(status.usage_percentage, 97.0);
        assert_eq!(status.warning_level, BudgetWarningLevel::Critical);
    }

    #[test]
    fn test_check_budget_time_range_filtering() {
        let mut store = create_test_store();
        let tracker = BudgetTracker::new(80);

        // Add sessions at different timestamps
        let session1 = create_test_session(1000, 10.0);
        let session2 = create_test_session(5000, 20.0);
        let session3 = create_test_session(9000, 30.0);

        store.insert_session(&session1).expect("insert failed");
        store.insert_session(&session2).expect("insert failed");
        store.insert_session(&session3).expect("insert failed");

        // Query for middle session only (5000 in range [4000, 6000])
        let status = tracker
            .get_budget_status(&store, 100.0, 4000, 6000)
            .expect("get_budget_status failed");

        assert_eq!(status.actual_spending_usd, 20.0);
        assert_eq!(status.usage_percentage, 20.0);
    }
}
