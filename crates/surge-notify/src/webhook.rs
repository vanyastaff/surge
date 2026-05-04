//! Webhook notification — POSTs JSON payload to configured URL.

use crate::deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use async_trait::async_trait;
use surge_core::notify_config::NotifyChannel;

/// Webhook deliverer; POSTs JSON to the channel's `url`.
pub struct WebhookDeliverer {
    client: reqwest::Client,
}

impl WebhookDeliverer {
    /// Construct with a fresh `reqwest::Client`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Construct with a caller-supplied `reqwest::Client` (e.g., shared
    /// across deliverers for connection pooling).
    #[must_use]
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for WebhookDeliverer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NotifyDeliverer for WebhookDeliverer {
    async fn deliver(
        &self,
        ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        let NotifyChannel::Webhook { url } = channel else {
            return Err(NotifyError::Transport(
                "WebhookDeliverer received non-Webhook channel".into(),
            ));
        };

        let payload = serde_json::json!({
            "severity": rendered.severity,
            "title": rendered.title,
            "body": rendered.body,
            "artifacts": rendered.artifact_paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            "run_id": ctx.run_id.to_string(),
            "node": ctx.node.to_string(),
        });

        let response = self
            .client
            .post(url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| NotifyError::Transport(format!("POST {url}: {e}")))?;

        if !response.status().is_success() {
            return Err(NotifyError::Transport(format!(
                "POST {url} returned status {}",
                response.status()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use surge_core::id::RunId;
    use surge_core::keys::NodeKey;
    use surge_core::notify_config::NotifySeverity;

    fn rendered() -> RenderedNotification {
        RenderedNotification {
            severity: NotifySeverity::Info,
            title: "T".into(),
            body: "B".into(),
            artifact_paths: vec![],
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn webhook_posts_json_to_url() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/hook", server.server_addr().to_ip().unwrap());
        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_clone = captured.clone();

        let handle = std::thread::spawn(move || {
            if let Ok(mut req) = server.recv() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(&mut req.as_reader(), &mut body);
                captured_clone.lock().unwrap().push(body);
                let _ = req.respond(tiny_http::Response::empty(200));
            }
        });

        let deliverer = WebhookDeliverer::new();
        let node = NodeKey::try_from("n").unwrap();
        let ctx = NotifyDeliveryContext {
            run_id: RunId::new(),
            node: &node,
        };
        let channel = NotifyChannel::Webhook { url: url.clone() };

        deliverer
            .deliver(&ctx, &channel, &rendered())
            .await
            .unwrap();
        handle.join().unwrap();

        let captured = captured.lock().unwrap().clone();
        assert!(!captured.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&captured[0]).unwrap();
        assert_eq!(parsed["title"], "T");
    }
}
