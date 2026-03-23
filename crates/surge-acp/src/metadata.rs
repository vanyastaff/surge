//! Agent metadata — rich display data (pricing, models, features, colors).
//!
//! Supplements the ACP registry with information not in the protocol spec:
//! vendor branding, pricing plans, model lists, feature flags, hex colors.
//!
//! Two-tier loading (same pattern as registry):
//! 1. Embedded fallback from `agent_metadata.json`
//! 2. Cache at `~/.surge/cache/agent_metadata.json`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use tracing::{debug, info, warn};

const METADATA_JSON: &str = include_str!("agent_metadata.json");

/// Global cached metadata store — parsed once, reused everywhere.
static EMBEDDED_METADATA: LazyLock<MetadataStore> = LazyLock::new(MetadataStore::load);

// ── Serde types ─────────────────────────────────────────────────────

/// Root of the metadata file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataFile {
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    pub version: String,
    pub updated_at: String,
    pub agents: HashMap<String, AgentMetadata>,
}

/// Rich metadata for a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub display_name: String,
    #[serde(default)]
    pub tagline: String,
    pub vendor: String,
    #[serde(default)]
    pub vendor_url: String,
    #[serde(default)]
    pub docs_url: String,
    #[serde(default)]
    pub icon_url: String,
    /// Hex color for vendor branding (e.g. "#D97757").
    #[serde(default)]
    pub color: String,
    /// "wrapper" (ACP adapter around a CLI) or "native" (direct ACP support).
    #[serde(default)]
    pub acp_type: String,
    /// Command + args to detect the real installed CLI version.
    #[serde(default)]
    pub version_command: Vec<String>,
    /// For wrapper agents: the name of the underlying CLI (e.g. "Claude Code").
    #[serde(default)]
    pub wrapped_cli_name: String,
    /// Known command names for this agent.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Primary binary name.
    #[serde(default)]
    pub binary_name: String,
    #[serde(default)]
    pub pricing: Option<PricingInfo>,
    #[serde(default)]
    pub context_window: Option<ContextWindow>,
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub features: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
}

/// Pricing information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingInfo {
    /// "subscription", "api-key", "byok", "free"
    #[serde(rename = "type")]
    pub pricing_type: String,
    #[serde(default)]
    pub plans: Vec<PricingPlan>,
}

/// A single pricing plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingPlan {
    pub name: String,
    pub price: String,
    #[serde(default)]
    pub note: String,
}

/// Context window information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextWindow {
    #[serde(default)]
    pub default: u64,
    #[serde(default)]
    pub max: u64,
    #[serde(default)]
    pub note: String,
}

/// Model information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub released: String,
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub strengths: Vec<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub swe_bench: Option<f64>,
    #[serde(default)]
    pub terminal_bench: Option<f64>,
    #[serde(default)]
    pub premium_multiplier: Option<f64>,
}

/// Model selection method for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSelection {
    pub method: String,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default)]
    pub flag: Option<String>,
    #[serde(default)]
    pub in_session: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

// ── MetadataStore ───────────────────────────────────────────────────

/// Store for agent metadata, loaded from embedded + cache.
#[derive(Debug, Clone)]
pub struct MetadataStore {
    file: MetadataFile,
}

