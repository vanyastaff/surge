//! Compile-time bundled profiles.
//!
//! `surge-cli` and `surge-daemon` ship with a fixed set of first-party
//! profiles. They are embedded via [`include_str!`] (compile-time inlined)
//! and parsed lazily on each access. Disk profiles under
//! `${SURGE_HOME}/profiles/` always take priority — bundled profiles are the
//! fallback the registry consults last.
//!
//! ## Adding a bundled profile
//!
//! 1. Drop the TOML into `crates/surge-core/bundled/profiles/<name>-<MAJOR>.<MINOR>.toml`.
//! 2. Add a `const FOO_TOML: &str = include_str!("...");` line below.
//! 3. Append `parse(FOO_TOML, "foo")` to [`BundledRegistry::all`].
//!
//! Step 2 makes the asset compile-time mandatory: a missing file is a build
//! error rather than a runtime surprise.

use semver::Version;

use crate::profile::Profile;

// ── Embedded asset constants ───────────────────────────────────────
// One per bundled profile. Filenames follow `name-MAJOR.MINOR.toml`;
// the canonical version lives inside the TOML body's `[role] version`.

const MOCK_TOML: &str = include_str!("../../bundled/profiles/mock-1.0.toml");

const DESCRIPTION_AUTHOR_TOML: &str =
    include_str!("../../bundled/profiles/description-author-1.0.toml");
const ROADMAP_PLANNER_TOML: &str =
    include_str!("../../bundled/profiles/roadmap-planner-1.0.toml");
const FLOW_GENERATOR_TOML: &str =
    include_str!("../../bundled/profiles/flow-generator-1.0.toml");

const SPEC_AUTHOR_TOML: &str = include_str!("../../bundled/profiles/spec-author-1.0.toml");
const ARCHITECT_TOML: &str = include_str!("../../bundled/profiles/architect-1.0.toml");
const IMPLEMENTER_TOML: &str = include_str!("../../bundled/profiles/implementer-1.0.toml");
const TEST_AUTHOR_TOML: &str = include_str!("../../bundled/profiles/test-author-1.0.toml");
const VERIFIER_TOML: &str = include_str!("../../bundled/profiles/verifier-1.0.toml");
const REVIEWER_TOML: &str = include_str!("../../bundled/profiles/reviewer-1.0.toml");
const PR_COMPOSER_TOML: &str = include_str!("../../bundled/profiles/pr-composer-1.0.toml");

const BUG_FIX_IMPLEMENTER_TOML: &str =
    include_str!("../../bundled/profiles/bug-fix-implementer-1.0.toml");
const REFACTOR_IMPLEMENTER_TOML: &str =
    include_str!("../../bundled/profiles/refactor-implementer-1.0.toml");
const SECURITY_REVIEWER_TOML: &str =
    include_str!("../../bundled/profiles/security-reviewer-1.0.toml");
const MIGRATION_IMPLEMENTER_TOML: &str =
    include_str!("../../bundled/profiles/migration-implementer-1.0.toml");

const PROJECT_CONTEXT_AUTHOR_TOML: &str =
    include_str!("../../bundled/profiles/project-context-author-1.0.toml");
const FEATURE_PLANNER_TOML: &str =
    include_str!("../../bundled/profiles/feature-planner-1.0.toml");

/// Total number of bundled profiles. Centralized so tests can spot-check
/// that nothing was added or dropped silently.
pub const BUNDLED_COUNT: usize = 17;

/// Look-up table for compile-time bundled profiles.
///
/// All accessors parse the underlying TOML on each call. The cost is
/// trivial (sub-millisecond per profile) and keeps the type `'static`-free.
#[derive(Debug, Clone, Copy)]
pub struct BundledRegistry;

impl BundledRegistry {
    /// Return every bundled profile, freshly parsed.
    ///
    /// Order matches the registration list below: bootstrap → execution →
    /// specialized → project → mock. The order has no semantic meaning —
    /// the registry resolves by name, not by index.
    ///
    /// # Panics
    /// Panics if any embedded TOML asset fails to parse against the current
    /// [`Profile`] schema. This is a build-time invariant; a panic here means
    /// a bundled file diverged from the type and CI should catch it.
    #[must_use]
    pub fn all() -> Vec<Profile> {
        vec![
            // Bootstrap (Task 9).
            parse(DESCRIPTION_AUTHOR_TOML, "description-author"),
            parse(ROADMAP_PLANNER_TOML, "roadmap-planner"),
            parse(FLOW_GENERATOR_TOML, "flow-generator"),
            // Execution (Task 10).
            parse(SPEC_AUTHOR_TOML, "spec-author"),
            parse(ARCHITECT_TOML, "architect"),
            parse(IMPLEMENTER_TOML, "implementer"),
            parse(TEST_AUTHOR_TOML, "test-author"),
            parse(VERIFIER_TOML, "verifier"),
            parse(REVIEWER_TOML, "reviewer"),
            parse(PR_COMPOSER_TOML, "pr-composer"),
            // Specialized (Task 11).
            parse(BUG_FIX_IMPLEMENTER_TOML, "bug-fix-implementer"),
            parse(REFACTOR_IMPLEMENTER_TOML, "refactor-implementer"),
            parse(SECURITY_REVIEWER_TOML, "security-reviewer"),
            parse(MIGRATION_IMPLEMENTER_TOML, "migration-implementer"),
            // Project (Task 12).
            parse(PROJECT_CONTEXT_AUTHOR_TOML, "project-context-author"),
            parse(FEATURE_PLANNER_TOML, "feature-planner"),
            // Mock (Task 31).
            parse(MOCK_TOML, "mock"),
        ]
    }

