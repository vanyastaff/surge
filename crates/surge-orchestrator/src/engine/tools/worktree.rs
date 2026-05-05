//! `WorktreeToolDispatcher` — file + shell tools rooted in the run's worktree.

use crate::engine::tools::{ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload};
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Maximum bytes of stdout/stderr to capture per shell command.
/// Longer output is tail-truncated with a marker line.
const TAIL_CAP: usize = 64 * 1024;

fn truncate_with_marker(s: String, cap: usize) -> String {
    if s.len() <= cap {
        s
    } else {
        let tail_start = s.len() - cap + 64;
        let tail: String = s.chars().skip(tail_start).collect();
        format!(
            "[truncated, original length = {} bytes; showing last {} bytes]\n{}",
            s.len(),
            cap - 64,
            tail
        )
    }
}

/// Tool dispatcher that constrains all file and shell operations to the run's
/// isolated git worktree. Prevents path-traversal attacks.
///
/// Per-call worktree resolution comes from [`ToolDispatchContext::worktree_root`]
/// at dispatch time, so a single dispatcher instance is safe to share across
/// multiple runs (daemon mode) or agent stages with different worktrees.
pub struct WorktreeToolDispatcher {
    /// Historical: the constructor accepted a worktree path, but `dispatch`
    /// now reads `ctx.worktree_root` per call. Kept for backwards
    /// compatibility with M6 construction sites that call
    /// `WorktreeToolDispatcher::new(cwd)`.
    worktree_root: PathBuf,
}

impl WorktreeToolDispatcher {
    /// Create a new dispatcher.
    ///
    /// The `worktree_root` argument is accepted for backwards compatibility
    /// with M6 callers but is no longer load-bearing: per-call worktree
    /// resolution happens in [`dispatch`][`ToolDispatcher::dispatch`] via
    /// [`ToolDispatchContext::worktree_root`].
    #[must_use]
    pub fn new(worktree_root: PathBuf) -> Self {
        Self { worktree_root }
    }

    /// Return the path stored at construction time.
    ///
    /// This path is no longer used by `dispatch` (which reads
    /// `ctx.worktree_root` instead). It is exposed so that callers
    /// constructed before the F6 fix can still read back the path they
    /// passed in.
    #[must_use]
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }
}

/// Canonicalize `worktree_root` so the `starts_with` path-guard check works
/// correctly on Windows (where `fs::canonicalize` adds the `\\?\` prefix).
/// Falls back to the original path if canonicalization fails.
fn canonical_root(worktree_root: &Path) -> PathBuf {
    std::fs::canonicalize(worktree_root).unwrap_or_else(|_| worktree_root.to_path_buf())
}

async fn read_file(worktree_root: &Path, call: &ToolCall) -> ToolResultPayload {
    let worktree_root = canonical_root(worktree_root);
    let worktree_root = worktree_root.as_path();
    let Some(args) = call.arguments.as_object() else {
        return ToolResultPayload::Error {
            message: "read_file: arguments must be an object".into(),
        };
    };
    let Some(rel_path) = args.get("path").and_then(|v| v.as_str()) else {
        return ToolResultPayload::Error {
            message: "read_file: missing 'path' arg".into(),
        };
    };
    let binary = args
        .get("binary")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let abs = worktree_root.join(rel_path);
    let canonical = match std::fs::canonicalize(&abs) {
        Ok(p) => p,
        Err(e) => {
            return ToolResultPayload::Error {
                message: format!("read_file: cannot canonicalize {}: {e}", abs.display()),
            };
        },
    };
    if !canonical.starts_with(worktree_root) {
        return ToolResultPayload::Error {
            message: format!(
                "read_file: path {} escapes worktree {}",
                canonical.display(),
                worktree_root.display()
            ),
        };
    }
    if binary {
        match tokio::fs::read(&canonical).await {
            Ok(bytes) => {
                use base64::{Engine, engine::general_purpose::STANDARD};
                ToolResultPayload::Ok {
                    content: serde_json::json!({
                        "content_base64": STANDARD.encode(&bytes),
                        "byte_len": bytes.len(),
                    }),
                }
            },
            Err(e) => ToolResultPayload::Error {
                message: format!("read_file: {e}"),
            },
        }
    } else {
        match tokio::fs::read_to_string(&canonical).await {
            Ok(s) => ToolResultPayload::Ok {
                content: serde_json::json!({
                    "content_text": s,
                }),
            },
            Err(e) => ToolResultPayload::Error {
                message: format!("read_file: {e}"),
            },
        }
    }
}

