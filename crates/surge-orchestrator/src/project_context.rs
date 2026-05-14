//! Stable project-context generation for `surge project describe`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::sandbox::AlwaysAllowSandbox;
use surge_acp::bridge::session::{AgentKind, MessageContent, SessionConfig};
use surge_acp::client::PermissionPolicy;
use surge_core::ContentHash;
use surge_core::agent_config::{ArtifactSource, Binding, TemplateVar};
use surge_core::keys::OutcomeKey;
use surge_core::profile::keyref::parse_key_ref;
use tracing::{debug, info, warn};

use crate::engine::config::{EngineRunConfig, ProjectContextSeed};
use crate::profile_loader::ProfileRegistry;
use crate::prompt::PromptRenderer;

const DEFAULT_MAX_FILE_BYTES: u64 = 64 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 256 * 1024;
const PROJECT_CONTEXT_PROFILE: &str = "project-context-author@1.0";
const SCAN_HASH_MARKER: &str = "surge:project-context scan_hash=";
const PROJECT_CONTEXT_AUTHOR_TIMEOUT: Duration = Duration::from_secs(300);

/// Options for generating or checking a root `project.md`.
#[derive(Debug, Clone)]
pub struct ProjectContextOptions {
    pub project_root: PathBuf,
    pub output_path: PathBuf,
    pub refresh: bool,
    pub dry_run: bool,
    pub limits: ScanLimits,
}

impl ProjectContextOptions {
    #[must_use]
    pub fn new(project_root: PathBuf, output_path: PathBuf) -> Self {
        Self {
            project_root,
            output_path,
            refresh: false,
            dry_run: false,
            limits: ScanLimits::default(),
        }
    }
}

/// Scan byte budgets.
#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

/// Result of `surge project describe`.
#[derive(Debug, Clone)]
pub struct ProjectContextOutcome {
    pub status: ProjectContextStatus,
    pub output_path: PathBuf,
    pub scan_hash: ContentHash,
    pub output_hash: ContentHash,
    pub profile_id: String,
    pub normalized_agent_id: String,
    pub skipped_files: Vec<SkippedFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectContextStatus {
    Drafted,
    NoChange,
    WouldDraft,
    WouldNoChange,
}

impl ProjectContextStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drafted => "drafted",
            Self::NoChange => "no_change",
            Self::WouldDraft => "would_draft",
            Self::WouldNoChange => "would_no_change",
        }
    }
}

/// Deterministic scan input supplied to the Project Context Author profile.
#[derive(Debug, Clone)]
pub struct ProjectScan {
    pub root: PathBuf,
    pub files: Vec<ScannedFile>,
    pub skipped_files: Vec<SkippedFile>,
    pub git: GitSnapshot,
    pub scan_context: String,
    pub hash: ContentHash,
}

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub relative_path: PathBuf,
    pub byte_len: u64,
    pub hash: ContentHash,
    pub redaction_count: usize,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
    pub relative_path: PathBuf,
    pub reason: String,
    pub byte_len: Option<u64>,
    pub hash: Option<ContentHash>,
}

#[derive(Debug, Clone, Default)]
pub struct GitSnapshot {
    pub branch: Option<String>,
    pub dirty: Option<bool>,
}

/// Errors from project-context scanning and generation.
#[derive(Debug, thiserror::Error)]
pub enum ProjectContextError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("profile resolution failed: {0}")]
    Profile(String),
    #[error("profile runtime agent id {agent_id:?} is unknown; known ids and aliases: {known}")]
    UnknownAgentId { agent_id: String, known: String },
    #[error("project context author bridge failed: {0}")]
    Bridge(String),
    #[error("project context author reported no artifact")]
    MissingAuthorArtifact,
}

