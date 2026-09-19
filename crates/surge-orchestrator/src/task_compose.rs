//! Installing composed artifacts into the project's `.surge/` layer (T12).
//!
//! When the flow generator (or a future compose branch) produces a **new**
//! flow or profile rather than selecting an existing one, the artifact is
//! installed into the repository — git-diffable, travel-with-the-code — but
//! only after three checks, in this order:
//!
//! 1. **Shape**: a flow is parsed and validated as a `FlowPurpose::Task` flow
//!    (sealed verifier, gate level); a profile is parsed through the strict
//!    profile loader.
//! 2. **Authority**: a composed profile may not declare
//!    `[verification] authority = true` (only the operator elevates a
//!    profile to verifier authority), and it may not shadow a bundled or
//!    home profile of the same name — that would let generated content take
//!    over a name the operator already trusts.
//! 3. **Trust**: the installed file is pinned in the trust store in the same
//!    step, so the system's own output is never re-prompted (T13's gate
//!    exists for content that arrived from elsewhere).
//!
//! The caller appends `ComposedArtifactInstalled` to the run log **after**
//! this function returns — the event records what was installed, and the
//! path it carries is the one returned here, not a string the caller
//! re-derives.

use std::path::Path;

use crate::engine::validate::FlowPurpose;
use surge_core::ContentHash;
use surge_core::artifact_contract::{ArtifactKind, RelPath};
use surge_core::graph::Graph;

/// Why a composed artifact was refused.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ComposeError {
    /// The artifact's file name is not a valid `<name>-<MAJOR>.<MINOR>.toml`
    /// reference (flows) or a valid profile name (profiles).
    #[error("composed artifact name {0:?} is not usable as a file name")]
    BadName(String),
    /// A composed flow failed task validation.
    #[error("composed flow {name} is not a valid task flow: {reason}")]
    FlowInvalid {
        /// Flow name.
        name: String,
        /// Rendered validation error.
        reason: String,
    },
    /// A composed profile did not parse through the strict loader.
    #[error("composed profile {name} does not parse: {reason}")]
    ProfileInvalid {
        /// Profile name.
        name: String,
        /// Parse/validation failure.
        reason: String,
    },
    /// A composed profile tried to claim verifier authority.
    #[error(
        "composed profile {name} declares `[verification] authority = true`; only the \
         operator may elevate a profile to verifier authority"
    )]
    ProfileClaimsAuthority {
        /// Profile name.
        name: String,
    },
    /// A composed profile would shadow a bundled or home profile.
    #[error(
        "composed profile {name} would shadow a {shadowed} profile of the same name; \
         pick a different name or edit the existing profile"
    )]
    ProfileShadowsExisting {
        /// Profile name.
        name: String,
        /// Layer the shadowed profile lives in (`bundled` or `home`).
        shadowed: &'static str,
    },
    /// The artifact could not be written.
    #[error("write {path}: {source}")]
    Write {
        /// Target path.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The TOML could not be rendered.
    #[error("render composed artifact: {0}")]
    Render(#[from] toml::ser::Error),
    /// The trust store could not be updated.
    #[error("trust store: {0}")]
    Trust(String),
}

/// A successfully installed composed artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedArtifact {
    /// What was installed.
    pub kind: ArtifactKind,
    /// Project-relative path of the installed file.
    pub path: RelPath,
    /// Content hash pinned in the trust store.
    pub hash: ContentHash,
}

/// Validate and install a composed **flow** as `.surge/flows/<name>.toml`.
///
/// `name` is the flow's catalog name (`bug-fix`, `custom-review`); a version
/// suffix may be included (`bug-fix-1.0`), and `-1.0` is appended when it is
/// not. The graph is validated as a task flow before anything is written.
///
/// # Errors
/// [`ComposeError`] for a bad name, invalid graph, write failure, or a trust
/// store that cannot be updated.
pub fn install_composed_flow(
    project_root: &Path,
    trust: &mut surge_persistence::trust_store::TrustStore,
    name: &str,
    graph: &Graph,
    now_ms: i64,
) -> Result<ComposedArtifact, ComposeError> {
    // Shape first: a graph that is not a valid task flow never reaches disk.
    crate::engine::validate::validate_for_task(
        graph,
        FlowPurpose::Task,
        None,
        1,
        graph.metadata.autonomy,
    )
    .map_err(|e| ComposeError::FlowInvalid {
        name: name.to_string(),
        reason: e.to_string(),
    })?;

    let file_stem = versioned_stem(name);
    let file_name = format!("{file_stem}.toml");
    let rel_path = RelPath::new(format!(".surge/flows/{file_name}"))
        .map_err(|_| ComposeError::BadName(name.to_string()))?;
    let text = toml::to_string_pretty(graph)?;
    let absolute = rel_path.join_onto(project_root);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ComposeError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&absolute, &text).map_err(|source| ComposeError::Write {
        path: absolute.clone(),
        source,
    })?;
    let hash = ContentHash::compute(text.as_bytes());
    trust
        .accept(&rel_path, hash, now_ms)
        .map_err(|e| ComposeError::Trust(e.to_string()))?;
    Ok(ComposedArtifact {
        kind: ArtifactKind::Flow,
        path: rel_path,
        hash,
    })
}

