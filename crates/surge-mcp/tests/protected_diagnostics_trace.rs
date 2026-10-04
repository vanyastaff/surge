//! Real SDK descendant-task diagnostics under the application-global metadata veto.
#![cfg(unix)]

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use surge_mcp::{McpServerConnection, stderr_log_path};
use tracing_subscriber::layer::SubscriberExt;

#[path = "common/protected_fixture.rs"]
mod protected_fixture;
use protected_fixture::{CHILD, Fixture};

#[derive(Default)]
struct PublicCapture {
    bytes: Vec<u8>,
    overflow: bool,
}

#[derive(Clone)]
struct CaptureWriter(Arc<Mutex<PublicCapture>>);

impl Write for CaptureWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut capture = self.0.lock().expect("capture lock");
        const LIMIT: usize = 256 * 1024;
        let available = LIMIT.saturating_sub(capture.bytes.len());
        capture
            .bytes
            .extend_from_slice(&bytes[..bytes.len().min(available)]);
        capture.overflow |= bytes.len() > available;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn actual_child_global_trace_contains_only_public_categories() {
    let capture = Arc::new(Mutex::new(PublicCapture::default()));
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::filter::filter_fn(|metadata| {
            surge_mcp::diagnostics::permits_target(metadata.target())
        }))
        .with(tracing_subscriber::EnvFilter::new(
            "trace,rmcp=trace,process_wrap=trace",
        ))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(CaptureWriter(capture.clone())),
        );
    tracing::subscriber::set_global_default(subscriber)
        .expect("isolated process-global subscriber");
    // Installation precedes every runtime and SDK descendant task in this binary.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let fixture = Fixture::new("trace-oracle");
    let script = CHILD.replace(
        "    method = msg['method']",
        "    method = msg['method']\n    if method == 'tools/call' and msg['params']['name'] == 'malformed':\n        with open(os.environ['RECORDER'] + '.malformed', 'w') as f:\n            f.write('actual malformed protocol dispatched')\n        print(json.dumps({'private': private}), flush=True)\n        print('malformed:' + private[-1], flush=True)\n        continue",
    ).replace(
        "    print(json.dumps({'jsonrpc': '2.0', 'id': msg['id'], 'result': result}), flush=True)",
        "    print(json.dumps({'jsonrpc': '2.0', 'id': msg['id'], 'result': result}), flush=True)\n    if method == 'initialize':\n        print(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/message', 'params': {'level': 'info', 'logger': private[-1], 'data': private}}), flush=True)",
    );
    std::fs::write(fixture.dir.path().join("child.py"), script).expect("trace child script");
    runtime.block_on(async {
        let conn = McpServerConnection::new(fixture.config.clone(), Some(fixture.dir.path().to_owned()));
        conn.list_tools().await.expect("actual SDK initialize and peer metadata");
        fixture.verify_transport();
        let success = conn.call_tool("success", serde_json::json!({})).await.expect("functional response");
        assert_eq!(serde_json::to_value(&success).expect("functional JSON"), serde_json::json!({"isError": false, "content": [{"type": "text", "text": "functional exact Ω\nline"}], "structuredContent": {"value": 7}, "_meta": {"public": "exact"}}));
        let error = conn.call_tool("error", serde_json::json!({})).await.expect("error envelope");
        fixture.assert_opaque(&format!("{error:?}"));
        let rpc_error = conn.call_tool("rpc_error", serde_json::json!({})).await.expect_err("RPC error");
        fixture.assert_opaque(&format!("{rpc_error} {rpc_error:?}"));
        // Actual malformed JSON and unrecognized protocol input exercise codec
        // diagnostics. Bound this observation separately from the server deadline.
        let _ = tokio::time::timeout(Duration::from_secs(1), conn.call_tool("malformed", serde_json::json!({}))).await;
        assert!(fixture.dir.path().join("private-recorder.json.malformed").exists(), "malformed protocol path never reached child");
        let cleanup = conn.shutdown().await;
        fixture.assert_opaque(&format!("{cleanup:?}"));
        let path = stderr_log_path(Some(fixture.dir.path()), "trace-oracle");
        let tee = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(tee) = tokio::fs::read_to_string(&path).await
                    && tee.lines().count() >= 7 { break tee; }
                tokio::task::yield_now().await;
            }
        }).await.expect("actual tee");
        fixture.assert_opaque(&tee);
    });
    let observed = capture.lock().expect("capture lock");
    assert!(!observed.overflow, "public trace capture overflowed");
    let text = String::from_utf8_lossy(&observed.bytes);
    fixture.assert_opaque(&text);
    assert!(
        text.contains("mcp_stderr_record"),
        "safe Surge stderr category disappeared"
    );
    assert!(
        !text.contains("rmcp::"),
        "dependency diagnostic namespace bypassed veto"
    );
    assert!(
        !text.contains("process_wrap::"),
        "process-wrapper diagnostic namespace bypassed veto"
    );
}
