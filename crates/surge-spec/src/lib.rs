//! Spec system for Surge — parsing, building, validation, and dependency graphs.

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
