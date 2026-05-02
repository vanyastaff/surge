//! Event aggregator for token usage tracking.
//!
//! Listens to `TokensConsumed` events from the ACP pool and aggregates
//! them into session, subtask, and spec-level usage records in the store.

use crate::Result;
use crate::models::{SessionUsage, SpecUsage, SubtaskUsage};
use crate::pricing::{
    PricingModel, claude_opus_pricing, claude_sonnet_35_pricing, gemini_pro_pricing,
    gpt4_turbo_pricing,
};
use crate::store::Store;
use std::collections::HashMap;
use std::sync::Arc;
use surge_core::SurgeEvent;
use surge_core::id::{SpecId, SubtaskId, TaskId};
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, warn};

/// Context associated with an active session.
///
/// Maps a session_id to the task, subtask, and spec it belongs to,
/// enabling proper aggregation of token usage.
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// Task this session belongs to.
    pub task_id: TaskId,
    /// Subtask this session belongs to (if any).
    pub subtask_id: Option<SubtaskId>,
    /// Spec this session is associated with.
    pub spec_id: SpecId,
}

/// Aggregator that listens to TokensConsumed events and writes to the store.
///
/// The aggregator maintains a mapping of session IDs to their context
/// (task, subtask, spec) and aggregates token usage into the persistence layer.
pub struct UsageAggregator {
    /// Storage backend.
    store: Arc<Mutex<Store>>,
    /// Session ID to context mapping.
    sessions: Arc<Mutex<HashMap<String, SessionContext>>>,
    /// Pricing models mapped by agent name.
    pricing: Arc<HashMap<String, PricingModel>>,
}

