use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_usage_monotonic_and_precedes_session_end() {
    tokio::time::timeout(Duration::from_secs(30), inner_test())
        .await
        .expect("test exceeded 30s — likely deadlock in token_tracking path");
}

async fn inner_test() {
    run_usage_case(true).await;
    run_usage_case(false).await;
}

async fn run_usage_case(with_usage: bool) {
    let wt = TempDir::new().unwrap();

    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = fixture_config(
        wt.path(),
        if with_usage {
            vec!["--usage".into()]
        } else {
            vec![]
        },
    );

    let sid = bridge.open_session(cfg).await.unwrap().session;
    bridge
        .send_message(sid, MessageContent::Text("go".into()))
        .await
        .unwrap();

    let mut reports = Vec::new();
    let mut contexts = Vec::new();
    bridge.close_session(sid).await.unwrap();
    loop {
        match timeout(Duration::from_secs(8), events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            BridgeEvent::ProviderUsageObserved { session, usage } => {
                assert_eq!(session, sid);
                reports.push(usage);
            },
            BridgeEvent::ContextUsageObserved { session, usage } => {
                assert_eq!(session, sid);
                contexts.push(usage);
            },
            BridgeEvent::SessionEnded { session, .. } if session == sid => break,
            _ => {},
        }
    }
    assert_eq!(
        reports.len(),
        1,
        "response usage must be observed before end"
    );
    if with_usage {
        match &reports[0] {
            surge_core::usage::TokenUsageObservation::Reported { cumulative, delta } => {
                assert_eq!(cumulative.total_tokens, 100);
                assert_eq!(cumulative.input_tokens, 80);
                assert_eq!(cumulative.output_tokens, 20);
                assert_eq!(delta, cumulative);
            },
            other => panic!("expected reported usage, got {other:?}"),
        }
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].used, 100);
        assert_eq!(contexts[0].size, 200_000);
    } else {
        assert!(matches!(
            reports[0],
            surge_core::usage::TokenUsageObservation::Unavailable {
                reason: surge_core::usage::UsageUnavailableReason::Missing,
                reported: None
            }
        ));
        assert!(contexts.is_empty());
    }

    bridge.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cumulative_reports_and_context_remain_independent_across_turns() {
    timeout(Duration::from_secs(30), async {
        let root = TempDir::new().unwrap();
        let script = root.path().join("usage.json");
        std::fs::write(&script, r#"[
          {"usage":{"totalTokens":5000000000,"inputTokens":4000000000,"outputTokens":1000000000,"cachedReadTokens":7},"context":{"used":100,"size":200000,"cost":{"amount":0.25,"currency":"EUR"}}},
          {"usage":{"totalTokens":5000000000,"inputTokens":4000000000,"outputTokens":1000000000,"cachedReadTokens":7},"context":{"used":50,"size":200000}},
          {"usage":null,"context":{"used":200,"size":200000}},
          {"usage":{"totalTokens":10,"inputTokens":8,"outputTokens":2},"context":{"used":25,"size":200000}},
          {"usage":{"totalTokens":5000000030,"inputTokens":4000000020,"outputTokens":1000000010,"cachedReadTokens":9},"context":null}
        ]"#).unwrap();
        let bridge = AcpBridge::with_defaults().unwrap();
        let mut events = bridge.subscribe();
        let cfg = fixture_config(root.path(), vec!["--usage-script".into(), script.to_string_lossy().into()]);
        let session = bridge.open_session(cfg).await.unwrap().session;
        for _ in 0..5 {
            bridge.send_message(session, MessageContent::Text("turn".into())).await.unwrap();
        }
        bridge.close_session(session).await.unwrap();
        let mut reports = vec![];
        let mut contexts = vec![];
        loop {
            match events.recv().await.unwrap() {
                BridgeEvent::ProviderUsageObserved { usage, .. } => reports.push(usage),
                BridgeEvent::ContextUsageObserved { usage, .. } => contexts.push(usage),
                BridgeEvent::SessionEnded { .. } => break,
                _ => {},
            }
        }
        use surge_core::usage::{TokenUsageObservation as Observation, UsageUnavailableReason as Reason};
        assert_eq!(reports.len(), 5);
        assert!(matches!(&reports[0], Observation::Reported { cumulative, delta } if cumulative.total_tokens == 5_000_000_000 && delta == cumulative));
        assert!(matches!(&reports[1], Observation::Reported { delta, .. } if delta.total_tokens == 0 && delta.cached_read_tokens == Some(0)));
        assert!(matches!(&reports[2], Observation::Unavailable { reason: Reason::Missing, reported: None }));
        assert!(matches!(&reports[3], Observation::Unavailable { reason: Reason::CounterRegression, reported: Some(raw) } if raw.total_tokens == 10));
        assert!(matches!(&reports[4], Observation::Reported { delta, .. } if delta.total_tokens == 30 && delta.input_tokens == 20 && delta.output_tokens == 10 && delta.cached_read_tokens == Some(2)));
        assert_eq!(contexts.iter().map(|c| c.used).collect::<Vec<_>>(), vec![100,50,200,25]);
        assert_eq!(contexts[0].cost.as_ref().unwrap().currency, "EUR");
        assert_eq!(contexts[0].cost.as_ref().unwrap().amount, 0.25);
        assert!(contexts[1].cost.is_none());
        assert!(!matches!(events.try_recv(), Ok(BridgeEvent::ProviderUsageObserved { .. } | BridgeEvent::ContextUsageObserved { .. })));
        bridge.shutdown().await.unwrap();
    }).await.unwrap();
}

