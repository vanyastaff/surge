//! Roadmap document parsing and rendering.
//!
//! Roadmap planning works with structured [`RoadmapArtifact`] values, while
//! users usually edit `.ai-factory/ROADMAP.md`. This module is the boundary
//! between those formats so command code does not need to guess whether a
//! roadmap is TOML or Markdown.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use surge_core::roadmap::{RoadmapArtifact, RoadmapMilestone, RoadmapStatus, RoadmapTask};
use surge_core::roadmap_patch::{RoadmapItemRef, RoadmapPatchApplyResult};
use thiserror::Error;

/// Format detected for a roadmap document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoadmapDocumentFormat {
    /// Structured TOML roadmap artifact.
    Toml,
    /// Human-authored Markdown roadmap.
    Markdown,
}

/// Parsed roadmap document plus enough metadata to render amendments back.
#[derive(Debug, Clone)]
pub struct ParsedRoadmapDocument {
    /// Structured roadmap used by amendment planning and patch application.
    pub roadmap: RoadmapArtifact,
    /// Original document format.
    pub format: RoadmapDocumentFormat,
    markdown: Option<MarkdownRoadmapDocument>,
}

/// Errors while parsing or rendering roadmap documents.
#[derive(Debug, Error)]
pub enum RoadmapDocumentError {
    /// TOML roadmap failed to parse.
    #[error("failed to parse TOML roadmap: {0}")]
    TomlParse(#[from] toml::de::Error),
    /// TOML roadmap failed to render.
    #[error("failed to render TOML roadmap: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    /// Markdown replacement patches are not supported without stable IDs.
    #[error(
        "markdown roadmap replacements require explicit stable IDs; apply this patch as a follow-up run instead"
    )]
    UnsupportedMarkdownReplacement,
    /// Markdown insertion rendering cannot preserve a non-append placement.
    #[error(
        "markdown roadmap cannot preserve insertion order for {item}; apply this patch as a follow-up run instead"
    )]
    UnsupportedMarkdownInsertionOrder {
        /// Item whose structured placement cannot be rendered safely.
        item: String,
    },
}

#[derive(Debug, Clone)]
struct MarkdownRoadmapDocument {
    milestones_section_end: usize,
    milestones: BTreeMap<String, MarkdownMilestoneLine>,
}

#[derive(Debug, Clone)]
struct MarkdownMilestoneLine {
    end_line_index: usize,
}

/// Parses a roadmap document from TOML or Markdown.
pub fn parse_roadmap_document(
    path: &Path,
    content: &str,
) -> Result<ParsedRoadmapDocument, RoadmapDocumentError> {
    if is_toml_path(path) {
        return Ok(ParsedRoadmapDocument {
            roadmap: toml::from_str(content)?,
            format: RoadmapDocumentFormat::Toml,
            markdown: None,
        });
    }

    Ok(parse_markdown_roadmap(content))
}

/// Renders an amended roadmap document in the same format as the original.
pub fn render_amended_roadmap_document(
    path: &Path,
    original_content: &str,
    parsed: &ParsedRoadmapDocument,
    patch_result: &RoadmapPatchApplyResult,
) -> Result<String, RoadmapDocumentError> {
    match parsed.format {
        RoadmapDocumentFormat::Toml => {
            let _ = path;
            toml::to_string_pretty(&patch_result.roadmap).map_err(RoadmapDocumentError::from)
        },
        RoadmapDocumentFormat::Markdown => {
            render_markdown_amendment(original_content, parsed, patch_result)
        },
    }
}

/// Human-readable structured identifiers for feature-planner prompts.
pub fn roadmap_identifiers_prompt(document: &ParsedRoadmapDocument) -> String {
    let mut output = String::from("\n\n## Surge roadmap identifiers\n\n");
    output.push_str("Use these stable IDs when creating roadmap patches:\n");

    for milestone in &document.roadmap.milestones {
        output.push_str(&format!(
            "- `{}`: {} ({})\n",
            milestone.id, milestone.title, milestone.status
        ));
        for task in &milestone.tasks {
            output.push_str(&format!(
                "  - `{}`: {} ({})\n",
                task.id, task.title, task.status
            ));
        }
    }

    output
}

fn is_toml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
}

