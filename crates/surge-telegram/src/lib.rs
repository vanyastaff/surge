//! Telegram cockpit for Surge — bot loop, callback router, card store, and recovery.
//!
//! This crate owns the long-running Telegram surfaces that pair the operator's
//! mobile app to active Surge runs. `surge-notify` continues to own outbound
//! transport primitives (`TelegramDeliverer`, `format_inbox_card`); this crate
//! owns the callback receiver, the command handlers, the cards SQLite table,
//! and the reconciliation logic.
//!
//! See [ADR 0012](../../../docs/adr/0012-surge-telegram-crate-split.md) for
//! the crate split rationale.

pub mod card;
pub mod cockpit;
pub mod commands;
pub mod error;

pub use card::{
    CardEmitter, CardKind, CardStore, EmitOutcome, RenderedCard, TelegramApi, render_bootstrap,
    render_completion, render_escalation, render_failure, render_human_gate, render_status,
};
pub use cockpit::{CockpitCtx, DispatchOutcome, dispatch};
pub use error::{Result, TelegramCockpitError};
