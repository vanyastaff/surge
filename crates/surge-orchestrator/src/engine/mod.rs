//! Engine — drives a frozen `Graph` through ACP sessions and persistence.
//!
//! See `docs/ARCHITECTURE.md`
//! for the full design contract. M5 ships sequential-pipeline-only support.
//! M6 adds loop execution, subgraph execution, and `Notify` delivery.
//!
//! # M6 Surface
//!
//! | Module | Purpose |
//! |---|---|
//! | [`frames`] | `LoopFrame` / `SubgraphFrame` stack, [`TerminalSignal`] routing |
//! | [`stage::loop_stage`] | `execute_loop_entry`, `on_loop_iteration_done` |
//! | [`stage::subgraph_stage`] | `execute_subgraph_entry`, `on_subgraph_done` |
//! | [`stage::notify`] | `execute_notify_stage` with [`surge_notify::NotifyDeliverer`] |
//! | [`routing`] | `next_node_after_with_counters` — edge `max_traversals` cap |
//! | [`validate`] | `validate_for_m6` — rejects multi-edge fanout (M8+) |
//!
//! # Constructor variants
//!
//! - [`Engine::new`]: default no-op `NotifyDeliverer` (log-only).
//! - [`Engine::new_with_notifier`]: production wiring with a real [`surge_notify::MultiplexingNotifier`].

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

// Submodules added incrementally as later phases land.
pub mod bootstrap;
pub mod config;
pub mod daemon_facade;
pub mod elevation;
pub mod engine;
pub mod error;
pub mod facade;
pub mod frames;
pub mod handle;
pub mod hooks;
pub mod ipc;
pub mod predicates;
pub mod replay;
pub mod routing;
pub mod run_task;
pub mod sandbox_factory;
pub mod snapshot;
pub mod stage;
pub mod tools;
pub mod validate;

pub use config::{EngineConfig, EngineRunConfig, SnapshotPolicy};
pub use daemon_facade::{DaemonClient, DaemonEngineFacade};
pub use engine::Engine;
pub use error::EngineError;
pub use facade::{EngineFacade, LocalEngineFacade};
pub use frames::{Frame, LoopFrame, SubgraphFrame, TerminalSignal};
pub use handle::{EngineRunEvent, RunHandle, RunOutcome, RunStatus, RunSummary};
pub use ipc::{
    DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent, RequestId,
};
