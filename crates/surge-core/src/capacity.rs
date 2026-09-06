//! Learned per-agent-runtime rate-limit capacity (R34–R36), and the pure
//! parking/rotation policy built on top of it (R37/R37.1/R38/R38.1, Task 12
//! M1 — see `docs/adr/0016-capacity-parking-and-wake.md`).
//!
//! A provider's rate-limit window length is a fact about a third party.
//! Surge does not ship a table of them (they drift silently, and a stale
//! constant lies in the direction of refusing work that would have
//! succeeded) — every field on [`CapacityWindow`] except `runtime` and
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
//! `runtime` names the canonical agent-runtime registry id (the result of
//! `surge_acp::Registry::normalize_agent_id`, e.g. `"claude-acp"`) — **not**
//! a distinct-login identifier. This module does not define a second place
//! credentials or login identity could live, and never reads, copies, or
//! stores any.
//!
//! **Why the key is the runtime, and not a separately-tracked login (A1,
//! Task 12 revision 6):** Surge never stores provider credentials — which
//! login is active is physically whatever the agent runtime's own CLI
//! config resolves to, and today's model has exactly one launch
//! configuration per registered runtime (`builtin_registry.json` carries
//! one entry per runtime; there is no second axis to key a second login
//! on). So for every installation expressible today, the runtime and the
//! logged-in identity coincide, and the runtime's canonical registry id is
//! the only key the engine path can actually produce — and produce
//! *stably*: `agent_id` aliases (`claude`, `claude-code`, `claude-acp`)
//! collapse to one string through the same normalization already used
//! elsewhere, while a raw, unnormalized `agent_id` would silently fragment
//! one runtime's observations across three keys. Where a future
//! installation *can* express two distinct logins sharing one runtime,
//! this key errs on the safe side: it over-parks (both logins share one
//! window) rather than under-parks — R37 is about refusing to start work,
//! so an excess refusal costs throughput, not correctness. Introducing a
//! fourth identity concept (beyond role/`ProfileKey`, runtime/`agent_id`,
//! and the registry's canonical id) with no real producer to fill it is
//! the same mistake this module exists to avoid one layer up; see A2 in
//! ADR-0016 for the follow-up that would actually distinguish separate
//! logins on one runtime.
//!
//! A provider retry delay Surge never actually saw is not an observation:
//! `surge-acp`'s pool sleeps on a policy default (60s) when a rate-limited
//! response carries no `Retry-After`, because it has to sleep on *some*
//! number — but that default belongs to the backoff policy alone and never
//! reaches [`CapacityWindow::resets_at`], which stays `None` in that case
//! (see [`CapacityWindow::observed_429`]). A run can legitimately sleep 60s
//! by policy while the very same runtime's capacity window still reports
//! `resets_at: None` — that is not a contradiction to reconcile, it is two
//! different questions (how long to wait vs. what Surge actually knows)
//! answered honestly.

use std::str::FromStr;
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

impl CapacitySource {
    /// Stable string form for durable storage (`surge-persistence`'s
    /// `runtime_capacity.source` column, Task 12 M2) — the same role
    /// [`crate::run_status::RunStatus::as_str`] plays for the registry's
    /// `runs.status` column. Deliberately independent of
    /// `#[derive(Serialize)]`'s own tag spelling: a persisted column format
    /// is a decision this type owns explicitly, not an accident of derive
    /// internals a future refactor of the derive attributes could silently
    /// change out from under an already-written database.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcpUsage => "acp_usage",
            Self::Observed429 => "observed_429",
        }
    }
}

/// [`CapacitySource::from_str`] was given an unrecognized label.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown CapacitySource: {0:?}")]
pub struct ParseCapacitySourceError(pub String);

impl FromStr for CapacitySource {
    type Err = ParseCapacitySourceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "acp_usage" => Self::AcpUsage,
            "observed_429" => Self::Observed429,
            other => return Err(ParseCapacitySourceError(other.to_string())),
        })
    }
}

/// A remaining-capacity share, validated to `[0.0, 1.0]` so a caller never
/// has to guard against a nonsensical fraction.
///
/// `#[serde(try_from = "f64")]` routes deserialization through
/// [`Self::new`] (via the [`TryFrom<f64>`](#impl-TryFrom<f64>-for-RemainingShare)
/// impl below) instead of the derived field-for-field decode a plain
/// `#[derive(Deserialize)]` would have produced — the latter bypassed
/// `new`'s range check entirely (the check lived only in the constructor, not
/// the type), so a NaN or out-of-range `f64` read straight off the wire
/// became a `RemainingShare` no caller ever validated. `NaN` in particular is
/// dangerous here precisely because it is silent: `NaN <= 0.0` is `false`,
/// so [`CapacityWindow::is_exhausted`] would have read a NaN remaining share
/// as "capacity available" and dispatched into it. Validation now lives on
/// the type, not on each caller remembering to call `new` — "one fact, one
/// place". A persistence reader (`surge-persistence`, M2) reading a
/// `remaining REAL` column must go through [`Self::new`] the same way: a
/// value that fails validation there means "the stored fraction is
/// unreadable", which should surface as [`CapacityStatus::Unclassified`],
/// never [`CapacityStatus::NeverObserved`] — those are different facts (see
/// that type's doc) and a decode failure is evidence of the former, not the
/// absence implied by the latter.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64")]
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

impl TryFrom<f64> for RemainingShare {
    type Error = InvalidRemainingShare;

