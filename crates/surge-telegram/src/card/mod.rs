//! Telegram cockpit card rendering — the public surface a cockpit caller
//! uses to produce `RenderedCard` values for every event-driven card kind.
//!
//! Card kinds and their button layouts are pinned by
//! [ADR 0011](../../../../docs/adr/0011-telegram-card-lifecycle.md);
//! `callback_data` strings follow
//! [ADR 0010](../../../../docs/adr/0010-telegram-callback-schema.md).

pub mod render;

pub use render::{
    CardKind, RenderedCard, render_bootstrap, render_completion, render_escalation,
    render_failure, render_human_gate, render_status,
};
