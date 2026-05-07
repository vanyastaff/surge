//! Property tests + insta snapshots for `surge_core::profile::registry`.
//!
//! Lives under `tests/` (integration) so it consumes the public API the same
//! way `surge-orchestrator::profile_loader` will.

use proptest::prelude::*;

use surge_core::edge::EdgeKind;
use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
use surge_core::keys::{OutcomeKey, ProfileKey};
use surge_core::profile::registry::{collect_chain, merge_chain, merge_pair};
use surge_core::profile::{
    ExpectedBinding, ExpectedBindingSource, Profile, ProfileBindings, ProfileHooks, ProfileOutcome,
    PromptTemplate, Role, RoleCategory, RuntimeCfg, ToolsCfg,
};
use surge_core::sandbox::SandboxConfig;

// ── Profile factories ──────────────────────────────────────────────

fn make_profile(name: &str) -> Profile {
    Profile {
        schema_version: 1,
        role: Role {
            id: ProfileKey::try_from(name).expect("test profile name must satisfy ProfileKey"),
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
            agent_id: "claude-code".into(),
        },
        sandbox: SandboxConfig::default(),
        tools: ToolsCfg::default(),
        approvals: surge_core::approvals::ApprovalConfig::default(),
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
        inspector_ui: surge_core::profile::InspectorUi::default(),
    }
}

fn make_hook(id: &str) -> Hook {
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

// ── Insta snapshots: representative merged profiles ────────────────

#[test]
fn snapshot_two_step_chain_with_overrides() {
    let mut root = make_profile("root");
    root.tools.default_mcp = vec!["fs".into(), "github".into()];
    root.hooks.entries = vec![make_hook("audit")];

    let mut child = make_profile("child");
    child.role.extends = Some(ProfileKey::try_from("root").unwrap());
    child.prompt.system = "child system prompt".into();
    child.runtime.agent_id = "codex".into();
    child.bindings.expected = vec![ExpectedBinding {
        name: "spec".into(),
        source: ExpectedBindingSource::RunArtifact,
        optional: false,
    }];

    let chain = vec![root, child];
    let merged = merge_chain(&chain).unwrap();
    let toml = toml::to_string(&merged).unwrap();
    insta::assert_snapshot!("merged_two_step_chain", toml);
}

#[test]
fn snapshot_three_step_chain_with_hook_dedup() {
    let mut gp = make_profile("gp");
    gp.hooks.entries = vec![make_hook("a"), make_hook("b")];

    let mut parent = make_profile("parent");
    parent.role.extends = Some(ProfileKey::try_from("gp").unwrap());
    let mut overriding_a = make_hook("a");
    overriding_a.command = "parent overrode a".into();
    parent.hooks.entries = vec![overriding_a, make_hook("c")];

    let mut child = make_profile("child");
    child.role.extends = Some(ProfileKey::try_from("parent").unwrap());
    child.hooks.entries = vec![make_hook("d")];

    let chain = vec![gp, parent, child];
    let merged = merge_chain(&chain).unwrap();
    let toml = toml::to_string(&merged).unwrap();
    insta::assert_snapshot!("merged_three_step_with_dedup", toml);
}

// ── Property tests ─────────────────────────────────────────────────

fn arb_profile_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,15}".prop_filter("non-empty after charset", |s| !s.is_empty())
}

fn arb_simple_profile(name: String) -> impl Strategy<Value = Profile> {
    (
        any::<bool>(),                             // has_custom_prompt
        prop::collection::vec("[a-z]{1,8}", 0..3), // mcp tools
        any::<bool>(),                             // overrides agent_id
    )
        .prop_map(move |(has_prompt, mcp, override_agent)| {
            let mut p = make_profile(&name);
            if has_prompt {
                p.prompt.system = format!("custom prompt for {name}");
            } else {
                p.prompt.system = String::new();
            }
            p.tools.default_mcp = mcp;
            if override_agent {
                p.runtime.agent_id = "codex".into();
            }
            p
        })
}

