//! `WorktreeToolDispatcher` — file + shell tools rooted in the run's worktree.

use crate::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use async_trait::async_trait;
use std::path::PathBuf;

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
pub struct WorktreeToolDispatcher {
    worktree_root: PathBuf,
}

impl WorktreeToolDispatcher {
    /// Create a new dispatcher rooted at `worktree_root`.
    ///
    /// The path is canonicalized on construction; if canonicalization fails
    /// (e.g. the directory doesn't exist yet) the original path is kept.
    #[must_use]
    pub fn new(worktree_root: PathBuf) -> Self {
        let canonical = std::fs::canonicalize(&worktree_root).unwrap_or(worktree_root);
        Self { worktree_root: canonical }
    }

    /// Return the canonicalized worktree root path.
    #[must_use]
    pub fn worktree_root(&self) -> &std::path::Path {
        &self.worktree_root
    }

    async fn read_file(&self, call: &ToolCall) -> ToolResultPayload {
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
        let binary = args.get("binary").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let abs = self.worktree_root.join(rel_path);
        let canonical = match std::fs::canonicalize(&abs) {
            Ok(p) => p,
            Err(e) => {
                return ToolResultPayload::Error {
                    message: format!("read_file: cannot canonicalize {}: {e}", abs.display()),
                }
            }
        };
        if !canonical.starts_with(&self.worktree_root) {
            return ToolResultPayload::Error {
                message: format!(
                    "read_file: path {} escapes worktree {}",
                    canonical.display(),
                    self.worktree_root.display()
                ),
            };
        }
        if binary {
            match tokio::fs::read(&canonical).await {
                Ok(bytes) => {
                    use base64::{engine::general_purpose::STANDARD, Engine};
                    ToolResultPayload::Ok {
                        content: serde_json::json!({
                            "content_base64": STANDARD.encode(&bytes),
                            "byte_len": bytes.len(),
                        }),
                    }
                }
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

    async fn write_file(&self, call: &ToolCall) -> ToolResultPayload {
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
        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("overwrite");
        let abs = self.worktree_root.join(rel_path);
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
                    message: format!("write_file: cannot canonicalize parent {}: {e}", parent.display()),
                }
            }
        };
        if !canonical_parent.starts_with(&self.worktree_root) {
            return ToolResultPayload::Error {
                message: format!(
                    "write_file: parent {} escapes worktree {}",
                    canonical_parent.display(),
                    self.worktree_root.display()
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
            }
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
            }
            other => {
                return ToolResultPayload::Error {
                    message: format!("write_file: unknown mode '{other}', expected create/overwrite/append"),
                }
            }
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

    async fn shell_exec(&self, call: &ToolCall) -> ToolResultPayload {
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
            self.worktree_root.join(rel)
        } else {
            self.worktree_root.clone()
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
                }
            }
        };

        let timeout = std::time::Duration::from_secs(timeout_secs);
        let output_fut = child.wait_with_output();
        let output = match tokio::time::timeout(timeout, output_fut).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return ToolResultPayload::Error {
                    message: format!("shell_exec: wait failed: {e}"),
                }
            }
            Err(_) => {
                return ToolResultPayload::Error {
                    message: format!("shell_exec: timeout after {timeout_secs}s"),
                }
            }
        };

        let stdout = truncate_with_marker(String::from_utf8_lossy(&output.stdout).into_owned(), TAIL_CAP);
        let stderr = truncate_with_marker(String::from_utf8_lossy(&output.stderr).into_owned(), TAIL_CAP);
        let exit_code = output.status.code().unwrap_or(-1);

        ToolResultPayload::Ok {
            content: serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
            }),
        }
    }
}

#[async_trait]
impl ToolDispatcher for WorktreeToolDispatcher {
    async fn dispatch(
        &self,
        _ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> ToolResultPayload {
        match call.tool.as_str() {
            "read_file" => self.read_file(call).await,
            "write_file" => self.write_file(call).await,
            "shell_exec" => self.shell_exec(call).await,
            other => ToolResultPayload::Unsupported {
                message: format!("WorktreeToolDispatcher: tool '{other}' not implemented (M5 supports read_file/write_file/shell_exec)"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(root: &'a std::path::Path, mem: &'a surge_core::run_state::RunMemory) -> ToolDispatchContext<'a> {
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
            }
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
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "v2");
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
            }
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
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }
}