fn parse_markdown_roadmap(content: &str) -> ParsedRoadmapDocument {
    let lines: Vec<&str> = content.lines().collect();
    let (section_start, section_end) = find_milestones_section(&lines);
    let mut used_ids = BTreeSet::new();
    let mut roadmap = RoadmapArtifact::new(Vec::new());
    let mut parsed_milestones = Vec::new();
    let mut current_milestone: Option<usize> = None;

    for (line_index, line) in lines
        .iter()
        .enumerate()
        .take(section_end)
        .skip(section_start)
    {
        let line = *line;
        if let Some(parsed) = parse_markdown_item(line, MarkdownItemKind::Milestone) {
            let id = unique_slug(&parsed.title, &mut used_ids);
            let mut milestone = RoadmapMilestone::new(id.clone(), parsed.title);
            milestone.status = parsed.status;
            roadmap.milestones.push(milestone);
            parsed_milestones.push((id, line_index));
            current_milestone = Some(roadmap.milestones.len() - 1);
            continue;
        }

        if let Some(milestone_index) = current_milestone
            && let Some(parsed) = parse_markdown_item(line, MarkdownItemKind::Task)
        {
            let milestone_id = roadmap.milestones[milestone_index].id.clone();
            let task_id = format!(
                "{}-task-{}",
                milestone_id,
                roadmap.milestones[milestone_index].tasks.len() + 1
            );
            let mut task = RoadmapTask::new(task_id, parsed.title);
            task.status = parsed.status;
            task.description = (!parsed.description.is_empty()).then_some(parsed.description);
            roadmap.milestones[milestone_index].tasks.push(task);
        }
    }

    let mut milestones = BTreeMap::new();
    for (index, (id, start_line_index)) in parsed_milestones.iter().enumerate() {
        let end_line_index = parsed_milestones
            .get(index + 1)
            .map(|(_, next_line_index)| *next_line_index)
            .unwrap_or(section_end);
        milestones.insert(id.clone(), MarkdownMilestoneLine { end_line_index });
        let _ = start_line_index;
    }

    ParsedRoadmapDocument {
        roadmap,
        format: RoadmapDocumentFormat::Markdown,
        markdown: Some(MarkdownRoadmapDocument {
            milestones_section_end: section_end,
            milestones,
        }),
    }
}

fn find_milestones_section(lines: &[&str]) -> (usize, usize) {
    let Some(heading_index) = lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case("## Milestones"))
    else {
        return (lines.len(), lines.len());
    };

    let section_start = heading_index + 1;
    let section_end = lines
        .iter()
        .enumerate()
        .skip(section_start)
        .find_map(|(index, line)| line.trim_start().starts_with("## ").then_some(index))
        .unwrap_or(lines.len());

    (section_start, section_end)
}

#[derive(Debug, Clone, Copy)]
enum MarkdownItemKind {
    Milestone,
    Task,
}

#[derive(Debug, Clone)]
struct ParsedMarkdownItem {
    title: String,
    description: String,
    status: RoadmapStatus,
}

fn parse_markdown_item(line: &str, kind: MarkdownItemKind) -> Option<ParsedMarkdownItem> {
    match kind {
        MarkdownItemKind::Milestone if !line.starts_with("- ") => return None,
        MarkdownItemKind::Task if !line.starts_with("  - ") && !line.starts_with("\t- ") => {
            return None;
        },
        _ => {},
    }

    let mut text = line.trim_start().strip_prefix("- ")?.trim_start();
    let mut status = RoadmapStatus::Pending;

    if let Some(rest) = text.strip_prefix('[') {
        let marker = rest.chars().next()?;
        let after_marker = rest.get(marker.len_utf8()..)?;
        if let Some(after_checkbox) = after_marker.strip_prefix("] ") {
            status = match marker {
                'x' | 'X' => RoadmapStatus::Completed,
                '~' => RoadmapStatus::Running,
                _ => RoadmapStatus::Pending,
            };
            text = after_checkbox.trim_start();
        }
    }

    let (title, description) = parse_title_and_description(text);
    (!title.is_empty()).then_some(ParsedMarkdownItem {
        title,
        description,
        status,
    })
}

