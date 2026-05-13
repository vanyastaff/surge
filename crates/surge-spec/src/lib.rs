//! Spec system for Surge — parsing, building, validation, and dependency graphs.
//!
//! # Deprecated
//!
//! This crate is retiring as part of the **Legacy pipeline retirement** milestone.
//! Use `surge-orchestrator::engine` and `flow.toml` for all new work. See
//! `docs/migrate-spec-to-flow.md` for the migration guide and `surge migrate-spec`
//! for the auto-translator.

// Suppress deprecation warnings for in-crate re-exports and intra-module uses.
// External consumers (surge-cli, surge-orchestrator) still see the warnings,
// which is the intent for the deprecation window.
#![allow(deprecated)]
// Pre-existing legacy code; M5 does not modify this crate.
// These allows suppress pedantic lints that fire when clippy::pedantic is
// requested transitively by surge-orchestrator.
#![allow(clippy::doc_markdown)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::redundant_else)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::single_match_else)]
#![allow(clippy::if_not_else)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::struct_excessive_bools)]

pub mod builder;
pub use builder::{SpecBuilder, SubtaskBuilder};
pub mod graph;
pub use graph::DependencyGraph;
pub mod parser;
pub use parser::SpecFile;
pub mod templates;
pub use templates::{TemplateKind, generate as generate_template};
pub mod validation;
pub use validation::{ValidationResult, validate as validate_spec};
