//! Stage-owned MCP receipts and reliable reply routing.

use super::StageError;
use std::collections::HashMap;
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_core::content_hash::ContentHash;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::stage_tool::{
    StageToolCallId, StageToolContext, StageToolReceipt, StageToolResult,
};
use surge_mcp::stage::{StageToolCall, StageToolReply};
use surge_persistence::runs::run_writer::RunWriter;

pub(super) struct StageCalls {
    pub context: StageToolContext,
    pending: HashMap<String, Pending>,
    receipts: HashMap<StageToolCallId, StageToolReceipt>,
}

struct Pending {
    call: StageToolCall,
    id: StageToolCallId,
    hash: ContentHash,
}

impl StageCalls {
    pub fn new(context: StageToolContext) -> Self {
        Self {
            context,
            pending: HashMap::new(),
            receipts: HashMap::new(),
        }
    }

    /// Authenticity is pinned by the transport; this also guards accidental cross-stage routing.
    pub fn admit(&mut self, call: StageToolCall) -> Option<String> {
        if call.context != self.context {
            let _ = call
                .reply
                .send(StageToolReply::rejected("stage identity mismatch"));
            return None;
        }
        let id = call
            .arguments
            .get("call_id")
            .and_then(|v| v.as_str())
            .and_then(|value| StageToolCallId::try_from(value.to_owned()).ok());
        let Some(id) = id else {
            let _ = call
                .reply
                .send(StageToolReply::rejected("valid call_id is required"));
            return None;
        };
        let hash = ContentHash::compute(
            serde_json::json!({"tool":call.tool,"arguments":call.arguments})
                .to_string()
                .as_bytes(),
        );
        if let Some(receipt) = self.receipts.get(&id) {
            let response = if receipt.arguments_hash == hash {
                receipt_reply(receipt)
            } else {
                StageToolReply::rejected("call_id already used with different arguments")
            };
            let _ = call.reply.send(response);
            return None;
        }
        if self.receipts.len() >= 128 {
            let _ = call
                .reply
                .send(StageToolReply::rejected("stage tool receipt limit reached"));
            return None;
        }
        let key = format!(
            "{}:{}:{}",
            self.context.session,
            self.context.generation,
            id.as_str()
        );
        if self.pending.contains_key(&key) {
            let _ = call
                .reply
                .send(StageToolReply::rejected("call_id is already pending"));
            return None;
        }
        self.pending.insert(key.clone(), Pending { call, id, hash });
        Some(key)
    }

