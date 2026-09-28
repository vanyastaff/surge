//! Fixed protocol-v1 JSON oracle, independent of the controlled SDK agent.
use agent_client_protocol::schema::v1 as acp;
use serde_json::{Value, json};

fn roundtrip<T: serde::de::DeserializeOwned + serde::Serialize>(wire: Value) -> T {
    let decoded: T = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), wire);
    decoded
}

#[test]
fn fixed_v1_handshake_and_stdio_descriptor() {
    let initialize: acp::InitializeRequest = roundtrip(json!({
        "protocolVersion":1,
        "clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":true,"auth":{"terminal":false}},
        "clientInfo":{"name":"surge","title":"Surge","version":"test"}
    }));
    assert_eq!(
        initialize.protocol_version,
        agent_client_protocol::schema::ProtocolVersion::V1
    );
    let session: acp::NewSessionRequest = roundtrip(json!({
        "cwd":"/workspace",
        "mcpServers":[{"name":"surge-stage","command":"/bin/surge","args":["internal-stage-mcp"],"env":[]}]
    }));
    let acp::McpServer::Stdio(server) = &session.mcp_servers[0] else {
        panic!("stdio descriptor changed")
    };
    assert_eq!(server.name, "surge-stage");
}

#[test]
fn fixed_v1_permission_cancel_and_prompt() {
    let _: acp::RequestPermissionRequest = roundtrip(json!({
        "sessionId":"session-a",
        "toolCall":{"toolCallId":"permission-a","title":"Write file","status":"pending"},
        "options":[{"optionId":"allow","name":"Allow once","kind":"allow_once"}]
    }));
    let _: acp::RequestPermissionResponse =
        roundtrip(json!({"outcome":{"outcome":"selected","optionId":"allow"}}));
    let _: acp::CancelNotification = roundtrip(json!({"sessionId":"session-a"}));
    let _: acp::PromptRequest = roundtrip(
        json!({"sessionId":"session-a","prompt":[{"type":"text","text":"exact prompt\nsecond line"}]}),
    );
    let _: acp::PromptResponse = roundtrip(json!({"stopReason":"end_turn"}));
}

#[test]
fn fixed_v1_observation_and_error_classification_input() {
    let _: acp::SessionNotification = roundtrip(json!({
        "sessionId":"session-a",
        "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}
    }));
    let auth: acp::Error = roundtrip(
        json!({"code":-32000,"message":"Authentication required","data":{"reason":"auth_required"}}),
    );
    assert_eq!(auth.message, "Authentication required");
}
