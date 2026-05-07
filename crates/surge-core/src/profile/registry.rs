//! Pure inheritance and merge logic for profiles.
//!
//! This module is `surge-core`'s slice of the registry: no I/O, no `tokio`,
//! no filesystem access. It defines:
//!
//! - [`Provenance`] — where a resolved profile came from in the lookup chain.
//! - [`ResolvedProfile`] — a [`Profile`] together with its provenance and the
//!   ordered chain of `extends` parents that fed into the merge.
//! - [`merge_chain`] — fold a chain of profiles `[root_parent, ..., child]`
//!   into a single resolved profile using shallow-merge semantics.
//!
//! Disk-walking, `SURGE_HOME` resolution, and 3-way (`versioned → latest →
//! bundled`) lookup live in `surge-orchestrator::profile_loader`. The walker
//! that turns a `Profile` plus a registry into a chain (with cycle detection)
//! lives next door in [`crate::profile::registry::chain`].
//!
//! ## Merge semantics (shallow)
//!
//! For each top-level field of [`Profile`]:
//!
//! - `runtime` (scalars) — child wins on non-default; see [`merge_runtime`].
//! - `tools` (`default_mcp` / `default_skills` / `default_shell_allowlist`,
//!   each `Vec<String>`) — child fully replaces parent for each list when
//!   non-empty; otherwise parent wins.
//! - `outcomes` (`Vec<ProfileOutcome>`) — child fully replaces parent when
//!   non-empty; otherwise parent wins.
//! - `bindings.expected` (`Vec<ExpectedBinding>`) — merged by `name`: child
//!   overrides matching entries; parent's other entries are preserved.
//! - `hooks.entries` (`Vec<Hook>`) — union dedup by `Hook::id`; child wins
//!   on collision (logged at WARN target `profile::merge`).
//! - `prompt.system` (`String`) — child wins when non-empty.
//! - `inspector_ui.fields` (`Vec<InspectorUiField>`) — child fully replaces
//!   parent when non-empty.
//! - `sandbox` ([`SandboxConfig`](crate::sandbox::SandboxConfig)) — child
//!   wins as a whole when not equal to the default value.
//! - `approvals` ([`ApprovalConfig`](crate::approvals::ApprovalConfig)) —
//!   child wins as a whole when not equal to the default value.
//! - `schema_version` — child wins.
//! - `role` — child wins (a child profile is always its own role).

use serde::{Deserialize, Serialize};

use crate::approvals::ApprovalConfig;
use crate::error::SurgeError;
use crate::hooks::Hook;
use crate::keys::ProfileKey;
use crate::profile::{
    ExpectedBinding, InspectorUi, Profile, ProfileBindings, ProfileHooks, ProfileOutcome,
    PromptTemplate, RuntimeCfg, ToolsCfg, default_agent_id,
};
use crate::sandbox::SandboxConfig;

/// Maximum supported depth of an `extends` chain.
///
/// Set high enough that real bundled hierarchies (typically 1–2 levels) and
/// user-authored derivations have plenty of headroom, but low enough that a
/// pathological chain is rejected before it can balloon resolve cost.
pub const MAX_EXTENDS_DEPTH: usize = 8;

/// Where a resolved profile was found.
///
/// Set by `surge-orchestrator::profile_loader::ProfileRegistry::resolve` after
/// matching a `ProfileKeyRef` against the disk and bundled stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Matched a versioned file (`name-MAJOR.MINOR.toml`) on disk.
    Versioned,
    /// Matched a latest file (`name.toml`) on disk.
    Latest,
    /// Matched a bundled fallback compiled into the binary.
    Bundled,
}

/// A profile after `extends` resolution and chain merging.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProfile {
    /// The merged profile.
    pub profile: Profile,
    /// Where the leaf (child) profile was found.
    pub provenance: Provenance,
    /// Ordered chain of profile keys that participated in the merge,
    /// from the root parent to the leaf child.
    pub chain: Vec<ProfileKey>,
}

