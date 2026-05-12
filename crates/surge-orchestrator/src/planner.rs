//! Planner phases — generates requirements and stories from user description.

use std::fs;
use std::path::Path;

use agent_client_protocol::{ContentBlock, TextContent};
use surge_acp::pool::{AgentPool, SessionHandle};
use surge_core::SurgeError;
use surge_core::event::SurgeEvent;
use surge_core::spec::{AcceptanceCriteria, Complexity, Subtask};
use surge_spec::SpecFile;
use tracing::info;

/// Stateless planner — builds prompts and parses story files for phases 1 & 2.
pub struct PlannerPhase;

impl PlannerPhase {
    // ── Phase 1: Spec Creation ──────────────────────────────────────────

    /// Build the prompt that asks an agent to generate `requirements.md`.
    #[must_use]
    pub fn build_requirements_prompt(user_description: &str, worktree_root: &Path) -> String {
        let project_structure = Self::read_project_structure(worktree_root);

        format!(
            r#"You are a product analyst. Create a requirements document
for the following feature request.

## Project Structure
{project_structure}

## Feature Request
{user_description}

## Your Task

Write `requirements.md` with these sections:

### Overview
One paragraph: what this feature does and why it's needed.

### User Stories
Format: "As a [user], I want to [action] so that [benefit]"
Maximum 5 user stories. Focus on the most important ones.

### Functional Requirements
- FR-001: System MUST [specific, testable requirement]
- FR-002: System MUST [specific, testable requirement]
Use MUST/SHOULD/MAY for priority.

### Success Criteria
Measurable outcomes:
- SC-001: [metric, e.g. "Users can complete X in under 2 minutes"]

### Out of Scope
What is explicitly NOT part of this feature.

## Rules
- Write WHAT and WHY, never HOW (no tech stack, no code, no architecture)
- Every requirement must be testable
- Be specific, not vague ("The system shall be fast" is NOT acceptable)
- If the request is too large, say so and suggest splitting
"#
        )
    }

    // ── Phase 2: Planning ───────────────────────────────────────────────

    /// Build the prompt that asks an agent to generate `architecture.md` + stories.
    #[must_use]
    pub fn build_planning_prompt(
        requirements: &str,
        worktree_root: &Path,
        spec_id: &str,
    ) -> String {
        let project_structure = Self::read_project_structure(worktree_root);
        let existing_patterns = Self::read_existing_patterns(worktree_root);

        format!(
            r#"You are a software architect. Create an implementation plan
for the following requirements.

## Project Structure
{project_structure}

## Existing Patterns
{existing_patterns}

## Requirements
{requirements}

## Your Task

### Step 1: Write `architecture.md`

Sections to include:
- **Tech Stack Decisions**: what libraries/crates to use and why
- **Data Model**: key types, structs, database schema changes
- **API Contracts**: endpoints, request/response formats
- **Key Design Decisions**: architectural choices with reasoning

### Step 2: Write story files

For each story, create `stories/story-NNN.md` with this format:

```
# Story NNN: [Title]

## Context
[What was done in previous stories that this builds on]

## What needs to be done
[Specific description of this story's scope]

## Architecture decisions (from architecture.md)
[Relevant decisions from architecture.md that apply here]

## Files to modify
- `path/to/file.rs` ← [what to do here]

## Reference: existing pattern
[Point to existing code that shows the pattern to follow]

## Acceptance criteria
- [ ] [specific, testable criterion]

## Out of scope
[What this story explicitly does NOT do]
```

### Step 3: Update spec.toml subtasks

For each story, add a subtask entry:

```toml
[[subtasks]]
title = "Story title"
story_file = "stories/story-001.md"
files = ["path/to/file.rs"]
complexity = "simple|standard|complex"

[[subtasks.acceptance_criteria]]
description = "criterion text"
```

## Rules
- Maximum 7 stories total
- Each story completable in one focused coding session
- Stories must be ordered by dependency (story B that depends on A comes after)
- Independent stories get [P] marker in story file header for parallel execution
- Files must be specific paths, not directories
- Each story has at least one acceptance criterion
- Do NOT split "write tests" into separate stories — tests belong in each story
- Architecture decisions in each story must reference architecture.md

## Spec ID
{spec_id}
"#
        )
    }

