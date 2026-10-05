//! Recheck durable pairing immediately before outgoing sensitive card requests.
use super::{Admission, PairingsAdmission, TelegramApi};
use crate::error::{Result, TelegramCockpitError};
use async_trait::async_trait;
use surge_notify::telegram::InboxKeyboardButton;

#[derive(Clone)]
pub(super) struct AdmittedTelegramApi<A> {
    pub(super) api: A,
    pub(super) admission: PairingsAdmission,
}

impl<A> AdmittedTelegramApi<A> {
    async fn require_admission(&self, chat_id: i64) -> Result<()> {
        if self.admission.is_admitted(chat_id).await? {
            Ok(())
        } else {
            tracing::info!(target: "telegram::auth", %chat_id, "outgoing card denied for unpaired chat");
            Err(TelegramCockpitError::Auth(teloxide::types::ChatId(chat_id)))
        }
    }
}

#[async_trait]
impl<A: TelegramApi> TelegramApi for AdmittedTelegramApi<A> {
    async fn send_message(
        &self,
        chat_id: i64,
        body_md: &str,
        keyboard: &[Vec<InboxKeyboardButton>],
    ) -> Result<i64> {
        self.require_admission(chat_id).await?;
        self.api.send_message(chat_id, body_md, keyboard).await
    }
    async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        body_md: &str,
        keyboard: &[Vec<InboxKeyboardButton>],
    ) -> Result<()> {
        self.require_admission(chat_id).await?;
        self.api
            .edit_message_text(chat_id, message_id, body_md, keyboard)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        card::emit::CardStore,
        cockpit::{production::SqliteCardStore, reconcile_open_cards},
        commands::status::RunSnapshotProvider,
    };
    use std::sync::{Arc, Mutex};
    use surge_core::id::RunId;
    use surge_persistence::{
        runs::{RunStatusSnapshot, Storage},
        telegram::pairings,
    };

    #[derive(Clone, Default)]
    struct RecordingApi {
        payloads: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl TelegramApi for RecordingApi {
        async fn send_message(
            &self,
            _chat_id: i64,
            body_md: &str,
            _keyboard: &[Vec<InboxKeyboardButton>],
        ) -> Result<i64> {
            self.payloads.lock().unwrap().push(body_md.to_owned());
            Ok(19)
        }
        async fn edit_message_text(
            &self,
            _chat_id: i64,
            _message_id: i64,
            body_md: &str,
            _keyboard: &[Vec<InboxKeyboardButton>],
        ) -> Result<()> {
            self.payloads.lock().unwrap().push(body_md.to_owned());
            Ok(())
        }
    }
    struct TerminalSnapshot;
    #[async_trait]
    impl RunSnapshotProvider for TerminalSnapshot {
        async fn snapshot(&self, run_id: RunId) -> Result<Option<RunStatusSnapshot>> {
            let mut snapshot = RunStatusSnapshot::empty(run_id);
            snapshot.terminal = true;
            snapshot.failed = true;
            snapshot.last_outcome = Some("sensitive recovery failure".into());
            Ok(Some(snapshot))
        }
    }
    async fn fixture() -> (
        tempfile::TempDir,
        Arc<Storage>,
        AdmittedTelegramApi<RecordingApi>,
    ) {
        let home = tempfile::tempdir().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let api = AdmittedTelegramApi {
            api: RecordingApi::default(),
            admission: PairingsAdmission {
                storage: storage.clone(),
            },
        };
        (home, storage, api)
    }
    fn pair(storage: &Storage) {
        pairings::pair(&storage.acquire_registry_conn().unwrap(), 42, "phone", 100).unwrap();
    }
    fn revoke(storage: &Storage) {
        pairings::revoke(&storage.acquire_registry_conn().unwrap(), 42, 200).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn outgoing_cards_recheck_pair_revoke_and_repair() {
        let (_home, storage, api) = fixture().await;
        assert!(api.send_message(42, "unpaired secret", &[]).await.is_err());
        assert!(
            api.edit_message_text(42, 19, "unpaired edit secret", &[])
                .await
                .is_err()
        );
        assert!(api.api.payloads.lock().unwrap().is_empty());
        pair(&storage);
        api.send_message(42, "paired", &[]).await.unwrap();
        api.edit_message_text(42, 19, "paired edit", &[])
            .await
            .unwrap();
        revoke(&storage);
        assert!(api.send_message(42, "revoked secret", &[]).await.is_err());
        assert!(
            api.edit_message_text(42, 19, "revoked edit secret", &[])
                .await
                .is_err()
        );
        assert_eq!(*api.api.payloads.lock().unwrap(), ["paired", "paired edit"]);
        pair(&storage);
        api.send_message(42, "repaired", &[]).await.unwrap();
        api.edit_message_text(42, 19, "repaired edit", &[])
            .await
            .unwrap();
        assert_eq!(
            *api.api.payloads.lock().unwrap(),
            ["paired", "paired edit", "repaired", "repaired edit"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revoked_reconcile_preserves_card_until_repair_without_sending() {
        let (_home, storage, api) = fixture().await;
        let store = SqliteCardStore {
            storage: storage.clone(),
        };
        let id = store
            .upsert(
                &RunId::new().to_string(),
                "gate",
                4,
                "human_gate",
                42,
                "hash",
                0,
            )
            .await
            .unwrap();
        store.mark_message_sent(&id, 19, "hash", 1).await.unwrap();
        pair(&storage);
        revoke(&storage);
        assert!(
            reconcile_open_cards(&store, &TerminalSnapshot, &api, 300)
                .await
                .is_err()
        );
        assert!(api.api.payloads.lock().unwrap().is_empty());
        assert!(
            store
                .find_by_id(&id)
                .await
                .unwrap()
                .unwrap()
                .closed_at
                .is_none()
        );
        pair(&storage);
        let report = reconcile_open_cards(&store, &TerminalSnapshot, &api, 400)
            .await
            .unwrap();
        assert_eq!(report.closed, 1);
        assert_eq!(api.api.payloads.lock().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairing_lookup_failure_is_reported_without_network_delegation() {
        let (_home, storage, api) = fixture().await;
        storage
            .acquire_registry_conn()
            .unwrap()
            .execute("DROP TABLE telegram_pairings", [])
            .unwrap();
        assert!(matches!(
            api.send_message(42, "secret", &[]).await,
            Err(crate::error::TelegramCockpitError::Persistence(_))
        ));
        assert!(api.api.payloads.lock().unwrap().is_empty());
    }
}