fn fixture_config(root: &std::path::Path, args: Vec<String>) -> SessionConfig {
    SessionConfig {
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        config_selections: Vec::new(),
        stage_mcp: None,
        agent_kind: AgentKind::Mock { args },
        working_dir: root.to_path_buf(),
        system_prompt: "stream".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sessions_have_independent_usage_baselines() {
    timeout(Duration::from_secs(30), async {
        let root = TempDir::new().unwrap();
        let bridge = AcpBridge::with_defaults().unwrap();
        let mut events = bridge.subscribe();
        let first = bridge
            .open_session(fixture_config(root.path(), vec!["--usage".into()]))
            .await
            .unwrap()
            .session;
        let second = bridge
            .open_session(fixture_config(root.path(), vec!["--usage".into()]))
            .await
            .unwrap()
            .session;
        for session in [first, second, first] {
            bridge
                .send_message(session, MessageContent::Text("turn".into()))
                .await
                .unwrap();
        }
        bridge.close_session(first).await.unwrap();
        bridge.close_session(second).await.unwrap();
        let mut deltas = vec![];
        let mut ended = vec![];
        while ended.len() < 2 {
            match events.recv().await.unwrap() {
                BridgeEvent::ProviderUsageObserved {
                    session,
                    usage: surge_core::usage::TokenUsageObservation::Reported { delta, .. },
                } => {
                    assert!(!ended.contains(&session));
                    deltas.push((session, delta.total_tokens));
                },
                BridgeEvent::SessionEnded { session, .. } => ended.push(session),
                _ => {},
            }
        }
        assert_eq!(deltas, vec![(first, 100), (second, 100), (first, 0)]);
        bridge.shutdown().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_cancel_observes_response_usage_before_session_end() {
    timeout(Duration::from_secs(30), async {
        run_cancel_case(false).await;
        run_cancel_case(true).await;
    })
    .await
    .unwrap();
}

async fn run_cancel_case(force: bool) {
    let root = TempDir::new().unwrap();
    let marker = root.path().join("prompt");
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();
    let mut args = vec![
        "--usage".into(),
        "--stall-prompt".into(),
        "--prompt-file".into(),
        marker.to_string_lossy().into(),
    ];
    if force {
        args.push("--ignore-cancel".into());
    }
    let sid = bridge
        .open_session(fixture_config(root.path(), args))
        .await
        .unwrap()
        .session;
    {
        let sending = bridge.send_message(sid, MessageContent::Text("wait".into()));
        tokio::pin!(sending);
        tokio::select! {
            result = &mut sending => panic!("prompt returned before cancellation: {result:?}"),
            () = async {
                while !marker.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
            } => {},
        }
        let closed = bridge.close_session(sid).await;
        if !force {
            closed.unwrap();
        }
        assert!(sending.await.is_err());
    }
    let mut saw_usage = false;
    loop {
        match events.recv().await.unwrap() {
            BridgeEvent::ProviderUsageObserved {
                usage: surge_core::usage::TokenUsageObservation::Reported { delta, .. },
                ..
            } => {
                assert!(!force);
                assert_eq!(delta.total_tokens, 100);
                saw_usage = true;
            },
            BridgeEvent::ProviderUsageObserved {
                usage:
                    surge_core::usage::TokenUsageObservation::Unavailable {
                        reason: surge_core::usage::UsageUnavailableReason::Missing,
                        ..
                    },
                ..
            } => {
                assert!(force);
                saw_usage = true;
            },
            BridgeEvent::SessionEnded { session, .. } if session == sid => break,
            _ => {},
        }
    }
    assert!(
        saw_usage,
        "graceful cancellation response usage was discarded"
    );
    bridge.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_prompt_marks_usage_unknown_before_session_end() {
    timeout(Duration::from_secs(30), async {
        let root = TempDir::new().unwrap();
        let bridge = AcpBridge::with_defaults().unwrap();
        let mut events = bridge.subscribe();
        let sid = bridge
            .open_session(fixture_config(
                root.path(),
                vec!["--scenario".into(), "prompt_error=rate_limit".into()],
            ))
            .await
            .unwrap()
            .session;
        assert!(
            bridge
                .send_message(sid, MessageContent::Text("fail".into()))
                .await
                .is_err()
        );
        bridge.close_session(sid).await.unwrap();
        let mut reports = vec![];
        loop {
            match events.recv().await.unwrap() {
                BridgeEvent::ProviderUsageObserved { usage, .. } => reports.push(usage),
                BridgeEvent::SessionEnded { .. } => break,
                _ => {},
            }
        }
        assert_eq!(reports.len(), 1);
        assert!(matches!(
            &reports[0],
            surge_core::usage::TokenUsageObservation::Unavailable {
                reason: surge_core::usage::UsageUnavailableReason::Missing,
                reported: None
            }
        ));
        assert!(!matches!(
            events.try_recv(),
            Ok(BridgeEvent::ProviderUsageObserved { .. })
        ));
        bridge.shutdown().await.unwrap();
    })
    .await
    .unwrap();
}
