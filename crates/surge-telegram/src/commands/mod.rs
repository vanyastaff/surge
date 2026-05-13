//! Bot command handlers — `/pair`, `/status`, `/runs`, plus deferred
//! mutating commands.
//!
//! Each handler is an `async fn` that takes a parsed command + the
//! dependencies it needs and returns a [`CommandReply`]. The teloxide
//! command dispatcher (in the bot loop, not landed yet) parses incoming
//! `/command args` messages and routes to the matching handler.

pub mod pair;
pub mod runs;
pub mod status;

pub use pair::{PairingTokenConsumer, PairingWriter, handle_pair};
pub use runs::{RunListProvider, RunRow, handle_runs};
pub use status::{RunSnapshotProvider, handle_status};

/// Reply produced by a bot command handler. The bot loop translates this
/// into a `bot.send_message(chat_id, reply.text)` call with Markdown
/// formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReply {
    /// Markdown-formatted message body.
    pub text: String,
}

impl CommandReply {
    /// Construct a reply from any `impl Into<String>`.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}
