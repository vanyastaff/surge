//! Context packs under a hard token budget, with a receipt.
//!
//! A node's context is not "whatever memory claims fit" in whatever order
//! they happen to arrive — it is a deliberate selection under a budget,
//! ordered so direct evidence is admitted ahead of low-trust recall
//! (`.autopilot/competitive-waves/spec.md` §23, Story 23–24; R20, R24–R26).
//! [`ContextPack::build`] is the one place that ordering and cut happen; it
//! is pure (no I/O, no wall-clock, no randomness), so the same
//! `(claims, budget)` always yields the same pack and the same receipt.
//!
//! The [`PackReceipt`] returned alongside the pack is not incidental — it is
//! the answer to "why didn't the agent know X", which is otherwise
//! unrecoverable once a run has finished (spec §8). This module only builds
//! the receipt; persisting it into the run event log and rendering it in
//! `surge run report` / replay are separate concerns owned elsewhere.

use crate::id::MemoryClaimId;
use crate::memory::MemoryClaim;
use serde::{Deserialize, Serialize};

// ── Budget ───────────────────────────────────────────────────────────

/// `surge.toml`-level budget for a memory-claims context pack. `surge.toml`
/// key: `[context_pack] budget_tokens`.
///
/// This is a knob with a conservative default, not a measured "correct"
/// size for any project — raise it once real packs show room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPackConfig {
    /// Hard ceiling on a pack's estimated token count.
    /// [`ContextPack::build`] never returns a pack whose selected claims'
    /// estimated cost exceeds this.
    #[serde(default = "default_budget_tokens")]
    pub budget_tokens: u32,
}

impl Default for ContextPackConfig {
    fn default() -> Self {
        Self {
            budget_tokens: default_budget_tokens(),
        }
    }
}

/// Conservative default: 2,000 tokens (roughly 8 KB of English text at this
/// module's ~4-chars-per-token estimate — see [`estimate_tokens`]).
/// Comfortably small next to a typical model context window.
fn default_budget_tokens() -> u32 {
    2_000
}

/// Rough, dependency-free token estimate: about 4 characters per token, the
/// commonly quoted rule of thumb for English text. There is no tokenizer
/// dependency in this tree (R46) and none is added for this — `build` only
/// needs a consistent, monotonic measure to compare candidates against a
/// budget, not a count that matches any particular model's real tokenizer.
fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

// ── Receipt ──────────────────────────────────────────────────────────

/// Why [`PackReceipt::dropped`] is non-empty. `None` on the receipt means
/// nothing was dropped — every candidate fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// One or more lower-confidence candidates were left out once the
    /// running token estimate would have exceeded the budget. Candidates
    /// are considered in `Confidence` order (most-trusted first), so a
    /// dropped claim is never higher-confidence than every selected one.
    OverBudget,
}

/// What [`ContextPack::build`] selected, what it left out, and why —
/// written for replay, so "why didn't the agent know X" is answerable
/// after the fact (spec §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackReceipt {
    /// Claims admitted into the pack, in the order they were considered
    /// (`Confidence` order, most-trusted first; ties keep the input's
    /// relative order).
    pub selected: Vec<MemoryClaimId>,
    /// Claims considered but left out because including them would have
    /// pushed the running token estimate over `budget`.
    pub dropped: Vec<MemoryClaimId>,
    /// Why anything in `dropped` is there — `None` when it's empty.
    pub reason: Option<DropReason>,
    /// The budget this pack was built against (estimated tokens).
    pub budget: u32,
    /// Estimated token count actually used by `selected`.
    pub used: u32,
}

// ── Pack ─────────────────────────────────────────────────────────────

/// A selection of memory claims that fits under a [`ContextPackConfig`]
/// budget, ordered by [`crate::memory::Confidence`] (most-trusted first).
/// Build one with [`ContextPack::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPack {
    claims: Vec<MemoryClaim>,
}

