//! Conflict resolution — AI-driven merge conflict resolution.

use std::fs;
use std::path::Path;

/// Content of a single merge conflict.
#[derive(Debug, Clone)]
pub struct ConflictContent {
    /// "Our" side of the conflict (target branch).
    pub ours: String,
    /// "Their" side of the conflict (feature branch — usually preferred).
    pub theirs: String,
}

/// Build a prompt that asks an agent to resolve a merge conflict.
///
/// The agent receives both sides and produces the resolved file content.
#[must_use]
pub fn build_resolution_prompt(path: &Path, conflict: &ConflictContent) -> String {
    format!(
        r#"Resolve this git merge conflict in `{}`.

## Our version (target branch)
```
{}
```

## Their version (feature branch — prefer this)
```
{}
```

## Rules
- Produce the final resolved file content
- Preserve functionality from both sides where possible
- Prefer "their" (feature branch) changes when in doubt
- Do NOT include conflict markers (<<<<<<<, =======, >>>>>>>)
- Respond ONLY with the resolved file content, no explanation
"#,
        path.display(),
        conflict.ours,
        conflict.theirs,
    )
}

/// Parse a file with conflict markers into a `ConflictContent`.
///
/// Extracts the `<<<<<<<` / `=======` / `>>>>>>>` sections.
/// Returns `None` if no conflict markers are found.
#[must_use]
pub fn parse_conflict_markers(content: &str) -> Option<ConflictContent> {
    let mut ours = String::new();
    let mut theirs = String::new();

    #[derive(PartialEq)]
    enum State {
        Outside,
        Ours,
        Theirs,
    }

    let mut state = State::Outside;
    let mut found = false;

    for line in content.lines() {
        if line.starts_with("<<<<<<<") {
            state = State::Ours;
            found = true;
            continue;
        }
        if line.starts_with("=======") && state == State::Ours {
            state = State::Theirs;
            continue;
        }
        if line.starts_with(">>>>>>>") && state == State::Theirs {
            state = State::Outside;
            continue;
        }

        match state {
            State::Ours => {
                ours.push_str(line);
                ours.push('\n');
            }
            State::Theirs => {
                theirs.push_str(line);
                theirs.push('\n');
            }
            State::Outside => {}
        }
    }

    if found {
        Some(ConflictContent { ours, theirs })
    } else {
        None
    }
}

/// Read a conflicted file from disk and parse its conflict markers.
pub fn read_conflict(path: &Path) -> Option<ConflictContent> {
    let content = fs::read_to_string(path).ok()?;
    parse_conflict_markers(&content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_build_resolution_prompt_contains_both_sides() {
        let conflict = ConflictContent {
            ours: "fn old() {}".to_string(),
            theirs: "fn new() {}".to_string(),
        };
        let prompt = build_resolution_prompt(&PathBuf::from("src/lib.rs"), &conflict);

        assert!(prompt.contains("src/lib.rs"));
        assert!(prompt.contains("fn old() {}"));
        assert!(prompt.contains("fn new() {}"));
        assert!(prompt.contains("prefer this"));
        assert!(prompt.contains("no explanation"));
    }

    #[test]
    fn test_parse_conflict_markers_simple() {
        let content = "\
before conflict
<<<<<<< HEAD
our line 1
our line 2
=======
their line 1
their line 2
>>>>>>> feature
after conflict
";
        let conflict = parse_conflict_markers(content).unwrap();
        assert!(conflict.ours.contains("our line 1"));
        assert!(conflict.ours.contains("our line 2"));
        assert!(conflict.theirs.contains("their line 1"));
        assert!(conflict.theirs.contains("their line 2"));
    }

    #[test]
    fn test_parse_conflict_markers_no_markers() {
        let content = "clean file\nno conflicts here\n";
        assert!(parse_conflict_markers(content).is_none());
    }

    #[test]
    fn test_parse_conflict_markers_with_branch_names() {
        let content = "\
<<<<<<< HEAD
old code
=======
new code
>>>>>>> auto-claude/feat-123
";
        let conflict = parse_conflict_markers(content).unwrap();
        assert!(conflict.ours.contains("old code"));
        assert!(conflict.theirs.contains("new code"));
    }

    #[test]
    fn test_read_conflict_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conflict.rs");
        fs::write(
            &path,
            "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n",
        )
        .unwrap();

        let conflict = read_conflict(&path).unwrap();
        assert!(conflict.ours.contains("ours"));
        assert!(conflict.theirs.contains("theirs"));
    }

    #[test]
    fn test_read_conflict_missing_file() {
        assert!(read_conflict(&PathBuf::from("/nonexistent/file.rs")).is_none());
    }
}
