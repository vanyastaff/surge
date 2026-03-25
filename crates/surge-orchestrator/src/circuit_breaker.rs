//! Circuit breaker for subtask execution resilience.

use std::sync::Arc;
use surge_core::event::SurgeEvent;
use surge_core::id::{SubtaskId, TaskId};
use surge_persistence::models::CircuitBreakerState;
use surge_persistence::store::Store;
use tokio::sync::{Mutex, broadcast};
use tracing::{info, warn};

/// Manages circuit breaker state for a single subtask.
///
/// Tracks consecutive failures and trips the circuit when threshold is exceeded.
/// Persists state to disk so circuit remains tripped across restarts.
pub struct CircuitBreaker {
    /// Current state of the circuit breaker.
    state: CircuitBreakerState,
    /// Threshold for tripping the circuit.
    threshold: u32,
    /// Reference to the persistence store.
    store: Option<Arc<Mutex<Store>>>,
    /// Event broadcaster for emitting circuit breaker events.
    event_tx: broadcast::Sender<SurgeEvent>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker for a subtask.
    ///
    /// Attempts to load existing state from the store. If no state exists,
    /// creates a new one with zero failures.
    pub async fn new(
        task_id: TaskId,
        subtask_id: SubtaskId,
        threshold: u32,
        store: Option<Arc<Mutex<Store>>>,
        event_tx: broadcast::Sender<SurgeEvent>,
    ) -> Self {
        // Try to load existing state from persistence
        let state = if let Some(store_ref) = &store {
            let store_guard = store_ref.lock().await;
            match store_guard.load_circuit_breaker_state(task_id, subtask_id) {
                Ok(Some(loaded_state)) => {
                    info!(
                        task_id = %task_id,
                        subtask_id = %subtask_id,
                        consecutive_failures = loaded_state.consecutive_failures,
                        is_tripped = loaded_state.is_tripped(),
                        "loaded circuit breaker state from persistence"
                    );
                    loaded_state
                }
                Ok(None) => {
                    info!(
                        task_id = %task_id,
                        subtask_id = %subtask_id,
                        "no existing circuit breaker state, creating new"
                    );
                    CircuitBreakerState::new(task_id, subtask_id)
                }
                Err(e) => {
                    warn!(
                        task_id = %task_id,
                        subtask_id = %subtask_id,
                        error = %e,
                        "failed to load circuit breaker state, creating new"
                    );
                    CircuitBreakerState::new(task_id, subtask_id)
                }
            }
        } else {
            CircuitBreakerState::new(task_id, subtask_id)
        };

        Self {
            state,
            threshold,
            store,
            event_tx,
        }
    }

    /// Check if the circuit breaker is currently tripped.
    #[must_use]
    pub fn is_tripped(&self) -> bool {
        self.state.is_tripped()
    }

