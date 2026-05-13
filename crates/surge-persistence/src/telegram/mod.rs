//! Telegram cockpit persistence — pairing tokens, paired-chat allowlist, cards.
//!
//! Tables live in the registry SQLite (`~/.surge/db/registry.sqlite`); see
//! `runs::migrations` for the migration sequence (`registry-0007-*` and
//! later).
//!
//! Repository functions take a `rusqlite::Connection` so the caller controls
//! transactions and pooling. The cockpit (`surge-telegram`) wraps these
//! helpers behind its own typed APIs.

pub mod cards;
pub mod pairing;
pub mod pairings;

pub use cards::{
    Card, CardsError, close as close_card, find_by_id as find_card_by_id,
    find_open as find_open_cards, mark_message_sent, update_content_hash, upsert as upsert_card,
};
pub use pairing::{PairingError, TOKEN_LEN, consume_pairing_token, mint_pairing_token};
pub use pairings::{Pairing, PairingsError, is_admitted, list_active, pair, revoke};
