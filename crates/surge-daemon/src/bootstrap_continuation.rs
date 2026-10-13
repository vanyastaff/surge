//! Validate durable planning output before committing or replaying child launch intent.
//!
//! What the child inherits — selection, validation and the operator's
//! flow-gate edits — is the bootstrap handoff's
//! ([`surge_orchestrator::bootstrap::handoff`], #98). This module adds only
//! daemon policy: completion confirmation through the tracked-run rule,
//! credential readiness for the child's agents, and the shared token budget.
use crate::bootstrap_runtime::BootstrapRuntime;
use std::{collections::HashSet, sync::Arc};
use surge_core::{
    RunId,
    bootstrap_continuation::BootstrapContinuation,
    bootstrap_operation::{BootstrapContentRef, BootstrapIntent},
    budget::BudgetGuard,
    graph::Graph,
    run_event::EventPayload,
    run_verdict::{TerminalClaim, TerminalOutcome, TerminalVerdict},
};
use surge_orchestrator::bootstrap::handoff::{
    BootstrapHandoff, HandoffContext, HandoffRejection, prepare, reload,
};
use surge_orchestrator::engine::RunOutcome;
use surge_persistence::runs::{ReadEvent, Storage, inspection::RunDatabaseInspection};

/// A continuation refusal; no child launch intent may be committed.
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum ContinuationError {
    /// Parent success or storage could not be confirmed.
    #[error("planning completion could not be confirmed")]
    Unconfirmed,
    /// Required approved output is missing, inconsistent, or invalid.
    #[error("planning artifacts or generated graph could not be validated")]
    Invalid,
    /// Recorded usage exhausted the operation allowance.
    #[error("planning exhausted the shared operation budget")]
    Exhausted,
    /// Usage evidence is insufficient to establish remaining allowance.
    #[error("remaining operation budget could not be confirmed")]
    BudgetUnconfirmed,
}

impl From<&HandoffRejection> for ContinuationError {
    fn from(rejection: &HandoffRejection) -> Self {
        if rejection.is_retryable() {
            Self::Unconfirmed
        } else {
            Self::Invalid
        }
    }
}

pub(crate) async fn prepare_child(
    storage: &Arc<Storage>,
    runtime: &BootstrapRuntime,
    parent: RunId,
    intent: &BootstrapIntent,
) -> Result<BootstrapContinuation, ContinuationError> {
    let RunDatabaseInspection::Present { events } = storage
        .inspect_run(parent)
        .await
        .map_err(|_| ContinuationError::Unconfirmed)?
        .database
    else {
        return Err(ContinuationError::Unconfirmed);
    };
    // The shared terminal verdict (#95): only a single `RunCompleted`
    // continues; a conflict or any other outcome is unconfirmed.
    let TerminalVerdict::Terminal(TerminalClaim {
        outcome: TerminalOutcome::Completed {
            terminal_node: terminal,
        },
        ..
    }) = ReadEvent::terminal_verdict(&events)
    else {
        return Err(ContinuationError::Unconfirmed);
    };
    if !crate::tracked_run::confirms(&events, &RunOutcome::Completed { terminal }) {
        return Err(ContinuationError::Unconfirmed);
    }
    let (profiles, agents) = (runtime.profiles(), runtime.agents());
    let context = HandoffContext {
        profiles: Some(&profiles),
        agents: Some(&agents),
    };
    let handoff = checked(runtime, prepare(storage, parent, &context).await)?;
    // Telemetry is an explicit write of this completion path, never a side
    // effect of reading the handoff.
    surge_orchestrator::bootstrap_driver::record_bootstrap_telemetry(
        storage,
        parent,
        &handoff.graph,
    )
    .await
    .map_err(|_| ContinuationError::Unconfirmed)?;
    let budget = remaining_budget(intent.budget(), &events)?;
    BootstrapContinuation::new(
        parent,
        BootstrapContentRef {
            name: "materialized:flow".into(),
            digest: handoff.identity,
        },
        handoff.source_refs(),
        budget,
    )
    .map_err(|_| ContinuationError::Invalid)
}

/// Reload the pinned continuation: the same handoff, the same graph identity
/// and the same inherited artifacts, or a refusal.
pub(crate) async fn load_child(
    storage: &Arc<Storage>,
    runtime: &BootstrapRuntime,
    child: &BootstrapContinuation,
) -> Result<Graph, ContinuationError> {
    let (profiles, agents) = (runtime.profiles(), runtime.agents());
    let context = HandoffContext {
        profiles: Some(&profiles),
        agents: Some(&agents),
    };
    let handoff = checked(
        runtime,
        reload(storage, child.parent(), &context, child.graph().digest).await,
    )?;
    let reloaded = handoff.source_refs();
    let pinned = child.artifacts();
    let same_inputs = reloaded.len() == pinned.len()
        && reloaded.iter().all(|artifact| {
            pinned
                .iter()
                .any(|pin| pin.name == artifact.name && pin.hash == artifact.hash)
        });
    if !same_inputs {
        return Err(ContinuationError::Invalid);
    }
    Ok(handoff.graph)
}

