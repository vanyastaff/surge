//! Host-owned configured routing snapshots. These never claim an observed account.
use super::{config::EngineConfig, configured_route_pin::RoutePinKey, error::EngineError};
use std::{collections::BTreeMap, path::Path};
use surge_core::{
    ContentHash,
    config::{AuthSource, CapacityRoute, ConfiguredSourceCompleteness},
};
use surge_persistence::work_items::recovery_cycles::{
    AccountEvidence, FrozenQuotaCandidate, FrozenQuotaPolicy, FrozenQuotaStage, QuotaRoutingMode,
    RecoveryCandidate,
};

/// Materialize declared inherited managed keys into the effective child override snapshot.
pub(crate) fn materialize_managed_env(route: &CapacityRoute, env: &mut BTreeMap<String, String>) {
    for source in &route.auth_sources {
        if let AuthSource::Env { target_key } = source
            && !env.contains_key(target_key)
            && let Ok(value) = std::env::var(target_key)
        {
            env.insert(target_key.clone(), value);
        }
    }
}
/// HMAC only the actual launch snapshot. Missing/opaque sources have no reusable pin.
pub(crate) fn configured_pin(
    home: &Path,
    runtime: &str,
    kind: &surge_acp::bridge::session::AgentKind,
    route: &CapacityRoute,
    effective_env: &BTreeMap<String, String>,
    working_dir: &Path,
) -> Option<ContentHash> {
    if route.completeness != ConfiguredSourceCompleteness::CompleteConfiguredSources {
        return None;
    }
    let mut files = Vec::new();
    for source in &route.auth_sources {
        match source {
            AuthSource::Env { target_key } if !effective_env.contains_key(target_key) => {
                return None;
            },
            AuthSource::File { path } => files.push(path.clone()),
            AuthSource::Env { .. } => {},
        }
    }
    let identity = serde_json::to_vec(&(runtime, format!("{kind:?}"), route)).ok()?;
    let key = RoutePinKey::load_or_create(home).ok()?;
    let pin = key
        .fingerprint(&identity, effective_env, &files, working_dir)
        .ok()?;
    Some(ContentHash::compute(pin.as_bytes()))
}