    // ── Phase 1 execution ───────────────────────────────────────────────

    /// Run phase 1: create `requirements.md` from user description.
    ///
    /// The agent is expected to create the file via ACP tools. If it doesn't,
    /// we capture the response text from events and write it as a fallback.
    pub async fn create_requirements(
        spec_dir: &Path,
        user_description: &str,
        pool: &AgentPool,
        session: &SessionHandle,
        worktree_root: &Path,
    ) -> Result<(), SurgeError> {
        let prompt = Self::build_requirements_prompt(user_description, worktree_root);
        let content = vec![ContentBlock::Text(TextContent::new(prompt))];

        // Subscribe before prompt to capture AgentMessageChunk events.
        let mut event_rx = pool.subscribe();

        pool.prompt(session, content).await?;

        // Agent may have created the file via ACP tools. If not, write captured text.
        let req_path = spec_dir.join("requirements.md");
        if !req_path.exists() {
            let mut text = String::new();
            while let Ok(event) = event_rx.try_recv() {
                if let SurgeEvent::AgentMessageChunk { text: chunk, .. } = event {
                    text.push_str(&chunk);
                }
            }
            if !text.is_empty() {
                fs::write(&req_path, text).map_err(|e| {
                    SurgeError::Spec(format!("Failed to write requirements.md: {e}"))
                })?;
            }
        }

        info!("requirements.md created at {}", req_path.display());
        Ok(())
    }

    // ── Phase 2 execution ───────────────────────────────────────────────

    /// Run phase 2: create `architecture.md` + stories, populate subtasks.
    pub async fn create_plan(
        spec_file: &mut SpecFile,
        spec_dir: &Path,
        pool: &AgentPool,
        session: &SessionHandle,
        worktree_root: &Path,
    ) -> Result<(), SurgeError> {
        let req_path = spec_dir.join("requirements.md");
        let requirements = fs::read_to_string(&req_path)
            .map_err(|e| SurgeError::Spec(format!("requirements.md not found: {e}")))?;

        let spec_id = spec_file.spec.id.to_string();
        let prompt = Self::build_planning_prompt(&requirements, worktree_root, &spec_id);
        let content = vec![ContentBlock::Text(TextContent::new(prompt))];

        // Agent creates architecture.md and stories/*.md via ACP file tools.
        pool.prompt(session, content).await?;

        // Read story files and populate subtasks in spec.toml.
        let stories_dir = spec_dir.join("stories");
        if stories_dir.exists() {
            spec_file.spec.subtasks = Self::load_subtasks_from_stories(&stories_dir)?;
            if let Some(path) = &spec_file.path {
                spec_file.save(path)?;
            }
            info!("plan created: {} stories", spec_file.spec.subtasks.len());
        }

        Ok(())
    }

    // ── Story parsing ───────────────────────────────────────────────────

