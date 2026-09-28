//! Protocol-conformant MCP client used by the controlled ACP peer.
use agent_client_protocol::schema::v1::{Error, McpServer, NewSessionRequest};
use serde_json::{Value, json};
use std::{cell::RefCell, process::Stdio, rc::Rc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};

struct Rpc {
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    next: u64,
}

pub struct Peer {
    rpc: Mutex<Rpc>,
    stop: RefCell<Option<oneshot::Sender<()>>>,
    waiter: RefCell<Option<tokio::task::JoinHandle<std::io::Result<std::process::ExitStatus>>>>,
    pub catalog: Value,
}

impl Peer {
    pub async fn connect(request: &NewSessionRequest) -> Result<Rc<Self>, Error> {
        let server = request
            .mcp_servers
            .iter()
            .find_map(|server| match server {
                McpServer::Stdio(server) if server.name == "surge-stage" => Some(server),
                _ => None,
            })
            .ok_or_else(|| Error::new(-32000, "missing surge-stage MCP descriptor"))?;
        let mut command = Command::new(&server.command);
        command
            .args(&server.args)
            // A provider may sanitize helper environments. Only the ACP descriptor
            // may supply Surge's endpoint and authentication capability.
            .env_remove("SURGE_STAGE_MCP_AUTH")
            .env_remove("SURGE_STAGE_MCP_ENDPOINT")
            .envs(server.env.iter().map(|env| (&env.name, &env.value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| Error::new(-32000, "MCP helper spawn failed"))?;
        if let Ok(path) = std::env::var("SURGE_TEST_MCP_HELPER_PID")
            && let Some(pid) = child.id()
        {
            std::fs::write(path, pid.to_string()).map_err(|_| Error::internal_error())?;
        }
        let input = child.stdin.take().ok_or_else(Error::internal_error)?;
        let output = child.stdout.take().ok_or_else(Error::internal_error)?;
        let (stop, mut stopped) = oneshot::channel();
        let waiter = tokio::task::spawn_local(async move {
            tokio::select! {
                result = child.wait() => result,
                _ = &mut stopped => { child.start_kill()?; child.wait().await },
            }
        });
        let mut peer = Self {
            rpc: Mutex::new(Rpc {
                input,
                output: BufReader::new(output).lines(),
                next: 1,
            }),
            stop: RefCell::new(Some(stop)),
            waiter: RefCell::new(Some(waiter)),
            catalog: Value::Null,
        };
        let setup = async {
            let rpc = peer.rpc.get_mut();
            rpc.call("initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"controlled-acp-peer","version":"1"}})).await?;
            rpc.notify("notifications/initialized", json!({})).await?;
            rpc.call("tools/list", json!({})).await
        }.await;
        match setup {
            Ok(catalog) => peer.catalog = catalog,
            Err(error) => {
                peer.close().await?;
                return Err(error);
            },
        }
        Ok(Rc::new(peer))
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value, Error> {
        self.rpc
            .lock()
            .await
            .call("tools/call", json!({"name":name,"arguments":arguments}))
            .await
    }

    pub async fn close(&self) -> Result<(), Error> {
        if let Some(stop) = self.stop.borrow_mut().take() {
            let _ = stop.send(());
        }
        let waiter = self.waiter.borrow_mut().take();
        if let Some(waiter) = waiter {
            tokio::time::timeout(Duration::from_secs(2), waiter)
                .await
                .map_err(|_| Error::new(-32000, "MCP helper cleanup unconfirmed"))?
                .map_err(|_| Error::internal_error())?
                .map_err(|_| Error::internal_error())?;
        }
        Ok(())
    }
}

impl Rpc {
    async fn notify(&mut self, method: &str, params: Value) -> Result<(), Error> {
        self.write(json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }
    async fn write(&mut self, value: Value) -> Result<(), Error> {
        let mut data = serde_json::to_vec(&value).map_err(|_| Error::internal_error())?;
        data.push(b'\n');
        self.input
            .write_all(&data)
            .await
            .map_err(|_| Error::internal_error())?;
        self.input
            .flush()
            .await
            .map_err(|_| Error::internal_error())
    }
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        let id = self.next;
        self.next += 1;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        loop {
            let line = self
                .output
                .next_line()
                .await
                .map_err(|_| Error::internal_error())?
                .ok_or_else(|| Error::new(-32000, "MCP helper disconnected"))?;
            let reply: Value = serde_json::from_str(&line).map_err(|_| Error::internal_error())?;
            if reply.get("id") != Some(&json!(id)) {
                continue;
            }
            if reply.get("error").is_some() {
                return Err(Error::new(-32000, "MCP RPC returned error"));
            }
            return reply
                .get("result")
                .cloned()
                .ok_or_else(Error::internal_error);
        }
    }
}

impl Peer {
    pub async fn exercise(&self, case: &str, turn: u32) -> Result<(), Error> {
        let tools = self
            .catalog
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(Error::internal_error)?;
        if tools.iter().any(|tool| {
            !matches!(
                tool.get("name").and_then(Value::as_str),
                Some("report_stage_outcome" | "request_human_input")
            )
        }) {
            return Err(Error::new(-32000, "stage catalog exposed a generic tool"));
        }
        let report = tools
            .iter()
            .find(|tool| tool.get("name") == Some(&json!("report_stage_outcome")))
            .ok_or_else(Error::internal_error)?;
        if !report["inputSchema"]["required"]
            .as_array()
            .is_some_and(|required| required.contains(&json!("call_id")))
        {
            return Err(Error::new(-32000, "missing required call_id schema"));
        }
        let mut smoke_nonce = None;
        if case == "human" || case == "stop" || case == "smoke" {
            let response = self
                .call(
                    "request_human_input",
                    json!({"call_id":"human-1","question":"Approve this change?"}),
                )
                .await?;
            if case == "smoke" {
                let value = content_value(&response)?;
                smoke_nonce = Some(
                    value
                        .get("nonce")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Error::new(-32000, "smoke response nonce missing"))?
                        .to_owned(),
                );
            }
            if case == "human"
                && content_value(&response)? != json!({"choice":"approved","nested":[1,2]})
            {
                return Err(Error::new(
                    -32000,
                    "human answer did not reach provider exactly",
                ));
            }
        }
        if case == "unknown"
            && let Ok(reply) = self
                .call(
                    "write_file",
                    json!({"call_id":"unknown-1","path":"should-not-exist","content":"bad"}),
                )
                .await
            && reply.get("isError") != Some(&Value::Bool(true))
        {
            return Err(Error::new(-32000, "generic tool was not rejected"));
        }
        let call_id = format!("report-{turn}");
        let args = json!({"call_id":call_id, "outcome":if case == "invalid" { "undeclared" } else { "done" }, "summary":smoke_nonce.as_deref().unwrap_or("real MCP completion"), "artifacts_produced":[]});
        let reply = self.call("report_stage_outcome", args.clone()).await?;
        if case == "invalid" {
            if reply.get("isError") != Some(&Value::Bool(true)) {
                return Err(Error::new(-32000, "invalid outcome was accepted"));
            }
            return Ok(());
        }
        if reply.get("isError") == Some(&Value::Bool(true)) {
            return Err(Error::new(-32000, "stage candidate rejected"));
        }
        if content_value(&reply)?.get("status")
            != Some(&json!("candidate received; final validation pending"))
        {
            return Err(Error::new(
                -32000,
                "candidate receipt made wrong completion claim",
            ));
        }
        if case == "duplicate" {
            let again = self.call("report_stage_outcome", args.clone()).await?;
            if again != reply {
                return Err(Error::new(-32000, "duplicate receipt changed"));
            }
            let mut changed = args;
            changed["summary"] = json!("changed");
            let conflict = self.call("report_stage_outcome", changed).await?;
            if conflict.get("isError") != Some(&Value::Bool(true)) {
                return Err(Error::new(-32000, "conflicting call identity accepted"));
            }
        }
        Ok(())
    }
}

fn content_value(result: &Value) -> Result<Value, Error> {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|entry| entry.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(Error::internal_error)?;
    serde_json::from_str(text).map_err(|_| Error::internal_error())
}
