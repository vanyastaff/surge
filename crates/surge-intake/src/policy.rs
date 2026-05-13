//! Tracker automation policy resolver.
//!
//! Maps a ticket's labels to an [`AutomationPolicy`] that determines how the
//! intake pipeline handles the ticket (tier L0 / L1 / L2 / L3).
//!
//! See [ROADMAP § "Tracker automation tiers"](../../../ROADMAP.md) and
//! [ADR 0014](../../../../docs/adr/0014-tracker-automation-tiers.md) for the
//! semantic model. This module is the single source of truth for tier
//! precedence; downstream code never re-implements label parsing.
//!
//! ### Precedence
//!
//! When multiple `surge:*` labels are present, the most restrictive policy wins:
//!
//! 1. `surge:disabled` → [`AutomationPolicy::Disabled`] (L0; ignore entirely).
//! 2. `surge:auto` → [`AutomationPolicy::Auto`] (L3; full automation, merge on green).
//! 3. `surge:template/<name>` → [`AutomationPolicy::Template`] (L2; skip bootstrap).
//! 4. `surge:enabled` → [`AutomationPolicy::Standard`] (L1; bootstrap + approval).
//! 5. No `surge:*` label → [`AutomationPolicy::Disabled`] (L0; tracker ignores).
//!
//! The function is total and deterministic — every label set maps to exactly
//! one variant and the same input always returns the same output.

use serde::{Deserialize, Serialize};

/// Discriminator written into `ticket_index.triage_decision` when the L0
/// short-circuit skips a ticket without paying the triage-author LLM cost.
///
/// Kept here as a `pub const` so the daemon and the CLI render-side share the
/// exact same string.
pub const TRIAGE_DECISION_L0: &str = "L0Skipped";

/// Discriminator used when an externally-closed ticket transitions to
/// `Skipped` via the router's external-state-change reflection path.
///
/// Lives alongside [`TRIAGE_DECISION_L0`] so all triage-decision strings are
/// declared in one place.
pub const TRIAGE_DECISION_EXTERNALLY_CLOSED: &str = "ExternallyClosed";

/// Label literals. Public so tests, the CLI, and the docs renderer can refer
/// to the canonical spelling without re-typing strings.
pub mod labels {
    /// Disables tracker automation for a ticket (L0).
    pub const DISABLED: &str = "surge:disabled";
    /// Enables L1 (full bootstrap + approval) — the default explicit opt-in.
    pub const ENABLED: &str = "surge:enabled";
    /// Enables L3 (full automation including merge-on-green).
    pub const AUTO: &str = "surge:auto";
    /// Prefix for L2 template labels. The actual label is
    /// `surge:template/<name>` where `<name>` is the bundled or user template.
    pub const TEMPLATE_PREFIX: &str = "surge:template/";
}

/// Resolved automation policy for a single ticket.
///
/// The variant determines what the intake pipeline does after triage:
/// - [`AutomationPolicy::Disabled`] — short-circuit before triage (no LLM cost).
/// - [`AutomationPolicy::Standard`] — full bootstrap + inbox card + approval.
/// - [`AutomationPolicy::Template`] — skip bootstrap; use named template directly.
/// - [`AutomationPolicy::Auto`] — full automation, optional auto-merge on green.
///
/// Marked `#[non_exhaustive]` so new tiers can be added without a workspace-wide
/// match-arm churn.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "tier")]
#[non_exhaustive]
pub enum AutomationPolicy {
    /// L0 — tracker ignores ticket entirely. Either `surge:disabled` is set,
    /// or no `surge:*` label is present (opt-in model).
    Disabled,
    /// L1 — `surge:enabled`. Full bootstrap (Description Author → Roadmap
    /// Planner → Flow Generator) followed by an inbox card for human approval.
    Standard,
    /// L2 — `surge:template/<name>`. Skip bootstrap; resolve `<name>` against
    /// the archetype registry and start the run directly.
    Template {
        /// The template name extracted from the `surge:template/<name>` label.
        /// Resolved against `ArchetypeRegistry` at run-launch time.
        name: String,
    },
    /// L3 — `surge:auto`. Identical to [`AutomationPolicy::Standard`] for the
    /// bootstrap leg (operator still sees the card for visibility) but every
    /// `HumanGate` auto-approves and the daemon attempts a merge on
    /// `Completed` outcome when `merge_when_clean` is `true`.
    Auto {
        /// When `true`, the `AutomationMergeGate` posts a merge action on the
        /// tracker if the resulting PR satisfies the green-checks + approved
        /// review policy. When `false`, automation runs but stops short of
        /// merging — operator merges manually.
        merge_when_clean: bool,
    },
}