    /// Read story files from a directory and convert them to subtasks.
    fn load_subtasks_from_stories(stories_dir: &Path) -> Result<Vec<Subtask>, SurgeError> {
        let mut stories: Vec<_> = fs::read_dir(stories_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .collect();

        // Sort by filename (story-001, story-002, ...).
        stories.sort_by_key(|e| e.path());

        let mut subtasks = vec![];
        for entry in stories {
            let path = entry.path();
            let content = fs::read_to_string(&path)?;
            let subtask = Self::parse_story_to_subtask(&path, &content)?;
            subtasks.push(subtask);
        }

        Ok(subtasks)
    }

    /// Parse a `story-NNN.md` file into a `Subtask`.
    fn parse_story_to_subtask(path: &Path, content: &str) -> Result<Subtask, SurgeError> {
        // Title from first heading: "# Story 002: Add JWT..."
        let title = content
            .lines()
            .find(|l| l.starts_with("# Story"))
            .map(|l| {
                l.trim_start_matches("# Story ")
                    .trim()
                    .split_once(": ")
                    .map(|(_, title)| title)
                    .unwrap_or(l.trim_start_matches("# ").trim())
            })
            .unwrap_or("Untitled story")
            .to_string();

        let files = Self::extract_files_section(content);
        let acceptance_criteria = Self::extract_criteria_section(content);

        let story_file = path
            .file_name()
            .map(|n| format!("stories/{}", n.to_string_lossy()))
            .unwrap_or_default();

        let mut subtask = Subtask::new(title, "", Complexity::Standard);
        subtask.files = files;
        subtask.acceptance_criteria = acceptance_criteria;
        subtask.story_file = Some(story_file);
        Ok(subtask)
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    /// Read the first two levels of the project directory, excluding build artefacts.
    fn read_project_structure(root: &Path) -> String {
        let exclude = ["target", ".git", "node_modules", ".surge"];
        let mut lines = vec![];

        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if exclude.contains(&name.as_str()) {
                    continue;
                }
                lines.push(name);
            }
        }

        lines.sort();
        lines.join("\n")
    }

    /// Read CLAUDE.md or AGENTS.md for existing patterns.
    fn read_existing_patterns(root: &Path) -> String {
        for name in &["CLAUDE.md", "AGENTS.md"] {
            if let Ok(content) = fs::read_to_string(root.join(name)) {
                return content.chars().take(3000).collect();
            }
        }
        "(no CLAUDE.md found)".to_string()
    }

    /// Parse `## Files to modify` section for file paths.
    ///
    /// Expects lines like: `- \`path/to/file.rs\` ← description`
    fn extract_files_section(content: &str) -> Vec<String> {
        let mut files = vec![];
        let mut in_section = false;

        for line in content.lines() {
            if line.starts_with("## Files to modify") {
                in_section = true;
                continue;
            }
            if in_section && line.starts_with("## ") {
                break;
            }
            if in_section
                && line.starts_with("- ")
                && let Some(start) = line.find('`')
                && let Some(end) = line[start + 1..].find('`')
            {
                files.push(line[start + 1..start + 1 + end].to_string());
            }
        }

        files
    }

    /// Parse `## Acceptance criteria` section for checkboxes.
    ///
    /// Expects lines like: `- [ ] criterion text`
    fn extract_criteria_section(content: &str) -> Vec<AcceptanceCriteria> {
        let mut criteria = vec![];
        let mut in_section = false;

        for line in content.lines() {
            if line.starts_with("## Acceptance criteria") {
                in_section = true;
                continue;
            }
            if in_section && line.starts_with("## ") {
                break;
            }
            if in_section && line.starts_with("- [ ] ") {
                let desc = line.trim_start_matches("- [ ] ").trim().to_string();
                if !desc.is_empty() {
                    criteria.push(AcceptanceCriteria::new(desc));
                }
            }
        }

        criteria
    }

    // ── Interactive clarification ───────────────────────────────────

    /// Build a prompt that asks an agent to generate clarifying questions
    /// before creating requirements.
    ///
    /// Used for complex features where the user description may be ambiguous.
    /// The agent returns 2-3 focused questions; answers are fed into phase 1.
    #[must_use]
    pub fn build_clarification_prompt(user_description: &str, worktree_root: &Path) -> String {
        let project_structure = Self::read_project_structure(worktree_root);

        format!(
            r#"You are a product analyst. Before writing requirements, you need
to clarify the feature request with the user.

## Project Structure
{project_structure}

## Feature Request
{user_description}

## Your Task

Generate 2-3 clarifying questions that would help you write better
requirements. Focus on:

1. **Scope ambiguity** — what's included vs excluded
2. **User impact** — who benefits and how
3. **Constraints** — performance, compatibility, deadlines

## Output Format

Return ONLY a numbered list of questions, nothing else:
1. [question]
2. [question]
3. [question]
"#
        )
    }

