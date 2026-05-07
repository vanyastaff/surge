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
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use surge_core::branch_config::{BranchArm, BranchConfig, Predicate};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::run_state::RunMemory;
use surge_orchestrator::engine::stage::branch::{BranchStageParams, execute_branch_stage};
use surge_persistence::runs::Storage;
use tokio::runtime::Runtime;

/// Per-transition wall-clock budget in microseconds. CI bench gate compares
/// every run's p95 against this value. See module-level docs for the rule.
pub const P95_BUDGET_US: u64 = 5_000;

struct BenchFixture {
    rt: Runtime,
    _tempdir: tempfile::TempDir,
    storage: std::sync::Arc<Storage>,
    writer_path: PathBuf,
    cfg: BranchConfig,
    node: NodeKey,
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
        memory: RunMemory::default(),
    }
}

fn branch_stage_transition(c: &mut Criterion) {
    let fixture = build_fixture();
    let mut group = c.benchmark_group("engine.stage_transition");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(50);

    group.bench_function("branch_node_one_transition", |b| {
        b.iter_custom(|iters| {
            let start = std::time::Instant::now();
            fixture.rt.block_on(async {
                for _ in 0..iters {
                    // Each iteration runs a fresh run because RunWriter is
                    // single-use; this dominates the measurement at low
                    // iters but stabilises at high `iters`.
                    let run_id = surge_core::id::RunId::new();
                    let writer = fixture
                        .storage
                        .create_run(run_id, &fixture.writer_path, None)
                        .await
                        .expect("create_run");
                    let _ = execute_branch_stage(BranchStageParams {
                        node: &fixture.node,
                        branch_config: &fixture.cfg,
                        writer: &writer,
                        run_memory: &fixture.memory,
                        worktree_root: &fixture.writer_path,
                    })
                    .await
                    .expect("branch stage");
                    writer.close().await.expect("close writer");
                }
            });
            start.elapsed()
        });
    });

    group.finish();
}

criterion_group!(benches, branch_stage_transition);
criterion_main!(benches);
