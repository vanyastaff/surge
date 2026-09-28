//! Independent raw MCP wire oracles against the actual hidden CLI subprocess.
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use surge_core::{
    RunId, SessionId, id::StageGenerationId, keys::NodeKey, stage_tool::StageToolContext,
};
use surge_mcp::stage::{AUTH_ENV, ENDPOINT_ENV, StageEndpoint, StageToolDefinition};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
};

struct Helper {
    child: Child,
    errors: ChildStderr,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}
impl Helper {
    fn spawn(endpoint: &StageEndpoint) -> Self {
        let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/debug/surge{}",
            std::env::consts::EXE_SUFFIX
        ));
        let mut child = Command::new(binary)
            .arg("internal-stage-mcp")
            .env(ENDPOINT_ENV, endpoint.address())
            .env(AUTH_ENV, endpoint.credential())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            errors: child.stderr.take().unwrap(),
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
        }
    }
    async fn send(&mut self, message: Value) {
        self.input
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn reply(&mut self, id: u64) -> Value {
        loop {
            let line = self
                .output
                .next_line()
                .await
                .unwrap()
                .expect("helper ended before reply");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == json!(id) {
                return value;
            }
        }
    }
    async fn initialize(&mut self) {
        self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"hardening-fixture","version":"1"}}})).await;
        assert!(self.reply(1).await.get("result").is_some());
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
    }
    async fn reap(&mut self) {
        if tokio::time::timeout(Duration::from_secs(3), self.child.wait())
            .await
            .is_err()
        {
            self.child.kill().await.unwrap();
            self.child.wait().await.unwrap();
            panic!("helper failed to settle after endpoint revoke");
        }
    }
}
fn endpoint() -> (
    StageEndpoint,
    tokio::sync::mpsc::Receiver<surge_mcp::stage::StageToolCall>,
) {
    let context = StageToolContext {
        run: RunId::new(),
        node: NodeKey::try_from("stage").unwrap(),
        session: SessionId::new(),
        generation: StageGenerationId::new(),
    };
    let tools = ["report_stage_outcome", "request_human_input"].into_iter().map(|name| StageToolDefinition {
        name: name.into(), description: "Controlled fixture".into(),
        input_schema: json!({"type":"object","properties":{"call_id":{"type":"string"}},"required":["call_id"]}),
    }).collect();
    StageEndpoint::open(context, tools).unwrap()
}
fn call(id: u64) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"request_human_input","arguments":{"call_id":format!("call-{id}")}}})
}

#[tokio::test]
async fn ninth_pending_call_never_reaches_stage_owner() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (endpoint, mut calls) = endpoint();
        let mut helper = Helper::spawn(&endpoint);
        helper.initialize().await;
        let mut pending = Vec::new();
        for id in 10..18 {
            helper.send(call(id)).await;
            pending.push(
                tokio::time::timeout(Duration::from_secs(1), calls.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        helper.send(call(18)).await;
        let ninth = tokio::time::timeout(Duration::from_millis(300), calls.recv()).await;
        let rejected = tokio::time::timeout(Duration::from_millis(300), helper.reply(18)).await;
        endpoint.close().await.unwrap();
        helper.reap().await;
        assert!(
            ninth.is_err(),
            "overload must be rejected before stage dispatch"
        );
        assert!(
            rejected.unwrap().get("error").is_some(),
            "overload needs an explicit MCP error"
        );
        assert!(pending.iter().all(|call| call.reply.is_closed()));
    })
    .await
    .expect("concurrency fixture watchdog");
}

#[tokio::test]
async fn oversized_unterminated_stdio_frame_ends_helper() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (endpoint, mut calls) = endpoint();
        let mut helper = Helper::spawn(&endpoint);
        helper.initialize().await;
        let oversized = vec![b'x'; 256 * 1024 + 1];
        let _ = helper.input.write_all(&oversized).await;
        let terminated =
            tokio::time::timeout(Duration::from_millis(800), helper.child.wait()).await;
        endpoint.close().await.unwrap();
        helper.reap().await;
        assert!(
            terminated.is_ok(),
            "oversized input must fail before waiting for newline"
        );
        assert!(!terminated.unwrap().unwrap().success());
        assert!(calls.try_recv().is_err());
    })
    .await
    .expect("oversized fixture watchdog");
}

