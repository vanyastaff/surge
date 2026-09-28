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
    let config = AgentConfig {
        command: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
        args: if reports_usage {
            vec!["--usage".into()]
        } else {
            vec![]
        },
        transport: Transport::Stdio,
        mcp_servers: vec![],
        capabilities: vec![],
        env: BTreeMap::new(),
        settings_files: vec![],
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
    let mut reported = vec![];
    while let Ok(event) = events.try_recv() {
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
    if reports_usage {
        assert_eq!(reported, vec![(80, 20)]);
    } else {
        assert!(
            reported.is_empty(),
            "missing usage must remain unknown, not fabricated zero consumption"
        );
    }
}
