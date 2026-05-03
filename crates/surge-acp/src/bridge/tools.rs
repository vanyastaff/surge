//! Tool definitions injected into ACP sessions, plus the engine-injected
//! `report_stage_outcome` and `request_human_input` builders. See spec §5.3 / §5.4.

use serde_json::{Value, json};
use surge_core::OutcomeKey;

/// Schema for a single tool the bridge declares to the agent at session-open time.
///
/// Constructed by the engine (M5) for caller-supplied MCP tools, and by the bridge
/// itself for the engine-injected `report_stage_outcome` and `request_human_input`.
/// The `input_schema` is JSON Schema and is passed straight to ACP without
/// re-parsing — `Value` keeps the schema opaque to the bridge.
#[derive(Debug, Clone)]
pub struct ToolDef {
    /// Tool name as the agent sees it (also the JSON-RPC method discriminator).
    pub name: String,
    /// One-sentence purpose description shown to the agent.
    pub description: String,
    /// Where the tool came from — drives the `mcp_id` field in `BridgeEvent::ToolCallMeta`.
    pub category: ToolCategory,
    /// JSON Schema for the input. Keep as `Value` so the bridge can pass it
    /// straight to ACP without re-parsing.
    pub input_schema: Value,
}

impl ToolDef {
    /// Construct a `ToolDef` with owned strings; useful for both production
    /// builders and tests.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        category: ToolCategory,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            category,
            input_schema,
        }
    }
}

/// Where the tool came from. Drives the `mcp_id` field in `BridgeEvent::ToolCallMeta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCategory {
    /// Engine-owned (`report_stage_outcome`, `request_human_input`).
    Injected,
    /// Provided by an MCP server with the given id.
    Mcp(String),
    /// Built-in ACP-side tool (filesystem, terminal). No MCP id.
    Builtin,
}

impl ToolCategory {
    /// Returns the MCP id if this is an MCP-sourced tool, else None.
    #[must_use]
    pub fn mcp_id(&self) -> Option<&str> {
        match self {
            Self::Mcp(id) => Some(id.as_str()),
            _ => None,
        }
    }
}

/// Constant tool name for `report_stage_outcome`. Used by the worker to
/// recognize the call without string-matching everywhere.
pub const REPORT_STAGE_OUTCOME: &str = "report_stage_outcome";
/// Constant tool name for `request_human_input`.
pub const REQUEST_HUMAN_INPUT: &str = "request_human_input";

/// Build the `report_stage_outcome` tool with a dynamic `enum` populated from
/// the node's declared outcomes. Caller must ensure `declared_outcomes` is
/// non-empty (`SessionConfig::validate` already checks this).
#[must_use]
pub fn build_report_stage_outcome_tool(declared_outcomes: &[OutcomeKey]) -> ToolDef {
    assert!(
        !declared_outcomes.is_empty(),
        "M3 contract: caller must check via SessionConfig::validate"
    );
    let outcomes_json: Vec<Value> = declared_outcomes
        .iter()
        .map(|k| Value::String(k.as_str().to_string()))
        .collect();
    ToolDef::new(
        REPORT_STAGE_OUTCOME,
        "Report your stage's outcome. Call this exactly once at the end.",
        ToolCategory::Injected,
        json!({
            "type": "object",
            "required": ["outcome", "summary"],
            "properties": {
                "outcome": {
                    "type": "string",
                    "enum": outcomes_json,
                    "description": "Which declared outcome best describes your result"
                },
                "summary": {
                    "type": "string",
                    "description": "1-3 sentences explaining what you did and why this outcome"
                },
                "artifacts_produced": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of file paths you created or modified"
                }
            }
        }),
    )
}

/// Build the `request_human_input` tool. Always the same shape — no dynamic schema.
#[must_use]
pub fn build_request_human_input_tool() -> ToolDef {
    ToolDef::new(
        REQUEST_HUMAN_INPUT,
        "Pause and ask the human for guidance. Use sparingly.",
        ToolCategory::Injected,
        json!({
            "type": "object",
            "required": ["question"],
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the human. Be specific."
                },
                "context": {
                    "type": "string",
                    "description": "Optional context the human needs to answer well."
                }
            }
        }),
    )
}

/// Build the engine-injected tools for a session: always
/// `report_stage_outcome` and, when `allows_escalation` is true,
/// `request_human_input`. The worker prepends these to the caller-supplied
/// tool list during session open (see `bridge::worker::filter_visible_tools`,
/// added in Phase 8.1).
#[must_use]
pub fn build_injected_tools(
    declared_outcomes: &[OutcomeKey],
    allows_escalation: bool,
) -> Vec<ToolDef> {
    let mut out = Vec::with_capacity(2);
    out.push(build_report_stage_outcome_tool(declared_outcomes));
    if allows_escalation {
        out.push(build_request_human_input_tool());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn ok(s: &str) -> OutcomeKey {
        OutcomeKey::from_str(s).unwrap()
    }

    #[test]
    fn report_stage_outcome_includes_dynamic_enum() {
        let t = build_report_stage_outcome_tool(&[ok("done"), ok("blocked")]);
        assert_eq!(t.name, REPORT_STAGE_OUTCOME);
        let enum_values = &t.input_schema["properties"]["outcome"]["enum"];
        assert_eq!(enum_values, &json!(["done", "blocked"]));
    }

    #[test]
    fn report_stage_outcome_is_marked_injected() {
        let t = build_report_stage_outcome_tool(&[ok("done")]);
        assert_eq!(t.category, ToolCategory::Injected);
        assert!(t.category.mcp_id().is_none());
    }

    #[test]
    fn request_human_input_is_static_shape() {
        let a = build_request_human_input_tool();
        let b = build_request_human_input_tool();
        // Two builds yield byte-identical schemas (no dynamism).
        assert_eq!(a.input_schema, b.input_schema);
    }

    #[test]
    fn build_injected_tools_skips_human_input_when_disabled() {
        let v = build_injected_tools(&[ok("done")], false);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, REPORT_STAGE_OUTCOME);
    }

    #[test]
    fn build_injected_tools_includes_human_input_when_enabled() {
        let v = build_injected_tools(&[ok("done")], true);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].name, REQUEST_HUMAN_INPUT);
    }

    #[test]
    fn mcp_category_returns_id() {
        let t = ToolDef::new(
            "shell_exec",
            "run a shell command",
            ToolCategory::Mcp("ops".into()),
            json!({}),
        );
        assert_eq!(t.category.mcp_id(), Some("ops"));
    }

    #[test]
    fn report_stage_outcome_schema_snapshot() {
        let t = build_report_stage_outcome_tool(&[ok("done"), ok("blocked"), ok("escalate")]);
        insta::assert_json_snapshot!("report_stage_outcome_schema", t.input_schema);
    }

    proptest::proptest! {
        #[test]
        fn outcome_enum_serializable_for_any_size(
            n in 1usize..32usize,
        ) {
            let outcomes: Vec<OutcomeKey> = (0..n)
                .map(|i| OutcomeKey::from_str(&format!("o{i}")).unwrap())
                .collect();
            let t = build_report_stage_outcome_tool(&outcomes);
            // Round-trip through JSON: serialize, deserialize, must be equal.
            let s = serde_json::to_string(&t.input_schema).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
            proptest::prop_assert_eq!(parsed, t.input_schema);
        }
    }
}
