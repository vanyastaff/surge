//! Acceptance #14 (spec §10): WorkspaceWriteSandbox stub demonstrates the
//! Sandbox trait surface is sufficient for M4's planned impls.
//!
//! This is NOT a real M4 impl — no OS enforcement, no canonical-path
//! resolution against symlinks. Its job is to lock the trait surface so M4
//! can land additively.

use std::path::{Path, PathBuf};

use surge_acp::bridge::{Sandbox, SandboxDecision};

#[derive(Clone, Debug)]
struct WorkspaceWriteSandbox {
    // Reserved for M4: M4's real impl will use this to bound write calls.
    #[allow(dead_code)]
    worktree_root: PathBuf,
}

impl WorkspaceWriteSandbox {
    fn new(worktree_root: PathBuf) -> Self {
        Self { worktree_root }
    }
}

impl Sandbox for WorkspaceWriteSandbox {
    fn visibility(&self, tool: &str, _mcp_id: Option<&str>) -> SandboxDecision {
        match tool {
            "read_text_file" | "write_text_file" | "list_directory" => SandboxDecision::Allow,
            _ => SandboxDecision::Allow,
        }
    }

    fn allows_tool(&self, tool: &str, _mcp_id: Option<&str>) -> SandboxDecision {
        let _ = tool;
        SandboxDecision::Allow
    }

    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

/// Subclass-style helper that demonstrates the asymmetric path that motivates
/// the visibility/allows_tool split.
#[derive(Clone, Debug)]
struct WorkspaceWriteSandboxWithPath {
    worktree_root: PathBuf,
    requested_path: PathBuf,
}

impl Sandbox for WorkspaceWriteSandboxWithPath {
    fn visibility(&self, _tool: &str, _mcp_id: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn allows_tool(&self, tool: &str, _mcp_id: Option<&str>) -> SandboxDecision {
        if tool == "write_text_file" && path_escapes(&self.worktree_root, &self.requested_path) {
            return SandboxDecision::Deny {
                reason: "path escapes worktree".into(),
            };
        }
        SandboxDecision::Allow
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

fn path_escapes(worktree: &Path, path: &Path) -> bool {
    path.canonicalize()
        .ok()
        .map(|c| !c.starts_with(worktree))
        .unwrap_or(true)
}

#[test]
fn workspace_write_sandbox_compiles_against_trait() {
    let s = WorkspaceWriteSandbox::new(std::env::temp_dir());
    let _: Box<dyn Sandbox> = Box::new(s);
}

#[test]
fn visibility_allow_diverges_from_allows_tool_for_escaping_path() {
    let wt = tempfile::tempdir().unwrap();
    let canonical_root = wt.path().canonicalize().unwrap();
    let outside = std::env::temp_dir().join("outside.txt");
    std::fs::write(&outside, "x").ok();
    let sandbox = WorkspaceWriteSandboxWithPath {
        worktree_root: canonical_root,
        requested_path: outside.clone(),
    };
    assert_eq!(
        sandbox.visibility("write_text_file", None),
        SandboxDecision::Allow,
        "visibility must allow — tool is in scope at session-open time"
    );
    match sandbox.allows_tool("write_text_file", None) {
        SandboxDecision::Deny { reason } => assert!(reason.contains("escapes")),
        other => panic!("expected Deny for escaping path, got {other:?}"),
    }
    let _ = std::fs::remove_file(&outside);
}