    /// Get the current consecutive failure count.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.state.consecutive_failures
    }

    /// Get the last error message.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.state.last_error.as_deref()
    }

    /// Get the next retry time in milliseconds since Unix epoch.
    #[must_use]
    pub fn next_retry_time(&self) -> Option<u64> {
        self.state.next_retry_time
    }

    /// Record a failure and potentially trip the circuit.
    ///
    /// Increments the consecutive failure count, saves state, and trips the
    /// circuit if threshold is exceeded. Emits appropriate events.
    pub async fn record_failure(&mut self, error_msg: String, next_retry_time_ms: Option<u64>) {
        self.state.record_failure(error_msg.clone(), next_retry_time_ms);

        let should_trip = self.state.consecutive_failures >= self.threshold
            && !self.state.is_tripped();

        if should_trip {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .inspect_err(|e| warn!("system clock before Unix epoch: {e}"))
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            self.state.trip(now_ms);

            warn!(
                task_id = %self.state.task_id,
                subtask_id = %self.state.subtask_id,
                consecutive_failures = self.state.consecutive_failures,
                threshold = self.threshold,
                "circuit breaker tripped"
            );

            // Emit circuit breaker tripped event
            let _ = self.event_tx.send(SurgeEvent::CircuitBreakerOpened {
                agent_name: format!("subtask-{}", self.state.subtask_id),
                reason: error_msg.clone(),
                failure_count: self.state.consecutive_failures,
            });
        }

        // Persist state
        self.save_state().await;
    }

    /// Reset the circuit breaker after a successful execution.
    ///
    /// Clears failure count and state, saves to persistence, and emits reset event.
    pub async fn reset(&mut self) {
        if self.state.consecutive_failures > 0 || self.state.is_tripped() {
            info!(
                task_id = %self.state.task_id,
                subtask_id = %self.state.subtask_id,
                previous_failures = self.state.consecutive_failures,
                "circuit breaker reset after successful execution"
            );

            self.state.reset();

            // Emit circuit breaker reset event
            let _ = self.event_tx.send(SurgeEvent::CircuitBreakerClosed {
                agent_name: format!("subtask-{}", self.state.subtask_id),
            });

            // Persist reset state
            self.save_state().await;
        }
    }

    /// Save the current state to persistence.
    async fn save_state(&mut self) {
        if let Some(store_ref) = &self.store {
            let mut store_guard = store_ref.lock().await;
            if let Err(e) = store_guard.save_circuit_breaker_state(&self.state) {
                warn!(
                    task_id = %self.state.task_id,
                    subtask_id = %self.state.subtask_id,
                    error = %e,
                    "failed to save circuit breaker state"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::{SubtaskId, TaskId};
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn test_circuit_breaker_new_state() {
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();
        let (event_tx, _rx) = broadcast::channel(10);

        let cb = CircuitBreaker::new(task_id, subtask_id, 3, None, event_tx).await;

        assert_eq!(cb.consecutive_failures(), 0);
        assert!(!cb.is_tripped());
        assert_eq!(cb.last_error(), None);
        assert_eq!(cb.next_retry_time(), None);
    }

    #[tokio::test]
    async fn test_circuit_breaker_record_failures() {
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();
        let (event_tx, mut rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(task_id, subtask_id, 3, None, event_tx).await;

        // First failure
        cb.record_failure("error 1".to_string(), None).await;
        assert_eq!(cb.consecutive_failures(), 1);
        assert!(!cb.is_tripped());

        // Second failure
        cb.record_failure("error 2".to_string(), None).await;
        assert_eq!(cb.consecutive_failures(), 2);
        assert!(!cb.is_tripped());

        // Third failure should trip the circuit
        cb.record_failure("error 3".to_string(), Some(12345)).await;
        assert_eq!(cb.consecutive_failures(), 3);
        assert!(cb.is_tripped());
        assert_eq!(cb.last_error(), Some("error 3"));
        assert_eq!(cb.next_retry_time(), Some(12345));

        // Should have received a CircuitBreakerOpened event
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, SurgeEvent::CircuitBreakerOpened { .. }));
    }

    #[tokio::test]
    async fn test_circuit_breaker_reset() {
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();
        let (event_tx, mut rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(task_id, subtask_id, 3, None, event_tx).await;

        // Record some failures
        cb.record_failure("error 1".to_string(), None).await;
        cb.record_failure("error 2".to_string(), None).await;
        assert_eq!(cb.consecutive_failures(), 2);

        // Reset should clear failures
        cb.reset().await;
        assert_eq!(cb.consecutive_failures(), 0);
        assert!(!cb.is_tripped());
        assert_eq!(cb.last_error(), None);
        assert_eq!(cb.next_retry_time(), None);

        // Should have received a CircuitBreakerClosed event
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, SurgeEvent::CircuitBreakerClosed { .. }));
    }

    #[tokio::test]
    async fn test_circuit_breaker_threshold() {
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();
        let (event_tx, _rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(task_id, subtask_id, 5, None, event_tx).await;

        // Record failures below threshold
        for i in 0..4 {
            cb.record_failure(format!("error {}", i + 1), None).await;
            assert!(!cb.is_tripped(), "should not trip at failure {}", i + 1);
        }

        // Fifth failure should trip
        cb.record_failure("error 5".to_string(), None).await;
        assert!(cb.is_tripped());
    }

    #[tokio::test]
    async fn test_circuit_breaker_no_duplicate_trip_events() {
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();
        let (event_tx, mut rx) = broadcast::channel(10);

        let mut cb = CircuitBreaker::new(task_id, subtask_id, 2, None, event_tx).await;

        // Trip the circuit
        cb.record_failure("error 1".to_string(), None).await;
        cb.record_failure("error 2".to_string(), None).await;
        assert!(cb.is_tripped());

        // Should have one event
        let _event = rx.try_recv().unwrap();
        assert!(rx.try_recv().is_err());

        // Additional failures should not emit more trip events
        cb.record_failure("error 3".to_string(), None).await;
        assert!(rx.try_recv().is_err());
    }
}