fn requested_primary_model(
    agent: &surge_core::agent_config::AgentConfig,
    recommended: &str,
) -> Option<String> {
    agent
        .model_override()
        .or(Some(recommended))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

pub(crate) fn freeze_policy(
    graph: &surge_core::Graph,
    config: &EngineConfig,
    home: &Path,
    working_dir: &Path,
) -> Result<Option<FrozenQuotaPolicy>, EngineError> {
    let surge_core::capacity::RotationPolicy::Candidate {
        profile: target_profile,
    } = &config.capacity.rotation
    else {
        return Ok(None);
    };
    let profiles = config.profile_registry.as_deref().ok_or_else(|| {
        EngineError::GraphInvalid("configured rotation requires host profile registry".into())
    })?;
    let builtin = surge_acp::Registry::builtin();
    let registry = config.agent_registry.as_deref().unwrap_or(&builtin);
    let target = profiles
        .resolve(
            &surge_core::profile::keyref::parse_key_ref(target_profile)
                .map_err(|e| EngineError::GraphInvalid(e.to_string()))?,
        )
        .map_err(|e| EngineError::GraphInvalid(e.to_string()))?;
    let mut stages = Vec::new();
    for node in graph
        .nodes
        .values()
        .chain(graph.subgraphs.values().flat_map(|g| g.nodes.values()))
    {
        let surge_core::node::NodeConfig::Agent(agent) = &node.config else {
            continue;
        };
        let primary = profiles
            .resolve(
                &surge_core::profile::keyref::parse_key_ref(agent.profile.as_str())
                    .map_err(|e| EngineError::GraphInvalid(e.to_string()))?,
            )
            .map_err(|e| EngineError::GraphInvalid(e.to_string()))?;
        let primary_runtime = agent
            .runtime_override()
            .unwrap_or(&primary.profile.runtime.agent_id);
        let candidates = freeze_candidates(
            [
                (
                    agent.profile.as_str(),
                    primary_runtime,
                    requested_primary_model(agent, &primary.profile.runtime.recommended_model),
                ),
                (
                    target_profile.as_str(),
                    target.profile.runtime.agent_id.as_str(),
                    Some(target.profile.runtime.recommended_model.clone()),
                ),
            ],
            registry,
            home,
            working_dir,
        )?;
        let mode = if candidates.len() > 1 {
            QuotaRoutingMode::Configured
        } else {
            QuotaRoutingMode::Disabled
        };
        let backoff = config
            .capacity
            .blind_backoff
            .unwrap_or(std::time::Duration::from_secs(60));
        stages.push(
            FrozenQuotaStage::new(
                node.id.clone(),
                mode,
                candidates,
                i64::try_from(backoff.as_millis())
                    .map_err(|e| EngineError::GraphInvalid(e.to_string()))?,
                60_000,
            )
            .map_err(|e| EngineError::GraphInvalid(e.to_string()))?,
        );
    }
    Ok(Some(
        FrozenQuotaPolicy::new(stages).map_err(|e| EngineError::GraphInvalid(e.to_string()))?,
    ))
}

fn freeze_candidates(
    targets: [(&str, &str, Option<String>); 2],
    registry: &surge_acp::Registry,
    home: &Path,
    working_dir: &Path,
) -> Result<Vec<FrozenQuotaCandidate>, EngineError> {
    let mut candidates = Vec::new();
    let mut family: Option<String> = None;
    for (profile, runtime, model) in targets {
        let entry = registry.find_normalized(runtime).ok_or_else(|| {
            EngineError::GraphInvalid("configured rotation registry target absent".into())
        })?;
        if candidates
            .iter()
            .any(|candidate: &FrozenQuotaCandidate| candidate.candidate().runtime() == entry.id)
        {
            continue;
        }
        let Some(route) = &entry.capacity_route else {
            return Err(EngineError::GraphInvalid(
                "configured rotation requires explicit provider family and route declarations"
                    .into(),
            ));
        };
        validate_route_declaration(
            route,
            family.as_deref(),
            candidates
                .iter()
                .filter_map(FrozenQuotaCandidate::configured_route),
        )?;
        family = Some(route.provider_family.clone());
        let mut launch =
            super::stage::agent::derive_agent_kind_from_id(profile, &entry.id, Some(registry))
                .map_err(|e| EngineError::GraphInvalid(e.to_string()))?;
        materialize_managed_env(route, &mut launch.env);
        let pin = configured_pin(
            home,
            &entry.id,
            &launch.kind,
            route,
            &launch.env,
            working_dir,
        );
        let candidate = FrozenQuotaCandidate::new(
            RecoveryCandidate::new(entry.id.clone(), AccountEvidence::Unknown)
                .map_err(|e| EngineError::GraphInvalid(e.to_string()))?,
            model.filter(|v| !v.is_empty()),
            ContentHash::compute(format!("{:?}", launch.kind).as_bytes()),
        )
        .map_err(|e| EngineError::GraphInvalid(e.to_string()))?
        .with_configured_snapshot(route.clone(), pin);
        candidates.push(candidate);
    }
    Ok(candidates)
}

fn validate_route_declaration<'a>(
    route: &CapacityRoute,
    family: Option<&str>,
    mut prior: impl Iterator<Item = &'a CapacityRoute>,
) -> Result<(), EngineError> {
    if route.provider_family.trim().is_empty() || route.configured_route.trim().is_empty() {
        return Err(EngineError::GraphInvalid(
            "configured capacity identity is empty".into(),
        ));
    }
    if family.is_some_and(|family| family != route.provider_family) {
        return Err(EngineError::GraphInvalid(
            "configured rotation targets have different declared provider families".into(),
        ));
    }
    if prior.any(|earlier| earlier.configured_route == route.configured_route) {
        return Err(EngineError::GraphInvalid(
            "configured rotation targets declare the same route".into(),
        ));
    }
    Ok(())
}

