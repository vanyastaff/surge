//! Email notification via `lettre` SMTP.

use crate::deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use async_trait::async_trait;
use std::sync::Arc;
use surge_core::notify_config::NotifyChannel;

/// Resolves a secret reference to recipient email + SMTP credentials.
#[async_trait]
pub trait EmailSecretResolver: Send + Sync {
    /// Resolve the recipient reference to credentials.
    async fn resolve(&self, to_ref: &str) -> Result<EmailCredentials, NotifyError>;
}

/// Resolved email credentials and recipient.
pub struct EmailCredentials {
    /// Recipient email address.
    pub recipient: String,
    /// SMTP server host (e.g., `smtp.gmail.com`).
    pub smtp_host: String,
    /// SMTP username for AUTH.
    pub smtp_user: String,
    /// SMTP password for AUTH.
    pub smtp_password: String,
    /// Sender address used in the From header.
    pub sender: String,
}

/// Email deliverer using `lettre` SMTP transport.
pub struct EmailDeliverer {
    resolver: Arc<dyn EmailSecretResolver>,
}

impl EmailDeliverer {
    /// Construct with a caller-supplied resolver.
    #[must_use]
    pub fn new(resolver: Arc<dyn EmailSecretResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl NotifyDeliverer for EmailDeliverer {
    async fn deliver(
        &self,
        _ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        use lettre::message::{Message, header};
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

        let NotifyChannel::Email { to_ref } = channel else {
            return Err(NotifyError::Transport(
                "EmailDeliverer received non-Email channel".into(),
            ));
        };

        let creds = self.resolver.resolve(to_ref).await?;

        let email = Message::builder()
            .from(
                creds
                    .sender
                    .parse()
                    .map_err(|e| NotifyError::Transport(format!("sender parse: {e}")))?,
            )
            .to(creds
                .recipient
                .parse()
                .map_err(|e| NotifyError::Transport(format!("recipient parse: {e}")))?)
            .subject(&rendered.title)
            .header(header::ContentType::TEXT_PLAIN)
            .body(rendered.body.clone())
            .map_err(|e| NotifyError::Transport(format!("message build: {e}")))?;

        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::relay(&creds.smtp_host)
                .map_err(|e| NotifyError::Transport(format!("smtp relay: {e}")))?
                .credentials(Credentials::new(creds.smtp_user, creds.smtp_password))
                .build();

        mailer
            .send(email)
            .await
            .map_err(|e| NotifyError::Transport(format!("smtp send: {e}")))?;
        Ok(())
    }
}
