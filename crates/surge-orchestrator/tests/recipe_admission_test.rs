//! Host-effect oracle: rejected admission must never call the transport.
mod fixtures;
use fixtures::mock_bridge::{MockBridge, RecordedCall};
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;
use std::{collections::BTreeMap, sync::Arc};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::{AgentKind, AlwaysAllowSandbox, SessionConfig};
use surge_core::id::{ExecutionWriterId, StageInvocationId};
use surge_orchestrator::recipe_admission::RecipeAdmissionBridge;
use surge_persistence::runs::Storage;

fn config(writer: ExecutionWriterId, invocation: StageInvocationId) -> SessionConfig {
    SessionConfig {
        effect_fence: None,
        writer_id: writer,
        invocation,
        runtime: "mock".into(),
        opening: Default::default(),
        config_selections: vec![],
        stage_mcp: None,
        agent_kind: AgentKind::Mock { args: vec![] },
        working_dir: std::env::temp_dir(),
        system_prompt: "test".into(),
        declared_outcomes: vec!["done".try_into().unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: Default::default(),
        bindings: BTreeMap::new(),
        env: BTreeMap::new(),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeat_writer_cannot_replay_provider_rpc() {
    let home = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(home.path()).await.unwrap();
        let mock = Arc::new(MockBridge::new());
        let bridge = RecipeAdmissionBridge::new(mock.clone(), storage.work_items());
        let writer = ExecutionWriterId::new();
        let invocation = StageInvocationId::new();
        bridge
            .open_session(config(writer, invocation))
            .await
            .unwrap();
        assert!(
            bridge
                .open_session(config(writer, invocation))
                .await
                .is_err(),
            "immutable replay is not RPC authority"
        );
        assert!(
            bridge
                .open_session(config(writer, StageInvocationId::new()))
                .await
                .is_err(),
            "changed replay is not RPC authority"
        );
        assert_eq!(
            mock.recorded_calls
                .lock()
                .await
                .iter()
                .filter(|call| matches!(call, RecordedCall::OpenSession))
                .count(),
            1
        );
    }
    home.close().unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_failure_prevents_provider_rpc() {
    let home = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(home.path()).await.unwrap();
        storage
            .acquire_registry_conn()
            .unwrap()
            .execute("DROP TABLE recipe_opening_admissions", [])
            .unwrap();
        let mock = Arc::new(MockBridge::new());
        let bridge = RecipeAdmissionBridge::new(mock.clone(), storage.work_items());
        assert!(
            bridge
                .open_session(config(ExecutionWriterId::new(), StageInvocationId::new()))
                .await
                .is_err()
        );
        assert!(mock.recorded_calls.lock().await.is_empty());
    }
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_opening_receipt_and_restart_never_grant_replay() {
    use surge_core::execution_recovery::{
        ProviderSessionDescriptor, ProviderSessionId, SessionOpening, SessionRestoreCapabilities,
    };
    let home = FixtureHome::new().unwrap();
    {
        let storage = Storage::open(home.path()).await.unwrap();
        let mock = Arc::new(MockBridge::new());
        let bridge = RecipeAdmissionBridge::new(mock.clone(), storage.work_items());
        let writer = ExecutionWriterId::new();
        let invocation = StageInvocationId::new();
        let mut mismatched = config(writer, invocation);
        mismatched.opening = SessionOpening::Continue(
            ProviderSessionDescriptor::new(
                ProviderSessionId::new("fixture-provider".into()).unwrap(),
                invocation,
                "wrong-runtime".into(),
                surge_core::ContentHash::compute(format!("{:?}", mismatched.agent_kind).as_bytes()),
                std::env::temp_dir(),
                SessionRestoreCapabilities {
                    resume: true,
                    load: true,
                },
            )
            .unwrap(),
        );
        assert!(
            bridge.open_session(mismatched).await.is_err(),
            "opening receipt must match admission"
        );
        assert!(
            mock.recorded_calls
                .lock()
                .await
                .iter()
                .any(|call| matches!(call, RecordedCall::CloseSession(_))),
            "mismatched writer must be closed"
        );
        let reopened = Storage::open(home.path()).await.unwrap();
        let bridge = RecipeAdmissionBridge::new(mock.clone(), reopened.work_items());
        assert!(
            bridge
                .open_session(config(writer, invocation))
                .await
                .is_err()
        );
        assert_eq!(
            mock.recorded_calls
                .lock()
                .await
                .iter()
                .filter(|call| matches!(call, RecordedCall::OpenSession))
                .count(),
            1
        );
        let unknown: i64 = reopened
            .acquire_registry_conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM recipe_opening_admissions WHERE exhaustion_receipt IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            unknown, 1,
            "failed opening remains a durable Unknown barrier"
        );
    }
    home.close().unwrap();
}
