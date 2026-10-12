//! ACP session-cumulative response counters; context occupancy is never spend.
use agent_client_protocol::schema::v1::{Usage, UsageUpdate};
use surge_core::usage::{
    ContextUsage, SessionCost, TokenUsageCounters, TokenUsageObservation, UsageUnavailableReason,
};

/// Missing or rejected reports retain the last accepted baseline. The next
/// accepted delta spans the entire interval since that baseline, not just a turn.
/// Optional counters retain independent high-water baselines through absence or
/// regression; recovery deltas span their last accepted known observation.
#[derive(Debug, Default)]
pub(crate) struct UsageTracker {
    baseline: Option<TokenUsageCounters>,
    unknown_initial_baseline: bool,
}

impl UsageTracker {
    pub(crate) fn restored() -> Self {
        Self {
            baseline: None,
            unknown_initial_baseline: true,
        }
    }

    pub(crate) fn observe(&mut self, usage: Option<&Usage>) -> TokenUsageObservation {
        let Some(usage) = usage else {
            return TokenUsageObservation::Unavailable {
                reason: UsageUnavailableReason::Missing,
                reported: None,
            };
        };
        let cumulative = TokenUsageCounters {
            total_tokens: usage.total_tokens,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            thought_tokens: usage.thought_tokens,
            cached_read_tokens: usage.cached_read_tokens,
            cached_write_tokens: usage.cached_write_tokens,
        };
        if self.unknown_initial_baseline {
            self.unknown_initial_baseline = false;
            self.baseline = Some(cumulative.clone());
            return TokenUsageObservation::Unavailable {
                reason: UsageUnavailableReason::BaselineUnknown,
                reported: Some(cumulative),
            };
        }
        let delta = if let Some(previous) = &self.baseline {
            let (Some(total), Some(input), Some(output)) = (
                cumulative.total_tokens.checked_sub(previous.total_tokens),
                cumulative.input_tokens.checked_sub(previous.input_tokens),
                cumulative.output_tokens.checked_sub(previous.output_tokens),
            ) else {
                tracing::warn!(
                    "ACP cumulative token counters regressed; retaining accepted baseline"
                );
                return TokenUsageObservation::Unavailable {
                    reason: UsageUnavailableReason::CounterRegression,
                    reported: Some(cumulative),
                };
            };
            TokenUsageCounters {
                total_tokens: total,
                input_tokens: input,
                output_tokens: output,
                thought_tokens: optional_delta(cumulative.thought_tokens, previous.thought_tokens),
                cached_read_tokens: optional_delta(
                    cumulative.cached_read_tokens,
                    previous.cached_read_tokens,
                ),
                cached_write_tokens: optional_delta(
                    cumulative.cached_write_tokens,
                    previous.cached_write_tokens,
                ),
            }
        } else {
            cumulative.clone()
        };
        let mut baseline = cumulative.clone();
        if let Some(previous) = &self.baseline {
            baseline.thought_tokens =
                retained_optional(cumulative.thought_tokens, previous.thought_tokens);
            baseline.cached_read_tokens =
                retained_optional(cumulative.cached_read_tokens, previous.cached_read_tokens);
            baseline.cached_write_tokens =
                retained_optional(cumulative.cached_write_tokens, previous.cached_write_tokens);
        }
        self.baseline = Some(baseline);
        TokenUsageObservation::Reported { cumulative, delta }
    }
}

fn retained_optional(current: Option<u64>, previous: Option<u64>) -> Option<u64> {
    match (current, previous) {
        (Some(current), Some(previous)) => Some(current.max(previous)),
        (current, previous) => current.or(previous),
    }
}

fn optional_delta(current: Option<u64>, previous: Option<u64>) -> Option<u64> {
    current?.checked_sub(previous?)
}

