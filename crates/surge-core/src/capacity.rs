//! Learned per-agent-account rate-limit capacity (R34–R36).
//!
//! A provider's rate-limit window length is a fact about a third party.
//! Surge does not ship a table of them (they drift silently, and a stale
//! constant lies in the direction of refusing work that would have
//! succeeded) — every field on [`CapacityWindow`] except `account` and
//! `source` is learned from an actual observation and is `None` until one
//! arrives. On a fresh machine there has never been an observation, so
//! `None` is the common case, not an edge case.
//!
//! Two independent things populate a window:
//! - an ACP `session/usage` signal ([`CapacitySource::AcpUsage`]) — checked
//!   against the actual crate source, not assumed: `agent-client-protocol`
//!   0.10.2's `unstable_session_usage` feature exposes exactly
//!   `UsageUpdate{used, size, cost, meta}` (context-tokens-used, context
//!   window size, optional cost) and `Usage{total_tokens, input_tokens,
//!   output_tokens, thought_tokens, cached_read_tokens,
//!   cached_write_tokens}` — see `agent-client-protocol-schema-0.11.2/src/
//!   client.rs:247-260` and `.../src/agent.rs:2763-2779`. Neither carries a
//!   provider rate-limit window, remaining share, or reset time, and both
//!   are `#[non_exhaustive]` with a `_meta` field whose own doc forbids
//!   assuming anything about its contents. There is nothing to wire, not
//!   "not wired yet" — the brief's "where available" is satisfied vacuously
//!   for this SDK version. The variant stays on the type (see
//!   `#[non_exhaustive]` below) so a future SDK that does expose one does
//!   not force a breaking change here;
//! - an observed HTTP 429 ([`CapacitySource::Observed429`]), which *is*
//!   live-wired: see [`CapacityWindow::from_observed_error`].
//!
//! `account` names an identifier from Surge's *existing* agent-profile
//! configuration (the `AgentPool` connection name / `SurgeConfig.agents`
//! key) — this module does not define a second place credentials or
//! account identity could live, and never reads, copies, or stores any.
//!
//! A provider retry delay Surge never actually saw is not an observation:
//! `surge-acp`'s pool sleeps on a policy default (60s) when a rate-limited
//! response carries no `Retry-After`, because it has to sleep on *some*
//! number — but that default belongs to the backoff policy alone and never
//! reaches [`CapacityWindow::resets_at`], which stays `None` in that case
//! (see [`CapacityWindow::observed_429`]). A run can legitimately sleep 60s
//! by policy while the very same account's capacity window still reports
//! `resets_at: None` — that is not a contradiction to reconcile, it is two
//! different questions (how long to wait vs. what Surge actually knows)
//! answered honestly.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where the numbers in a [`CapacityWindow`] came from.
///
/// `#[non_exhaustive]`: a third source is exactly the case
/// [`CapacitySource::AcpUsage`]'s doc anticipates (a future ACP SDK that
/// does expose a rate-limit signal) — without this marker, adding it would
/// silently break every external `match` on this type instead of forcing
/// each one to decide how to handle the new source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CapacitySource {
    /// Read from an ACP `session/usage` signal (`unstable_session_usage`).
    /// No constructor exists for this variant in this delivery: verified
    /// against `agent-client-protocol` 0.10.2 / `agent-client-protocol-
    /// schema` 0.11.2 that no field carries the data a constructor would
    /// need (see the module docs above).
    AcpUsage,
    /// Inferred from an observed HTTP 429 (rate-limit) response.
    Observed429,
}

/// A remaining-capacity share, validated to `[0.0, 1.0]` so a caller never
/// has to guard against a nonsensical fraction.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RemainingShare(f64);

impl RemainingShare {
    /// No capacity left — the state right after an observed 429.
    pub const EXHAUSTED: Self = Self(0.0);
    /// The full window, unused.
    pub const FULL: Self = Self(1.0);

    /// Validate `value` as a remaining share.
    ///
    /// # Errors
    /// [`InvalidRemainingShare`] when `value` is outside `[0.0, 1.0]`.
    pub fn new(value: f64) -> Result<Self, InvalidRemainingShare> {
        if (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidRemainingShare(value))
        }
    }

    /// The fraction of the window still available, in `[0.0, 1.0]`.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