    pub async fn event(
        &mut self,
        writer: &RunWriter,
        key: &str,
        declared: &[surge_core::OutcomeKey],
    ) -> Result<Option<BridgeEvent>, StageError> {
        let Some((tool, args)) = self.arguments(key) else {
            return Ok(None);
        };
        let tool = tool.to_owned();
        let args = args.clone();
        let session = self.context.session;
        if tool == "report_stage_outcome" {
            let parsed = serde_json::from_value::<surge_core::stage_tool::StageOutcomeCandidate>(
                args.clone(),
            );
            let candidate = match parsed {
                Ok(candidate) if declared.contains(&candidate.outcome) => candidate,
                _ => {
                    self.finish(
                        writer,
                        key,
                        StageToolResult::Rejected {
                            tool,
                            reason: "invalid or undeclared outcome candidate".into(),
                        },
                    )
                    .await?;
                    return Ok(None);
                },
            };
            self.finish(
                writer,
                key,
                StageToolResult::OutcomeCandidate {
                    candidate: candidate.clone(),
                },
            )
            .await?;
            return Ok(Some(BridgeEvent::OutcomeReported {
                session,
                outcome: candidate.outcome,
                summary: candidate.summary,
                artifacts_produced: candidate.artifacts_produced,
                verification_report: candidate.verification_report.map(Box::new),
            }));
        }
        if tool == "request_human_input" {
            let Some(question) = args
                .get("question")
                .and_then(|value| value.as_str())
                .filter(|s| !s.trim().is_empty())
            else {
                self.finish(
                    writer,
                    key,
                    StageToolResult::Rejected {
                        tool,
                        reason: "question must be nonempty".into(),
                    },
                )
                .await?;
                return Ok(None);
            };
            return Ok(Some(BridgeEvent::HumanInputRequested {
                session,
                call_id: key.to_owned(),
                question: question.to_owned(),
                context: args
                    .get("context")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned),
            }));
        }
        self.finish(
            writer,
            key,
            StageToolResult::Rejected {
                tool,
                reason: "tool is not exposed by stage MCP v1".into(),
            },
        )
        .await?;
        Ok(None)
    }

    pub fn arguments(&self, key: &str) -> Option<(&str, &serde_json::Value)> {
        self.pending
            .get(key)
            .map(|pending| (pending.call.tool.as_str(), &pending.call.arguments))
    }

    pub async fn disconnected(&mut self, key: &str) {
        if let Some(pending) = self.pending.get_mut(key) {
            pending.call.reply.closed().await;
        } else {
            std::future::pending::<()>().await;
        }
    }

    pub async fn finish(
        &mut self,
        writer: &RunWriter,
        key: &str,
        result: StageToolResult,
    ) -> Result<(), StageError> {
        let pending = self
            .pending
            .remove(key)
            .ok_or_else(|| StageError::Bridge("missing authenticated call".into()))?;
        let receipt = StageToolReceipt {
            context: self.context.clone(),
            call_id: pending.id,
            arguments_hash: pending.hash,
            result,
        };
        writer
            .append_event(VersionedEventPayload::new(EventPayload::StageToolReceipt {
                receipt: receipt.clone(),
            }))
            .await
            .map_err(|error| StageError::Storage(error.to_string()))?;
        let response = receipt_reply(&receipt);
        self.receipts.insert(receipt.call_id.clone(), receipt);
        let _ = pending.call.reply.send(response);
        Ok(())
    }

    pub async fn reply(
        &mut self,
        writer: &RunWriter,
        bridge: &dyn surge_acp::bridge::facade::BridgeFacade,
        session: surge_core::SessionId,
        key: String,
        payload: ToolResultPayload,
    ) -> Result<(), StageError> {
        let Some(pending) = self.pending.get(&key) else {
            return bridge
                .reply_to_tool(session, key, payload)
                .await
                .map_err(|e| StageError::Bridge(format!("reply_to_tool: {e}")));
        };
        let tool = pending.call.tool.clone();
        let result = match payload {
            ToolResultPayload::Ok { result_json } => {
                let output = serde_json::from_str(&result_json)
                    .map_err(|e| StageError::Bridge(format!("tool result JSON: {e}")))?;
                if tool == "request_human_input" {
                    StageToolResult::HumanResponse { response: output }
                } else {
                    StageToolResult::Rejected {
                        tool,
                        reason: "tool is not exposed by stage MCP v1".into(),
                    }
                }
            },
            ToolResultPayload::Error { message } => StageToolResult::Rejected {
                tool,
                reason: message,
            },
            ToolResultPayload::Unsupported => StageToolResult::Rejected {
                tool,
                reason: "unsupported tool".into(),
            },
        };
        self.finish(writer, &key, result).await
    }
}

fn receipt_reply(receipt: &StageToolReceipt) -> StageToolReply {
    let value = match &receipt.result {
        StageToolResult::OutcomeCandidate { .. } => {
            serde_json::json!({"status":"candidate received; final validation pending","receipt":receipt})
        },
        StageToolResult::HumanResponse { response } => response.clone(),
        StageToolResult::Rejected { reason, .. } => serde_json::json!({"error":reason}),
    };
    StageToolReply {
        is_error: matches!(receipt.result, StageToolResult::Rejected { .. }),
        value,
    }
}

pub(super) fn prepare(
    config: &mut surge_acp::bridge::session::SessionConfig,
    context: StageToolContext,
) -> Result<
    (
        surge_mcp::stage::StageEndpoint,
        tokio::sync::mpsc::Receiver<StageToolCall>,
        StageCalls,
    ),
    StageError,
> {
    let catalog = config
        .stage_tools()
        .into_iter()
        .map(|tool| surge_mcp::stage::StageToolDefinition {
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
        })
        .collect();
    let (endpoint, receiver) = surge_mcp::stage::StageEndpoint::open(context.clone(), catalog)
        .map_err(|error| StageError::Bridge(error.to_string()))?;
    config.stage_mcp = Some(Box::new(surge_acp::bridge::session::StageMcpConfig {
        session: context.session,
        server: surge_acp::bridge::McpServerStdio::new("surge-stage", helper_path()?)
            .args(vec!["internal-stage-mcp".into()])
            .env(vec![
                surge_acp::bridge::EnvVariable::new(
                    surge_mcp::stage::AUTH_ENV,
                    endpoint.credential(),
                ),
                surge_acp::bridge::EnvVariable::new(
                    surge_mcp::stage::ENDPOINT_ENV,
                    endpoint.address(),
                ),
            ]),
    }));
    Ok((endpoint, receiver, StageCalls::new(context)))
}

