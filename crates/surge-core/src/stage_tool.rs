//! Authenticated stage-tool identities and nonterminal durable receipts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::content_hash::ContentHash;
use crate::id::{RunId, SessionId, StageGenerationId};
use crate::keys::{NodeKey, OutcomeKey};

/// Identity pinned by the engine, never supplied by model tool arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageToolContext {
    pub run: RunId,
    pub node: NodeKey,
    pub session: SessionId,
    pub generation: StageGenerationId,
}

/// Caller-selected retry identity, scoped to one authenticated stage generation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StageToolCallId(String);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("call_id must contain 1..=128 ASCII letters, digits, underscores, dots or hyphens")]
pub struct InvalidStageToolCallId;

impl TryFrom<String> for StageToolCallId {
    type Error = InvalidStageToolCallId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(InvalidStageToolCallId);
        }
        Ok(Self(value))
    }
}

impl From<StageToolCallId> for String {
    fn from(value: StageToolCallId) -> Self {
        value.0
    }
}

impl StageToolCallId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A candidate is not an outcome: validators and prompt completion remain pending.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageOutcomeCandidate {
    pub outcome: OutcomeKey,
    pub summary: String,
    #[serde(default)]
    pub artifacts_produced: Vec<String>,
}

/// Durable result returned by an authenticated stage-tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StageToolResult {
    /// Basic parsing and declared-outcome acceptance only, never final validation.
    OutcomeCandidate { candidate: StageOutcomeCandidate },
    /// The durable human resolution returned to the provider.
    HumanResponse { response: Value },
    /// An idempotent rejection; retrying changed arguments requires a new call ID.
    Rejected { tool: String, reason: String },
}

/// Recorded before the MCP reply. Replay must not mark a stage or task complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageToolReceipt {
    pub context: StageToolContext,
    pub call_id: StageToolCallId,
    pub arguments_hash: ContentHash,
    pub result: StageToolResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_roundtrip_is_nonterminal_and_v8_history_remains_readable() {
        use crate::run_event::{EventPayload, RunEvent, VersionedEventPayload};
        use crate::run_state::{RunState, apply};
        let context = StageToolContext {
            run: RunId::new(),
            node: NodeKey::try_from("impl_1").unwrap(),
            session: SessionId::new(),
            generation: StageGenerationId::new(),
        };
        let payload = EventPayload::StageToolReceipt {
            receipt: StageToolReceipt {
                context: context.clone(),
                call_id: "report-1".to_owned().try_into().unwrap(),
                arguments_hash: ContentHash::compute(b"arguments"),
                result: StageToolResult::OutcomeCandidate {
                    candidate: StageOutcomeCandidate {
                        outcome: OutcomeKey::try_from("done").unwrap(),
                        summary: "candidate".into(),
                        artifacts_produced: vec![],
                    },
                },
            },
        };
        let wrapper = VersionedEventPayload::new(payload.clone());
        assert_eq!(wrapper.schema_version, 9);
        let bytes = serde_json::to_vec(&wrapper).unwrap();
        assert_eq!(
            crate::migrations::migrate_payload(9, &bytes).unwrap(),
            payload
        );
        let event = RunEvent {
            run_id: context.run,
            seq: 1,
            timestamp: chrono::Utc::now(),
            payload,
        };
        assert_eq!(
            apply(RunState::NotStarted, &event).unwrap(),
            RunState::NotStarted
        );
        let old =
            br#"{"schema_version":8,"payload":{"type":"run_aborted","reason":"old history"}}"#;
        assert_eq!(
            crate::migrations::migrate_payload(8, old).unwrap(),
            EventPayload::RunAborted {
                reason: "old history".into()
            }
        );
        assert!(matches!(
            crate::migrations::migrate_payload(10, &bytes),
            Err(crate::SurgeError::SchemaTooNew { .. })
        ));
    }

    #[test]
    fn deserialization_enforces_call_identity_constraints() {
        for bad in ["", "../with/slash", "hello world", "☃"] {
            assert!(StageToolCallId::try_from(bad.to_owned()).is_err());
            assert!(serde_json::from_value::<StageToolCallId>(Value::String(bad.into())).is_err());
        }
        assert!(StageToolCallId::try_from("a".repeat(129)).is_err());
        let id: StageToolCallId = serde_json::from_str("\"report-1.retry_2\"").unwrap();
        assert_eq!(id.as_str(), "report-1.retry_2");
    }
}
