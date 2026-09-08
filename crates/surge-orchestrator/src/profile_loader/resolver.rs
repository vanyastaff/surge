//! `surge_core::ReferenceResolver` impl for [`ProfileRegistry`].
//!
//! This is the seam graph validation (`surge_core::validate_with_resolver`)
//! uses to resolve profile references and agent-runtime identity against
//! the project's real profile registry, instead of a test double. Before
//! this adapter existed, every `ReferenceResolver` implementor in the
//! workspace was test-only — see `engine/validate.rs`'s
//! `validate_for_m6_with_resolver` doc comment for that history.
//!
//! `template_exists` / `named_agent_exists` answer permissively (`true`):
//! `ProfileRegistry` has no template or named-agent registry to consult, so
//! answering anything else would be inventing a rejection this registry
//! cannot back (ticket 22 tracks building those registries). This adapter
//! revives exactly two of the resolver-backed rules —
//! `ValidationErrorKind::ProfileNotFound` and
//! `ValidationErrorKind::SameRuntimeVerification` — not all three.

use surge_core::ReferenceResolver;
use surge_core::parse_key_ref;

use super::ProfileRegistry;

impl ReferenceResolver for ProfileRegistry {
    fn profile_exists(&self, name: &str) -> bool {
        let Ok(key_ref) = parse_key_ref(name) else {
            return false;
        };
        self.resolve(&key_ref).is_ok()
    }

    fn template_exists(&self, _name: &str) -> bool {
        // No template registry backs `ProfileRegistry` (ticket 22) —
        // permissive is the honest answer, not a lie a real check papers
        // over.
        true
    }

    fn named_agent_exists(&self, _id: &str) -> bool {
        // Same reasoning as `template_exists`: no named-agent registry yet.
        true
    }

    fn profile_runtime(&self, name: &str) -> Option<String> {
        // Reuse the one place the engine itself normalizes a profile's
        // `runtime.agent_id` through `surge_acp::Registry`'s alias table
        // (`resolve_profile_runtime_id` / `canonical_runtime_id_for`,
        // `engine::stage::agent`) — not `resolved.profile.runtime.agent_id`
        // verbatim. `surge_acp::REGISTRY_ID_ALIASES` maps several spellings
        // to one runtime id (`"claude"` and `"claude-code"` both →
        // `"claude-acp"`; `"codex"` and `"codex-cli"` both → `"codex-acp"`),
        // so two profiles naming the same runtime under different aliases
        // must resolve to the same `String` here — the whole point of
        // `ValidationErrorKind::SameRuntimeVerification` (W5) existing.
        crate::engine::stage::agent::resolve_profile_runtime_id(Some(self), name)
            .map(crate::engine::capacity::CanonicalRuntimeId::into_string)
    }
}

#[cfg(test)]
mod tests {
    use surge_core::ReferenceResolver;
    use surge_core::keys::{NodeKey, OutcomeKey};
    use surge_core::node::NodeConfig;
    use tempfile::TempDir;

    use super::ProfileRegistry;

    fn registry_with_disk(dir: &std::path::Path) -> ProfileRegistry {
        let disk = crate::profile_loader::DiskProfileSet::scan(dir).unwrap();
        ProfileRegistry::new(disk)
    }

