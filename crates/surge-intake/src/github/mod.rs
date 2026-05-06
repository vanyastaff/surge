//! GitHub Issues adapter for `surge-intake`.

pub mod client;
pub mod source;

pub use source::{GitHubConfig, GitHubIssuesTaskSource};