    /// Backs `#[serde(try_from = "f64")]` on this type — see the type doc
    /// for why deserialization must not bypass [`Self::new`]'s range check.
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
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
///
/// Slices the **lowercased** copy, never the original `text`: `text` and
/// `text.to_lowercase()` can disagree in UTF-8 byte length (e.g. `İ` U+0130
/// lowercases to a 3-byte `i̇` from a 2-byte original), so a byte offset
/// found in one is not guaranteed to land on a char boundary — or even mean
/// the same position — in the other. `text` reaches this function from an
/// agent runtime the operator does not control, so it must never panic on
/// arbitrary Unicode in it.
///
/// Stays `pub`: `surge_acp::bridge::worker`'s `classify_prompt_dispatch_error`
/// is a second, cross-crate caller of this exact function (Task 12 M0) — not
/// only [`parse_reset_hint`] below — so this signature and contract are
/// frozen, not folded away.
///
/// **Intentionally separate from [`parse_reset_hint`] (this module) and
/// `surge_acp::pool::parse_retry_after` (a private, differently-scoped
/// function in that crate): observation vs. policy. Do not unify these
/// three — see ADR-0016.** This function and [`parse_reset_hint`] answer
/// "what reset time did the provider actually say" (an observation Surge
/// records verbatim or not at all); `pool::parse_retry_after` answers "how
/// long should this pool sleep before its next retry" (a backoff *policy*
/// that is allowed — indeed has — to fall back to an invented default when
/// the provider said nothing, because a retry loop has to sleep on *some*
/// number). A future "let's merge the parsers, they look similar" refactor
/// would quietly let that invented policy default leak into an
/// observation, which is exactly the fabrication this module exists to
/// refuse. A regression test cannot catch its own premise disappearing;
/// this comment is the guard a test can't be.
#[must_use]
pub fn parse_retry_after_secs(text: &str) -> Option<u64> {
    let lower = text.to_lowercase();
    let (marker_pos, marker_len) = if let Some(pos) = lower.find("retry-after") {
        (pos, "retry-after".len())
    } else {
        let pos = lower.find("retry after")?;
        (pos, "retry after".len())
    };
    let after = &lower[marker_pos + marker_len..];
    let after = after.trim_start();
    let after = after.strip_prefix(':').unwrap_or(after).trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u64>().ok()
}

/// Best-effort provider reset-*instant* parse from a free-text error/response
/// message, widening [`parse_retry_after_secs`] with three additional,
/// narrowly-anchored provider shapes (Task 12 M1) — added because the M0
/// measurement (`docs/adr/0016-capacity-parking-and-wake.md`) found four of
/// nine reconstructed real provider error bodies carrying a reset hint
/// `parse_retry_after_secs` could not read at all.
///
/// **Intentionally separate from `surge_acp::pool::parse_retry_after`: see
/// [`parse_retry_after_secs`]'s doc — that boundary (observation vs. policy)
/// applies here identically. Do not unify these; see ADR-0016.**
///
/// Tries, in order, and returns the first that recognizes something:
/// 1. [`parse_retry_after_secs`] (the literal `"retry-after"`/`"retry after"`
///    forms, including Surge's own rendered [`crate::error::SurgeError::RateLimit`]
///    prose) — not duplicated here, called directly.
/// 2. OpenAI's rate-limit prose, anchored on the literal phrase
///    `"try again in"` followed immediately by a number and a trailing `s`
///    (e.g. `"Please try again in 20s."`).
/// 3. Google's `google.rpc.RetryInfo.retryDelay` field (a protobuf
///    `Duration`, always serialized with a trailing `s`), anchored on the
///    literal key `"retrydelay"` (matched case-insensitively against
///    `lower`, so both the exact-case JSON key and any differently-cased
///    rendering of it match); only the integer-seconds part before an
///    optional fractional remainder is kept (sub-second precision does not
///    change which `Decision` a caller reaches).
/// 4. An epoch-suffixed exhaustion message, anchored on the literal phrase
///    `"usage limit reached|"` immediately followed by a decimal Unix
///    timestamp — accepted **only** when that timestamp is strictly after
///    `observed_at` and within [`MAX_RESET_HORIZON`] of it. Anthropic's
///    Claude Code CLI is the observed source of this exact shape.
///
/// Deliberately does **not** copy `surge_acp::pool::parse_retry_after`'s
/// numeric fallback (grab any plausible-looking number near the word
/// "retry" even with no recognized anchor phrase) — that fallback is
/// exactly the fabricated-signal shape this module exists to refuse. A
/// number with no anchor stays `None`, however plausible it looks; pinned
/// by this module's own test suite (search for `unanchored`).
#[must_use]
pub fn parse_reset_hint(text: &str, observed_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if let Some(secs) = parse_retry_after_secs(text) {
        return checked_seconds_from(observed_at, secs);
    }
    let lower = text.to_lowercase();
    if let Some(secs) = parse_openai_try_again_in(&lower) {
        return checked_seconds_from(observed_at, secs);
    }
    if let Some(secs) = parse_google_retry_delay(&lower) {
        return checked_seconds_from(observed_at, secs);
    }
    parse_usage_limit_reached_epoch(&lower, observed_at)
}

/// Add `secs` seconds to `observed_at` without ever panicking — `secs`
/// reaches this from free-text an agent runtime or a provider produced,
/// which the operator does not control, so an absurd value (a misparsed
/// digit run, an adversarial response) must degrade to `None`, never a
/// crash. Both failure modes are real and were reachable before this
/// helper existed: `chrono::Duration::seconds` panics if `secs` exceeds its
/// internal representable range (tighter than `i64::MAX` seconds), and
/// `DateTime<Utc>`'s `Add` panics if the addition overflows the
/// representable calendar range. `TimeDelta::try_seconds` and
/// `checked_add_signed` are the fallible form of each.
fn checked_seconds_from(observed_at: DateTime<Utc>, secs: u64) -> Option<DateTime<Utc>> {
    let secs = i64::try_from(secs).ok()?;
    let delta = chrono::Duration::try_seconds(secs)?;
    observed_at.checked_add_signed(delta)
}

/// Upper bound a [`parse_reset_hint`] epoch-form result must fall within
/// (relative to `observed_at`) to be trusted as a genuine future reset
/// rather than, say, a misparsed millisecond timestamp read as seconds (which
/// would land implausibly far in the future). Provider rate-limit windows
/// are observed in minutes-to-a-day; 30 days is generous headroom above
/// that while still rejecting an obviously-wrong order of magnitude.
const MAX_RESET_HORIZON: chrono::Duration = chrono::Duration::days(30);

/// OpenAI prose form: `"...try again in 20s..."`. Anchored on the literal
/// phrase `"try again in"`; the number must be immediately followed by `s`
/// with no intervening characters, so `"try again in 20 seconds"` and
/// `"try again in 20 minutes"` are deliberately left unrecognized rather
/// than guessed at (widen with its own anchor and test if that shape shows
/// up in practice — see the module's classifier-widening precedent).
fn parse_openai_try_again_in(lower: &str) -> Option<u64> {
    let pos = lower.find("try again in")?;
    let after = lower[pos + "try again in".len()..].trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &after[digits.len()..];
    if !rest.starts_with('s') {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Google `google.rpc.RetryInfo.retryDelay` form: a protobuf `Duration`
/// string such as `"retryDelay":"30s"` or `"retryDelay": "0.500000s"`.
/// Anchored on the literal key `"retrydelay"`; only the whole-seconds
/// portion before an optional `.` fractional remainder is kept, and the
/// value must still end in `s` (a protobuf `Duration`'s wire form always
/// does) to confirm this is the shape expected rather than an unrelated
/// number that happens to follow the key.
fn parse_google_retry_delay(lower: &str) -> Option<u64> {
    let pos = lower.find("retrydelay")?;
    let after = lower[pos + "retrydelay".len()..].trim_start_matches([':', '"', ' ']);
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &after[digits.len()..];
    let rest = rest.strip_prefix('.').map_or(rest, |after_dot| {
        after_dot
            .char_indices()
            .find(|(_, c)| !c.is_ascii_digit())
            .map_or("", |(idx, _)| &after_dot[idx..])
    });
    if !rest.starts_with('s') {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Anthropic Claude Code CLI epoch-suffix form: `"usage limit reached|<epoch
/// seconds>"`. Anchored on the exact literal `"usage limit reached|"`
/// (already substring-matched by [`RATE_LIMIT_PATTERNS`]'s `"usage limit
/// reached"` entry, without the `|`); accepted only when the epoch parses,
/// is strictly after `observed_at`, and is within [`MAX_RESET_HORIZON`] of
/// it — a timestamp in the past or implausibly far in the future is
/// evidence of a misparse, not a signal to trust.
fn parse_usage_limit_reached_epoch(
    lower: &str,
    observed_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let pos = lower.find("usage limit reached|")?;
    let after = &lower[pos + "usage limit reached|".len()..];
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    let epoch_secs: i64 = digits.parse().ok()?;
    let reset_at = DateTime::from_timestamp(epoch_secs, 0)?;
    if reset_at <= observed_at || reset_at - observed_at > MAX_RESET_HORIZON {
        return None;
    }
    Some(reset_at)
}

/// A learned rate-limit window for one agent runtime (canonical
/// `surge_acp::Registry` id — see the module doc's "Why the key is the
/// runtime" section).
///
/// `window`, `remaining`, and `resets_at` are `Option` because they are
/// facts Surge learns by observation, never hardcodes: absent means "not
/// yet observed", not "assume some default". A caller that only knows
/// `remaining` (e.g. right after a 429) and not `window` must not guess the
/// window — leave it `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapacityWindow {
    runtime: String,
    window: Option<Duration>,
    remaining: Option<RemainingShare>,
    resets_at: Option<DateTime<Utc>>,
    source: CapacitySource,
}

impl CapacityWindow {
    /// Canonical agent-runtime registry id this window belongs to (see the
    /// module doc's "Why the key is the runtime" section) — not a
    /// distinct-login identifier.
    #[must_use]
    pub fn runtime(&self) -> &str {
        &self.runtime
    }

    /// Length of the provider's rate-limit window, if ever learned from
    /// observation. `None` on a fresh runtime — never a fabricated default.
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

    /// Build a window from an observed HTTP 429: the runtime's remaining
    /// share is exhausted *right now*, by definition. `retry_after` (when
    /// the provider actually sent one) fixes `resets_at`; a missing
    /// `Retry-After` leaves `resets_at` unknown rather than assuming a
    /// fallback delay.
    ///
    /// A single 429 does not reveal the window's *length* — only that the
    /// runtime is exhausted and, maybe, when it resets. A caller that has
    /// tracked an earlier reset for this runtime can widen the result with
    /// [`Self::with_learned_window`].
    #[must_use]
    pub fn observed_429(
        runtime: impl Into<String>,
        retry_after: Option<Duration>,
        observed_at: DateTime<Utc>,
    ) -> Self {
        // Never panics on an absurd `retry_after` (agent/provider text feeds
        // this indirectly via `from_observed_error`, and a caller can pass
        // any `Duration` directly) — `checked_seconds_from` returns `None`
        // rather than overflowing `chrono::Duration`'s internal range or
        // `DateTime<Utc>`'s representable range.
        let resets_at = retry_after.and_then(|d| checked_seconds_from(observed_at, d.as_secs()));
        Self {
            runtime: runtime.into(),
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
    ///
    /// Resolves `resets_at` through [`parse_reset_hint`] directly (not
    /// through [`Self::observed_429`]'s `Duration`-shaped parameter, and not
    /// through [`parse_retry_after_secs`] alone) — this is the one live
    /// caller on the classification path, so this is where the three
    /// widened M1 provider shapes (OpenAI, Google, Anthropic's epoch
    /// suffix) actually start reaching a real `CapacityWindow` instead of
    /// existing only as `parse_reset_hint`'s own direct unit tests.
    #[must_use]
    pub fn from_observed_error(
        runtime: impl Into<String>,
        error: &str,
        observed_at: DateTime<Utc>,
    ) -> Option<Self> {
        if !looks_like_rate_limit(error) {
            return None;
        }
        Some(Self {
            runtime: runtime.into(),
            window: None,
            remaining: Some(RemainingShare::EXHAUSTED),
            resets_at: parse_reset_hint(error, observed_at),
            source: CapacitySource::Observed429,
        })
    }

    /// Learn the window length from the elapsed time between two
    /// consecutive observed resets of the same runtime: the only honest
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

    /// Reassemble a window from parts a caller already has individually —
    /// used by a persistence reader (`surge-persistence`, Task 12 M2)
    /// rebuilding a [`CapacityWindow`] from a durable `runtime_capacity` row
    /// (one column per field here).
    ///
    /// Unlike [`Self::observed_429`] and [`Self::from_observed_error`],
    /// which each *derive* some fields from a single observation, this
    /// constructor takes every field already computed — it is for the one
    /// caller that has all of them separately (a SQLite row) and only needs
    /// them assembled, not reduced from raw text.
    ///
    /// Does not itself validate `remaining`: [`RemainingShare`] is
    /// constructible only through [`RemainingShare::new`], so any value
    /// passed here already went through that range check at the caller —
    /// see that type's doc for why a persistence reader must run a raw
    /// `remaining REAL` column value through it before it can even be
    /// passed here.
    #[must_use]
    pub fn from_parts(
        runtime: impl Into<String>,
        window: Option<Duration>,
        remaining: Option<RemainingShare>,
        resets_at: Option<DateTime<Utc>>,
        source: CapacitySource,
    ) -> Self {
        Self {
            runtime: runtime.into(),
            window,
            remaining,
            resets_at,
            source,
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
    /// runtime is still exhausted — it just means Surge has no fresher
    /// signal).
    #[must_use]
    pub fn seconds_until_reset(&self, now: DateTime<Utc>) -> Option<i64> {
        self.resets_at
            .map(|reset_at| (reset_at - now).num_seconds())
            .filter(|secs| *secs > 0)
    }
}

/// What Surge has to say about a runtime's rate-limit capacity, including
/// the difference between two kinds of "no window" that must never look the
/// same to an operator:
///
/// - a fresh runtime nobody has ever heard a failure from
///   ([`Self::NeverObserved`]) — the common, expected case (R35.1);
/// - a runtime that *has* failed, but whose failure(s) matched none of
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
    /// No failure has ever been observed for this runtime, within whatever
    /// this status's producer's observation is scoped to: one process's
    /// in-memory tracker since it started (`surge-acp::health`), or one
    /// run's persisted event log (`surge inbox`'s scan).
    NeverObserved,
    /// A capacity window was learned from an observed 429.
    Known(CapacityWindow),
    /// At least one failure was observed for this runtime, but none of them
    /// matched [`RATE_LIMIT_PATTERNS`] — Surge cannot say whether the
    /// runtime is rate-limited or failing for an unrelated reason.
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

/// How long a node's dispatch is expected to take, learned from history
/// (e.g. `stage_executions` for nodes of the same archetype — see
/// `surge_orchestrator`'s `WorkEstimator` seam, M3) rather than guessed.
///
/// Deliberately does **not** implement [`Default`], even though clippy may
/// suggest one: a `Default` would hand every caller a `WorkEstimate` that
/// *looks* like a real zero-duration estimate, indistinguishable from
/// "history says this node takes no time at all". The only honest spelling
/// of "no estimate available" is `Option<&WorkEstimate>` being `None` —
/// [`CapacityPolicy::decide`]'s rule 4 depends on that distinction never
/// being collapsible by an absent-minded `.unwrap_or_default()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkEstimate {
    /// Expected wall-clock duration for one dispatch of the node this
    /// estimate is for.
    duration: Duration,
}

impl WorkEstimate {
    /// Build an estimate from a learned duration.
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }

    /// Expected wall-clock duration for one dispatch.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.duration
    }
}

/// R41 seam (rotate to another configured agent runtime instead of parking
/// on exhaustion) — the shape [`CapacityPolicy::decide`] is prepared to
/// consult once a verified rotation target exists.
///
/// **Not wired live in this delivery.** Verifying that a candidate profile
/// really targets a *different* [`crate::runtime::RuntimeKind`] needs
/// `surge_acp::Registry`, which this crate does not depend on (`surge-core`
/// stays I/O- and registry-free) — that verification is
/// `surge-orchestrator`'s job, not yet scheduled as a numbered task. Until
/// it lands, [`CapacityPolicy::decide`] never emits [`Decision::Rotate`]
/// regardless of this field's value: today, every profile pointed at one
/// agent runtime resolves to the same launch command and the same login,
/// so "rotate to the next profile of the same runtime" would not change
/// anything — it would silently repeat rules 2/3's already-exhausted
/// dispatch with extra steps, which is worse than refusing outright. See
/// ADR-0016 for the full argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotationPolicy {
    /// No rotation candidate configured, or rotation disabled.
    Disabled,
    /// A candidate profile believed to target a different agent runtime.
    /// Carried for the seam's shape only in this delivery — see the type
    /// doc: `decide` never acts on this variant yet.
    Enabled {
        /// The profile `decide` would rotate to, once rotation is live.
        to: crate::keys::ProfileKey,
    },
}

/// Why a [`Decision::Dispatch`] is going ahead despite something less than
/// a clean, fully-informed green light — logged by the caller so an
/// operator sees *which* condition applied, not just that dispatch
/// happened.
///
/// `#[non_exhaustive]`: this enum's only consumer is a log line
/// (`warn!(reason = ?degraded, ...)` at the call site) — an unhandled
/// future variant falling into a `_` arm there loses nothing but a more
/// specific message, so the forced wildcard costs an external matcher
/// nothing. Contrast [`Decision`] and [`WakeBasis`] below, where the same
/// marker would be actively harmful (see their docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Degraded {
    /// The runtime is exhausted and no reset time has ever been learned for
    /// it at all — [`CapacityWindow::resets_at`] is `None`.
    ExhaustedNoResetTime,
    /// The runtime is exhausted and a reset time *was* learned once, but it
    /// has already passed ([`CapacityWindow::seconds_until_reset`] returned
    /// `None` even though [`CapacityWindow::resets_at`] is `Some`) — a stale
    /// signal, not evidence the runtime is still exhausted for the reason
    /// that signal named.
    ExhaustedResetElapsed,
    /// A [`WorkEstimate`] was supplied, but `decide` had no basis to compare
    /// it against: [`CapacityWindow::window`] (a learned duration) or
    /// [`CapacityWindow::remaining`] (or both) is absent. This is what
    /// every [`CapacityWindow`] this crate can construct today looks like
    /// (see [`CapacityPolicy::decide`]'s doc) — there is nothing to weigh
    /// the estimate against, not "weighed it and it came up short" (that is
    /// [`Self::EstimateExceedsRemaining`]).
    EstimateNotComparable,
    /// A [`WorkEstimate`] was supplied and **was** compared against the
    /// runtime's remaining capacity — the comparison itself succeeded, and
    /// the estimate exceeds what remains. Kept distinct from
    /// [`Self::EstimateNotComparable`] because the two are actionable
    /// differently: this one is a real, sized signal an operator can act on
    /// (the node's declared work does not fit the window left), while
    /// [`Self::EstimateNotComparable`] just means there was no data to
    /// judge by.
    EstimateExceedsRemaining,
}

/// How a [`Decision::Park`]'s `wake_at` was derived — logged and folded
/// into the run's event log ([`crate::run_event::EventPayload::RunParked`])
/// so a consecutive-blind-park counter (rule 4's escalation cap, tracked by
/// the caller, not by this pure function) can tell "we know when the
/// provider resets" apart from "we are guessing on a policy timer" by
/// replaying the log, not by re-deriving it from `reason` text.
///
/// **No `#[non_exhaustive]`, unlike [`Degraded`]**: the whole reason this
/// type exists is so that counter can `match` on it exhaustively. A `_`
/// arm here would silently absorb a future third basis into whichever
/// bucket the wildcard was written against, breaking the counter instead
/// of forcing it to be taught the new case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeBasis {
    /// `wake_at` came from an actually-observed provider reset time
    /// ([`CapacityWindow::resets_at`]).
    ObservedReset,
    /// `wake_at` came from the configured blind-backoff policy
    /// (`CapacityPolicy::blind_backoff`) because no usable reset time was
    /// available — a guess, not an observation; see the module doc's
    /// closing paragraph on why this does not corrupt `resets_at`.
    PolicyBackoff,
}

/// What [`CapacityPolicy::decide`] concluded a caller should do.
///
/// `#[must_use]`: a silently-dropped `Decision` means the run dispatches by
/// default — exactly the "warn and dispatch anyway" behavior R37 exists to
/// replace — so discarding one without acting on it is always a bug.
///
/// **No `#[non_exhaustive]`, unlike an error taxonomy such as
/// `surge_orchestrator::engine::stage::StageError` or
/// `surge_acp::bridge::error::SendMessageError` (neither defined in this
/// crate)**: each variant here demands categorically different caller
/// behavior (dispatch now / park until later / rotate to another runtime),
/// so a future variant (e.g. `Decision::Defer`) must not be able to
/// silently fall into whatever an existing `_` arm already does — that
/// would run the *wrong* one of these three behaviors under the new
/// variant's name. The forced compile error on a new variant is the
/// intended cost here, the opposite trade-off from an error taxonomy where
/// callers are expected to already have a catch-all.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead and dispatch the node now.
    Dispatch {
        /// `None` for a clean, fully-informed green light; `Some(_)` names
        /// the specific reason dispatch is going ahead despite something
        /// less than that — log it, don't silently drop it.
        degraded: Option<Degraded>,
    },
    /// Do not dispatch. Park the run until `wake_at` instead of stalling it
    /// indefinitely or silently retrying.
    Park {
        /// When to wake the run and re-evaluate. Never `Option`: a `Park`
        /// with no concrete wake time is unconstructible by this type —
        /// see the module doc and R37's "instead of stalling".
        wake_at: DateTime<Utc>,
        /// Why `wake_at` is what it is.
        basis: WakeBasis,
    },
    /// Rotate to a different, verified-different-runtime profile instead of
    /// parking. **Never emitted by [`CapacityPolicy::decide`] in this
    /// delivery** — see [`RotationPolicy`]'s doc; kept as a variant so that
    /// whatever future work does wire live rotation is an additive change,
    /// not a breaking one.
    Rotate {
        /// The profile to rotate to.
        to: crate::keys::ProfileKey,
    },
}

/// Pure, I/O-free capacity-aware dispatch policy (R37/R37.1/R38/R38.1).
///
/// `decide` never reads a clock, a config file, or a registry itself — `now`
/// and every configured knob are threaded in by the caller
/// (`surge-orchestrator`, M3) so this stays trivially unit-testable and
/// replay-deterministic; the same seam `surge-persistence`'s
/// `runs::clock::Clock` trait and `RecoveryOptions.now_ms` already use for
/// the identical reason (see ADR-0016) — `now` is a parameter, never read
/// from `Utc::now()` inline.
///
/// # Rule order (load-bearing, pinned by this module's tests)
///
/// 1. The runtime is exhausted ([`CapacityWindow::is_exhausted`]) **and** a
///    usable (still-future) reset time is known
///    ([`CapacityWindow::seconds_until_reset`] returns `Some`) →
///    [`Decision::Park`] with [`WakeBasis::ObservedReset`] and
///    `wake_at == `[`CapacityWindow::resets_at`]`().unwrap()`.
/// 2. Exhausted, no *usable* reset time (none was ever learned, or the one
///    that was has already elapsed), and [`Self::blind_backoff`] is
///    configured → [`Decision::Park`] with [`WakeBasis::PolicyBackoff`] and
///    `wake_at == now + blind_backoff`.
/// 3. Exhausted, no usable reset time, and the operator has removed
///    `blind_backoff` from `surge.toml` → [`Decision::Dispatch`], flagged
///    [`Degraded::ExhaustedNoResetTime`] or [`Degraded::ExhaustedResetElapsed`]
///    depending on which "no usable time" case applies. This is rule 2's
///    predecessor surviving as an explicit, human-chosen opt-out of the
///    blind-backoff mechanism, not a silent default.
/// 4. Not exhausted, and `estimate` is `None` → [`Decision::Dispatch`] with
///    `degraded: None`, unconditionally. **This is the one negative
///    guarantee this module makes measurable**: the absence of a
///    [`WorkEstimate`] must never, by itself, cause a refusal — see this
///    module's own red-before-green test for it.
/// 5. Otherwise (not exhausted, `estimate` is `Some`) — attempt the
///    comparison against the runtime's remaining capacity. **Structurally
///    unreachable with any `CapacityWindow` this crate's own constructors
///    can produce today**: [`CapacityWindow::remaining`] is only ever
///    `None` or exactly [`RemainingShare::EXHAUSTED`] (the sole live
///    constructor is [`CapacityWindow::observed_429`], and an exhausted
///    window is already handled by rules 1–3 above), so a genuine fraction
///    strictly between the two never occurs in production, and
///    [`CapacityWindow::window`] is populated only by the rare
///    [`CapacityWindow::with_learned_window`] path. This module still
///    implements the real comparison (not a stub) so the arm is
///    *pluggable*, not imitated — see §2 of the M1 plan and ADR-0016: the
///    deliverable this milestone owes here is the mechanism, not a fake
///    median-cost table nothing in this crate could source. Exercised in
///    this module's own tests via direct (private-field) construction of a
///    comparable window, which no production caller can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityPolicy {
    /// Blind backoff duration used by rules 2/3 when no reset time observed
    /// by the provider is usable. `None` means the operator has opted out
    /// of blind parking (`surge.toml`'s `[capacity].blind_backoff` removed)
    /// — `decide` then dispatches anyway, flagged
    /// [`Degraded::ExhaustedNoResetTime`]/[`Degraded::ExhaustedResetElapsed`],
    /// rather than inventing a duration.
    pub blind_backoff: Option<Duration>,
    /// R41 seam. See [`RotationPolicy`]'s doc: `decide` never emits
    /// [`Decision::Rotate`] from this field in this delivery.
    pub rotation: RotationPolicy,
}

