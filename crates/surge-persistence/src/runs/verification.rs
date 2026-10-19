//! Normalized transactional verification context. No journal replay per append.
use super::error::WriterError;
use rusqlite::{OptionalExtension, Transaction};
use surge_core::{
    Graph,
    run_event::EventPayload,
    verification_evidence::{
        VerificationContext, VerificationCriteria, VerificationInvalidation, VerificationSubject,
    },
};

pub(super) fn maintain(
    tx: &Transaction<'_>,
    seq: u64,
    payload: &EventPayload,
) -> Result<Option<surge_core::verification_evidence::VerificationClaim>, WriterError> {
    if !matches!(
        payload,
        EventPayload::VerificationSubjectObserved { .. }
            | EventPayload::VerificationCriteriaAccepted { .. }
            | EventPayload::TaskVerified { .. }
            | EventPayload::TaskStatusChanged { .. }
            | EventPayload::RoadmapUpdated { .. }
            | EventPayload::PipelineMaterialized { .. }
            | EventPayload::GraphRevisionAccepted { .. }
    ) {
        return Ok(None);
    }
    let (subject_json, graph_json): (Option<String>, Option<String>) = tx.query_row(
        "SELECT subject_json,graph_json FROM verification_context WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut context = VerificationContext {
        subject: subject_json
            .as_deref()
            .map(serde_json::from_str::<VerificationSubject>)
            .transpose()?,
        ..Default::default()
    };
    let task = match payload {
        EventPayload::VerificationCriteriaAccepted { task_id, .. }
        | EventPayload::TaskVerified { task_id, .. } => Some(task_id),
        _ => None,
    };
    if let Some(task) = task {
        let json: Option<String> = tx
            .query_row(
                "SELECT criteria_json FROM verification_criteria WHERE task_id=?",
                [task.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = json {
            context.criteria.insert(
                task.clone(),
                serde_json::from_str::<VerificationCriteria>(&json)?,
            );
        }
    }
    match context.observe(payload) {
        VerificationInvalidation::All => {
            tx.execute("UPDATE task_ledger SET verified=0", [])?;
            tx.execute("DELETE FROM verification_criteria", [])?;
        },
        VerificationInvalidation::Task(task) => {
            if !context.criteria.contains_key(&task) {
                tx.execute(
                    "DELETE FROM verification_criteria WHERE task_id=?",
                    [task.as_str()],
                )?;
            }
            tx.execute("INSERT INTO task_ledger (task_id,status,verified,updated_seq) VALUES (?,'pending',0,?) ON CONFLICT(task_id) DO UPDATE SET verified=0", rusqlite::params![task.as_str(),seq as i64])?;
        },
        VerificationInvalidation::None => {},
    }
    match payload {
        EventPayload::VerificationSubjectObserved { subject } => {
            tx.execute(
                "UPDATE verification_context SET subject_json=?,updated_seq=? WHERE singleton=1",
                rusqlite::params![
                    subject.as_ref().map(serde_json::to_string).transpose()?,
                    seq as i64
                ],
            )?;
        },
        EventPayload::VerificationCriteriaAccepted { task_id, criteria } => {
            if let Some(criteria) = criteria {
                tx.execute("INSERT INTO verification_criteria VALUES (?,?,?) ON CONFLICT(task_id) DO UPDATE SET criteria_json=excluded.criteria_json,updated_seq=excluded.updated_seq", rusqlite::params![task_id.as_str(),serde_json::to_string(criteria)?,seq as i64])?;
            } else {
                tx.execute(
                    "DELETE FROM verification_criteria WHERE task_id=?",
                    [task_id.as_str()],
                )?;
            }
        },
        EventPayload::RoadmapUpdated { .. } => {
            tx.execute("DELETE FROM verification_criteria", [])?;
        },
        EventPayload::PipelineMaterialized { graph, .. } if graph_json.is_none() => {
            store_graph(tx, seq, graph)?;
        },
        EventPayload::GraphRevisionAccepted { graph, .. } => {
            store_graph(tx, seq, graph)?;
        },
        EventPayload::TaskVerified {
            task_id,
            node,
            evidence,
            report,
        } => {
            let graph = graph_json
                .as_deref()
                .map(serde_json::from_str::<Graph>)
                .transpose()?;
            let claim = surge_core::verification_evidence::classify_claim(
                graph.as_ref(),
                node,
                task_id,
                *evidence,
                report.as_ref(),
                context.subject.as_ref(),
                context.criteria.get(task_id),
            );
            let valid = claim == surge_core::verification_evidence::VerificationClaim::Verified;
            // Stored bytes, not event metadata alone, must still exist and match.
            let path: Option<String> = tx.query_row("SELECT path FROM artifacts WHERE content_hash=? AND name='verification-report' ORDER BY produced_at_seq DESC LIMIT 1", [evidence.to_string()], |row| row.get(0)).optional()?;
            let stored = path
                .as_ref()
                .and_then(|path| std::fs::read(path).ok())
                .is_some_and(|bytes| surge_core::ContentHash::compute(&bytes) == *evidence);
            if valid
                && stored
                && let (Some(path), Some(binding)) = (
                    path,
                    report.as_ref().and_then(|report| report.binding.as_ref()),
                )
            {
                tx.execute("INSERT INTO verification_proofs VALUES (?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET evidence=excluded.evidence,report_path=excluded.report_path,binding_json=excluded.binding_json", rusqlite::params![task_id.as_str(),evidence.to_string(),path,serde_json::to_string(binding)?])?;
            }
            return Ok(Some(if valid && !stored {
                surge_core::verification_evidence::VerificationClaim::Unbound
            } else {
                claim
            }));
        },
        _ => {},
    }
    Ok(None)
}

fn store_graph(tx: &Transaction<'_>, seq: u64, graph: &Graph) -> Result<(), WriterError> {
    let effective = surge_core::escalation::with_default_escalation_gates(graph);
    tx.execute(
        "UPDATE verification_context SET graph_json=?,updated_seq=? WHERE singleton=1",
        rusqlite::params![serde_json::to_string(&effective)?, seq as i64],
    )?;
    Ok(())
}

/// Historical currentness and its sealed locator, read without migrations or repair.
#[derive(Debug, Clone)]
pub struct VerificationProofRecord {
    /// Stable task identity.
    pub task_id: String,
    /// Fresh at the last durable observation.
    pub verified: bool,
    /// Host-sealed revision and criteria contract.
    pub binding: surge_core::verification_evidence::VerificationBinding,
    /// Immutable stored report locator.
    pub report_path: std::path::PathBuf,
    /// Hash of the sealed report bytes.
    pub evidence: surge_core::ContentHash,
}
impl super::Storage {
    /// Read proof locators without creating storage or applying migrations.
    ///
    /// # Errors
    /// Returns storage errors for absent, unsupported or corrupt journals.
    pub fn inspect_verification_proofs(
        &self,
        run: surge_core::RunId,
    ) -> Result<Vec<VerificationProofRecord>, super::StorageError> {
        let mut conn =
            super::connection::RetainedConnection::read_only(&self.events_db_path(&run))?;
        let tx = conn.transaction()?;
        super::projection::require_current(&tx)?;
        let mut statement = tx.prepare("SELECT p.task_id,l.verified,p.binding_json,p.report_path,p.evidence FROM verification_proofs p JOIN task_ledger l USING(task_id)")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut proofs = Vec::new();
        for row in rows {
            let (task_id, verified, json, path, hash) = row?;
            let evidence = hash.parse().map_err(|_| {
                super::StorageError::SerializationFailed(serde_json::Error::io(
                    std::io::Error::other("invalid proof content hash"),
                ))
            })?;
            proofs.push(VerificationProofRecord {
                task_id,
                verified,
                binding: serde_json::from_str(&json)?,
                report_path: path.into(),
                evidence,
            });
        }
        Ok(proofs)
    }
}

#[cfg(test)]
mod graph_authority_tests {
    use super::*;
    use crate::runs::Storage;
    use surge_core::node::{LedgerEffect, Node, NodeConfig, OutcomeDecl};
    use surge_core::roadmap::{
        VerificationCheck, VerificationCheckResult, VerificationReportArtifact,
        VerificationReportOutcome,
    };
    use surge_core::verification_evidence::{
        VerificationBinding, VerificationCriteria, VerificationSubject,
    };
    use surge_core::{ContentHash, RunId, VersionedEventPayload};

    fn graph(authorizes: bool) -> Graph {
        let mut graph = surge_core::BundledFlows::by_name_latest("linear-3")
            .unwrap()
            .graph;
        graph.nodes.clear();
        graph.edges.clear();
        graph.subgraphs.clear();
        graph.start = "parent_end".parse().unwrap();
        for name in ["parent_end", "verify_1"] {
            let id: surge_core::NodeKey = name.parse().unwrap();
            graph.nodes.insert(
                id.clone(),
                Node {
                    id,
                    position: surge_core::node::Position::default(),
                    declared_outcomes: if authorizes && name == "verify_1" {
                        vec![OutcomeDecl {
                            id: "passed".parse().unwrap(),
                            description: "fixed authority".into(),
                            edge_kind_hint: surge_core::edge::EdgeKind::Forward,
                            is_terminal: false,
                            ledger_effect: LedgerEffect::Verified,
                        }]
                    } else {
                        vec![]
                    },
                    config: NodeConfig::Terminal(surge_core::terminal_config::TerminalConfig {
                        kind: surge_core::terminal_config::TerminalKind::Success,
                        message: None,
                    }),
                },
            );
        }
        graph
    }

    fn evidence(root: &std::path::Path) -> (VerificationReportArtifact, Vec<u8>, ContentHash) {
        let report = VerificationReportArtifact {
            schema_version: 1,
            task_id: "fixed-task".into(),
            outcome: VerificationReportOutcome::Passed,
            summary: "fixed passing report".into(),
            checks: vec![VerificationCheck {
                command: "fixed acceptance".into(),
                result: VerificationCheckResult::Passed,
                covers: vec!["criterion:1".into()],
                note: None,
            }],
            evidence: vec![],
            binding: Some(VerificationBinding {
                subject: VerificationSubject {
                    repository: root.join(".git"),
                    worktree: root.to_owned(),
                    tree: "a".repeat(40),
                    checkpoint: "b".repeat(40),
                },
                criteria: VerificationCriteria {
                    hash: ContentHash::compute(b"fixed accepted criterion"),
                    epoch: "00000000000000000000000001".parse().unwrap(),
                    required: vec!["criterion:1".into()],
                    definitions: vec![("criterion:1".into(), "fixed accepted criterion".into())],
                },
            }),
        };
        let bytes = toml::to_string(&report).unwrap().into_bytes();
        let hash = ContentHash::compute(&bytes);
        (report, bytes, hash)
    }

    async fn append_fixture(
        writer: &crate::runs::RunWriter,
        root: &std::path::Path,
        parent: bool,
        child: bool,
        revision: Option<bool>,
    ) -> (VerificationReportArtifact, ContentHash, std::path::PathBuf) {
        let (report, bytes, hash) = evidence(root);
        let path = root.join("sealed-report.toml");
        std::fs::write(&path, bytes).unwrap();
        let binding = report.binding.as_ref().unwrap();
        let mut payloads = vec![EventPayload::PipelineMaterialized {
            graph: Box::new(graph(parent)),
            graph_hash: ContentHash::compute(b"parent"),
        }];
        if let Some(authorizes) = revision {
            payloads.push(EventPayload::GraphRevisionAccepted {
                patch_id: surge_core::RoadmapPatchId::new("rpatch-revoked").unwrap(),
                target: surge_core::roadmap_patch::RoadmapPatchTarget::ProjectRoadmap {
                    roadmap_path: "roadmap.toml".into(),
                },
                previous_graph_hash: ContentHash::compute(b"parent"),
                graph: Box::new(graph(authorizes)),
                graph_hash: ContentHash::compute(b"accepted-revision"),
                active_pickup: surge_core::roadmap_patch::ActivePickupPolicy::Allowed,
            });
        }
        payloads.extend([
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph(child)),
                graph_hash: ContentHash::compute(b"child"),
            },
            EventPayload::VerificationSubjectObserved {
                subject: Some(binding.subject.clone()),
            },
            EventPayload::VerificationCriteriaAccepted {
                task_id: "fixed-task".into(),
                criteria: Some(binding.criteria.clone()),
            },
            EventPayload::ArtifactProduced {
                node: "verify_1".parse().unwrap(),
                artifact: hash,
                path: path.clone(),
                name: "verification-report".into(),
                source_path: None,
            },
            EventPayload::TaskVerified {
                task_id: "fixed-task".into(),
                node: "verify_1".parse().unwrap(),
                evidence: hash,
                report: Some(report.clone()),
            },
            EventPayload::RequirementRevised {
                node: "verify_1".parse().unwrap(),
                task: Some("human-task".into()),
                text: "keep history".into(),
            },
            EventPayload::TaskAcceptedByHuman {
                node: "verify_1".parse().unwrap(),
                task: Some("human-task".into()),
                findings: None,
                comment: Some("keep answer".into()),
            },
        ]);
        for payload in payloads {
            writer
                .append_event(VersionedEventPayload::new(payload))
                .await
                .unwrap();
        }
        (report, hash, path)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_append_and_rebuild_ignore_future_child() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            for (parent, child, revision) in [
                (false, true, None),
                (true, false, None),
                (true, true, Some(false)),
            ] {
                let run = RunId::new();
                let writer = storage.create_run(run, home.path(), None).await.unwrap();
                append_fixture(&writer, home.path(), parent, child, revision).await;
                let authoritative = revision.unwrap_or(parent);
                let before = writer.reader().task_ledger().await.unwrap();
                writer.rebuild_views().await.unwrap();
                let after = writer.reader().task_ledger().await.unwrap();
                let proofs = storage.inspect_verification_proofs(run).unwrap();
                writer.close().await.unwrap();
                for ledger in [before, after] {
                    assert_eq!(
                        ledger
                            .iter()
                            .find(|row| row.task_id.as_str() == "fixed-task")
                            .unwrap()
                            .verified,
                        authoritative
                    );
                    let human = ledger
                        .iter()
                        .find(|row| row.task_id.as_str() == "human-task")
                        .unwrap();
                    assert!(human.accepted_by_human && human.requirement_revised);
                }
                assert_eq!(proofs.len(), usize::from(authoritative));
            }
        }
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_legacy_readonly_refuses_then_owned_replay_repairs() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            for (parent, child, revision) in [
                (false, true, None),
                (true, false, None),
                (true, true, Some(false)),
            ] {
                let authoritative = revision.unwrap_or(parent);
                let run = RunId::new();
                let writer = storage.create_run(run, home.path(), None).await.unwrap();
                let (report, hash, path) =
                    append_fixture(&writer, home.path(), parent, child, revision).await;
                writer.close().await.unwrap();
                let conn = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
                // Literal pre-fix projection, independent of the currently linked maintainer.
                conn.execute(
                    "UPDATE verification_context SET graph_json=?",
                    [serde_json::to_string(&graph(child)).unwrap()],
                )
                .unwrap();
                conn.execute(
                    "UPDATE task_ledger SET verified=? WHERE task_id='fixed-task'",
                    [child],
                )
                .unwrap();
                conn.execute("DELETE FROM verification_proofs", []).unwrap();
                if child {
                    conn.execute(
                        "INSERT INTO verification_proofs VALUES('fixed-task',?,?,?)",
                        rusqlite::params![
                            hash.to_string(),
                            path.to_string_lossy(),
                            serde_json::to_string(report.binding.as_ref().unwrap()).unwrap()
                        ],
                    )
                    .unwrap();
                }
                conn.execute_batch("DROP TABLE IF EXISTS projection_semantics; DELETE FROM _migrations WHERE id='per-run-0011-graph-authority'; INSERT INTO graph_snapshots(at_seq,snapshot,bytes_compressed) VALUES(8,X'1234',2);").unwrap();
                let raw_before: Vec<Vec<u8>> = conn
                    .prepare("SELECT payload FROM events ORDER BY seq")
                    .unwrap()
                    .query_map([], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                let proof_before = storage.inspect_verification_proofs(run);
                let reader = storage.open_run_reader(run).await.unwrap();
                let ledger_before = reader.task_ledger().await;
                drop(reader);
                assert!(proof_before.is_err(), "legacy proofs require owned replay");
                assert!(
                    ledger_before.is_err(),
                    "legacy ledger must not republish stale positives"
                );
                assert!(
                    storage
                        .sync_task_ledger_index(run, home.path())
                        .await
                        .is_err()
                );
                drop(conn);
                for _ in 0..2 {
                    let writer = storage.open_run_writer(run).await.unwrap();
                    let ledger = writer.reader().task_ledger().await.unwrap();
                    let proofs = storage.inspect_verification_proofs(run).unwrap();
                    writer.close().await.unwrap();
                    assert_eq!(
                        ledger
                            .iter()
                            .find(|row| row.task_id.as_str() == "fixed-task")
                            .unwrap()
                            .verified,
                        authoritative
                    );
                    assert_eq!(proofs.len(), usize::from(authoritative));
                }
                let conn = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
                let raw_after: Vec<Vec<u8>> = conn
                    .prepare("SELECT payload FROM events ORDER BY seq")
                    .unwrap()
                    .query_map([], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                assert_eq!(raw_before, raw_after);
                let snapshot: Vec<u8> = conn
                    .query_row(
                        "SELECT snapshot FROM graph_snapshots WHERE at_seq=8",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(snapshot, vec![0x12, 0x34]);
                storage
                    .sync_task_ledger_index(run, home.path())
                    .await
                    .unwrap();
                assert_eq!(
                    storage
                        .task_ledger_store()
                        .get(run, "fixed-task")
                        .unwrap()
                        .unwrap()
                        .verified,
                    authoritative
                );
            }
        }
        home.close().unwrap();
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_failed_replay_keeps_untrusted_marker_and_prior_views() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            let run = RunId::new();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            append_fixture(&writer, home.path(), true, true, None).await;
            writer.close().await.unwrap();
            let conn = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
            conn.execute_batch("CREATE TABLE IF NOT EXISTS projection_semantics(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version>=0)); INSERT OR REPLACE INTO projection_semantics VALUES(1,0);").unwrap();
            conn.execute("INSERT INTO events(timestamp,kind,payload,schema_version) VALUES(9,'RunAborted',X'00',1)",[]).unwrap();
            let result = storage.open_run_writer(run).await;
            let refused = result.is_err();
            if let Ok(writer) = result {
                writer.close().await.unwrap();
            }
            assert!(
                refused,
                "corrupt historical replay must refuse writer publication"
            );
            let version: i64 = conn
                .query_row("SELECT version FROM projection_semantics", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(version, 0);
            let verified: bool = conn
                .query_row(
                    "SELECT verified FROM task_ledger WHERE task_id='fixed-task'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                verified,
                "failed transaction must roll back the reset and replay"
            );
            assert!(storage.inspect_verification_proofs(run).is_err());
        }
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_future_marker_refuses_readers_and_writer() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            let run = RunId::new();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            append_fixture(&writer, home.path(), true, true, None).await;
            writer.close().await.unwrap();
            let conn = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
            conn.execute_batch("CREATE TABLE IF NOT EXISTS projection_semantics(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version>=0)); INSERT OR REPLACE INTO projection_semantics VALUES(1,99);").unwrap();
            let proof = storage.inspect_verification_proofs(run);
            let reader = storage.open_run_reader(run).await.unwrap();
            let ledger = reader.task_ledger().await;
            drop(reader);
            let result = storage.open_run_writer(run).await;
            let refused = result.is_err();
            if let Ok(writer) = result {
                writer.close().await.unwrap();
            }
            assert!(
                proof.is_err() && ledger.is_err() && refused,
                "future projection semantics must fail closed"
            );
            assert_eq!(
                conn.query_row("SELECT version FROM projection_semantics", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                99
            );
        }
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_replay_does_not_restore_proof_without_sealed_bytes() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            let run = RunId::new();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            let (_, _, path) = append_fixture(&writer, home.path(), true, true, None).await;
            writer.close().await.unwrap();
            std::fs::remove_file(path).unwrap();
            let conn = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
            conn.execute_batch("CREATE TABLE IF NOT EXISTS projection_semantics(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version>=0)); INSERT OR REPLACE INTO projection_semantics VALUES(1,0);").unwrap();
            drop(conn);
            let writer = storage.open_run_writer(run).await.unwrap();
            let ledger = writer.reader().task_ledger().await.unwrap();
            let proofs = storage.inspect_verification_proofs(run).unwrap();
            writer.close().await.unwrap();
            assert!(
                !ledger
                    .iter()
                    .find(|row| row.task_id.as_str() == "fixed-task")
                    .unwrap()
                    .verified
            );
            assert!(proofs.is_empty());
        }
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_normalizes_sqlite_parent_escalation_nodes() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            let run = RunId::new();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            let parent = surge_core::BundledFlows::by_name_latest("linear-3")
                .unwrap()
                .graph;
            writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::PipelineMaterialized {
                        graph: Box::new(parent),
                        graph_hash: ContentHash::compute(b"literal-parent"),
                    },
                ))
                .await
                .unwrap();
            let conn = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
            let json: String = conn
                .query_row("SELECT graph_json FROM verification_context", [], |row| {
                    row.get(0)
                })
                .unwrap();
            writer.close().await.unwrap();
            let normalized: Graph = serde_json::from_str(&json).unwrap();
            assert!(
                normalized
                    .nodes
                    .contains_key(&"verify_1_escalation".parse().unwrap())
            );
            assert!(matches!(
                normalized.nodes[&"verify_1_escalation".parse().unwrap()].config,
                NodeConfig::HumanGate(_)
            ));
        }
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_sql_replay_failure_rolls_back_and_retry_succeeds() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            let run = RunId::new();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            append_fixture(&writer, home.path(), true, true, None).await;
            writer.close().await.unwrap();
            let conn = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
            conn.execute_batch("CREATE TABLE IF NOT EXISTS projection_semantics(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version>=0)); INSERT OR REPLACE INTO projection_semantics VALUES(1,0); CREATE TRIGGER fail_replay BEFORE INSERT ON artifacts BEGIN SELECT RAISE(ABORT,'injected replay failure'); END;").unwrap();
            let result = storage.open_run_writer(run).await;
            let error = result.as_ref().err().map(ToString::to_string);
            if let Ok(writer) = result {
                writer.close().await.unwrap();
            }
            assert!(error.is_some_and(|error| error.contains("injected replay failure")));
            assert_eq!(
                conn.query_row("SELECT version FROM projection_semantics", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert!(
                conn.query_row(
                    "SELECT verified FROM task_ledger WHERE task_id='fixed-task'",
                    [],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
            );
            conn.execute_batch("DROP TRIGGER fail_replay").unwrap();
            let writer = storage.open_run_writer(run).await.unwrap();
            let ledger = writer.reader().task_ledger().await.unwrap();
            writer.close().await.unwrap();
            assert!(
                ledger
                    .iter()
                    .find(|row| row.task_id.as_str() == "fixed-task")
                    .unwrap()
                    .verified
            );
            assert_eq!(
                conn.query_row("SELECT version FROM projection_semantics", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
        home.close().unwrap();
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_gap_cannot_certify_missing_accepted_revocation() {
        type RawEvent = (i64, i64, String, Vec<u8>, i64);
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            let donor = RunId::new();
            let writer = storage.create_run(donor, home.path(), None).await.unwrap();
            let (report, hash, path) =
                append_fixture(&writer, home.path(), true, true, Some(false)).await;
            writer.close().await.unwrap();
            let source = rusqlite::Connection::open(storage.events_db_path(&donor)).unwrap();
            let rows: Vec<RawEvent> = source
                .prepare(
                    "SELECT seq,timestamp,kind,payload,schema_version FROM events ORDER BY seq",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(rows[1].2, "GraphRevisionAccepted");
            drop(source);

            let run = RunId::new();
            storage
                .create_run(run, home.path(), None)
                .await
                .unwrap()
                .close()
                .await
                .unwrap();
            let conn = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
            // Seed a historical gap from the outset; neither append-only trigger
            // is bypassed and no existing journal row is changed or deleted.
            for (seq, timestamp, kind, payload, schema) in rows.iter().filter(|row| row.0 != 2) {
                conn.execute("INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(?,?,?,?,?)",rusqlite::params![seq,timestamp,kind,payload,schema]).unwrap();
            }
            let raw_before: Vec<RawEvent> = rows.into_iter().filter(|row| row.0 != 2).collect();
            assert_eq!((raw_before[0].0, raw_before[1].0), (1, 3));
            let binding = report.binding.as_ref().unwrap();
            conn.execute(
                "UPDATE projection_semantics SET version=0 WHERE singleton=1",
                [],
            )
            .unwrap();
            conn.execute("UPDATE verification_context SET subject_json=?,graph_json=?,updated_seq=4 WHERE singleton=1",rusqlite::params![serde_json::to_string(&binding.subject).unwrap(),serde_json::to_string(&graph(true)).unwrap()]).unwrap();
            conn.execute(
                "INSERT INTO verification_criteria VALUES('fixed-task',?,5)",
                [serde_json::to_string(&binding.criteria).unwrap()],
            )
            .unwrap();
            conn.execute("INSERT INTO task_ledger(task_id,status,verified,last_authority_node,updated_seq) VALUES('fixed-task','completed',1,'verify_1',7)",[]).unwrap();
            conn.execute("INSERT INTO task_ledger(task_id,status,verified,updated_seq,accepted_by_human,requirement_revised) VALUES('sentinel','completed',0,1,1,1)",[]).unwrap();
            conn.execute(
                "INSERT INTO verification_proofs VALUES('fixed-task',?,?,?)",
                rusqlite::params![
                    hash.to_string(),
                    path.to_string_lossy(),
                    serde_json::to_string(binding).unwrap()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO graph_snapshots(at_seq,snapshot,bytes_compressed) VALUES(9,X'1234',2)",
                [],
            )
            .unwrap();
            let normalized_rows = || {
                [
                    "SELECT singleton,subject_json,graph_json,updated_seq FROM verification_context ORDER BY singleton",
                    "SELECT task_id,criteria_json,updated_seq FROM verification_criteria ORDER BY task_id",
                    "SELECT task_id,evidence,report_path,binding_json FROM verification_proofs ORDER BY task_id",
                    "SELECT at_seq,snapshot,bytes_compressed FROM graph_snapshots ORDER BY at_seq",
                ].map(|sql| {
                    let mut statement = conn.prepare(sql).unwrap();
                    let columns = statement.column_count();
                    statement.query_map([], |row| {
                        (0..columns).map(|column| row.get::<_, rusqlite::types::Value>(column)).collect::<rusqlite::Result<Vec<_>>>()
                    }).unwrap().collect::<Result<Vec<_>, _>>().unwrap()
                })
            };
            let normalized_before = normalized_rows();
            assert!(storage.inspect_verification_proofs(run).is_err());
            assert!(
                storage
                    .sync_task_ledger_index(run, home.path())
                    .await
                    .is_err()
            );
            let result = storage.open_run_writer(run).await;
            let error = result.as_ref().err().map(ToString::to_string);
            if let Ok(writer) = result {
                writer.close().await.unwrap();
            }
            let incorrectly_exposed = storage
                .inspect_verification_proofs(run)
                .is_ok_and(|proofs| proofs.iter().any(|proof| proof.verified));
            assert!(
                error
                    .as_ref()
                    .is_some_and(|error| error.contains("expected seq=2, got seq=3")),
                "missing revocation cannot be certified: error={error:?}, exposed_verified={incorrectly_exposed}"
            );
            assert_eq!(
                conn.query_row("SELECT version FROM projection_semantics", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            let ledger: (bool,bool,bool) = conn.query_row("SELECT verified,accepted_by_human,requirement_revised FROM task_ledger WHERE task_id='sentinel'",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
            assert_eq!(ledger, (false, true, true));
            assert!(
                conn.query_row(
                    "SELECT verified FROM task_ledger WHERE task_id='fixed-task'",
                    [],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
            );
            assert!(storage.inspect_verification_proofs(run).is_err());
            let raw_after: Vec<RawEvent> = conn
                .prepare(
                    "SELECT seq,timestamp,kind,payload,schema_version FROM events ORDER BY seq",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(raw_after, raw_before);
            assert_eq!(normalized_rows(), normalized_before);
            assert_eq!(
                conn.query_row(
                    "SELECT snapshot FROM graph_snapshots WHERE at_seq=9",
                    [],
                    |row| row.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
                vec![0x12, 0x34]
            );
            // Refusal releases actual writer ownership; reopening cannot turn
            // the same incomplete prefix into a trusted projection either.
            assert!(storage.open_run_writer(run).await.is_err());
        }
        home.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn graph_authority_failed_batch_reuses_rolled_back_sequences() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            let run = RunId::new();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            let first = writer
                .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
                    reason: "first retained event".into(),
                }))
                .await
                .unwrap();
            assert_eq!(first.as_u64(), 1);
            let failed = writer
                .append_events(vec![
                    VersionedEventPayload::new(EventPayload::TaskDiscovered {
                        task_id: "rolled-back".into(),
                        discovered_from: "seed".into(),
                        title: "must roll back with journal".into(),
                    }),
                    VersionedEventPayload::new(EventPayload::StageRouteCommitted {
                        invocation: surge_core::id::StageInvocationId::new(),
                        outcome_commit_seq: 999,
                    }),
                ])
                .await;
            assert!(failed.is_err());
            let next = writer
                .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
                    reason: "second retained event".into(),
                }))
                .await
                .unwrap();
            assert_eq!(
                next.as_u64(),
                2,
                "rolled-back rows never consume durable sequence positions"
            );
            writer.rebuild_views().await.unwrap();
            let reader = writer.reader();
            let rows = reader
                .read_events(crate::runs::EventSeq(1)..crate::runs::EventSeq(3))
                .await
                .unwrap();
            let ledger = reader.task_ledger().await.unwrap();
            drop(reader);
            writer.close().await.unwrap();
            assert_eq!(
                rows.iter().map(|row| row.seq.as_u64()).collect::<Vec<_>>(),
                [1, 2]
            );
            assert!(
                ledger.is_empty(),
                "failed batch rolled back its view writes too"
            );
        }
        home.close().unwrap();
    }
}
