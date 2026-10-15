//! Bootstrap admission must not depend on the desktop launch directory.
#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use surge_core::{RunId, bootstrap_operation::BootstrapIntent, budget::BudgetGuard};

struct Daemon {
    child: Child,
    socket: PathBuf,
    log: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Daemon {
    fn spawn(home: &Path, cwd: &Path) -> Self {
        Self::with_active_limit(home, cwd, 8)
    }

    fn with_active_limit(home: &Path, cwd: &Path, maximum: usize) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let log = home.join("startup-test.log");
        let child = Command::new(env!("CARGO_BIN_EXE_surge-daemon"))
            .args(["--shutdown-grace", "100ms"])
            .args(["--max-active", &maximum.to_string(), "--max-queue", "8"])
            .env("SURGE_HOME", home.canonicalize().unwrap())
            .env_remove("SURGE_PROFILES_DIR")
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        Self {
            child,
            socket: home.join("daemon/daemon.sock"),
            log,
        }
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while UnixStream::connect(&self.socket).is_err() {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "daemon startup exited"
            );
            assert!(Instant::now() < deadline, "daemon startup timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn rpc(&self, request: Value) -> Value {
        let mut connection = UnixStream::connect(&self.socket).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        writeln!(connection, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(connection).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn submit(&self, project: &Path, id: RunId) -> Value {
        let intent = BootstrapIntent::new(
            project.to_path_buf(),
            "Plan a small test application".into(),
            BudgetGuard::default(),
        )
        .unwrap();
        self.rpc(
            json!({"method":"start_bootstrap","request_id":1,"operation_id":id,"intent":intent}),
        )
    }

    fn diagnostic(&self, id: RunId) -> String {
        let status =
            self.rpc(json!({"method":"bootstrap_status","request_id":2,"operation_id":id}));
        format!(
            "{status}; daemon log: {}",
            std::fs::read_to_string(&self.log).unwrap()
        )
    }
}

fn git(project: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn configured_project(root: &Path, marker: &Path) -> PathBuf {
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    let script = root.join("test-provider.sh");
    // An intentionally failing ACP test fixture: dispatch is observable without any real provider.
    std::fs::write(&script, "printf 'started\\n' >> \"$1\"\nexit 1\n").unwrap();
    let config = json!({"schema_version":1,"default_agent":"startup-fixture","agents":{"startup-fixture":{
        "command":"/bin/sh","args":[script,marker],"transport":"stdio"
    }}});
    std::fs::write(
        project.join("surge.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    surge_core::SurgeConfig::load(&project.join("surge.toml")).unwrap();
    git(&project, &["init"]);
    git(&project, &["config", "user.name", "Surge Test"]);
    git(&project, &["config", "user.email", "test@example.invalid"]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-m", "configured test project"]);
    project.canonicalize().unwrap()
}

fn fixture_profiles(home: &Path) {
    let profiles = home.join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    for mut profile in surge_core::profile::bundled::BundledRegistry::all() {
        if ["description-author", "roadmap-planner", "flow-generator"]
            .contains(&profile.role.id.as_str())
        {
            profile.runtime.agent_id = "startup-fixture".into();
            profile.hooks = surge_core::profile::ProfileHooks::default();
            std::fs::write(
                profiles.join(format!("{}.toml", profile.role.id)),
                toml::to_string(&profile).unwrap(),
            )
            .unwrap();
        }
    }
}

fn reservation_count(home: &Path) -> i64 {
    let connection = rusqlite::Connection::open_with_flags(
        home.join("db/registry.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    connection
        .query_row("SELECT count(*) FROM bootstrap_operations", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn bundled_planning_dispatches_the_projects_selected_default_provider() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("unrelated");
    std::fs::create_dir(&cwd).unwrap();
    let home = root.path().join("home");
    let selected = root.path().join("selected-provider");
    let original = root.path().join("original-provider");
    let project = configured_project(root.path(), &selected);
    let path = project.join("surge.toml");
    let mut config = surge_core::SurgeConfig::load(&path).unwrap();
    let mut claude = config.agents["startup-fixture"].clone();
    claude.args[1] = original.to_string_lossy().into_owned();
    claude.env.insert(
        "UNUSED_PROVIDER_SECRET".into(),
        surge_core::config::AgentEnvValue::Inject {
            from: format!("SURGE_UNUSED_{}", RunId::new()),
            default: None,
            required: true,
        },
    );
    config.agents.insert("claude-acp".into(), claude);
    std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
    git(&project, &["add", "."]);
    git(
        &project,
        &["commit", "-m", "two controlled provider recipes"],
    );
    // No disk profile overrides: exercise the actual bundled planning profiles.
    let mut daemon = Daemon::spawn(&home, &cwd);
    daemon.ready();
    let id = RunId::new();
    let accepted = daemon.submit(&project, id);
    assert_eq!(accepted["method"], "bootstrap_operation", "{accepted}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !selected.exists() {
        assert!(
            !original.exists(),
            "bundled profile launched the original provider instead of the project default"
        );
        assert!(Instant::now() < deadline, "{}", daemon.diagnostic(id));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!original.exists());
    assert_eq!(reservation_count(&home), 1);
}

#[test]
fn selected_provider_missing_credential_refuses_before_reservation_or_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let marker = root.path().join("selected-provider");
    let project = configured_project(root.path(), &marker);
    let path = project.join("surge.toml");
    let mut config = surge_core::SurgeConfig::load(&path).unwrap();
    config
        .agents
        .get_mut("startup-fixture")
        .unwrap()
        .env
        .insert(
            "REQUIRED_PROVIDER_SECRET".into(),
            surge_core::config::AgentEnvValue::Inject {
                from: format!("SURGE_REQUIRED_{}", RunId::new()),
                default: None,
                required: true,
            },
        );
    std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
    git(&project, &["add", "."]);
    git(
        &project,
        &[
            "commit",
            "-m",
            "selected provider requires absent credential",
        ],
    );
    let mut daemon = Daemon::spawn(&home, root.path());
    daemon.ready();
    let rejected = daemon.submit(&project, RunId::new());
    assert_eq!(rejected["method"], "error", "{rejected}");
    assert!(
        rejected["message"].as_str().unwrap().contains("credential"),
        "{rejected}"
    );
    assert_eq!(reservation_count(&home), 0);
    assert!(!marker.exists());
}

fn captured_pins(home: &Path, id: RunId) -> String {
    let connection = rusqlite::Connection::open_with_flags(
        home.join("db/registry.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    connection
        .query_row(
            "SELECT capture_json FROM bootstrap_operations WHERE operation_id=?",
            [id.to_string()],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn cold_restart_refuses_changed_project_or_profile_without_recapture_or_dispatch() {
    for changed in ["project", "profile", "default", "provenance"] {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("unrelated");
        std::fs::create_dir(&cwd).unwrap();
        let home = root.path().join("home");
        let marker = root.path().join("provider-started");
        let project = configured_project(root.path(), &marker);
        if changed != "provenance" {
            fixture_profiles(&home);
        }
        if changed == "default" {
            let path = project.join("surge.toml");
            let mut config = surge_core::SurgeConfig::load(&path).unwrap();
            config.agents.insert(
                "alternative-fixture".into(),
                config.agents["startup-fixture"].clone(),
            );
            std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
            git(&project, &["add", "."]);
            git(&project, &["commit", "-m", "two recipes before capture"]);
        }
        // Zero active slots keeps the real accepted operation queued while
        // its original immutable capture is committed by the production owner.
        let mut first = Daemon::with_active_limit(&home, &cwd, 0);
        first.ready();
        let id = RunId::new();
        let accepted = first.submit(&project, id);
        assert_eq!(accepted["method"], "bootstrap_operation", "{accepted}");
        assert_eq!(accepted["status"]["state"]["phase"], "queued_planning");
        let original = captured_pins(&home, id);
        let capture: surge_core::bootstrap_operation::BootstrapCapture =
            serde_json::from_str(&original).unwrap();
        assert_eq!(
            serde_json::to_value(capture.fields().planning_run).unwrap(),
            accepted["status"]["planning_run"]
        );
        assert_eq!(
            serde_json::to_value(capture.fields().implementation_run).unwrap(),
            accepted["status"]["implementation_run"]
        );
        assert_eq!(reservation_count(&home), 1);
        assert!(!marker.exists());
        assert_no_engine_dispatch(&home, &capture);
        drop(first); // Kill and join the original host; no in-memory runtime survives.
        change_captured_input(&home, &project, changed);
        let mut restarted = Daemon::spawn(&home, &cwd);
        restarted.ready();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let reply = restarted
                .rpc(json!({"method":"bootstrap_status","request_id":3,"operation_id":id}));
            if reply["status"]["state"]["state"] == "needs_attention" {
                assert_eq!(
                    reply["status"]["state"]["reason"], "configuration_changed",
                    "{changed}: {reply}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{changed}: {}",
                restarted.diagnostic(id)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let duplicate = restarted.submit(&project, id);
        assert_eq!(
            duplicate["status"]["planning_run"],
            accepted["status"]["planning_run"]
        );
        assert_eq!(
            duplicate["status"]["implementation_run"],
            accepted["status"]["implementation_run"]
        );
        assert_eq!(captured_pins(&home, id), original);
        assert_eq!(reservation_count(&home), 1);
        assert_no_engine_dispatch(&home, &capture);
        assert!(
            !marker.exists(),
            "{changed}: recovery launched a provider despite changed pins"
        );
    }
}

fn assert_no_engine_dispatch(
    home: &Path,
    capture: &surge_core::bootstrap_operation::BootstrapCapture,
) {
    let pins = capture.fields();
    for run in [pins.planning_run, pins.implementation_run] {
        assert!(
            !home
                .join("runs")
                .join(run.to_string())
                .join("events.sqlite")
                .exists(),
            "provider execution must not have materialized a run"
        );
    }
    assert!(!pins.planning_worktree.exists());
    assert!(!pins.implementation_worktree.exists());
}

fn change_captured_input(home: &Path, project: &Path, changed: &str) {
    if changed == "project" || changed == "default" {
        let path = project.join("surge.toml");
        let mut config = surge_core::SurgeConfig::load(&path).unwrap();
        if changed == "default" {
            config.default_agent = "alternative-fixture".into();
        } else {
            config.init.project_context_auto_seed = !config.init.project_context_auto_seed;
        }
        std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
    } else if changed == "provenance" {
        let profiles = home.join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        let profile = surge_core::profile::bundled::BundledRegistry::all()
            .into_iter()
            .find(|profile| profile.role.id.as_str() == "description-author")
            .unwrap();
        std::fs::write(
            profiles.join("description-author.toml"),
            toml::to_string(&profile).unwrap(),
        )
        .unwrap();
    } else {
        let path = home.join("profiles/description-author.toml");
        let mut profile: surge_core::profile::Profile =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        profile
            .prompt
            .system
            .push_str("\nChanged after original acceptance.\n");
        std::fs::write(path, toml::to_string(&profile).unwrap()).unwrap();
    }
}

#[test]
fn empty_unrelated_startup_directory_admits_configured_project() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("empty");
    std::fs::create_dir(&cwd).unwrap();
    let home = root.path().join("home");
    let marker = root.path().join("provider-started");
    let project = configured_project(root.path(), &marker);
    fixture_profiles(&home);
    let mut daemon = Daemon::spawn(&home, &cwd);
    daemon.ready();
    let id = RunId::new();
    let reply = daemon.submit(&project, id);
    assert_eq!(reply["method"], "bootstrap_operation", "{reply}");
    assert_eq!(reservation_count(&home), 1);
    let duplicate = daemon.submit(&project, id);
    assert_eq!(
        duplicate["status"]["planning_run"],
        reply["status"]["planning_run"]
    );
    assert_eq!(reservation_count(&home), 1);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "configured test provider was never dispatched: {}",
            daemon.diagnostic(id)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "started\n");
}

#[test]
fn absent_or_invalid_project_configuration_cannot_reserve_or_dispatch() {
    for content in [None, Some("not valid TOML [")] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let marker = root.path().join("provider-started");
        let project = configured_project(root.path(), &marker);
        if let Some(content) = content {
            std::fs::write(project.join("surge.toml"), content).unwrap();
        } else {
            std::fs::remove_file(project.join("surge.toml")).unwrap();
        }
        git(&project, &["add", "-A"]);
        git(&project, &["commit", "-m", "invalid project input"]);
        fixture_profiles(&home);
        let mut daemon = Daemon::spawn(&home, root.path());
        daemon.ready();
        let reply = daemon.submit(&project, RunId::new());
        assert_eq!(reply["method"], "error", "{reply}");
        assert_eq!(reservation_count(&home), 0);
        assert!(!marker.exists());
    }
}

#[test]
fn one_default_host_dispatches_each_projects_distinct_recipe() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let cwd = root.path().join("unrelated");
    std::fs::create_dir(&cwd).unwrap();
    let mut daemon = Daemon::spawn(&home, &cwd);
    daemon.ready();
    let mut markers = Vec::new();
    let wrong_marker = root.path().join("wrong-provider");
    let mut planning_runs = Vec::new();
    for name in ["first", "second"] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        let marker = directory.join("dispatched");
        let project = configured_project(&directory, &marker);
        if name == "second" {
            let path = project.join("surge.toml");
            let mut config = surge_core::SurgeConfig::load(&path).unwrap();
            config.agents.insert(
                "alternative-fixture".into(),
                config.agents["startup-fixture"].clone(),
            );
            config.agents.get_mut("startup-fixture").unwrap().args[1] =
                wrong_marker.to_string_lossy().into_owned();
            config.default_agent = "alternative-fixture".into();
            std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
            git(&project, &["add", "."]);
            git(
                &project,
                &["commit", "-m", "distinct second project default"],
            );
        }
        let reply = daemon.submit(&project, RunId::new());
        assert_eq!(reply["method"], "bootstrap_operation", "{reply}");
        planning_runs.push(reply["status"]["planning_run"].clone());
        markers.push(marker);
    }
    assert_ne!(planning_runs[0], planning_runs[1]);
    assert_eq!(reservation_count(&home), 2);
    let deadline = Instant::now() + Duration::from_secs(15);
    while markers.iter().any(|marker| !marker.exists()) {
        assert!(
            Instant::now() < deadline,
            "project-specific recipes did not both dispatch"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    for marker in markers {
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "started\n");
    }
    assert!(!wrong_marker.exists());
}

#[test]
fn project_capacity_cannot_override_default_hosts_global_policy() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let marker = root.path().join("provider-started");
    let project = configured_project(root.path(), &marker);
    let path = project.join("surge.toml");
    let mut config = surge_core::SurgeConfig::load(&path).unwrap();
    // The exact field update below is distinct from the daemon default policy.
    config
        .capacity
        .fallback_agents
        .push("startup-fixture".into());
    std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-m", "different capacity"]);
    fixture_profiles(&home);
    let mut daemon = Daemon::spawn(&home, root.path());
    daemon.ready();
    let reply = daemon.submit(&project, RunId::new());
    assert_eq!(reply["method"], "error", "{reply}");
    assert_eq!(reservation_count(&home), 0);
    assert!(!marker.exists());
}

#[test]
fn invalid_nearest_host_entry_cannot_fall_back_to_valid_ancestor() {
    for entry in ["directory", "dangling", "malformed"] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("surge.toml"), "schema_version = 1\n").unwrap();
        let cwd = root.path().join("child");
        std::fs::create_dir(&cwd).unwrap();
        let config = cwd.join("surge.toml");
        match entry {
            "directory" => std::fs::create_dir(config).unwrap(),
            "dangling" => std::os::unix::fs::symlink("missing", config).unwrap(),
            _ => std::fs::write(config, "invalid [").unwrap(),
        }
        let home = root.path().join("home");
        let mut daemon = Daemon::spawn(&home, &cwd);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = daemon.child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(2), "{entry}: {status}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{entry}: invalid host entry allowed daemon startup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!daemon.socket.exists());
    }
}