/// Walk the `extends` chain starting from `leaf`, collecting every parent
/// profile in root-first order so the result is suitable input for
/// [`merge_chain`].
///
/// `lookup` resolves a parent reference (the value of `Profile.role.extends`)
/// to the parent [`Profile`]. The closure is the `surge-core` seam through
/// which `surge-orchestrator::profile_loader::ProfileRegistry` plugs disk +
/// bundled lookup without `surge-core` knowing about either store.
///
/// The walker enforces:
///
/// - **Cycle detection.** If a profile id appears twice in the chain it
///   returns [`SurgeError::ProfileExtendsCycle`].
/// - **Depth guard.** Chains longer than [`MAX_EXTENDS_DEPTH`] return
///   [`SurgeError::ProfileExtendsTooDeep`]. Depth counts the number of
///   parents traversed: a leaf with no `extends` has depth 0.
/// - **Missing parents.** If `lookup` returns `Ok(None)` for a referenced
///   parent, the walker propagates [`SurgeError::ProfileNotFound`].
///
/// # Errors
/// See list above. Any error returned by `lookup` is propagated unchanged.
pub fn collect_chain<F>(leaf: Profile, mut lookup: F) -> Result<Vec<Profile>, SurgeError>
where
    F: FnMut(&ProfileKey) -> Result<Option<Profile>, SurgeError>,
{
    // Build the chain leaf-first, then reverse to root-first before returning.
    let mut chain: Vec<Profile> = Vec::with_capacity(2);
    let mut seen_ids: Vec<String> = Vec::with_capacity(2);

    seen_ids.push(leaf.role.id.as_str().to_string());
    chain.push(leaf);

    // The leaf occupies depth 0; every additional ancestor adds one.
    let mut depth: usize = 0;
    loop {
        // Look at the most recently appended profile's `extends`.
        let next_ref = chain
            .last()
            .expect("chain has at least the leaf at this point")
            .role
            .extends
            .clone();

        let Some(parent_key) = next_ref else {
            // No parent — chain is complete.
            break;
        };

        if depth + 1 > MAX_EXTENDS_DEPTH {
            // The extra +1 is the parent we are about to fetch.
            tracing::error!(
                target: "profile::chain",
                max = MAX_EXTENDS_DEPTH,
                chain = ?seen_ids,
                "extends chain exceeded MAX_EXTENDS_DEPTH"
            );
            return Err(SurgeError::ProfileExtendsTooDeep {
                max: MAX_EXTENDS_DEPTH,
                chain: seen_ids,
            });
        }

        let parent_id_str = parent_key.as_str().to_string();
        if seen_ids.contains(&parent_id_str) {
            // Record the cycle in observed order so the error message names
            // the path through the cycle.
            seen_ids.push(parent_id_str.clone());
            tracing::error!(
                target: "profile::chain",
                chain = ?seen_ids,
                "extends cycle detected"
            );
            return Err(SurgeError::ProfileExtendsCycle { chain: seen_ids });
        }

        let parent = lookup(&parent_key)?.ok_or_else(|| {
            tracing::error!(
                target: "profile::chain",
                missing = %parent_key,
                "extends parent not found"
            );
            SurgeError::ProfileNotFound(parent_key.as_str().to_string())
        })?;

        seen_ids.push(parent_id_str);
        chain.push(parent);
        depth += 1;
    }

    // Reverse so the root parent is at index 0 and the leaf is last —
    // exactly what `merge_chain` consumes.
    chain.reverse();

    tracing::debug!(
        target: "profile::chain",
        depth,
        chain_len = chain.len(),
        leaf = chain.last().map(|p| p.role.id.as_str()),
        "extends chain resolved"
    );

    Ok(chain)
}