impl ContextPack {
    /// The selected claims, in the order they were admitted (`Confidence`
    /// order, most-trusted first).
    #[must_use]
    pub fn claims(&self) -> &[MemoryClaim] {
        &self.claims
    }

    /// True when nothing was selected — either `claims` was empty, or the
    /// budget could not admit even the single cheapest candidate.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    /// Number of claims selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.claims.len()
    }

    /// Select `claims` into a pack under `budget`, and return the receipt
    /// explaining the selection.
    ///
    /// Candidates are considered in [`crate::memory::Confidence`] order
    /// (`Verified` before `NameMatched` before `Asserted`, per that type's
    /// documented ranking) — direct evidence is ordered ahead of low-trust
    /// recall, never the reverse, even when a lower-confidence candidate is
    /// individually cheaper (spec §23; this is a contract of this module,
    /// not an implementation detail). Within a confidence level, candidates
    /// keep their relative order from `claims` (a stable sort).
    ///
    /// Each candidate in that order is admitted if doing so would not push
    /// the running estimated token cost over `budget.budget_tokens`;
    /// otherwise it is dropped and the (cheaper) candidates still to come
    /// are still considered — a low-confidence candidate is never admitted
    /// ahead of a higher-confidence one, but the budget's remainder is
    /// still put to use. The returned pack's estimated cost never exceeds
    /// the budget, for any input.
    ///
    /// Pure: no I/O, no wall-clock, no randomness.
    #[must_use]
    pub fn build(claims: Vec<MemoryClaim>, budget: ContextPackConfig) -> (Self, PackReceipt) {
        let mut ordered = claims;
        ordered.sort_by_key(MemoryClaim::confidence);

        let mut selected = Vec::with_capacity(ordered.len());
        let mut selected_ids = Vec::new();
        let mut dropped = Vec::new();
        let mut used: u32 = 0;

        for claim in ordered {
            let cost = estimate_tokens(claim.text());
            match used.checked_add(cost) {
                Some(next_used) if next_used <= budget.budget_tokens => {
                    used = next_used;
                    selected_ids.push(claim.id());
                    selected.push(claim);
                },
                _ => dropped.push(claim.id()),
            }
        }

        let reason = if dropped.is_empty() {
            None
        } else {
            Some(DropReason::OverBudget)
        };

        let receipt = PackReceipt {
            selected: selected_ids,
            dropped,
            reason,
            budget: budget.budget_tokens,
            used,
        };
        (Self { claims: selected }, receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_hash::ContentHash;
    use crate::memory::{ClaimStatus, Confidence, Provenance};

    /// Build a claim with an explicit confidence level, sidestepping
    /// [`MemoryClaim::from_transcript`]'s fixed `Asserted`/`Unverified`
    /// pair. Always `Unverified` here since none of these tests need a
    /// verified claim's stricter provenance — `MemoryClaim::new` only ties
    /// `status == Verified` to provenance, not `confidence`.
    fn claim(text: &str, confidence: Confidence) -> MemoryClaim {
        let hash = ContentHash::compute(text.as_bytes());
        MemoryClaim::new(
            MemoryClaimId::new(),
            text,
            Provenance::unverified("test:fixture", hash),
            confidence,
            ClaimStatus::Unverified,
        )
        .expect("Unverified status is always constructible")
    }

    #[test]
    fn context_pack_config_default_is_conservative() {
        assert_eq!(ContextPackConfig::default().budget_tokens, 2_000);
    }

    #[test]
    fn context_pack_config_absent_fields_default_on_parse() {
        // Backward-compat guard: a `surge.toml` predating this key parses
        // as an empty table and must decode with the conservative default.
        let parsed: ContextPackConfig = toml::from_str("").unwrap();
        assert_eq!(parsed, ContextPackConfig::default());
    }

    #[test]
    fn empty_input_yields_an_empty_pack_and_a_fit_receipt() {
        let (pack, receipt) =
            ContextPack::build(Vec::new(), ContextPackConfig { budget_tokens: 100 });

        assert!(pack.is_empty());
        assert_eq!(pack.len(), 0);
        assert!(receipt.selected.is_empty());
        assert!(receipt.dropped.is_empty());
        assert_eq!(receipt.reason, None);
        assert_eq!(receipt.used, 0);
        assert_eq!(receipt.budget, 100);
    }

    #[test]
    fn a_claim_costing_more_than_the_whole_budget_is_dropped_not_truncated() {
        // 200 chars ~ 50 estimated tokens; budget of 10 cannot admit it.
        let oversized = claim(&"x".repeat(200), Confidence::Asserted);
        let id = oversized.id();

        let (pack, receipt) =
            ContextPack::build(vec![oversized], ContextPackConfig { budget_tokens: 10 });

        assert!(pack.is_empty());
        assert_eq!(receipt.dropped, vec![id]);
        assert_eq!(receipt.reason, Some(DropReason::OverBudget));
        assert_eq!(receipt.used, 0);
    }

    #[test]
    fn confidence_order_beats_a_cheaper_lower_confidence_candidate() {
        // Chosen so the arithmetic is checkable by hand from this module's
        // documented ~4-chars-per-token estimate: 100 chars -> 25 tokens
        // exactly fills a budget of 25, leaving zero room for anything
        // else, however cheap.
        let verified = claim(&"v".repeat(100), Confidence::Verified);
        let cheap_asserted = claim("hi", Confidence::Asserted);
        let verified_id = verified.id();
        let asserted_id = cheap_asserted.id();

        // Input order is deliberately reversed (cheap claim first) so a
        // naive "greedy by size" selector — which this must not be — would
        // pick the cheap one and still have room left over.
        let (pack, receipt) = ContextPack::build(
            vec![cheap_asserted, verified],
            ContextPackConfig { budget_tokens: 25 },
        );

        assert_eq!(pack.len(), 1);
        assert_eq!(pack.claims()[0].id(), verified_id);
        assert_eq!(receipt.selected, vec![verified_id]);
        assert_eq!(receipt.dropped, vec![asserted_id]);
        assert_eq!(receipt.reason, Some(DropReason::OverBudget));
        assert_eq!(receipt.used, 25);
    }

    #[test]
    fn mixed_confidence_claims_are_ordered_verified_then_name_matched_then_asserted() {
        let asserted = claim("asserted claim", Confidence::Asserted);
        let verified = claim("verified claim", Confidence::Verified);
        let name_matched = claim("name-matched claim", Confidence::NameMatched);

        // Input order is deliberately scrambled relative to confidence rank.
        let (pack, receipt) = ContextPack::build(
            vec![asserted.clone(), verified.clone(), name_matched.clone()],
            ContextPackConfig {
                budget_tokens: 1_000,
            },
        );

        assert_eq!(
            pack.claims()
                .iter()
                .map(MemoryClaim::id)
                .collect::<Vec<_>>(),
            vec![verified.id(), name_matched.id(), asserted.id()],
        );
        assert!(receipt.dropped.is_empty());
        assert_eq!(receipt.reason, None);
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 256,
            ..Default::default()
        })]

        /// The property the ticket asks for by name: for any set of claim
        /// texts and any budget, the estimated cost of what gets selected
        /// never exceeds the budget.
        #[test]
        fn selected_cost_never_exceeds_budget(
            texts in proptest::collection::vec(".{0,300}", 0..20),
            budget_tokens in 0u32..2_000,
        ) {
            let claims: Vec<MemoryClaim> = texts
                .into_iter()
                .map(|text| {
                    let hash = ContentHash::compute(text.as_bytes());
                    MemoryClaim::from_transcript(text, "test:fixture", hash)
                })
                .collect();

            let (_pack, receipt) =
                ContextPack::build(claims, ContextPackConfig { budget_tokens });

            proptest::prop_assert!(receipt.used <= budget_tokens);
            proptest::prop_assert!(receipt.used <= receipt.budget);
        }
    }
}