/// Recheck a skipped route using today's declared source snapshot, without provider RPCs.
pub(crate) fn verify_skipped_snapshot(
    home: &Path,
    working_dir: &Path,
    registry: &surge_acp::Registry,
    target: &FrozenQuotaCandidate,
) -> Result<(), super::stage::StageError> {
    let route = target.configured_route().ok_or_else(|| {
        super::stage::StageError::RecoveryRequired(
            "skipped route has no configured provenance".into(),
        )
    })?;
    let entry = registry
        .find_normalized(target.candidate().runtime())
        .ok_or_else(|| {
            super::stage::StageError::RecoveryRequired("skipped registry route disappeared".into())
        })?;
    if entry.capacity_route.as_ref() != Some(route) {
        return Err(super::stage::StageError::RecoveryRequired(
            "skipped configured route declaration changed".into(),
        ));
    }
    let mut launch = super::stage::agent::derive_agent_kind_from_id(
        "configured capacity route",
        &entry.id,
        Some(registry),
    )?;
    materialize_managed_env(route, &mut launch.env);
    let actual = configured_pin(
        home,
        &entry.id,
        &launch.kind,
        route,
        &launch.env,
        working_dir,
    );
    if target.configured_pin().is_none() || actual.as_ref() != target.configured_pin() {
        return Err(super::stage::StageError::RecoveryRequired(
            "skipped configured source changed before provider effect".into(),
        ));
    }
    Ok(())
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    fn route(completeness: ConfiguredSourceCompleteness) -> CapacityRoute {
        CapacityRoute {
            provider_family: "family".into(),
            configured_route: "profile-one".into(),
            auth_sources: vec![AuthSource::Env {
                target_key: "MANAGED_TOKEN".into(),
            }],
            completeness,
        }
    }

    #[test]
    fn primary_recommended_model_drift_changes_recipe_but_node_override_wins() {
        let mut agent: surge_core::agent_config::AgentConfig =
            toml::from_str(r#"profile = "role@1.0""#).unwrap();
        assert_eq!(
            requested_primary_model(&agent, "sonnet"),
            Some("sonnet".into())
        );
        assert_ne!(
            requested_primary_model(&agent, "sonnet"),
            requested_primary_model(&agent, "opus")
        );
        agent.custom_fields.insert(
            "runtime".into(),
            toml::from_str::<toml::Value>(r#"model = "fixed""#).unwrap(),
        );
        assert_eq!(
            requested_primary_model(&agent, "opus"),
            Some("fixed".into())
        );
        assert!(
            requested_primary_model(&toml::from_str(r#"profile = "role@1.0""#).unwrap(), "")
                .is_none()
        );
    }

    #[test]
    fn duplicate_declared_routes_cannot_claim_distinct_targets() {
        let primary = route(ConfiguredSourceCompleteness::CompleteConfiguredSources);
        assert!(
            validate_route_declaration(
                &primary,
                Some(&primary.provider_family),
                std::iter::once(&primary)
            )
            .is_err()
        );
    }

    #[test]
    fn opaque_missing_or_changed_sources_never_reuse_a_pin() {
        let home = tempfile::tempdir().unwrap();
        let kind = surge_acp::bridge::session::AgentKind::Custom {
            binary: "fixture".into(),
            args: Vec::new(),
        };
        let complete = route(ConfiguredSourceCompleteness::CompleteConfiguredSources);
        let mut env = BTreeMap::from([("MANAGED_TOKEN".into(), "explicit-secret-one".into())]);
        let original =
            configured_pin(home.path(), "a", &kind, &complete, &env, home.path()).unwrap();
        assert!(!original.to_string().contains("explicit-secret-one"));
        env.insert("MANAGED_TOKEN".into(), "explicit-secret-two".into());
        assert_ne!(
            configured_pin(home.path(), "a", &kind, &complete, &env, home.path()).unwrap(),
            original
        );
        assert!(
            configured_pin(
                home.path(),
                "a",
                &kind,
                &complete,
                &BTreeMap::new(),
                home.path()
            )
            .is_none()
        );
        assert!(
            configured_pin(
                home.path(),
                "a",
                &kind,
                &route(ConfiguredSourceCompleteness::Opaque),
                &env,
                home.path()
            )
            .is_none()
        );
        let mut inaccessible = complete;
        inaccessible.auth_sources.push(AuthSource::File {
            path: "absent-auth-file".into(),
        });
        assert!(
            configured_pin(home.path(), "a", &kind, &inaccessible, &env, home.path()).is_none()
        );
    }
}
