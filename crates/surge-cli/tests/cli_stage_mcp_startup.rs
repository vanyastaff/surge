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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_stage_helper_exits_with_provider_stdin_still_open() {
    use std::process::Stdio;
    use std::time::Duration;
    use surge_core::id::{RunId, SessionId, StageGenerationId};
    use surge_core::stage_tool::StageToolContext;
    use surge_mcp::stage::{AUTH_ENV, ENDPOINT_ENV, StageEndpoint};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let dir = tempfile::tempdir().unwrap();
    let context = StageToolContext {
        run: RunId::new(),
        node: "work".try_into().unwrap(),
        session: SessionId::new(),
        generation: StageGenerationId::new(),
    };
    let (endpoint, _calls) = StageEndpoint::open(context, Vec::new()).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_surge"))
        .arg("internal-stage-mcp")
        .current_dir(dir.path())
        .env(AUTH_ENV, endpoint.credential())
        .env(ENDPOINT_ENV, endpoint.address())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let handshake = tokio::time::timeout(Duration::from_secs(5), async {
        stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"held-stdin\",\"version\":\"1\"}}}\n").await?;
        let mut line = String::new();
        stdout.read_line(&mut line).await?;
        let response: serde_json::Value = serde_json::from_str(&line).map_err(|_| std::io::Error::other("invalid helper response"))?;
        if response["id"] != 1 || response.get("result").is_none() { return Err(std::io::Error::other("actual initialize failed")); }
        stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n").await?;
        line.clear();
        stdout.read_line(&mut line).await?;
        let response: serde_json::Value = serde_json::from_str(&line).map_err(|_| std::io::Error::other("invalid helper catalog"))?;
        if response["id"] != 2 || response["result"]["tools"] != serde_json::json!([]) { return Err(std::io::Error::other("actual catalog failed")); }
        Ok::<(), std::io::Error>(())
    }).await;
    if !matches!(handshake, Ok(Ok(()))) {
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        panic!("actual stage helper handshake timed out");
    }
    if endpoint.close().await.is_err() {
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        panic!("actual stage endpoint cleanup failed");
    }
    // Keep provider pipe open: ordinary Runtime::drop would wait on stdin's
    // uncancellable blocking read, while the dedicated host path must exit.
    let exited = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    if exited.is_err() {
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        panic!("revoked helper hung while provider kept stdin open");
    }
    assert_eq!(exited.unwrap().unwrap().code(), Some(0));
    drop(stdin);
}
