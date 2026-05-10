//! Compile-time bundled flow templates.
//!
//! Bundled flows are first-party `flow.toml` assets embedded with
//! `include_str!`, matching the bundled profile registry pattern. They are
//! used by bootstrap and by `surge engine run --template`.

use semver::Version;

use crate::graph::Graph;

const BOOTSTRAP_1_0_TOML: &str = include_str!("../bundled/flows/bootstrap-1.0.toml");
const LINEAR_3_1_0_TOML: &str = include_str!("../bundled/flows/linear-3-1.0.toml");
const LINEAR_WITH_REVIEW_1_0_TOML: &str =
    include_str!("../bundled/flows/linear-with-review-1.0.toml");
const MULTI_MILESTONE_1_0_TOML: &str = include_str!("../bundled/flows/multi-milestone-1.0.toml");
const BUG_FIX_1_0_TOML: &str = include_str!("../bundled/flows/bug-fix-1.0.toml");
const REFACTOR_1_0_TOML: &str = include_str!("../bundled/flows/refactor-1.0.toml");
const SPIKE_1_0_TOML: &str = include_str!("../bundled/flows/spike-1.0.toml");
const SINGLE_TASK_1_0_TOML: &str = include_str!("../bundled/flows/single-task-1.0.toml");

/// Total number of bundled flow assets registered in [`BundledFlows`].
pub const BUNDLED_FLOW_COUNT: usize = 8;

/// Parsed bundled flow plus registry metadata derived from the asset name.
#[derive(Debug, Clone, PartialEq)]
pub struct BundledFlow {
    /// Stable lookup name, e.g. `"bootstrap"`.
    pub name: String,
    /// Semantic version from the bundled filename.
    pub version: Version,
    /// Parsed flow graph.
    pub graph: Graph,
}

/// Look-up table for compile-time bundled flows.
#[derive(Debug, Clone, Copy)]
pub struct BundledFlows;

impl BundledFlows {
    /// Return every bundled flow, freshly parsed.
    ///
    /// # Panics
    /// Panics if any embedded flow fails to parse. This is a build-time
    /// invariant and should be caught by unit tests / CI.
    #[must_use]
    pub fn all() -> Vec<BundledFlow> {
        vec![
            parse(BOOTSTRAP_1_0_TOML, "bootstrap", "1.0.0"),
            parse(LINEAR_3_1_0_TOML, "linear-3", "1.0.0"),
            parse(LINEAR_WITH_REVIEW_1_0_TOML, "linear-with-review", "1.0.0"),
            parse(MULTI_MILESTONE_1_0_TOML, "multi-milestone", "1.0.0"),
            parse(BUG_FIX_1_0_TOML, "bug-fix", "1.0.0"),
            parse(REFACTOR_1_0_TOML, "refactor", "1.0.0"),
            parse(SPIKE_1_0_TOML, "spike", "1.0.0"),
            parse(SINGLE_TASK_1_0_TOML, "single-task", "1.0.0"),
        ]
    }

    /// Find a bundled flow by exact `(name, version)`.
    #[must_use]
    pub fn by_name_version(name: &str, version: &Version) -> Option<BundledFlow> {
        Self::all()
            .into_iter()
            .find(|flow| flow.name == name && &flow.version == version)
    }

    /// Find the highest-version bundled flow with the given name.
    #[must_use]
    pub fn by_name_latest(name: &str) -> Option<BundledFlow> {
        Self::all()
            .into_iter()
            .filter(|flow| flow.name == name)
            .max_by(|a, b| a.version.cmp(&b.version))
    }
}

fn parse(raw: &str, expected_name: &str, version: &str) -> BundledFlow {
    let graph: Graph = toml::from_str(raw)
        .unwrap_or_else(|e| panic!("bundled flow {expected_name:?} failed to parse: {e}"));
    assert!(
        graph.metadata.name == expected_name,
        "bundled flow constant for {expected_name:?} contained metadata.name = {:?}",
        graph.metadata.name,
    );
    let version = Version::parse(version)
        .unwrap_or_else(|e| panic!("bundled flow {expected_name:?} has invalid version: {e}"));
    tracing::trace!(
        target: "flow::bundled",
        name = expected_name,
        version = %version,
        "loaded bundled flow"
    );
    BundledFlow {
        name: expected_name.to_owned(),
        version,
        graph,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_returns_expected_count() {
        assert_eq!(BundledFlows::all().len(), BUNDLED_FLOW_COUNT);
    }

    #[test]
    fn bootstrap_resolves_by_latest_name() {
        let flow = BundledFlows::by_name_latest("bootstrap").expect("bootstrap flow is bundled");
        assert_eq!(flow.name, "bootstrap");
        assert_eq!(flow.version, Version::new(1, 0, 0));
        assert_eq!(flow.graph.start.as_ref(), "description_author");
    }

    #[test]
    fn bootstrap_resolves_by_exact_version() {
        let flow = BundledFlows::by_name_version("bootstrap", &Version::new(1, 0, 0))
            .expect("bootstrap@1.0.0 is bundled");
        assert_eq!(flow.graph.metadata.name, "bootstrap");
    }

    #[test]
    fn unknown_flow_is_none() {
        assert!(BundledFlows::by_name_latest("does-not-exist").is_none());
    }

    #[test]
    fn archetype_templates_resolve_by_latest_name() {
        for name in [
            "linear-3",
            "linear-with-review",
            "multi-milestone",
            "bug-fix",
            "refactor",
            "spike",
            "single-task",
        ] {
            let flow = BundledFlows::by_name_latest(name).expect("archetype flow is bundled");
            assert_eq!(flow.name, name);
            assert_eq!(flow.version, Version::new(1, 0, 0));
        }
    }
}
