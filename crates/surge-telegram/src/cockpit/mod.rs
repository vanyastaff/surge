//! Cockpit dispatch — the seam between the engine's [`RunEventTap`] broadcast
//! and the per-card emitter.
//!
//! Drives the event-to-card mapping table fixed in Decision 11 of the
//! Telegram cockpit milestone plan: each engine event is translated into
//! exactly one card action.

pub mod callback;
pub mod dispatch;
pub mod production;
pub mod recover;
pub mod run;
pub mod snooze;

pub use callback::{
    Admission, CallbackCtx, CallbackOutcome, CallbackParseError, CallbackVerb, EngineResolver,
    ParsedCallback, handle_callback, parse_callback_data,
};
pub use dispatch::{CockpitCtx, DispatchOutcome, dispatch};
pub use recover::{ReconcileReport, reconcile_open_cards};
pub use run::{CockpitRuntime, UpdateRoutes, drive_tap_loop, drive_update_loop, run_cockpit};
pub use snooze::{
    CockpitSnoozeQueue, CockpitSnoozeRescheduler, DueSnooze, SNOOZE_END_FOOTER, TickReport,
};
