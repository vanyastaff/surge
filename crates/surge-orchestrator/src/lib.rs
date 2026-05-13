//! Orchestrator — graph executor (`engine` module) + bootstrap chain, project
//! context, and roadmap-amendment surfaces.
//!
//! The legacy spec pipeline (budget, circuit_breaker, conflict, context,
//! executor, gates, parallel, phases, pipeline, planner, project, qa, retry,
//! schedule) was retired as part of the **Legacy pipeline retirement**
//! milestone. All work now flows through [`engine::Engine`].

// Suppress remaining pedantic lints carried over from the legacy surface.
// As the engine module tightens its own lints, this list will shrink.
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

pub mod archetype_registry;
pub mod bootstrap;
pub mod bootstrap_driver;
pub mod engine;
pub mod feature_driver;
pub mod flow_amendment;
pub mod profile_loader;
pub mod project_context;
pub mod prompt;
pub mod roadmap_amendment;
pub mod roadmap_document;
pub mod roadmap_target;
pub mod triage;

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
