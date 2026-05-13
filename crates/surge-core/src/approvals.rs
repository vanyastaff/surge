//! Approval policy and delivery channel types.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default timeout surge uses when the operator has not declared their own.
/// Long because elevation requests legitimately span human-scale time
/// (overnight, business-day boundaries).
pub const DEFAULT_ELEVATION_TIMEOUT_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalConfig {
    pub policy: ApprovalPolicy,
    #[serde(default)]
    pub sandbox_approval: bool,
    #[serde(default)]
    pub mcp_elicitations: bool,
    #[serde(default)]
    pub request_permissions: bool,
    #[serde(default)]
    pub skill_approval: bool,
    #[serde(default)]
    pub elevation: bool,
    /// Channels for sandbox-elevation requests and other agent-stage approval prompts.
    /// Distinct from `HumanGateConfig::delivery_channels` (gate-explicit prompts).
    #[serde(default)]
    pub elevation_channels: Vec<ApprovalChannel>,
    /// Timeout for ACP `request_permission` elevations. When the operator
    /// fails to respond within this window the engine appends
    /// `SandboxElevationTimedOut`, denies the request, and replies
    /// `Cancelled` to the agent. `None` falls back to
    /// [`DEFAULT_ELEVATION_TIMEOUT_SECS`].
    #[serde(default, with = "humantime_serde::option")]
    pub elevation_timeout: Option<Duration>,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            policy: ApprovalPolicy::OnRequest,
            sandbox_approval: false,
            mcp_elicitations: false,
            request_permissions: false,
            skill_approval: false,
            elevation: true,
            elevation_channels: Vec::new(),
            elevation_timeout: None,
        }
    }
}

impl ApprovalConfig {
    /// Resolved elevation timeout: configured value or
    /// [`DEFAULT_ELEVATION_TIMEOUT_SECS`].
    #[must_use]
    pub fn resolved_elevation_timeout(&self) -> Duration {
        self.elevation_timeout
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_ELEVATION_TIMEOUT_SECS))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    Untrusted,
    #[default]
    OnRequest,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalChannel {
    Telegram { chat_id_ref: String },
    Desktop { duration: ApprovalDuration },
    Email { to_ref: String },
    Webhook { url: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDuration {
    Persistent,
    Transient,
}

/// Discriminator over `ApprovalChannel` — used in events where the full
/// channel struct is unnecessary (only need to know which channel was used).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChannelKind {
    Telegram,
    Desktop,
    Email,
    Webhook,
}

impl ApprovalChannel {
    #[must_use]
    pub fn kind(&self) -> ApprovalChannelKind {
        match self {
            Self::Telegram { .. } => ApprovalChannelKind::Telegram,
            Self::Desktop { .. } => ApprovalChannelKind::Desktop,
            Self::Email { .. } => ApprovalChannelKind::Email,
            Self::Webhook { .. } => ApprovalChannelKind::Webhook,
        }
    }
}

impl ApprovalChannelKind {
    /// Stable, lowercase string identifier for this channel kind.
    ///
    /// Matches the `serde(rename_all = "snake_case")` representation. Used
    /// by storage code that needs a TEXT representation (e.g., the
    /// `pending_approvals.channel` column) without going through full
    /// serde serialization. Stable across releases — append-only.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Desktop => "desktop",
            Self::Email => "email",
            Self::Webhook => "webhook",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_on_request() {
        let cfg = ApprovalConfig::default();
        assert_eq!(cfg.policy, ApprovalPolicy::OnRequest);
        assert!(cfg.elevation);
    }

    #[test]
    fn channel_kind_extraction() {
        let ch = ApprovalChannel::Telegram {
            chat_id_ref: "$DEFAULT".into(),
        };
        assert_eq!(ch.kind(), ApprovalChannelKind::Telegram);
    }

    #[test]
    fn channel_kind_as_str_matches_serde_repr() {
        assert_eq!(ApprovalChannelKind::Telegram.as_str(), "telegram");
        assert_eq!(ApprovalChannelKind::Desktop.as_str(), "desktop");
        assert_eq!(ApprovalChannelKind::Email.as_str(), "email");
        assert_eq!(ApprovalChannelKind::Webhook.as_str(), "webhook");
    }

    #[test]
    fn channel_toml_roundtrip() {
        let ch = ApprovalChannel::Webhook {
            url: "https://example.com/hook".into(),
        };
        let toml_s = toml::to_string(&ch).unwrap();
        let parsed: ApprovalChannel = toml::from_str(&toml_s).unwrap();
        assert_eq!(ch, parsed);
    }

    #[test]
    fn config_toml_roundtrip() {
        let cfg = ApprovalConfig {
            policy: ApprovalPolicy::OnRequest,
            sandbox_approval: true,
            mcp_elicitations: false,
            request_permissions: true,
            skill_approval: false,
            elevation: true,
            elevation_channels: vec![ApprovalChannel::Telegram {
                chat_id_ref: "$DEFAULT".into(),
            }],
            elevation_timeout: Some(Duration::from_secs(3_600)),
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: ApprovalConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }
}