/// Fold a chain of profiles `[root_parent, ..., child]` into one merged profile.
///
/// The slice is interpreted root-first: index 0 is the most senior ancestor,
/// the last index is the leaf. Each successive profile is merged on top of the
/// running result via [`merge_pair`].
///
/// # Errors
/// Returns [`SurgeError::ProfileFieldConflict`] only if a future merge step
/// detects an unrecoverable conflict. The current shallow semantics never
/// produce conflicts (collisions are resolved deterministically with `child
/// wins`), so this signature is forward-compatible.
///
/// # Panics
/// Panics if `chain` is empty — `merge_chain` requires at least the leaf
/// profile. Callers (the registry walker) must ensure non-empty input.
pub fn merge_chain(chain: &[Profile]) -> Result<Profile, SurgeError> {
    let (first, rest) = chain
        .split_first()
        .expect("merge_chain requires a non-empty profile chain");
    let mut acc = first.clone();
    for child in rest {
        acc = merge_pair(&acc, child);
    }
    Ok(acc)
}

/// Merge `child` on top of `parent` and return the merged profile.
#[must_use]
pub fn merge_pair(parent: &Profile, child: &Profile) -> Profile {
    Profile {
        schema_version: child.schema_version,
        role: child.role.clone(),
        runtime: merge_runtime(&parent.runtime, &child.runtime),
        sandbox: merge_sandbox(&parent.sandbox, &child.sandbox),
        tools: merge_tools(&parent.tools, &child.tools),
        approvals: merge_approvals(&parent.approvals, &child.approvals),
        outcomes: merge_outcomes(&parent.outcomes, &child.outcomes),
        bindings: merge_bindings(&parent.bindings, &child.bindings),
        hooks: merge_hooks(&parent.hooks, &child.hooks),
        prompt: merge_prompt(&parent.prompt, &child.prompt),
        inspector_ui: merge_inspector_ui(&parent.inspector_ui, &child.inspector_ui),
    }
}

/// Merge `RuntimeCfg`. Each field uses "child wins on non-default":
///
/// - `recommended_model`: child wins when non-empty.
/// - `default_temperature`: child wins when distinct from the serde default
///   (`0.2`); with `f32` precision via `f32::EPSILON`.
/// - `default_max_tokens`: child wins when distinct from the serde default
///   (`200_000`).
/// - `load_rules_lazily`: child `Some` wins over parent.
/// - `agent_id`: child wins when distinct from the serde default
///   (`"claude-code"`).
fn merge_runtime(parent: &RuntimeCfg, child: &RuntimeCfg) -> RuntimeCfg {
    const DEFAULT_TEMP: f32 = 0.2;
    const DEFAULT_MAX_TOKENS: u32 = 200_000;
    let default_agent = default_agent_id();

    RuntimeCfg {
        recommended_model: if child.recommended_model.is_empty() {
            parent.recommended_model.clone()
        } else {
            child.recommended_model.clone()
        },
        default_temperature: if (child.default_temperature - DEFAULT_TEMP).abs() < f32::EPSILON {
            parent.default_temperature
        } else {
            child.default_temperature
        },
        default_max_tokens: if child.default_max_tokens == DEFAULT_MAX_TOKENS {
            parent.default_max_tokens
        } else {
            child.default_max_tokens
        },
        load_rules_lazily: child.load_rules_lazily.or(parent.load_rules_lazily),
        agent_id: if child.agent_id == default_agent {
            parent.agent_id.clone()
        } else {
            child.agent_id.clone()
        },
    }
}

fn merge_sandbox(parent: &SandboxConfig, child: &SandboxConfig) -> SandboxConfig {
    if child == &SandboxConfig::default() {
        parent.clone()
    } else {
        child.clone()
    }
}

fn merge_approvals(parent: &ApprovalConfig, child: &ApprovalConfig) -> ApprovalConfig {
    if child == &ApprovalConfig::default() {
        parent.clone()
    } else {
        child.clone()
    }
}

fn merge_tools(parent: &ToolsCfg, child: &ToolsCfg) -> ToolsCfg {
    ToolsCfg {
        default_mcp: if child.default_mcp.is_empty() {
            parent.default_mcp.clone()
        } else {
            child.default_mcp.clone()
        },
        default_skills: if child.default_skills.is_empty() {
            parent.default_skills.clone()
        } else {
            child.default_skills.clone()
        },
        default_shell_allowlist: if child.default_shell_allowlist.is_empty() {
            parent.default_shell_allowlist.clone()
        } else {
            child.default_shell_allowlist.clone()
        },
    }
}

