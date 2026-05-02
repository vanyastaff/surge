//! Notify node configuration.

use crate::agent_config::ArtifactSource;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotifyConfig {
    pub channel: NotifyChannel,
    pub template: NotifyTemplate,
    #[serde(default)]
    pub on_failure: NotifyFailureAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotifyChannel {
    Telegram { chat_id_ref: String },
    Slack { channel_ref: String },
    Email { to_ref: String },
    Desktop,
    Webhook { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotifyTemplate {
    pub severity: NotifySeverity,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub artifacts: Vec<ArtifactSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifySeverity {
    Info,
    Warn,
    Error,
    Success,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyFailureAction {
    #[default]
    Continue,
    Fail,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_with_slack_channel_roundtrips() {
        let cfg = NotifyConfig {
            channel: NotifyChannel::Slack {
                channel_ref: "#deploys".into(),
            },
            template: NotifyTemplate {
                severity: NotifySeverity::Success,
                title: "Run complete".into(),
                body: "Run {{run_id}} succeeded".into(),
                artifacts: vec![],
            },
            on_failure: NotifyFailureAction::Continue,
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: NotifyConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn desktop_channel_carries_no_fields() {
        let ch = NotifyChannel::Desktop;
        let toml_s = toml::to_string(&ch).unwrap();
        let parsed: NotifyChannel = toml::from_str(&toml_s).unwrap();
        assert_eq!(ch, parsed);
    }
}
