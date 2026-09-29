//! Operator services: the read/answer logic every operator surface shares.
//!
//! `surge` CLI subcommands, the MCP server (`surge mcp serve`), the desktop
//! app and the Telegram bot are thin adapters over these functions — they own
//! argument parsing, rendering and daemon connection, never the classification,
//! validation or compilation itself.
//!
//! - [`inbox`] groups runs by what they need from the operator.
//! - [`pending`] inspects a blocked run and builds/delivers the answer.
//! - [`report`] compiles the Run Report and the OTLP trace.
//! - [`resolve_run_id`] turns an operator-typed id or suffix into a [`RunId`].
//!
//! [`RunId`]: surge_core::RunId

mod error;
pub mod inbox;
mod journal;
pub mod pending;
pub mod report;
mod run_id;

pub use error::{OperatorError, RunIdError};
pub use inbox::{AttentionGroup, InboxEntry, classify, collect_entries};
pub use journal::{fold_run_state, read_run_events};
pub use pending::{PendingInput, build_answer, deliver_answer, inspect_pending};
pub use report::{compile_report, compile_trace};
pub use run_id::resolve_run_id;
