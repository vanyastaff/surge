//! Real worker/child oracle for the injected final admission contract.
//! Host SQL predicates and real ownership leases are checked by host tests.
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use surge_acp::bridge::*;

struct Fence {
    checks: AtomicUsize,
    target: usize,
    permitted: AtomicBool,
    reached: tokio::sync::Notify,
    release: (Mutex<bool>, Condvar),
}

impl Fence {
    fn new(target: usize) -> Arc<Self> {
        Arc::new(Self {
            checks: AtomicUsize::new(0),
            target,
            permitted: AtomicBool::new(true),
            reached: tokio::sync::Notify::new(),
            release: (Mutex::new(false), Condvar::new()),
        })
    }
    async fn reached(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.reached.notified())
            .await
            .unwrap();
    }
    fn refuse_and_release(&self) {
        self.permitted.store(false, Ordering::SeqCst);
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_all();
    }
}

impl HostEffectFence for Fence {
    fn check(&self) -> Result<(), HostEffectRefused> {
        if self.checks.fetch_add(1, Ordering::SeqCst) + 1 == self.target {
            self.reached.notify_one();
            let (_released, timeout) = self
                .release
                .1
                .wait_timeout_while(
                    self.release.0.lock().unwrap(),
                    Duration::from_secs(4),
                    |released| !*released,
                )
                .unwrap();
            if timeout.timed_out() {
                return Err(HostEffectRefused);
            }
        }
        if self.permitted.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(HostEffectRefused)
        }
    }
}

fn config(root: &Path, fence: Arc<Fence>) -> SessionConfig {
    SessionConfig {
        effect_fence: Some(fence),
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        stage_mcp: None,
        agent_kind: AgentKind::Custom {
            binary: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
            args: vec![
                "--wire-all".into(),
                "--wire-log".into(),
                root.join("wire").display().to_string(),
                "--pid-file".into(),
                root.join("pid").display().to_string(),
                "--config-options".into(),
                "--config-file".into(),
                root.join("choices").display().to_string(),
            ],
        },
        working_dir: root.into(),
        system_prompt: "test".into(),
        declared_outcomes: vec![surge_core::OutcomeKey::try_from("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: Default::default(),
        bindings: BTreeMap::new(),
        env: BTreeMap::new(),
        config_selections: vec![],
    }
}

fn operations(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("wire"))
        .unwrap_or_default()
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["operation"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

async fn next_permission(events: &mut tokio::sync::broadcast::Receiver<BridgeEvent>) -> String {
    loop {
        if let BridgeEvent::PermissionRequested { request_id, .. } = events.recv().await.unwrap() {
            return request_id;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_rechecks_spawn_initialize_new_session_and_option_after_queue_admission() {
    for target in 1..=7 {
        let root = tempfile::tempdir().unwrap();
        let fence = Fence::new(target);
        let bridge = Arc::new(AcpBridge::with_defaults().unwrap());
        let mut request = config(root.path(), fence.clone());
        if target >= 6 {
            request.config_selections.push(session::ConfigSelection {
                category: session::ConfigCategory::Model,
                value: "Mock Opus".into(),
                best_effort: false,
            });
        }
        let caller = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.open_session(request).await }
        });
        fence.reached().await;
        fence.refuse_and_release();
        assert!(matches!(
            caller.await.unwrap(),
            Err(OpenSessionError::HostEffectRefused(_))
        ));
        assert_eq!(fence.checks.load(Ordering::SeqCst), target);
        assert!(
            target != 1 || !root.path().join("pid").exists(),
            "physical spawn ran after first refusal"
        );
        Arc::try_unwrap(bridge)
            .ok()
            .unwrap()
            .shutdown()
            .await
            .unwrap();
        let expected = match target {
            1..=3 => vec![],
            4 | 5 => vec!["initialize"],
            _ => vec!["initialize", "new_session"],
        };
        assert_eq!(operations(root.path()), expected);
        assert!(!root.path().join("choices").exists());
        assert_eq!(
            Arc::strong_count(&fence),
            1,
            "failed opening retained an unsettled effect owner"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_prompt_checks_stored_session_fence_and_close_remains_available() {
    for target in [6, 7] {
        let root = tempfile::tempdir().unwrap();
        let fence = Fence::new(target);
        let bridge = Arc::new(AcpBridge::with_defaults().unwrap());
        let opened = bridge
            .open_session(config(root.path(), fence.clone()))
            .await
            .unwrap();
        let caller = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .send_message(
                        opened.session,
                        MessageContent::Text("one pending prompt".into()),
                    )
                    .await
            }
        });
        fence.reached().await;
        fence.refuse_and_release();
        assert!(matches!(
            caller.await.unwrap(),
            Err(SendMessageError::HostEffectRefused(_))
        ));
        assert_eq!(operations(root.path()), vec!["initialize", "new_session"]);
        bridge.close_session(opened.session).await.unwrap();
        Arc::try_unwrap(bridge)
            .ok()
            .unwrap()
            .shutdown()
            .await
            .unwrap();
        assert_eq!(Arc::strong_count(&fence), 1);
    }
}

#[derive(Clone)]
struct Elevate;
impl Sandbox for Elevate {
    fn visibility(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn allows_tool(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Elevate {
            capability: "fixture".into(),
        }
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_grant_is_checked_after_approval_await_and_automatic_allow() {
    for elevated in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let fence = Fence::new(8);
        let bridge = Arc::new(AcpBridge::with_defaults().unwrap());
        let mut request = config(root.path(), fence.clone());
        if elevated {
            request.sandbox = Box::new(Elevate);
        }
        if let AgentKind::Custom { args, .. } = &mut request.agent_kind {
            args.push("--permission".into());
        }
        let opened = bridge.open_session(request).await.unwrap();
        let mut events = bridge.subscribe();
        let caller = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .send_message(
                        opened.session,
                        MessageContent::Text("permission request".into()),
                    )
                    .await
            }
        });
        if elevated {
            let request_id =
                tokio::time::timeout(Duration::from_secs(3), next_permission(&mut events))
                    .await
                    .unwrap();
            bridge
                .reply_to_permission(
                    opened.session,
                    request_id,
                    RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                        SelectedPermissionOutcome::new("allow"),
                    )),
                )
                .await
                .unwrap();
        }
        fence.reached().await;
        fence.refuse_and_release();
        assert!(caller.await.unwrap().is_err());
        let wire: Vec<serde_json::Value> = std::fs::read_to_string(root.path().join("wire"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let response = wire
            .iter()
            .find(|row| row["operation"] == "current_permission_response")
            .unwrap();
        assert_eq!(response["request"]["outcome"]["outcome"], "cancelled");
        bridge.close_session(opened.session).await.unwrap();
        Arc::try_unwrap(bridge)
            .ok()
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_awaiter_does_not_drop_the_queued_worker_capability() {
    let root = tempfile::tempdir().unwrap();
    let fence = Fence::new(1);
    let bridge = Arc::new(AcpBridge::with_defaults().unwrap());
    let request = config(root.path(), fence.clone());
    let caller = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.open_session(request).await }
    });
    fence.reached().await;
    caller.abort();
    assert!(matches!(caller.await,Err(error) if error.is_cancelled()));
    assert!(
        Arc::strong_count(&fence) > 1,
        "queued opening lost its own capability"
    );
    fence.refuse_and_release();
    Arc::try_unwrap(bridge)
        .ok()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    assert_eq!(Arc::strong_count(&fence), 1);
    assert!(!root.path().join("pid").exists());
}
