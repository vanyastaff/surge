//! Profile (role) configuration.

use crate::approvals::ApprovalConfig;
use crate::artifact_contract::ArtifactContractRef;
use crate::edge::EdgeKind;
use crate::hooks::{Hook, HookTrigger};
use crate::keys::{OutcomeKey, ProfileKey};
use crate::sandbox::SandboxConfig;
use serde::{Deserialize, Serialize};

pub mod bundled;
pub mod keyref;
pub mod registry;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub schema_version: u32,
    pub role: Role,
    pub runtime: RuntimeCfg,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub tools: ToolsCfg,
    #[serde(default)]
    pub approvals: ApprovalConfig,
    pub outcomes: Vec<ProfileOutcome>,
    #[serde(default)]
    pub bindings: ProfileBindings,
    #[serde(default)]
    pub hooks: ProfileHooks,
    pub prompt: PromptTemplate,
    #[serde(default)]
    pub inspector_ui: InspectorUi,
}

impl Profile {
    /// Iterate hooks declared directly on this profile that fire on the
    /// requested `trigger`. `extends`-chain resolution is intentionally NOT
    /// performed here — it is owned by the future `Profile registry & bundled
    /// roles` milestone. The orchestrator's `HookExecutor` consumes a single
    /// already-resolved `Profile`.
    pub fn hooks_for_trigger(&self, trigger: HookTrigger) -> impl Iterator<Item = &Hook> {
        self.hooks
            .entries
            .iter()
            .filter(move |hook| hook.trigger == trigger)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Role {
    pub id: ProfileKey,
    pub version: semver::Version,
    pub display_name: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub category: RoleCategory,
    pub description: String,
    pub when_to_use: String,
    /// Inheritance reference. Parsed but NOT resolved in M1 — engine handles
    /// resolution in a later milestone.
    #[serde(default)]
    pub extends: Option<ProfileKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleCategory {
    Agents,
    Gates,
    Flow,
    Io,
    #[serde(rename = "_bootstrap")]
    Bootstrap,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeCfg {
    pub recommended_model: String,
    #[serde(default = "default_temperature")]
    pub default_temperature: f32,
    #[serde(default = "default_max_tokens_profile")]
    pub default_max_tokens: u32,
    #[serde(default)]
    pub load_rules_lazily: Option<bool>,
    /// Identifier of the agent runtime this profile targets, matching an entry
    /// id in the `surge_acp::Registry` (e.g. `"claude-code"`, `"codex"`,
    /// `"gemini-cli"`, `"mock"`). The orchestrator resolves this to a binary
    /// path / launch profile via the agent registry. Defaults to
    /// `"claude-code"` so existing TOML profiles authored before the
    /// `Profile registry & bundled roles` milestone keep parsing.
    #[serde(default = "default_agent_id")]
    pub agent_id: String,
}

fn default_temperature() -> f32 {
    0.2
}
fn default_max_tokens_profile() -> u32 {
    200_000
}
pub(crate) fn default_agent_id() -> String {
    "claude-code".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolsCfg {
    #[serde(default)]
    pub default_mcp: Vec<String>,
    #[serde(default)]
    pub default_skills: Vec<String>,
    #[serde(default)]
    pub default_shell_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileOutcome {
    pub id: OutcomeKey,
    pub description: String,
    pub edge_kind_hint: EdgeKind,
    #[serde(default)]
    pub required_artifacts: Vec<String>,
    #[serde(default)]
    pub produced_artifacts: Vec<ProfileArtifactDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileArtifactDeclaration {
    pub path: String,
    pub contract: ArtifactContractRef,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProfileBindings {
    #[serde(default)]
    pub expected: Vec<ExpectedBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpectedBinding {
    pub name: String,
    pub source: ExpectedBindingSource,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ExpectedBindingSource {
    NodeOutput { from_role: ProfileKey },
    RunArtifact,
    Any,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProfileHooks {
    #[serde(default)]
    pub entries: Vec<Hook>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptTemplate {
    pub system: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct InspectorUi {
    #[serde(default)]
    pub fields: Vec<InspectorUiField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InspectorUiField {
    pub id: String,
    pub label: String,
    pub kind: InspectorFieldKind,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub help: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InspectorFieldKind {
    Number {
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },
    Toggle,
    Select {
        options: Vec<String>,
    },
    Text {
        #[serde(default)]
        multiline: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_contract::{ARTIFACT_SCHEMA_VERSION, ArtifactKind};

    #[test]
    fn minimal_profile_roundtrips() {
        let p = Profile {
            schema_version: 1,
            role: Role {
                id: ProfileKey::try_from("implementer").unwrap(),
                version: semver::Version::parse("1.0.0").unwrap(),
                display_name: "Implementer".into(),
                icon: None,
                category: RoleCategory::Agents,
                description: "Writes code.".into(),
                when_to_use: "Standard implementation work.".into(),
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
                produced_artifacts: vec![],
            }],
            bindings: ProfileBindings::default(),
            hooks: ProfileHooks::default(),
            prompt: PromptTemplate {
                system: "You are an implementer.".into(),
            },
            inspector_ui: InspectorUi::default(),
        };
        let toml_s = toml::to_string(&p).unwrap();
        let parsed: Profile = toml::from_str(&toml_s).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn outcome_artifact_declarations_roundtrip() {
        let text = r#"
            schema_version = 1

            [role]
            id = "description-author"
            version = "1.0.0"
            display_name = "Description Author"
            category = "_bootstrap"
            description = "Drafts description artifacts"
            when_to_use = "Bootstrap"

            [runtime]
            recommended_model = "claude-opus-4-7"

            [[outcomes]]
            id = "drafted"
            description = "Drafted"
            edge_kind_hint = "forward"
            required_artifacts = ["description.md"]

            [[outcomes.produced_artifacts]]
            path = "description.md"
            contract = { kind = "description", schema_version = 1 }

            [prompt]
            system = "Draft description.md"
        "#;

        let parsed: Profile = toml::from_str(text).unwrap();

        assert_eq!(parsed.outcomes[0].produced_artifacts.len(), 1);
        assert_eq!(
            parsed.outcomes[0].produced_artifacts[0].path,
            "description.md"
        );
        assert_eq!(
            parsed.outcomes[0].produced_artifacts[0].contract.kind,
            ArtifactKind::Description
        );
        assert_eq!(
            parsed.outcomes[0].produced_artifacts[0]
                .contract
                .schema_version,
            ARTIFACT_SCHEMA_VERSION
        );
    }

    #[test]
    fn extends_field_roundtrips_but_is_not_resolved() {
        let p_text = r#"
            schema_version = 1

            [role]
            id = "rust-implementer"
            version = "1.0.0"
            display_name = "Rust Implementer"
            category = "agents"
            description = "Rust-focused implementer"
            when_to_use = "Rust crates"
            extends = "generic-implementer@1.0"

            [runtime]
            recommended_model = "claude-opus-4-7"

            [[outcomes]]
            id = "done"
            description = "Success"
            edge_kind_hint = "forward"

            [prompt]
            system = "Rust expert."
        "#;
        let p: Profile = toml::from_str(p_text).unwrap();
        assert_eq!(
            p.role.extends.as_ref().unwrap().as_str(),
            "generic-implementer@1.0"
        );
    }

    #[test]
    fn role_category_bootstrap_serializes_with_underscore() {
        let cat = RoleCategory::Bootstrap;
        let json = serde_json::json!(cat);
        assert_eq!(json, "_bootstrap");
    }

    #[test]
    fn inspector_field_select_with_options() {
        let f = InspectorUiField {
            id: "review_focus".into(),
            label: "Review focus".into(),
            kind: InspectorFieldKind::Select {
                options: vec!["general".into(), "security".into()],
            },
            default: Some(toml::Value::String("general".into())),
            help: None,
        };
        let toml_s = toml::to_string(&f).unwrap();
        let parsed: InspectorUiField = toml::from_str(&toml_s).unwrap();
        assert_eq!(f, parsed);
    }
}
