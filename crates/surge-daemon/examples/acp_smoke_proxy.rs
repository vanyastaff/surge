//! Test-only transparent ACP exact-model witness. Never installed as a runtime.
#[path = "../tests/support/live_provider_capture.rs"]
mod capture;

use std::{io, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, BufReader};

const MAX_FRAME: usize = 1024 * 1024;

fn main() {
    if fixture_provider() {
        return;
    }
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| "proxy runtime unavailable")
        .and_then(|runtime| runtime.block_on(run()));
    if matches!(result, Err("capture unavailable" | "capture write")) {
        // Fixed classification only; no diagnostic payload or provider stderr.
        let _ = std::io::Write::write_all(&mut std::io::stderr().lock(), b"capture_unavailable\n");
    }
    if result.is_err() {
        // Deliberately do not print error payloads, command arguments or provider stderr.
        std::process::exit(1);
    }
}

async fn frame<R: AsyncBufRead + Unpin>(reader: &mut R, bytes: &mut Vec<u8>) -> io::Result<usize> {
    let limit = MAX_FRAME.saturating_add(1).saturating_sub(bytes.len());
    let read = reader
        .take(u64::try_from(limit).map_err(io::Error::other)?)
        .read_until(b'\n', bytes)
        .await?;
    if bytes.len() > MAX_FRAME || (read != 0 && bytes.last() != Some(&b'\n')) {
        return Err(io::Error::other("ACP frame limit"));
    }
    Ok(read)
}