impl CapacityPolicy {
    /// See the type doc for the full, load-bearing rule order.
    ///
    /// No `#[must_use]` on this method itself — `clippy::double_must_use`
    /// flags that as redundant given [`Decision`] already carries the
    /// marker on its own declaration, which is what actually matters: the
    /// marker fires at every call site regardless of which method returned
    /// the value.
    pub fn decide(
        &self,
        estimate: Option<&WorkEstimate>,
        status: &CapacityStatus,
        now: DateTime<Utc>,
    ) -> Decision {
        if let Some(window) = status.window()
            && window.is_exhausted()
        {
            // Rule 1: a usable (still-future) reset time is known. The
            // check for "still usable" lives in `seconds_until_reset`, not
            // `resets_at().is_some()` — a stale, past `resets_at` is not
            // evidence of anything (see that method's own doc); checking
            // `is_some()` alone would have parked on a `wake_at` already in
            // the past.
            if let Some(wake_at) = window.resets_at()
                && window.seconds_until_reset(now).is_some()
            {
                return Decision::Park {
                    wake_at,
                    basis: WakeBasis::ObservedReset,
                };
            }

            // Rules 2/3: no *usable* reset time — either none was ever
            // learned, or the one that was has gone stale. `blind_backoff`
            // alone decides Park vs. Dispatch; the elapsed-vs-never
            // distinction only matters to the `None` (Dispatch) arm, so it
            // is computed there, not hoisted above the match — hoisting it
            // previously left a value computed and then silently discarded
            // on the `Some(backoff)` arm, which is exactly the shape that
            // let a broken elapsed/never check hide behind a passing test
            // suite (a mutation to that check has no observable effect on
            // a branch that never reads it).
            return match self.blind_backoff {
                Some(backoff) => Decision::Park {
                    // Never panics on an operator-configured `backoff`:
                    // clamp to the latest representable instant rather than
                    // overflow `DateTime<Utc>`'s range. `CapacityConfig::
                    // validate` rejects an unrepresentable `blind_backoff`
                    // at config-load time (a clear config error beats a
                    // runtime panic), so this clamp is defense in depth for
                    // a value that reaches `decide` some other way, not the
                    // expected path.
                    wake_at: checked_seconds_from(now, backoff.as_secs())
                        .unwrap_or(DateTime::<Utc>::MAX_UTC),
                    basis: WakeBasis::PolicyBackoff,
                },
                None => Decision::Dispatch {
                    degraded: Some(if window.resets_at().is_some() {
                        Degraded::ExhaustedResetElapsed
                    } else {
                        Degraded::ExhaustedNoResetTime
                    }),
                },
            };
        }

        // Rule 4: no estimate — never a refusal, on any non-exhausted
        // status (`NeverObserved`, `Unclassified`, or a `Known` window that
        // is not exhausted).
        let Some(estimate) = estimate else {
            return Decision::Dispatch { degraded: None };
        };

        // Rule 5: otherwise, attempt the comparison (see the type doc for
        // why this is unreachable with today's producers).
        match status.window().and_then(|w| compare_estimate(estimate, w)) {
            Some(true) => Decision::Dispatch { degraded: None },
            Some(false) => Decision::Dispatch {
                degraded: Some(Degraded::EstimateExceedsRemaining),
            },
            None => Decision::Dispatch {
                degraded: Some(Degraded::EstimateNotComparable),
            },
        }
    }
}

