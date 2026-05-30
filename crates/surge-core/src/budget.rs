//! Budget evaluation for run cost/token enforcement.
//!
//! Pure, deterministic comparison of accrued [`CostSummary`] against resolved
//! [`BudgetLimits`]. The engine evaluates this at stage boundaries — where the
//! run's cumulative cost has already been folded into run memory — and acts on
//! the verdict (warn → notify, exceeded → escalate/abort per policy).
//!
//! No I/O, no wall-clock: the same `(limits, costs)` always yields the same
//! [`BudgetVerdict`], so budget decisions replay identically.

use crate::run_state::CostSummary;

/// Which budget dimension a verdict refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetDimension {
    /// US-dollar spend.
    Usd,
    /// Total tokens (prompt + output).
    Tokens,
}

/// Resolved spend limits for a run.
///
/// `None` on a dimension means "no limit". A non-positive `usd` or zero
/// `tokens` limit is treated as unset (degenerate, never triggers). Override
/// resolution (global → run → milestone) happens upstream; this type holds the
/// already-resolved effective limits.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BudgetLimits {
    /// Hard USD ceiling. `None` = unlimited.
    pub usd: Option<f64>,
    /// Hard token ceiling (prompt + output). `None` = unlimited.
    pub tokens: Option<u64>,
    /// Warn once accrued spend reaches this percentage (1–100) of a limit.
    /// `0` disables the warn band (only `Exceeded` can fire).
    pub warn_threshold_pct: u8,
}

/// Result of comparing accrued cost against limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BudgetVerdict {
    /// Within budget, or no effective limits set.
    Ok,
    /// Crossed the warn threshold on a dimension but not yet exceeded.
    Warn {
        /// Dimension that triggered the warning.
        dimension: BudgetDimension,
        /// Percentage of the limit reached, clamped to `1..=100`.
        pct: u8,
    },
    /// Reached or passed a limit on a dimension.
    Exceeded {
        /// Dimension that was exceeded.
        dimension: BudgetDimension,
    },
}

impl BudgetLimits {
    /// Whether no effective limit is set (both dimensions unset/degenerate).
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        !(matches!(self.usd, Some(u) if u > 0.0) || matches!(self.tokens, Some(t) if t > 0))
    }

    /// Evaluate accrued `costs` against these limits.
    ///
    /// `Exceeded` takes priority over `Warn`; when both dimensions sit in the
    /// same band, USD is reported first. Deterministic — no wall-clock, no I/O.
    #[must_use]
    pub fn evaluate(&self, costs: &CostSummary) -> BudgetVerdict {
        let total_tokens = costs.tokens_in.saturating_add(costs.tokens_out);

        // Exceeded — USD first, then tokens.
        if let Some(limit) = self.usd.filter(|u| *u > 0.0)
            && costs.cost_usd >= limit
        {
            return BudgetVerdict::Exceeded {
                dimension: BudgetDimension::Usd,
            };
        }
        if let Some(limit) = self.tokens.filter(|t| *t > 0)
            && total_tokens >= limit
        {
            return BudgetVerdict::Exceeded {
                dimension: BudgetDimension::Tokens,
            };
        }

        // Warn — only when a warn band is configured.
        let warn = u64::from(self.warn_threshold_pct.min(100));
        if warn == 0 {
            return BudgetVerdict::Ok;
        }
        if let Some(limit) = self.usd.filter(|u| *u > 0.0) {
            let pct = usd_pct(costs.cost_usd, limit);
            if u64::from(pct) >= warn {
                return BudgetVerdict::Warn {
                    dimension: BudgetDimension::Usd,
                    pct,
                };
            }
        }
        if let Some(limit) = self.tokens.filter(|t| *t > 0) {
            // Integer math: floor(total * 100 / limit), saturating, capped 100.
            let pct = total_tokens.saturating_mul(100) / limit;
            if pct >= warn {
                return BudgetVerdict::Warn {
                    dimension: BudgetDimension::Tokens,
                    pct: u8::try_from(pct.min(100)).unwrap_or(100),
                };
            }
        }
        BudgetVerdict::Ok
    }
}

