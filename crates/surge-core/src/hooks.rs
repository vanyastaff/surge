//! Lifecycle hook configuration with structured matcher.

use crate::keys::{NodeKey, OutcomeKey};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hook {
    pub id: String,
    pub trigger: HookTrigger,
    /// Structured match expression. Empty matcher (`MatcherSpec::default()`)
    /// matches every event of the configured trigger.
    #[serde(default)]
    pub matcher: MatcherSpec,
    pub command: String,
    #[serde(default)]
    pub on_failure: HookFailureMode,
    #[serde(default)]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub inherit: HookInheritance,
}

/// Structured matcher. Each set field is an additional `AND` constraint;
/// an empty `MatcherSpec` matches everything.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MatcherSpec {
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub outcome: Option<OutcomeKey>,
    #[serde(default)]
    pub node: Option<NodeKey>,
    #[serde(default)]
    pub tool_arg_contains: Option<String>,
    #[serde(default)]
    pub file_glob: Option<String>,
}

impl MatcherSpec {
    #[must_use]
    pub fn is_unconditional(&self) -> bool {
        self.tool.is_none()
            && self.outcome.is_none()
            && self.node.is_none()
            && self.tool_arg_contains.is_none()
            && self.file_glob.is_none()
    }

    /// Evaluate against a context. Pure function — engine builds the
    /// `MatchContext` from the current event before calling.
    #[must_use]
    pub fn matches(&self, ctx: &MatchContext<'_>) -> bool {
        if self.is_unconditional() {
            return true;
        }
        if let Some(want) = &self.tool
            && ctx.tool != Some(want.as_str())
        {
            return false;
        }
        if let Some(want) = &self.outcome
            && ctx.outcome != Some(want)
        {
            return false;
        }
        if let Some(want) = &self.node
            && ctx.node != Some(want)
        {
            return false;
        }
        if let Some(needle) = &self.tool_arg_contains {
            match ctx.tool_args_text {
                Some(haystack) if haystack.contains(needle.as_str()) => {},
                _ => return false,
            }
        }
        if let Some(glob) = &self.file_glob {
            // M1 stub: substring-match against file path. Engine will replace
            // with proper glob matcher (probably via the `globset` crate).
            match ctx.file_path {
                Some(p) => {
                    if !p.to_string_lossy().contains(glob.trim_start_matches('*')) {
                        return false;
                    }
                },
                None => return false,
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct MatchContext<'a> {
    pub trigger: HookTrigger,
    pub tool: Option<&'a str>,
    pub tool_args_text: Option<&'a str>,
    pub outcome: Option<&'a OutcomeKey>,
    pub node: Option<&'a NodeKey>,
    pub file_path: Option<&'a Path>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookTrigger {
    PreToolUse,
    PostToolUse,
    OnOutcome,
    OnError,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookFailureMode {
    Reject,
    #[default]
    Warn,
    Ignore,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookInheritance {
    #[default]
    Extend,
    Replace,
    Disable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_matcher_is_unconditional() {
        let m = MatcherSpec::default();
        assert!(m.is_unconditional());
    }

    #[test]
    fn tool_filter_matches() {
        let m = MatcherSpec {
            tool: Some("edit_file".into()),
            ..Default::default()
        };
        let ctx = MatchContext {
            trigger: HookTrigger::PreToolUse,
            tool: Some("edit_file"),
            tool_args_text: None,
            outcome: None,
            node: None,
            file_path: None,
        };
        assert!(m.matches(&ctx));
    }

    #[test]
    fn tool_filter_rejects_mismatch() {
        let m = MatcherSpec {
            tool: Some("edit_file".into()),
            ..Default::default()
        };
        let ctx = MatchContext {
            trigger: HookTrigger::PreToolUse,
            tool: Some("read_file"),
            tool_args_text: None,
            outcome: None,
            node: None,
            file_path: None,
        };
        assert!(!m.matches(&ctx));
    }

    #[test]
    fn hook_toml_roundtrip() {
        let h = Hook {
            id: "fmt-check".into(),
            trigger: HookTrigger::PostToolUse,
            matcher: MatcherSpec {
                tool: Some("edit_file".into()),
                ..Default::default()
            },
            command: "cargo fmt --check".into(),
            on_failure: HookFailureMode::Warn,
            timeout_seconds: Some(30),
            inherit: HookInheritance::Extend,
        };
        let toml_s = toml::to_string(&h).unwrap();
        let parsed: Hook = toml::from_str(&toml_s).unwrap();
        assert_eq!(h, parsed);
    }

    #[test]
    fn hook_with_default_matcher_parses() {
        let toml_s = r#"
            id = "always"
            trigger = "on_outcome"
            command = "echo hi"
        "#;
        let h: Hook = toml::from_str(toml_s).unwrap();
        assert!(h.matcher.is_unconditional());
        assert_eq!(h.on_failure, HookFailureMode::Warn);
    }

    #[test]
    fn outcome_filter_uses_typed_key() {
        let outcome_key = OutcomeKey::try_from("done").unwrap();
        let m = MatcherSpec {
            outcome: Some(outcome_key.clone()),
            ..Default::default()
        };
        let ctx = MatchContext {
            trigger: HookTrigger::OnOutcome,
            tool: None,
            tool_args_text: None,
            outcome: Some(&outcome_key),
            node: None,
            file_path: None,
        };
        assert!(m.matches(&ctx));
    }
}
