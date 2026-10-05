//! Ordinary command replies share durable admission; pairing bootstrap is exempt.
use super::{Admission, PairingsAdmission};
use crate::error::{Result, TelegramCockpitError};

pub(super) async fn send_reply(
    bot: &teloxide::Bot,
    admission: &PairingsAdmission,
    chat_id: i64,
    text: &str,
) -> Result<()> {
    if !admission.is_admitted(chat_id).await? {
        tracing::info!(target: "telegram::auth", %chat_id, "outgoing reply denied for unpaired chat");
        return Err(TelegramCockpitError::Auth(teloxide::types::ChatId(chat_id)));
    }
    use teloxide::prelude::Requester as _;
    bot.send_message(teloxide::types::ChatId(chat_id), text)
        .await?;
    Ok(())
}

pub(super) async fn send_pairing_reply(
    bot: &teloxide::Bot,
    chat_id: i64,
    text: &str,
) -> Result<()> {
    use teloxide::prelude::Requester as _;
    bot.send_message(teloxide::types::ChatId(chat_id), text)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_persistence::{runs::Storage, telegram::pairings};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn bot() -> (MockServer, teloxide::Bot) {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/bottest/SendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok":true,"result":{"message_id":99,"date":0,"chat":{"id":42,"type":"private"},"text":"result"}})))
            .mount(&server).await;
        let bot = teloxide::Bot::new("test").set_api_url(server.uri().parse().unwrap());
        (server, bot)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revoked_command_reply_never_reaches_bot_endpoint() {
        let home = tempfile::tempdir().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let admission = PairingsAdmission {
            storage: storage.clone(),
        };
        let (server, bot) = bot().await;
        let conn = storage.acquire_registry_conn().unwrap();
        pairings::pair(&conn, 42, "phone", 100).unwrap();
        // Models revocation after handler admission and before its awaited result returns.
        pairings::revoke(&conn, 42, 200).unwrap();
        drop(conn);
        assert!(
            send_reply(&bot, &admission, 42, "sensitive status reply")
                .await
                .is_err()
        );
        assert!(server.received_requests().await.unwrap().is_empty());
        pairings::pair(&storage.acquire_registry_conn().unwrap(), 42, "phone", 300).unwrap();
        send_reply(&bot, &admission, 42, "admitted status reply")
            .await
            .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        storage
            .acquire_registry_conn()
            .unwrap()
            .execute("DROP TABLE telegram_pairings", [])
            .unwrap();
        assert!(matches!(
            send_reply(&bot, &admission, 42, "lookup failure secret").await,
            Err(TelegramCockpitError::Persistence(_))
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairing_reply_remains_available_without_admission() {
        let (server, bot) = bot().await;
        send_pairing_reply(&bot, 42, "pairing result")
            .await
            .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