fn helper_path() -> Result<std::path::PathBuf, StageError> {
    let current = std::env::current_exe()
        .map_err(|error| StageError::Bridge(format!("locate stage helper: {error}")))?;
    let parent = current
        .parent()
        .ok_or_else(|| StageError::Bridge("stage helper executable has no parent".into()))?;
    let directory = if parent.file_name().is_some_and(|name| name == "deps") {
        parent
            .parent()
            .ok_or_else(|| StageError::Bridge("stage helper build directory missing".into()))?
    } else {
        parent
    };
    Ok(directory.join(if cfg!(windows) { "surge.exe" } else { "surge" }))
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;
    use std::collections::BTreeMap;
    use surge_acp::bridge::{
        sandbox::AlwaysAllowSandbox,
        session::{AgentKind, SessionConfig},
    };
    use surge_core::id::{RunId, SessionId, StageGenerationId};

    #[tokio::test]
    async fn descriptor_is_self_contained_without_provider_environment_secrets() {
        let mut config = SessionConfig {
            writer_id: surge_core::id::ExecutionWriterId::new(),
            invocation: surge_core::id::StageInvocationId::new(),
            runtime: "fixture".into(),
            opening: surge_core::execution_recovery::SessionOpening::default(),
            config_selections: Vec::new(),
            stage_mcp: None,
            agent_kind: AgentKind::Mock { args: vec![] },
            working_dir: std::env::temp_dir(),
            system_prompt: String::new(),
            declared_outcomes: vec!["done".parse().unwrap()],
            allows_escalation: true,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: surge_acp::client::PermissionPolicy::default(),
            bindings: BTreeMap::new(),
            env: BTreeMap::new(),
        };
        let context = StageToolContext {
            run: RunId::new(),
            node: "agent".parse().unwrap(),
            session: SessionId::new(),
            generation: StageGenerationId::new(),
        };
        let (endpoint, _receiver, _calls) = prepare(&mut config, context).unwrap();
        let stage = config.stage_mcp.as_ref().unwrap();
        let env: BTreeMap<_, _> = stage
            .server
            .env
            .iter()
            .map(|entry| (entry.name.as_str(), entry.value.as_str()))
            .collect();
        assert_eq!(env.len(), 2);
        assert_eq!(env[surge_mcp::stage::AUTH_ENV], endpoint.credential());
        assert_eq!(env[surge_mcp::stage::ENDPOINT_ENV], endpoint.address());
        assert!(!config.env.contains_key(surge_mcp::stage::AUTH_ENV));
        assert!(!config.env.contains_key(surge_mcp::stage::ENDPOINT_ENV));
        assert_eq!(stage.server.command, helper_path().unwrap());
        assert_eq!(stage.server.args, ["internal-stage-mcp"]);
        assert!(!format!("{stage:?}").contains(endpoint.credential()));
        endpoint.close().await.unwrap();
    }
}

#[cfg(test)]
mod verification_transport_tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reports_keep_candidate_identity_and_changed_retry_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        let storage = surge_persistence::runs::Storage::open(home.path())
            .await
            .unwrap();
        let run = surge_core::RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        let context = StageToolContext {
            run,
            node: "verify".parse().unwrap(),
            session: surge_core::SessionId::new(),
            generation: surge_core::id::StageGenerationId::new(),
        };
        let mut calls = StageCalls::new(context.clone());
        for (call_id, summary) in [("first", "first candidate"), ("second", "second candidate")] {
            let args = serde_json::json!({"call_id":call_id,"outcome":"passed","summary":summary,"verification_report":{"schema_version":1,"task_id":"t1","outcome":"passed","summary":summary,"checks":[{"command":"test","result":"passed","covers":["criterion:1"]}]}});
            let (reply, receive) = tokio::sync::oneshot::channel();
            let key = calls
                .admit(StageToolCall {
                    context: context.clone(),
                    tool: "report_stage_outcome".into(),
                    arguments: args.clone(),
                    reply,
                })
                .unwrap();
            let event = calls
                .event(&writer, &key, &["passed".parse().unwrap()])
                .await
                .unwrap()
                .unwrap();
            let BridgeEvent::OutcomeReported {
                verification_report: Some(report),
                ..
            } = event
            else {
                panic!("candidate report lost")
            };
            assert_eq!(report.summary, summary);
            assert!(!receive.await.unwrap().is_error);
            let (reply, receive) = tokio::sync::oneshot::channel();
            assert!(
                calls
                    .admit(StageToolCall {
                        context: context.clone(),
                        tool: "report_stage_outcome".into(),
                        arguments: args.clone(),
                        reply
                    })
                    .is_none()
            );
            assert!(
                !receive.await.unwrap().is_error,
                "identical retry returns immutable receipt"
            );
            let mut changed = args;
            changed["verification_report"]["summary"] = serde_json::json!("different report");
            let (reply, receive) = tokio::sync::oneshot::channel();
            assert!(
                calls
                    .admit(StageToolCall {
                        context: context.clone(),
                        tool: "report_stage_outcome".into(),
                        arguments: changed,
                        reply
                    })
                    .is_none()
            );
            assert!(
                receive.await.unwrap().is_error,
                "same call ID cannot replace its report"
            );
        }
    }
}