impl UsageAggregator {
    /// Create a new usage aggregator with the given store.
    pub fn new(store: Store) -> Self {
        // Initialize pricing map with default models for common agents
        let mut pricing = HashMap::new();

        // Claude variants
        pricing.insert("claude".to_string(), claude_sonnet_35_pricing());
        pricing.insert("claude-sonnet".to_string(), claude_sonnet_35_pricing());
        pricing.insert("claude-opus".to_string(), claude_opus_pricing());
        pricing.insert("claude-haiku".to_string(), claude_sonnet_35_pricing()); // Use sonnet pricing for haiku

        // GPT variants
        pricing.insert("gpt".to_string(), gpt4_turbo_pricing());
        pricing.insert("gpt-4".to_string(), gpt4_turbo_pricing());
        pricing.insert("gpt4".to_string(), gpt4_turbo_pricing());

        // Gemini variants
        pricing.insert("gemini".to_string(), gemini_pro_pricing());
        pricing.insert("gemini-pro".to_string(), gemini_pro_pricing());

        Self {
            store: Arc::new(Mutex::new(store)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pricing: Arc::new(pricing),
        }
    }

    /// Create a new usage aggregator with the given store and custom pricing.
    ///
    /// If `custom_pricing` is provided, it will be used for all agents.
    /// Otherwise, falls back to default pricing models for common agents.
    pub fn new_with_pricing(store: Store, custom_pricing: Option<PricingModel>) -> Self {
        let mut pricing = HashMap::new();

        if let Some(model) = custom_pricing {
            // Use custom pricing for all common agent name variants
            pricing.insert("claude".to_string(), model.clone());
            pricing.insert("claude-sonnet".to_string(), model.clone());
            pricing.insert("claude-opus".to_string(), model.clone());
            pricing.insert("claude-haiku".to_string(), model.clone());
            pricing.insert("gpt".to_string(), model.clone());
            pricing.insert("gpt-4".to_string(), model.clone());
            pricing.insert("gpt4".to_string(), model.clone());
            pricing.insert("gemini".to_string(), model.clone());
            pricing.insert("gemini-pro".to_string(), model);
        } else {
            // Fall back to defaults
            pricing.insert("claude".to_string(), claude_sonnet_35_pricing());
            pricing.insert("claude-sonnet".to_string(), claude_sonnet_35_pricing());
            pricing.insert("claude-opus".to_string(), claude_opus_pricing());
            pricing.insert("claude-haiku".to_string(), claude_sonnet_35_pricing());
            pricing.insert("gpt".to_string(), gpt4_turbo_pricing());
            pricing.insert("gpt-4".to_string(), gpt4_turbo_pricing());
            pricing.insert("gpt4".to_string(), gpt4_turbo_pricing());
            pricing.insert("gemini".to_string(), gemini_pro_pricing());
            pricing.insert("gemini-pro".to_string(), gemini_pro_pricing());
        }

        Self {
            store: Arc::new(Mutex::new(store)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pricing: Arc::new(pricing),
        }
    }

    /// Register a session with its context.
    ///
    /// This must be called before any TokensConsumed events are received for
    /// this session, otherwise the events will be ignored.
    pub async fn register_session(&self, session_id: String, context: SessionContext) {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(session_id, context);
    }

    /// Unregister a session when it's no longer active.
    ///
    /// This cleans up the internal session mapping to prevent memory leaks.
    pub async fn unregister_session(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(session_id);
    }

    /// Get a reference to the underlying store for checkpoint operations.
    ///
    /// Returns a cloned Arc to the store, allowing concurrent access for
    /// checkpoint saves during task execution.
    #[must_use]
    pub fn store(&self) -> Arc<Mutex<Store>> {
        Arc::clone(&self.store)
    }

    /// Start listening to events from the given broadcast channel.
    ///
    /// This spawns a background task that processes TokensConsumed events
    /// and aggregates them into the store. Returns a join handle for the
    /// background task.
    pub fn start_listening(
        &self,
        mut event_rx: broadcast::Receiver<SurgeEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let store = Arc::clone(&self.store);
        let sessions = Arc::clone(&self.sessions);
        let pricing = Arc::clone(&self.pricing);

        tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        if let Err(e) = Self::handle_event(&store, &sessions, &pricing, event).await
                        {
                            warn!("Failed to handle event: {}", e);
                        }
                    },
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Event channel closed, stopping aggregator");
                        break;
                    },
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Aggregator lagged, skipped {} events", skipped);
                    },
                }
            }
        })
    }

    /// Handle a single event.
    async fn handle_event(
        store: &Arc<Mutex<Store>>,
        sessions: &Arc<Mutex<HashMap<String, SessionContext>>>,
        pricing: &Arc<HashMap<String, PricingModel>>,
        event: SurgeEvent,
    ) -> Result<()> {
        // Only process TokensConsumed events
        let SurgeEvent::TokensConsumed {
            session_id,
            agent_name,
            input_tokens,
            output_tokens,
            thought_tokens,
            cached_read_tokens,
            cached_write_tokens,
            ..
        } = event
        else {
            return Ok(());
        };

        // Look up session context
        let context = {
            let sessions = sessions.lock().await;
            sessions.get(&session_id).cloned()
        };

        let Some(context) = context else {
            warn!(
                "Received TokensConsumed for unknown session: {}",
                session_id
            );
            return Ok(());
        };

        // Get current timestamp
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Calculate cost using pricing model
        let estimated_cost_usd = pricing.get(&agent_name).map(|model| {
            model.calculate_cost(
                input_tokens,
                output_tokens,
                thought_tokens,
                cached_read_tokens,
                cached_write_tokens,
            )
        });

        // Create session usage record
        let session = SessionUsage {
            session_id: session_id.clone(),
            agent_name,
            task_id: context.task_id,
            subtask_id: context.subtask_id,
            spec_id: context.spec_id,
            timestamp_ms,
            input_tokens,
            output_tokens,
            thought_tokens,
            cached_read_tokens,
            cached_write_tokens,
            estimated_cost_usd,
        };

        // Write to store
        let mut store = store.lock().await;

        // Insert session record
        store.insert_session(&session)?;

        // Aggregate into subtask usage if this session has a subtask
        if let Some(subtask_id) = context.subtask_id {
            let existing = store.get_subtask(subtask_id, context.task_id, context.spec_id)?;

            let subtask = if let Some(mut existing) = existing {
                existing.add_session(&session);
                Some(existing)
            } else {
                SubtaskUsage::from_session(&session)
            };

            if let Some(subtask) = subtask {
                store.upsert_subtask(&subtask)?;
            }
        }

        // Aggregate into spec usage
        let existing = store.get_spec(context.spec_id)?;

        let spec = if let Some(mut existing) = existing {
            existing.add_session(&session);
            existing
        } else {
            SpecUsage::from_session(&session)
        };

        store.upsert_spec(&spec)?;

        debug!(
            "Aggregated {} input + {} output tokens for session {}",
            input_tokens, output_tokens, session_id
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::{SpecId, SubtaskId, TaskId};
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn test_register_and_unregister_session() {
        let store = Store::in_memory().unwrap();
        let aggregator = UsageAggregator::new(store);

        let session_id = "test-session".to_string();
        let context = SessionContext {
            task_id: TaskId::new(),
            subtask_id: Some(SubtaskId::new()),
            spec_id: SpecId::new(),
        };

        // Register
        aggregator
            .register_session(session_id.clone(), context.clone())
            .await;

        let sessions = aggregator.sessions.lock().await;
        assert!(sessions.contains_key(&session_id));

        drop(sessions);

        // Unregister
        aggregator.unregister_session(&session_id).await;

        let sessions = aggregator.sessions.lock().await;
        assert!(!sessions.contains_key(&session_id));
    }

    #[tokio::test]
    async fn test_handle_tokens_consumed_event() {
        let store = Store::in_memory().unwrap();
        let aggregator = UsageAggregator::new(store);

        let session_id = "test-session".to_string();
        let spec_id = SpecId::new();
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();

        // Register session
        let context = SessionContext {
            task_id,
            subtask_id: Some(subtask_id),
            spec_id,
        };
        aggregator
            .register_session(session_id.clone(), context)
            .await;

        // Create event
        let event = SurgeEvent::TokensConsumed {
            session_id: session_id.clone(),
            agent_name: "claude".to_string(),
            spec_id: Some(spec_id),
            subtask_id: Some(subtask_id),
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: Some(200),
            cached_read_tokens: Some(100),
            cached_write_tokens: Some(50),
            estimated_cost_usd: Some(0.005),
        };

        // Handle event
        UsageAggregator::handle_event(
            &aggregator.store,
            &aggregator.sessions,
            &aggregator.pricing,
            event,
        )
        .await
        .unwrap();

        // Verify session was stored
        let store = aggregator.store.lock().await;
        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.input_tokens, 1000);
        assert_eq!(session.output_tokens, 500);
        assert_eq!(session.thought_tokens, Some(200));

        // Verify subtask was aggregated
        let subtask = store
            .get_subtask(subtask_id, task_id, spec_id)
            .unwrap()
            .unwrap();
        assert_eq!(subtask.input_tokens, 1000);
        assert_eq!(subtask.output_tokens, 500);
        assert_eq!(subtask.thought_tokens, 200);
        assert_eq!(subtask.session_count, 1);

        // Verify spec was aggregated
        let spec = store.get_spec(spec_id).unwrap().unwrap();
        assert_eq!(spec.input_tokens, 1000);
        assert_eq!(spec.output_tokens, 500);
        assert_eq!(spec.thought_tokens, 200);
        assert_eq!(spec.session_count, 1);
    }

    #[tokio::test]
    async fn test_handle_multiple_sessions() {
        let store = Store::in_memory().unwrap();
        let aggregator = UsageAggregator::new(store);

        let spec_id = SpecId::new();
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();

        // Register first session
        let session1_id = "session-1".to_string();
        aggregator
            .register_session(
                session1_id.clone(),
                SessionContext {
                    task_id,
                    subtask_id: Some(subtask_id),
                    spec_id,
                },
            )
            .await;

        // Register second session
        let session2_id = "session-2".to_string();
        aggregator
            .register_session(
                session2_id.clone(),
                SessionContext {
                    task_id,
                    subtask_id: Some(subtask_id),
                    spec_id,
                },
            )
            .await;

        // Handle first event
        let event1 = SurgeEvent::TokensConsumed {
            session_id: session1_id.clone(),
            agent_name: "claude".to_string(),
            spec_id: Some(spec_id),
            subtask_id: Some(subtask_id),
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            estimated_cost_usd: Some(0.005),
        };
        UsageAggregator::handle_event(
            &aggregator.store,
            &aggregator.sessions,
            &aggregator.pricing,
            event1,
        )
        .await
        .unwrap();

        // Handle second event
        let event2 = SurgeEvent::TokensConsumed {
            session_id: session2_id.clone(),
            agent_name: "claude".to_string(),
            spec_id: Some(spec_id),
            subtask_id: Some(subtask_id),
            input_tokens: 800,
            output_tokens: 400,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            estimated_cost_usd: Some(0.004),
        };
        UsageAggregator::handle_event(
            &aggregator.store,
            &aggregator.sessions,
            &aggregator.pricing,
            event2,
        )
        .await
        .unwrap();

        // Verify aggregation
        let store = aggregator.store.lock().await;

        // Subtask should have both sessions aggregated
        let subtask = store
            .get_subtask(subtask_id, task_id, spec_id)
            .unwrap()
            .unwrap();
        assert_eq!(subtask.input_tokens, 1800); // 1000 + 800
        assert_eq!(subtask.output_tokens, 900); // 500 + 400
        assert_eq!(subtask.session_count, 2);
        // Session1: 1000 input ($0.003) + 500 output ($0.0075) = $0.0105
        // Session2: 800 input ($0.0024) + 400 output ($0.006) = $0.0084
        // Total = $0.0189
        assert!((subtask.estimated_cost_usd - 0.0189).abs() < 1e-6);

        // Spec should have both sessions aggregated
        let spec = store.get_spec(spec_id).unwrap().unwrap();
        assert_eq!(spec.input_tokens, 1800);
        assert_eq!(spec.output_tokens, 900);
        assert_eq!(spec.session_count, 2);
        assert!((spec.estimated_cost_usd - 0.0189).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_handle_unknown_session() {
        let store = Store::in_memory().unwrap();
        let aggregator = UsageAggregator::new(store);

        // Don't register the session

        // Create event for unknown session
        let event = SurgeEvent::TokensConsumed {
            session_id: "unknown-session".to_string(),
            agent_name: "claude".to_string(),
            spec_id: None,
            subtask_id: None,
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            estimated_cost_usd: Some(0.005),
        };

        // Handle event - should not error but should log warning
        UsageAggregator::handle_event(
            &aggregator.store,
            &aggregator.sessions,
            &aggregator.pricing,
            event,
        )
        .await
        .unwrap();

        // Verify nothing was stored
        let store = aggregator.store.lock().await;
        let session = store.get_session("unknown-session").unwrap();
        assert!(session.is_none());
    }

    #[tokio::test]
    async fn test_handle_non_tokens_consumed_event() {
        let store = Store::in_memory().unwrap();
        let aggregator = UsageAggregator::new(store);

        // Create a different event type
        let event = SurgeEvent::AgentConnected {
            agent_name: "claude".to_string(),
        };

        // Handle event - should be ignored without error
        UsageAggregator::handle_event(
            &aggregator.store,
            &aggregator.sessions,
            &aggregator.pricing,
            event,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_start_listening() {
        let store = Store::in_memory().unwrap();
        let aggregator = UsageAggregator::new(store);

        let spec_id = SpecId::new();
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();
        let session_id = "test-session".to_string();

        // Register session
        aggregator
            .register_session(
                session_id.clone(),
                SessionContext {
                    task_id,
                    subtask_id: Some(subtask_id),
                    spec_id,
                },
            )
            .await;

        // Create broadcast channel
        let (tx, rx) = broadcast::channel(16);

        // Start listening
        let handle = aggregator.start_listening(rx);

        // Send event
        let event = SurgeEvent::TokensConsumed {
            session_id: session_id.clone(),
            agent_name: "claude".to_string(),
            spec_id: Some(spec_id),
            subtask_id: Some(subtask_id),
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            estimated_cost_usd: Some(0.005),
        };
        tx.send(event).unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify the event was processed
        let store = aggregator.store.lock().await;
        let session = store.get_session(&session_id).unwrap();
        assert!(session.is_some());
        assert_eq!(session.unwrap().input_tokens, 1000);

        // Clean up
        drop(tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_session_without_subtask() {
        let store = Store::in_memory().unwrap();
        let aggregator = UsageAggregator::new(store);

        let session_id = "test-session".to_string();
        let spec_id = SpecId::new();
        let task_id = TaskId::new();

        // Register session without subtask
        let context = SessionContext {
            task_id,
            subtask_id: None, // No subtask
            spec_id,
        };
        aggregator
            .register_session(session_id.clone(), context)
            .await;

        // Create event
        let event = SurgeEvent::TokensConsumed {
            session_id: session_id.clone(),
            agent_name: "claude".to_string(),
            spec_id: Some(spec_id),
            subtask_id: None,
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            estimated_cost_usd: Some(0.005),
        };

        // Handle event
        UsageAggregator::handle_event(
            &aggregator.store,
            &aggregator.sessions,
            &aggregator.pricing,
            event,
        )
        .await
        .unwrap();

        // Verify session was stored
        let store = aggregator.store.lock().await;
        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.input_tokens, 1000);
        assert!(session.subtask_id.is_none());

        // Verify spec was aggregated
        let spec = store.get_spec(spec_id).unwrap().unwrap();
        assert_eq!(spec.input_tokens, 1000);
        assert_eq!(spec.output_tokens, 500);
        assert_eq!(spec.session_count, 1);
        assert_eq!(spec.subtask_count, 0); // No subtask
    }

    #[tokio::test]
    async fn test_cost_calculation() {
        let store = Store::in_memory().unwrap();
        let aggregator = UsageAggregator::new(store);

        let session_id = "test-session".to_string();
        let spec_id = SpecId::new();
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();

        // Register session
        let context = SessionContext {
            task_id,
            subtask_id: Some(subtask_id),
            spec_id,
        };
        aggregator
            .register_session(session_id.clone(), context)
            .await;

        // Create event with known token counts
        let event = SurgeEvent::TokensConsumed {
            session_id: session_id.clone(),
            agent_name: "claude".to_string(),
            spec_id: Some(spec_id),
            subtask_id: Some(subtask_id),
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: Some(200),
            cached_read_tokens: Some(100),
            cached_write_tokens: Some(50),
            estimated_cost_usd: Some(0.005),
        };

        // Handle event
        UsageAggregator::handle_event(
            &aggregator.store,
            &aggregator.sessions,
            &aggregator.pricing,
            event,
        )
        .await
        .unwrap();

        // Verify cost was calculated correctly
        // Based on claude_sonnet_35_pricing():
        // - Input: $3.00/M tokens → 1000 tokens = $0.003
        // - Output: $15.00/M tokens → 500 tokens = $0.0075
        // - Thought: $15.00/M tokens → 200 tokens = $0.003
        // - Cache read: $0.30/M tokens → 100 tokens = $0.00003
        // - Cache write: $3.75/M tokens → 50 tokens = $0.0001875
        // Total = $0.0137175
        let expected_cost = 0.0137175;

        let store = aggregator.store.lock().await;
        let session = store.get_session(&session_id).unwrap().unwrap();

        assert!(session.estimated_cost_usd.is_some());
        let actual_cost = session.estimated_cost_usd.unwrap();
        assert!(
            (actual_cost - expected_cost).abs() < 1e-6,
            "Expected cost {}, got {}",
            expected_cost,
            actual_cost
        );
    }
}