/// Map a handoff result into daemon terms, adding the daemon's credential
/// readiness check for the child's agents.
fn checked(
    runtime: &BootstrapRuntime,
    handoff: Result<BootstrapHandoff, HandoffRejection>,
) -> Result<BootstrapHandoff, ContinuationError> {
    let handoff = handoff.map_err(|rejection| {
        tracing::warn!(%rejection, "bootstrap continuation refused");
        ContinuationError::from(&rejection)
    })?;
    runtime
        .validate_graph(&handoff.graph)
        .map_err(|_| ContinuationError::Invalid)?;
    Ok(handoff)
}

fn remaining_budget(
    mut budget: BudgetGuard,
    events: &[ReadEvent],
) -> Result<BudgetGuard, ContinuationError> {
    let Some(limit) = budget.limits.tokens else {
        return Ok(budget);
    };
    let mut sessions = HashSet::new();
    let mut observed = HashSet::new();
    let mut unavailable = HashSet::new();
    let mut used = 0_u64;
    for event in events {
        match &event.payload.payload {
            EventPayload::SessionOpened { session, .. } => {
                sessions.insert(*session);
            },
            payload if payload.token_delta().is_some() => {
                if let Some((session, input, output, _)) = payload.token_delta() {
                    observed.insert(session);
                    used = used
                        .checked_add(input)
                        .and_then(|used| used.checked_add(output))
                        .ok_or(ContinuationError::BudgetUnconfirmed)?;
                }
            },
            EventPayload::ProviderUsageObserved {
                session,
                usage: surge_core::TokenUsageObservation::Unavailable { .. },
            } => {
                unavailable.insert(*session);
            },
            _ => {},
        }
    }
    if !sessions.is_subset(&observed) || !unavailable.is_empty() {
        return Err(ContinuationError::BudgetUnconfirmed);
    }
    let remaining = limit
        .checked_sub(used)
        .filter(|remaining| *remaining > 0)
        .ok_or(ContinuationError::Exhausted)?;
    budget.limits.tokens = Some(remaining);
    Ok(budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::{SessionId, VersionedEventPayload, keys::NodeKey};
    fn event(payload: EventPayload) -> ReadEvent {
        ReadEvent {
            seq: surge_persistence::runs::EventSeq(1),
            timestamp_ms: 0,
            kind: payload.discriminant_str().into(),
            payload: VersionedEventPayload::new(payload),
        }
    }

    #[test]
    fn provider_budget_ignores_context_and_rejects_unknown_spend() {
        use surge_core::{
            ContextUsage, TokenUsageCounters, TokenUsageObservation, UsageUnavailableReason,
        };
        let session = SessionId::new();
        let mut budget = BudgetGuard::default();
        budget.limits.tokens = Some(100);
        let counters = TokenUsageCounters {
            total_tokens: 20,
            input_tokens: 15,
            output_tokens: 5,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
        };
        let mut events = vec![
            event(EventPayload::ContextUsageObserved {
                session,
                usage: ContextUsage {
                    used: 10_000,
                    size: 20_000,
                    cost: None,
                },
            }),
            event(EventPayload::ProviderUsageObserved {
                session,
                usage: TokenUsageObservation::Reported {
                    cumulative: counters.clone(),
                    delta: counters,
                },
            }),
        ];
        assert_eq!(
            remaining_budget(budget, &events).unwrap().limits.tokens,
            Some(80)
        );
        events.insert(
            0,
            event(EventPayload::ProviderUsageObserved {
                session,
                usage: TokenUsageObservation::Unavailable {
                    reason: UsageUnavailableReason::Missing,
                    reported: None,
                },
            }),
        );
        assert!(matches!(
            remaining_budget(budget, &events),
            Err(ContinuationError::BudgetUnconfirmed)
        ));
    }

    #[test]
    fn shared_token_budget_subtracts_planning_and_never_turns_zero_into_unlimited() {
        let session = SessionId::new();
        let mut budget = BudgetGuard::default();
        budget.limits.tokens = Some(100);
        let mut events = vec![event(EventPayload::SessionOpened {
            handoff: None,
            opened: None,
            node: NodeKey::try_new("planner").unwrap(),
            session,
            agent: "planner".into(),
            agent_id: None,
        })];
        assert!(matches!(
            remaining_budget(budget, &events),
            Err(ContinuationError::BudgetUnconfirmed)
        ));
        events.push(event(EventPayload::TokensConsumed {
            session,
            prompt_tokens: 20,
            output_tokens: 30,
            cache_hits: 0,
            model: "fixture".into(),
            cost_usd: None,
        }));
        assert_eq!(
            remaining_budget(budget, &events).unwrap().limits.tokens,
            Some(50)
        );
        events.push(event(EventPayload::TokensConsumed {
            session,
            prompt_tokens: 10,
            output_tokens: 40,
            cache_hits: 0,
            model: "fixture".into(),
            cost_usd: None,
        }));
        assert!(matches!(
            remaining_budget(budget, &events),
            Err(ContinuationError::Exhausted)
        ));
    }
}
