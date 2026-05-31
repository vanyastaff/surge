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
use serde::{Deserialize, Serialize};

/// Which budget dimension a verdict refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetLimits {
    /// Hard USD ceiling. `None` = unlimited.
    #[serde(default)]
    pub usd: Option<f64>,
    /// Hard token ceiling (prompt + output). `None` = unlimited.
    #[serde(default)]
    pub tokens: Option<u64>,
    /// Warn once accrued spend reaches this percentage (1–100) of a limit.
    /// `0` disables the warn band (only `Exceeded` can fire).
    #[serde(default)]
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

/// What the engine does when a run reaches a budget limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPolicy {
    /// Stop the run on the first breach (default — the safe AFK posture: an
    /// unattended run never overruns its budget).
    #[default]
    Abort,
    /// Record warn/exceed events but never stop the run. For operators who
    /// only want visibility, not enforcement.
    WarnOnly,
}

/// Per-run budget configuration: the resolved limits plus the breach policy.
/// Frozen at run start and carried on [`crate::run_event::RunConfig`]-adjacent
/// engine config so enforcement is deterministic and replayable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetGuard {
    /// Effective spend limits.
    #[serde(default)]
    pub limits: BudgetLimits,
    /// Action on breach.
    #[serde(default)]
    pub policy: BudgetPolicy,
}

impl BudgetGuard {
    /// Whether this guard enforces nothing (no effective limit).
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.limits.is_unlimited()
    }
}

/// The concrete action the engine takes at a stage boundary after evaluating
/// the budget — the IO-free decision separated from event emission / abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetAction {
    /// Nothing to do — proceed to the next stage.
    Continue,
    /// Emit a one-time `BudgetWarningRaised` and proceed.
    Warn {
        /// Dimension that triggered the warning.
        dimension: BudgetDimension,
        /// Percentage of the limit reached (1..=100).
        pct: u8,
    },
    /// Emit `BudgetExceeded` and abort the run.
    Abort {
        /// Dimension that was exceeded.
        dimension: BudgetDimension,
    },
}

/// Decide the engine action from a verdict, the policy, and whether a warning
/// has already been raised this run. Pure and deterministic.
///
/// - `Ok` → `Continue`.
/// - `Warn` → `Warn` once, then `Continue` (idempotent via `already_warned`).
/// - `Exceeded` under `Abort` → `Abort`; under `WarnOnly` → a one-time `Warn`
///   at 100% (visibility without stopping), then `Continue`.
#[must_use]
pub fn decide(verdict: BudgetVerdict, policy: BudgetPolicy, already_warned: bool) -> BudgetAction {
    match verdict {
        BudgetVerdict::Ok => BudgetAction::Continue,
        BudgetVerdict::Warn { dimension, pct } => {
            if already_warned {
                BudgetAction::Continue
            } else {
                BudgetAction::Warn { dimension, pct }
            }
        },
        BudgetVerdict::Exceeded { dimension } => match policy {
            BudgetPolicy::Abort => BudgetAction::Abort { dimension },
            BudgetPolicy::WarnOnly => {
                if already_warned {
                    BudgetAction::Continue
                } else {
                    BudgetAction::Warn {
                        dimension,
                        pct: 100,
                    }
                }
            },
        },
    }
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
        assert_eq!(
            limits.evaluate(&costs(999.0, 1_000_000, 0)),
            BudgetVerdict::Ok
        );
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

    #[test]
    fn decide_ok_continues() {
        assert_eq!(
            decide(BudgetVerdict::Ok, BudgetPolicy::Abort, false),
            BudgetAction::Continue
        );
    }

    #[test]
    fn decide_warn_fires_once_then_continues() {
        let verdict = BudgetVerdict::Warn {
            dimension: BudgetDimension::Usd,
            pct: 85,
        };
        assert_eq!(
            decide(verdict, BudgetPolicy::Abort, false),
            BudgetAction::Warn {
                dimension: BudgetDimension::Usd,
                pct: 85,
            }
        );
        // Already warned → no repeat.
        assert_eq!(
            decide(verdict, BudgetPolicy::Abort, true),
            BudgetAction::Continue
        );
    }

    #[test]
    fn decide_exceeded_aborts_under_abort_policy() {
        assert_eq!(
            decide(
                BudgetVerdict::Exceeded {
                    dimension: BudgetDimension::Tokens,
                },
                BudgetPolicy::Abort,
                false,
            ),
            BudgetAction::Abort {
                dimension: BudgetDimension::Tokens,
            }
        );
    }

    #[test]
    fn decide_exceeded_warns_once_under_warn_only() {
        let verdict = BudgetVerdict::Exceeded {
            dimension: BudgetDimension::Usd,
        };
        assert_eq!(
            decide(verdict, BudgetPolicy::WarnOnly, false),
            BudgetAction::Warn {
                dimension: BudgetDimension::Usd,
                pct: 100,
            }
        );
        assert_eq!(
            decide(verdict, BudgetPolicy::WarnOnly, true),
            BudgetAction::Continue
        );
    }

    #[test]
    fn guard_default_is_unlimited_abort() {
        let g = BudgetGuard::default();
        assert!(g.is_unlimited());
        assert_eq!(g.policy, BudgetPolicy::Abort);
    }
}
