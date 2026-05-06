//! Computational selection of candidate tickets to feed Triage Author.
//!
//! In MVP we use Jaccard similarity over title+description tokens. RFC-0014
//! replaces this with embedding-based selection.

use crate::types::{TaskDetails, TaskSummary};
use std::collections::HashSet;

/// Return the top-`limit` candidates from `candidates` by Jaccard similarity to `target`.
///
/// Title and summary of each candidate are tokenised (lowercase ASCII alphanumeric,
/// length >= 3) and compared. Candidates with similarity 0 are excluded; the
/// caller's own task (matched by id) is also excluded.
pub fn top_by_keyword_overlap(
    target: &TaskDetails,
    candidates: &[CandidateInput],
    limit: usize,
) -> Vec<ScoredCandidate> {
    let target_tokens = tokenize(&target.title, &target.description);
    if target_tokens.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<ScoredCandidate> = candidates
        .iter()
        .filter(|c| c.task_id != target.task_id.as_str())
        .map(|c| {
            let tokens = tokenize(&c.title, &c.summary);
            let score = jaccard(&target_tokens, &tokens);
            ScoredCandidate {
                task_id: c.task_id.clone(),
                title: c.title.clone(),
                summary: c.summary.clone(),
                score,
            }
        })
        .filter(|c| c.score > 0.0)
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}

fn tokenize(title: &str, body: &str) -> HashSet<String> {
    let combined = format!("{title} {body}");
    combined
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 { 0.0 } else { inter / union }
}

/// Input shape for keyword overlap selection.
///
/// Construct via `from_summary` (when only a title is available) or
/// `from_details` (when a full task body is available).
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateInput {
    pub task_id: String,
    pub title: String,
    pub summary: String,
}

impl CandidateInput {
    /// Build from a `TaskSummary` (no body — `summary` is empty).
    pub fn from_summary(s: &TaskSummary) -> Self {
        Self {
            task_id: s.task_id.as_str().into(),
            title: s.title.clone(),
            summary: String::new(),
        }
    }

    /// Build from a `TaskDetails` (`summary` is the description text).
    pub fn from_details(d: &TaskDetails) -> Self {
        Self {
            task_id: d.task_id.as_str().into(),
            title: d.title.clone(),
            summary: d.description.clone(),
        }
    }
}

/// One ranked candidate with its similarity score.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredCandidate {
    pub task_id: String,
    pub title: String,
    pub summary: String,
    pub score: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskId;
    use chrono::Utc;

    fn details(task_id: &str, title: &str, body: &str) -> TaskDetails {
        TaskDetails {
            task_id: TaskId::try_new(task_id).unwrap(),
            source_id: "test".into(),
            title: title.into(),
            description: body.into(),
            status: "open".into(),
            labels: vec![],
            url: format!("https://example/{task_id}"),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            assignee: None,
            raw_payload: serde_json::json!({}),
        }
    }

    fn cand(task_id: &str, title: &str, summary: &str) -> CandidateInput {
        CandidateInput {
            task_id: task_id.into(),
            title: title.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn top_keeps_only_overlapping() {
        let target = details(
            "github:r#1",
            "Fix parser panic on nested objects",
            "Stack overflow when nesting exceeds 16",
        );
        let cs = vec![
            cand(
                "github:r#2",
                "Parser crash with deep nesting",
                "stack overflow on 20+",
            ),
            cand("github:r#3", "Add new logo", "ui design refresh"),
        ];
        let top = top_by_keyword_overlap(&target, &cs, 5);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].task_id, "github:r#2");
        assert!(top[0].score > 0.0);
    }

    #[test]
    fn excludes_self() {
        let target = details("github:r#1", "Fix bug", "important");
        let cs = vec![cand("github:r#1", "Fix bug", "important")];
        let top = top_by_keyword_overlap(&target, &cs, 5);
        assert!(top.is_empty());
    }

    #[test]
    fn respects_limit() {
        let target = details("github:r#1", "deep nesting parser", "stack overflow");
        let cs: Vec<_> = (10..30)
            .map(|i| {
                cand(
                    &format!("github:r#{i}"),
                    "parser nesting stack overflow problem",
                    "deep stack",
                )
            })
            .collect();
        let top = top_by_keyword_overlap(&target, &cs, 5);
        assert_eq!(top.len(), 5);
    }
}
