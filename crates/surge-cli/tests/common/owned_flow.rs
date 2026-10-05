//! Isolated committed source and retained daemon process for owned Flow tests.
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub fn commit_project(project: &Path) {
    let repo = git2::Repository::init(project).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.com").unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .unwrap();
}

pub struct Daemon(pub Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn start_daemon(home: &Path, project: &Path, checkpoint: Option<&str>) -> Daemon {
    std::fs::create_dir_all(home).unwrap();
    let executable = Path::new(env!("CARGO_BIN_EXE_surge")).with_file_name(if cfg!(windows) {
        "surge-daemon.exe"
    } else {
        "surge-daemon"
    });
    let log = std::fs::File::create(home.join("fixture-daemon.log")).unwrap();
    let mut command = Command::new(executable);
    command
        .current_dir(project)
        .env("SURGE_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .stdout(Stdio::null())
        .stderr(log);
    if let Some(node) = checkpoint {
        command.env("SURGE_CHECKPOINT_EXIT", node);
    }
    let mut daemon = Daemon(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "fixture daemon exited during startup"
        );
        let ready = Command::new(env!("CARGO_BIN_EXE_surge"))
            .args(["daemon", "status"])
            .env("SURGE_HOME", home)
            .current_dir(project)
            .output()
            .unwrap();
        if ready.status.success() && String::from_utf8_lossy(&ready.stdout).contains("ping:   ok") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fixture daemon startup timed out: {ready:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    daemon
}