struct ProxyArgs {
    capture_path: PathBuf,
    required_model: String,
    mode: capture::SelectionMode,
    native: bool,
    program: std::ffi::OsString,
    provider_args: Vec<std::ffi::OsString>,
}
fn native_option<I: Iterator<Item = std::ffi::OsString>>(
    args: &mut std::iter::Peekable<I>,
    native: &mut bool,
) -> Result<(), &'static str> {
    while args.peek().is_some_and(|arg| arg == "--native-multi-turn") {
        if *native {
            return Err("duplicate proxy option");
        }
        *native = true;
        args.next();
    }
    Ok(())
}
fn proxy_args(args: impl Iterator<Item = std::ffi::OsString>) -> Result<ProxyArgs, &'static str> {
    let mut args = args.peekable();
    let mut native = false;
    native_option(&mut args, &mut native)?;
    let first = args.next().ok_or("missing capture destination")?;
    let capture_path = if first == "--capture-dir" {
        PathBuf::from(args.next().ok_or("missing capture directory")?)
            .join(format!("{}.json", surge_core::RunId::new()))
    } else {
        PathBuf::from(&first)
    };
    if first
        .to_str()
        .is_some_and(|arg| arg.starts_with("--") && arg != "--capture-dir")
    {
        return Err("unknown proxy option");
    }
    native_option(&mut args, &mut native)?;
    let next = args.next().ok_or("missing model")?;
    let (mode, model) = if next == "--initial-provider" {
        (
            capture::SelectionMode::InitialProvider,
            args.next().ok_or("missing model")?,
        )
    } else {
        (capture::SelectionMode::ExplicitSetAck, next)
    };
    let required_model = model.into_string().map_err(|_| "model must be UTF-8")?;
    if required_model.starts_with("--") {
        return Err("unknown proxy option");
    }
    native_option(&mut args, &mut native)?;
    let program = args.next().ok_or("missing program")?;
    if program.to_str().is_some_and(|arg| arg.starts_with("--")) {
        return Err("unknown proxy option");
    }
    Ok(ProxyArgs {
        capture_path,
        required_model,
        mode,
        native,
        program,
        provider_args: args.collect(),
    })
}
fn publish(
    evidence: &mut capture::EvidenceFile,
    snapshot: &capture::Capture,
    fail_rejection_publication: bool,
) -> Result<(), &'static str> {
    // Explicit deterministic fixture injection. Ordinary invocations always use
    // the actual retained EvidenceFile; a failed publication never forwards.
    if fail_rejection_publication && snapshot.rejection.is_some() {
        return Err("capture unavailable");
    }
    evidence.write(snapshot)
}
async fn run() -> Result<(), &'static str> {
    let mut supplied = std::env::args_os().skip(1).peekable();
    let fail_rejection_publication = supplied
        .peek()
        .is_some_and(|arg| arg == "--fixture-rejection-publication-failure");
    if fail_rejection_publication {
        supplied.next();
    }
    let args = proxy_args(supplied)?;
    let mut command = std::process::Command::new(args.program);
    command
        .args(args.provider_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut witness = if args.native {
        capture::ProtocolCapture::native(args.required_model, args.mode)?
    } else {
        capture::ProtocolCapture::with_mode(args.required_model, args.mode)?
    };
    let mut evidence = capture::EvidenceFile::create(&args.capture_path)?;
    publish(&mut evidence, &witness.snapshot, fail_rejection_publication)?;
    let mut child = capture::OwnedProcess::spawn(command, false).map_err(|_| "provider spawn")?;
    if child.id() == 0 {
        return Err("provider identity unavailable");
    }
    #[cfg(windows)]
    child.container().map_err(|_| "provider Job observation")?;
    let (mut input, output, mut error) = child.pipes().map_err(|_| "provider pipes")?;
    let stderr =
        tokio::spawn(async move { tokio::io::copy(&mut error, &mut tokio::io::sink()).await });
    let (stdin, mut receiver) = input_reader();
    let mut output = BufReader::new(output);
    let mut bytes = Vec::new();
    let mut stdout = tokio::io::stdout();
    let result = loop {
        tokio::select! {
            incoming = receiver.recv() => {
                let Some(incoming) = incoming else {
                    let ended = witness.eof();
                    if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture unavailable"); }
                    break ended;
                };
                let value = match serde_json::from_slice(&incoming) {
                    Ok(value) => value,
                    Err(_) => {
                        witness.framing_failure(capture::Direction::Client);
                        if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture unavailable"); }
                        break Err("client framing");
                    }
                };
                if witness.client(&value).is_err() {
                    if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture unavailable"); }
                    break Err("model proof unavailable");
                }
                if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture write"); }
                if capture::forward(&mut input, &incoming).await.is_err() { break Err("provider input"); }
            },
            read = frame(&mut output, &mut bytes) => {
                match read {
                    Ok(0) => {
                        let ended = witness.eof();
                        if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture unavailable"); }
                        break ended;
                    },
                    Err(_) => {
                        witness.framing_failure(capture::Direction::Provider);
                        if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture unavailable"); }
                        break Err("provider framing");
                    }, _ => {}
                }
                let value = match serde_json::from_slice(&bytes) {
                    Ok(value) => value,
                    Err(_) => {
                        witness.framing_failure(capture::Direction::Provider);
                        if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture unavailable"); }
                        break Err("provider framing");
                    }
                };
                if witness.provider(&value).is_err() {
                    if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture unavailable"); }
                    break Err("provider metadata");
                }
                if publish(&mut evidence, &witness.snapshot, fail_rejection_publication).is_err() { break Err("capture write"); }
                if capture::forward(&mut stdout, &bytes).await.is_err() { break Err("client output"); }
                bytes.clear();
            }
        }
    };
    drop(input);
    child.terminate().map_err(|_| "provider terminate")?;
    child
        .wait(Duration::from_secs(3))
        .await
        .map_err(|_| "provider settlement")?;
    // Descendants inherit the host group/Job. Held descendant pipes keep this
    // pending until the retaining bridge/watchdog owner settles that container.
    let drained = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::io::copy(&mut output, &mut tokio::io::sink()).await?;
        stderr.await.map_err(io::Error::other)??;
        Ok::<(), io::Error>(())
    })
    .await;
    if stdin.is_finished() {
        stdin.join().map_err(|_| "client reader")?;
    }
    drained
        .map_err(|_| "provider drain timeout")?
        .map_err(|_| "provider drain")?;
    result
}

fn input_reader() -> (
    std::thread::JoinHandle<()>,
    tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let thread = std::thread::spawn(move || {
        use std::io::{BufRead, Read};
        let mut reader = std::io::stdin().lock();
        loop {
            let mut bytes = Vec::new();
            let result = (&mut reader)
                .take((MAX_FRAME + 1) as u64)
                .read_until(b'\n', &mut bytes);
            match result {
                Ok(0) => break,
                Ok(_) if bytes.len() <= MAX_FRAME => {
                    if sender.blocking_send(bytes).is_err() {
                        break;
                    }
                },
                _ => {
                    let _ = sender.blocking_send(vec![]);
                    break;
                },
            }
        }
    });
    (thread, receiver)
}

