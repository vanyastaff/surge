//! Operator services: the read/answer logic every operator surface shares.
//!
//! `surge` CLI subcommands, the MCP server (`surge mcp serve`), the desktop
//! app and the Telegram bot are thin adapters over these functions — they own
//! argument parsing, rendering and daemon connection, never the classification,
//! validation or compilation itself.
//!
//! - [`inbox`] groups runs by what they need from the operator.
//! - [`pending`] inspects a blocked run and builds/delivers the answer.
//! - [`backlog`] lists the actionable backlog and the full task ledger.
//! - [`steer`] queues a steer message; [`memory`] searches project memory.
//! - [`report`] compiles the Run Report and the OTLP trace.
//! - [`resolve_run_id`] turns an operator-typed id or suffix into a [`RunId`].
//!
//! [`RunId`]: surge_core::RunId

pub mod backlog;
mod error;
pub mod inbox;
mod journal;
pub mod memory;
pub mod pending;
pub mod report;
mod run_id;
pub mod steer;

pub use backlog::{LedgerQuery, ReadyQuery, query_ledger, query_ready};
pub use error::{OperatorError, OperatorErrorKind, RunIdError};
pub use inbox::{AttentionGroup, DoneReason, InboxEntry, classify, collect_entries};
pub use journal::{fold_run_state, read_run_events};
pub use memory::{MemoryQuery, query_memory};
pub use pending::{
    GateOption, OperatorAnswer, PendingInput, PendingKind, ValidatedAnswer, deliver_answer,
    inspect_pending,
};
pub use report::{compile_report, compile_trace};
pub use run_id::resolve_run_id;
pub use steer::{cancel_steer, list_steers, queue_steer};
