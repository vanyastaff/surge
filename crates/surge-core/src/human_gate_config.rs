//! HumanGate node configuration.

use crate::agent_config::ArtifactSource;
use crate::approvals::ApprovalChannel;
use crate::keys::OutcomeKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HumanGateConfig {
    /// Channels where the gate's approval card is sent, in priority order.
    /// Distinct from `ApprovalConfig::elevation_channels`.
    pub delivery_channels: Vec<ApprovalChannel>,
    #[serde(default)]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub on_timeout: TimeoutAction,
    pub summary: SummaryTemplate,
    pub options: Vec<ApprovalOption>,
    #[serde(default)]
    pub allow_freetext: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutAction {
    #[default]
    Reject,
    Escalate,
    Continue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalOption {
    pub outcome: OutcomeKey,
    pub label: String,
    #[serde(default)]
    pub style: OptionStyle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionStyle {
    Primary,
    Danger,
    Warn,
    #[default]
    Normal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SummaryTemplate {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub show_artifacts: Vec<ArtifactSource>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_gate_toml_roundtrip() {
        let cfg = HumanGateConfig {
            delivery_channels: vec![ApprovalChannel::Telegram { chat_id_ref: "$DEFAULT".into() }],
            timeout_seconds: Some(3600),
            on_timeout: TimeoutAction::Escalate,
            summary: SummaryTemplate {
                title: "Approve plan?".into(),
                body: "{{plan_summary}}".into(),
                show_artifacts: vec![],
            },
            options: vec![
                ApprovalOption {
                    outcome: OutcomeKey::try_from("approve").unwrap(),
                    label: "Approve".into(),
                    style: OptionStyle::Primary,
                },
                ApprovalOption {
                    outcome: OutcomeKey::try_from("reject").unwrap(),
                    label: "Reject".into(),
                    style: OptionStyle::Danger,
                },
            ],
            allow_freetext: true,
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: HumanGateConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn default_timeout_action_is_reject() {
        assert_eq!(TimeoutAction::default(), TimeoutAction::Reject);
    }

    #[test]
    fn default_option_style_is_normal() {
        assert_eq!(OptionStyle::default(), OptionStyle::Normal);
    }
}
