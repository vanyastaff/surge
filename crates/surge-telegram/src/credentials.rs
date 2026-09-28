//! Runtime-only Telegram credentials. Config contains references, never bot tokens.
use std::fmt;
use surge_core::config::TelegramConfig;

/// Fail-closed credential configuration errors; no error includes a secret value.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// A token reference must be explicitly configured.
    #[error("configure telegram.bot_token_env with an environment variable name")]
    MissingTokenReference,
    /// References are names, not literal token values.
    #[error("telegram.bot_token_env must be a valid environment variable name, not a token")]
    InvalidTokenReference,
    /// The configured variable must exist and be nonempty.
    #[error("configured Telegram token environment variable is missing or empty")]
    MissingToken,
    /// A delivery chat must be explicit and nonzero.
    #[error("configure an explicit nonzero Telegram delivery chat_id or chat_id_env")]
    MissingChat,
}

struct BotToken(String);
impl fmt::Debug for BotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl fmt::Display for BotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Runtime credentials without serialization or public token access.
#[derive(Debug)]
pub struct TelegramCredentials {
    token: BotToken,
    /// Explicit nonsecret delivery target.
    pub chat_id: i64,
}
impl fmt::Display for TelegramCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED Telegram credentials]")
    }
}
impl TelegramCredentials {
    /// Resolve environment references at runtime.
    ///
    /// # Errors
    /// Missing/invalid reference, secret or delivery target fails closed.
    pub fn load(config: &TelegramConfig) -> Result<Self, CredentialError> {
        Self::from_lookup(config, |name| std::env::var(name).ok())
    }

    fn from_lookup(
        config: &TelegramConfig,
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, CredentialError> {
        let reference = config
            .bot_token_env
            .as_deref()
            .ok_or(CredentialError::MissingTokenReference)?;
        if !valid_env_name(reference) {
            return Err(CredentialError::InvalidTokenReference);
        }
        let token = lookup(reference)
            .filter(|value| !value.trim().is_empty())
            .ok_or(CredentialError::MissingToken)?;
        let chat_id = config
            .chat_id
            .or_else(|| {
                config
                    .chat_id_env
                    .as_deref()
                    .and_then(&mut lookup)
                    .and_then(|value| value.parse().ok())
            })
            .filter(|id| *id != 0)
            .ok_or(CredentialError::MissingChat)?;
        Ok(Self {
            token: BotToken(token),
            chat_id,
        })
    }

    /// Construct the transport without exposing the token to configuration/logging.
    /// Upstream raw transport diagnostics are not a redacted secret surface.
    pub fn bot(&self) -> teloxide::Bot {
        teloxide::Bot::new(self.token.0.clone())
    }
}

/// Whether a string is an environment variable name rather than a credential value.
#[must_use]
fn valid_env_name(value: &str) -> bool {
    let mut chars = value.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolved_secret_is_redacted_in_debug_and_display() {
        let config = TelegramConfig {
            bot_token_env: Some("BOT_SECRET".into()),
            chat_id: Some(42),
            chat_id_env: None,
        };
        let credentials = TelegramCredentials::from_lookup(&config, |_| {
            Some("73123:RUNTIME_SECRET_LITERAL".into())
        })
        .unwrap();
        assert!(!format!("{credentials:?} {credentials}").contains("RUNTIME_SECRET_LITERAL"));
        assert_eq!(credentials.chat_id, 42);
    }
    #[test]
    fn missing_target_or_literal_reference_is_rejected_without_echo() {
        let config = TelegramConfig {
            bot_token_env: Some("73123:REFERENCE_SECRET_LITERAL".into()),
            ..Default::default()
        };
        let error = TelegramCredentials::from_lookup(&config, |_| None).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("REFERENCE_SECRET_LITERAL"));
        let config = TelegramConfig {
            bot_token_env: Some("BOT_SECRET".into()),
            ..Default::default()
        };
        assert!(matches!(
            TelegramCredentials::from_lookup(&config, |_| Some("secret".into())),
            Err(CredentialError::MissingChat)
        ));
    }
}
