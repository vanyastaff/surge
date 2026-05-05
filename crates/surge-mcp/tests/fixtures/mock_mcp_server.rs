//! Minimal stdio MCP server fixture for surge-mcp integration tests.
//! Declares two tools: `echo` (returns the input) and `crash_now`
//! (exits the process). Built only with `--features mock-server`.

#[cfg(not(feature = "mock-server"))]
fn main() {
    panic!("build with --features mock-server")
}

#[cfg(feature = "mock-server")]
fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async_main());
}

#[cfg(feature = "mock-server")]
async fn async_main() {
    use rmcp::{
        ServerHandler, ServiceExt,
        handler::server::{router::tool::ToolRouter, wrapper::Parameters},
        model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
        schemars, tool, tool_handler, tool_router,
        transport::io::stdio,
    };

    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    struct EchoArgs {
        text: String,
    }

    #[derive(Clone)]
    struct Mock {
        #[allow(dead_code)]
        tool_router: ToolRouter<Self>,
    }

    #[tool_router]
    impl Mock {
        fn new() -> Self {
            Self {
                tool_router: Self::tool_router(),
            }
        }

        #[tool(description = "Echo the input text")]
        async fn echo(
            &self,
            Parameters(EchoArgs { text }): Parameters<EchoArgs>,
        ) -> CallToolResult {
            CallToolResult::success(vec![Content::text(text)])
        }

        #[tool(description = "Crash the server immediately")]
        async fn crash_now(&self) -> CallToolResult {
            std::process::exit(1);
        }
    }

    #[tool_handler]
    impl ServerHandler for Mock {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }
    }

    let (read, write) = stdio();
    let server = Mock::new();
    let service = server
        .serve((read, write))
        .await
        .expect("serve mock server");
    service.waiting().await.expect("mock server wait");
}
