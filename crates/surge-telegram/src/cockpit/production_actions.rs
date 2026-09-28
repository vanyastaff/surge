//! Card-bound operator actions. Telegram acknowledgement precedes journal/engine work.
use crate::{
    CardStore,
    cockpit::{
        callback::{CallbackCtx, CallbackOutcome, EngineResolver},
        production::ProductionRoutes,
    },
    error::{Result, TelegramCockpitError},
};
use teloxide::{
    payloads::{AnswerCallbackQuerySetters, SendMessageSetters},
    prelude::Requester,
    types::{ChatId, ForceReply},
};

/// Presentation persistence is separate from the already accepted decision.
enum AcceptedCardStatus {
    Closed,
    UpdateFailed,
}

impl ProductionRoutes {
    pub(super) async fn dispatch_callback(&self, chat_id: i64, data: &str, query_id: &str) {
        // Stop Telegram's spinner even when admission, storage or engine resolution fails.
        if let Err(error) = self
            .bot
            .answer_callback_query(query_id.to_owned())
            .text("Processing…")
            .await
        {
            tracing::warn!(error = %crate::error::TelegramCockpitError::from(error),"callback acknowledgement failed");
        }
        if data.starts_with("inbox:") {
            if !self.admit(chat_id).await {
                return;
            }
            let message = match &self.inbox {
                Some(inbox) => match inbox.handle(chat_id, data).await {
                    Ok(message) => message,
                    Err(error) => action_error(&error),
                },
                None => "Inbox actions are not configured.".to_owned(),
            };
            self.send_reply(chat_id, &message).await;
            return;
        }
        let ctx = CallbackCtx {
            store: self.store.clone(),
            admission: self.admission.clone(),
            engine: self.engine.clone(),
        };
        let outcome = crate::cockpit::callback::handle_callback(chat_id, data, &ctx).await;
        let text = match outcome {
            Ok(CallbackOutcome::Resolved { .. }) => "Decision accepted.".to_owned(),
            Ok(CallbackOutcome::ResolvedCardUpdateFailed { .. }) => {
                accepted_card_update_failed("Decision")
            },
            Ok(CallbackOutcome::PendingEditFeedback { card_id, .. }) => {
                match self.request_edit(chat_id, &card_id).await {
                    Ok(()) => return,
                    Err(error) => action_error(&error),
                }
            },
            Ok(CallbackOutcome::StaleTap { .. }) => {
                "This approval is stale or already answered. Use the current card.".to_owned()
            },
            Ok(CallbackOutcome::AdmissionDenied { .. }) => {
                "This chat cannot act on this card.".to_owned()
            },
            Ok(CallbackOutcome::Acknowledged { .. }) => "Acknowledged.".to_owned(),
            Ok(CallbackOutcome::NotImplemented { .. }) => {
                "This action is not available yet.".to_owned()
            },
            Ok(CallbackOutcome::Unknown { .. }) => "Unknown action.".to_owned(),
            Err(error) => action_error(&error),
        };
        self.send_reply(chat_id, &text).await;
    }

    async fn request_edit(&self, chat_id: i64, card_id: &str) -> Result<()> {
        let card = self
            .store
            .find_by_id(card_id)
            .await?
            .ok_or(TelegramCockpitError::CardNotFound)?;
        self.engine.card_request(&card).await?;
        if card.chat_id != chat_id {
            return Err(TelegramCockpitError::CardClosed);
        }
        if card.pending_edit_prompt_message_id.is_some() {
            self.send_reply(
                chat_id,
                "Reply to the existing feedback prompt for this card.",
            )
            .await;
            return Ok(());
        }
        let message = self
            .bot
            .send_message(
                ChatId(chat_id),
                "Reply with the changes requested for this approval.",
            )
            .reply_markup(ForceReply::new())
            .await?;
        let conn = self
            .store
            .storage
            .acquire_registry_conn()
            .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        surge_persistence::telegram::cards::set_edit_prompt(
            &conn,
            card_id,
            i64::from(message.id.0),
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
        Ok(())
    }

    pub(super) async fn dispatch_edit_reply(&self, chat_id: i64, reply_to: i64, text: &str) {
        if !self.admit(chat_id).await {
            return;
        }
        let result = self.resolve_edit_reply(chat_id, reply_to, text).await;
        let message = match result {
            Ok(AcceptedCardStatus::Closed) => "Feedback accepted.".to_owned(),
            Ok(AcceptedCardStatus::UpdateFailed) => accepted_card_update_failed("Feedback"),
            Err(error) => action_error(&error),
        };
        self.send_reply(chat_id, &message).await;
    }

    async fn resolve_edit_reply(
        &self,
        chat_id: i64,
        reply_to: i64,
        text: &str,
    ) -> Result<AcceptedCardStatus> {
        let card = {
            let conn = self
                .store
                .storage
                .acquire_registry_conn()
                .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?;
            surge_persistence::telegram::cards::find_by_edit_prompt(&conn, chat_id, reply_to)
                .map_err(|e| TelegramCockpitError::Persistence(e.to_string()))?
                .ok_or(TelegramCockpitError::CardClosed)?
        };
        if text.trim().is_empty() {
            return Err(TelegramCockpitError::CardClosed);
        }
        self.engine
            .resolve_card(&card, serde_json::json!({"outcome":"edit","comment":text}))
            .await?;
        match self
            .store
            .close(&card.card_id, chrono::Utc::now().timestamp_millis())
            .await
        {
            Ok(()) => Ok(AcceptedCardStatus::Closed),
            Err(error) => {
                tracing::warn!(%error, card_id = %card.card_id, "feedback accepted but card close failed");
                Ok(AcceptedCardStatus::UpdateFailed)
            },
        }
    }
}

fn action_error(error: &TelegramCockpitError) -> String {
    tracing::warn!(%error,"operator action was not accepted");
    match error {
        TelegramCockpitError::CardClosed
        | TelegramCockpitError::CardNotFound
        | TelegramCockpitError::EngineResolve(
            surge_orchestrator::engine::EngineError::StaleGateRequest,
        ) => "This approval is stale or already answered. Reply to the current feedback prompt."
            .into(),
        _ => "The decision could not be accepted. Check daemon logs and retry.".into(),
    }
}

fn accepted_card_update_failed(subject: &str) -> String {
    format!(
        "{subject} accepted; card status update failed. Do not submit again. Check daemon logs."
    )
}
