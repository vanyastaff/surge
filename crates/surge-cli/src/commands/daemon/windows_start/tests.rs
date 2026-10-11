use super::*;
use interprocess::local_socket::{ListenerOptions, tokio::prelude::*};
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/test-support/runtime_home.rs"
));

#[test]
#[ignore = "explicit child-process fixture; launched by native ownership tests"]
fn listener_child() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        if let Some(socket) = std::env::var_os("SURGE_STARTUP_TEST_SOCKET") {
            let name =
                surge_orchestrator::engine::ipc::local_socket_name_from_path(Path::new(&socket))
                    .unwrap();
            let listener = ListenerOptions::new().name(name).create_tokio().unwrap();
            let _stream = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
        } else {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

fn launch(home: &Path, socket: Option<&Path>) -> Startup {
    let owner = RuntimeHomeOwner::prepare(home).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "commands::daemon::windows_start::tests::listener_child",
            "--ignored",
            "--nocapture",
        ])
        .env_remove("SURGE_STARTUP_TEST_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(socket) = socket {
        command.env("SURGE_STARTUP_TEST_SOCKET", socket);
    }
    Startup::spawn(command, owner, Vec::new()).unwrap()
}

#[tokio::test]
async fn foreign_listener_cannot_handoff_live_spawned_child() {
    let home = FixtureHome::new().unwrap();
    let socket = home.path().join("daemon/foreign.sock");
    let name = surge_orchestrator::engine::ipc::local_socket_name_from_path(&socket).unwrap();
    let listener = ListenerOptions::new().name(name).create_tokio().unwrap();
    let mut startup = launch(home.path(), None);
    let error = child_listener_ready(&socket, startup.child())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not belong to the spawned child")
    );
    assert!(startup.child().try_wait().unwrap().is_none());
    // Keep the actual Child handle available to assert observed exit after settlement.
    settle_child(startup.child());
    assert!(startup.child().try_wait().unwrap().is_some());
    // Drain the rejected connection; it belonged to this still-live foreign server.
    drop(listener.accept().await.unwrap());
    // The foreign listener still accepts after only the launched child was stopped.
    let name = surge_orchestrator::engine::ipc::local_socket_name_from_path(&socket).unwrap();
    let (connected, accepted) = tokio::join!(LocalSocketStream::connect(name), listener.accept());
    drop(connected.unwrap());
    drop(accepted.unwrap());
    drop(listener);
    drop(startup);
    home.close().unwrap();
}

#[tokio::test]
async fn exact_child_listener_is_ready_and_remains_live_until_settled() {
    let home = FixtureHome::new().unwrap();
    let socket = home.path().join("daemon/owned.sock");
    let mut startup = launch(home.path(), Some(&socket));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    tokio::time::timeout_at(deadline, async {
        loop {
            if child_listener_ready(&socket, startup.child())
                .await
                .unwrap()
            {
                break;
            }
            assert!(startup.child().try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(startup.child().try_wait().unwrap().is_none());
    settle_child(startup.child());
    assert!(startup.child().try_wait().unwrap().is_some());
    drop(startup);
    home.close().unwrap();
}

#[test]
fn failed_start_settles_direct_child_before_namespace_release() {
    let home = FixtureHome::new().unwrap();
    let mut startup = launch(home.path(), None);
    let directory = startup._home.directory(RuntimeDirectory::Daemon).unwrap();
    let log = directory.open_append("failure.log".as_ref()).unwrap();
    let (stdio, lease) = log.stdio_clone().unwrap();
    startup.logs.push(lease);
    settle_child(startup.child());
    assert!(startup.child().try_wait().unwrap().is_some());
    drop(log);
    drop(directory);
    drop(stdio);
    drop(startup);
    home.close().unwrap();
}

#[test]
fn failed_spawn_releases_retained_output_and_home() {
    let home = FixtureHome::new().unwrap();
    let owner = RuntimeHomeOwner::prepare(home.path()).unwrap();
    let directory = owner.directory(RuntimeDirectory::Daemon).unwrap();
    let log = directory.open_append("spawn-failure.log".as_ref()).unwrap();
    let (stdio, lease) = log.stdio_clone().unwrap();
    let mut command = Command::new(home.path().join("absent-daemon.exe"));
    command.stdout(stdio);
    let result = Startup::spawn(command, owner, vec![lease]);
    assert!(matches!(result, Err(error) if error.to_string() == "spawn surge-daemon"));
    drop(log);
    drop(directory);
    home.close().unwrap();
}

#[tokio::test]
async fn cancellation_settles_child_before_startup_owner_drops() {
    let home = FixtureHome::new().unwrap();
    let mut startup = launch(home.path(), None);
    let pid = startup.child().id();
    assert!(surge_daemon::pidfile::is_alive(pid));
    let task = tokio::spawn(async move {
        let owner = startup;
        std::future::pending::<()>().await;
        drop(owner);
    });
    tokio::task::yield_now().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(!surge_daemon::pidfile::is_alive(pid));
    home.close().unwrap();
}
