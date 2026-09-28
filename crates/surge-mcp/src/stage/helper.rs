//! stdio MCP facade. Every call crosses the authenticated local channel.

use super::transport::{AUTH_ENV, ENDPOINT_ENV, Operation, Request, read_frame, write_frame};
use super::{StageToolDefinition, StageToolReply, StageTransportError};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use tokio::io::AsyncReadExt;

struct Helper {
    address: String,
    auth: String,
    tools: Vec<Tool>,
}

impl ServerHandler for Helper {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult {
            tools: self.tools.clone(),
            ..Default::default()
        }))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation = Operation::Call {
            tool: request.name.into_owned(),
            arguments: serde_json::Value::Object(request.arguments.unwrap_or_default()),
        };
        let cancellation = context
            .extensions
            .get::<super::bounded_stdio::RequestCancellation>()
            .map_or_else(|| context.ct.clone(), |token| token.0.clone());
        let result: Result<StageToolReply, StageTransportError> = tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(StageTransportError::Rejected("request cancelled".into())),
            result = self.request(operation) => result,
        };
        let reply = result.unwrap_or_else(|_| {
            StageToolReply::rejected("stage connection ended before a durable reply")
        });
        let blocks = vec![Content::text(reply.value.to_string())];
        Ok(if reply.is_error {
            CallToolResult::error(blocks)
        } else {
            CallToolResult::success(blocks)
        })
    }
}

impl Helper {
    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        operation: Operation,
    ) -> Result<T, StageTransportError> {
        let mut connection = super::local::connect(&self.address).await?;
        write_frame(
            &mut connection,
            &Request {
                auth: self.auth.clone(),
                operation,
            },
        )
        .await?;
        read_frame(&mut connection).await
    }
}

/// Serve the hidden `surge` helper command. Stdout is reserved for MCP frames.
///
/// Credentials are inherited environment values, never command-line arguments.
/// Endpoint revocation also stops an idle stdio helper whose provider stays alive.
pub async fn serve_stdio() -> Result<(), StageTransportError> {
    let address = std::env::var(ENDPOINT_ENV)
        .map_err(|_| StageTransportError::Rejected("missing stage endpoint".into()))?;
    let auth = std::env::var(AUTH_ENV)
        .map_err(|_| StageTransportError::Rejected("missing stage authentication".into()))?;
    let mut helper = Helper {
        address,
        auth,
        tools: vec![],
    };
    let catalog: Vec<StageToolDefinition> = helper.request(Operation::Catalog).await?;
    for definition in catalog {
        let schema = definition
            .input_schema
            .as_object()
            .ok_or_else(|| StageTransportError::Rejected("invalid catalog schema".into()))?
            .clone();
        helper
            .tools
            .push(Tool::new(definition.name, definition.description, schema));
    }
    let mut monitor = super::local::connect(&helper.address).await?;
    write_frame(
        &mut monitor,
        &Request {
            auth: helper.auth.clone(),
            operation: Operation::Monitor,
        },
    )
    .await?;
    let _: bool = read_frame(&mut monitor).await?;
    let mut byte = [0];
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = super::bounded_stdio::BoundedStdio::new(
        tokio::io::stdin(),
        tokio::io::stdout(),
        failed.clone(),
    );
    let transport = rmcp::transport::WorkerTransport::spawn(worker);
    let transport_cancel = transport.cancel_token();
    let service = tokio::select! {
        _ = monitor.read(&mut byte) => return Ok(()),
        service = helper.serve(transport) => service.map_err(|_| StageTransportError::Rejected("MCP initialization failed".into()))?,
    };
    let cancel = service.cancellation_token();
    let wait = service.waiting();
    tokio::pin!(wait);
    tokio::select! {
        result = &mut wait => { result.map_err(|_| StageTransportError::CleanupUnconfirmed)?; },
        _ = monitor.read(&mut byte) => {
            transport_cancel.cancel();
            cancel.cancel();
            tokio::time::timeout(std::time::Duration::from_secs(2), &mut wait).await
                .map_err(|_| StageTransportError::CleanupUnconfirmed)?
                .map_err(|_| StageTransportError::CleanupUnconfirmed)?;
        },
    }
    if failed.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(StageTransportError::Rejected(
            "stage stdio transport failed".into(),
        ));
    }
    Ok(())
}
