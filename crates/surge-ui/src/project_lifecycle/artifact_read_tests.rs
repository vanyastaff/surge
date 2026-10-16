use super::*;
#[test]
fn accepted_roadmap_rejects_oversized_regular_file_before_parsing() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("accepted-roadmap");
    let mut bytes = b"schema_version=2\n[[milestones]]\nid=\"first\"\ntitle=\"First\"\n".to_vec();
    bytes.resize(1024 * 1024 + 1, b' ');
    std::fs::write(&path, &bytes).unwrap();
    let result = loading::read_accepted_roadmap(&path, surge_core::ContentHash::compute(&bytes));
    assert!(
        result.is_err(),
        "accepted roadmap must reject files larger than1MiB"
    );
}
#[cfg(unix)]
#[test]
fn accepted_roadmap_fifo_is_rejected_without_waiting_for_a_writer() {
    const MARKER: &str = "SURGE_HOME_FIFO_READ_PROBE";
    if let Some(path) = std::env::var_os(MARKER) {
        let path = std::path::PathBuf::from(path);
        std::fs::write(path.with_extension("ready"), b"opening").unwrap();
        assert!(
            loading::read_accepted_roadmap(&path, surge_core::ContentHash::compute(b"unused"))
                .is_err()
        );
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let mut child = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "project_lifecycle::tests::artifact_read_tests::accepted_roadmap_fifo_is_rejected_without_waiting_for_a_writer", "--nocapture"]).env(MARKER, &path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let completed = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(
        path.with_extension("ready").exists(),
        "child reached the actual read seam"
    );
    assert!(
        completed.is_some_and(|status| status.success()),
        "accepted roadmap FIFO read blocked with no writer"
    );
}