/// [`RemainingShare::new`] was given a value outside `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("remaining share must be within [0.0, 1.0], got {0}")]
pub struct InvalidRemainingShare(f64);

/// Word/phrase patterns that mark a free-text error/response as a
/// rate-limit (or capacity-exhaustion) signal, case-insensitive. Plain
/// substring patterns only — the standalone HTTP `429` status code is
/// handled separately by [`has_standalone_429`], because a bare `"429"`
/// substring also matches a byte count, a millisecond duration, a port
/// number, or a hex id (`"read 1429 bytes"`, `"timed out after 4290ms"`);
/// a capacity model whose entire premise is "never invent a signal" cannot
/// use a check that manufactures one from an unrelated number.
///
/// This is currently read by `surge doctor agent`'s ad hoc probe
/// classification, `surge inbox`'s run-event-log scan, and half of
/// `surge-acp::health::record_failure` — the half that populates the R34–R36
/// capacity observation, an operator-facing surface with no automatic-retry
/// consequence of its own.
///
/// It is deliberately **not** shared with the other half of that same
/// method (`rate_limited`/`rate_limit_reset`, which drives
/// `resolve_agent`'s fallback-routing decision — see
/// `is_rate_limited_for_routing` in `surge-acp::health`) or with
/// `surge-acp::pool`'s `is_rate_limit` (which decides whether to
/// short-circuit that pool's retry loop): a prior round of review found
/// that unifying onto this wider list had silently changed
/// `surge-acp::pool`'s retry-loop short-circuit behavior for four patterns
/// nobody had decided to change, with a comment claiming otherwise.
/// Widening a policy-affecting classifier is a decision to own with
/// dedicated tests proving the new policy, not a side effect of code
/// reuse — so this list stays scoped to consumers where over-inclusion
/// costs nothing but a displayed line, and each routing decision keeps its
/// own narrow, separately-tested classifier.
///
/// The set:
/// - `surge-acp::pool`'s pre-existing `is_rate_limit` patterns that are not
///   already subsumed by another entry here (`"rate_limit_exceeded"` is
///   deliberately omitted: `"rate_limit"` already matches it as a
///   substring, and a redundant pattern that can never independently fire
///   is dead weight a test would not catch drifting);
/// - four real provider error shapes neither classifier recognized before
///   this module existed (`rate_limit_error` and `rate_limit_exceeded`,
///   named in the brief that first motivated this list, were already
///   covered by pool's pre-existing `"rate_limit"` substring — the actual
///   net-new set is exactly these four), added so an unrecognized shape
///   does not read as "nothing happened" (see
///   [`CapacityWindow::from_observed_error`]'s doc and
///   `CapacityStatus::Unclassified`).
const RATE_LIMIT_PATTERNS: &[&str] = &[
    // From `surge-acp::pool`'s pre-existing `is_rate_limit` (its `"429"`
    // entry is handled by `has_standalone_429` instead, see above).
    "rate limit",
    "rate_limit",
    "too many requests",
    "quota exceeded",
    // Real provider shapes neither classifier recognized before.
    "insufficient_quota",  // OpenAI
    "resource_exhausted",  // Google (`RESOURCE_EXHAUSTED`; matched lowercased)
    "overloaded_error",    // Anthropic
    "usage limit reached", // prose
];

/// `true` when `lower` (already-lowercased text) contains `"429"` as a
/// standalone number — not embedded in a longer digit run on either side.
/// Matches `"429"`, `"HTTP 429"`, `"Request failed with 429"`; rejects
/// `"1429"`, `"4290"`, `"port 14290"`.
fn has_standalone_429(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut search_from = 0;
    while let Some(rel_idx) = lower[search_from..].find("429") {
        let idx = search_from + rel_idx;
        let preceded_by_digit = idx > 0 && bytes[idx - 1].is_ascii_digit();
        let followed_by_digit = bytes.get(idx + 3).is_some_and(u8::is_ascii_digit);
        if !preceded_by_digit && !followed_by_digit {
            return true;
        }
        search_from = idx + 1;
    }
    false
}

/// Case-insensitive detection of an HTTP 429 / rate-limit / quota-exhaustion
/// signal in a free-text error/response message. See [`RATE_LIMIT_PATTERNS`]
/// and [`has_standalone_429`] for the exact rules and why each earns its
/// place.
#[must_use]
pub fn looks_like_rate_limit(text: &str) -> bool {
    let lower = text.to_lowercase();
    has_standalone_429(&lower)
        || RATE_LIMIT_PATTERNS
            .iter()
            .any(|pattern| lower.contains(pattern))
}

