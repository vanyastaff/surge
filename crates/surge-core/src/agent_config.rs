//! Agent node configuration.

use crate::approvals::ApprovalConfig;
use crate::edge::ExceededAction;
use crate::hooks::Hook;
use crate::keys::{NodeKey, ProfileKey};
use crate::sandbox::SandboxConfig;
use crate::skill::SkillRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentConfig {
    pub profile: ProfileKey,
    #[serde(default)]
    pub prompt_overrides: Option<PromptOverride>,
    #[serde(default)]
    pub tool_overrides: Option<ToolOverride>,
    #[serde(default)]
    pub sandbox_override: Option<SandboxConfig>,
    #[serde(default)]
    pub approvals_override: Option<ApprovalConfig>,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub rules_overrides: Option<RulesOverride>,
    #[serde(default)]
    pub limits: NodeLimits,
    #[serde(default)]
    pub hooks: Vec<Hook>,
    #[serde(default)]
    pub custom_fields: BTreeMap<String, toml::Value>,
}

impl AgentConfig {
    /// Skills declared on this node, read from `custom_fields["skills"]`.
    ///
    /// Node-level skill declarations ride the existing `custom_fields`
    /// extension point rather than a dedicated field: `custom_fields` is
    /// additive by construction (any reader that doesn't recognize a key
    /// just ignores it), so this capability doesn't force every
    /// `AgentConfig` struct literal across the workspace to grow a new
    /// field. A missing `"skills"` key means the node declared none — not
    /// an error. A present-but-malformed entry is a graph-authoring
    /// mistake and is surfaced typed, never silently dropped (see
    /// [`DeclaredSkillsError`]).
    ///
    /// Each declared [`SkillRef`] is a request, not a discovery result:
    /// `hash: None` means "match by name/provider/version" (the flow author
    /// hasn't pinned a specific pack version); `hash: Some(_)` pins the
    /// exact content the author trusts. Binding this list against a
    /// [`crate::skill::SkillCatalog`] and gating an unpinned or
    /// hash-mismatched entry behind operator approval is the engine's job,
    /// not this method's.
    ///
    /// # Errors
    /// [`DeclaredSkillsError`] when `custom_fields["skills"]` exists but
    /// does not deserialize as an array of skill references.
    #[must_use = "a malformed declaration (Err) must be surfaced, not silently dropped"]
    pub fn declared_skills(&self) -> Result<Vec<SkillRef>, DeclaredSkillsError> {
        match self.custom_fields.get("skills") {
            None => Ok(Vec::new()),
            Some(value) => value
                .clone()
                .try_into::<Vec<SkillRef>>()
                .map_err(DeclaredSkillsError),
        }
    }
}

