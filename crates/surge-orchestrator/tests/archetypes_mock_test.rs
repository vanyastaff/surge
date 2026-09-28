//! Task 5.1 — drive every bundled `examples/flow_*.toml` archetype through
//! the engine against a deterministic mock ACP bridge.
//!
//! The bridge chooses the first declared outcome for every agent session.
//! The examples are authored so those first outcomes follow the happy path,
//! which gives this suite a deterministic terminal run for every archetype
//! without requiring an external ACP binary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig, SessionState};
use surge_core::graph::Graph;
use surge_core::id::{RunId, SessionId};
use surge_core::keys::OutcomeKey;
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;
use tokio::sync::{Mutex, broadcast};

const ARCHETYPES: &[&str] = &[
    "flow_terminal_only.toml",
    "flow_minimal_agent.toml",
    "flow_linear_3.toml",
    "flow_single_loop.toml",
    "flow_multi_milestone.toml",
    "flow_bug_fix.toml",
    "flow_refactor.toml",
    "flow_spike.toml",
];

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

fn load_archetype(name: &str) -> Graph {
    let path = examples_dir().join(name);
    let toml_s =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&toml_s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Valid `spec` contract artifacts (kind=spec, schema_version=1), reused
/// from the repo's canonical fixtures — see
/// `crates/surge-orchestrator/tests/fixtures/artifacts/valid/`.
const SPEC_TOML: &str = include_str!("fixtures/artifacts/valid/spec.toml");
const SPEC_MD: &str = include_str!("fixtures/artifacts/valid/spec.md");
/// Valid `verification-report` contract artifact (kind=verification-report,
/// schema_version=1), required by `verifier@2.0`'s `passed`/`failed`
/// outcomes.
const VERIFICATION_REPORT_TOML: &str =
    include_str!("fixtures/artifacts/valid/verification-report.toml");

/// Files without a profile artifact contract (`implementer@2.0` declares no
/// `produced_artifacts`) but that downstream archetype stages bind by name
/// per their `append_system` prompt hint (e.g. `flow_bug_fix.toml`'s
/// `implement_1` binds `reproduce_1`'s `reproduction`). Content is
/// unconstrained since nothing validates it; only the file's existence and
/// stem (used as the artifact's logical name — see `logical_artifact_name`
/// in `engine/stage/agent.rs`) matter.
const UNCONTRACTED_ARTIFACTS: &[(&str, &str)] = &[
    ("reproduction.md", "# Reproduction\nSteps to reproduce.\n"),
    ("baseline.md", "# Baseline\nCharacterized behavior.\n"),
    ("changes.patch", "# Changes\nSummary of the diff.\n"),
    ("findings.md", "# Findings\nExperiment results.\n"),
];

struct DeterministicMockBridge {
    tx: broadcast::Sender<BridgeEvent>,
    outcomes: Mutex<HashMap<SessionId, OutcomeKey>>,
    working_dirs: Mutex<HashMap<SessionId, PathBuf>>,
}

impl DeterministicMockBridge {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            tx,
            outcomes: Mutex::new(HashMap::new()),
            working_dirs: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl BridgeFacade for DeterministicMockBridge {
    fn legacy_stage_event_adapter(&self) -> bool {
        true
    }
    async fn open_session(&self, config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        let session = SessionId::new();
        let outcome = config
            .declared_outcomes
            .first()
            .cloned()
            .unwrap_or_else(|| OutcomeKey::try_from("done").expect("'done' is a valid outcome"));
        self.outcomes.lock().await.insert(session, outcome);
        self.working_dirs
            .lock()
            .await
            .insert(session, config.working_dir);
        Ok(session)
    }

    async fn send_message(
        &self,
        session: SessionId,
        _content: MessageContent,
    ) -> Result<(), SendMessageError> {
        let outcome = self
            .outcomes
            .lock()
            .await
            .get(&session)
            .cloned()
            .unwrap_or_else(|| OutcomeKey::try_from("done").expect("'done' is a valid outcome"));

        // Every archetype stage's outcome may be bound downstream by name
        // (e.g. `flow_bug_fix.toml`'s `implement_1` binds `reproduce_1`'s
        // `reproduction`), and `spec-author@1.0`/`verifier@2.0` additionally
        // have a `produced_artifacts` contract (validated by
        // `validate_profile_artifact_contracts` in `engine/stage/agent.rs`)
        // requiring specific, schema-valid files. Writing the full superset
        // into every session's working dir and always reporting it is
        // harmless for profiles that don't need it —
        // `validate_profile_artifact_contracts` skips validation entirely
        // when a profile's outcome declares no `produced_artifacts`, and an
        // unbound artifact is simply never referenced — while satisfying
        // every archetype's actual requirement without needing this mock to
        // know which profile a session belongs to.
        let mut artifacts_produced = Vec::new();
        if let Some(working_dir) = self.working_dirs.lock().await.get(&session).cloned() {
            let _ = std::fs::write(working_dir.join("spec.toml"), SPEC_TOML);
            let _ = std::fs::write(working_dir.join("spec.md"), SPEC_MD);
            artifacts_produced.push("spec.toml".to_string());
            artifacts_produced.push("spec.md".to_string());
            let _ = std::fs::write(
                working_dir.join("verification-report.toml"),
                VERIFICATION_REPORT_TOML,
            );
            artifacts_produced.push("verification-report.toml".to_string());
            for (name, content) in UNCONTRACTED_ARTIFACTS {
                let _ = std::fs::write(working_dir.join(name), content);
                artifacts_produced.push((*name).to_string());
            }
        }

        let _ = self.tx.send(BridgeEvent::OutcomeReported {
            session,
            outcome,
            summary: "deterministic mock outcome".into(),
            artifacts_produced,
        });
        Ok(())
    }

    async fn session_state(&self, _session: SessionId) -> Result<SessionState, BridgeError> {
        Err(BridgeError::WorkerDead)
    }

    async fn close_session(&self, _session: SessionId) -> Result<(), CloseSessionError> {
        Ok(())
    }

    async fn reply_to_tool(
        &self,
        _session: SessionId,
        _call_id: String,
        _payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        Ok(())
    }

    async fn reply_to_permission(
        &self,
        _session: SessionId,
        _request_id: String,
        _response: agent_client_protocol::schema::v1::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.tx.subscribe()
    }
}

async fn run_archetype(name: &str) -> Vec<surge_persistence::runs::reader::ReadEvent> {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Storage::open(dir.path()).await.expect("storage");
    let bridge = Arc::new(DeterministicMockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    // Without a `ProfileRegistry`, the engine falls back to the legacy M5
    // mock path, which never merges a profile's own `[sandbox]` section —
    // so `verifier@2.0`'s `mode = "read-only"` never lands on the node, and
    // its `passed` outcome (`ledger_effect = "verified"`) trips the
    // sealed-verifier gate in `engine/stage/agent.rs` unconditionally. The
    // gate checks the *resolved* sandbox mode, not actual filesystem
    // activity, so a wired (bundled-only) registry is required for
    // `flow_bug_fix`/`flow_refactor` to reach `Completed`.
    let profile_registry = Arc::new(ProfileRegistry::new(DiskProfileSet::empty()));
    let engine = Engine::new(
        bridge,
        storage.clone(),
        dispatcher,
        EngineConfig {
            profile_registry: Some(profile_registry),
            ..EngineConfig::default()
        },
    );

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            load_archetype(name),
            dir.path().to_path_buf(),
            // The archetypes' first stage binds `user_prompt`
            // (`ArtifactSource::InitialPrompt`), as every real run has one.
            EngineRunConfig {
                initial_prompt: format!("Exercise the {name} archetype."),
                ..EngineRunConfig::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{name}: start_run failed: {e}"));

    let outcome = tokio::time::timeout(Duration::from_secs(30), handle.await_completion())
        .await
        .unwrap_or_else(|_| panic!("{name}: run hung > 30s"))
        .expect("await_completion");
    match outcome {
        RunOutcome::Completed { .. } => {},
        other => panic!("{name}: expected Completed, got {other:?}"),
    }
    drop(engine);

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let last = reader.current_seq().await.unwrap();
    reader
        .read_events(EventSeq(0)..EventSeq(last.0 + 1))
        .await
        .unwrap()
}

/// Write a trivial always-succeeding executable that ignores its arguments,
/// for use as `SURGE_BIN`.
///
/// `spec-author@1.0` declares real `on_outcome` hooks (`{surge} artifact
/// validate --kind spec spec.toml`/`spec.md`, `on_failure = "reject"`) that
/// `ProcessSpawner` shells out to. With no `SURGE_BIN`, `resolve_surge_bin`
/// falls back to `current_exe()` — this *test* binary — so the hook would
/// invoke the test harness itself with `artifact validate ...` as libtest
/// arguments, which it rejects, driving the outcome into an endless
/// reject/retry loop this deterministic mock bridge (which always reports
/// the same first outcome) can never escape. This suite exercises the
/// engine's archetype/binding plumbing, not the hook's shell-validation path
/// (covered by `crates/surge-orchestrator/src/engine/hooks/mod.rs`'s own
/// tests), so a stand-in that always succeeds is the right fixture here.
fn write_noop_surge_bin(dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = dir.join("surge-noop.bat");
        std::fs::write(&path, "@exit /b 0\r\n").unwrap();
        path
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("surge-noop.sh");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_archetypes_complete_against_deterministic_mock_bridge() {
    let bin_dir = tempfile::tempdir().expect("tempdir");
    let noop_bin = write_noop_surge_bin(bin_dir.path());
    // SAFETY: this is the only test in this binary and it runs before any
    // other code reads the environment, so there is no data race.
    unsafe {
        std::env::set_var("SURGE_BIN", &noop_bin);
    }

    for name in ARCHETYPES {
        let events = run_archetype(name).await;
        assert!(
            events.iter().all(|ev| !matches!(
                ev.payload.payload,
                EventPayload::StageFailed { .. } | EventPayload::RunFailed { .. }
            )),
            "{name}: run contained StageFailed/RunFailed: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev.payload.payload, EventPayload::RunCompleted { .. })),
            "{name}: missing RunCompleted event"
        );
    }
}