fn merge_outcomes(parent: &[ProfileOutcome], child: &[ProfileOutcome]) -> Vec<ProfileOutcome> {
    if child.is_empty() {
        parent.to_vec()
    } else {
        child.to_vec()
    }
}

fn merge_bindings(parent: &ProfileBindings, child: &ProfileBindings) -> ProfileBindings {
    let mut merged: Vec<ExpectedBinding> = parent.expected.clone();
    for child_entry in &child.expected {
        if let Some(slot) = merged.iter_mut().find(|p| p.name == child_entry.name) {
            *slot = child_entry.clone();
        } else {
            merged.push(child_entry.clone());
        }
    }
    ProfileBindings { expected: merged }
}

fn merge_hooks(parent: &ProfileHooks, child: &ProfileHooks) -> ProfileHooks {
    let mut merged: Vec<Hook> = parent.entries.clone();
    for child_hook in &child.entries {
        if let Some(slot) = merged.iter_mut().find(|p| p.id == child_hook.id) {
            tracing::warn!(
                target: "profile::merge",
                hook_id = %child_hook.id,
                "child hook id collides with parent; child wins"
            );
            *slot = child_hook.clone();
        } else {
            merged.push(child_hook.clone());
        }
    }
    ProfileHooks { entries: merged }
}

fn merge_prompt(parent: &PromptTemplate, child: &PromptTemplate) -> PromptTemplate {
    PromptTemplate {
        system: if child.system.is_empty() {
            parent.system.clone()
        } else {
            child.system.clone()
        },
    }
}

