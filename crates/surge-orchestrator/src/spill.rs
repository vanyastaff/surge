//! Output-spill policy for oversized tool results.
//!
//! [`spill_if_oversized`] moves a tool's output to the **existing**
//! content-addressed artifact store (`surge_persistence::artifacts::ArtifactStore`)
//! once it crosses a configured cap, per
//! `.autopilot/competitive-waves/spec.md` §16 ("spill goes to the existing
//! artifact store; no new store"). The node gets back a bounded preview plus
//! a locator ([`ArtifactLocator`]) it (or a human, later) can use to fetch
//! the full text. A failed artifact save never loses the output: the
//! original content is returned unchanged rather than dropped.

use surge_core::content_hash::ContentHash;
use surge_core::id::RunId;
use surge_persistence::artifacts::ArtifactStore;

/// How many bytes of a spilled output are kept as an inline preview,
/// independent of the spill cap itself (a cap of 64 KiB does not mean a
/// 64 KiB preview — the point of spilling is to stop returning that much).
const PREVIEW_BYTES: usize = 2048;

/// Where the full text of a spilled tool output can be read back from.
///
/// The exact shape is deliberately hidden from callers outside this module
/// (see [`ArtifactLocator::to_uri`]) — `surge-orchestrator::spill` owns the
/// locator's form per `.autopilot/competitive-waves/spec.md`'s boundary
/// table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactLocator {
    run_id: RunId,
    hash: ContentHash,
}

impl ArtifactLocator {
    /// Opaque URI form: `artifact://<run-id>/<sha256-hex>`. Resolve it by
    /// parsing out the hash and calling
    /// `ArtifactStore::open(run_id, hash)` — the store this module already
    /// wrote to.
    #[must_use]
    pub fn to_uri(&self) -> String {
        format!("artifact://{}/{}", self.run_id, self.hash.to_hex())
    }

    /// The run this artifact belongs to.
    #[must_use]
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Content hash of the full spilled output.
    #[must_use]
    pub fn hash(&self) -> ContentHash {
        self.hash
    }
}

/// Result of running a tool's output through [`spill_if_oversized`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutput {
    /// Output was at or under the cap (or could not be spilled — see
    /// [`spill_if_oversized`]'s doc): returned to the node unchanged.
    Inline(serde_json::Value),
    /// Output exceeded the cap and now lives in the artifact store.
    Spilled {
        /// First [`PREVIEW_BYTES`] bytes of the output, UTF-8-safe
        /// (lossy-converted, never a panic on a split multi-byte char).
        preview: String,
        /// How to fetch the full text.
        locator: ArtifactLocator,
        /// Total size of the original output, in bytes.
        total_bytes: usize,
    },
}

impl ToolOutput {
    /// The JSON value actually sent back to the node. Callers should not
    /// need to match on `ToolOutput`'s variants themselves — this is the
    /// one place the `Spilled` shape gets serialized.
    #[must_use]
    pub fn into_content(self) -> serde_json::Value {
        match self {
            Self::Inline(v) => v,
            Self::Spilled {
                preview,
                locator,
                total_bytes,
            } => serde_json::json!({
                "spilled": true,
                "preview": preview,
                "total_bytes": total_bytes,
                "locator": locator.to_uri(),
            }),
        }
    }
}