impl MetadataStore {
    /// Get the global cached metadata store (parsed once on first access).
    #[must_use]
    pub fn global() -> &'static Self {
        &EMBEDDED_METADATA
    }

    /// Load from embedded JSON (creates a new instance — prefer `global()`).
    #[must_use]
    pub fn embedded() -> Self {
        Self::from_json(METADATA_JSON).unwrap_or_else(|e| {
            tracing::error!("Failed to parse embedded metadata: {e}");
            Self {
                file: MetadataFile {
                    schema: None,
                    version: "0.0.0".into(),
                    updated_at: "unknown".into(),
                    agents: HashMap::new(),
                },
            }
        })
    }

    /// Load from cache, fall back to embedded.
    #[must_use]
    pub fn load() -> Self {
        if let Some(path) = cache_path() {
            if path.exists() {
                if let Ok(json) = std::fs::read_to_string(&path) {
                    if let Ok(store) = Self::from_json(&json) {
                        info!(
                            agents = store.file.agents.len(),
                            "loaded metadata from cache"
                        );
                        return store;
                    } else {
                        warn!("cached metadata corrupt, using embedded");
                    }
                }
            } else {
                debug!("no cached metadata, using embedded");
            }
        }
        Self::embedded()
    }

    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let file: MetadataFile =
            serde_json::from_str(json).map_err(|e| format!("Metadata parse error: {e}"))?;
        Ok(Self { file })
    }

    /// Save to cache.
    pub fn save_cache(json: &str) -> Result<PathBuf, String> {
        let dir = cache_dir().ok_or("Cannot determine home directory")?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create cache dir: {e}"))?;
        let path = dir.join("agent_metadata.json");
        std::fs::write(&path, json).map_err(|e| format!("Cannot write metadata cache: {e}"))?;
        info!("metadata cache updated: {}", path.display());
        Ok(path)
    }

    /// Get metadata for a specific agent.
    #[must_use]
    pub fn get(&self, agent_id: &str) -> Option<&AgentMetadata> {
        self.file.agents.get(agent_id)
    }

    /// Get all agent IDs with metadata.
    #[must_use]
    pub fn agent_ids(&self) -> Vec<&str> {
        self.file.agents.keys().map(String::as_str).collect()
    }

    /// Version of the metadata file.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.file.version
    }

    /// When the metadata was last updated.
    #[must_use]
    pub fn updated_at(&self) -> &str {
        &self.file.updated_at
    }

    /// Parse hex color string to (r, g, b) floats in 0.0-1.0 range.
    #[must_use]
    pub fn parse_color(hex: &str) -> Option<(f32, f32, f32)> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
        Some((r, g, b))
    }
}

// ── Cache paths ─────────────────────────────────────────────────────

fn cache_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE")
            .ok()
            .map(|p| PathBuf::from(p).join(".surge").join("cache"))
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .ok()
            .map(|p| PathBuf::from(p).join(".surge").join("cache"))
    }
}

fn cache_path() -> Option<PathBuf> {
    cache_dir().map(|p| p.join("agent_metadata.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_parses() {
        let store = MetadataStore::embedded();
        assert!(!store.file.agents.is_empty());
        assert!(store.get("claude-acp").is_some());
    }

    #[test]
    fn test_claude_metadata() {
        let store = MetadataStore::embedded();
        let claude = store.get("claude-acp").unwrap();
        assert_eq!(claude.vendor, "Anthropic");
        assert_eq!(claude.color, "#D97757");
        assert!(!claude.models.is_empty());
        assert!(claude.models.iter().any(|m| m.name == "Claude Opus 4.6"));
    }

    #[test]
    fn test_parse_color() {
        let (r, g, b) = MetadataStore::parse_color("#D97757").unwrap();
        assert!((r - 0.851).abs() < 0.01);
        assert!((g - 0.467).abs() < 0.01);
        assert!((b - 0.341).abs() < 0.01);
    }

    #[test]
    fn test_parse_color_invalid() {
        assert!(MetadataStore::parse_color("invalid").is_none());
        assert!(MetadataStore::parse_color("#GGG").is_none());
    }

    #[test]
    fn test_all_agents_have_vendor() {
        let store = MetadataStore::embedded();
        for (id, meta) in &store.file.agents {
            assert!(!meta.vendor.is_empty(), "Agent '{id}' has no vendor");
        }
    }

    #[test]
    fn test_pricing_types() {
        let store = MetadataStore::embedded();
        let claude = store.get("claude-acp").unwrap();
        assert_eq!(claude.pricing.as_ref().unwrap().pricing_type, "api-key");

        let copilot = store.get("github-copilot-cli").unwrap();
        assert_eq!(copilot.pricing.as_ref().unwrap().pricing_type, "subscription");

        let goose = store.get("goose").unwrap();
        assert_eq!(goose.pricing.as_ref().unwrap().pricing_type, "byok");
    }
}