/// Best-effort `Retry-After` (seconds) parse from a free-text error message.
/// Case-insensitive; supports both a header-style delay-seconds value
/// (`"Retry-After: 120"`) and the prose form Surge's own
/// [`crate::error::SurgeError::RateLimit`] renders (`"retry after 30s"`).
/// Returns `None` when absent — callers must not invent a default; see
/// [`CapacityWindow::observed_429`].
#[must_use]
pub fn parse_retry_after_secs(text: &str) -> Option<u64> {
    let lower = text.to_lowercase();
    let (marker_pos, marker_len) = if let Some(pos) = lower.find("retry-after") {
        (pos, "retry-after".len())
    } else {
        let pos = lower.find("retry after")?;
        (pos, "retry after".len())
    };
    let after = &text[marker_pos + marker_len..];
    let after = after.trim_start();
    let after = after.strip_prefix(':').unwrap_or(after).trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u64>().ok()
}

/// A learned rate-limit window for one configured agent account.
///
/// `window`, `remaining`, and `resets_at` are `Option` because they are
/// facts Surge learns by observation, never hardcodes: absent means "not
/// yet observed", not "assume some default". A caller that only knows
/// `remaining` (e.g. right after a 429) and not `window` must not guess the
/// window — leave it `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapacityWindow {
    account: String,
    window: Option<Duration>,
    remaining: Option<RemainingShare>,
    resets_at: Option<DateTime<Utc>>,
    source: CapacitySource,
}

impl CapacityWindow {
    /// Identifier of the agent account this window belongs to — an
    /// identifier from Surge's existing agent-profile configuration, not a
    /// new account model.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }

    /// Length of the provider's rate-limit window, if ever learned from
    /// observation. `None` on a fresh account — never a fabricated default.
    #[must_use]
    pub fn window(&self) -> Option<Duration> {
        self.window
    }

    /// Fraction of the window still available, if known.
    #[must_use]
    pub fn remaining(&self) -> Option<RemainingShare> {
        self.remaining
    }

    /// When the window resets, if known.
    #[must_use]
    pub fn resets_at(&self) -> Option<DateTime<Utc>> {
        self.resets_at
    }

    /// Where these numbers came from.
    #[must_use]
    pub fn source(&self) -> CapacitySource {
        self.source
    }

    /// Build a window from an observed HTTP 429: the account's remaining
    /// share is exhausted *right now*, by definition. `retry_after` (when
    /// the provider actually sent one) fixes `resets_at`; a missing
    /// `Retry-After` leaves `resets_at` unknown rather than assuming a
    /// fallback delay.
    ///
    /// A single 429 does not reveal the window's *length* — only that the
    /// account is exhausted and, maybe, when it resets. A caller that has
    /// tracked an earlier reset for this account can widen the result with
    /// [`Self::with_learned_window`].
    #[must_use]
    pub fn observed_429(
        account: impl Into<String>,
        retry_after: Option<Duration>,
        observed_at: DateTime<Utc>,
    ) -> Self {
        let resets_at = retry_after.and_then(|d| {
            i64::try_from(d.as_secs())
                .ok()
                .map(|secs| observed_at + chrono::Duration::seconds(secs))
        });
        Self {
            account: account.into(),
            window: None,
            remaining: Some(RemainingShare::EXHAUSTED),
            resets_at,
            source: CapacitySource::Observed429,
        }
    }

    /// Try to read an observed-429 capacity signal out of a free-text
    /// error/response message (whatever an ACP bridge or CLI command
    /// surfaced). Returns `None` when the message does not look like a
    /// rate-limit response — an ordinary failure carries no capacity
    /// information, and inventing one would be exactly the fabricated
    /// default this module refuses to produce.
    ///
    /// A caller that also needs to tell "definitely not a rate limit" apart
    /// from "no failure happened at all" (the distinction
    /// [`CapacityStatus::Unclassified`] exists for) must track that itself
    /// — this constructor only answers the one question in its name.
    #[must_use]
    pub fn from_observed_error(
        account: impl Into<String>,
        error: &str,
        observed_at: DateTime<Utc>,
    ) -> Option<Self> {
        if !looks_like_rate_limit(error) {
            return None;
        }
        let retry_after = parse_retry_after_secs(error).map(Duration::from_secs);
        Some(Self::observed_429(account, retry_after, observed_at))
    }

