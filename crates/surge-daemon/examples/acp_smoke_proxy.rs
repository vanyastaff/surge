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

async fn run() -> Result<(), &'static str> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next().ok_or("missing capture destination")?;
    let capture_path = if first == "--capture-dir" {
        PathBuf::from(args.next().ok_or("missing capture directory")?)
            .join(format!("{}.json", surge_core::RunId::new()))
    } else {
        PathBuf::from(first)
    };
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
    let program = args.next().ok_or("missing program")?;
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut witness = capture::ProtocolCapture::with_mode(required_model, mode)?;
    let mut evidence = capture::EvidenceFile::create(&capture_path)?;
    evidence.write(&witness.snapshot)?;
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
                let Some(incoming) = incoming else { break Ok(()); };
                let value = match serde_json::from_slice(&incoming) { Ok(value) => value, Err(_) => break Err("client framing") };
                if witness.client(&value).is_err() { break Err("model proof unavailable"); }
                if evidence.write(&witness.snapshot).is_err() { break Err("capture write"); }
                if capture::forward(&mut input, &incoming).await.is_err() { break Err("provider input"); }
            },
            read = frame(&mut output, &mut bytes) => {
                match read { Ok(0) => break Ok(()), Err(_) => break Err("provider framing"), _ => {} }
                let value = match serde_json::from_slice(&bytes) { Ok(value) => value, Err(_) => break Err("provider framing") };
                if witness.provider(&value).is_err() { break Err("provider metadata"); }
                if evidence.write(&witness.snapshot).is_err() { break Err("capture write"); }
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
    let mut output = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        let result = match request["method"].as_str().unwrap_or_default() {
            "initialize" => {
                serde_json::json!({"agentInfo":{"name":"@agentclientprotocol/codex-acp","version":"2.2.2"}})
            },
            "session/new" => serde_json::json!({"sessionId":"fixture-session","configOptions":[{
                "id":"model","category":"model","type":"select","currentValue":"other",
                "options":[{"value":if case == "missing-model" { "other" } else { "gpt-6.1-sol" }}]}]}),
            "session/set_config_option" => serde_json::json!({"configOptions":[{
                "id":"model","category":"model","type":"select","currentValue":if case == "wrong-ack" { "other" } else { "gpt-6.1-sol" }}]}),
            "session/prompt" => {
                std::fs::write(&marker, b"actual provider prompt").unwrap();
                serde_json::json!({"stopReason":"end_turn"})
            },
            _ => serde_json::json!({}),
        };
        let response = serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":result});
        serde_json::to_writer(&mut output, &response).unwrap();
        output.write_all(b"\n").unwrap();
        output.flush().unwrap();
    }
}