    /// Parse clarifying questions from agent response.
    ///
    /// Extracts numbered items (1. ... , 2. ... , etc.).
    #[must_use]
    pub fn parse_clarification_questions(response: &str) -> Vec<String> {
        response
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                // Match lines like "1. question" or "1) question"
                if trimmed.len() > 2
                    && trimmed.as_bytes()[0].is_ascii_digit()
                    && (trimmed.as_bytes()[1] == b'.' || trimmed.as_bytes()[1] == b')')
                {
                    Some(trimmed[2..].trim().to_string())
                } else {
                    None
                }
            })
            .filter(|q| !q.is_empty())
            .collect()
    }

    /// Build an enriched requirements prompt that includes user answers to
    /// clarifying questions.
    #[must_use]
    pub fn build_requirements_prompt_with_answers(
        user_description: &str,
        qa_pairs: &[(String, String)],
        worktree_root: &Path,
    ) -> String {
        let mut base = Self::build_requirements_prompt(user_description, worktree_root);

        if !qa_pairs.is_empty() {
            base.push_str("\n## Clarifications from the user\n\n");
            for (question, answer) in qa_pairs {
                base.push_str(&format!("**Q:** {question}\n**A:** {answer}\n\n"));
            }
        }

        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_build_requirements_prompt_contains_key_sections() {
        let prompt =
            PlannerPhase::build_requirements_prompt("Add JWT auth", &PathBuf::from("/tmp/fake"));

        assert!(
            prompt.contains("Add JWT auth"),
            "should contain user description"
        );
        assert!(
            prompt.contains("### Overview"),
            "should have Overview section"
        );
        assert!(
            prompt.contains("### User Stories"),
            "should have User Stories section"
        );
        assert!(
            prompt.contains("### Functional Requirements"),
            "should have FR section"
        );
        assert!(
            prompt.contains("### Success Criteria"),
            "should have SC section"
        );
        assert!(
            prompt.contains("### Out of Scope"),
            "should have Out of Scope section"
        );
        assert!(
            prompt.contains("WHAT and WHY, never HOW"),
            "should instruct no implementation details"
        );
    }

    #[test]
    fn test_build_planning_prompt_contains_story_format() {
        let prompt = PlannerPhase::build_planning_prompt(
            "## Requirements\n- FR-001: must do X",
            &PathBuf::from("/tmp/fake"),
            "spec_123",
        );

        assert!(
            prompt.contains("stories/story-NNN.md"),
            "should describe story file format"
        );
        assert!(
            prompt.contains("## Acceptance criteria"),
            "should have criteria section in template"
        );
        assert!(
            prompt.contains("## Files to modify"),
            "should have files section in template"
        );
        assert!(
            prompt.contains("Maximum 7 stories"),
            "should limit story count"
        );
        assert!(prompt.contains("spec_123"), "should include spec ID");
    }

    #[test]
    fn test_parse_story_to_subtask_extracts_title() {
        let content = "# Story 002: Add JWT authentication middleware\n\nSome body text\n";
        let subtask =
            PlannerPhase::parse_story_to_subtask(&PathBuf::from("stories/story-002.md"), content)
                .unwrap();

        assert_eq!(subtask.title, "Add JWT authentication middleware");
        assert_eq!(subtask.story_file.as_deref(), Some("stories/story-002.md"));
    }

    #[test]
    fn test_parse_story_to_subtask_extracts_files() {
        let content = "\
# Story 001: Setup

## Files to modify
- `crates/surge-api/src/middleware/auth.rs` ← create this file
- `crates/surge-api/src/router.rs` ← apply middleware

## Other section
";
        let subtask =
            PlannerPhase::parse_story_to_subtask(&PathBuf::from("stories/story-001.md"), content)
                .unwrap();

        assert_eq!(subtask.files.len(), 2);
        assert_eq!(subtask.files[0], "crates/surge-api/src/middleware/auth.rs");
        assert_eq!(subtask.files[1], "crates/surge-api/src/router.rs");
    }

    #[test]
    fn test_parse_story_to_subtask_extracts_criteria() {
        let content = "\
# Story 001: Setup

## Acceptance criteria
- [ ] GET /api/agents returns 401 without Authorization header
- [ ] GET /api/agents returns 200 with valid token

## Out of scope
";
        let subtask =
            PlannerPhase::parse_story_to_subtask(&PathBuf::from("stories/story-001.md"), content)
                .unwrap();

        assert_eq!(subtask.acceptance_criteria.len(), 2);
        assert_eq!(
            subtask.acceptance_criteria[0].description,
            "GET /api/agents returns 401 without Authorization header"
        );
        assert!(!subtask.acceptance_criteria[0].met);
    }

    #[test]
    fn test_load_subtasks_from_stories_sorts_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let stories_dir = dir.path().join("stories");
        fs::create_dir_all(&stories_dir).unwrap();

        // Create out of order
        fs::write(
            stories_dir.join("story-002.md"),
            "# Story 002: Second\n\n## Acceptance criteria\n- [ ] works\n",
        )
        .unwrap();
        fs::write(
            stories_dir.join("story-001.md"),
            "# Story 001: First\n\n## Acceptance criteria\n- [ ] works\n",
        )
        .unwrap();

        let subtasks = PlannerPhase::load_subtasks_from_stories(&stories_dir).unwrap();

        assert_eq!(subtasks.len(), 2);
        assert_eq!(subtasks[0].title, "First");
        assert_eq!(subtasks[1].title, "Second");
    }

    #[test]
    fn test_read_project_structure_excludes_target() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::create_dir(dir.path().join("target")).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();

        let structure = PlannerPhase::read_project_structure(dir.path());

        assert!(structure.contains("src"));
        assert!(!structure.contains("target"));
        assert!(!structure.contains(".git"));
    }

    #[test]
    fn test_build_clarification_prompt_contains_key_parts() {
        let prompt =
            PlannerPhase::build_clarification_prompt("Add user auth", &PathBuf::from("/tmp/fake"));

        assert!(prompt.contains("Add user auth"));
        assert!(prompt.contains("Scope ambiguity"));
        assert!(prompt.contains("2-3 clarifying questions"));
    }

    #[test]
    fn test_parse_clarification_questions() {
        let response = "Sure, here are my questions:\n\
            1. Should we support OAuth or just password auth?\n\
            2. What user roles are needed?\n\
            3. Is there a deadline for this feature?\n";

        let questions = PlannerPhase::parse_clarification_questions(response);
        assert_eq!(questions.len(), 3);
        assert!(questions[0].contains("OAuth"));
        assert!(questions[1].contains("roles"));
        assert!(questions[2].contains("deadline"));
    }

    #[test]
    fn test_parse_clarification_questions_parenthesis_format() {
        let response = "1) First question?\n2) Second question?\n";
        let questions = PlannerPhase::parse_clarification_questions(response);
        assert_eq!(questions.len(), 2);
    }

    #[test]
    fn test_parse_clarification_questions_empty() {
        let questions = PlannerPhase::parse_clarification_questions("No questions here.");
        assert!(questions.is_empty());
    }

    #[test]
    fn test_build_requirements_prompt_with_answers() {
        let qa_pairs = vec![
            ("OAuth or password?".to_string(), "OAuth only".to_string()),
            ("Deadline?".to_string(), "End of Q1".to_string()),
        ];
        let prompt = PlannerPhase::build_requirements_prompt_with_answers(
            "Add auth",
            &qa_pairs,
            &PathBuf::from("/tmp/fake"),
        );

        assert!(prompt.contains("Add auth"));
        assert!(prompt.contains("Clarifications from the user"));
        assert!(prompt.contains("OAuth or password?"));
        assert!(prompt.contains("OAuth only"));
        assert!(prompt.contains("End of Q1"));
    }
}
