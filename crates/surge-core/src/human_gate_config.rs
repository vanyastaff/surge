//! HumanGate node configuration.

use crate::agent_config::ArtifactSource;
use crate::approvals::ApprovalChannel;
use crate::keys::OutcomeKey;
use crate::run_event::BootstrapStage;
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
    /// Operating mode for this gate. `Generic` is the default M5 behaviour:
    /// the gate emits only `HumanInputRequested` / `HumanInputResolved` and
    /// has no special semantics. `Bootstrap { stage }` participates in the
    /// bootstrap flow: the handler emits `BootstrapApprovalRequested`
    /// before the operator card is sent and `BootstrapApprovalDecided`
    /// after the operator replies, plus `BootstrapEditRequested` when the
    /// operator chose `edit`. Carries `#[serde(default)]` so existing
    /// graphs without the field decode as `Generic`.
    #[serde(default)]
    pub mode: HumanGateMode,
}

/// Operating mode for a `HumanGateConfig`. See the `mode` field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanGateMode {
    /// Default M5 behaviour — the gate is opaque to the bootstrap driver and
    /// emits only the generic `HumanInputRequested` / `HumanInputResolved`
    /// events.
    #[default]
    Generic,
    /// Bootstrap-aware gate. Each operator decision is mirrored to
    /// `BootstrapApprovalRequested` / `BootstrapApprovalDecided` events tagged
    /// with the bootstrap stage; an `edit` outcome additionally emits
    /// `BootstrapEditRequested { stage, feedback }` so the Flow Generator's
    /// edit-feedback binding (`ArtifactSource::EditFeedback`) can resolve
    /// against the most recent feedback for the stage.
    Bootstrap {
        /// Which bootstrap stage this gate guards.
        stage: BootstrapStage,
    },
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
            delivery_channels: vec![ApprovalChannel::Telegram {
                chat_id_ref: "$DEFAULT".into(),
            }],
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
            mode: HumanGateMode::default(),
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

    #[test]
    fn default_mode_is_generic() {
        assert_eq!(HumanGateMode::default(), HumanGateMode::Generic);
    }

    #[test]
    fn bootstrap_mode_toml_roundtrip() {
        let cfg = HumanGateConfig {
            delivery_channels: vec![ApprovalChannel::Telegram {
                chat_id_ref: "$DEFAULT".into(),
            }],
            timeout_seconds: None,
            on_timeout: TimeoutAction::default(),
            summary: SummaryTemplate {
                title: "Approve roadmap?".into(),
                body: "{{roadmap_artifact}}".into(),
                show_artifacts: vec![],
            },
            options: vec![
                ApprovalOption {
                    outcome: OutcomeKey::try_from("approve").unwrap(),
                    label: "Approve".into(),
                    style: OptionStyle::Primary,
                },
                ApprovalOption {
                    outcome: OutcomeKey::try_from("edit").unwrap(),
                    label: "Edit".into(),
                    style: OptionStyle::Warn,
                },
                ApprovalOption {
                    outcome: OutcomeKey::try_from("reject").unwrap(),
                    label: "Reject".into(),
                    style: OptionStyle::Danger,
                },
            ],
            allow_freetext: true,
            mode: HumanGateMode::Bootstrap {
                stage: BootstrapStage::Roadmap,
            },
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: HumanGateConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn legacy_toml_without_mode_field_defaults_to_generic() {
        // Backward-compat guard: persisted graphs from before Task 7
        // do not carry `mode` and must decode as `HumanGateMode::Generic`.
        let toml_s = r#"
            delivery_channels = []
            summary = { title = "t", body = "b" }
            options = [{ outcome = "approve", label = "Approve" }]
        "#;
        let parsed: HumanGateConfig = toml::from_str(toml_s).unwrap();
        assert_eq!(parsed.mode, HumanGateMode::Generic);
    }
}
