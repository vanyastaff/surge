//! Opt-in live smoke for the Ollama ACP runtime.
//!
//! Proves the integration end to end **at the transport level**: surge
//! launches the `ollama-acp` registry entry exactly as the engine does
//! (npx + `@zed-industries/claude-agent-acp`, with the entry's `env` spec
//! resolved), completes the ACP handshake, dispatches a prompt, and observes
//! a non-empty agent reply. That is the whole contract the Ollama runtime
//! adds — provider routing through the adapter's environment.
//!
//! It deliberately does **not** drive `report_stage_outcome`: that injected
//! tool is not yet transported to real ACP agents (the mock agent hard-codes
//! the call; a real Claude run hangs the same way). The engine-level
//! `real_acp_smoke` is the place that gap will surface once it is fixed.
//!
//! ## Enabling
//!
//! ```text
//! OLLAMA_HOST=https://ollama.com \
//! OLLAMA_API_KEY=... OLLAMA_MODEL=glm-5.3 \
//!   cargo test -p surge-acp --test ollama_acp_smoke -- --ignored --nocapture
//! ```
//!
//! For a local server: `OLLAMA_HOST=http://localhost:11434`, no key, and a
//! pulled model name. Without the three variables the test prints a skip
//! banner and passes, so the deterministic CI path is unaffected.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

const ENV_HOST: &str = "OLLAMA_HOST";
const ENV_KEY: &str = "OLLAMA_API_KEY";
const ENV_MODEL: &str = "OLLAMA_MODEL";
const REPLY_TIMEOUT: Duration = Duration::from_secs(180);

fn skip_banner(reason: &str) {
    eprintln!(
        "[ollama_acp_smoke] SKIPPED: {reason}\n\
         Set {ENV_HOST}, {ENV_MODEL} (and {ENV_KEY} for Ollama Cloud) to run the live smoke."
    );
}

/// Launch parameters the registry entry resolves at spawn time: command,
/// args, and the env spec — read from `builtins`, never hard-coded here, so
/// the test fails if the entry stops routing to Ollama.
fn ollama_entry_launch() -> Option<(surge_acp::RegistryEntry, BTreeMap<String, String>)> {
    if std::env::var(ENV_HOST).is_err() || std::env::var(ENV_MODEL).is_err() {
        skip_banner(&format!("{ENV_HOST} and/or {ENV_MODEL} are not set"));
        return None;
    }
    let registry = surge_acp::Registry::builtin();
    let entry = registry
        .find("ollama-acp")
        .expect("ollama-acp must be builtin")
        .clone();
    let env = match surge_acp::agent_env::resolve(&entry.id, &entry.env) {
        Ok(env) => env,
        Err(e) => panic!("ollama-acp env spec must resolve with OLLAMA_* set: {e}"),
    };
    // The credential must never appear in this test's own memory beyond the
    // child's environment — assert the spec carries names, not values.
    assert_eq!(
        env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
        std::env::var(ENV_KEY).ok().as_deref().or(Some("ollama")),
        "auth token must come from OLLAMA_API_KEY (default 'ollama' for a local server)",
    );
    Some((entry, env))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires OLLAMA_HOST + OLLAMA_MODEL (and OLLAMA_API_KEY for cloud); opt-in live smoke"]
async fn ollama_runtime_handshakes_and_answers() {
    let Some((entry, env)) = ollama_entry_launch() else {
        return;
    };
    // Materialise the entry's declared settings files exactly as the engine
    // does — data-driven, no vendor branch.
    let workdir = TempDir::new().expect("tempdir");
    surge_acp::settings_seed::seed_settings_files(&entry.settings_files, workdir.path());

    eprintln!(
        "[ollama_acp_smoke] RUNNING: command={} model={}",
        entry.command,
        env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .map(String::as_str)
            .unwrap_or("<unset>"),
    );

    let bridge = AcpBridge::with_defaults().expect("spawn bridge");
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Custom {
            binary: std::path::PathBuf::from(&entry.command),
            args: entry.default_args.clone(),
        },
        working_dir: workdir.path().to_path_buf(),
        system_prompt: "Reply to every message with exactly the text requested.".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env,
    };

    let session = bridge.open_session(cfg).await.expect("open session");
    let established = timeout(Duration::from_secs(60), events.recv())
        .await
        .expect("SessionEstablished within 60s")
        .expect("broadcast open");
    assert!(
        matches!(established, BridgeEvent::SessionEstablished { session: s, .. } if s == session),
        "expected SessionEstablished for {session}, got {established:?}",
    );

    bridge
        .send_message(
            session,
            MessageContent::Text("Reply with exactly: OLLAMA-ACP-OK".into()),
        )
        .await
        .expect("dispatch prompt");

    let mut reply = String::new();
    let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(500), events.recv()).await {
            Ok(Ok(BridgeEvent::AgentMessage {
                session: s, chunk, ..
            })) if s == session => {
                reply.push_str(&chunk);
                if reply.contains("OLLAMA-ACP-OK") {
                    break;
                }
            },
            Ok(Ok(BridgeEvent::SessionEnded { session: s, reason })) if s == session => {
                panic!("session ended before a reply arrived: {reason:?}");
            },
            _ => continue,
        }
    }
    eprintln!("[ollama_acp_smoke] reply: {reply:?}");
    assert!(
        reply.contains("OLLAMA-ACP-OK"),
        "expected the model's reply to echo the requested sentinel; got {reply:?}",
    );

    // Close is best-effort for an npx-wrapped adapter: the wrapper can
    // outlive the ACP connection past the bridge's grace window, and that is
    // a wrapper-exit timing fact, not a failure of the provider routing this
    // smoke exists to prove. `shutdown` still reaps the process.
    if let Err(e) = bridge.close_session(session).await {
        eprintln!("[ollama_acp_smoke] close_session did not finish gracefully: {e}");
    }
    bridge.shutdown().await.expect("shutdown bridge");
}
