//! Internal helpers shared by legacy `SurgeClient` and the new
//! `bridge::BridgeClient`. Not part of `surge-acp`'s public API
//! (the module is declared `mod shared;` in `lib.rs`, not `pub mod`).

pub(crate) mod content_block;
pub mod path_guard;
pub(crate) mod secrets;