    #[test]
    fn profile_exists_true_for_bundled_profile() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        assert!(reg.profile_exists("implementer@1.0"));
    }

    #[test]
    fn profile_exists_false_for_unknown_profile() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        assert!(!reg.profile_exists("definitely-not-a-real-profile"));
    }

    #[test]
    fn profile_exists_false_for_unparseable_reference() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        assert!(!reg.profile_exists("@bad@ref@"));
    }

    #[test]
    fn profile_runtime_resolves_bundled_agent_id() {
        // `implementer-1.0.toml` declares `agent_id = "claude-code"`, which
        // is a `surge_acp::REGISTRY_ID_ALIASES` alias, not a registered
        // runtime id itself — the real registry normalizes it to
        // `"claude-acp"` (F1: this used to assert the raw, un-normalized
        // `"claude-code"`, which is exactly the value two differently
        // spelled same-runtime profiles would fail to compare equal on).
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        assert_eq!(
            reg.profile_runtime("implementer@1.0").as_deref(),
            Some("claude-acp")
        );
    }

    /// A minimal on-disk profile TOML with a caller-chosen role id and
    /// `runtime.agent_id`, for tests that need a profile the bundled set
    /// does not ship (e.g. `agent_id = "claude"` — none of the bundled
    /// profiles use that particular alias spelling).
    fn minimal_profile_toml(role_id: &str, agent_id: &str) -> String {
        format!(
            r#"
schema_version = 1
outcomes = []

[role]
id = "{role_id}"
version = "1.0.0"
display_name = "{role_id}"
category = "agents"
description = "resolver.rs composition-test fixture"
when_to_use = "surge-orchestrator profile_loader::resolver tests only"

[runtime]
recommended_model = "claude-opus-4-7"
agent_id = "{agent_id}"

[prompt]
system = "test fixture prompt"
"#
        )
    }

    fn agent_node(
        key: &str,
        profile: &str,
        ledger_effect: surge_core::LedgerEffect,
    ) -> surge_core::node::Node {
        surge_core::node::Node {
            id: NodeKey::try_from(key).unwrap(),
            position: surge_core::node::Position::default(),
            declared_outcomes: vec![surge_core::node::OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "resolver.rs composition-test fixture outcome".into(),
                edge_kind_hint: surge_core::edge::EdgeKind::Forward,
                is_terminal: false,
                ledger_effect,
            }],
            config: NodeConfig::Agent(surge_core::agent_config::AgentConfig {
                profile: surge_core::keys::ProfileKey::try_from(profile).unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: surge_core::agent_config::NodeLimits::default(),
                hooks: vec![],
                custom_fields: std::collections::BTreeMap::default(),
            }),
        }
    }

    /// A `impl_1 -> verify_1 -> end` graph: `impl_1` declares
    /// `ReadyForVerification` on `impl_profile`, `verify_1` declares
    /// `Verified` on `verify_profile` and forwards to a `Terminal{Success}`.
    /// Exactly the shape W5 (`SameRuntimeVerification`) looks for, and
    /// structurally complete (no dangling outcome, a reachable terminal) so
    /// `validate_with_resolver`'s `Err` arm — when it fires — carries only
    /// the finding under test, not incidental structural noise.
    fn graph_impl_then_verify(
        impl_profile: &str,
        verify_profile: &str,
    ) -> surge_core::graph::Graph {
        use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
        use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
        use surge_core::keys::EdgeKey;
        use surge_core::terminal_config::{TerminalConfig, TerminalKind};

        let impl_key = NodeKey::try_from("impl_1").unwrap();
        let verify_key = NodeKey::try_from("verify_1").unwrap();
        let end_key = NodeKey::try_from("end").unwrap();

        let mut nodes = std::collections::BTreeMap::new();
        nodes.insert(
            impl_key.clone(),
            agent_node(
                "impl_1",
                impl_profile,
                surge_core::LedgerEffect::ReadyForVerification,
            ),
        );
        nodes.insert(
            verify_key.clone(),
            agent_node(
                "verify_1",
                verify_profile,
                surge_core::LedgerEffect::Verified,
            ),
        );
        nodes.insert(
            end_key.clone(),
            surge_core::node::Node {
                id: end_key.clone(),
                position: surge_core::node::Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );

        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "resolver-w5-fixture".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: impl_key.clone(),
            nodes,
            edges: vec![
                Edge {
                    id: EdgeKey::try_from("e1").unwrap(),
                    from: PortRef {
                        node: impl_key,
                        outcome: OutcomeKey::try_from("done").unwrap(),
                    },
                    to: verify_key.clone(),
                    kind: EdgeKind::Forward,
                    policy: EdgePolicy::default(),
                },
                Edge {
                    id: EdgeKey::try_from("e2").unwrap(),
                    from: PortRef {
                        node: verify_key,
                        outcome: OutcomeKey::try_from("done").unwrap(),
                    },
                    to: end_key,
                    kind: EdgeKind::Forward,
                    policy: EdgePolicy::default(),
                },
            ],
            subgraphs: std::collections::BTreeMap::new(),
        }
    }

    /// T1 (first case) — the composition path was untested: every W5 test
    /// elsewhere used a hand-written fake resolver, which is exactly how F1
    /// (raw `agent_id`, no alias normalization) went unnoticed. This test
    /// goes through the REAL `ProfileRegistry`: two bundled profiles
    /// (`implementer@1.0`, `verifier@1.0`), both declaring
    /// `agent_id = "claude-code"` on disk.
    #[test]
    fn same_runtime_verification_fires_through_the_real_profile_registry() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        let g = graph_impl_then_verify("implementer@1.0", "verifier@1.0");

        let findings = match surge_core::validate_with_resolver(&g, &reg) {
            Ok(warnings) => warnings,
            Err(errs) => errs,
        };
        match findings
            .iter()
            .find(|f| {
                matches!(
                    f.kind,
                    surge_core::ValidationErrorKind::SameRuntimeVerification { .. }
                )
            })
            .map(|f| &f.kind)
        {
            Some(surge_core::ValidationErrorKind::SameRuntimeVerification { runtime, .. }) => {
                assert_eq!(runtime, "claude-acp");
            },
            other => panic!("expected SameRuntimeVerification, got {other:?} in {findings:?}"),
        }
    }

    /// T1 (second case) — the two profiles spell the *same* runtime two
    /// different ways: `"claude"` and `"claude-code"` both alias to
    /// `"claude-acp"` in `surge_acp::REGISTRY_ID_ALIASES`. Before F1 this
    /// compared the two raw strings directly and never matched.
    #[test]
    fn same_runtime_verification_fires_across_agent_id_aliases() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("w5-alias-claude.toml"),
            minimal_profile_toml("w5-fixture-claude-alias", "claude"),
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("w5-alias-claude-code.toml"),
            minimal_profile_toml("w5-fixture-claude-code-alias", "claude-code"),
        )
        .unwrap();
        let reg = registry_with_disk(tmp.path());

        let g = graph_impl_then_verify(
            "w5-fixture-claude-alias@1.0",
            "w5-fixture-claude-code-alias@1.0",
        );

        let findings = match surge_core::validate_with_resolver(&g, &reg) {
            Ok(warnings) => warnings,
            Err(errs) => errs,
        };
        assert!(
            findings.iter().any(|f| matches!(
                f.kind,
                surge_core::ValidationErrorKind::SameRuntimeVerification { .. }
            )),
            "expected SameRuntimeVerification across `claude`/`claude-code` aliases, \
             got {findings:?}"
        );
    }

    #[test]
    fn profile_runtime_is_none_for_unknown_profile() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        assert_eq!(reg.profile_runtime("definitely-not-a-real-profile"), None);
    }

    #[test]
    fn template_and_named_agent_checks_stay_permissive() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        assert!(reg.template_exists("anything"));
        assert!(reg.named_agent_exists("anything"));
    }
}