/// Whether `estimate` can be judged against `window`'s remaining capacity,
/// and if so, whether it fits within what remains. `None` when the data
/// needed for a genuine comparison is not available (see
/// [`Degraded::EstimateNotComparable`]'s doc) — a missing
/// [`CapacityWindow::window`] duration, or an absent
/// [`CapacityWindow::remaining`]. A **present** `remaining`, including
/// exactly [`RemainingShare::FULL`], **is** comparable: a full window with a
/// known length is the *most* comparable case there is (trivially, any
/// estimate shorter than the whole window fits) — only the complete
/// absence of one of the two inputs makes a comparison impossible. (By the
/// time `decide` reaches this, [`RemainingShare::EXHAUSTED`] has already
/// been handled by rules 1–3, so a non-`None` `remaining` here is never
/// exactly zero either.)
fn compare_estimate(estimate: &WorkEstimate, window: &CapacityWindow) -> Option<bool> {
    let remaining = window.remaining()?;
    let full_window = window.window()?;
    let remaining_time = full_window.mul_f64(remaining.get());
    Some(estimate.duration() <= remaining_time)
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
    fn from_observed_error_resolves_reset_time_through_parse_reset_hint_not_just_retry_after_secs()
    {
        // Review finding #1: `from_observed_error` must route through
        // `parse_reset_hint`, not only `parse_retry_after_secs` — otherwise
        // the M1 parser widening (OpenAI/Google/epoch) has no live caller
        // at all, reachable only from `parse_reset_hint`'s own direct unit
        // tests. Google's `retryDelay` is the same shape ADR-0016 claims
        // this milestone moved from "unrecognized" to "recognized" on the
        // live classification path.
        let observed_at = Utc::now();
        let google_body = r#"{"error":{"code":429,"message":"Resource has been exhausted","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"30s"}]}}"#;
        let window = CapacityWindow::from_observed_error("gemini", google_body, observed_at)
            .expect("RESOURCE_EXHAUSTED must still classify as a rate limit");
        assert_eq!(
            window.resets_at(),
            Some(observed_at + chrono::Duration::seconds(30)),
            "from_observed_error must resolve the Google retryDelay reset hint, not just the \
             literal Retry-After forms"
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
        assert_eq!(window.runtime(), "claude-work");
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
    fn capacity_source_string_form_round_trips() {
        for source in [CapacitySource::AcpUsage, CapacitySource::Observed429] {
            assert_eq!(source.as_str().parse::<CapacitySource>().unwrap(), source);
        }
    }

    #[test]
    fn capacity_source_unknown_string_is_error() {
        assert!("nonsense".parse::<CapacitySource>().is_err());
    }

    #[test]
    fn from_parts_assembles_every_field() {
        let resets_at = t("2026-01-01T00:00:00Z");
        let remaining = RemainingShare::new(0.25).unwrap();
        let window = CapacityWindow::from_parts(
            "claude-acp",
            Some(Duration::from_secs(3600)),
            Some(remaining),
            Some(resets_at),
            CapacitySource::Observed429,
        );
        assert_eq!(window.runtime(), "claude-acp");
        assert_eq!(window.window(), Some(Duration::from_secs(3600)));
        assert_eq!(window.remaining(), Some(remaining));
        assert_eq!(window.resets_at(), Some(resets_at));
        assert_eq!(window.source(), CapacitySource::Observed429);
    }

    #[test]
    fn from_parts_accepts_every_field_absent() {
        // A fresh runtime nobody has failed against yet: every learned
        // field stays `None`, same as any other constructor on this type.
        let window =
            CapacityWindow::from_parts("claude-acp", None, None, None, CapacitySource::AcpUsage);
        assert_eq!(window.window(), None);
        assert_eq!(window.remaining(), None);
        assert_eq!(window.resets_at(), None);
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

    /// Regression: this function is reached from `surge-acp`'s bridge worker
    /// on **every** rejected prompt whose text matches `looks_like_rate_limit`
    /// — text that originates at an agent runtime the operator does not
    /// control, so it must never panic on it, no matter what Unicode shows
    /// up. It used to: the marker position/length are computed against
    /// `text.to_lowercase()`, but the slice was taken out of the *original*
    /// `text` — for any character whose lowercasing changes UTF-8 byte
    /// length (e.g. `İ` U+0130 → `i̇`, 2 bytes → 3), the two strings'
    /// coordinate frames diverge and the borrowed index can land inside a
    /// multi-byte character of `text`, which panics `str` indexing instead
    /// of returning `None`. Fixed by slicing `lower` (self-consistent by
    /// construction) instead of `text`.
    ///
    /// Split into two independent `#[test]`s (M0 review finding): under the
    /// pre-fix code, this first assertion doesn't just fail, it **panics**
    /// (an out-of-bounds `str` index) — so in a single combined test, the
    /// panic aborts the test function before a second assertion below it
    /// ever runs. A second assertion in that same function is not an
    /// independent regression guard: it is unreachable on red (the panic
    /// fires first) and was reported to pass even under the buggy code on
    /// green (the off-by-one byte offset happened to land on an ASCII space
    /// for that particular input), so it proved nothing either way. Two
    /// separate `#[test]`s give each input its own pass/fail signal.
    #[test]
    fn parse_retry_after_secs_does_not_panic_on_multi_byte_case_folding() {
        // A length-changing character sits between the standalone-429
        // marker and "Retry-After", so byte offsets computed against the
        // lowercased copy no longer line up with the original string; the
        // emoji right after the marker is 4 bytes wide, so an off-by-one
        // index has nowhere valid to land except inside it.
        let text = "429 Too Many Requests İRetry-After😀30";
        // Must not panic. The emoji sits between the marker and the digits,
        // so no digits are recoverable here — the point is survival, not a
        // specific value.
        assert_eq!(parse_retry_after_secs(text), None);
    }

    #[test]
    fn parse_retry_after_secs_still_extracts_digits_past_a_multi_byte_case_fold() {
        // A length-changing character earlier in the string, with an
        // otherwise ordinary header afterward, must still extract correctly
        // — proving the fix does not merely avoid panicking but keeps
        // parsing the common case right. Independent test: this must fail
        // on its own if a future change breaks parsing here, without
        // depending on the panic-survival assertion above having run first.
        let text_with_valid_digits = "429 Too Many Requests İ: Retry-After: 30";
        assert_eq!(parse_retry_after_secs(text_with_valid_digits), Some(30));
    }

    /// Regression: three overflow panics found by mutation testing, all on
    /// text/config an operator does not fully control. `parse_retry_after_secs`
    /// only extracts *digits*, with no upper bound on how many — a huge but
    /// syntactically valid number in agent/provider text used to reach
    /// `chrono::Duration::seconds` (which panics past its internal range)
    /// and `DateTime + Duration` (which panics on overflow). Fixed via
    /// `checked_seconds_from`'s `TimeDelta::try_seconds` +
    /// `checked_add_signed`, returning `None` instead of crashing.
    #[test]
    fn parse_reset_hint_does_not_panic_on_a_retry_after_value_that_overflows_the_datetime_add() {
        // This magnitude (1e14s ~= 3.17M years) is well *under*
        // `chrono::Duration`'s own internal bound (~9.2e15s) — `try_seconds`
        // alone would not catch it. The panic is one layer up: adding it to
        // `observed_at` (~year 2026) overflows `DateTime<Utc>`'s
        // representable range (~year 262143), which only
        // `checked_add_signed` catches.
        let observed_at = Utc::now();
        assert_eq!(
            parse_reset_hint("Retry-After: 99999999999999", observed_at),
            None
        );
    }

    #[test]
    fn parse_reset_hint_does_not_panic_on_a_retry_after_value_that_overflows_time_delta_itself() {
        // This magnitude (~1e17s) exceeds `chrono::Duration`'s own internal
        // bound directly — `TimeDelta::try_seconds` itself must return
        // `None` here, before any `DateTime` addition is attempted.
        let observed_at = Utc::now();
        assert_eq!(
            parse_reset_hint("Retry-After: 99999999999999999", observed_at),
            None
        );
    }

    #[test]
    fn observed_429_does_not_panic_on_an_absurdly_large_retry_after_duration() {
        // Same class, the pre-existing `observed_429` path (review finding
        // #2: fix all three call sites in one commit, not just the new
        // one — otherwise a future refactor "inherits" the half left
        // panicking as if it were already safe).
        let window =
            CapacityWindow::observed_429("claude", Some(Duration::from_secs(u64::MAX)), Utc::now());
        assert_eq!(window.resets_at(), None);
    }

    #[test]
    fn decide_does_not_panic_on_an_absurdly_large_blind_backoff() {
        let now = Utc::now();
        let window = CapacityWindow::observed_429("claude-acp", None, now);
        let status = CapacityStatus::Known(window);
        let policy = CapacityPolicy {
            blind_backoff: Some(Duration::from_secs(9_000_000_000_000)),
            rotation: RotationPolicy::Disabled,
        };
        assert_eq!(
            policy.decide(None, &status, now),
            Decision::Park {
                wake_at: DateTime::<Utc>::MAX_UTC,
                basis: WakeBasis::PolicyBackoff,
            }
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
            "Usage limit reached for this workspace"
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

    // ── `RemainingShare` deserialization (§5) ──────────────────────────

    #[test]
    fn remaining_share_new_rejects_nan() {
        // The guard already lived in `new` before this milestone's work —
        // pinned here as existing behavior, not a new fix.
        // `(0.0..=1.0).contains(&x)` is `false` for any comparison against
        // `NaN` (`PartialOrd`'s contract), so `new` already refused it; the
        // gap this milestone closes is the *derived* `Deserialize` bypassing
        // `new` entirely (see the type doc).
        assert!(RemainingShare::new(f64::NAN).is_err());
    }

    #[test]
    fn remaining_share_deserialize_rejects_out_of_range_via_try_from() {
        // `1.5` is valid JSON (unlike a bare `NaN` token, which standard
        // JSON cannot even encode — a test asserting that would be green
        // and prove nothing, since the failure would come from
        // `serde_json`'s own `f64` parse, never reaching `TryFrom`). This is
        // what actually proves the range guard survived the move to
        // `#[serde(try_from = "f64")]`.
        let result: Result<RemainingShare, _> = serde_json::from_str("1.5");
        assert!(result.is_err());
    }

    #[test]
    fn remaining_share_deserialize_accepts_valid_fraction() {
        let parsed: RemainingShare = serde_json::from_str("0.5").unwrap();
        assert_eq!(parsed, RemainingShare::new(0.5).unwrap());
    }

    // ── `parse_reset_hint` (§3, revision 4) ─────────────────────────────

    #[test]
    fn parse_reset_hint_delegates_to_parse_retry_after_secs_first() {
        let observed_at = t("2026-01-01T00:00:00Z");
        assert_eq!(
            parse_reset_hint("Retry-After: 30", observed_at),
            Some(observed_at + chrono::Duration::seconds(30))
        );
    }

    #[test]
    fn parse_reset_hint_recognizes_openai_try_again_in_prose() {
        let observed_at = t("2026-01-01T00:00:00Z");
        assert_eq!(
            parse_reset_hint(
                "Rate limit reached for gpt-4 in organization org-x. Please try again in 20s.",
                observed_at
            ),
            Some(observed_at + chrono::Duration::seconds(20))
        );
    }

    #[test]
    fn parse_reset_hint_recognizes_google_retry_delay_field() {
        let observed_at = t("2026-01-01T00:00:00Z");
        let body = r#"{"error":{"code":429,"message":"Resource has been exhausted","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"30s"}]}}"#;
        assert_eq!(
            parse_reset_hint(body, observed_at),
            Some(observed_at + chrono::Duration::seconds(30))
        );
    }

    #[test]
    fn parse_reset_hint_recognizes_google_retry_delay_with_fractional_seconds() {
        let observed_at = t("2026-01-01T00:00:00Z");
        // A protobuf `Duration`'s wire form may carry sub-second precision;
        // the whole-seconds part is what this policy layer can act on.
        let body = r#""retryDelay":"0.500000s""#;
        assert_eq!(parse_reset_hint(body, observed_at), Some(observed_at));
    }

    #[test]
    fn parse_reset_hint_recognizes_epoch_suffixed_usage_limit_reached() {
        let observed_at = t("2026-01-01T00:00:00Z");
        let reset_at = observed_at + chrono::Duration::hours(4);
        let text = format!("usage limit reached|{}", reset_at.timestamp());
        assert_eq!(parse_reset_hint(&text, observed_at), Some(reset_at));
    }

    #[test]
    fn parse_reset_hint_rejects_epoch_in_the_past() {
        let observed_at = t("2026-01-01T00:00:00Z");
        let past = observed_at - chrono::Duration::hours(1);
        let text = format!("usage limit reached|{}", past.timestamp());
        assert_eq!(parse_reset_hint(&text, observed_at), None);
    }

    #[test]
    fn parse_reset_hint_rejects_epoch_beyond_the_reset_horizon() {
        let observed_at = t("2026-01-01T00:00:00Z");
        let far_future = observed_at + MAX_RESET_HORIZON + chrono::Duration::days(1);
        let text = format!("usage limit reached|{}", far_future.timestamp());
        assert_eq!(parse_reset_hint(&text, observed_at), None);
    }

    #[test]
    fn parse_reset_hint_rejects_unanchored_numbers_near_retry_vocabulary() {
        // The exact negative `surge_acp::pool`'s own numeric fallback would
        // have caught and this module must not: "retry" appears, a number
        // appears, but no recognized anchor phrase connects them.
        let observed_at = Utc::now();
        assert_eq!(
            parse_reset_hint("rate limited, wait ~45 seconds and retry", observed_at),
            None
        );
    }

    #[test]
    fn parse_reset_hint_returns_none_for_overloaded_error_with_no_timing_info() {
        // A real, still-unrecognized provider shape (Anthropic 529):
        // `looks_like_rate_limit` recognizes this text as capacity-related,
        // but no reset-hint rule recognizes a timing signal in it — this
        // stays `None`, not a guess.
        let observed_at = Utc::now();
        let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(parse_reset_hint(body, observed_at), None);
    }

    // ── Golden set: remaining M0-measurement shapes, pinned against
    // `parse_reset_hint` directly (not just `looks_like_rate_limit`) ──────

    #[test]
    fn parse_reset_hint_returns_none_for_bare_429_with_no_hint() {
        let observed_at = Utc::now();
        assert_eq!(parse_reset_hint("429 Too Many Requests", observed_at), None);
    }

    #[test]
    fn parse_reset_hint_returns_none_for_anthropic_rate_limit_error_shape() {
        let observed_at = Utc::now();
        let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of request tokens has exceeded your per-minute rate limit"}}"#;
        assert_eq!(parse_reset_hint(body, observed_at), None);
    }

    #[test]
    fn parse_reset_hint_returns_none_for_openai_insufficient_quota_shape() {
        let observed_at = Utc::now();
        let body = r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#;
        assert_eq!(parse_reset_hint(body, observed_at), None);
    }

    #[test]
    fn parse_reset_hint_recognizes_surges_own_rate_limit_display_form() {
        // The exact prose `SurgeError::RateLimit`'s own `Display` renders —
        // this is the text that ends up in a persisted `StageFailed.reason`
        // (see `stage::tests::rate_limited_display_still_classifies_
        // through_the_full_stage_failed_reason`), so `parse_reset_hint`
        // must read Surge's own shape too, not only a raw provider header.
        let observed_at = t("2026-01-01T00:00:00Z");
        assert_eq!(
            parse_reset_hint(
                "Rate limit exceeded for agent 'claude': retry after 30s (attempt 2)",
                observed_at
            ),
            Some(observed_at + chrono::Duration::seconds(30))
        );
    }

    // ── `CapacityPolicy::decide` (§1, §3 — the rule order is load-bearing) ──

    fn base_policy() -> CapacityPolicy {
        CapacityPolicy {
            blind_backoff: None,
            rotation: RotationPolicy::Disabled,
        }
    }

    /// A `CapacityWindow` with a genuinely fractional `remaining` share and
    /// a learned `window` duration — **not constructible via any public
    /// `CapacityWindow` API** (see `CapacityPolicy::decide`'s rule 5 doc);
    /// built directly through the private fields this test module shares
    /// with the rest of the file, purely to exercise rule 5's comparison
    /// branch as real code.
    fn comparable_window(remaining: f64, window_secs: u64) -> CapacityWindow {
        CapacityWindow {
            runtime: "claude-acp".to_string(),
            window: Some(Duration::from_secs(window_secs)),
            remaining: Some(RemainingShare::new(remaining).unwrap()),
            resets_at: None,
            source: CapacitySource::Observed429,
        }
    }

    #[test]
    fn decide_rule1_exhausted_with_usable_reset_parks_on_observed_reset() {
        let now = t("2026-01-01T00:00:00Z");
        let window = CapacityWindow::observed_429("claude-acp", Some(Duration::from_secs(60)), now);
        let wake_at = window.resets_at().expect("retry_after was Some");
        let status = CapacityStatus::Known(window);
        assert_eq!(
            base_policy().decide(None, &status, now),
            Decision::Park {
                wake_at,
                basis: WakeBasis::ObservedReset,
            }
        );
    }

    #[test]
    fn decide_rule2_exhausted_no_reset_time_with_blind_backoff_parks_on_policy_backoff() {
        let now = t("2026-01-01T00:00:00Z");
        let window = CapacityWindow::observed_429("claude-acp", None, now);
        let status = CapacityStatus::Known(window);
        let policy = CapacityPolicy {
            blind_backoff: Some(Duration::from_secs(300)),
            rotation: RotationPolicy::Disabled,
        };
        assert_eq!(
            policy.decide(None, &status, now),
            Decision::Park {
                wake_at: now + chrono::Duration::seconds(300),
                basis: WakeBasis::PolicyBackoff,
            }
        );
    }

    #[test]
    fn decide_rule3_exhausted_no_reset_time_blind_backoff_off_dispatches_degraded() {
        let now = t("2026-01-01T00:00:00Z");
        let window = CapacityWindow::observed_429("claude-acp", None, now);
        let status = CapacityStatus::Known(window);
        assert_eq!(
            base_policy().decide(None, &status, now),
            Decision::Dispatch {
                degraded: Some(Degraded::ExhaustedNoResetTime)
            }
        );
    }

    #[test]
    fn decide_rule3_elapsed_reset_time_blind_backoff_off_dispatches_reset_elapsed() {
        // The check for "usable" reset time lives in `seconds_until_reset`,
        // not `resets_at().is_some()` (§2 of the plan) — a stale `resets_at`
        // must not be read as evidence of anything, and must not silently
        // collapse into the "never had a reset time at all" reason either.
        let observed_at = t("2026-01-01T00:00:00Z");
        let window =
            CapacityWindow::observed_429("claude-acp", Some(Duration::from_secs(30)), observed_at);
        let well_after_reset = t("2026-01-01T01:00:00Z");
        let status = CapacityStatus::Known(window);
        assert_eq!(
            base_policy().decide(None, &status, well_after_reset),
            Decision::Dispatch {
                degraded: Some(Degraded::ExhaustedResetElapsed)
            }
        );
    }

    #[test]
    fn decide_rule1_wins_over_blind_backoff_when_reset_time_is_usable() {
        // Crossed case (review finding #5, mutation D): a usable observed
        // reset must win over blind backoff even when `blind_backoff` IS
        // configured — every other rule-1 test uses `base_policy()`
        // (`blind_backoff: None`), under which hoisting the entire blind-
        // backoff branch above rule 1 is undetectable. Asserting the exact
        // `wake_at` (not just "a Park happened") pins that it comes from
        // the observed `resets_at`, not from `now + blind_backoff`.
        let now = t("2026-01-01T00:00:00Z");
        let window = CapacityWindow::observed_429("claude-acp", Some(Duration::from_secs(60)), now);
        let wake_at = window.resets_at().expect("retry_after was Some");
        let status = CapacityStatus::Known(window);
        let policy = CapacityPolicy {
            blind_backoff: Some(Duration::from_secs(300)),
            rotation: RotationPolicy::Disabled,
        };
        assert_eq!(
            policy.decide(None, &status, now),
            Decision::Park {
                wake_at,
                basis: WakeBasis::ObservedReset,
            },
            "an observed, still-usable reset time must win over a configured blind backoff"
        );
    }

    #[test]
    fn decide_elapsed_reset_time_with_blind_backoff_parks_on_policy_backoff() {
        // Crossed case (review finding #5, mutation E): the combination
        // "resets_at elapsed" + "blind_backoff configured" had no test at
        // all before this one — `decide_rule2_*` only covers "never had a
        // reset time" + backoff configured, and `decide_rule3_elapsed_*`
        // only covers "elapsed" + no backoff. Asserting the exact
        // `wake_at` (derived from `now + backoff`, not from the stale
        // `resets_at`) pins the elapsed sub-case independently of the
        // never-had-one sub-case.
        let observed_at = t("2026-01-01T00:00:00Z");
        let window =
            CapacityWindow::observed_429("claude-acp", Some(Duration::from_secs(30)), observed_at);
        let well_after_reset = t("2026-01-01T01:00:00Z");
        let status = CapacityStatus::Known(window);
        let policy = CapacityPolicy {
            blind_backoff: Some(Duration::from_secs(300)),
            rotation: RotationPolicy::Disabled,
        };
        assert_eq!(
            policy.decide(None, &status, well_after_reset),
            Decision::Park {
                wake_at: well_after_reset + chrono::Duration::seconds(300),
                basis: WakeBasis::PolicyBackoff,
            }
        );
    }

    #[test]
    fn decide_rule4_missing_estimate_never_causes_refusal_on_any_non_exhausted_status() {
        // RED-before-GREEN acceptance criterion: the absence of a
        // `WorkEstimate` must never, by itself, cause anything other than a
        // clean dispatch — on *every* status that is not exhausted.
        let now = Utc::now();
        let policy = base_policy();
        for status in [
            CapacityStatus::NeverObserved,
            CapacityStatus::Unclassified,
            CapacityStatus::Known(comparable_window(0.5, 3600)),
        ] {
            assert_eq!(
                policy.decide(None, &status, now),
                Decision::Dispatch { degraded: None },
                "status {status:?} with no estimate must dispatch cleanly, never refuse"
            );
        }
    }

    #[test]
    fn decide_rule5_estimate_present_but_never_observed_flags_not_comparable() {
        let now = Utc::now();
        let estimate = WorkEstimate::new(Duration::from_secs(120));
        assert_eq!(
            base_policy().decide(Some(&estimate), &CapacityStatus::NeverObserved, now),
            Decision::Dispatch {
                degraded: Some(Degraded::EstimateNotComparable)
            }
        );
    }

    #[test]
    fn decide_rule5_comparable_window_estimate_fits_dispatches_cleanly() {
        let now = Utc::now();
        // 1 hour window, half remaining => 30 min available; 20 min estimate fits.
        let status = CapacityStatus::Known(comparable_window(0.5, 3600));
        let estimate = WorkEstimate::new(Duration::from_secs(20 * 60));
        assert_eq!(
            base_policy().decide(Some(&estimate), &status, now),
            Decision::Dispatch { degraded: None }
        );
    }

    #[test]
    fn decide_rule5_comparable_window_estimate_exceeds_remaining() {
        let now = Utc::now();
        // 1 hour window, half remaining => 30 min available; 50 min estimate does not fit.
        let status = CapacityStatus::Known(comparable_window(0.5, 3600));
        let estimate = WorkEstimate::new(Duration::from_secs(50 * 60));
        assert_eq!(
            base_policy().decide(Some(&estimate), &status, now),
            Decision::Dispatch {
                degraded: Some(Degraded::EstimateExceedsRemaining)
            }
        );
    }

    #[test]
    fn decide_rule5_full_window_is_comparable_not_flagged() {
        // A full, known-length window is the *most* comparable case there
        // is (review finding #3): `remaining == RemainingShare::FULL` must
        // not be treated as "not a real fraction."
        let now = Utc::now();
        let status = CapacityStatus::Known(comparable_window(1.0, 3600));
        let estimate = WorkEstimate::new(Duration::from_secs(60));
        assert_eq!(
            base_policy().decide(Some(&estimate), &status, now),
            Decision::Dispatch { degraded: None }
        );
    }

    #[test]
    fn decide_never_rotates_even_when_rotation_policy_is_enabled() {
        // Task 12 revision 6 (§1(3c), §3 M1): the seam exists on the type,
        // but `decide` structurally refuses to act on it in this delivery —
        // pin the refusal, not a (not-yet-live) rotation behavior.
        let now = t("2026-01-01T00:00:00Z");
        let rotation = RotationPolicy::Enabled {
            to: crate::keys::ProfileKey::try_from("implementer@1.0").unwrap(),
        };

        let window = CapacityWindow::observed_429("claude-acp", None, now);
        let parking_policy = CapacityPolicy {
            blind_backoff: Some(Duration::from_secs(60)),
            rotation: rotation.clone(),
        };
        assert_eq!(
            parking_policy.decide(None, &CapacityStatus::Known(window.clone()), now),
            Decision::Park {
                wake_at: now + chrono::Duration::seconds(60),
                basis: WakeBasis::PolicyBackoff,
            }
        );

        let dispatching_policy = CapacityPolicy {
            blind_backoff: None,
            rotation,
        };
        assert_eq!(
            dispatching_policy.decide(None, &CapacityStatus::Known(window), now),
            Decision::Dispatch {
                degraded: Some(Degraded::ExhaustedNoResetTime)
            }
        );
    }
}
