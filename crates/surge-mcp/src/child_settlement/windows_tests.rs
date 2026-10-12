//! Native job settlement is verified in isolated hosts because shutdown is terminal.
use super::*;
use std::time::{Duration, Instant};
use surge_process::windows_job::{Job, LaunchSpec};

const PROBE: &str = "child_settlement::windows_tests::native_host_probe";
const FIXTURE: &str = "child_settlement::windows_tests::native_child_fixture";

fn launch(path: &std::path::Path) -> LaunchSpec {
    LaunchSpec {
        program: std::env::current_exe().unwrap().into_os_string(),
        args: vec!["--exact".into(), FIXTURE.into(), "--nocapture".into()],
        environment: std::env::vars_os()
            .chain([
                ("SURGE_MCP_JOB_FIXTURE".into(), "leader".into()),
                ("SURGE_MCP_JOB_PIDFILE".into(), path.as_os_str().to_owned()),
            ])
            .collect(),
        current_dir: None,
    }
}

fn isolated(case: &str) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", PROBE, "--nocapture"])
        .env("SURGE_MCP_JOB_CASE", case)
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "native job settlement failed: {case}");
            return;
        }
        if started.elapsed() > Duration::from_secs(25) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native job settlement timed out: {case}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn runtime_drop_and_unpolled_close_settle_native_descendants() {
    isolated("drop");
    isolated("close");
}

#[test]
fn native_host_probe() {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    let Ok(case) = std::env::var("SURGE_MCP_JOB_CASE") else {
        return;
    };
    if case == "final-refuse" {
        final_admission_refusal();
        return;
    }
    if case == "pipes" {
        failed_native_pipes();
        return;
    }
    if case == "refuse" || case == "cancel" {
        observer_settlement(&case);
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let pidfile = directory.path().join("descendant.pid");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (mut transport, stderr) = {
        let _entered = runtime.enter();
        ObservedTransport::spawn(launch(&pidfile), None, "native-fixture").unwrap()
    };
    let job = Job::open(transport.job_name().unwrap()).unwrap().unwrap();
    let started = Instant::now();
    while !pidfile.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "descendant never started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: u32 = std::fs::read_to_string(&pidfile).unwrap().parse().unwrap();
    // SAFETY: the live fixture PID is supplied by its retained native job leader;
    // request only synchronization access and retain the returned owned handle.
    let descendant = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.unwrap();
    // SAFETY: descendant is a valid retained process handle with synchronization access.
    assert_eq!(unsafe { WaitForSingleObject(descendant, 0) }, WAIT_TIMEOUT);
    assert!(job.active_processes().unwrap() >= 2);
    drop(stderr);
    if case == "close" {
        drop(transport.close());
        drop(runtime);
        let requested = Instant::now();
        while job.active_processes().unwrap() != 0 {
            assert!(requested.elapsed() < Duration::from_secs(6));
            std::thread::sleep(Duration::from_millis(10));
        }
    } else {
        drop(runtime);
    }
    drop(transport);
    shutdown_children_and_join();
    assert_eq!(job.active_processes().unwrap(), 0);
    // SAFETY: the retained handle remains valid after its process exits.
    assert_eq!(unsafe { WaitForSingleObject(descendant, 0) }, WAIT_OBJECT_0);
    // SAFETY: this is the sole owner and the last use of the successful OpenProcess handle.
    unsafe { CloseHandle(descendant) }.unwrap();
    assert!(tracker::reserve().is_err());
}

#[test]
fn native_child_fixture() {
    let Ok(mode) = std::env::var("SURGE_MCP_JOB_FIXTURE") else {
        return;
    };
    if mode == "leader" {
        let descendant = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", FIXTURE, "--nocapture"])
            .env("SURGE_MCP_JOB_FIXTURE", "descendant")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pidfile = std::path::PathBuf::from(std::env::var_os("SURGE_MCP_JOB_PIDFILE").unwrap());
        let pending = pidfile.with_extension("pending");
        std::fs::write(&pending, descendant.id().to_string()).unwrap();
        std::fs::rename(pending, pidfile).unwrap();
        // The retained native job, rather than this dropped leader handle, owns cleanup.
        drop(descendant);
    }
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn observer_refusal_and_cancellation_settle_native_descendants() {
    isolated("refuse");
    isolated("cancel");
}

struct Observer {
    pidfile: std::path::PathBuf,
    refuse: bool,
    ready: tokio::sync::Notify,
    job: Mutex<Option<Job>>,
}

#[async_trait::async_trait]
impl HostWriterObserver for Observer {
    async fn before_child(
        &self,
        _: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, crate::writer_observer::WriterObservationError>
    {
        Ok(surge_core::id::ExecutionWriterId::new())
    }
    async fn child_started(
        &self,
        _: surge_core::id::ExecutionWriterId,
        _: Option<u32>,
    ) -> Result<(), crate::writer_observer::WriterObservationError> {
        panic!("native job observation was discarded");
    }
    async fn child_started_observed(
        &self,
        _: surge_core::id::ExecutionWriterId,
        _: Option<u32>,
        name: Option<&str>,
    ) -> Result<(), crate::writer_observer::WriterObservationError> {
        let job = Job::open(name.unwrap()).unwrap().unwrap();
        let started = Instant::now();
        while !self.pidfile.exists() {
            assert!(started.elapsed() < Duration::from_secs(5));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(job.active_processes().unwrap() >= 2);
        *self.job.lock().unwrap() = Some(job);
        self.ready.notify_one();
        if self.refuse {
            Err(crate::writer_observer::WriterObservationError(
                "fixture refusal".into(),
            ))
        } else {
            std::future::pending().await
        }
    }
}

fn observer_settlement(case: &str) {
    use surge_core::mcp_config::{McpServerRef, McpTransportConfig};
    let directory = tempfile::tempdir().unwrap();
    let pidfile = directory.path().join("descendant.pid");
    let spec = launch(&pidfile);
    let config = McpServerRef::new(
        "native-fixture".into(),
        McpTransportConfig::stdio(
            spec.program.into(),
            spec.args
                .into_iter()
                .map(|value| value.into_string().unwrap())
                .collect(),
            spec.environment
                .into_iter()
                .map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap()))
                .collect(),
        ),
        None,
        Duration::from_secs(10),
        true,
    );
    let observer = Arc::new(Observer {
        pidfile,
        refuse: case == "refuse",
        ready: tokio::sync::Notify::new(),
        job: Mutex::new(None),
    });
    let connection = Arc::new(crate::connection::McpServerConnection::new_owned(
        config,
        None,
        observer.clone(),
    ));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let connection = connection.clone();
        let operation = tokio::spawn(async move { connection.list_tools().await });
        tokio::time::timeout(Duration::from_secs(8), observer.ready.notified())
            .await
            .unwrap();
        if case == "cancel" {
            operation.abort();
            assert!(operation.await.unwrap_err().is_cancelled());
        } else {
            assert!(operation.await.unwrap().is_err());
        }
    });
    drop(connection);
    drop(runtime);
    shutdown_children_and_join();
    assert_eq!(
        observer
            .job
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .active_processes()
            .unwrap(),
        0
    );
}