    /// Learn the window length from the elapsed time between two
    /// consecutive observed resets of the same account: the only honest
    /// source of a provider's window length, since Surge never ships one.
    /// A no-op when this window has no `resets_at` yet, or when
    /// `previous_reset_at` is not strictly earlier than it.
    #[must_use]
    pub fn with_learned_window(self, previous_reset_at: DateTime<Utc>) -> Self {
        let window = self.resets_at.and_then(|reset_at| {
            (reset_at > previous_reset_at)
                .then(|| (reset_at - previous_reset_at).to_std().ok())
                .flatten()
        });
        match window {
            Some(window) => Self {
                window: Some(window),
                ..self
            },
            None => self,
        }
    }

    /// `true` once `remaining` is known to be fully exhausted.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        matches!(self.remaining, Some(r) if r.get() <= 0.0)
    }

    /// Seconds from `now` until `resets_at`, when a reset time is known and
    /// still in the future. `None` when the reset time is unknown *or*
    /// already passed (a stale, past `resets_at` is not evidence the
    /// account is still exhausted — it just means Surge has no fresher
    /// signal).
    #[must_use]
    pub fn seconds_until_reset(&self, now: DateTime<Utc>) -> Option<i64> {
        self.resets_at
            .map(|reset_at| (reset_at - now).num_seconds())
            .filter(|secs| *secs > 0)
    }
}

/// What Surge has to say about an account's rate-limit capacity, including
/// the difference between two kinds of "no window" that must never look the
/// same to an operator:
///
/// - a fresh account nobody has ever heard a failure from
///   ([`Self::NeverObserved`]) — the common, expected case (R35.1);
/// - an account that *has* failed, but whose failure(s) matched none of
///   [`RATE_LIMIT_PATTERNS`] ([`Self::Unclassified`]) — Surge saw something
///   and cannot say whether it was a rate limit.
///
/// Folding the second into the first is exactly the failure mode a
/// closed-form classifier produced for the memory-claim locator audit: an
/// unrecognized shape silently vanished into "nothing to report" instead of
/// surfacing as "something Surge could not read". Widening
/// [`RATE_LIMIT_PATTERNS`] shrinks how often `Unclassified` fires; it cannot
/// prove it will never fire again, so the distinction has to stay
/// representable rather than resolved by "add enough patterns".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CapacityStatus {
    /// No failure has ever been observed for this account, within whatever
    /// this status's producer's observation is scoped to: one process's
    /// in-memory tracker since it started (`surge-acp::health`), or one
    /// run's persisted event log (`surge inbox`'s scan).
    NeverObserved,
    /// A capacity window was learned from an observed 429.
    Known(CapacityWindow),
    /// At least one failure was observed for this account, but none of them
    /// matched [`RATE_LIMIT_PATTERNS`] — Surge cannot say whether the
    /// account is rate-limited or failing for an unrelated reason.
    ///
    /// Known imprecision in `surge-acp::health`'s producer of this variant:
    /// its "at least one failure" count is a strict superset of that
    /// tracker's own `total_failures` — it also counts failures already
    /// unambiguously classified as something *other* than a rate limit by
    /// the caller (a 401 auth failure, a connection reset, a timeout).
    /// Narrowing this would need importing those classifiers into the
    /// count, a change outside R34–R36's scope; until then, read this
    /// variant as "at least one failure happened and none of them looked
    /// like a rate limit", not "this failure might be a rate limit".
    Unclassified,
}

impl CapacityStatus {
    /// The learned window, if this status is [`Self::Known`]. `None` for
    /// both [`Self::NeverObserved`] and [`Self::Unclassified`] — a caller
    /// that only wants the number and not the distinction between "no data"
    /// and "unreadable data" can use this; a caller building an operator
    /// surface should match on the full enum instead.
    #[must_use]
    pub fn window(&self) -> Option<&CapacityWindow> {
        match self {
            Self::Known(window) => Some(window),
            Self::NeverObserved | Self::Unclassified => None,
        }
    }

