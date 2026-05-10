//! Criterion benchmark: per-stage transition latency for a synchronous
//! `Branch` node — `StageEntered → OutcomeReported → EdgeTraversed`. Branch
//! is the cleanest target because there is no agent in the loop; the bench
//! measures the engine + persistence overhead only.
//!
//! ## Budget
//!
//! `P95_BUDGET_US` encodes the per-transition budget. The plan rule is:
//! "p95 of the first clean baseline plus 25% headroom". The first checked-in
//! value is a *seed* — operators are expected to overwrite it after their
//! first `cargo bench --bench stage_transition --save-baseline ci` run on
//! the CI machine. The CI gate compares against this constant, so a deliberate
//! bump requires a code change and review.
//!
//! Run locally:
//!
//! ```text
//! cargo bench -p surge-orchestrator --bench stage_transition
//! ```
//!
//! Save a baseline:
//!
//! ```text
//! cargo bench -p surge-orchestrator --bench stage_transition -- --save-baseline ci
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use surge_core::branch_config::{BranchArm, BranchConfig, Predicate};
use surge_core::edge::EdgeKind;
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_orchestrator::engine::stage::branch::{BranchStageParams, execute_branch_stage};
use surge_persistence::runs::{RunWriter, Storage};
use tokio::runtime::Runtime;

/// Per-transition wall-clock budget in microseconds. CI bench gate compares
/// every run's p95 against this value. See module-level docs for the rule.
pub const P95_BUDGET_US: u64 = 5_000;
const BUDGET_CHECK_ENV: &str = "SURGE_STAGE_TRANSITION_BUDGET_CHECK";
const BUDGET_SAMPLE_COUNT: usize = 64;
const WARMUP_TRANSITIONS: usize = 8;

struct BenchFixture {
    rt: Runtime,
    _tempdir: tempfile::TempDir,
    storage: std::sync::Arc<Storage>,
    writer_path: PathBuf,
    cfg: BranchConfig,
    node: NodeKey,
    edge: EdgeKey,
    next: NodeKey,
    memory: RunMemory,
}

fn build_fixture() -> BenchFixture {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime for criterion bench");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let writer_path = tempdir.path().to_path_buf();
    let storage = rt.block_on(async {
        Storage::open(&writer_path)
            .await
            .expect("open storage for bench")
    });

    let cfg = BranchConfig {
        predicates: vec![BranchArm {
            condition: Predicate::FileExists {
                path: "Cargo.toml".into(),
            },
            outcome: OutcomeKey::try_from("rust").unwrap(),
        }],
        default_outcome: OutcomeKey::try_from("generic").unwrap(),
    };
    std::fs::write(writer_path.join("Cargo.toml"), b"x").expect("seed Cargo.toml");

    BenchFixture {
        rt,
        _tempdir: tempdir,
        storage,
        writer_path,
        cfg,
        node: NodeKey::try_from("decide").unwrap(),
        edge: EdgeKey::try_from("e_decide_end").unwrap(),
        next: NodeKey::try_from("end").unwrap(),
        memory: RunMemory::default(),
    }
}

async fn create_run_writer(fixture: &BenchFixture) -> RunWriter {
    fixture
        .storage
        .create_run(RunId::new(), &fixture.writer_path, None)
        .await
        .expect("create_run")
}

async fn execute_one_transition(fixture: &BenchFixture, writer: &RunWriter) {
    writer
        .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
            node: fixture.node.clone(),
            attempt: 1,
        }))
        .await
        .expect("append StageEntered");

    let outcome = execute_branch_stage(BranchStageParams {
        node: &fixture.node,
        branch_config: &fixture.cfg,
        writer,
        run_memory: &fixture.memory,
        worktree_root: &fixture.writer_path,
    })
    .await
    .expect("branch stage");

    writer
        .append_event(VersionedEventPayload::new(EventPayload::EdgeTraversed {
            edge: fixture.edge.clone(),
            from: fixture.node.clone(),
            to: fixture.next.clone(),
            kind: EdgeKind::Forward,
        }))
        .await
        .expect("append EdgeTraversed");
    writer
        .append_event(VersionedEventPayload::new(EventPayload::StageCompleted {
            node: fixture.node.clone(),
            outcome,
        }))
        .await
        .expect("append StageCompleted");
}

fn close_writer(fixture: &BenchFixture, writer: RunWriter) {
    fixture
        .rt
        .block_on(async { writer.close().await.expect("close writer") });
}

fn sample_stage_transition_latencies(fixture: &BenchFixture, samples: usize) -> Vec<Duration> {
    let writer = fixture.rt.block_on(create_run_writer(fixture));
    let durations = fixture.rt.block_on(async {
        for _ in 0..WARMUP_TRANSITIONS {
            execute_one_transition(fixture, &writer).await;
        }

        let mut durations = Vec::with_capacity(samples);
        for _ in 0..samples {
            let started = Instant::now();
            execute_one_transition(fixture, &writer).await;
            durations.push(started.elapsed());
        }
        durations
    });
    close_writer(fixture, writer);
    durations
}

fn p95_micros(samples: &[Duration]) -> u128 {
    assert!(!samples.is_empty(), "p95 requires at least one sample");
    let mut micros: Vec<u128> = samples.iter().map(Duration::as_micros).collect();
    micros.sort_unstable();
    let rank = (micros.len() * 95).div_ceil(100).saturating_sub(1);
    micros[rank]
}

fn enforce_budget_if_requested(fixture: &BenchFixture) {
    if std::env::var_os(BUDGET_CHECK_ENV).is_none() {
        return;
    }

    let samples = sample_stage_transition_latencies(fixture, BUDGET_SAMPLE_COUNT);
    let observed_p95 = p95_micros(&samples);
    assert!(
        observed_p95 <= u128::from(P95_BUDGET_US),
        "stage_transition p95 budget exceeded: observed {observed_p95}us, budget {P95_BUDGET_US}us"
    );
}

fn branch_stage_transition(c: &mut Criterion) {
    let fixture = build_fixture();
    enforce_budget_if_requested(&fixture);

    let mut group = c.benchmark_group("engine.stage_transition");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(50);

    group.bench_function("branch_node_one_transition", |b| {
        b.iter_custom(|iters| {
            let writer = fixture.rt.block_on(create_run_writer(&fixture));
            let start = Instant::now();
            fixture.rt.block_on(async {
                for _ in 0..iters {
                    execute_one_transition(&fixture, &writer).await;
                }
            });
            let elapsed = start.elapsed();
            close_writer(&fixture, writer);
            elapsed
        });
    });

    group.finish();
}

criterion_group!(benches, branch_stage_transition);
criterion_main!(benches);