impl AutomationPolicy {
    /// Stable short string for telemetry, comments, and the `surge intake list`
    /// renderer. The values are: `"L0"`, `"L1"`, `"L2"`, `"L3"`.
    #[must_use]
    pub fn tier_code(&self) -> &'static str {
        match self {
            Self::Disabled => "L0",
            Self::Standard => "L1",
            Self::Template { .. } => "L2",
            Self::Auto { .. } => "L3",
        }
    }

    /// `true` for [`AutomationPolicy::Disabled`]. Convenience for the L0
    /// short-circuit in `handle_triage_event`.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }

    /// Template name when this policy is `Template`, otherwise `None`.
    #[must_use]
    pub fn template_name(&self) -> Option<&str> {
        match self {
            Self::Template { name } => Some(name.as_str()),
            _ => None,
        }
    }
}

/// Resolve the automation policy from a ticket's label set.
///
/// Precedence (most restrictive wins):
/// 1. `surge:disabled` → [`AutomationPolicy::Disabled`].
/// 2. `surge:auto` → [`AutomationPolicy::Auto { merge_when_clean: true }`].
/// 3. `surge:template/<name>` → [`AutomationPolicy::Template { name }`] (first
///    match wins when multiple template labels are present — a tracker
///    misconfiguration which we log at WARN at the call site).
/// 4. `surge:enabled` → [`AutomationPolicy::Standard`].
/// 5. None of the above → [`AutomationPolicy::Disabled`] (opt-in model).
///
/// Empty template name (`"surge:template/"`) is treated as no template — falls
/// through to the next precedence step. This is defensive: a tracker could
/// truncate the suffix.
///
/// The function is pure (no I/O), total, and deterministic.
#[must_use]
pub fn resolve_policy(labels: &[String]) -> AutomationPolicy {
    tracing::debug!(
        target: "intake::policy",
        label_count = labels.len(),
        "resolve_policy: scanning labels"
    );

    if labels.iter().any(|l| l == labels::DISABLED) {
        tracing::info!(target: "intake::policy", tier = "L0", reason = "surge:disabled", "policy resolved");
        return AutomationPolicy::Disabled;
    }

    if labels.iter().any(|l| l == labels::AUTO) {
        tracing::info!(target: "intake::policy", tier = "L3", reason = "surge:auto", "policy resolved");
        return AutomationPolicy::Auto {
            merge_when_clean: true,
        };
    }

    if let Some(name) = labels.iter().find_map(|l| {
        l.strip_prefix(labels::TEMPLATE_PREFIX)
            .filter(|n| !n.is_empty())
            .map(ToOwned::to_owned)
    }) {
        tracing::info!(
            target: "intake::policy",
            tier = "L2",
            template = %name,
            "policy resolved"
        );
        return AutomationPolicy::Template { name };
    }

    if labels.iter().any(|l| l == labels::ENABLED) {
        tracing::info!(target: "intake::policy", tier = "L1", reason = "surge:enabled", "policy resolved");
        return AutomationPolicy::Standard;
    }

    tracing::info!(target: "intake::policy", tier = "L0", reason = "no surge:* label", "policy resolved");
    AutomationPolicy::Disabled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lbl(s: &str) -> String {
        s.to_owned()
    }

    #[test]
    fn empty_labels_resolve_to_disabled() {
        assert_eq!(resolve_policy(&[]), AutomationPolicy::Disabled);
    }

    #[test]
    fn unrelated_labels_resolve_to_disabled() {
        let labels = [lbl("bug"), lbl("priority:high"), lbl("kind/feature")];
        assert_eq!(resolve_policy(&labels), AutomationPolicy::Disabled);
    }

    #[test]
    fn enabled_resolves_to_standard() {
        assert_eq!(
            resolve_policy(&[lbl(labels::ENABLED)]),
            AutomationPolicy::Standard
        );
    }

    #[test]
    fn auto_resolves_to_l3_with_merge() {
        assert_eq!(
            resolve_policy(&[lbl(labels::AUTO)]),
            AutomationPolicy::Auto {
                merge_when_clean: true
            }
        );
    }

    #[test]
    fn template_extracts_name() {
        let labels_in = [lbl("surge:template/rust-crate")];
        assert_eq!(
            resolve_policy(&labels_in),
            AutomationPolicy::Template {
                name: "rust-crate".to_owned()
            }
        );
    }

    #[test]
    fn empty_template_suffix_is_ignored() {
        let labels_in = [lbl("surge:template/"), lbl(labels::ENABLED)];
        assert_eq!(resolve_policy(&labels_in), AutomationPolicy::Standard);
    }

    #[test]
    fn disabled_wins_over_everything() {
        let labels_in = [
            lbl(labels::AUTO),
            lbl(labels::ENABLED),
            lbl("surge:template/x"),
            lbl(labels::DISABLED),
        ];
        assert_eq!(resolve_policy(&labels_in), AutomationPolicy::Disabled);
    }

    #[test]
    fn auto_wins_over_template_and_enabled() {
        let labels_in = [
            lbl(labels::ENABLED),
            lbl("surge:template/x"),
            lbl(labels::AUTO),
        ];
        assert_eq!(
            resolve_policy(&labels_in),
            AutomationPolicy::Auto {
                merge_when_clean: true
            }
        );
    }

    #[test]
    fn template_wins_over_enabled() {
        let labels_in = [lbl(labels::ENABLED), lbl("surge:template/web")];
        assert_eq!(
            resolve_policy(&labels_in),
            AutomationPolicy::Template {
                name: "web".to_owned()
            }
        );
    }

    #[test]
    fn first_template_label_wins_when_multiple() {
        let labels_in = [lbl("surge:template/alpha"), lbl("surge:template/beta")];
        match resolve_policy(&labels_in) {
            AutomationPolicy::Template { name } => assert_eq!(name, "alpha"),
            other => panic!("expected Template, got {other:?}"),
        }
    }

    #[test]
    fn tier_code_matches_variant() {
        assert_eq!(AutomationPolicy::Disabled.tier_code(), "L0");
        assert_eq!(AutomationPolicy::Standard.tier_code(), "L1");
        assert_eq!(
            AutomationPolicy::Template {
                name: "x".to_owned()
            }
            .tier_code(),
            "L2"
        );
        assert_eq!(
            AutomationPolicy::Auto {
                merge_when_clean: false
            }
            .tier_code(),
            "L3"
        );
    }

    #[test]
    fn helpers_consistent_with_variant() {
        assert!(AutomationPolicy::Disabled.is_disabled());
        assert!(!AutomationPolicy::Standard.is_disabled());
        assert_eq!(
            AutomationPolicy::Template {
                name: "x".to_owned()
            }
            .template_name(),
            Some("x")
        );
        assert_eq!(AutomationPolicy::Standard.template_name(), None);
    }

    #[test]
    fn triage_decision_constants_are_stable() {
        assert_eq!(TRIAGE_DECISION_L0, "L0Skipped");
        assert_eq!(TRIAGE_DECISION_EXTERNALLY_CLOSED, "ExternallyClosed");
    }

    #[test]
    fn serde_round_trip_each_variant() {
        let cases = [
            AutomationPolicy::Disabled,
            AutomationPolicy::Standard,
            AutomationPolicy::Template {
                name: "rust-crate".to_owned(),
            },
            AutomationPolicy::Auto {
                merge_when_clean: true,
            },
            AutomationPolicy::Auto {
                merge_when_clean: false,
            },
        ];
        for original in cases {
            let json = serde_json::to_string(&original).expect("serialize");
            let parsed: AutomationPolicy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, original, "round-trip failed for {json}");
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    fn label_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(labels::DISABLED.to_owned()),
            Just(labels::ENABLED.to_owned()),
            Just(labels::AUTO.to_owned()),
            "[a-z]{1,8}".prop_map(|n| format!("{}{n}", labels::TEMPLATE_PREFIX)),
            "[a-z0-9:_/-]{1,16}",
        ]
    }

    proptest! {
        #[test]
        fn precedence_is_deterministic(labels in vec(label_strategy(), 0..12)) {
            let first = resolve_policy(&labels);
            let second = resolve_policy(&labels);
            prop_assert_eq!(first, second);
        }

        #[test]
        fn disabled_dominates(other in vec(label_strategy(), 0..12)) {
            let mut labels = other.clone();
            labels.push(labels::DISABLED.to_owned());
            prop_assert_eq!(resolve_policy(&labels), AutomationPolicy::Disabled);
        }

        #[test]
        fn auto_dominates_template_and_enabled(extra in vec("[a-z]{1,6}", 0..6)) {
            let mut labels = vec![labels::AUTO.to_owned()];
            for n in &extra {
                labels.push(format!("{}{n}", labels::TEMPLATE_PREFIX));
                labels.push(labels::ENABLED.to_owned());
            }
            prop_assert_eq!(
                resolve_policy(&labels),
                AutomationPolicy::Auto { merge_when_clean: true }
            );
        }

        #[test]
        fn tier_code_only_one_of_four(labels in vec(label_strategy(), 0..12)) {
            let code = resolve_policy(&labels).tier_code();
            prop_assert!(matches!(code, "L0" | "L1" | "L2" | "L3"));
        }

        #[test]
        fn enabled_without_higher_resolves_standard(extra in vec("[a-z0-9]{1,6}", 0..6)) {
            let mut labels = vec![labels::ENABLED.to_owned()];
            for n in extra {
                labels.push(format!("noise:{n}"));
            }
            prop_assert_eq!(resolve_policy(&labels), AutomationPolicy::Standard);
        }
    }
}
