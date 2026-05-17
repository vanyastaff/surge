//! U7 — `surge mcp` IPC contract: the `McpProbe` / `McpLogs` verbs are
//! wire-additive *and* present in the exhaustive `request_id()` matches
//! on both `DaemonRequest` and `DaemonResponse`. Those matches have no
//! wildcard, so a missing arm is a compile error; these tests pin the
//! runtime behaviour and the newline-framed round trip.

use surge_orchestrator::engine::ipc::{
    DaemonRequest, DaemonResponse, McpProbeReport, read_request_frame, read_response_frame,
    write_frame,
};
use tokio::io::BufReader;

#[tokio::test]
async fn mcp_probe_request_frames_round_trip_and_carry_request_id() {
    let cases = [
        DaemonRequest::McpProbe {
            request_id: 7,
            name: None,
        },
        DaemonRequest::McpProbe {
            request_id: 8,
            name: Some("filesystem".into()),
        },
        DaemonRequest::McpLogs {
            request_id: 9,
            name: "filesystem".into(),
            tail: Some(50),
        },
    ];
    for req in cases {
        let rid = req.request_id();
        let (mut tx, rx) = tokio::io::duplex(16 * 1024);
        write_frame(&mut tx, &req).await.expect("write frame");
        let mut reader = BufReader::new(rx);
        let decoded = read_request_frame(&mut reader)
            .await
            .expect("read frame")
            .expect("a frame");
        assert_eq!(
            decoded.request_id(),
            rid,
            "request_id() arm missing for a new verb"
        );
        assert_eq!(
            serde_json::to_value(&req).unwrap()["method"],
            serde_json::to_value(&decoded).unwrap()["method"],
            "tagged method discriminant must survive the round trip"
        );
    }
}

#[tokio::test]
async fn mcp_response_frames_round_trip_and_carry_request_id() {
    let cases = [
        DaemonResponse::McpProbeOk {
            request_id: 11,
            servers: vec![McpProbeReport::new(
                "filesystem".into(),
                "healthy".into(),
                Some(5),
                None,
            )],
        },
        DaemonResponse::McpLogsOk {
            request_id: 12,
            server: "filesystem".into(),
            scope: "probe".into(),
            lines: vec!["INFO listening".into()],
        },
    ];
    for resp in cases {
        let rid = resp.request_id();
        let (mut tx, rx) = tokio::io::duplex(16 * 1024);
        write_frame(&mut tx, &resp).await.expect("write frame");
        let mut reader = BufReader::new(rx);
        let decoded = read_response_frame(&mut reader)
            .await
            .expect("read frame")
            .expect("a frame");
        assert_eq!(
            decoded.request_id(),
            rid,
            "response request_id() arm missing for a new verb"
        );
    }
}
