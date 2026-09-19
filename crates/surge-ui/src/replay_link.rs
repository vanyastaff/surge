//! Replay bridge: rebuild a finished (or never-attached) run's cockpit
//! state from its durable event log in the registry storage.
//!
//! The per-run event log is SQLite at `~/.surge/runs/<run_id>/` (event
//! sourcing, ADR-0003). The live per-run broadcast has no history
//! replay, so a run that finished before the cockpit attached shows an
//! empty pane; this module folds the *persisted* events through the
//! same [`crate::run_stream::RunStreamState`] fold the live pump uses,
//! so a replayed run and a live run project identically.
//!
//! Opening a reader needs the tokio runtime (spawn_blocking inside);
//! calls run through the app's background executor.

use std::collections::HashMap;

use surge_core::id::RunId;
use surge_persistence::runs::Storage;

/// A run's durable telemetry, folded from its event log.
#[derive(Clone, Default)]
pub struct ReplaySnapshot {
    /// Last persisted seq (the "events" KPI).
    pub last_seq: u64,
    /// Log rows, oldest first (the UI reverses for display).
    pub rows: Vec<crate::run_stream::RunLogRow>,
    /// Stage pipeline as of the end of the log.
    pub stages: Vec<crate::run_stream::StageRow>,
    /// Accumulated token/cost counters.
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
}

/// One artifact of a run, for the cockpit's artifact pane.
#[derive(Debug, Clone)]
pub struct ReplayArtifact {
    /// Logical name (e.g. `spec.toml`, `patch.diff`).
    pub name: String,
    pub size_bytes: u64,
    /// Content hash — the artifact's address for `read_artifact_text`.
    pub content_hash: String,
}

/// Fold a run's whole persisted log into one snapshot. Reads the events
/// DB directly (`Storage::open` + `open_run_reader`), so replay works
/// whether or not a daemon is connected.
///
/// # Errors
/// Verbatim strings for the cockpit's note line — a missing run DB is
/// the normal case for queued runs.
pub async fn load_replay(run_id: RunId) -> Result<ReplaySnapshot, String> {
    require_tokio_runtime()?;
    let reader = open_reader(run_id).await?;
    let events = reader
        .read_run_events()
        .await
        .map_err(|e| format!("read events: {e}"))?;

    let mut stream = crate::run_stream::RunStreamState::default();
    for event in &events {
        let time = event
            .timestamp
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string();
        stream.apply_event_at(event.seq, &event.payload, time);
    }
    Ok(ReplaySnapshot {
        last_seq: stream.last_seq,
        rows: stream.log.into_iter().collect(),
        stages: stream.stages,
        tokens_in: stream.tokens_in,
        tokens_out: stream.tokens_out,
        cost_usd: stream.cost_usd,
    })
}

/// `RunReader` reads through `tokio::task::spawn_blocking`, which
/// panics outside a tokio runtime (the gpui test harness provides its
/// own executor). A caller without one gets an error, not a panic.
fn require_tokio_runtime() -> Result<(), String> {
    if tokio::runtime::Handle::try_current().is_err() {
        return Err("no tokio runtime (registry storage unavailable)".to_string());
    }
    Ok(())
}

/// List a run's artifacts (metadata only). Empty when the run has none
/// — not an error.
///
/// # Errors
/// Registry open/reader failures, verbatim.
pub async fn load_artifacts(run_id: RunId) -> Result<Vec<ReplayArtifact>, String> {
    let reader = open_reader(run_id).await?;
    let records = reader
        .artifacts()
        .await
        .map_err(|e| format!("read artifacts: {e}"))?;

    Ok(records
        .iter()
        .map(|r| ReplayArtifact {
            name: r.name.clone(),
            size_bytes: r.size_bytes,
            content_hash: r.content_hash.to_string(),
        })
        .collect())
}

/// Read one artifact's bytes by content hash and decode as UTF-8 when
/// plausibly text (bounded — a multi-MB artifact must not freeze the
/// render thread that shows it).
///
/// # Errors
/// Registry open/read failures, verbatim.
pub async fn read_artifact_text(
    run_id: RunId,
    content_hash: String,
) -> Result<Option<String>, String> {
    use std::str::FromStr;
    use surge_core::ContentHash;
    let hash =
        ContentHash::from_str(&content_hash).map_err(|e| format!("bad content hash: {e}"))?;
    let reader = open_reader(run_id).await?;
    let bytes = reader
        .read_artifact(&hash)
        .await
        .map_err(|e| format!("read artifact: {e}"))?;
    Ok(decode_text(&bytes))
}

/// UTF-8 decode with a hard size cap.
fn decode_text(bytes: &[u8]) -> Option<String> {
    const MAX_TEXT_BYTES: usize = 512 * 1024;
    const MAX_TEXT_CHARS: usize = 200_000;
    if bytes.is_empty() || bytes.len() > MAX_TEXT_BYTES {
        return None;
    }
    std::str::from_utf8(bytes)
        .ok()
        .map(|s| s.chars().take(MAX_TEXT_CHARS).collect())
}

/// Open the registry and the run's reader. One call per load.
async fn open_reader(run_id: RunId) -> Result<surge_persistence::runs::RunReader, String> {
    require_tokio_runtime()?;
    let home = surge_core::home::surge_home_dir()
        .ok_or_else(|| "could not resolve SURGE_HOME".to_string())?;
    let storage = Storage::open(home)
        .await
        .map_err(|e| format!("open registry: {e}"))?;
    reader_for(&storage, run_id).await
}

/// Reader or an empty-error string when the run DB is absent (queued
/// runs have no log yet — the caller treats this as "nothing to show").
async fn reader_for(
    storage: &std::sync::Arc<Storage>,
    run_id: RunId,
) -> Result<surge_persistence::runs::RunReader, String> {
    storage
        .open_run_reader(run_id)
        .await
        .map_err(|e| format!("open run log: {e}"))
}

/// Cache of replayed snapshots keyed by run id (one screen's worth).
pub type ReplayCache = HashMap<RunId, ReplaySnapshot>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_text_rejects_binary_and_oversize() {
        // Empty and binary payloads decode to None.
        assert!(decode_text(b"").is_none());
        assert!(decode_text(&[0, 159, 146, 150]).is_none());
        // Text within the cap decodes.
        assert_eq!(decode_text(b"hello").as_deref(), Some("hello"));

        // A file over the cap is refused, not truncated into a lie.
        let oversized = vec![b'a'; 512 * 1024 + 1];
        assert!(decode_text(&oversized).is_none());

        // Just under the cap decodes, capped at the char budget.
        let just_under = vec![b'x'; 512 * 1024];
        let decoded = decode_text(&just_under).expect("under-cap text decodes");
        assert_eq!(decoded.len(), 200_000, "decoded to the char cap");
    }

    #[test]
    fn require_runtime_errors_without_tokio_context() {
        // Outside a runtime (this test thread) the guard must refuse —
        // `spawn_blocking` inside the reader would panic otherwise.
        if tokio::runtime::Handle::try_current().is_err() {
            assert!(require_tokio_runtime().is_err());
        }
    }
}