proptest! {
    /// Merging always preserves the leaf's role.id.
    #[test]
    fn merge_pair_preserves_child_role(
        parent_name in arb_profile_name(),
        child_name in arb_profile_name(),
    ) {
        prop_assume!(parent_name != child_name);
        let parent = make_profile(&parent_name);
        let child = make_profile(&child_name);
        let merged = merge_pair(&parent, &child);
        prop_assert_eq!(merged.role.id.as_str(), child_name.as_str());
    }

    /// Hook ids in a merged chain are always unique.
    #[test]
    fn merge_chain_hook_ids_are_unique(
        n_parent in 0usize..5,
        n_child in 0usize..5,
        overlap in 0usize..3,
    ) {
        let mut parent = make_profile("p");
        for i in 0..n_parent {
            parent.hooks.entries.push(make_hook(&format!("h{i}")));
        }
        let mut child = make_profile("c");
        // Child reuses the first `overlap` hook ids from parent.
        let overlap = overlap.min(n_parent);
        for i in 0..overlap {
            let mut h = make_hook(&format!("h{i}"));
            h.command = "child override".into();
            child.hooks.entries.push(h);
        }
        for i in 0..n_child {
            child.hooks.entries.push(make_hook(&format!("c{i}")));
        }

        let merged = merge_pair(&parent, &child);
        let ids: Vec<&str> = merged.hooks.entries.iter().map(|h| h.id.as_str()).collect();
        let mut deduped = ids.clone();
        deduped.sort_unstable();
        deduped.dedup();
        prop_assert_eq!(ids.len(), deduped.len());

        // Overrides took effect: every overlap id has child's command.
        for i in 0..overlap {
            let id = format!("h{i}");
            let h = merged.hooks.entries.iter().find(|h| h.id == id).unwrap();
            prop_assert_eq!(&h.command, "child override");
        }
    }

    /// merge_chain on a singleton equals identity.
    #[test]
    fn merge_chain_singleton_is_identity(name in arb_profile_name()) {
        let p = make_profile(&name);
        let merged = merge_chain(std::slice::from_ref(&p)).unwrap();
        prop_assert_eq!(&merged, &p);
    }

    /// Three-step merge: empty-prompt child inherits from middle inherits from root.
    #[test]
    fn three_step_empty_child_prompt_inherits_first_non_empty(
        root_prompt in any::<bool>(),
        mid_prompt in any::<bool>(),
    ) {
        let mut root = make_profile("root");
        root.prompt.system = if root_prompt { "root prompt".into() } else { String::new() };

        let mut mid = make_profile("mid");
        mid.role.extends = Some(ProfileKey::try_from("root").unwrap());
        mid.prompt.system = if mid_prompt { "mid prompt".into() } else { String::new() };

        let mut child = make_profile("child");
        child.role.extends = Some(ProfileKey::try_from("mid").unwrap());
        child.prompt.system = String::new();

        let chain = vec![root, mid, child];
        let merged = merge_chain(&chain).unwrap();
        let expected = if mid_prompt {
            "mid prompt"
        } else if root_prompt {
            "root prompt"
        } else {
            ""
        };
        prop_assert_eq!(merged.prompt.system, expected);
    }

    /// collect_chain preserves total order: leaf is always last.
    #[test]
    fn collect_chain_leaf_is_last(
        depth in 1usize..6,
    ) {
        // Build chain: leaf -> p1 -> ... -> p(depth-1)
        let mut profiles: Vec<Profile> = Vec::new();
        for i in 0..depth {
            let name = if i == 0 { "leaf".to_string() } else { format!("p{i}") };
            let parent = if i + 1 < depth { Some(format!("p{}", i + 1)) } else { None };
            let mut p = make_profile(&name);
            p.role.extends = parent.map(|x| ProfileKey::try_from(x.as_str()).unwrap());
            profiles.push(p);
        }
        let leaf = profiles[0].clone();
        let table: std::collections::HashMap<String, Profile> = profiles
            .iter()
            .map(|p| (p.role.id.as_str().to_string(), p.clone()))
            .collect();
        let chain = collect_chain(leaf, |key| Ok(table.get(key.as_str()).cloned())).unwrap();
        prop_assert_eq!(chain.len(), depth);
        prop_assert_eq!(chain.last().unwrap().role.id.as_str(), "leaf");
    }
}

prop_compose! {
    fn arb_profile_with_name()(name in arb_profile_name()) -> Profile {
        make_profile(&name)
    }
}

proptest! {
    /// Merging is idempotent in the sense that merging child onto child equals child
    /// (modulo role: child role wins anyway).
    #[test]
    fn merge_pair_self_is_identity(name in arb_profile_name()) {
        let p = make_profile(&name);
        let merged = merge_pair(&p, &p);
        prop_assert_eq!(merged, p);
    }
}

#[allow(dead_code)]
fn _unused_ensures_arb_simple_profile_compiles() {
    let _ = arb_simple_profile("x".into());
}
