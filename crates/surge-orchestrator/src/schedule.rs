//! Schedule — AI-driven optimal ordering of subtasks into execution batches.

use surge_core::spec::Subtask;

/// Build a prompt that asks an agent to order subtasks into optimal batches.
///
/// The agent considers dependencies, effort, impact, and parallelism to
/// produce a JSON schedule.
#[must_use]
pub fn build_schedule_prompt(subtasks: &[Subtask], requirements_summary: &str) -> String {
    let items_list = format_subtasks(subtasks);

    format!(
        r#"You are a project manager. Order these tasks optimally.

## Project Context
{requirements_summary}

## Tasks
{items_list}

## Rules
- Blocking tasks come first
- Independent tasks run in parallel (same batch, max 3)
- High effort + high impact first
- Explain reasoning

## Output (JSON only)
{{"batches": [{{"order": 0, "items": ["subtask_id_1"], "reason": "why"}}]}}
"#
    )
}

/// Parse a schedule response into ordered batches of subtask IDs.
///
/// Expects JSON with `{"batches": [{"order": N, "items": ["id1", ...]}]}`.
/// Returns `None` if the response is not valid JSON or missing fields.
#[must_use]
pub fn parse_schedule_response(response: &str) -> Option<Vec<Vec<String>>> {
    // Find JSON object in the response (agent may wrap it in markdown fences).
    let json_str = extract_json_block(response)?;
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let batches = value.get("batches")?.as_array()?;
    let mut result: Vec<(i64, Vec<String>)> = Vec::new();

    for batch in batches {
        let order = batch.get("order").and_then(|v| v.as_i64()).unwrap_or(0);
        let items = batch
            .get("items")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        result.push((order, items));
    }

    result.sort_by_key(|(order, _)| *order);
    Some(result.into_iter().map(|(_, items)| items).collect())
}

fn format_subtasks(subtasks: &[Subtask]) -> String {
    subtasks
        .iter()
        .map(|s| {
            let deps = if s.depends_on.is_empty() {
                "none".to_string()
            } else {
                s.depends_on
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!(
                "- id: {}\n  title: {}\n  complexity: {:?}\n  depends_on: {}\n  files: {}",
                s.id,
                s.title,
                s.complexity,
                deps,
                s.files.join(", "),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract a JSON object from text that may contain markdown fences.
fn extract_json_block(text: &str) -> Option<&str> {
    // Try to find ```json ... ``` block first.
    if let Some(start) = text.find("```json") {
        let after_fence = &text[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return Some(after_fence[..end].trim());
        }
    }
    // Try bare ``` block.
    if let Some(start) = text.find("```") {
        let after_fence = &text[start + 3..];
        if let Some(end) = after_fence.find("```") {
            let inner = after_fence[..end].trim();
            if inner.starts_with('{') {
                return Some(inner);
            }
        }
    }
    // Try to find a bare JSON object.
    let trimmed = text.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::SubtaskId;
    use surge_core::spec::{Complexity, SubtaskExecution};

    fn make_subtask(title: &str, complexity: Complexity) -> Subtask {
        Subtask {
            id: SubtaskId::new(),
            title: title.to_string(),
            description: String::new(),
            complexity,
            files: vec!["src/main.rs".to_string()],
            acceptance_criteria: vec![],
            depends_on: vec![],
            story_file: None,
            agent: None,
            execution: SubtaskExecution::default(),
        }
    }

    #[test]
    fn test_build_schedule_prompt_contains_tasks() {
        let subtasks = vec![
            make_subtask("Setup DB", Complexity::Standard),
            make_subtask("Add API", Complexity::Complex),
        ];
        let prompt = build_schedule_prompt(&subtasks, "Build a REST API");

        assert!(prompt.contains("Setup DB"));
        assert!(prompt.contains("Add API"));
        assert!(prompt.contains("Build a REST API"));
        assert!(prompt.contains("batches"));
    }

    #[test]
    fn test_parse_schedule_response_valid_json() {
        let response = r#"{"batches": [
            {"order": 1, "items": ["id-b"], "reason": "depends on A"},
            {"order": 0, "items": ["id-a", "id-c"], "reason": "independent"}
        ]}"#;

        let batches = parse_schedule_response(response).unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec!["id-a", "id-c"]); // order 0 first
        assert_eq!(batches[1], vec!["id-b"]);
    }

    #[test]
    fn test_parse_schedule_response_with_fences() {
        let response = "Here is the schedule:\n```json\n{\"batches\": [{\"order\": 0, \"items\": [\"x\"]}]}\n```";
        let batches = parse_schedule_response(response).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0], vec!["x"]);
    }

    #[test]
    fn test_parse_schedule_response_invalid() {
        assert!(parse_schedule_response("not json at all").is_none());
        assert!(parse_schedule_response("{}").is_none());
    }

    #[test]
    fn test_extract_json_block_bare() {
        let text = r#"  {"key": "value"}  "#;
        assert_eq!(extract_json_block(text), Some(r#"{"key": "value"}"#));
    }

    #[test]
    fn test_extract_json_block_fenced() {
        let text = "text\n```json\n{\"a\": 1}\n```\nmore";
        assert_eq!(extract_json_block(text), Some("{\"a\": 1}"));
    }
}
