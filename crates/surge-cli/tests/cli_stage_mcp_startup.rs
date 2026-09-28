//! Stage MCP stdout must never contain interactive CLI housekeeping.

#[test]
fn stage_helper_skips_orphan_prompt_and_preserves_branches() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("Test", "test@example.invalid").unwrap();
    let commit_id = repo
        .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
        .unwrap();
    repo.branch("surge/orphan", &repo.find_commit(commit_id).unwrap(), false)
        .unwrap();
    let manager = surge_git::GitManager::new(dir.path().to_path_buf()).unwrap();
    assert!(
        !surge_git::OrphanScanner::new(manager)
            .scan()
            .unwrap()
            .is_empty()
    );

    // Missing credentials should fail the helper immediately and silently on
    // stdout. The old startup path consumed this protocol frame as a cleanup
    // answer and wrote a human prompt into the MCP stream.
    assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .arg("internal-stage-mcp")
        .current_dir(dir.path())
        .env_remove("SURGE_STAGE_MCP_AUTH")
        .env_remove("SURGE_STAGE_MCP_ENDPOINT")
        .timeout(std::time::Duration::from_secs(5))
        .write_stdin("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n")
        .assert()
        .failure()
        .stdout("");
    assert!(
        repo.find_branch("surge/orphan", git2::BranchType::Local)
            .is_ok()
    );
}
