//! Actual subprocess oracle: private transport remains exact, diagnostics are opaque.
#![cfg(unix)]

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_core::mcp_config::McpTransportConfig;
use surge_mcp::{McpContent, McpRegistry, McpServerConnection, stderr_log_path};

#[path = "common/protected_fixture.rs"]
mod protected_fixture;
use protected_fixture::Fixture;

/// Fixture startup readiness only; this supplies no production containment evidence.
struct CatalogReadyObserver(PathBuf);

#[async_trait::async_trait]
impl surge_mcp::writer_observer::HostWriterObserver for CatalogReadyObserver {
    async fn before_child(
        &self,
        _server: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, surge_mcp::writer_observer::WriterObservationError>
    {
        Ok(surge_core::id::ExecutionWriterId::new())
    }

    async fn child_started(
        &self,
        _writer: surge_core::id::ExecutionWriterId,
        pid: Option<u32>,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        use surge_mcp::writer_observer::WriterObservationError;
        let pid = pid.ok_or_else(|| WriterObservationError("fixture child PID missing".into()))?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match std::fs::read_to_string(&self.0) {
                    Ok(value) if value.trim().parse::<u32>() == Ok(pid) => return Ok(()),
                    Ok(_) => {
                        return Err(WriterObservationError(
                            "fixture readiness PID mismatch".into(),
                        ));
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(_) => {
                        return Err(WriterObservationError(
                            "fixture readiness read failed".into(),
                        ));
                    },
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .map_err(|_| WriterObservationError("fixture startup readiness timed out".into()))?
    }
}

fn delayed_catalog_fixture() -> Fixture {
    let fixture = Fixture::new("deadline-oracle");
    let script = fixture.dir.path().join("child.py");
    let original = std::fs::read_to_string(&script).unwrap();
    let ready = original.replace(
        "for line in sys.stdin:",
        "with open(os.environ['RECORDER'] + '.ready-writing', 'w') as f:\n    f.write(str(os.getpid()))\nos.replace(os.environ['RECORDER'] + '.ready-writing', os.environ['RECORDER'] + '.ready')\nfor line in sys.stdin:",
    );
    // Deterministic cold-start counterfactual exceeds the 120ms RPC deadline.
    std::fs::write(&script, format!("import time; time.sleep(0.25)\n{ready}")).unwrap();
    fixture
}

async fn assert_cold_start_is_not_catalog_evidence() {
    let mut fixture = delayed_catalog_fixture();
    fixture.config.call_timeout = Duration::from_millis(120);
    let connection =
        McpServerConnection::new(fixture.config.clone(), Some(fixture.dir.path().to_owned()));
    assert!(matches!(connection.list_tools().await,
        Err(surge_mcp::McpError::Timeout(timeout)) if timeout == Duration::from_millis(120)));
    assert!(
        !fixture
            .dir
            .path()
            .join("private-recorder.json.requests")
            .exists(),
        "cold initialization timeout must not stand in for the two-page catalog oracle"
    );
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn actual_catalog_deadline_spans_pages_and_call_timeout_remains_exact() {
    assert_cold_start_is_not_catalog_evidence().await;
    // Each page takes 250 ms: separate per-page deadlines would fit 400 ms, the
    // shared one cannot, and page two still arrives ~150 ms before it expires.
    for (budget, succeeds) in [(400, false), (1000, true)] {
        let mut fixture = delayed_catalog_fixture();
        let script = fixture.dir.path().join("child.py");
        let original = std::fs::read_to_string(&script).unwrap();
        std::fs::write(&script, original.replace(
            "    if method == 'initialize':",
            "    if method == 'tools/list':\n        import time; time.sleep(0.25)\n    elif method == 'tools/call':\n        import time; time.sleep(0.6)\n    if method == 'initialize':",
        )).unwrap();
        let McpTransportConfig::Stdio { env, .. } = &mut fixture.config.transport else {
            panic!("stdio fixture");
        };
        env.insert("PAGINATE".into(), "1".into());
        fixture.config.call_timeout = Duration::from_millis(budget);
        let connection = McpServerConnection::new_owned(
            fixture.config.clone(),
            Some(fixture.dir.path().to_owned()),
            Arc::new(CatalogReadyObserver(
                fixture.dir.path().join("private-recorder.json.ready"),
            )),
        );
        let catalog = connection.list_tools().await;
        if succeeds {
            let names = catalog
                .unwrap()
                .into_iter()
                .map(|tool| tool.name.into_owned())
                .collect::<Vec<_>>();
            assert_eq!(names, ["echo", "echo-second"]);
        } else {
            assert!(
                matches!(catalog, Err(surge_mcp::McpError::Timeout(timeout)) if timeout == Duration::from_millis(budget)),
                "catalog pages received separate deadlines"
            );
        }
        let requests =
            std::fs::read_to_string(fixture.dir.path().join("private-recorder.json.requests"))
                .unwrap();
        assert_eq!(
            requests
                .lines()
                .filter(|method| *method == "tools/list")
                .count(),
            2,
            "both actual pages must be reached"
        );
        let call = connection.call_tool("success", serde_json::json!({})).await;
        if succeeds {
            assert_eq!(
                serde_json::to_value(call.unwrap()).unwrap()["structuredContent"]["value"],
                7
            );
        } else {
            assert!(
                matches!(call, Err(surge_mcp::McpError::Timeout(timeout)) if timeout == Duration::from_millis(budget)),
                "actual tool call ignored accepted timeout"
            );
        }
        connection.shutdown().await.unwrap();
        fixture.verify_transport();
    }
}

#[tokio::test]
async fn actual_child_diagnostics_are_opaque_and_transport_is_exact() {
    let fixture = Fixture::new("direct-oracle");
    let conn =
        McpServerConnection::new(fixture.config.clone(), Some(fixture.dir.path().to_owned()));
    assert_eq!(
        conn.list_tools().await.expect("real child handshake")[0].name,
        "echo"
    );
    fixture.verify_transport();
    let success = conn
        .call_tool("success", serde_json::json!({}))
        .await
        .expect("success");
    assert_eq!(
        serde_json::to_value(&success).expect("success JSON"),
        serde_json::json!({"isError": false, "content": [{"type": "text", "text": "functional exact Ω\nline"}], "structuredContent": {"value": 7}, "_meta": {"public": "exact"}})
    );
    let error_result = conn
        .call_tool("error", serde_json::json!({}))
        .await
        .expect("error envelope");
    let rpc_error = conn
        .call_tool("rpc_error", serde_json::json!({}))
        .await
        .expect_err("RPC error");
    assert!(
        rpc_error.source().is_none(),
        "raw external source chain retained"
    );
    let observations = format!("{error_result:?}\n{rpc_error}\n{rpc_error:?}");
    conn.shutdown().await.expect("child cleanup");
    // The child emits all records before accepting initialize. Give the independent tee
    // task a bounded opportunity to publish them without relying on fixed timing.
    let path = stderr_log_path(Some(fixture.dir.path()), "direct-oracle");
    let tee = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&path).await
                && text.lines().count() >= 7
            {
                break text;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("public tee publication");
    fixture.assert_opaque(&tee);
    fixture.assert_opaque(&observations);
    assert_eq!(error_result.is_error, Some(true));
    assert!(error_result.structured_content.is_none());
    assert!(error_result.meta.is_none());
}

#[tokio::test]
async fn registry_error_payload_is_opaque_and_success_remains_exact() {
    let fixture = Fixture::new("registry-oracle");
    let registry = McpRegistry::from_config(
        std::slice::from_ref(&fixture.config),
        Some(fixture.dir.path()),
    );
    let success = registry
        .call_tool(
            "registry-oracle",
            "success",
            serde_json::json!({}),
            Duration::from_secs(10),
        )
        .await
        .expect("success");
    assert!(
        matches!(&success.content[..], [McpContent::Text(text)] if text == "functional exact Ω\nline")
    );
    let result = registry
        .call_tool(
            "registry-oracle",
            "error",
            serde_json::json!({}),
            Duration::from_secs(10),
        )
        .await
        .expect("error envelope");
    fixture.verify_transport();
    registry.shutdown().await.expect("registry cleanup");
    assert!(result.is_error);
    fixture.assert_opaque(&format!("{result:?}"));
}

struct RefuseAfterObservation {
    after_started: bool,
    refused: std::sync::atomic::AtomicBool,
    pid: std::sync::Mutex<Option<u32>>,
}

#[async_trait::async_trait]
impl surge_mcp::writer_observer::HostWriterObserver for RefuseAfterObservation {
    fn before_effect(
        &self,
        _server: &str,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        if self.refused.load(std::sync::atomic::Ordering::SeqCst) {
            Err(surge_mcp::writer_observer::WriterObservationError(
                "private callback diagnostic".into(),
            ))
        } else {
            Ok(())
        }
    }

    async fn before_child(
        &self,
        _server: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, surge_mcp::writer_observer::WriterObservationError>
    {
        tokio::task::yield_now().await;
        if !self.after_started {
            self.refused
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(surge_core::id::ExecutionWriterId::new())
    }

    async fn child_started(
        &self,
        _writer: surge_core::id::ExecutionWriterId,
        pid: Option<u32>,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        *self.pid.lock().expect("pid capture") = pid;
        tokio::task::yield_now().await;
        self.refused
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn final_spawn_gate_follows_awaited_writer_intent() {
    let fixture = Fixture::new("spawn-gate");
    let observer = std::sync::Arc::new(RefuseAfterObservation {
        after_started: false,
        refused: false.into(),
        pid: std::sync::Mutex::new(None),
    });
    let conn = McpServerConnection::new_owned(
        fixture.config.clone(),
        Some(fixture.dir.path().to_owned()),
        observer.clone(),
    );
    let result = conn.list_tools().await;
    assert!(
        matches!(&result, Err(surge_mcp::McpError::EffectRefused { .. })),
        "physical spawn refusal lost its typed effect classification"
    );
    assert_eq!(
        conn.status().await,
        surge_mcp::McpHealth::Disconnected,
        "spawn refusal changed server liveness"
    );
    for _ in 0..8 {
        assert!(
            matches!(
                conn.list_tools().await,
                Err(surge_mcp::McpError::EffectRefused { .. })
            ),
            "refusal entered backoff or exhausted restart budget"
        );
        assert_eq!(
            conn.status().await,
            surge_mcp::McpHealth::Disconnected,
            "repeated refusal changed liveness"
        );
    }
    let _ = conn.shutdown().await;
    assert!(result.is_err(), "post-intent refusal must prevent spawn");
    assert!(
        observer.pid.lock().expect("pid").is_none(),
        "child started after final refusal"
    );
    assert!(
        !fixture.dir.path().join("private-recorder.json").exists(),
        "actual child executed after final refusal"
    );
    let error = result.expect_err("refusal");
    assert!(!format!("{error:?} {error}").contains("private callback diagnostic"));
}

#[tokio::test]
async fn final_initialize_gate_follows_awaited_child_observation_and_reaps() {
    let fixture = Fixture::new("initialize-gate");
    let observer = std::sync::Arc::new(RefuseAfterObservation {
        after_started: true,
        refused: false.into(),
        pid: std::sync::Mutex::new(None),
    });
    let conn = McpServerConnection::new_owned(
        fixture.config.clone(),
        Some(fixture.dir.path().to_owned()),
        observer.clone(),
    );
    let result = conn.list_tools().await;
    let _ = conn.shutdown().await;
    assert!(
        result.is_err(),
        "post-child refusal must prevent initialize"
    );
    assert!(
        !fixture
            .dir
            .path()
            .join("private-recorder.json.requests")
            .exists(),
        "initialize was sent after final refusal"
    );
    let pid = observer
        .pid
        .lock()
        .expect("pid")
        .expect("actual spawned child");
    let output = tokio::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .await
        .expect("process observation");
    assert!(
        output.stdout.is_empty(),
        "refused actual child was not reaped"
    );
}

#[tokio::test]
async fn tool_call_gate_checks_after_lazy_connection() {
    let fixture = Fixture::new("call-gate");
    let observer = std::sync::Arc::new(CallRefusal {
        refused: false.into(),
    });
    let conn = McpServerConnection::new_owned(
        fixture.config.clone(),
        Some(fixture.dir.path().to_owned()),
        observer.clone(),
    );
    conn.list_tools().await.expect("initial admitted catalog");
    observer
        .refused
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let result = conn.call_tool("success", serde_json::json!({})).await;
    let _ = conn.shutdown().await;
    assert!(
        result.is_err(),
        "tool call must recheck final admission after connection"
    );
    let requests =
        std::fs::read_to_string(fixture.dir.path().join("private-recorder.json.requests"))
            .expect("protocol observations");
    assert!(
        !requests.contains("tools/call"),
        "refused tool request reached actual child"
    );
}

struct CallRefusal {
    refused: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl surge_mcp::writer_observer::HostWriterObserver for CallRefusal {
    fn before_effect(
        &self,
        _server: &str,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        if self.refused.load(std::sync::atomic::Ordering::SeqCst) {
            Err(surge_mcp::writer_observer::WriterObservationError(
                "private callback diagnostic".into(),
            ))
        } else {
            Ok(())
        }
    }
    async fn before_child(
        &self,
        _server: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, surge_mcp::writer_observer::WriterObservationError>
    {
        tokio::task::yield_now().await;
        Ok(surge_core::id::ExecutionWriterId::new())
    }
    async fn child_started(
        &self,
        _writer: surge_core::id::ExecutionWriterId,
        _pid: Option<u32>,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        tokio::task::yield_now().await;
        Ok(())
    }
}

#[tokio::test]
async fn actual_invalid_utf8_overlong_and_flood_stderr_remains_bounded() {
    let mut fixture = Fixture::new("stress-oracle");
    if let McpTransportConfig::Stdio { env, .. } = &mut fixture.config.transport {
        env.insert("STDERR_STRESS".into(), "1".into());
    }
    let conn =
        McpServerConnection::new(fixture.config.clone(), Some(fixture.dir.path().to_owned()));
    conn.list_tools()
        .await
        .expect("invalid UTF8 and huge stderr must not break handshake");
    conn.shutdown().await.expect("child cleanup");
    let path = stderr_log_path(Some(fixture.dir.path()), "stress-oracle");
    let tee = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&path).await
                && text.ends_with("mcp_stderr_records_suppressed")
            {
                break text;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("complete bounded suppression publication");
    fixture.assert_opaque(&tee);
    assert!(tee.len() < 20_000, "tee grew beyond fixed safe records");
    assert!(tee.lines().count() <= 501, "public record budget exceeded");
    assert!(
        tee.contains("mcp_stderr_overlong_record"),
        "overlong record category missing"
    );
    assert!(
        tee.lines().all(|line| matches!(
            line,
            "mcp_stderr_record" | "mcp_stderr_overlong_record" | "mcp_stderr_records_suppressed"
        )),
        "raw diagnostic content persisted"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path)
            .expect("tee metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.parent().expect("capture directory"))
            .expect("directory metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

struct ParkAfterSpawn {
    entered: tokio::sync::Notify,
    pid: std::sync::atomic::AtomicU32,
}

#[async_trait::async_trait]
impl surge_mcp::writer_observer::HostWriterObserver for ParkAfterSpawn {
    async fn before_child(
        &self,
        _server: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, surge_mcp::writer_observer::WriterObservationError>
    {
        tokio::task::yield_now().await;
        Ok(surge_core::id::ExecutionWriterId::new())
    }
    async fn child_started(
        &self,
        _writer: surge_core::id::ExecutionWriterId,
        pid: Option<u32>,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        self.pid.store(
            pid.expect("actual child PID"),
            std::sync::atomic::Ordering::SeqCst,
        );
        self.entered.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn canceled_postspawn_waiter_retains_observer_until_real_child_reaped() {
    let fixture = Fixture::new("canceled-initialize");
    let observer = std::sync::Arc::new(ParkAfterSpawn {
        entered: tokio::sync::Notify::new(),
        pid: 0.into(),
    });
    let weak = std::sync::Arc::downgrade(&observer);
    let conn = std::sync::Arc::new(McpServerConnection::new_owned(
        fixture.config.clone(),
        Some(fixture.dir.path().to_owned()),
        observer.clone(),
    ));
    let caller = {
        let conn = conn.clone();
        tokio::spawn(async move { conn.list_tools().await })
    };
    tokio::time::timeout(Duration::from_secs(5), observer.entered.notified())
        .await
        .expect("post-spawn observation barrier");
    let pid = observer.pid.load(std::sync::atomic::Ordering::SeqCst);
    caller.abort();
    assert!(caller.await.expect_err("canceled waiter").is_cancelled());
    drop(conn);
    drop(observer);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if weak.upgrade().is_none() {
                let output = tokio::process::Command::new("/bin/ps")
                    .args(["-p", &pid.to_string(), "-o", "pid="])
                    .output()
                    .await
                    .expect("actual process observation");
                assert!(
                    output.stdout.is_empty(),
                    "observer released before actual child reaped"
                );
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancellation-independent owned child settlement");
    assert!(
        !fixture
            .dir
            .path()
            .join("private-recorder.json.requests")
            .exists(),
        "canceled pre-init child received initialize"
    );
}

struct PageFence(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl surge_mcp::writer_observer::HostWriterObserver for PageFence {
    fn before_effect(
        &self,
        _server: &str,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 3 {
            Ok(())
        } else {
            Err(surge_mcp::writer_observer::WriterObservationError(
                "page denied".into(),
            ))
        }
    }
    async fn before_child(
        &self,
        _server: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, surge_mcp::writer_observer::WriterObservationError>
    {
        tokio::task::yield_now().await;
        Ok(surge_core::id::ExecutionWriterId::new())
    }
    async fn child_started(
        &self,
        _writer: surge_core::id::ExecutionWriterId,
        _pid: Option<u32>,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        tokio::task::yield_now().await;
        Ok(())
    }
}

#[tokio::test]
async fn every_actual_catalog_page_rechecks_permission_and_keeps_tool_order() {
    let mut fixture = Fixture::new("page-gate");
    if let McpTransportConfig::Stdio { env, .. } = &mut fixture.config.transport {
        env.insert("PAGINATE".into(), "1".into());
    }
    let observer = std::sync::Arc::new(PageFence(0.into()));
    let conn = McpServerConnection::new_owned(
        fixture.config.clone(),
        Some(fixture.dir.path().to_owned()),
        observer,
    );
    let result = conn.list_tools().await;
    assert!(
        matches!(result, Err(surge_mcp::McpError::EffectRefused { .. })),
        "second page must require a fresh admission"
    );
    assert_eq!(
        conn.status().await,
        surge_mcp::McpHealth::Healthy,
        "permission refusal changed liveness"
    );
    conn.shutdown().await.expect("cleanup after page refusal");
    let requests =
        std::fs::read_to_string(fixture.dir.path().join("private-recorder.json.requests"))
            .expect("actual page recorder");
    assert_eq!(
        requests
            .lines()
            .filter(|method| *method == "tools/list")
            .count(),
        1,
        "refused second page reached child"
    );
    let conn =
        McpServerConnection::new(fixture.config.clone(), Some(fixture.dir.path().to_owned()));
    let tools = conn.list_tools().await.expect("ordinary paginated catalog");
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["echo", "echo-second"]
    );
    conn.shutdown().await.expect("ordinary paginated cleanup");
}

#[tokio::test]
async fn real_health_tick_refuses_probe_and_reconnect_without_liveness_changes() {
    let fixture = Fixture::new("health-gate");
    let observer = std::sync::Arc::new(CallRefusal {
        refused: false.into(),
    });
    let conn = std::sync::Arc::new(McpServerConnection::new_owned(
        fixture.config.clone(),
        Some(fixture.dir.path().to_owned()),
        observer.clone(),
    ));
    conn.list_tools()
        .await
        .expect("actual admitted child and catalog");
    let recorded: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.dir.path().join("private-recorder.json"))
            .expect("private process recorder"),
    )
    .expect("recorder JSON");
    let pid = recorded["pid"].as_u64().expect("actual child PID");
    let token = tokio_util::sync::CancellationToken::new();
    // No real child handshake or IO waits run against an accelerated deadline.
    tokio::time::pause();
    let monitor = conn.spawn_health_monitor(token.clone());
    tokio::task::yield_now().await;
    observer
        .refused
        .store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(60)).await;
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        conn.status().await,
        surge_mcp::McpHealth::Healthy,
        "refused health RPC became liveness failure"
    );
    // Actual transport death would normally provoke recovery. Permission refusal
    // must also suppress the monitor's proactive reconnect path.
    let status = std::process::Command::new("/bin/kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .expect("terminate actual fixture child");
    assert!(status.success(), "actual child termination failed");
    for _ in 0..4 {
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::task::yield_now().await;
    }
    tokio::time::resume();
    token.cancel();
    monitor.await.expect("monitor stopped");
    assert_eq!(
        conn.status().await,
        surge_mcp::McpHealth::Healthy,
        "refusal triggered crash or restart state"
    );
    let requests =
        std::fs::read_to_string(fixture.dir.path().join("private-recorder.json.requests"))
            .expect("actual request recorder");
    let spawns = std::fs::read_to_string(fixture.dir.path().join("private-recorder.json.spawns"))
        .expect("actual spawn recorder");
    assert_eq!(
        requests
            .lines()
            .filter(|method| *method == "tools/list")
            .count(),
        1,
        "refused health probe reached child"
    );
    assert_eq!(
        spawns.lines().count(),
        1,
        "refused monitor reconnected child"
    );
    conn.shutdown()
        .await
        .expect("cleanup remains available after refusal");
}

fn recorded_methods(fixture: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fixture.dir.path().join("private-recorder.json.requests"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn advance_health_intervals() {
    tokio::time::pause();
    for _ in 0..4 {
        tokio::time::advance(Duration::from_secs(61)).await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }
    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn selected_catalog_validates_all_and_starts_no_monitors_in_either_mode() {
    for on_demand in [false, true] {
        let allowed = Fixture::new("a-allowed");
        let unselected = Fixture::new("b-unselected");
        let configs = [allowed.config.clone(), unselected.config.clone()];
        let observer: std::sync::Arc<dyn surge_mcp::writer_observer::HostWriterObserver> =
            std::sync::Arc::new(CallRefusal {
                refused: false.into(),
            });
        let registry = if on_demand {
            McpRegistry::from_config_owned_on_demand(&configs, Some(allowed.dir.path()), &observer)
        } else {
            McpRegistry::from_config_owned(&configs, Some(allowed.dir.path()), &observer)
        };
        let error = registry
            .list_tools_for_servers(&["a-allowed".into(), "z-unknown".into()])
            .await
            .unwrap_err();
        assert!(matches!(error, surge_mcp::McpError::ServerNotConfigured(_)));
        assert!(
            registry
                .list_tools_for_servers(&[])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(recorded_methods(&allowed).is_empty());
        assert!(recorded_methods(&unselected).is_empty());
        let tools = registry
            .list_tools_for_servers(&["a-allowed".into(), "a-allowed".into()])
            .await
            .unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].server, "a-allowed");
        assert_eq!(tools[0].tool, "echo");
        allowed.verify_transport();
        advance_health_intervals().await;
        assert_eq!(recorded_methods(&allowed), ["initialize", "tools/list"]);
        assert!(recorded_methods(&unselected).is_empty());
        assert!(!unselected.dir.path().join("private-recorder.json").exists());
        registry.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn on_demand_call_and_later_denial_keep_owner_without_background_reconnect() {
    let mut allowed = Fixture::new("a-allowed");
    allowed.config.restart_on_crash = true;
    let unselected = Fixture::new("b-unselected");
    let observer: std::sync::Arc<dyn surge_mcp::writer_observer::HostWriterObserver> =
        std::sync::Arc::new(CallRefusal {
            refused: false.into(),
        });
    let registry = McpRegistry::from_config_owned_on_demand(
        &[allowed.config.clone(), unselected.config.clone()],
        Some(allowed.dir.path()),
        &observer,
    );
    registry
        .list_tools_for_servers(&["a-allowed".into()])
        .await
        .unwrap();
    let result = registry
        .call_tool(
            "a-allowed",
            "success",
            serde_json::json!({}),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(
        matches!(&result.content[0], McpContent::Text(text) if text == "functional exact Ω\nline")
    );
    let before = recorded_methods(&allowed);
    // Later denied stage does not tear down the earlier connection's owner.
    assert!(
        registry
            .list_tools_for_servers(&[])
            .await
            .unwrap()
            .is_empty()
    );
    advance_health_intervals().await;
    assert_eq!(recorded_methods(&allowed), before);
    let recorded: serde_json::Value = serde_json::from_slice(
        &std::fs::read(allowed.dir.path().join("private-recorder.json")).unwrap(),
    )
    .unwrap();
    let pid = recorded["pid"].as_u64().unwrap();
    assert!(
        std::process::Command::new("/bin/kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    advance_health_intervals().await;
    assert_eq!(recorded_methods(&allowed), before);
    assert_eq!(
        std::fs::read_to_string(allowed.dir.path().join("private-recorder.json.spawns"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(recorded_methods(&unselected).is_empty());
    // Only a newly admitted request observes death and initiates ordinary restart.
    assert!(
        registry
            .list_tools_for_servers(&["a-allowed".into()])
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        registry
            .list_tools_for_servers(&["a-allowed".into()])
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(allowed.dir.path().join("private-recorder.json.spawns"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(recorded_methods(&unselected).is_empty());
    registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn legacy_all_catalog_still_performs_actual_proactive_health_probe() {
    let allowed = Fixture::new("legacy-proactive");
    let registry = McpRegistry::from_config(
        std::slice::from_ref(&allowed.config),
        Some(allowed.dir.path()),
    );
    registry.list_all_tools().await.unwrap();
    advance_health_intervals().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while recorded_methods(&allowed)
            .iter()
            .filter(|method| *method == "tools/list")
            .count()
            < 2
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        recorded_methods(&allowed)
            .iter()
            .filter(|method| *method == "tools/list")
            .count()
            >= 2
    );
    registry.shutdown().await.unwrap();
}
