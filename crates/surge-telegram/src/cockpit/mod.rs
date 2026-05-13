//! Cockpit dispatch — the seam between the engine's [`RunEventTap`] broadcast
//! and the per-card emitter.
//!
//! Drives the event-to-card mapping table fixed in Decision 11 of the
//! Telegram cockpit milestone plan: each engine event is translated into
//! exactly one card action.

pub mod callback;
pub mod dispatch;

pub use callback::{
    Admission, CallbackCtx, CallbackOutcome, CallbackParseError, CallbackVerb, EngineResolver,
    ParsedCallback, handle_callback, parse_callback_data,
};
pub use dispatch::{CockpitCtx, DispatchOutcome, dispatch};
