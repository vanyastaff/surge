//! Output-spill threshold configuration.
//!
//! [`OutputSpillConfig`] is a `surge.toml`-level threshold consumed by
//! `surge_orchestrator::spill` (`.autopilot/competitive-waves/spec.md` §16:
//! "spill goes to the existing artifact store; no new store"). It used to
//! share a file with the unrelated `Loop` flow-graph node config
//! (`loop_config.rs`) — moved here because output-spill is not a "loop"
//! concept, and the spec's boundary table gives large-output policy to
//! `surge-orchestrator::spill`, not to the flow-graph loop module.

use serde::{Deserialize, Serialize};

/// Threshold beyond which a tool's output is moved to the artifact store
/// instead of flowing to the node in full; see `surge_orchestrator::spill`.
/// `surge.toml` key: `output_spill`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputSpillConfig {
    /// Tool output at or under this many bytes is returned to the node
    /// unchanged. Output beyond it is spilled: the node gets a bounded
    /// preview plus a locator for the full text.
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

impl Default for OutputSpillConfig {
    fn default() -> Self {
        Self {
            max_output_bytes: default_max_output_bytes(),
        }
    }
}

/// Conservative default: 64 KiB. Comfortably above a normal file read or
/// command output, well below what meaningfully erodes a node's context
/// budget.
fn default_max_output_bytes() -> usize {
    65536
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_spill_config_toml_roundtrip() {
        let cfg = OutputSpillConfig {
            max_output_bytes: 1024,
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: OutputSpillConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn output_spill_config_default_is_conservative() {
        assert_eq!(OutputSpillConfig::default().max_output_bytes, 65536);
    }

    #[test]
    fn output_spill_config_absent_fields_default_on_parse() {
        // Backward-compat guard: a `surge.toml` predating this key parses
        // as an empty table and must decode with the conservative default.
        let parsed: OutputSpillConfig = toml::from_str("").unwrap();
        assert_eq!(parsed, OutputSpillConfig::default());
    }
}