fn fixture_provider() -> bool {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some(mode @ ("--fixture-diagnostic-failure" | "--fixture-diagnostic-unconfirmed")) => {
            let path = PathBuf::from(args.next().unwrap());
            let mut report = capture::EvidenceFile::create(&path).unwrap();
            report
                .write_diagnostic(&capture::SmokeDiagnostic {
                    phase: capture::SmokePhase::Completion,
                    failure: Some(capture::SmokeFailure::ProviderFailed),
                    settled: mode != "--fixture-diagnostic-unconfirmed",
                    binding_verified: false,
                    counts: Some(capture::SmokeCounts {
                        run_failed: 1,
                        ..Default::default()
                    }),
                })
                .unwrap();
            drop(report);
            eprintln!(
                "AUTH_SENTINEL PROMPT_SENTINEL TOOL_SENTINEL NONCE_SENTINEL RAW_ERROR_SENTINEL"
            );
            std::process::exit(7);
        },
        Some("--fixture-raw-failure") => {
            eprintln!(
                "AUTH_SENTINEL PROMPT_SENTINEL TOOL_SENTINEL NONCE_SENTINEL RAW_ERROR_SENTINEL"
            );
            std::process::exit(7);
        },
        Some("--fixture-wire") => {
            fixture_wire(
                args.next().unwrap().into_string().unwrap(),
                PathBuf::from(args.next().unwrap()),
            );
            true
        },
        Some("--fixture-overflow") => {
            use std::io::Write;
            let mut output = std::io::stdout().lock();
            for _ in 0..300 {
                if output.write_all(&[b'x'; 4096]).is_err() {
                    break;
                }
            }
            let _ = output.flush();
            true
        },
        Some("--fixture-provider") => {
            let marker = args.next().expect("fixture marker");
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--fixture-writer")
                .spawn()
                .unwrap();
            std::fs::write(marker, child.id().to_string()).unwrap();
            let _ = child.wait();
            true
        },
        Some("--fixture-writer") => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        _ => false,
    }
}

