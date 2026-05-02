//! 12.11 [P2.X5] — smoke-test that legacy persistence APIs still resolve.
//!
//! The M2 work added a new `runs` module alongside the legacy
//! `aggregator/budget/memory/pricing/store` modules without intending to
//! break them. This test simply names a few legacy public items so that
//! a future rename or accidental removal turns into a compile error here.
//! No runtime behavior is exercised — the body uses `let _ =` to keep the
//! item references purely linkage-sized.

use surge_persistence::{
    aggregator::{SessionContext, UsageAggregator},
    budget::{BudgetStatus, BudgetTracker, BudgetWarningLevel},
    pricing::{
        claude_opus_pricing, claude_sonnet_35_pricing, get_model_pricing, gpt4_turbo_pricing,
        PricingModel,
    },
    store::Store,
    PersistenceError, Result,
};

#[test]
fn legacy_module_items_still_resolve_at_compile_time() {
    // Pricing helpers — function pointers prove the symbols are linked.
    let _: fn() -> PricingModel = claude_opus_pricing;
    let _: fn() -> PricingModel = claude_sonnet_35_pricing;
    let _: fn() -> PricingModel = gpt4_turbo_pricing;
    let _: fn(&str) -> PricingModel = get_model_pricing;

    // Budget types — sizeof check forces monomorphization.
    let _ = std::mem::size_of::<BudgetStatus>();
    let _ = std::mem::size_of::<BudgetTracker>();
    let _ = std::mem::size_of::<BudgetWarningLevel>();

    // Aggregator types.
    let _ = std::mem::size_of::<SessionContext>();
    let _ = std::mem::size_of::<UsageAggregator>();

    // Store + error type.
    let _ = std::mem::size_of::<Store>();
    let _: fn(rusqlite::Error) -> PersistenceError = PersistenceError::from;

    // Result alias.
    let _: Result<()> = Ok(());
}
