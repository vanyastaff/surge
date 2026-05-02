use criterion::{Criterion, criterion_group, criterion_main};
use std::path::PathBuf;
use surge_core::{
    approvals::ApprovalPolicy,
    run_event::{EventPayload, RunConfig},
    sandbox::SandboxMode,
};

fn event_serde_roundtrip(c: &mut Criterion) {
    let payload = EventPayload::RunStarted {
        pipeline_template: None,
        project_path: PathBuf::from("/work"),
        initial_prompt: "test".into(),
        config: RunConfig {
            sandbox_default: SandboxMode::WorkspaceWrite,
            approval_default: ApprovalPolicy::OnRequest,
            auto_pr: false,
        },
    };
    c.bench_function("event_serde_roundtrip", |b| {
        b.iter(|| {
            let bytes = payload.to_bincode().unwrap();
            let _: EventPayload = EventPayload::from_bincode(&bytes).unwrap();
        })
    });
}

criterion_group!(benches, event_serde_roundtrip);
criterion_main!(benches);
