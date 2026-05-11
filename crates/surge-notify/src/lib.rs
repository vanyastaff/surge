//! `surge-notify` — pluggable channel delivery for `NodeKind::Notify`.
//!
//! The crate exposes the `NotifyDeliverer` trait and a default
//! `MultiplexingNotifier` that dispatches on `NotifyChannel` variant
//! to one of five built-in channel impls (Desktop, Webhook, Slack,
//! Email, Telegram). See `docs/ARCHITECTURE.md`
//! §10 for the design contract.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

pub mod deliverer;
pub mod desktop;
pub mod email;
pub mod messages;
pub mod multiplexer;
pub mod render;
pub mod slack;
pub mod telegram;
pub mod webhook;

pub use deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
pub use desktop::DesktopDeliverer;
pub use email::{EmailCredentials, EmailDeliverer, EmailSecretResolver};
pub use messages::{
    InboxCardPayload, NotifyMessage, RoadmapAmendmentNotificationKind,
    RoadmapAmendmentNotificationPayload,
};
pub use multiplexer::MultiplexingNotifier;
pub use render::{RenderContext, render};
pub use slack::{SlackCredentials, SlackDeliverer, SlackSecretResolver};
pub use telegram::{TelegramCredentials, TelegramDeliverer, TelegramSecretResolver};
pub use webhook::WebhookDeliverer;