/// Generate or refresh the stable `project.md` artifact.
///
/// # Errors
/// Returns [`ProjectContextError`] for filesystem failures or if the bundled
/// Project Context Author profile cannot resolve to a known ACP registry entry.
pub fn describe_project(
    options: ProjectContextOptions,
) -> Result<ProjectContextOutcome, ProjectContextError> {
    debug!(
        root = %options.project_root.display(),
        output = %options.output_path.display(),
        refresh = options.refresh,
        dry_run = options.dry_run,
        "project context describe started"
    );
    let scan = scan_project(&options.project_root, options.limits)?;
    let invocation = project_context_invocation(&options.project_root, &scan)?;
    let rendered = render_project_context(&scan, &invocation);
    finish_project_context(options, scan, invocation, rendered)
}

/// Generate or refresh project context through a real ACP bridge session.
///
/// This path exercises the Project Context Author profile contract directly:
/// it renders the profile prompt from `worktree_root` and `scan_context`, opens
/// an ACP session, waits for `OutcomeReported`, reads the reported artifact, and
/// then applies the same hash/no-change/write logic as [`describe_project`].
///
/// # Errors
/// Returns [`ProjectContextError`] for scan/profile failures, bridge failures,
/// author timeout/session termination, missing artifacts, or output writes.
pub async fn describe_project_with_bridge(
    options: ProjectContextOptions,
    bridge: Arc<dyn BridgeFacade>,
) -> Result<ProjectContextOutcome, ProjectContextError> {
    debug!(
        root = %options.project_root.display(),
        output = %options.output_path.display(),
        refresh = options.refresh,
        dry_run = options.dry_run,
        "bridge-backed project context describe started"
    );
    let scan = scan_project(&options.project_root, options.limits)?;
    let invocation = project_context_invocation(&options.project_root, &scan)?;
    let rendered = invoke_project_context_author(
        &options.project_root,
        &options.output_path,
        &invocation,
        bridge,
    )
    .await?;
    finish_project_context(options, scan, invocation, rendered)
}

fn finish_project_context(
    options: ProjectContextOptions,
    scan: ProjectScan,
    invocation: ProjectContextInvocation,
    rendered: String,
) -> Result<ProjectContextOutcome, ProjectContextError> {
    let rendered = ensure_project_context_metadata(rendered, &scan, &invocation);
    let output_hash = ContentHash::compute(rendered.as_bytes());
    let existing = std::fs::read_to_string(&options.output_path).ok();
    let existing_scan_hash = existing.as_deref().and_then(extract_scan_hash);
    let same_scan = existing_scan_hash == Some(scan.hash);
    let same_content = existing
        .as_ref()
        .is_some_and(|content| ContentHash::compute(content.as_bytes()) == output_hash);
    let changed = options.refresh || !same_scan || !same_content;

    debug!(
        scan_hash = %scan.hash,
        output_hash = %output_hash,
        same_scan,
        same_content,
        changed,
        skipped = scan.skipped_files.len(),
        "project context change check complete"
    );

    let status = match (options.dry_run, changed) {
        (true, true) => ProjectContextStatus::WouldDraft,
        (true, false) => ProjectContextStatus::WouldNoChange,
        (false, true) => {
            write_project_context(&options.output_path, &rendered)?;
            ProjectContextStatus::Drafted
        },
        (false, false) => ProjectContextStatus::NoChange,
    };

    info!(
        output = %options.output_path.display(),
        status = status.as_str(),
        scan_hash = %scan.hash,
        output_hash = %output_hash,
        profile = %invocation.profile_id,
        agent_id = %invocation.normalized_agent_id,
        "project context describe completed"
    );

    Ok(ProjectContextOutcome {
        status,
        output_path: options.output_path,
        scan_hash: scan.hash,
        output_hash,
        profile_id: invocation.profile_id,
        normalized_agent_id: invocation.normalized_agent_id,
        skipped_files: scan.skipped_files,
    })
}

