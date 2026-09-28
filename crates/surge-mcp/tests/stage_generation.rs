//! Real hidden CLI helper lifetime across authenticated stage generations.
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use surge_core::{
    RunId, SessionId, id::StageGenerationId, keys::NodeKey, stage_tool::StageToolContext,
};
use surge_mcp::stage::{
    AUTH_ENV, ENDPOINT_ENV, StageEndpoint, StageToolDefinition, StageToolReply,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Helper {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}
impl Helper {
    fn spawn(address: &str, auth: &str) -> Self {
        let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/debug/surge{}",
            std::env::consts::EXE_SUFFIX
        ));
        let mut child = Command::new(binary)
            .arg("internal-stage-mcp")
            .env(ENDPOINT_ENV, address)
            .env(AUTH_ENV, auth)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
        }
    }
    async fn send(&mut self, message: Value) {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.unwrap();
        self.input.flush().await.unwrap();
    }
    async fn reply(&mut self, id: u64) -> Value {
        loop {
            let line = self
                .output
                .next_line()
                .await
                .unwrap()
                .expect("helper closed before reply");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == json!(id) {
                return value;
            }
        }
    }
    async fn initialize(&mut self) {
        self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"generation-fixture","version":"1"}}})).await;
        assert!(self.reply(1).await.get("result").is_some());
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        self.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
            .await;
        assert_eq!(
            self.reply(2).await["result"]["tools"][0]["name"],
            "report_stage_outcome"
        );
    }
    async fn exited(&mut self) -> std::process::ExitStatus {
        tokio::time::timeout(Duration::from_secs(3), self.child.wait())
            .await
            .expect("revoked helper remained alive")
            .unwrap()
    }
}
fn catalog() -> Vec<StageToolDefinition> {
    vec![StageToolDefinition {
        name: "report_stage_outcome".into(),
        description: "Candidate receipt".into(),
        input_schema: json!({"type":"object","properties":{"call_id":{"type":"string"}},"required":["call_id"]}),
    }]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_generation_cannot_reenter_while_fresh_helper_is_usable() {
    tokio::time::timeout(Duration::from_secs(10), generations())
        .await
        .expect("generation fixture exceeded watchdog");
}
async fn generations() {
    let old_context = StageToolContext {
        run: RunId::new(),
        node: NodeKey::try_from("stage").unwrap(),
        session: SessionId::new(),
        generation: StageGenerationId::new(),
    };
    let (old, mut old_calls) = StageEndpoint::open(old_context.clone(), catalog()).unwrap();
    let old_address = old.address().to_owned();
    let old_auth = old.credential().to_owned();
    let mut previous = Helper::spawn(&old_address, &old_auth);
    previous.initialize().await;
    previous.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"report_stage_outcome","arguments":{"call_id":"reused"}}})).await;
    let pending = old_calls.recv().await.unwrap();
    assert_eq!(pending.context, old_context);
    old.close().await.unwrap();
    assert!(
        previous.exited().await.success(),
        "revoked service cleanup failed"
    );
    assert!(
        pending.reply.is_closed(),
        "old invocation retained authority after revoke"
    );

    let fresh_context = StageToolContext {
        session: SessionId::new(),
        generation: StageGenerationId::new(),
        ..old_context
    };
    let (fresh, mut fresh_calls) = StageEndpoint::open(fresh_context.clone(), catalog()).unwrap();
    let mut current = Helper::spawn(fresh.address(), fresh.credential());
    current.initialize().await;
    // An actual helper replaying the old authenticated locator cannot initialize or call.
    let mut replay = Helper::spawn(&old_address, &old_auth);
    assert!(!replay.exited().await.success());
    assert!(replay.output.next_line().await.unwrap().is_none());
    let mut stale_credential = Helper::spawn(fresh.address(), &old_auth);
    assert!(!stale_credential.exited().await.success());
    assert!(stale_credential.output.next_line().await.unwrap().is_none());
    current.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"report_stage_outcome","arguments":{"call_id":"reused"}}})).await;
    let accepted = fresh_calls.recv().await.unwrap();
    assert_eq!(accepted.context, fresh_context);
    accepted
        .reply
        .send(StageToolReply {
            is_error: false,
            value: json!({"receipt":"fresh-only"}),
        })
        .unwrap();
    let result = current.reply(3).await;
    assert_ne!(result["result"]["isError"], true);
    assert!(
        result["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("fresh-only")
    );
    fresh.close().await.unwrap();
    assert!(current.exited().await.success());
}
