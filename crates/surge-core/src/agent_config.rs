//! Agent node configuration.

use crate::approvals::ApprovalConfig;
use crate::edge::ExceededAction;
use crate::hooks::Hook;
use crate::keys::{NodeKey, ProfileKey};
use crate::sandbox::SandboxConfig;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Binding {
    pub source: ArtifactSource,
    pub target: TemplateVar,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactSource {
    NodeOutput { node: NodeKey, artifact: String },
    RunArtifact { name: String },
    GlobPattern { node: NodeKey, pattern: String },
    Static { content: String },
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
}