/// `custom_fields["skills"]` exists but is not a valid list of skill
/// references (wrong shape, unknown `provider` tag, ...).
#[derive(Debug, thiserror::Error)]
#[error("node's custom_fields.skills is not a valid skill reference list: {0}")]
pub struct DeclaredSkillsError(#[source] toml::de::Error);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Binding {
    pub source: ArtifactSource,
    pub target: TemplateVar,
    /// Resolve missing artifacts as an empty string instead of failing the stage.
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactSource {
    NodeOutput {
        node: NodeKey,
        artifact: String,
    },
    RunArtifact {
        name: String,
    },
    GlobPattern {
        node: NodeKey,
        pattern: String,
    },
    Static {
        content: String,
    },
    /// Operator-supplied free-text feedback from the most recent
    /// `BootstrapEditRequested` event whose target stage corresponds to
    /// `from_node`. Resolves to an empty string when no edit has yet
    /// occurred. Used by bootstrap profiles when re-entered via the
    /// `Backtrack` edge after an `edit` HumanGate decision.
    EditFeedback {
        from_node: NodeKey,
    },
    /// The pipeline run's initial prompt as captured in `RunStarted.initial_prompt`.
    /// Synthesized into `RunMemory` at run start under the artifact name
    /// `"user_prompt"`; this variant is the canonical way for bootstrap
    /// profiles to bind the user's free-form intent into a `{{user_prompt}}`
    /// template variable.
    InitialPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TemplateVar(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptOverride {
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub append_system: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolOverride {
    #[serde(default)]
    pub mcp_add: Vec<String>,
    #[serde(default)]
    pub mcp_remove: Vec<String>,
    #[serde(default)]
    pub skills_add: Vec<String>,
    #[serde(default)]
    pub skills_remove: Vec<String>,
    #[serde(default)]
    pub shell_allowlist_add: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RulesOverride {
    #[serde(default)]
    pub disable_inherited: bool,
    #[serde(default)]
    pub additional_rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeLimits {
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub circuit_breaker: Option<CbConfig>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

impl Default for NodeLimits {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout(),
            max_retries: default_max_retries(),
            circuit_breaker: None,
            max_tokens: default_max_tokens(),
        }
    }
}

fn default_timeout() -> u32 {
    900
}
fn default_max_retries() -> u32 {
    3
}
fn default_max_tokens() -> u32 {
    200_000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CbConfig {
    pub max_failures: u32,
    pub window_seconds: u32,
    pub on_open: ExceededAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_match_spec() {
        let l = NodeLimits::default();
        assert_eq!(l.timeout_seconds, 900);
        assert_eq!(l.max_retries, 3);
        assert_eq!(l.max_tokens, 200_000);
    }

    #[test]
    fn minimal_agent_config_toml_roundtrips() {
        let cfg = AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: Vec::new(),
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks: Vec::new(),
            custom_fields: BTreeMap::new(),
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: AgentConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn binding_with_node_output_source_roundtrips() {
        let b = Binding {
            source: ArtifactSource::NodeOutput {
                node: NodeKey::try_from("spec_1").unwrap(),
                artifact: "spec.md".into(),
            },
            target: TemplateVar("spec".into()),
            optional: false,
        };
        let toml_s = toml::to_string(&b).unwrap();
        let parsed: Binding = toml::from_str(&toml_s).unwrap();
        assert_eq!(b, parsed);
    }

    #[test]
    fn edit_feedback_source_round_trips() {
        let b = Binding {
            source: ArtifactSource::EditFeedback {
                from_node: NodeKey::try_from("description_author").unwrap(),
            },
            target: TemplateVar("edit_feedback".into()),
            optional: false,
        };
        let toml_s = toml::to_string(&b).unwrap();
        let parsed: Binding = toml::from_str(&toml_s).unwrap();
        assert_eq!(b, parsed);
    }

    #[test]
    fn initial_prompt_source_round_trips() {
        let b = Binding {
            source: ArtifactSource::InitialPrompt,
            target: TemplateVar("user_prompt".into()),
            optional: false,
        };
        let toml_s = toml::to_string(&b).unwrap();
        let parsed: Binding = toml::from_str(&toml_s).unwrap();
        assert_eq!(b, parsed);
    }

    #[test]
    fn agent_with_all_optional_fields_set_roundtrips() {
        let cfg = AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: Some(PromptOverride {
                system: None,
                append_system: Some("Extra rule.".into()),
            }),
            tool_overrides: Some(ToolOverride {
                mcp_add: vec!["filesystem".into()],
                mcp_remove: vec![],
                skills_add: vec!["rust-expert".into()],
                skills_remove: vec![],
                shell_allowlist_add: vec!["cargo".into()],
            }),
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![Binding {
                source: ArtifactSource::RunArtifact {
                    name: "description.md".into(),
                },
                target: TemplateVar("description".into()),
                optional: false,
            }],
            rules_overrides: Some(RulesOverride {
                disable_inherited: false,
                additional_rules: vec!["No unwrap()".into()],
            }),
            limits: NodeLimits {
                timeout_seconds: 1200,
                max_retries: 5,
                circuit_breaker: Some(CbConfig {
                    max_failures: 3,
                    window_seconds: 60,
                    on_open: ExceededAction::Fail,
                }),
                max_tokens: 100_000,
            },
            hooks: Vec::new(),
            custom_fields: {
                let mut m = BTreeMap::new();
                m.insert("max_files".into(), toml::Value::Integer(20));
                m
            },
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: AgentConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn declared_skills_absent_key_returns_empty() {
        let cfg = AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: Vec::new(),
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks: Vec::new(),
            custom_fields: BTreeMap::new(),
        };
        assert_eq!(cfg.declared_skills().unwrap(), Vec::new());
    }

    #[test]
    fn declared_skills_parses_toml_declared_list() {
        use crate::skill::SkillProvider;

        let toml_s = r#"
            profile = "implementer@1.0"

            [custom_fields]
            skills = [
                { name = "code-reviewer", provider = "project_dir" },
                { name = "rust-expert", provider = "user_dir", version = "2.0", hash = "sha256:0000000000000000000000000000000000000000000000000000000000000000" },
            ]
        "#;
        let cfg: AgentConfig = toml::from_str(toml_s).unwrap();
        let declared = cfg.declared_skills().unwrap();

        assert_eq!(declared.len(), 2);
        assert_eq!(declared[0].name, "code-reviewer");
        assert_eq!(declared[0].provider, SkillProvider::ProjectDir);
        assert_eq!(declared[0].version, None);
        assert_eq!(declared[0].hash, None);
        assert_eq!(declared[1].name, "rust-expert");
        assert_eq!(declared[1].provider, SkillProvider::UserDir);
        assert_eq!(declared[1].version.as_deref(), Some("2.0"));
        assert!(declared[1].hash.is_some());
    }

    #[test]
    fn declared_skills_malformed_entry_is_typed_error() {
        let toml_s = r#"
            profile = "implementer@1.0"

            [custom_fields]
            skills = [ { provider = "project_dir" } ]
        "#;
        let cfg: AgentConfig = toml::from_str(toml_s).unwrap();
        assert!(cfg.declared_skills().is_err());
    }
}