/// Spill `content` to `store` if its serialized size exceeds `cap_bytes`.
///
/// `name` labels the artifact for humans (the store's own index; not part of
/// the locator's identity, which is content-addressed). `store` is
/// `Option` because the caller may not have one available — treated the
/// same as a failed save: the original `content` is returned, never
/// dropped. This is deliberate, not a placeholder: an artifact-store outage
/// must never make a tool result vanish.
///
/// # Panics
/// Never panics: preview truncation uses a lossy UTF-8 conversion rather
/// than slicing on a byte boundary that could split a multi-byte character.
pub async fn spill_if_oversized(
    run_id: RunId,
    name: &str,
    content: serde_json::Value,
    cap_bytes: usize,
    store: Option<&ArtifactStore>,
) -> ToolOutput {
    let serialized = content.to_string();
    if serialized.len() <= cap_bytes {
        return ToolOutput::Inline(content);
    }

    let Some(store) = store else {
        return ToolOutput::Inline(content);
    };

    let bytes = serialized.into_bytes();
    match store.put(run_id, name, &bytes).await {
        Ok(artifact_ref) => {
            let preview_len = PREVIEW_BYTES.min(bytes.len());
            let preview = String::from_utf8_lossy(&bytes[..preview_len]).into_owned();
            ToolOutput::Spilled {
                preview,
                locator: ArtifactLocator {
                    run_id,
                    hash: artifact_ref.hash,
                },
                total_bytes: bytes.len(),
            }
        },
        // Save failed — the original result stays visible rather than lost.
        Err(_) => ToolOutput::Inline(content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(root: &std::path::Path) -> ArtifactStore {
        ArtifactStore::new(root.join("runs"))
    }

    #[tokio::test]
    async fn output_under_cap_stays_inline() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let run_id = RunId::new();
        let content = serde_json::json!({ "text": "small" });

        let out = spill_if_oversized(run_id, "tool-output", content.clone(), 4096, Some(&s)).await;

        assert_eq!(out, ToolOutput::Inline(content));
    }

    #[tokio::test]
    async fn output_over_cap_spills_to_the_artifact_store() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let run_id = RunId::new();
        let big_text = "x".repeat(10_000);
        let content = serde_json::json!({ "text": big_text });

        let out = spill_if_oversized(run_id, "tool-output", content.clone(), 100, Some(&s)).await;

        let ToolOutput::Spilled {
            preview,
            locator,
            total_bytes,
        } = out
        else {
            panic!("expected Spilled");
        };
        assert!(preview.len() <= PREVIEW_BYTES);
        assert_eq!(total_bytes, content.to_string().len());

        // The locator resolves back to the exact original bytes via the
        // *existing* artifact store — no second storage mechanism.
        let fetched = s.open(run_id, locator.hash()).await.unwrap();
        assert_eq!(fetched, content.to_string().into_bytes());
    }

    #[tokio::test]
    async fn no_store_available_keeps_the_original_output_visible() {
        let run_id = RunId::new();
        let big_text = "y".repeat(10_000);
        let content = serde_json::json!({ "text": big_text });

        let out = spill_if_oversized(run_id, "tool-output", content.clone(), 100, None).await;

        assert_eq!(out, ToolOutput::Inline(content));
    }

    #[tokio::test]
    async fn a_failed_save_keeps_the_original_output_visible() {
        // Point the store at a path that cannot be created (a file, not a
        // directory, sits where the artifacts dir would need to go) so
        // `put` fails; the failure must not lose the tool's output.
        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("runs");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let s = ArtifactStore::new(blocked);
        let run_id = RunId::new();
        let big_text = "z".repeat(10_000);
        let content = serde_json::json!({ "text": big_text });

        let out = spill_if_oversized(run_id, "tool-output", content.clone(), 100, Some(&s)).await;

        assert_eq!(out, ToolOutput::Inline(content));
    }

    #[test]
    fn spilled_into_content_never_leaks_the_locator_struct_shape() {
        let out = ToolOutput::Spilled {
            preview: "abc".into(),
            locator: ArtifactLocator {
                run_id: RunId::new(),
                hash: ContentHash::compute(b"abc"),
            },
            total_bytes: 12345,
        };
        let json = out.into_content();
        assert_eq!(json["spilled"], serde_json::json!(true));
        assert_eq!(json["preview"], serde_json::json!("abc"));
        assert_eq!(json["total_bytes"], serde_json::json!(12345));
        assert!(json["locator"].as_str().unwrap().starts_with("artifact://"));
    }
}