/// Validate and install a composed **profile** as
/// `.surge/profiles/_generated/<name>.toml`.
///
/// The profile is loaded through the same strict path the registry uses, so
/// a file that would not resolve at dispatch time is refused here. Two
/// content rules are enforced on top of parsing:
///
/// - no `[verification] authority = true` (only the operator elevates a
///   profile to verifier authority);
/// - no shadowing of a bundled or home profile by name.
///
/// # Errors
/// [`ComposeError`] for a bad name, parse failure, authority claim,
/// shadowing, write failure, or a trust store that cannot be updated.
pub fn install_composed_profile(
    project_root: &Path,
    home_profiles_dir: Option<&Path>,
    trust: &mut surge_persistence::trust_store::TrustStore,
    name: &str,
    profile_toml: &str,
    now_ms: i64,
) -> Result<ComposedArtifact, ComposeError> {
    // A composed profile is a *generated* one: it lands in `_generated/`, so
    // its file name cannot collide with a hand-authored profile.
    let safe_name = name.trim();
    if safe_name.is_empty()
        || !safe_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ComposeError::BadName(name.to_string()));
    }

    // Parse through the same strict loader the registry uses: a file that
    // would fail to resolve at dispatch time must fail here.
    let parsed: surge_core::Profile =
        toml::from_str(profile_toml).map_err(|e| ComposeError::ProfileInvalid {
            name: name.to_string(),
            reason: e.to_string(),
        })?;
    if parsed.verification.authority {
        return Err(ComposeError::ProfileClaimsAuthority {
            name: name.to_string(),
        });
    }
    if let Some(layer) = shadows_existing_profile(&parsed, home_profiles_dir) {
        return Err(ComposeError::ProfileShadowsExisting {
            name: name.to_string(),
            shadowed: layer,
        });
    }

    let rel_path = RelPath::new(format!(".surge/profiles/_generated/{safe_name}.toml"))
        .map_err(|_| ComposeError::BadName(name.to_string()))?;
    let absolute = rel_path.join_onto(project_root);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ComposeError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&absolute, profile_toml).map_err(|source| ComposeError::Write {
        path: absolute.clone(),
        source,
    })?;
    let hash = ContentHash::compute(profile_toml.as_bytes());
    trust
        .accept(&rel_path, hash, now_ms)
        .map_err(|e| ComposeError::Trust(e.to_string()))?;
    Ok(ComposedArtifact {
        kind: ArtifactKind::Profile,
        path: rel_path,
        hash,
    })
}

/// `name` with a `-1.0` suffix when it does not already carry one.
fn versioned_stem(name: &str) -> String {
    let trimmed = name.trim();
    let has_version = trimmed.rsplit_once('-').is_some_and(|(_, tail)| {
        tail.split_once('.').is_some_and(|(major, minor)| {
            major.chars().all(|c| c.is_ascii_digit()) && minor.chars().all(|c| c.is_ascii_digit())
        })
    });
    if has_version {
        trimmed.to_string()
    } else {
        format!("{trimmed}-1.0")
    }
}