fn parse_title_and_description(text: &str) -> (String, String) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("**")
        && let Some(end_index) = rest.find("**")
    {
        let title = rest[..end_index].trim().to_string();
        let description = rest[end_index + 2..]
            .trim_start_matches(|ch: char| ch.is_whitespace() || ch == '-' || ch == '\u{2014}')
            .trim()
            .to_string();
        return (title, description);
    }

    if let Some(separator) = find_title_separator(text) {
        return (
            text[..separator].trim().to_string(),
            text[separator..]
                .trim_start_matches(|ch: char| ch.is_whitespace() || ch == '-' || ch == '\u{2014}')
                .trim()
                .to_string(),
        );
    }

    (text.to_string(), String::new())
}

fn find_title_separator(text: &str) -> Option<usize> {
    [" - ", " -- "]
        .iter()
        .filter_map(|separator| text.find(separator))
        .chain(text.find('\u{2014}'))
        .min()
}

fn unique_slug(title: &str, used_ids: &mut BTreeSet<String>) -> String {
    let base = slugify(title);
    if used_ids.insert(base.clone()) {
        return base;
    }

    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if used_ids.insert(candidate.clone()) {
            return candidate;
        }
    }

    unreachable!("unbounded suffix search always returns");
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut pending_separator = false;

    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(ch.to_ascii_lowercase());
            pending_separator = false;
        } else {
            pending_separator = true;
        }
    }

    if slug.is_empty() {
        "milestone".to_string()
    } else {
        slug
    }
}

fn render_markdown_amendment(
    original_content: &str,
    parsed: &ParsedRoadmapDocument,
    patch_result: &RoadmapPatchApplyResult,
) -> Result<String, RoadmapDocumentError> {
    if !patch_result.replaced_items.is_empty() {
        return Err(RoadmapDocumentError::UnsupportedMarkdownReplacement);
    }
    validate_markdown_append_only_insertions(parsed, patch_result)?;

    let Some(markdown) = parsed.markdown.as_ref() else {
        return Ok(patch_result.markdown.clone());
    };

    let mut lines: Vec<String> = original_content.lines().map(ToString::to_string).collect();
    let mut insertions: BTreeMap<usize, Vec<String>> = BTreeMap::new();

    if !patch_result.inserted_milestones.is_empty() {
        let mut block = Vec::new();
        for milestone_id in &patch_result.inserted_milestones {
            let Some(milestone) = find_milestone(&patch_result.roadmap, milestone_id) else {
                continue;
            };
            if !block.is_empty() {
                block.push(String::new());
            }
            block.extend(render_milestone_block(milestone));
        }
        insertions
            .entry(markdown.milestones_section_end)
            .or_default()
            .extend(block);
    }

    let inserted_milestone_ids: BTreeSet<&str> = patch_result
        .inserted_milestones
        .iter()
        .map(String::as_str)
        .collect();

    for task_ref in &patch_result.inserted_tasks {
        let RoadmapItemRef::Task {
            milestone_id,
            task_id,
        } = task_ref
        else {
            continue;
        };
        if inserted_milestone_ids.contains(milestone_id.as_str()) {
            continue;
        }
        let Some(location) = markdown.milestones.get(milestone_id.as_str()) else {
            continue;
        };
        let Some(task) = find_task(
            &patch_result.roadmap,
            milestone_id.as_str(),
            task_id.as_str(),
        ) else {
            continue;
        };
        insertions
            .entry(location.end_line_index)
            .or_default()
            .push(render_task_line(task));
    }

    for (index, block) in insertions.into_iter().rev() {
        let insertion_index = index.min(lines.len());
        lines.splice(insertion_index..insertion_index, block);
    }

    let mut rendered = lines.join("\n");
    if original_content.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn validate_markdown_append_only_insertions(
    parsed: &ParsedRoadmapDocument,
    patch_result: &RoadmapPatchApplyResult,
) -> Result<(), RoadmapDocumentError> {
    let inserted_milestones: BTreeSet<&str> = patch_result
        .inserted_milestones
        .iter()
        .map(String::as_str)
        .collect();
    let inserted_tasks: BTreeSet<(&str, &str)> = patch_result
        .inserted_tasks
        .iter()
        .filter_map(|item| match item {
            RoadmapItemRef::Task {
                milestone_id,
                task_id,
            } => Some((milestone_id.as_str(), task_id.as_str())),
            RoadmapItemRef::Milestone { .. } => None,
        })
        .collect();

    reject_non_append_milestones(&patch_result.roadmap, &inserted_milestones)?;
    reject_non_append_tasks(&parsed.roadmap, &patch_result.roadmap, &inserted_tasks)
}

fn reject_non_append_milestones(
    roadmap: &RoadmapArtifact,
    inserted_milestones: &BTreeSet<&str>,
) -> Result<(), RoadmapDocumentError> {
    let mut saw_inserted = false;
    for milestone in &roadmap.milestones {
        if inserted_milestones.contains(milestone.id.as_str()) {
            saw_inserted = true;
            continue;
        }
        if saw_inserted {
            return Err(RoadmapDocumentError::UnsupportedMarkdownInsertionOrder {
                item: format!("milestone {}", milestone.id),
            });
        }
    }
    Ok(())
}

fn reject_non_append_tasks(
    original: &RoadmapArtifact,
    amended: &RoadmapArtifact,
    inserted_tasks: &BTreeSet<(&str, &str)>,
) -> Result<(), RoadmapDocumentError> {
    for original_milestone in &original.milestones {
        let Some(amended_milestone) = find_milestone(amended, &original_milestone.id) else {
            continue;
        };
        let mut saw_inserted = false;
        for task in &amended_milestone.tasks {
            let is_inserted =
                inserted_tasks.contains(&(original_milestone.id.as_str(), task.id.as_str()));
            if is_inserted {
                saw_inserted = true;
                continue;
            }
            if saw_inserted {
                return Err(RoadmapDocumentError::UnsupportedMarkdownInsertionOrder {
                    item: format!("task {}/{}", original_milestone.id, task.id),
                });
            }
        }
    }
    Ok(())
}

fn render_milestone_block(milestone: &RoadmapMilestone) -> Vec<String> {
    let mut block = vec![format!(
        "- [{}] **{}**",
        checkbox_marker(milestone.status),
        milestone.title
    )];
    block.extend(milestone.tasks.iter().map(render_task_line));
    block
}

fn find_milestone<'a>(
    roadmap: &'a RoadmapArtifact,
    milestone_id: &str,
) -> Option<&'a RoadmapMilestone> {
    roadmap
        .milestones
        .iter()
        .find(|milestone| milestone.id == milestone_id)
}

