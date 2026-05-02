use chrono::Utc;
use criterion::{Criterion, criterion_group, criterion_main};
use std::path::PathBuf;
use surge_core::{
    approvals::ApprovalPolicy,
    id::{RunId, SessionId},
    keys::NodeKey,
    run_event::{EventPayload, RunConfig, RunEvent},
    run_state::fold,
    sandbox::SandboxMode,
};

fn make_event(seq: u64, payload: EventPayload) -> RunEvent {
    RunEvent {
        run_id: RunId::new(),
        seq,
        timestamp: Utc::now(),
        payload,
    }
}

fn build_typical_event_log(n: usize) -> Vec<RunEvent> {
    let mut events = Vec::with_capacity(n);
    events.push(make_event(
        1,
        EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/work"),
            initial_prompt: "test".into(),
            config: RunConfig {
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
            },
        },
    ));
    let _node = NodeKey::try_from("impl_1").unwrap();
    for i in 1..n {
        events.push(make_event(
            (i + 1) as u64,
            EventPayload::TokensConsumed {
                session: SessionId::new(),
                prompt_tokens: 1000,
                output_tokens: 500,
                cache_hits: 100,
                model: "claude-opus-4-7".into(),
                cost_usd: Some(0.03),
            },
        ));
    }
    events
}

fn fold_1k_typical(c: &mut Criterion) {
    let events = build_typical_event_log(1000);
    c.bench_function("fold_1k_events_typical_graph", |b| {
        b.iter(|| fold(criterion::black_box(&events)).unwrap())
    });
}

fn fold_10k_typical(c: &mut Criterion) {
    let events = build_typical_event_log(10_000);
    c.bench_function("fold_10k_events_typical_graph", |b| {
        b.iter(|| fold(criterion::black_box(&events)).unwrap())
    });
}

criterion_group!(benches, fold_1k_typical, fold_10k_typical);
criterion_main!(benches);