/// Which layer a composed profile would shadow, if any.
fn shadows_existing_profile(
    profile: &surge_core::Profile,
    home_profiles_dir: Option<&Path>,
) -> Option<&'static str> {
    let id = profile.role.id.as_str();
    if let Some(home) = home_profiles_dir {
        // The home lane's file naming is `<id>-<version>.toml` or `<id>.toml`.
        if let Ok(entries) = std::fs::read_dir(home) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if let Some(stem) = name.strip_suffix(".toml")
                    && (stem == id || stem.starts_with(&format!("{id}-")))
                {
                    return Some("home");
                }
            }
        }
    }
    if surge_core::profile::bundled::BundledRegistry::by_name_latest(id).is_some() {
        return Some("bundled");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_persistence::trust_store::TrustStore;

    fn project() -> (
        tempfile::TempDir,
        surge_persistence::trust_store::TrustStore,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let trust = TrustStore::open(dir.path(), "test-repo").unwrap();
        (dir, trust)
    }

    fn linear_3() -> Graph {
        surge_core::BundledFlows::by_name_latest("linear-3")
            .expect("bundled")
            .graph
    }

    #[test]
    fn composed_flow_installs_and_pins() {
        let (dir, mut trust) = project();
        let graph = linear_3();
        let artifact =
            install_composed_flow(dir.path(), &mut trust, "custom-review", &graph, 1).unwrap();
        assert_eq!(artifact.kind, ArtifactKind::Flow);
        assert_eq!(
            artifact.path.as_str(),
            ".surge/flows/custom-review-1.0.toml"
        );
        assert!(dir.path().join(artifact.path.as_str()).exists());
        // The install pinned the file: the T13 gate must not re-prompt.
        let content = std::fs::read(dir.path().join(artifact.path.as_str())).unwrap();
        assert!(
            trust.check(&artifact.path, &content).unwrap().is_none(),
            "installed artifacts are pinned in the same step"
        );
    }

    #[test]
    fn name_with_a_version_is_not_double_suffixed() {
        let (dir, mut trust) = project();
        let artifact =
            install_composed_flow(dir.path(), &mut trust, "bug-fix-2.1", &linear_3(), 1).unwrap();
        assert_eq!(artifact.path.as_str(), ".surge/flows/bug-fix-2.1.toml");
    }

    #[test]
    fn composed_flow_without_a_verifier_is_refused_and_not_written() {
        let (dir, mut trust) = project();
        // `single-task` has no verifier: invalid as a task flow.
        let graph = surge_core::BundledFlows::by_name_latest("single-task")
            .unwrap()
            .graph;
        let error =
            install_composed_flow(dir.path(), &mut trust, "no-verifier", &graph, 1).unwrap_err();
        assert!(matches!(error, ComposeError::FlowInvalid { .. }));
        assert!(
            !dir.path()
                .join(".surge/flows/no-verifier-1.0.toml")
                .exists(),
            "a refused flow must not reach disk"
        );
    }

    fn profile_toml(authority: bool) -> String {
        format!(
            r#"
schema_version = 1

[role]
id = "generated-reviewer"
version = "1.0.0"
display_name = "Generated Reviewer"
category = "agents"
description = "A generated profile"
when_to_use = "Generated"

[runtime]
recommended_model = "test"
agent_id = "claude-code"

[[outcomes]]
id = "reviewed"
description = "Review complete"
edge_kind_hint = "forward"

[verification]
authority = {authority}

[prompt]
system = "Review the work."
"#
        )
    }

    #[test]
    fn composed_profile_installs_under_generated() {
        let (dir, mut trust) = project();
        let artifact = install_composed_profile(
            dir.path(),
            None,
            &mut trust,
            "generated-reviewer",
            &profile_toml(false),
            1,
        )
        .unwrap();
        assert_eq!(artifact.kind, ArtifactKind::Profile);
        assert_eq!(
            artifact.path.as_str(),
            ".surge/profiles/_generated/generated-reviewer.toml"
        );
        assert!(dir.path().join(artifact.path.as_str()).exists());
    }

    #[test]
    fn profile_claiming_authority_is_refused() {
        let (dir, mut trust) = project();
        let error = install_composed_profile(
            dir.path(),
            None,
            &mut trust,
            "generated-reviewer",
            &profile_toml(true),
            1,
        )
        .unwrap_err();
        assert!(matches!(error, ComposeError::ProfileClaimsAuthority { .. }));
        assert!(
            !dir.path()
                .join(".surge/profiles/_generated/generated-reviewer.toml")
                .exists()
        );
    }

    #[test]
    fn profile_shadowing_a_bundled_name_is_refused() {
        let (dir, mut trust) = project();
        // `implementer` is bundled; a generated profile may not take the name.
        let bundled = surge_core::profile::bundled::BundledRegistry::all()
            .into_iter()
            .find(|p| p.role.id.as_str() == "implementer")
            .expect("implementer is bundled");
        let toml_text = toml::to_string(&bundled).unwrap();
        let error =
            install_composed_profile(dir.path(), None, &mut trust, "implementer", &toml_text, 1)
                .unwrap_err();
        assert!(matches!(
            error,
            ComposeError::ProfileShadowsExisting {
                shadowed: "bundled",
                ..
            }
        ));
    }

    #[test]
    fn bad_name_is_refused_before_any_write() {
        let (dir, mut trust) = project();
        let error = install_composed_profile(
            dir.path(),
            None,
            &mut trust,
            "../escape",
            &profile_toml(false),
            1,
        )
        .unwrap_err();
        assert!(matches!(error, ComposeError::BadName(_)));
    }
}