async fn invoke_project_context_author(
    root: &Path,
    output_path: &Path,
    invocation: &ProjectContextInvocation,
    bridge: Arc<dyn BridgeFacade>,
) -> Result<String, ProjectContextError> {
    let prompt = PromptRenderer::strict()
        .render(&invocation.prompt_template, &invocation.binding_values)
        .map_err(|e| ProjectContextError::Bridge(format!("prompt render failed: {e}")))?;
    let mut receiver = bridge.subscribe();
    let session = bridge
        .open_session(project_context_session_config(
            root,
            invocation,
            prompt.clone(),
        )?)
        .await
        .map_err(|e| ProjectContextError::Bridge(format!("open_session: {e}")))?;

    bridge
        .send_message(session, MessageContent::Text(prompt))
        .await
        .map_err(|e| ProjectContextError::Bridge(format!("send_message: {e}")))?;

    let deadline = tokio::time::Instant::now() + PROJECT_CONTEXT_AUTHOR_TIMEOUT;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(ProjectContextError::Bridge(
                "timed out waiting for OutcomeReported".to_string(),
            ));
        }
        let event = tokio::time::timeout(deadline.duration_since(now), receiver.recv())
            .await
            .map_err(|_| {
                ProjectContextError::Bridge("timed out waiting for OutcomeReported".to_string())
            })?
            .map_err(|e| ProjectContextError::Bridge(format!("bridge event stream: {e}")))?;

        match event {
            BridgeEvent::OutcomeReported {
                session: event_session,
                outcome,
                artifacts_produced,
                ..
            } if event_session == session => {
                let content =
                    read_author_output(root, output_path, outcome.as_ref(), &artifacts_produced)?;
                if let Err(e) = bridge.close_session(session).await {
                    warn!(error = %e, "project context author session close failed");
                }
                return Ok(content);
            },
            BridgeEvent::SessionEnded {
                session: event_session,
                reason,
            } if event_session == session => {
                return Err(ProjectContextError::Bridge(format!(
                    "session ended before OutcomeReported: {reason:?}"
                )));
            },
            _ => {},
        }
    }
}