fn find_task<'a>(
    roadmap: &'a RoadmapArtifact,
    milestone_id: &str,
    task_id: &str,
) -> Option<&'a RoadmapTask> {
    find_milestone(roadmap, milestone_id)?
        .tasks
        .iter()
        .find(|task| task.id == task_id)
}

fn render_task_line(task: &RoadmapTask) -> String {
    format!(
        "  - [{}] **{}**{}",
        checkbox_marker(task.status),
        task.title,
        render_description_suffix(task.description.as_deref())
    )
}

fn checkbox_marker(status: RoadmapStatus) -> &'static str {
    match status {
        RoadmapStatus::Completed => "x",
        RoadmapStatus::Running => "~",
        RoadmapStatus::Pending
        | RoadmapStatus::Paused
        | RoadmapStatus::Failed
        | RoadmapStatus::Skipped => " ",
    }
}

fn render_description_suffix(description: Option<&str>) -> String {
    let Some(description) = description
        .map(str::trim)
        .filter(|description| !description.is_empty())
    else {
        return String::new();
    };

    format!(" - {description}")
}

#[cfg(test)]
mod tests {
    use surge_core::roadmap::RoadmapStatus;

    use super::*;

    #[test]
    fn parses_markdown_project_roadmap_into_structured_ids() {
        let markdown = r#"# Roadmap

## Milestones

- [x] **Core graph executor** - Existing implementation
  - [x] **Validate graph**
- [ ] **Telegram approvals**
  - [ ] **Gate messages**

## Completed

- Legacy notes
"#;

        let parsed = parse_roadmap_document(Path::new(".ai-factory/ROADMAP.md"), markdown)
            .expect("markdown roadmap should parse");

        assert_eq!(parsed.format, RoadmapDocumentFormat::Markdown);
        assert_eq!(parsed.roadmap.milestones.len(), 2);
        assert_eq!(parsed.roadmap.milestones[0].id, "core-graph-executor");
        assert_eq!(
            parsed.roadmap.milestones[0].status,
            RoadmapStatus::Completed
        );
        assert_eq!(
            parsed.roadmap.milestones[1].tasks[0].id,
            "telegram-approvals-task-1"
        );
    }