#[test]
fn failed_native_pipe_preparation_retains_job_until_empty() {
    isolated("pipes");
}

fn failed_native_pipes() {
    let directory = tempfile::tempdir().unwrap();
    let pidfile = directory.path().join("descendant.pid");
    let mut child = surge_process::windows_job::NativeChild::spawn(&launch(&pidfile)).unwrap();
    let job = Job::open(child.job_name()).unwrap().unwrap();
    let started = Instant::now();
    while !pidfile.exists() {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(job.active_processes().unwrap() >= 2);
    drop(child.take_stdout());
    let (send, ticket) = tracker::reserve().unwrap();
    let reservation = Reservation {
        send,
        ticket,
        child_spawned: true,
    };
    let slot = Arc::new(Mutex::new(Some(ChildParcel {
        child,
        _owner: None,
    })));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let failure = {
        let _entered = runtime.enter();
        pipes(&slot)
    };
    assert!(failure.is_err());
    fallback(&slot, &reservation);
    reservation.ticket.completion.wait();
    assert!(tracker::lock(&slot).is_none());
    assert_eq!(job.active_processes().unwrap(), 0);
    drop(runtime);
    shutdown_children_and_join();
}

#[test]
fn native_final_admission_refuses_after_preparation_without_spawning() {
    isolated("final-refuse");
}

struct FinalAdmission(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl HostWriterObserver for FinalAdmission {
    fn before_effect(&self, _: &str) -> Result<(), crate::writer_observer::WriterObservationError> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Ok(())
        } else {
            Err(crate::writer_observer::WriterObservationError(
                "final admission refused".into(),
            ))
        }
    }
    async fn before_child(
        &self,
        _: &str,
    ) -> Result<surge_core::id::ExecutionWriterId, crate::writer_observer::WriterObservationError>
    {
        panic!("transport spawn does not reserve a new writer");
    }
    async fn child_started(
        &self,
        _: surge_core::id::ExecutionWriterId,
        _: Option<u32>,
    ) -> Result<(), crate::writer_observer::WriterObservationError> {
        panic!("refused child cannot start");
    }
}

fn final_admission_refusal() {
    let directory = tempfile::tempdir().unwrap();
    let pidfile = directory.path().join("descendant.pid");
    let observer = Arc::new(FinalAdmission(std::sync::atomic::AtomicUsize::new(0)));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    {
        let _entered = runtime.enter();
        assert!(matches!(
            ObservedTransport::spawn(launch(&pidfile), Some(observer.clone()), "fixture"),
            Err(ChildStartError::EffectRefused)
        ));
    }
    drop(runtime);
    shutdown_children_and_join();
    assert_eq!(observer.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(!pidfile.exists());
}