fn project_context_session_config(
    root: &Path,
    invocation: &ProjectContextInvocation,
    prompt: String,
) -> Result<SessionConfig, ProjectContextError> {
    let declared_outcomes = ["drafted", "no_change"]
        .into_iter()
        .map(|outcome| {
            OutcomeKey::try_from(outcome).map_err(|e| {
                ProjectContextError::Bridge(format!("invalid declared outcome {outcome:?}: {e}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let bindings = BTreeMap::from([
        ("profile".to_string(), invocation.profile_id.clone()),
        ("agent".to_string(), invocation.normalized_agent_id.clone()),
    ]);
    Ok(SessionConfig {
        agent_kind: invocation.agent_kind(),
        working_dir: root.to_path_buf(),
        system_prompt: prompt,
        declared_outcomes,
        allows_escalation: false,
        tools: Vec::new(),
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings,
    })
}

fn read_author_output(
    root: &Path,
    output_path: &Path,
    outcome: &str,
    artifacts_produced: &[String],
) -> Result<String, ProjectContextError> {
    let candidate = artifacts_produced.first().map_or_else(
        || {
            (outcome == "no_change")
                .then_some(output_path.to_path_buf())
                .ok_or(ProjectContextError::MissingAuthorArtifact)
        },
        |reported| author_artifact_path(root, reported),
    )?;
    std::fs::read_to_string(&candidate).map_err(|source| ProjectContextError::Io {
        path: candidate,
        source,
    })
}

fn author_artifact_path(root: &Path, reported: &str) -> Result<PathBuf, ProjectContextError> {
    let path = PathBuf::from(reported);
    let candidate = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let root = root
        .canonicalize()
        .map_err(|source| ProjectContextError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    let candidate = candidate
        .canonicalize()
        .map_err(|source| ProjectContextError::Io {
            path: candidate.clone(),
            source,
        })?;
    if !candidate.starts_with(&root) {
        return Err(ProjectContextError::Bridge(format!(
            "reported artifact path escapes project root: {}",
            candidate.display()
        )));
    }
    Ok(candidate)
}

/// Seed every config-derived field on an [`EngineRunConfig`].
///
/// Currently covers two seeds, both unconditionally needed on every run
/// regardless of entry point (CLI in-process, daemon IPC server,
/// daemon-side ticket launcher):
///
/// - **`project_context`** — read from the configured `project.md` when
///   `init.project_context_auto_seed` is enabled and the run config
///   does not already carry one.
/// - **`mcp_servers`** — cloned from `SurgeConfig::mcp_servers` so the
///   engine can build its `Arc<McpRegistry>` per run. This is a
///   structural copy (no I/O), but keeping it next to the file-backed
///   project-context seed prevents the two from drifting at individual
///   call sites — every entry point now goes through the same helper.
#[must_use]
pub fn with_project_context_seed(
    mut run_config: EngineRunConfig,
    project_root: &Path,
    config: &surge_core::SurgeConfig,
) -> EngineRunConfig {
    if run_config.project_context.is_none() && config.init.project_context_auto_seed {
        run_config.project_context = load_project_context_seed(project_root, config);
    }
    if run_config.mcp_servers.is_empty() && !config.mcp_servers.is_empty() {
        run_config.mcp_servers = config.mcp_servers.clone();
    }
    run_config
}

/// Load the configured project context file as a stable run seed.
#[must_use]
pub fn load_project_context_seed(
    project_root: &Path,
    config: &surge_core::SurgeConfig,
) -> Option<ProjectContextSeed> {
    let path = if config.init.project_context_path.is_absolute() {
        config.init.project_context_path.clone()
    } else {
        project_root.join(&config.init.project_context_path)
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let seed = ProjectContextSeed::new(path.clone(), content);
            debug!(
                path = %path.display(),
                bytes = seed.content.len(),
                hash = %seed.hash,
                "loaded project context seed"
            );
            Some(seed)
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!(
                path = %path.display(),
                "project context seed not found; run starts without it"
            );
            None
        },
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "project context seed unreadable; run starts without it"
            );
            None
        },
    }
}

/// Scan high-signal project files into deterministic profile input.
///
/// # Errors
/// Returns [`ProjectContextError::Io`] if required root metadata cannot be read.
pub fn scan_project(root: &Path, limits: ScanLimits) -> Result<ProjectScan, ProjectContextError> {
    debug!(
        root = %root.display(),
        max_file_bytes = limits.max_file_bytes,
        max_total_bytes = limits.max_total_bytes,
        "project context scan started"
    );
    let root = root.to_path_buf();
    let mut files = Vec::new();
    let mut skipped_files = Vec::new();
    let mut total_bytes = 0_u64;

    for relative_path in high_signal_paths() {
        let absolute_path = root.join(&relative_path);
        if !absolute_path.exists() {
            continue;
        }
        let metadata =
            std::fs::metadata(&absolute_path).map_err(|source| ProjectContextError::Io {
                path: absolute_path.clone(),
                source,
            })?;
        if !metadata.is_file() {
            continue;
        }
        let byte_len = metadata.len();
        if byte_len > limits.max_file_bytes {
            warn!(
                category = "size_budget",
                path = %relative_path.display(),
                bytes = byte_len,
                "project context skipped oversized file"
            );
            skipped_files.push(SkippedFile {
                relative_path,
                reason: "oversized_file".to_string(),
                byte_len: Some(byte_len),
                hash: None,
            });
            continue;
        }
        if total_bytes.saturating_add(byte_len) > limits.max_total_bytes {
            warn!(
                category = "size_budget",
                path = %relative_path.display(),
                bytes = byte_len,
                total = total_bytes,
                "project context skipped file due to total budget"
            );
            skipped_files.push(SkippedFile {
                relative_path,
                reason: "total_budget_exceeded".to_string(),
                byte_len: Some(byte_len),
                hash: None,
            });
            continue;
        }

        match std::fs::read_to_string(&absolute_path) {
            Ok(raw) => {
                let hash = ContentHash::compute(raw.as_bytes());
                let (content, redaction_count) = redact_secret_like_values(&raw);
                debug!(
                    path = %relative_path.display(),
                    bytes = byte_len,
                    hash = %hash,
                    redactions = redaction_count,
                    "project context included file"
                );
                total_bytes += byte_len;
                files.push(ScannedFile {
                    relative_path,
                    byte_len,
                    hash,
                    redaction_count,
                    content,
                });
            },
            Err(source) => {
                warn!(
                    category = "read",
                    path = %relative_path.display(),
                    "project context skipped unreadable optional file"
                );
                skipped_files.push(SkippedFile {
                    relative_path,
                    reason: "unreadable".to_string(),
                    byte_len: Some(byte_len),
                    hash: None,
                });
                debug!(error = %source, "optional project context file read failed");
            },
        }
    }

    skipped_files.extend(skipped_directory_summary(&root));
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    skipped_files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let git = git_snapshot(&root);
    let scan_without_hash = render_scan_context(&root, &files, &skipped_files, &git, None);
    let hash = ContentHash::compute(scan_without_hash.as_bytes());
    let scan_context = render_scan_context(&root, &files, &skipped_files, &git, Some(hash));

    debug!(
        root = %root.display(),
        included = files.len(),
        skipped = skipped_files.len(),
        total_bytes,
        scan_hash = %hash,
        "project context scan completed"
    );

    Ok(ProjectScan {
        root,
        files,
        skipped_files,
        git,
        scan_context,
        hash,
    })
}

#[derive(Debug, Clone)]
struct ProjectContextInvocation {
    profile_id: String,
    normalized_agent_id: String,
    agent_command: String,
    agent_args: Vec<String>,
    prompt_template: String,
    bindings: Vec<Binding>,
    binding_values: Vec<(TemplateVar, String)>,
}

impl ProjectContextInvocation {
    fn agent_kind(&self) -> AgentKind {
        AgentKind::Custom {
            binary: PathBuf::from(&self.agent_command),
            args: self.agent_args.clone(),
        }
    }
}

fn project_context_invocation(
    root: &Path,
    scan: &ProjectScan,
) -> Result<ProjectContextInvocation, ProjectContextError> {
    let registry =
        ProfileRegistry::load().map_err(|e| ProjectContextError::Profile(e.to_string()))?;
    let key_ref = parse_key_ref(PROJECT_CONTEXT_PROFILE)
        .map_err(|e| ProjectContextError::Profile(e.to_string()))?;
    let resolved = registry
        .resolve(&key_ref)
        .map_err(|e| ProjectContextError::Profile(e.to_string()))?;
    let agent_registry = surge_acp::Registry::builtin();
    let normalized_agent_id = agent_registry
        .normalize_agent_id(&resolved.profile.runtime.agent_id)
        .ok_or_else(|| ProjectContextError::UnknownAgentId {
            agent_id: resolved.profile.runtime.agent_id.clone(),
            known: agent_registry.known_ids_and_aliases().join(", "),
        })?;
    let entry = agent_registry.find(&normalized_agent_id).ok_or_else(|| {
        ProjectContextError::UnknownAgentId {
            agent_id: normalized_agent_id.clone(),
            known: agent_registry.known_ids_and_aliases().join(", "),
        }
    })?;
    let bindings = vec![
        Binding {
            source: ArtifactSource::Static {
                content: root.display().to_string(),
            },
            target: TemplateVar("worktree_root".to_string()),
            optional: false,
        },
        Binding {
            source: ArtifactSource::Static {
                content: scan.scan_context.clone(),
            },
            target: TemplateVar("scan_context".to_string()),
            optional: false,
        },
    ];
    let binding_values = vec![
        (
            TemplateVar("worktree_root".to_string()),
            root.display().to_string(),
        ),
        (
            TemplateVar("scan_context".to_string()),
            scan.scan_context.clone(),
        ),
    ];
    debug!(
        profile = %resolved.profile.role.id,
        agent_id = %resolved.profile.runtime.agent_id,
        normalized_agent_id = %normalized_agent_id,
        bindings = bindings.len(),
        "project context author profile resolved"
    );
    Ok(ProjectContextInvocation {
        profile_id: format!(
            "{}@{}",
            resolved.profile.role.id.as_str(),
            resolved.profile.role.version
        ),
        normalized_agent_id,
        agent_command: entry.command.clone(),
        agent_args: entry.default_args.clone(),
        prompt_template: resolved.profile.prompt.system.clone(),
        bindings,
        binding_values,
    })
}

fn render_project_context(scan: &ProjectScan, invocation: &ProjectContextInvocation) -> String {
    let project_name = scan
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let primary_language = if scan_has_file(scan, "Cargo.toml") {
        "Rust"
    } else {
        "Unknown"
    };
    let framework = if scan_has_file(scan, "Cargo.toml") {
        "Cargo workspace"
    } else {
        "Not detected"
    };
    let stack = stack_lines(scan);
    let directories = directory_lines(&scan.root);
    let build = build_command_lines(scan);
    let tests = test_lines(scan);
    let conventions = convention_lines(scan);

    format!(
        r#"<!-- {SCAN_HASH_MARKER}{scan_hash} -->
<!-- surge:project-context profile={profile} agent={agent} bindings={bindings} -->

## Project name
{project_name}

## Primary language
{primary_language}

## Framework
{framework}

## Stack
{stack}

## Key directories
{directories}

## Build commands
{build}

## Tests
{tests}

## Conventions
{conventions}
"#,
        scan_hash = scan.hash,
        profile = invocation.profile_id,
        agent = invocation.normalized_agent_id,
        bindings = invocation.bindings.len(),
    )
}

fn ensure_project_context_metadata(
    content: String,
    scan: &ProjectScan,
    invocation: &ProjectContextInvocation,
) -> String {
    let has_scan_hash = extract_scan_hash(&content).is_some();
    let has_profile_marker = content
        .lines()
        .any(|line| line.starts_with("<!-- surge:project-context profile="));
    if has_scan_hash && has_profile_marker {
        return content;
    }

    let mut metadata = String::new();
    if !has_scan_hash {
        metadata.push_str(&format!("<!-- {SCAN_HASH_MARKER}{} -->\n", scan.hash));
    }
    if !has_profile_marker {
        metadata.push_str(&format!(
            "<!-- surge:project-context profile={} agent={} bindings={} -->\n",
            invocation.profile_id,
            invocation.normalized_agent_id,
            invocation.bindings.len()
        ));
    }
    metadata.push('\n');
    metadata.push_str(content.trim_start());
    metadata
}

fn stack_lines(scan: &ProjectScan) -> String {
    let mut lines = Vec::new();
    if scan_has_file(scan, "Cargo.toml") {
        if let Some(edition) = cargo_edition(scan) {
            lines.push(format!("- Rust {edition} workspace managed by Cargo."));
        } else {
            lines.push("- Rust workspace managed by Cargo.".to_string());
        }
    }
    if file_contains(scan, "Cargo.toml", "tokio") {
        lines.push("- Async runtime: tokio.".to_string());
    }
    if file_contains(scan, "Cargo.toml", "rusqlite") {
        lines.push("- Database: SQLite via rusqlite.".to_string());
    }
    if file_contains(scan, "Cargo.toml", "agent-client-protocol") {
        lines.push("- Agent protocol: agent-client-protocol (ACP).".to_string());
    }
    if file_contains(scan, "Cargo.toml", "clap") {
        lines.push("- CLI: clap derive commands.".to_string());
    }
    if lines.is_empty() {
        "- Stack not detected from scanned files.".to_string()
    } else {
        lines.join("\n")
    }
}

fn cargo_edition(scan: &ProjectScan) -> Option<String> {
    let manifest = scan
        .files
        .iter()
        .find(|file| file.relative_path.as_path() == Path::new("Cargo.toml"))?;
    let value = toml::from_str::<toml::Value>(&manifest.content).ok()?;
    value
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("edition"))
        .or_else(|| {
            value
                .get("package")
                .and_then(|package| package.get("edition"))
        })
        .and_then(toml::Value::as_str)
        .map(ToString::to_string)
}

fn directory_lines(root: &Path) -> String {
    let mut lines = Vec::new();
    for (dir, purpose) in [
        ("crates", "Rust workspace crates"),
        ("docs", "project documentation"),
        ("examples", "sample flow.toml graphs"),
        (".ai-factory", "AI Factory context artifacts"),
        (".github", "CI workflows"),
    ] {
        if root.join(dir).is_dir() {
            lines.push(format!("- `{dir}/` - {purpose}."));
        }
    }
    if lines.is_empty() {
        "- No standard project directories detected.".to_string()
    } else {
        lines.join("\n")
    }
}

fn build_command_lines(scan: &ProjectScan) -> String {
    let mut lines = Vec::new();
    if scan_has_file(scan, "justfile") {
        lines.push("- `just build` - build through the project task runner.".to_string());
        lines.push("- `just lint` - run lint checks when configured.".to_string());
    }
    if scan_has_file(scan, "Cargo.toml") {
        lines.push("- `cargo build --workspace` - compile the Rust workspace.".to_string());
        lines.push("- `cargo test --workspace` - run workspace tests.".to_string());
    }
    if lines.is_empty() {
        "- Build commands not detected.".to_string()
    } else {
        lines.join("\n")
    }
}

fn test_lines(scan: &ProjectScan) -> String {
    if scan_has_file(scan, "Cargo.toml") {
        "- Rust tests live in crate-local `#[cfg(test)]` modules and integration tests under `crates/*/tests/`.\n- Run focused tests with `cargo test -p <crate> <filter>`.".to_string()
    } else {
        "- Test layout not detected from scanned files.".to_string()
    }
}

fn convention_lines(scan: &ProjectScan) -> String {
    let mut lines = Vec::new();
    if file_contains(
        scan,
        "AGENTS.md",
        "No `unwrap()` / `expect()` in library code",
    ) {
        lines.push(
            "- Library code avoids `unwrap()` / `expect()`; use typed errors and `?`.".to_string(),
        );
    }
    if file_contains(scan, "AGENTS.md", "tracing::*") {
        lines.push(
            "- Runtime logging uses `tracing::*`; CLI-only user output may use `println!`."
                .to_string(),
        );
    }
    if file_contains(scan, "AGENTS.md", "Workspace-managed dependencies") {
        lines.push("- Dependency versions belong in root `[workspace.dependencies]`.".to_string());
    }
    if file_contains(scan, "clippy.toml", "cognitive-complexity-threshold") {
        lines.push("- Strict clippy limits keep functions small and low-complexity.".to_string());
    }
    if lines.is_empty() {
        "- No conventions detected from scanned files.".to_string()
    } else {
        lines.join("\n")
    }
}

fn scan_has_file(scan: &ProjectScan, relative: &str) -> bool {
    let relative = Path::new(relative);
    scan.files
        .iter()
        .any(|file| file.relative_path.as_path() == relative)
}

fn file_contains(scan: &ProjectScan, relative: &str, needle: &str) -> bool {
    let relative = Path::new(relative);
    scan.files
        .iter()
        .find(|file| file.relative_path.as_path() == relative)
        .is_some_and(|file| file.content.contains(needle))
}

fn write_project_context(path: &Path, content: &str) -> Result<(), ProjectContextError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ProjectContextError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, content).map_err(|source| ProjectContextError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn high_signal_paths() -> Vec<PathBuf> {
    [
        "AGENTS.md",
        "CLAUDE.md",
        "README.md",
        "Cargo.toml",
        "justfile",
        "rustfmt.toml",
        "clippy.toml",
        "surge.toml",
        "surge.example.toml",
        "docs/README.md",
        "docs/ARCHITECTURE.md",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn skipped_directory_summary(root: &Path) -> Vec<SkippedFile> {
    let mut skipped = Vec::new();
    for dir in [
        "target",
        ".git",
        ".worktrees",
        "node_modules",
        "dist",
        "build",
    ] {
        let relative_path = PathBuf::from(dir);
        if root.join(&relative_path).exists() {
            debug!(path = %relative_path.display(), "project context skipped generated/heavy dir");
            skipped.push(SkippedFile {
                relative_path,
                reason: "generated_or_heavy_directory".to_string(),
                byte_len: None,
                hash: None,
            });
        }
    }
    skipped
}

fn redact_secret_like_values(raw: &str) -> (String, usize) {
    let mut redactions = 0;
    let mut out = Vec::new();
    for line in raw.lines() {
        if should_redact_line(line) {
            redactions += 1;
            out.push(redact_line(line));
        } else {
            out.push(line.to_string());
        }
    }
    (out.join("\n"), redactions)
}

fn should_redact_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if lower.contains("_env") {
        return false;
    }
    [
        "token", "secret", "password", "api_key", "apikey", "chat_id",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn redact_line(line: &str) -> String {
    if let Some((head, _)) = line.split_once('=') {
        return format!("{}= \"<redacted>\"", head.trim_end());
    }
    if let Some((head, _)) = line.split_once(':') {
        return format!("{}: <redacted>", head.trim_end());
    }
    "<redacted>".to_string()
}

fn git_snapshot(root: &Path) -> GitSnapshot {
    let branch = git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let dirty = git_output(root, &["status", "--short"]).map(|out| !out.trim().is_empty());
    debug!(
        branch = ?branch,
        dirty = ?dirty,
        "project context git snapshot collected"
    );
    GitSnapshot { branch, dirty }
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn render_scan_context(
    root: &Path,
    files: &[ScannedFile],
    skipped_files: &[SkippedFile],
    git: &GitSnapshot,
    scan_hash: Option<ContentHash>,
) -> String {
    let mut out = String::new();
    out.push_str("# Project scan\n\n");
    out.push_str(&format!("root: {}\n", root.display()));
    if let Some(hash) = scan_hash {
        out.push_str(&format!("scan_hash: {hash}\n"));
    }
    if let Some(branch) = &git.branch {
        out.push_str(&format!("git_branch: {branch}\n"));
    }
    if let Some(dirty) = git.dirty {
        out.push_str(&format!("git_dirty: {dirty}\n"));
    }
    out.push_str("\n## Included files\n");
    for file in files {
        out.push_str(&format!(
            "\n### `{}`\nbytes: {}\nhash: {}\nredactions: {}\n\n```text\n{}\n```\n",
            file.relative_path.display(),
            file.byte_len,
            file.hash,
            file.redaction_count,
            file.content
        ));
    }
    out.push_str("\n## Skipped files\n");
    for skipped in skipped_files {
        out.push_str(&format!(
            "- `{}`: {}",
            skipped.relative_path.display(),
            skipped.reason
        ));
        if let Some(byte_len) = skipped.byte_len {
            out.push_str(&format!(" ({byte_len} bytes)"));
        }
        if let Some(hash) = skipped.hash {
            out.push_str(&format!(" {hash}"));
        }
        out.push('\n');
    }
    out
}

fn extract_scan_hash(content: &str) -> Option<ContentHash> {
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("<!-- ") else {
            continue;
        };
        let Some(hash_part) = rest.strip_prefix(SCAN_HASH_MARKER) else {
            continue;
        };
        let raw = hash_part.trim_end_matches(" -->").trim();
        return raw.parse().ok();
    }
    None
}
