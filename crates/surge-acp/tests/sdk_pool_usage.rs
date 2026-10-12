//! Preserve optional v1 end-turn usage on the live legacy pool path.
use agent_client_protocol::schema::v1::{ContentBlock, TextContent};
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};
use surge_acp::{client::PermissionPolicy, pool::AgentPool};
use surge_core::{
    SurgeEvent,
    config::{AgentConfig, ResilienceConfig, Transport},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_prompt_usage_preserves_unknown_and_emits_exactly_once() {
    tokio::time::timeout(Duration::from_secs(20), async {
        exercise(false).await;
        exercise(true).await;
    })
    .await
    .unwrap();
}
async fn exercise(reports_usage: bool) {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("usage.json");
    std::fs::write(
        &script,
        r#"[
        {"usage":{"totalTokens":100,"inputTokens":80,"outputTokens":20}},
        {"usage":{"totalTokens":100,"inputTokens":80,"outputTokens":20}},
        {"usage":{"totalTokens":10,"inputTokens":8,"outputTokens":2}},
        {"usage":null}
    ]"#,
    )
    .unwrap();
    let config = AgentConfig {
        command: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
        args: if reports_usage {
            vec!["--usage-script".into(), script.to_string_lossy().into()]
        } else {
            vec![]
        },
        transport: Transport::Stdio,
        mcp_servers: vec![],
        capabilities: vec![],
        env: BTreeMap::new(),
        settings_files: vec![],
        capacity_route: None,
    };
    let pool = AgentPool::new(
        HashMap::from([("fixture".into(), config)]),
        "fixture".into(),
        root.path().into(),
        PermissionPolicy::AutoApprove,
        ResilienceConfig::default(),
    )
    .unwrap();
    let mut events = pool.subscribe();
    let session = pool.create_session(None, None, root.path()).await.unwrap();
    let response = pool
        .prompt(
            &session,
            vec![ContentBlock::Text(TextContent::new("usage oracle"))],
        )
        .await
        .unwrap();
    assert_eq!(response.usage.is_some(), reports_usage);
    pool.prompt(
        &session,
        vec![ContentBlock::Text(TextContent::new("repeat usage"))],
    )
    .await
    .unwrap();
    if reports_usage {
        for _ in 0..2 {
            pool.prompt(
                &session,
                vec![ContentBlock::Text(TextContent::new(
                    "regression or missing",
                ))],
            )
            .await
            .unwrap();
        }
    }
    let mut reported = vec![];
    let mut observations = vec![];
    while let Ok(event) = events.try_recv() {
        if let SurgeEvent::ProviderUsageObserved { usage, .. } = &event {
            observations.push(usage.clone());
        }
        if let SurgeEvent::TokensConsumed {
            input_tokens,
            output_tokens,
            ..
        } = event
        {
            reported.push((input_tokens, output_tokens));
        }
    }
    pool.shutdown().await.unwrap();
    assert_eq!(observations.len(), if reports_usage { 4 } else { 2 });
    if reports_usage {
        assert_eq!(reported, vec![(80, 20), (0, 0)]);
        assert!(
            matches!(&observations[2], surge_core::usage::TokenUsageObservation::Unavailable { reason: surge_core::usage::UsageUnavailableReason::CounterRegression, reported: Some(raw) } if raw.total_tokens == 10)
        );
        assert!(matches!(
            &observations[3],
            surge_core::usage::TokenUsageObservation::Unavailable {
                reason: surge_core::usage::UsageUnavailableReason::Missing,
                reported: None
            }
        ));
    } else {
        assert!(observations.iter().all(|usage| matches!(
            usage,
            surge_core::usage::TokenUsageObservation::Unavailable {
                reason: surge_core::usage::UsageUnavailableReason::Missing,
                reported: None
            }
        )));
        assert!(
            reported.is_empty(),
            "missing usage must remain unknown, not fabricated zero consumption"
        );
    }
}