fn fixture_wire(case: String, marker: PathBuf) {
    use std::io::{BufRead, Write};
    if case.starts_with("native") {
        std::fs::write(marker.with_extension("pid"), std::process::id().to_string()).unwrap();
    }
    let mut output = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        if case.starts_with("native") {
            let mut observed = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&marker)
                .unwrap();
            writeln!(
                observed,
                "{} {}",
                request["method"].as_str().unwrap_or("reply"),
                surge_core::ContentHash::compute(line.as_bytes())
            )
            .unwrap();
        }
        let mut result = match request["method"].as_str().unwrap_or_default() {
            "initialize" => {
                serde_json::json!({"agentInfo":{"name":"@agentclientprotocol/codex-acp","version":"2.2.2"}})
            },
            "session/new" => serde_json::json!({"sessionId":"fixture-session","configOptions":[{
                "id":"model","category":"model","type":"select","currentValue":"other",
                "options":[{"value":if case == "missing-model" { "other" } else { "gpt-6.1-sol" }}]}]}),
            "session/set_config_option" => serde_json::json!({"configOptions":[{
                "id":"model","category":"model","type":"select","currentValue":if case == "wrong-ack" { "other" } else { "gpt-6.1-sol" },"options":[{"value":"gpt-6.1-sol"}]}]}),
            "session/prompt" => {
                if !case.starts_with("native") {
                    std::fs::write(&marker, b"actual provider prompt").unwrap();
                }
                serde_json::json!({"stopReason":"end_turn"})
            },
            _ => serde_json::json!({}),
        };
        let method = request["method"].as_str().unwrap_or_default();
        if method == "session/new" && case == "native-initial" {
            result["configOptions"][0]["currentValue"] = serde_json::json!("gpt-6.1-sol");
        }
        if method == "session/set_config_option" {
            mutate_fixture_options(&case, &mut result);
        }
        if (method == "session/prompt" && case == "native-hold")
            || (method == "initialize" && case == "native-hold-init")
            || (method == "session/new" && case == "native-hold-new")
            || (method == "session/set_config_option" && case == "native-hold-config")
        {
            continue;
        }
        if case == "native-provider-request" {
            if method.is_empty() {
                continue;
            }
            if method == "session/prompt" {
                let message = serde_json::json!({"jsonrpc":"2.0","id":request["id"],"method":"session/request_permission","params":{"tool":"TOOL_SENTINEL"}});
                serde_json::to_writer(&mut output, &message).unwrap();
                output.write_all(b"\n").unwrap();
                output.flush().unwrap();
                continue;
            }
        }
        if method == "session/prompt" && case.starts_with("native-update-") {
            let mut update = serde_json::json!({"configOptions":[{"id":"model","category":"model","type":"select","currentValue":"gpt-6.1-sol","options":[{"value":"gpt-6.1-sol"}]}]});
            mutate_fixture_options(&case.replace("native-update-", "native-ack-"), &mut update);
            let message = serde_json::json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"config_option_update","configOptions":update["configOptions"]}}});
            serde_json::to_writer(&mut output, &message).unwrap();
            output.write_all(b"\n").unwrap();
            output.flush().unwrap();
        }
        if method == "session/prompt" && case == "native-full-update" {
            let message = serde_json::json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"config_option_update","configOptions":[
                {"id":"model","category":"model","type":"select","currentValue":"gpt-6.1-sol","options":[{"value":"gpt-6.1-sol"}]},
                {"id":"mode","category":"mode","type":"select","currentValue":"auto","options":[{"value":"auto"}]},
                {"id":"thought","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"high"}]},
                {"id":"collaboration","type":"select","currentValue":"default","options":[{"value":"default"}]}]}}});
            serde_json::to_writer(&mut output, &message).unwrap();
            output.write_all(b"\n").unwrap();
            output.flush().unwrap();
        }
        let mut response = serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":result});
        if method == "session/prompt" && case == "native-malformed-response" {
            response["error"] = serde_json::json!({"message":"ERROR_SENTINEL"});
        }
        if method == "session/prompt" && case == "native-error-response" {
            response.as_object_mut().unwrap().remove("result");
            response["error"] = serde_json::json!({"message":"ERROR_SENTINEL"});
        }
        if case == format!("native-duplicate-{method}") {
            serde_json::to_writer(&mut output, &response).unwrap();
            output.write_all(b"\n").unwrap();
            output.flush().unwrap();
        }
        serde_json::to_writer(&mut output, &response).unwrap();
        output.write_all(b"\n").unwrap();
        output.flush().unwrap();
    }
}

fn mutate_fixture_options(case: &str, result: &mut serde_json::Value) {
    use serde_json::json;
    match case.strip_prefix("native-ack-").unwrap_or_default() {
        "removed" => result["configOptions"][0]["options"] = json!([{"value":"other"}]),
        "malformed" => result["configOptions"][0]["options"] = json!("invalid"),
        "ambiguous" => {
            let duplicate = result["configOptions"][0].clone();
            result["configOptions"]
                .as_array_mut()
                .unwrap()
                .push(duplicate);
        },
        "identity" => result["configOptions"][0]["id"] = json!("other-model"),
        _ => {},
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(args: &[&str]) -> Result<ProxyArgs, &'static str> {
        proxy_args(args.iter().map(std::ffi::OsString::from))
    }
    #[test]
    fn proxy_option_is_explicit_and_does_not_consume_provider_arguments() {
        let args = parsed(&["capture", "gpt-6.1-sol", "provider", "--native-multi-turn"]).unwrap();
        assert!(!args.native);
        assert_eq!(args.provider_args, ["--native-multi-turn"]);
        let args = parsed(&[
            "--native-multi-turn",
            "--capture-dir",
            "private",
            "--initial-provider",
            "gpt-6.1-sol",
            "provider",
            "--unknown",
        ])
        .unwrap();
        assert!(args.native);
        assert_eq!(args.mode, capture::SelectionMode::InitialProvider);
        for args in [
            vec!["--unknown", "capture", "gpt-6.1-sol", "provider"],
            vec![
                "--native-multi-turn",
                "capture",
                "--native-multi-turn",
                "gpt-6.1-sol",
                "provider",
            ],
            vec!["capture", "gpt-6.1-sol", "--unknown"],
            vec![
                "capture",
                "--initial-provider",
                "--initial-provider",
                "provider",
            ],
            vec!["--capture-dir"],
            vec!["capture", "gpt-6.1-sol"],
        ] {
            assert!(parsed(&args).is_err());
        }
    }
}