    /// Find a bundled profile by `(name, version)`. Both fields are matched
    /// exactly against `Profile.role.id` and `Profile.role.version`.
    #[must_use]
    pub fn by_name_version(name: &str, version: &Version) -> Option<Profile> {
        Self::all()
            .into_iter()
            .find(|p| p.role.id.as_str() == name && &p.role.version == version)
    }

    /// Find the highest-version bundled profile with the given name.
    #[must_use]
    pub fn by_name_latest(name: &str) -> Option<Profile> {
        Self::all()
            .into_iter()
            .filter(|p| p.role.id.as_str() == name)
            .max_by(|a, b| a.role.version.cmp(&b.role.version))
    }
}

fn parse(raw: &str, expected_name: &str) -> Profile {
    let profile: Profile = toml::from_str(raw).unwrap_or_else(|e| {
        panic!("bundled profile {expected_name:?} failed to parse: {e}")
    });
    assert!(
        profile.role.id.as_str() == expected_name,
        "bundled profile constant for {expected_name:?} contained role.id = {:?}",
        profile.role.id.as_str(),
    );
    tracing::trace!(
        target: "profile::bundled",
        name = expected_name,
        version = %profile.role.version,
        "loaded bundled profile"
    );
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_returns_expected_count() {
        assert_eq!(BundledRegistry::all().len(), BUNDLED_COUNT);
    }

    #[test]
    fn all_profile_ids_are_unique() {
        let all = BundledRegistry::all();
        let mut ids: Vec<&str> = all.iter().map(|p| p.role.id.as_str()).collect();
        ids.sort_unstable();
        let original_len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), original_len, "duplicate bundled profile id");
    }

    #[test]
    fn mock_resolves_by_name() {
        let p = BundledRegistry::by_name_latest("mock").expect("mock@1.0 must be bundled");
        assert_eq!(p.role.id.as_str(), "mock");
        assert_eq!(p.runtime.agent_id, "mock");
    }

    #[test]
    fn mock_resolves_by_exact_version() {
        let v = Version::new(1, 0, 0);
        let p = BundledRegistry::by_name_version("mock", &v).expect("mock@1.0.0 must match");
        assert_eq!(p.role.version, v);
    }

    #[test]
    fn unknown_name_is_none() {
        assert!(BundledRegistry::by_name_latest("does-not-exist").is_none());
    }

    #[test]
    fn implementer_default_agent_is_claude_code() {
        let p = BundledRegistry::by_name_latest("implementer").expect("bundled");
        assert_eq!(p.runtime.agent_id, "claude-code");
    }

    #[test]
    fn every_bundled_profile_resolves_through_merge_chain() {
        // For every bundled profile, walk its `extends` chain (using the
        // bundled set as the lookup) and merge. Any failure here means a
        // bundled profile references a parent that isn't bundled or that
        // produces a cycle / depth violation.
        let all = BundledRegistry::all();
        // The lookup mirrors what `surge-orchestrator::profile_loader` will
        // do: parse `name@version` into a ProfileKeyRef, then resolve through
        // the bundled set by name+version (or name+latest when version
        // omitted).
        let lookup = |key: &crate::keys::ProfileKey| -> Result<Option<crate::profile::Profile>, crate::error::SurgeError> {
            let parsed = crate::profile::keyref::parse_key_ref(key.as_str())
                .map_err(|e| crate::error::SurgeError::InvalidProfileKey(e.to_string()))?;
            let resolved = match parsed.version {
                Some(ref v) => BundledRegistry::by_name_version(parsed.name.as_str(), v),
                None => BundledRegistry::by_name_latest(parsed.name.as_str()),
            };
            Ok(resolved)
        };

        for leaf in all {
            let leaf_name = leaf.role.id.as_str().to_string();
            let chain = crate::profile::registry::collect_chain(leaf, lookup)
                .unwrap_or_else(|e| panic!("collect_chain failed for {leaf_name:?}: {e}"));

            assert!(
                !chain.is_empty(),
                "chain must include leaf for {leaf_name:?}"
            );

            let merged = crate::profile::registry::merge_chain(&chain)
                .unwrap_or_else(|e| panic!("merge_chain failed for {leaf_name:?}: {e}"));

            // Sanity checks on the merged result.
            assert!(
                !merged.role.display_name.is_empty(),
                "{leaf_name:?} merged has empty display_name"
            );
            assert!(
                !merged.outcomes.is_empty(),
                "{leaf_name:?} merged has no outcomes"
            );
            assert!(
                !merged.runtime.agent_id.is_empty(),
                "{leaf_name:?} merged has empty agent_id"
            );
            assert!(
                !merged.prompt.system.is_empty(),
                "{leaf_name:?} merged has empty prompt.system"
            );
        }
    }

    #[test]
    fn specialized_variants_extend_known_parents() {
        // Spot-check the four extends-based variants explicitly.
        for name in [
            "bug-fix-implementer",
            "refactor-implementer",
            "security-reviewer",
            "migration-implementer",
        ] {
            let p = BundledRegistry::by_name_latest(name)
                .unwrap_or_else(|| panic!("{name} must be bundled"));
            let parent = p
                .role
                .extends
                .as_ref()
                .unwrap_or_else(|| panic!("{name} must declare extends"));
            // The parent reference uses ProfileKey form; resolve via key parse.
            let parent_ref = crate::profile::keyref::parse_key_ref(parent.as_str())
                .unwrap_or_else(|e| panic!("{name} extends {parent} fails to parse: {e}"));
            let resolved = match parent_ref.version {
                Some(ref v) => BundledRegistry::by_name_version(parent_ref.name.as_str(), v),
                None => BundledRegistry::by_name_latest(parent_ref.name.as_str()),
            };
            assert!(
                resolved.is_some(),
                "{name} extends {parent}, which is not bundled"
            );
        }
    }
}