/// Floored integer percentage of `spend` against a positive `limit`, capped at
/// 100. Caller guarantees `limit > 0`; both inputs are finite.
fn usd_pct(spend: f64, limit: f64) -> u8 {
    let pct = (spend / limit * 100.0).floor().clamp(0.0, 100.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "pct is floored and clamped to [0, 100], so the u8 cast is exact"
    )]
    let out = pct as u8;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn costs(cost_usd: f64, tokens_in: u64, tokens_out: u64) -> CostSummary {
        CostSummary {
            tokens_in,
            tokens_out,
            cache_hits: 0,
            cost_usd,
        }
    }

    #[test]
    fn unlimited_when_no_effective_limits() {
        let limits = BudgetLimits::default();
        assert!(limits.is_unlimited());
        assert_eq!(limits.evaluate(&costs(999.0, 1_000_000, 0)), BudgetVerdict::Ok);
    }

    #[test]
    fn non_positive_limits_are_unset() {
        let limits = BudgetLimits {
            usd: Some(0.0),
            tokens: Some(0),
            warn_threshold_pct: 80,
        };
        assert!(limits.is_unlimited());
        assert_eq!(limits.evaluate(&costs(5.0, 100, 100)), BudgetVerdict::Ok);
    }

    #[test]
    fn under_threshold_is_ok() {
        let limits = BudgetLimits {
            usd: Some(10.0),
            tokens: None,
            warn_threshold_pct: 80,
        };
        assert_eq!(limits.evaluate(&costs(5.0, 0, 0)), BudgetVerdict::Ok);
    }

    #[test]
    fn usd_warn_at_threshold() {
        let limits = BudgetLimits {
            usd: Some(10.0),
            tokens: None,
            warn_threshold_pct: 80,
        };
        assert_eq!(
            limits.evaluate(&costs(8.0, 0, 0)),
            BudgetVerdict::Warn {
                dimension: BudgetDimension::Usd,
                pct: 80,
            }
        );
    }

    #[test]
    fn usd_exceeded_at_limit() {
        let limits = BudgetLimits {
            usd: Some(10.0),
            tokens: None,
            warn_threshold_pct: 80,
        };
        assert_eq!(
            limits.evaluate(&costs(10.0, 0, 0)),
            BudgetVerdict::Exceeded {
                dimension: BudgetDimension::Usd,
            }
        );
        assert_eq!(
            limits.evaluate(&costs(12.5, 0, 0)),
            BudgetVerdict::Exceeded {
                dimension: BudgetDimension::Usd,
            }
        );
    }

    #[test]
    fn tokens_warn_and_exceed() {
        let limits = BudgetLimits {
            usd: None,
            tokens: Some(1_000),
            warn_threshold_pct: 90,
        };
        assert_eq!(limits.evaluate(&costs(0.0, 400, 400)), BudgetVerdict::Ok);
        assert_eq!(
            limits.evaluate(&costs(0.0, 500, 450)),
            BudgetVerdict::Warn {
                dimension: BudgetDimension::Tokens,
                pct: 95,
            }
        );
        assert_eq!(
            limits.evaluate(&costs(0.0, 600, 400)),
            BudgetVerdict::Exceeded {
                dimension: BudgetDimension::Tokens,
            }
        );
    }

    #[test]
    fn exceeded_beats_warn_and_usd_reported_first() {
        let limits = BudgetLimits {
            usd: Some(10.0),
            tokens: Some(1_000),
            warn_threshold_pct: 50,
        };
        // USD exceeded, tokens only warn → Exceeded(Usd).
        assert_eq!(
            limits.evaluate(&costs(10.0, 600, 0)),
            BudgetVerdict::Exceeded {
                dimension: BudgetDimension::Usd,
            }
        );
    }

    #[test]
    fn warn_threshold_zero_disables_warn_band() {
        let limits = BudgetLimits {
            usd: Some(10.0),
            tokens: None,
            warn_threshold_pct: 0,
        };
        // 99% spend, no warn band → Ok until exceeded.
        assert_eq!(limits.evaluate(&costs(9.9, 0, 0)), BudgetVerdict::Ok);
        assert_eq!(
            limits.evaluate(&costs(10.0, 0, 0)),
            BudgetVerdict::Exceeded {
                dimension: BudgetDimension::Usd,
            }
        );
    }
}