async fn write_file(worktree_root: &Path, call: &ToolCall) -> ToolResultPayload {
    let worktree_root = canonical_root(worktree_root);
    let worktree_root = worktree_root.as_path();
    let Some(args) = call.arguments.as_object() else {
        return ToolResultPayload::Error {
            message: "write_file: arguments must be an object".into(),
        };
    };
    let Some(rel_path) = args.get("path").and_then(|v| v.as_str()) else {
        return ToolResultPayload::Error {
            message: "write_file: missing 'path' arg".into(),
        };
    };
    let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
        return ToolResultPayload::Error {
            message: "write_file: missing 'content' arg".into(),
        };
    };
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("overwrite");
    let abs = worktree_root.join(rel_path);
    // For write paths, the parent must canonicalize within worktree;
    // the leaf may not yet exist.
    let Some(parent) = abs.parent() else {
        return ToolResultPayload::Error {
            message: format!("write_file: invalid path {}", abs.display()),
        };
    };
    let canonical_parent = match std::fs::canonicalize(parent) {
        Ok(p) => p,
        Err(e) => {
            return ToolResultPayload::Error {
                message: format!(
                    "write_file: cannot canonicalize parent {}: {e}",
                    parent.display()
                ),
            };
        },
    };
    if !canonical_parent.starts_with(worktree_root) {
        return ToolResultPayload::Error {
            message: format!(
                "write_file: parent {} escapes worktree {}",
                canonical_parent.display(),
                worktree_root.display()
            ),
        };
    }
    let leaf = abs.file_name().unwrap_or_default();
    let final_path = canonical_parent.join(leaf);
    let result = match mode {
        "create" => {
            if final_path.exists() {
                return ToolResultPayload::Error {
                    message: format!("write_file create: {} already exists", final_path.display()),
                };
            }
            tokio::fs::write(&final_path, content).await
        },
        "overwrite" => tokio::fs::write(&final_path, content).await,
        "append" => {
            use tokio::io::AsyncWriteExt;
            match tokio::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&final_path)
                .await
            {
                Ok(mut f) => f.write_all(content.as_bytes()).await,
                Err(e) => Err(e),
            }
        },
        other => {
            return ToolResultPayload::Error {
                message: format!(
                    "write_file: unknown mode '{other}', expected create/overwrite/append"
                ),
            };
        },
    };
    match result {
        Ok(()) => ToolResultPayload::Ok {
            content: serde_json::json!({
                "bytes_written": content.len(),
            }),
        },
        Err(e) => ToolResultPayload::Error {
            message: format!("write_file: {e}"),
        },
    }
}

async fn shell_exec(worktree_root: &Path, call: &ToolCall) -> ToolResultPayload {
    let worktree_root = canonical_root(worktree_root);
    let worktree_root = worktree_root.as_path();
    let Some(args) = call.arguments.as_object() else {
        return ToolResultPayload::Error {
            message: "shell_exec: arguments must be an object".into(),
        };
    };
    let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
        return ToolResultPayload::Error {
            message: "shell_exec: missing 'command' arg".into(),
        };
    };
    let cwd = if let Some(rel) = args.get("cwd_relative").and_then(|v| v.as_str()) {
        let joined = worktree_root.join(rel);
        let canonical = match std::fs::canonicalize(&joined) {
            Ok(p) => p,
            Err(e) => {
                return ToolResultPayload::Error {
                    message: format!(
                        "shell_exec: cannot canonicalize cwd_relative {}: {e}",
                        joined.display()
                    ),
                };
            },
        };
        if !canonical.starts_with(worktree_root) {
            return ToolResultPayload::Error {
                message: format!(
                    "shell_exec: cwd_relative {} escapes worktree {}",
                    canonical.display(),
                    worktree_root.display()
                ),
            };
        }
        canonical
    } else {
        worktree_root.to_path_buf()
    };
    let timeout_secs = args
        .get("timeout_seconds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(300);

    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    cmd.current_dir(&cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ToolResultPayload::Error {
                message: format!("shell_exec: spawn failed: {e}"),
            };
        },
    };

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let output_fut = child.wait_with_output();
    let output = match tokio::time::timeout(timeout, output_fut).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return ToolResultPayload::Error {
                message: format!("shell_exec: wait failed: {e}"),
            };
        },
        Err(_) => {
            return ToolResultPayload::Error {
                message: format!("shell_exec: timeout after {timeout_secs}s"),
            };
        },
    };

    let stdout = truncate_with_marker(
        String::from_utf8_lossy(&output.stdout).into_owned(),
        TAIL_CAP,
    );
    let stderr = truncate_with_marker(
        String::from_utf8_lossy(&output.stderr).into_owned(),
        TAIL_CAP,
    );
    let exit_code = output.status.code().unwrap_or(-1);

    ToolResultPayload::Ok {
        content: serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
        }),
    }
}