fn merge_inspector_ui(parent: &InspectorUi, child: &InspectorUi) -> InspectorUi {
    if child.fields.is_empty() {
        InspectorUi {
            fields: parent.fields.clone(),
        }
    } else {
        InspectorUi {
            fields: child.fields.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::hooks::{HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
    use crate::keys::OutcomeKey;
    use crate::profile::{
        ExpectedBindingSource, InspectorFieldKind, InspectorUiField, Role, RoleCategory,
    };
    use crate::sandbox::SandboxMode;

    fn make_profile(name: &str) -> Profile {
        Profile {
            schema_version: 1,
            role: Role {
                id: ProfileKey::try_from(name).unwrap(),
                version: semver::Version::new(1, 0, 0),
                display_name: name.into(),
                icon: None,
                category: RoleCategory::Agents,
                description: format!("desc {name}"),
                when_to_use: format!("when {name}"),
                extends: None,
            },
            runtime: RuntimeCfg {
                recommended_model: "claude-opus-4-7".into(),
                default_temperature: 0.2,
                default_max_tokens: 200_000,
                load_rules_lazily: None,
                agent_id: default_agent_id(),
            },
            sandbox: SandboxConfig::default(),
            tools: ToolsCfg::default(),
            approvals: ApprovalConfig::default(),
            outcomes: vec![ProfileOutcome {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "Success".into(),
                edge_kind_hint: EdgeKind::Forward,
                required_artifacts: vec![],
            }],
            bindings: ProfileBindings::default(),
            hooks: ProfileHooks::default(),
            prompt: PromptTemplate {
                system: format!("system {name}"),
            },
            inspector_ui: InspectorUi::default(),
        }
    }

    fn hook(id: &str) -> Hook {
        Hook {
            id: id.into(),
            trigger: HookTrigger::PreToolUse,
            matcher: MatcherSpec::default(),
            command: format!("cmd {id}"),
            on_failure: HookFailureMode::default(),
            timeout_seconds: None,
            inherit: HookInheritance::default(),
        }
    }

    #[test]
    fn merge_chain_singleton_returns_clone() {
        let p = make_profile("solo");
        let merged = merge_chain(std::slice::from_ref(&p)).unwrap();
        assert_eq!(merged, p);
    }

    #[test]
    #[should_panic(expected = "merge_chain requires a non-empty profile chain")]
    fn merge_chain_empty_panics() {
        let _ = merge_chain(&[]);
    }

    #[test]
    fn child_role_wins() {
        let parent = make_profile("parent");
        let child = make_profile("child");
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.role.id.as_str(), "child");
    }

    #[test]
    fn child_prompt_wins_when_non_empty() {
        let parent = make_profile("parent");
        let mut child = make_profile("child");
        child.prompt.system = "child prompt".into();
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.prompt.system, "child prompt");
    }

    #[test]
    fn empty_child_prompt_falls_back_to_parent() {
        let mut parent = make_profile("parent");
        parent.prompt.system = "parent prompt".into();
        let mut child = make_profile("child");
        child.prompt.system = String::new();
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.prompt.system, "parent prompt");
    }

    #[test]
    fn child_outcomes_fully_replace_parent() {
        let parent = make_profile("parent");
        let mut child = make_profile("child");
        child.outcomes = vec![ProfileOutcome {
            id: OutcomeKey::try_from("rejected").unwrap(),
            description: "Rejected".into(),
            edge_kind_hint: EdgeKind::Backtrack,
            required_artifacts: vec![],
        }];
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.outcomes.len(), 1);
        assert_eq!(merged.outcomes[0].id.as_str(), "rejected");
    }

    #[test]
    fn empty_child_outcomes_fall_back_to_parent() {
        let parent = make_profile("parent");
        let mut child = make_profile("child");
        child.outcomes = vec![];
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.outcomes.len(), 1);
        assert_eq!(merged.outcomes[0].id.as_str(), "done");
    }

    #[test]
    fn tools_each_list_replaces_independently() {
        let mut parent = make_profile("parent");
        parent.tools.default_mcp = vec!["fs".into()];
        parent.tools.default_skills = vec!["debugging".into()];
        parent.tools.default_shell_allowlist = vec!["ls".into()];

        let mut child = make_profile("child");
        child.tools.default_mcp = vec!["github".into()];
        // child leaves default_skills empty → should inherit parent's
        // child overrides default_shell_allowlist with empty set: stays parent's

        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.tools.default_mcp, vec!["github".to_string()]);
        assert_eq!(merged.tools.default_skills, vec!["debugging".to_string()]);
        assert_eq!(merged.tools.default_shell_allowlist, vec!["ls".to_string()]);
    }

    #[test]
    fn bindings_merged_by_name_child_wins() {
        let mut parent = make_profile("parent");
        parent.bindings.expected = vec![
            ExpectedBinding {
                name: "spec".into(),
                source: ExpectedBindingSource::Any,
                optional: false,
            },
            ExpectedBinding {
                name: "context".into(),
                source: ExpectedBindingSource::RunArtifact,
                optional: true,
            },
        ];
        let mut child = make_profile("child");
        child.bindings.expected = vec![ExpectedBinding {
            name: "spec".into(),
            source: ExpectedBindingSource::RunArtifact,
            optional: true,
        }];

        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.bindings.expected.len(), 2);
        let spec = merged
            .bindings
            .expected
            .iter()
            .find(|b| b.name == "spec")
            .unwrap();
        assert_eq!(spec.source, ExpectedBindingSource::RunArtifact);
        assert!(spec.optional);
        let ctx = merged
            .bindings
            .expected
            .iter()
            .find(|b| b.name == "context")
            .unwrap();
        // parent entry preserved untouched
        assert_eq!(ctx.source, ExpectedBindingSource::RunArtifact);
    }

    #[test]
    fn hooks_unioned_with_child_winning_on_collision() {
        let mut parent = make_profile("parent");
        parent.hooks.entries = vec![hook("a"), hook("b")];
        let mut child = make_profile("child");
        let mut overriding_a = hook("a");
        overriding_a.command = "child cmd a".into();
        child.hooks.entries = vec![overriding_a, hook("c")];

        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.hooks.entries.len(), 3);
        let a = merged.hooks.entries.iter().find(|h| h.id == "a").unwrap();
        assert_eq!(a.command, "child cmd a");
        assert!(merged.hooks.entries.iter().any(|h| h.id == "b"));
        assert!(merged.hooks.entries.iter().any(|h| h.id == "c"));
    }

    #[test]
    fn sandbox_child_overrides_when_non_default() {
        let parent = make_profile("parent");
        let mut child = make_profile("child");
        child.sandbox = SandboxConfig {
            mode: SandboxMode::WorkspaceWrite,
            ..SandboxConfig::default()
        };
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.sandbox.mode, SandboxMode::WorkspaceWrite);
    }

    #[test]
    fn sandbox_falls_back_to_parent_when_child_default() {
        let mut parent = make_profile("parent");
        parent.sandbox = SandboxConfig {
            mode: SandboxMode::WorkspaceNetwork,
            ..SandboxConfig::default()
        };
        let child = make_profile("child");
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.sandbox.mode, SandboxMode::WorkspaceNetwork);
    }

    #[test]
    fn runtime_recommended_model_child_wins_when_non_empty() {
        let mut parent = make_profile("parent");
        parent.runtime.recommended_model = "parent-model".into();
        let mut child = make_profile("child");
        child.runtime.recommended_model = "child-model".into();
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.runtime.recommended_model, "child-model");
    }

    #[test]
    fn runtime_recommended_model_parent_wins_when_child_empty() {
        let mut parent = make_profile("parent");
        parent.runtime.recommended_model = "parent-model".into();
        let mut child = make_profile("child");
        child.runtime.recommended_model = String::new();
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.runtime.recommended_model, "parent-model");
    }

    #[test]
    fn runtime_max_tokens_parent_wins_when_child_default() {
        let mut parent = make_profile("parent");
        parent.runtime.default_max_tokens = 50_000;
        let child = make_profile("child"); // default 200_000
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.runtime.default_max_tokens, 50_000);
    }

    #[test]
    fn runtime_agent_id_child_wins_when_non_default() {
        let mut parent = make_profile("parent");
        parent.runtime.agent_id = "claude-code".into(); // default
        let mut child = make_profile("child");
        child.runtime.agent_id = "codex".into();
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.runtime.agent_id, "codex");
    }

    #[test]
    fn runtime_agent_id_parent_wins_when_child_default() {
        let mut parent = make_profile("parent");
        parent.runtime.agent_id = "mock".into();
        let child = make_profile("child"); // default "claude-code"
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.runtime.agent_id, "mock");
    }

    #[test]
    fn inspector_ui_fields_child_replaces_when_non_empty() {
        let mut parent = make_profile("parent");
        parent.inspector_ui.fields = vec![InspectorUiField {
            id: "p_field".into(),
            label: "P".into(),
            kind: InspectorFieldKind::Toggle,
            default: None,
            help: None,
        }];
        let mut child = make_profile("child");
        child.inspector_ui.fields = vec![InspectorUiField {
            id: "c_field".into(),
            label: "C".into(),
            kind: InspectorFieldKind::Toggle,
            default: None,
            help: None,
        }];
        let merged = merge_pair(&parent, &child);
        assert_eq!(merged.inspector_ui.fields.len(), 1);
        assert_eq!(merged.inspector_ui.fields[0].id, "c_field");
    }

    #[test]
    fn three_step_chain_folds_left_to_right() {
        let mut grandparent = make_profile("gp");
        grandparent.runtime.recommended_model = "gp-model".into();
        grandparent.tools.default_mcp = vec!["fs".into()];
        grandparent.hooks.entries = vec![hook("gp_hook")];

        let mut parent = make_profile("parent");
        parent.runtime.recommended_model = String::new(); // inherit gp
        parent.tools.default_skills = vec!["debugging".into()];
        parent.hooks.entries = vec![hook("parent_hook")];

        let mut child = make_profile("child");
        child.runtime.recommended_model = String::new(); // inherit
        // child has no tools — should inherit parent.default_skills + gp.default_mcp
        child.hooks.entries = vec![hook("child_hook")];

        let merged = merge_chain(&[grandparent, parent, child]).unwrap();
        assert_eq!(merged.runtime.recommended_model, "gp-model");
        assert_eq!(merged.tools.default_mcp, vec!["fs".to_string()]);
        assert_eq!(merged.tools.default_skills, vec!["debugging".to_string()]);
        let hook_ids: Vec<&str> = merged
            .hooks
            .entries
            .iter()
            .map(|h| h.id.as_str())
            .collect();
        assert_eq!(hook_ids, vec!["gp_hook", "parent_hook", "child_hook"]);
        // role of leaf wins
        assert_eq!(merged.role.id.as_str(), "child");
    }

    #[test]
    fn provenance_serializes_with_snake_case() {
        assert_eq!(
            serde_json::to_string(&Provenance::Versioned).unwrap(),
            "\"versioned\""
        );
        assert_eq!(
            serde_json::to_string(&Provenance::Latest).unwrap(),
            "\"latest\""
        );
        assert_eq!(
            serde_json::to_string(&Provenance::Bundled).unwrap(),
            "\"bundled\""
        );
    }

    // ── collect_chain tests ────────────────────────────────────────

    fn with_extends(name: &str, parent: Option<&str>) -> Profile {
        let mut p = make_profile(name);
        p.role.extends = parent.map(|p| ProfileKey::try_from(p).unwrap());
        p
    }

    #[test]
    fn collect_chain_leaf_only() {
        let leaf = make_profile("leaf");
        let chain = collect_chain(leaf, |_| {
            panic!("lookup must not be invoked when leaf has no extends");
        })
        .unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].role.id.as_str(), "leaf");
    }

    #[test]
    fn collect_chain_two_levels_root_first() {
        let leaf = with_extends("child", Some("base"));
        let base = make_profile("base");
        let chain = collect_chain(leaf, move |key| {
            if key.as_str() == "base" {
                Ok(Some(base.clone()))
            } else {
                Ok(None)
            }
        })
        .unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].role.id.as_str(), "base");
        assert_eq!(chain[1].role.id.as_str(), "child");
    }

    #[test]
    fn collect_chain_three_levels_root_first() {
        let leaf = with_extends("child", Some("mid"));
        let mid = with_extends("mid", Some("root"));
        let root = make_profile("root");
        let chain = collect_chain(leaf, move |key| match key.as_str() {
            "mid" => Ok(Some(mid.clone())),
            "root" => Ok(Some(root.clone())),
            _ => Ok(None),
        })
        .unwrap();
        let names: Vec<&str> = chain.iter().map(|p| p.role.id.as_str()).collect();
        assert_eq!(names, vec!["root", "mid", "child"]);
    }

    #[test]
    fn collect_chain_missing_parent_is_profile_not_found() {
        let leaf = with_extends("child", Some("missing"));
        let err = collect_chain(leaf, |_| Ok(None)).unwrap_err();
        match err {
            SurgeError::ProfileNotFound(key) => assert_eq!(key, "missing"),
            other => panic!("expected ProfileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn collect_chain_self_cycle_detected() {
        // A profile that extends itself.
        let leaf = with_extends("loop_a", Some("loop_a"));
        let leaf_for_lookup = leaf.clone();
        let err = collect_chain(leaf, move |key| {
            if key.as_str() == "loop_a" {
                Ok(Some(leaf_for_lookup.clone()))
            } else {
                Ok(None)
            }
        })
        .unwrap_err();
        assert!(matches!(err, SurgeError::ProfileExtendsCycle { .. }));
    }

    #[test]
    fn collect_chain_multi_step_cycle_detected() {
        // child -> mid -> child (cycle through mid back to child)
        let leaf = with_extends("child", Some("mid"));
        let leaf_for_lookup = leaf.clone();
        let mid = with_extends("mid", Some("child"));
        let err = collect_chain(leaf, move |key| match key.as_str() {
            "mid" => Ok(Some(mid.clone())),
            "child" => Ok(Some(leaf_for_lookup.clone())),
            _ => Ok(None),
        })
        .unwrap_err();
        match err {
            SurgeError::ProfileExtendsCycle { chain } => {
                assert!(chain.iter().any(|s| s == "child"));
                assert!(chain.iter().any(|s| s == "mid"));
            },
            other => panic!("expected ProfileExtendsCycle, got {other:?}"),
        }
    }

    #[test]
    fn collect_chain_depth_guard_triggers_at_max_plus_one() {
        // Build chain of MAX_EXTENDS_DEPTH + 2 distinct levels:
        // leaf -> p1 -> p2 -> ... -> p(MAX_EXTENDS_DEPTH + 1)
        let total = MAX_EXTENDS_DEPTH + 2;
        let mut profiles: Vec<Profile> = Vec::with_capacity(total);
        for i in 0..total {
            let parent_name = if i + 1 < total {
                Some(format!("p{}", i + 1))
            } else {
                None
            };
            let name = if i == 0 {
                "leaf".to_string()
            } else {
                format!("p{i}")
            };
            profiles.push(with_extends(&name, parent_name.as_deref()));
        }
        let leaf = profiles[0].clone();
        let lookup_table: std::collections::HashMap<String, Profile> = profiles
            .iter()
            .map(|p| (p.role.id.as_str().to_string(), p.clone()))
            .collect();
        let err = collect_chain(leaf, move |key| {
            Ok(lookup_table.get(key.as_str()).cloned())
        })
        .unwrap_err();
        assert!(matches!(err, SurgeError::ProfileExtendsTooDeep { .. }));
    }

    #[test]
    fn collect_chain_at_exact_max_depth_succeeds() {
        // Build chain of MAX_EXTENDS_DEPTH + 1 levels (depth == MAX_EXTENDS_DEPTH):
        // leaf -> p1 -> p2 -> ... -> p(MAX_EXTENDS_DEPTH)
        let total = MAX_EXTENDS_DEPTH + 1;
        let mut profiles: Vec<Profile> = Vec::with_capacity(total);
        for i in 0..total {
            let parent_name = if i + 1 < total {
                Some(format!("p{}", i + 1))
            } else {
                None
            };
            let name = if i == 0 {
                "leaf".to_string()
            } else {
                format!("p{i}")
            };
            profiles.push(with_extends(&name, parent_name.as_deref()));
        }
        let leaf = profiles[0].clone();
        let lookup_table: std::collections::HashMap<String, Profile> = profiles
            .iter()
            .map(|p| (p.role.id.as_str().to_string(), p.clone()))
            .collect();
        let chain = collect_chain(leaf, move |key| {
            Ok(lookup_table.get(key.as_str()).cloned())
        })
        .unwrap();
        assert_eq!(chain.len(), total);
        assert_eq!(chain.first().unwrap().role.id.as_str(), &format!("p{MAX_EXTENDS_DEPTH}"));
        assert_eq!(chain.last().unwrap().role.id.as_str(), "leaf");
    }

    #[test]
    fn collect_chain_then_merge_chain_full_round_trip() {
        // child overrides parent prompt; root provides default tools
        let mut root = make_profile("root");
        root.tools.default_mcp = vec!["fs".into()];

        let mut mid = with_extends("mid", Some("root"));
        mid.prompt.system = "mid prompt".into();

        let mut child = with_extends("child", Some("mid"));
        child.prompt.system = "child prompt".into();

        let chain = collect_chain(child, move |key| match key.as_str() {
            "mid" => Ok(Some(mid.clone())),
            "root" => Ok(Some(root.clone())),
            _ => Ok(None),
        })
        .unwrap();
        let merged = merge_chain(&chain).unwrap();
        assert_eq!(merged.role.id.as_str(), "child");
        assert_eq!(merged.prompt.system, "child prompt");
        assert_eq!(merged.tools.default_mcp, vec!["fs".to_string()]);
    }

    #[test]
    fn collect_chain_propagates_lookup_error() {
        let leaf = with_extends("child", Some("base"));
        let err = collect_chain(leaf, |_| {
            Err(SurgeError::Config("boom".into()))
        })
        .unwrap_err();
        assert!(matches!(err, SurgeError::Config(msg) if msg == "boom"));
    }
}