    /// `true` for [`Self::NeverObserved`] — used to keep the common case
    /// out of JSON output (`#[serde(skip_serializing_if = ...)]`) without
    /// hiding [`Self::Unclassified`], which must stay visible.
    #[must_use]
    pub fn is_never_observed(&self) -> bool {
        matches!(self, Self::NeverObserved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("valid literal")
            .with_timezone(&Utc)
    }

    #[test]
    fn from_observed_error_ignores_non_rate_limit_text() {
        // The primary case: an ordinary failure carries no capacity
        // information, and this must not manufacture one.
        assert!(
            CapacityWindow::from_observed_error("claude", "internal server error", Utc::now())
                .is_none()
        );
    }

    #[test]
    fn observed_429_with_retry_after_sets_resets_at() {
        let observed_at = t("2026-01-01T00:00:00Z");
        let window = CapacityWindow::observed_429(
            "claude-work",
            Some(Duration::from_secs(120)),
            observed_at,
        );
        assert_eq!(window.account(), "claude-work");
        assert_eq!(window.source(), CapacitySource::Observed429);
        assert_eq!(window.remaining(), Some(RemainingShare::EXHAUSTED));
        assert_eq!(window.window(), None);
        // Hand-computed expected instant, not re-derived from the formula
        // under test: 00:00:00 + 120s = 00:02:00.
        assert_eq!(window.resets_at(), Some(t("2026-01-01T00:02:00Z")));
    }

    #[test]
    fn observed_429_without_retry_after_leaves_resets_at_unknown() {
        // The provider sent no Retry-After: `resets_at` stays `None`, never
        // a fallback delay (that would be the invented default R35.1 rules
        // out).
        let window = CapacityWindow::observed_429("claude", None, Utc::now());
        assert_eq!(window.resets_at(), None);
        assert_eq!(window.remaining(), Some(RemainingShare::EXHAUSTED));
    }

    #[test]
    fn with_learned_window_computes_duration_between_two_resets() {
        let previous_reset_at = t("2026-01-01T00:00:00Z");
        let second_reset_at = t("2026-01-01T01:00:00Z");
        // `Duration::ZERO` retry-after observed exactly at the second reset
        // gives a window whose `resets_at` is precisely `second_reset_at`.
        let window = CapacityWindow::observed_429("claude", Some(Duration::ZERO), second_reset_at)
            .with_learned_window(previous_reset_at);
        // Hand-computed: one hour between the two resets.
        assert_eq!(window.window(), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn with_learned_window_is_a_no_op_without_a_known_reset() {
        let window = CapacityWindow::observed_429("claude", None, Utc::now())
            .with_learned_window(Utc::now());
        assert_eq!(window.window(), None);
    }

    #[test]
    fn is_exhausted_true_right_after_observed_429() {
        let window = CapacityWindow::observed_429("claude", None, Utc::now());
        assert!(window.is_exhausted());
    }

    #[test]
    fn seconds_until_reset_positive_before_and_none_after() {
        let observed_at = t("2026-01-01T00:00:00Z");
        let window =
            CapacityWindow::observed_429("claude", Some(Duration::from_secs(60)), observed_at);
        assert_eq!(
            window.seconds_until_reset(t("2026-01-01T00:00:30Z")),
            Some(30)
        );
        // Past the reset time: no fresher signal, not "still exhausted".
        assert_eq!(window.seconds_until_reset(t("2026-01-01T00:01:30Z")), None);
    }

    #[test]
    fn remaining_share_rejects_out_of_range() {
        assert!(RemainingShare::new(1.5).is_err());
        assert!(RemainingShare::new(-0.1).is_err());
        assert!(RemainingShare::new(0.5).is_ok());
    }

    #[test]
    fn parse_retry_after_secs_various_formats() {
        assert_eq!(parse_retry_after_secs("Retry-After: 120"), Some(120));
        assert_eq!(parse_retry_after_secs("retry-after: 30"), Some(30));
        assert_eq!(
            parse_retry_after_secs("429 Too Many Requests; Retry-After: 60"),
            Some(60)
        );
        assert_eq!(parse_retry_after_secs("no retry header here"), None);
        // Prose form: exactly what `SurgeError::RateLimit`'s own `Display`
        // renders — this is the text that ends up in a run's persisted
        // `StageFailed.reason`, so the parser must read Surge's own shape,
        // not only a raw provider header.
        assert_eq!(
            parse_retry_after_secs(
                "Rate limit exceeded for agent 'claude': retry after 30s (attempt 2)"
            ),
            Some(30)
        );
    }

    #[test]
    fn looks_like_rate_limit_matches_pools_pre_existing_patterns() {
        // `surge-acp::pool`'s own `is_rate_limit` (a separate, unchanged
        // classifier — see `RATE_LIMIT_PATTERNS`'s doc for why the two stay
        // apart) already covered all of these; this classifier must too, or
        // an operator-facing surface would show less than the pool already
        // knew.
        assert!(looks_like_rate_limit("429 Too Many Requests"));
        assert!(looks_like_rate_limit("Rate limit exceeded"));
        assert!(looks_like_rate_limit("Too many requests, slow down"));
        assert!(looks_like_rate_limit("quota exceeded for this month"));
    }

    #[test]
    fn rate_limit_exceeded_is_caught_by_the_rate_limit_substring_not_a_dedicated_pattern() {
        // `"rate_limit_exceeded"` earns no entry of its own in
        // `RATE_LIMIT_PATTERNS` — `"rate_limit"` already matches it as a
        // substring. This test is what would catch that entry silently
        // becoming load-bearing (e.g. if `"rate_limit"` were ever narrowed)
        // without anyone noticing it was the last thing still matching this
        // shape.
        assert!(looks_like_rate_limit("rate_limit_exceeded"));
        assert!(looks_like_rate_limit("rate_limit_error")); // Anthropic; same substring
    }

    #[test]
    fn looks_like_rate_limit_matches_real_provider_shapes() {
        // These four are the actual net-new coverage this module adds:
        // `rate_limit_error`/`rate_limit_exceeded` were already reachable
        // through pool's pre-existing `"rate_limit"` substring (see the
        // test above) — claiming otherwise here would be exactly the kind
        // of overstated test-comment a later audit should catch.
        assert!(looks_like_rate_limit("insufficient_quota")); // OpenAI
        assert!(looks_like_rate_limit("RESOURCE_EXHAUSTED")); // Google (any case)
        assert!(looks_like_rate_limit("overloaded_error")); // Anthropic
        assert!(looks_like_rate_limit(
            "Usage limit reached for this account"
        ));
    }

    #[test]
    fn looks_like_rate_limit_rejects_unrelated_failures() {
        assert!(!looks_like_rate_limit("internal server error"));
        // The bare word "too many" (without "requests") is deliberately
        // excluded: this is capacity-unrelated, and a classifier keyed on
        // it alone would misfire on it.
        assert!(!looks_like_rate_limit("too many open connections"));
    }

    #[test]
    fn has_standalone_429_matches_the_status_code_not_an_embedded_number() {
        assert!(has_standalone_429("429"));
        assert!(has_standalone_429("HTTP 429"));
        assert!(has_standalone_429("Request failed with 429"));
        // Embedded in a longer digit run on either side: not a status code.
        assert!(!has_standalone_429("read 1429 bytes"));
        assert!(!has_standalone_429("timed out after 4290ms"));
        assert!(!has_standalone_429("port 14290"));
    }

    #[test]
    fn looks_like_rate_limit_rejects_429_embedded_in_a_longer_number() {
        // The regression this module's own review round caught: a bare
        // `"429"` substring check would manufacture a rate-limit signal out
        // of an unrelated byte count or duration — exactly the invented
        // signal this module exists to refuse.
        assert!(!looks_like_rate_limit("read 1429 bytes from the socket"));
        assert!(!looks_like_rate_limit("connection timed out after 4290ms"));
        assert!(looks_like_rate_limit("Request failed with 429"));
    }

    #[test]
    fn capacity_status_window_is_some_only_when_known() {
        let known = CapacityStatus::Known(CapacityWindow::observed_429("claude", None, Utc::now()));
        assert!(known.window().is_some());
        assert!(CapacityStatus::NeverObserved.window().is_none());
        assert!(CapacityStatus::Unclassified.window().is_none());
    }

    #[test]
    fn capacity_status_is_never_observed_distinguishes_from_unclassified() {
        // The whole point of the type: these two must not collapse into
        // the same predicate.
        assert!(CapacityStatus::NeverObserved.is_never_observed());
        assert!(!CapacityStatus::Unclassified.is_never_observed());
    }
}
