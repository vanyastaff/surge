//! Orchestrator — drives specs through the full pipeline.

// Pre-existing legacy code (budget, circuit_breaker, conflict, context,
// executor, gates, parallel, phases, pipeline, planner, project, qa, retry,
// schedule); M5 does not modify these modules.  These allows suppress pedantic
// lints that fire when clippy::pedantic is enabled on the engine module, which
// Rust propagates to the entire crate via -D flags.
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::explicit_iter_loop)]
#![allow(clippy::if_not_else)]
#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::implicit_clone)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::needless_raw_string_hashes)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::redundant_else)]
#![allow(clippy::single_match_else)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::unnested_or_patterns)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::unused_async)]
#![allow(clippy::unused_self)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::bool_to_int_with_if)]
#![allow(clippy::borrow_as_ptr)]
#![allow(clippy::format_collect)]
#![allow(clippy::format_push_string)]
#![allow(clippy::similar_names)]
#![allow(clippy::default_trait_access)]
#![allow(clippy::elidable_lifetime_names)]
#![allow(clippy::excessive_nesting)]
#![allow(clippy::manual_ok_err)]
#![allow(clippy::module_inception)]
#![allow(clippy::needless_continue)]
#![allow(clippy::manual_range_contains)]
#![allow(clippy::manual_string_new)]
#![allow(clippy::match_wildcard_for_single_variants)]

pub mod bootstrap;
pub mod budget;
pub mod circuit_breaker;
pub mod conflict;
pub mod context;
pub mod engine;
pub mod executor;
pub mod gates;
pub mod parallel;
pub mod phases;
pub mod pipeline;
pub mod planner;
pub mod project;
pub mod qa;
pub mod retry;
pub mod schedule;
pub mod triage;

pub use budget::{BudgetStatus, BudgetTracker};
pub use parallel::ParallelExecutor;
pub use phases::Phase;
pub use pipeline::{Orchestrator, OrchestratorConfig, PipelineResult};
pub use planner::PlannerPhase;
pub use project::{ProjectConfig, ProjectExecutor, ProjectResult};

/// TOML source of the `Triage Author` bootstrap profile, bundled at compile time.
///
/// Used by `surge-orchestrator::triage` (T7.2) to spawn the agent without
/// requiring the user to install profiles into `~/.surge/profiles/_bootstrap/`
/// before first run.
pub const BOOTSTRAP_TRIAGE_AUTHOR_TOML: &str =
    include_str!("../profiles/_bootstrap/triage-author-1.0.toml");

#[cfg(test)]
mod bootstrap_profile_tests {
    use super::*;

    #[test]
    fn triage_author_toml_parses() {
        let value: toml::Value = toml::from_str(BOOTSTRAP_TRIAGE_AUTHOR_TOML)
            .expect("triage-author-1.0.toml is valid TOML");
        assert_eq!(value["id"].as_str(), Some("_bootstrap/triage-author"));
        assert_eq!(value["display_name"].as_str(), Some("Triage Author"));
        assert_eq!(value["version"].as_str(), Some("1.0"));
        let outcomes = value["declared_outcomes"]
            .as_array()
            .expect("declared_outcomes is array");
        assert_eq!(outcomes.len(), 4);
    }
}