    #[test]
    fn renders_markdown_insertions_without_dropping_existing_sections() {
        let markdown = r#"# Roadmap

## Milestones

- [ ] **Core graph executor**

## Completed

- Old milestone
"#;
        let parsed = parse_roadmap_document(Path::new(".ai-factory/ROADMAP.md"), markdown)
            .expect("markdown roadmap should parse");
        let mut roadmap = parsed.roadmap.clone();
        let mut milestone = RoadmapMilestone::new("telegram-approvals", "Telegram approvals");
        milestone.tasks.push(RoadmapTask::new(
            "telegram-approvals-task-1",
            "Send approval request",
        ));
        roadmap.milestones.push(milestone.clone());
        let patch_result = RoadmapPatchApplyResult {
            roadmap,
            markdown: String::new(),
            inserted_milestones: vec![milestone.id],
            inserted_tasks: Vec::new(),
            replaced_items: Vec::new(),
            dependencies_added: Vec::new(),
        };

        let rendered = render_amended_roadmap_document(
            Path::new(".ai-factory/ROADMAP.md"),
            markdown,
            &parsed,
            &patch_result,
        )
        .expect("markdown amendment should render");

        assert!(rendered.contains("- [ ] **Telegram approvals**"));
        assert!(rendered.contains("  - [ ] **Send approval request**"));
        assert!(rendered.contains("## Completed"));
        assert!(rendered.find("Telegram approvals") < rendered.find("## Completed"));
    }

    #[test]
    fn rejects_markdown_milestone_insertion_that_would_reorder_output() {
        let markdown = r#"# Roadmap

## Milestones

- [ ] **Core graph executor**

## Completed
"#;
        let parsed = parse_roadmap_document(Path::new(".ai-factory/ROADMAP.md"), markdown)
            .expect("markdown roadmap should parse");
        let mut roadmap = parsed.roadmap.clone();
        roadmap
            .milestones
            .insert(0, RoadmapMilestone::new("telegram", "Telegram approvals"));
        let patch_result = RoadmapPatchApplyResult {
            roadmap,
            markdown: String::new(),
            inserted_milestones: vec!["telegram".into()],
            inserted_tasks: Vec::new(),
            replaced_items: Vec::new(),
            dependencies_added: Vec::new(),
        };

        let err = render_amended_roadmap_document(
            Path::new(".ai-factory/ROADMAP.md"),
            markdown,
            &parsed,
            &patch_result,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            RoadmapDocumentError::UnsupportedMarkdownInsertionOrder { .. }
        ));
    }

    #[test]
    fn rejects_markdown_task_insertion_that_would_reorder_output() {
        let markdown = r#"# Roadmap

## Milestones

- [ ] **Core graph executor**
  - [ ] **Validate graph**

## Completed
"#;
        let parsed = parse_roadmap_document(Path::new(".ai-factory/ROADMAP.md"), markdown)
            .expect("markdown roadmap should parse");
        let mut roadmap = parsed.roadmap.clone();
        roadmap.milestones[0].tasks.insert(
            0,
            RoadmapTask::new("core-graph-executor-task-new", "New task"),
        );
        let patch_result = RoadmapPatchApplyResult {
            roadmap,
            markdown: String::new(),
            inserted_milestones: Vec::new(),
            inserted_tasks: vec![RoadmapItemRef::Task {
                milestone_id: "core-graph-executor".into(),
                task_id: "core-graph-executor-task-new".into(),
            }],
            replaced_items: Vec::new(),
            dependencies_added: Vec::new(),
        };

        let err = render_amended_roadmap_document(
            Path::new(".ai-factory/ROADMAP.md"),
            markdown,
            &parsed,
            &patch_result,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            RoadmapDocumentError::UnsupportedMarkdownInsertionOrder { .. }
        ));
    }
}
