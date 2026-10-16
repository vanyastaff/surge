//! Pure node/profile runtime precedence; recipe resolution remains with the run registry.
use surge_core::{agent_config::AgentConfig, profile::registry::ResolvedProfile};

/// Raw provider selector requested by a node or inherited from its resolved profile.
///
/// This does not canonicalize aliases, validate credentials, or grant launch authority.
/// Callers must resolve the returned selector against their actual run agent registry.
#[must_use]
pub fn effective_runtime_selector<'a>(
    node: &'a AgentConfig,
    profile: &'a ResolvedProfile,
) -> &'a str {
    node.runtime_override()
        .unwrap_or(profile.profile.runtime.agent_id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_loader::{DiskProfileSet, ProfileRegistry};

    #[test]
    fn precedence_is_node_then_profile_and_raw_alias_is_not_canonicalized() {
        let root = tempfile::tempdir().unwrap();
        let registry = ProfileRegistry::new(DiskProfileSet::scan(root.path()).unwrap());
        let profile = registry
            .resolve(&surge_core::profile::keyref::parse_key_ref("description-author@1.0").unwrap())
            .unwrap();
        let node: AgentConfig = toml::from_str("profile = 'description-author@1.0'").unwrap();
        assert_eq!(effective_runtime_selector(&node, &profile), "claude-code");
        let overridden = node.with_runtime_override("codex", Some("chosen-model"));
        assert_eq!(effective_runtime_selector(&overridden, &profile), "codex");
        assert_eq!(overridden.model_override(), Some("chosen-model"));
    }
}
