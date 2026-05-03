//! Token-usage extraction from ACP `SessionUpdate` carrying
//! `unstable_session_usage` metadata. See spec §5.7.
//!
//! Single isolation point for the SDK shape — a future SDK upgrade only
//! touches this file.

use crate::bridge::session_inner::TokenUsageSnapshot;

/// Try to extract a cumulative usage snapshot from a `SessionUpdate`.
/// Returns `None` if the update carries no usage data (most updates don't).
///
/// **SDK 0.10.2 shape note:** The `unstable_session_usage` feature gate exposes
/// `SessionUpdate::UsageUpdate(UsageUpdate)`, which carries `used: u64` (tokens
/// currently in context) and `size: u64` (total context window) plus an optional
/// `cost`. It does **not** carry the per-request prompt/output/cache token counts
/// that `TokenUsageSnapshot` expects. A direct mapping is not possible without
/// additional per-turn accounting that falls outside the M3 scope.
///
/// Defensive: malformed payloads return `None`, not panic
/// (per spec §11.7 future-proofing). Phase 10's `bridge_token_tracking` test
/// will surface what the Mock agent actually emits on `MOCK_ACP_USAGE=on`.
#[allow(dead_code)] // wired in handle_session_notification (this task)
pub(crate) fn extract_usage(
    update: &agent_client_protocol::SessionUpdate,
) -> Option<TokenUsageSnapshot> {
    // The SDK's UsageUpdate carries context-window totals (`used`, `size`),
    // not per-request input/output/cache token breakdowns. Mapping would require
    // additional per-turn delta accounting outside M3 scope.
    //
    // Returning None here is the correct Phase 10 stub:
    // - The `flush_pending_token_usage` paths in subprocess_waiter /
    //   close_session_impl will be no-ops (nothing to flush).
    // - Phase 10's bridge_token_tracking test will confirm the gap and drive
    //   the real implementation (likely: delta accounting on top of UsageUpdate,
    //   or a separate unstable per-turn token feature once the SDK exposes it).
    let _ = update;
    None
}
