//! Provider-reported token spend is separate from ACP context-window occupancy.
use serde::{Deserialize, Serialize};

/// Exact cumulative or incremental counters from an ACP prompt response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageCounters {
    /// Provider total, retained independently of the individual categories.
    pub total_tokens: u64,
    /// Provider-reported input tokens.
    pub input_tokens: u64,
    /// Provider-reported output tokens.
    pub output_tokens: u64,
    /// Thought tokens when the provider reports them.
    pub thought_tokens: Option<u64>,
    /// Tokens read from cache when reported.
    pub cached_read_tokens: Option<u64>,
    /// Tokens written to cache when reported.
    pub cached_write_tokens: Option<u64>,
}

/// Provider evidence and the accepted delta; absent evidence never means zero spend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TokenUsageObservation {
    /// Valid counters and the accepted incremental consumption.
    Reported {
        /// Original cumulative provider evidence.
        cumulative: TokenUsageCounters,
        /// Difference from the previous accepted counters.
        delta: TokenUsageCounters,
    },
    /// Consumption cannot be determined from this response.
    Unavailable {
        /// Why this observation cannot establish consumption.
        reason: UsageUnavailableReason,
        /// Raw counters retained for diagnosis when present.
        reported: Option<TokenUsageCounters>,
    },
}

/// Why provider evidence cannot establish incremental token consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageUnavailableReason {
    /// A restored session has no accepted baseline for its cumulative counters.
    BaselineUnknown,
    /// The prompt response carried no token counters.
    Missing,
    /// A cumulative counter fell below its previous accepted value.
    CounterRegression,
}

/// ACP usage-update notification: occupancy, never token consumption.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextUsage {
    /// Current context-window occupancy.
    pub used: u64,
    /// Context-window capacity.
    pub size: u64,
    /// Session-level cost metadata, not an incremental USD price.
    pub cost: Option<SessionCost>,
}

/// Provider-reported session cost in its original currency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCost {
    /// Provider amount without currency conversion.
    pub amount: f64,
    /// Provider currency identifier.
    pub currency: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{id::SessionId, run_event::EventPayload};

    #[test]
    fn raw_provider_evidence_roundtrips_without_losing_large_counters() {
        let counters = TokenUsageCounters {
            total_tokens: u64::MAX,
            input_tokens: u64::MAX - 1,
            output_tokens: 1,
            thought_tokens: Some(0),
            cached_read_tokens: None,
            cached_write_tokens: Some(3),
        };
        let payload = EventPayload::ProviderUsageObserved {
            session: SessionId::new(),
            usage: TokenUsageObservation::Reported {
                cumulative: counters.clone(),
                delta: counters.clone(),
            },
        };
        let encoded = serde_json::to_string(&payload).unwrap();
        let decoded: EventPayload = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(decoded.discriminant_str(), "ProviderUsageObserved");
        assert_eq!(decoded.token_delta().unwrap().1, u64::MAX - 1);
    }

    #[test]
    fn occupancy_and_missing_counters_never_become_consumption() {
        let session = SessionId::new();
        let context = EventPayload::ContextUsageObserved {
            session,
            usage: ContextUsage {
                used: 800,
                size: 1000,
                cost: Some(SessionCost {
                    amount: 1.5,
                    currency: "EUR".into(),
                }),
            },
        };
        let missing = EventPayload::ProviderUsageObserved {
            session,
            usage: TokenUsageObservation::Unavailable {
                reason: UsageUnavailableReason::Missing,
                reported: None,
            },
        };
        for payload in [context, missing] {
            assert!(payload.token_delta().is_none());
            let encoded = serde_json::to_string(&payload).unwrap();
            assert_eq!(
                serde_json::from_str::<EventPayload>(&encoded).unwrap(),
                payload
            );
        }
    }
}
