//! `surge-notify` — pluggable channel delivery for `NodeKind::Notify`.
//!
//! The crate exposes the `NotifyDeliverer` trait and a default
//! `MultiplexingNotifier` that dispatches on `NotifyChannel` variant
//! to one of five built-in channel impls (Desktop, Webhook, Slack,
//! Email, Telegram). See `docs/superpowers/specs/2026-05-04-surge-orchestrator-engine-m6-design.md`
//! §10 for the design contract.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

pub mod deliverer;
pub mod desktop;
pub mod multiplexer;
pub mod render;

pub use deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
pub use desktop::DesktopDeliverer;
pub use multiplexer::MultiplexingNotifier;
pub use render::{RenderContext, render};