#[async_trait]
impl ToolDispatcher for WorktreeToolDispatcher {
    async fn dispatch(&self, ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload {
        match call.tool.as_str() {
            "read_file" => read_file(ctx.worktree_root, call).await,
            "write_file" => write_file(ctx.worktree_root, call).await,
            "shell_exec" => shell_exec(ctx.worktree_root, call).await,
            other => ToolResultPayload::Unsupported {
                message: format!(
                    "WorktreeToolDispatcher: tool '{other}' not implemented (M5 supports read_file/write_file/shell_exec)"
                ),
            },
        }
    }

    fn declared_tools(&self) -> Vec<crate::engine::tools::DeclaredTool> {
        use crate::engine::tools::DeclaredTool;
        use serde_json::json;
        vec![
            DeclaredTool {
                name: "read_file".into(),
                description: Some(
                    "Read the contents of a file inside the run's worktree. \
                     Use `binary: true` to receive base64-encoded bytes."
                        .into(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative path from the worktree root."
                        },
                        "binary": {
                            "type": "boolean",
                            "description": "If true, return base64-encoded bytes instead of UTF-8 text."
                        },
                    },
                    "required": ["path"],
                }),
            },
            DeclaredTool {
                name: "write_file".into(),
                description: Some(
                    "Write text content to a file inside the run's worktree. \
                     Supports create, overwrite, and append modes."
                        .into(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative path from the worktree root."
                        },
                        "content": {
                            "type": "string",
                            "description": "Text content to write."
                        },
                        "mode": {
                            "type": "string",
                            "enum": ["create", "overwrite", "append"],
                            "description": "Write mode: create (fail if exists), overwrite (default), or append."
                        },
                    },
                    "required": ["path", "content"],
                }),
            },
            DeclaredTool {
                name: "shell_exec".into(),
                description: Some(
                    "Execute a shell command inside the run's worktree. \
                     stdout and stderr are captured and returned; output longer \
                     than 64 KiB is tail-truncated."
                        .into(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to run."
                        },
                        "cwd_relative": {
                            "type": "string",
                            "description": "Optional working directory relative to the worktree root. Defaults to the worktree root."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Timeout in seconds. Defaults to 300."
                        },
                    },
                    "required": ["command"],
                }),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(
        root: &'a std::path::Path,
        mem: &'a surge_core::run_state::RunMemory,
    ) -> ToolDispatchContext<'a> {
        ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: root,
            run_memory: mem,
        }
    }

    #[tokio::test]
    async fn read_file_returns_text_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, "hello").unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            arguments: serde_json::json!({"path": "hello.txt"}),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                assert_eq!(content["content_text"].as_str().unwrap(), "hello");
            },
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_file_rejects_path_escaping_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let escape = outside.path().join("secret.txt");
        std::fs::write(&escape, "secret").unwrap();

        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            arguments: serde_json::json!({
                "path": format!("../{}/secret.txt", outside.path().file_name().unwrap().to_string_lossy()),
            }),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        assert!(matches!(result, ToolResultPayload::Error { .. }));
    }

    #[tokio::test]
    async fn write_file_overwrite_creates_then_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        for content in ["v1", "v2"] {
            let call = ToolCall {
                call_id: "c1".into(),
                tool: "write_file".into(),
                arguments: serde_json::json!({
                    "path": "out.txt",
                    "content": content,
                    "mode": "overwrite",
                }),
            };
            let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
            assert!(matches!(result, ToolResultPayload::Ok { .. }));
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
            "v2"
        );
    }

    #[tokio::test]
    async fn write_file_create_rejects_existing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("exists.txt"), "old").unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "write_file".into(),
            arguments: serde_json::json!({
                "path": "exists.txt",
                "content": "new",
                "mode": "create",
            }),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        assert!(matches!(result, ToolResultPayload::Error { .. }));
    }

    #[tokio::test]
    async fn unknown_tool_returns_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "glob".into(),
            arguments: serde_json::json!({}),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        assert!(matches!(result, ToolResultPayload::Unsupported { .. }));
    }

    #[tokio::test]
    async fn shell_exec_runs_simple_command() {
        let dir = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        // `echo hi` works on both Windows (via cmd /C) and Unix (via sh -c).
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({"command": "echo hi"}),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                let stdout = content["stdout"].as_str().unwrap();
                assert!(stdout.contains("hi"), "stdout was {stdout:?}");
                assert_eq!(content["exit_code"].as_i64().unwrap(), 0);
            },
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_exec_reports_nonzero_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        // Portable variant: 1-line script that exits 7.
        let cmd = if cfg!(windows) {
            "cmd /C exit 7"
        } else {
            "exit 7"
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({"command": cmd}),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                assert_eq!(content["exit_code"].as_i64().unwrap(), 7);
            },
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_exec_rejects_cwd_escaping_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let d = WorktreeToolDispatcher::new(dir.path().to_path_buf());
        let mem = surge_core::run_state::RunMemory::default();
        // Construct a relative path that resolves outside the worktree.
        let escape_rel = format!(
            "../{}",
            outside.path().file_name().unwrap().to_string_lossy()
        );
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({
                "command": "echo hi",
                "cwd_relative": escape_rel,
            }),
        };
        let result = d.dispatch(&ctx(d.worktree_root(), &mem), &call).await;
        assert!(
            matches!(result, ToolResultPayload::Error { .. }),
            "expected Error for escaping cwd_relative, got {result:?}"
        );
    }
}