#[tokio::test]
async fn cancellation_releases_pending_call_and_fresh_call_remains_usable() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (endpoint, mut calls) = endpoint();
        let mut helper = Helper::spawn(&endpoint);
        helper.initialize().await;
        helper.send(call(10)).await;
        let pending = calls.recv().await.unwrap();
        helper.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":10}})).await;
        let cancelled = helper.reply(10).await;
        assert_eq!(cancelled["result"]["isError"], true);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !pending.reply.is_closed() { tokio::task::yield_now().await; }
        }).await.expect("cancelled human call retained authority");
        helper.send(json!({"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"report_stage_outcome","arguments":{"call_id":"fresh"}}})).await;
        let fresh = calls.recv().await.unwrap();
        fresh.reply.send(surge_mcp::stage::StageToolReply { is_error: false, value: json!({"receipt":"controlled-owner"}) }).unwrap();
        assert_ne!(helper.reply(11).await["result"]["isError"], true);
        helper.send(call(12)).await;
        let pending = calls.recv().await.unwrap();
        endpoint.close().await.unwrap();
        helper.reap().await;
        assert!(pending.reply.is_closed());
    }).await.expect("cancellation fixture watchdog");
}

#[tokio::test]
async fn request_129_exhausts_helper_but_notifications_do_not_spend_quota() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (endpoint, mut calls) = endpoint();
        let mut helper = Helper::spawn(&endpoint);
        helper.initialize().await; // request1
        for _ in 0..200 {
            helper.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9999}})).await;
        }
        for id in 2..=128 {
            helper.send(json!({"jsonrpc":"2.0","id":id,"method":"ping"})).await;
            assert!(helper.reply(id).await.get("result").is_some());
        }
        helper.send(json!({"jsonrpc":"2.0","id":129,"method":"ping"})).await;
        let rejected = helper.reply(129).await;
        let status = tokio::time::timeout(Duration::from_secs(3), helper.child.wait()).await.unwrap().unwrap();
        endpoint.close().await.unwrap();
        assert!(rejected.get("error").is_some());
        assert!(!status.success());
        assert!(calls.try_recv().is_err());
    }).await.expect("request lifetime fixture watchdog");
}

#[tokio::test]
async fn malformed_and_batch_input_close_without_echoing_payload() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for malformed in [
            "[{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}]\n",
            "{\"secret-marker-do-not-echo\": invalid}\n",
        ] {
            let (endpoint, mut calls) = endpoint();
            let mut helper = Helper::spawn(&endpoint);
            helper.initialize().await;
            helper.input.write_all(malformed.as_bytes()).await.unwrap();
            let status = tokio::time::timeout(Duration::from_secs(3), helper.child.wait())
                .await
                .unwrap()
                .unwrap();
            endpoint.close().await.unwrap();
            let mut errors = String::new();
            helper.errors.read_to_string(&mut errors).await.unwrap();
            assert!(!status.success());
            assert!(!errors.contains("secret-marker-do-not-echo"));
            assert!(calls.try_recv().is_err());
        }
    })
    .await
    .expect("malformed fixture watchdog");
}

#[tokio::test]
async fn revocation_interrupts_flood_even_when_stdout_is_blocked() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (endpoint, mut calls) = endpoint();
        let mut helper = Helper::spawn(&endpoint);
        helper.initialize().await;
        let mut pending = Vec::new();
        for id in 10..18 {
            helper.send(call(id)).await;
            pending.push(calls.recv().await.unwrap());
        }
        pending.pop().unwrap().reply.send(surge_mcp::stage::StageToolReply { is_error: false, value: json!({"controlled_response":"x".repeat(200 * 1024)}) }).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let flood: String = (20..100).map(|id| format!("{}\n", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"request_human_input","arguments":{"call_id":"flood","padding":"x".repeat(4096)}}}))).collect();
        let _ = tokio::time::timeout(Duration::from_millis(100), helper.input.write_all(flood.as_bytes())).await;
        endpoint.close().await.unwrap();
        helper.reap().await;
        assert!(pending.iter().all(|call| call.reply.is_closed()));
    }).await.expect("flood revoke fixture watchdog");
}

#[tokio::test]
async fn duplicate_active_rpc_id_closes_without_ambiguous_second_response() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (endpoint, mut calls) = endpoint();
        let mut helper = Helper::spawn(&endpoint);
        helper.initialize().await;
        helper.send(call(10)).await;
        let pending = calls.recv().await.unwrap();
        helper.send(call(10)).await;
        let terminated =
            tokio::time::timeout(Duration::from_millis(500), helper.child.wait()).await;
        endpoint.close().await.unwrap();
        helper.reap().await;
        assert!(
            terminated.is_ok(),
            "duplicate active wire ID must close instead of competing with original reply"
        );
        assert!(!terminated.unwrap().unwrap().success());
        assert!(pending.reply.is_closed());
        assert!(calls.try_recv().is_err());
    })
    .await
    .expect("duplicate ID fixture watchdog");
}