pub(crate) fn context_usage(update: UsageUpdate) -> ContextUsage {
    ContextUsage {
        used: update.used,
        size: update.size,
        cost: update.cost.and_then(|cost| {
            if cost.amount.is_finite() && cost.amount >= 0.0 {
                Some(SessionCost {
                    amount: cost.amount,
                    currency: cost.currency,
                })
            } else {
                tracing::warn!("ACP session cost invalid; leaving cost unknown");
                None
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::Usage;
    use surge_core::usage::{TokenUsageObservation, UsageUnavailableReason};

    #[test]
    fn reports_cumulative_deltas_without_resetting_on_missing_or_regression() {
        let mut tracker = UsageTracker::default();
        let first = Usage::new(5_000_000_000, 4_000_000_000, 1_000_000_000);
        assert!(
            matches!(tracker.observe(Some(&first)), TokenUsageObservation::Reported { cumulative, delta } if cumulative == delta)
        );
        assert!(matches!(
            tracker.observe(None),
            TokenUsageObservation::Unavailable {
                reason: UsageUnavailableReason::Missing,
                ..
            }
        ));
        assert!(matches!(
            tracker.observe(Some(&Usage::new(1, 1, 0))),
            TokenUsageObservation::Unavailable {
                reason: UsageUnavailableReason::CounterRegression,
                ..
            }
        ));
        let next = Usage::new(5_000_000_030, 4_000_000_020, 1_000_000_010).cached_read_tokens(50);
        assert!(
            matches!(tracker.observe(Some(&next)), TokenUsageObservation::Reported { delta, .. } if delta.total_tokens == 30 && delta.input_tokens == 20 && delta.output_tokens == 10 && delta.cached_read_tokens.is_none())
        );
        assert!(
            matches!(tracker.observe(Some(&next)), TokenUsageObservation::Reported { delta, .. } if delta.total_tokens == 0 && delta.cached_read_tokens == Some(0))
        );
    }
    #[test]
    fn optional_counters_remain_unknown_when_missing_new_or_regressed() {
        let mut tracker = UsageTracker::default();
        tracker.observe(Some(&Usage::new(10, 8, 2).thought_tokens(4)));
        assert!(
            matches!(tracker.observe(Some(&Usage::new(20, 16, 4))), TokenUsageObservation::Reported { delta, .. } if delta.thought_tokens.is_none())
        );
        assert!(
            matches!(tracker.observe(Some(&Usage::new(30, 24, 6).thought_tokens(8))), TokenUsageObservation::Reported { delta, .. } if delta.thought_tokens == Some(4))
        );
        assert!(
            matches!(tracker.observe(Some(&Usage::new(40, 32, 8).thought_tokens(2))), TokenUsageObservation::Reported { delta, .. } if delta.thought_tokens.is_none())
        );
    }

    #[test]
    fn context_cost_preserves_currency_and_rejects_invalid_amounts() {
        use agent_client_protocol::schema::v1::{Cost, UsageUpdate};
        for invalid in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(
                context_usage(UsageUpdate::new(42, 100).cost(Cost::new(invalid, "EUR")))
                    .cost
                    .is_none()
            );
        }
        let usage = context_usage(UsageUpdate::new(42, 100).cost(Cost::new(0.0, "EUR")));
        assert_eq!(usage.cost.unwrap().currency, "EUR");
    }
    #[test]
    fn restored_provider_history_is_not_charged_as_new_spend() {
        let mut tracker = UsageTracker::restored();
        assert!(matches!(
            tracker.observe(None),
            TokenUsageObservation::Unavailable {
                reason: UsageUnavailableReason::Missing,
                ..
            }
        ));
        assert!(
            matches!(tracker.observe(Some(&Usage::new(1000, 800, 200))), TokenUsageObservation::Unavailable { reason: UsageUnavailableReason::BaselineUnknown, reported: Some(raw) } if raw.total_tokens == 1000)
        );
        assert!(
            matches!(tracker.observe(Some(&Usage::new(1030, 820, 210))), TokenUsageObservation::Reported { delta, .. } if delta.total_tokens == 30 && delta.input_tokens == 20 && delta.output_tokens == 10)
        );
    }
    #[test]
    fn sdk_drops_malformed_usage_and_tracker_preserves_unknown() {
        use agent_client_protocol::schema::v1::PromptResponse;
        for usage in [
            serde_json::json!({"totalTokens": -1, "inputTokens": 8, "outputTokens": 2}),
            serde_json::json!({"totalTokens": 10, "inputTokens": "8", "outputTokens": 2}),
        ] {
            assert!(
                serde_json::from_value::<Usage>(usage.clone()).is_err(),
                "invalid Usage fixture: {usage}"
            );
            let raw = serde_json::json!({"stopReason":"end_turn", "usage":usage});
            let response =
                serde_json::from_value::<PromptResponse>(raw.clone()).unwrap_or_else(|error| {
                    panic!("optional usage must not reject response {raw}: {error}")
                });
            assert!(
                response.usage.is_none(),
                "malformed usage unexpectedly accepted: {raw}"
            );
            let observed = UsageTracker::default().observe(response.usage.as_ref());
            assert!(matches!(
                observed,
                TokenUsageObservation::Unavailable {
                    reason: UsageUnavailableReason::Missing,
                    reported: None
                }
            ));
        }
    }
    #[test]
    fn optional_regression_does_not_charge_old_tokens_on_recovery() {
        let mut tracker = UsageTracker::default();
        tracker.observe(Some(&Usage::new(100, 80, 20).cached_read_tokens(100)));
        for (total, cached, expected) in
            [(110, 50, None), (120, 100, Some(0)), (130, 110, Some(10))]
        {
            assert!(
                matches!(tracker.observe(Some(&Usage::new(total, total - 20, 20).cached_read_tokens(cached))), TokenUsageObservation::Reported { delta, .. } if delta.cached_read_tokens == expected)
            );
        }
    }
    #[test]
    fn sdk_keeps_required_spend_when_optional_counter_is_malformed() {
        use agent_client_protocol::schema::v1::PromptResponse;
        let raw = serde_json::json!({"stopReason":"end_turn", "usage": {
            "totalTokens":10, "inputTokens":8, "outputTokens":2,
            "thoughtTokens":"bad", "cachedReadTokens":"bad", "cachedWriteTokens":"bad"
        }});
        let response =
            serde_json::from_value::<PromptResponse>(raw.clone()).unwrap_or_else(|error| {
                panic!("optional counter errors must preserve response {raw}: {error}")
            });
        let usage = response.usage.as_ref().unwrap();
        assert_eq!(usage.total_tokens, 10);
        assert_eq!(usage.input_tokens, 8);
        assert_eq!(usage.output_tokens, 2);
        assert!(usage.thought_tokens.is_none());
        assert!(usage.cached_read_tokens.is_none());
        assert!(usage.cached_write_tokens.is_none());
        assert!(
            matches!(UsageTracker::default().observe(Some(usage)), TokenUsageObservation::Reported { delta, .. } if delta.total_tokens == 10 && delta.cached_read_tokens.is_none() && delta.cached_write_tokens.is_none() && delta.thought_tokens.is_none())
        );
    }
}
