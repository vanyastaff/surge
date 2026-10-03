//! Durable task controls use the daemon owner, never a second embedded engine.
use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;
use surge_core::{
    id::{WorkItemId, WorkItemOperationId},
    work_item::{WorkItemCommand, WorkItemPr, WorkItemRequirements},
};

#[derive(Debug, Subcommand)]
pub enum TaskCommands {
    /// Create a durable task with accepted requirements from a JSON file.
    Create {
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        title: String,
        #[arg(long)]
        requirements: PathBuf,
    },
    /// Inspect accepted requirements, workspace, PR and cumulative usage.
    Show { item: WorkItemId },
    /// Page durable tasks.
    List {
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Page immutable accepted revisions.
    Revisions {
        item: WorkItemId,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Page discussion and proposed amendments.
    Discussion {
        item: WorkItemId,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Page associated run attempts.
    Attempts {
        item: WorkItemId,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Explicitly accept revised requirements while the task is inactive.
    Edit {
        item: WorkItemId,
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        version: u64,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        requirements: PathBuf,
    },
    /// Add discussion or propose requirements without accepting them.
    Discuss {
        item: WorkItemId,
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        version: u64,
        body: String,
        #[arg(long)]
        proposal: Option<PathBuf>,
    },
    /// Explicitly accept a discussion proposal while the task is inactive.
    AcceptProposal {
        item: WorkItemId,
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        version: u64,
        #[arg(long)]
        revision: u64,
        proposal: u64,
    },
    /// Start one associated run using the current accepted revision.
    Start {
        item: WorkItemId,
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        version: u64,
        flow: PathBuf,
        /// Optional JSON document containing frozen `{"stages":[...]}` quota recovery policy.
        #[arg(long)]
        quota_recovery: Option<PathBuf>,
    },
    /// Suspend execution after confirmed cleanup without aborting its attempt.
    Suspend {
        item: WorkItemId,
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        version: u64,
    },
    /// Continue the same suspended attempt and saved provider session.
    Continue {
        item: WorkItemId,
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        version: u64,
        /// Explicitly replace an unrestorable provider session; never implied.
        #[arg(long)]
        new_session: bool,
    },
    /// Archive an inactive task without deleting its workspace or PR.
    Archive {
        item: WorkItemId,
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        version: u64,
    },
    /// Record the task's single normalized GitHub PR identity.
    AttachPr {
        item: WorkItemId,
        #[arg(long)]
        operation: WorkItemOperationId,
        #[arg(long)]
        version: u64,
        repository: String,
        number: u64,
    },
}
fn requirements(path: PathBuf) -> Result<WorkItemRequirements> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
impl TaskCommands {
    fn command(self) -> Result<WorkItemCommand> {
        Ok(match self {
            Self::Suspend {
                item,
                operation,
                version,
            } => WorkItemCommand::Suspend {
                item,
                operation_id: operation,
                expected_version: version,
            },
            Self::Continue {
                item,
                operation,
                version,
                new_session,
            } => WorkItemCommand::Continue {
                item,
                operation_id: operation,
                expected_version: version,
                new_session,
            },
            Self::Create {
                operation,
                project,
                title,
                requirements: path,
            } => WorkItemCommand::Create {
                operation_id: operation,
                project,
                title,
                requirements: requirements(path)?,
            },
            Self::Show { item } => WorkItemCommand::Show { item },
            Self::List { after, limit } => WorkItemCommand::List { after, limit },
            Self::Revisions { item, after, limit } => {
                WorkItemCommand::Revisions { item, after, limit }
            },
            Self::Discussion { item, after, limit } => {
                WorkItemCommand::Discussion { item, after, limit }
            },
            Self::Attempts { item, after, limit } => {
                WorkItemCommand::Attempts { item, after, limit }
            },
            Self::Edit {
                item,
                operation,
                version,
                revision,
                requirements: path,
            } => WorkItemCommand::Edit {
                item,
                operation_id: operation,
                expected_version: version,
                expected_revision: revision,
                requirements: requirements(path)?,
            },
            Self::Discuss {
                item,
                operation,
                version,
                body,
                proposal,
            } => WorkItemCommand::Discuss {
                item,
                operation_id: operation,
                expected_version: version,
                body,
                proposal: proposal.map(requirements).transpose()?,
            },
            Self::AcceptProposal {
                item,
                operation,
                version,
                revision,
                proposal,
            } => WorkItemCommand::AcceptProposal {
                item,
                operation_id: operation,
                expected_version: version,
                expected_revision: revision,
                proposal,
            },
            Self::Start {
                item,
                operation,
                version,
                flow,
                quota_recovery,
            } => WorkItemCommand::Start {
                item,
                operation_id: operation,
                expected_version: version,
                graph: Box::new(toml::from_str(&std::fs::read_to_string(flow)?)?),
                quota_recovery: quota_recovery
                    .map(|path| -> anyhow::Result<serde_json::Value> {
                        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
                    })
                    .transpose()?,
            },
            Self::Archive {
                item,
                operation,
                version,
            } => WorkItemCommand::Archive {
                item,
                operation_id: operation,
                expected_version: version,
            },
            Self::AttachPr {
                item,
                operation,
                version,
                repository,
                number,
            } => WorkItemCommand::AttachPr {
                item,
                operation_id: operation,
                expected_version: version,
                pr: WorkItemPr {
                    provider: "github".into(),
                    url: format!("https://github.com/{repository}/pull/{number}"),
                    repository,
                    number,
                },
            },
        })
    }
}
pub async fn execute(command: TaskCommands) -> Result<()> {
    crate::commands::engine::ensure_daemon_running().await?;
    let socket = surge_daemon::pidfile::socket_path()?;
    let facade =
        surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket).await?;
    let result = facade.work_item(command.command()?).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
