//! `[capacity]` section of `surge.toml` — operator-tunable knobs for
//! [`crate::capacity::CapacityPolicy`] (Task 12, R37/R37.1/R38/R38.1).
//!
//! Kept in its own module rather than folded into `capacity.rs`, mirroring
//! the existing `spill_config.rs`/`loop_config.rs` split: a `surge.toml`
//! section is a different concern (parsing, defaults, validation against
//! other config) from the pure policy it configures, and `capacity.rs` is
//! already close to this crate's own size-budget guidance without it.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default blind-backoff duration (rule 2/3 of
/// [`crate::capacity::CapacityPolicy::decide`]): how long to park a run
/// when the runtime is exhausted but no provider-observed reset time is
/// usable. Generated into `surge.toml` by `surge init` as a visible,
/// editable line (via `CapacityConfig`'s own `Default` impl —
/// `SurgeConfig::save` serializes every field regardless of
/// `#[serde(default)]`, which only governs *parsing*) — not left as an
/// invisible constant an operator can't see or override, per this module's
/// own design principle applied to itself. 5 minutes: short enough that a
/// run parked purely on a guess does not sit idle for long, long enough not
/// to hammer an exhausted provider with near-immediate re-checks.
pub const DEFAULT_BLIND_BACKOFF: Duration = Duration::from_secs(5 * 60);

/// Default cap on consecutive blind parks (no successful dispatch between
/// them) before escalating to the operator instead of parking forever on
/// guesses alone (rule 4, `CapacityPolicy::decide`'s doc). Conservative: a
/// handful of guessed backoffs is normal; dozens in a row without a single
/// successful dispatch is a signal something is stuck, not merely
/// rate-limited.
pub const DEFAULT_BLIND_PARK_LIMIT: u32 = 5;

fn default_blind_park_limit() -> u32 {
    DEFAULT_BLIND_PARK_LIMIT
}

/// `surge.toml` key: `capacity`.
///
/// **`blind_backoff`'s parse-default and `Default`-value deliberately
/// differ, and that is not an inconsistency:** `#[serde(default, with =
/// "humantime_serde::option")]` is the literal precedent this field copies
/// from `ApprovalConfig::elevation_timeout` (`approvals.rs`) — a TOML file
/// *missing* the key parses as `None`, because TOML has no `null` and
/// `Option<T>`'s own zero-value default is the only thing a bare
/// `#[serde(default)]` can supply. That is exactly the mechanism an
/// operator uses to opt out (rule 3 of `CapacityPolicy::decide`): **delete
/// the `blind_backoff` line**, and re-parsing gives `None`, not a sentinel
/// value. `CapacityConfig`'s `Default` impl answers a *different*
/// question — "what does a brand-new `surge.toml` `surge init` writes look
/// like" — and answers `Some(`[`DEFAULT_BLIND_BACKOFF`]`)` so the generated
/// file has a real, visible, editable line rather than an invisible
/// in-code constant an operator has to go read the source to discover.
/// `blind_park_limit` has no such tension (`0` is never a meaningful
/// value), so its parse-default and `Default` value are the same
/// [`DEFAULT_BLIND_PARK_LIMIT`] via [`default_blind_park_limit`]. Neither
/// field needs a `CONFIG_SCHEMA_VERSION` bump (`docs/schema-versioning.md`'s
/// additive-field exception): a config missing this whole section decodes
/// cleanly either way.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CapacityConfig {
    /// Blind-backoff duration for rules 2/3 of
    /// `CapacityPolicy::decide` — parked for this long when the runtime is
    /// exhausted but no usable provider reset time is known. `None` is an
    /// explicit operator opt-out of blind parking (rule 3's "removed by the
    /// operator" branch, spelled by deleting this line from `surge.toml`):
    /// `decide` then dispatches anyway, flagged
    /// `Degraded::ExhaustedNoResetTime`/`Degraded::ExhaustedResetElapsed`,
    /// rather than guessing a wait. See the type doc for why this parses to
    /// `None` when absent yet a fresh `CapacityConfig::default()` carries
    /// `Some(`[`DEFAULT_BLIND_BACKOFF`]`)`.
    #[serde(default, with = "humantime_serde::option")]
    pub blind_backoff: Option<Duration>,
    /// Cap on consecutive blind parks (no successful dispatch between
    /// them) before escalating to the operator instead of parking forever
    /// on guesses. Consumed by the caller that counts parks across the
    /// event log (`surge-daemon`, M4) — `CapacityPolicy::decide` itself is
    /// a single, stateless call and does not enforce this cap.
    #[serde(default = "default_blind_park_limit")]
    pub blind_park_limit: u32,
}

impl Default for CapacityConfig {
    fn default() -> Self {
        Self {
            blind_backoff: Some(DEFAULT_BLIND_BACKOFF),
            blind_park_limit: default_blind_park_limit(),
        }
    }
}

impl CapacityConfig {
    /// Validate this section.
    ///
    /// # Errors
    /// [`crate::SurgeError::Config`] when `blind_park_limit` is `0` — a cap
    /// of zero would escalate before ever attempting a single blind park,
    /// which is not "no blind parking" (that is `blind_backoff: None`) but
    /// a silently-inconsistent way to spell it. Also errors when
    /// `blind_backoff` cannot be represented as a wake-up instant at all
    /// (see below) — an operator gets a clear config error at load time
    /// instead of `CapacityPolicy::decide` discovering it at run time.
    pub fn validate(&self) -> Result<(), crate::SurgeError> {
        if self.blind_park_limit == 0 {
            return Err(crate::SurgeError::Config(
                "capacity.blind_park_limit must be at least 1 (delete capacity.blind_backoff \
                 instead of setting a zero limit to disable blind parking)"
                    .to_string(),
            ));
        }
        if let Some(backoff) = self.blind_backoff {
            // `CapacityPolicy::decide` clamps an unrepresentable
            // `blind_backoff` to `DateTime::<Utc>::MAX_UTC` rather than
            // panicking (defense in depth — see its own doc), but a config
            // this absurd is a mistake worth catching here, at load time,
            // with a message that names the field — not silently clamped
            // three layers away with no operator-visible signal at all.
            let representable = i64::try_from(backoff.as_secs())
                .ok()
                .and_then(chrono::Duration::try_seconds)
                .is_some();
            if !representable {
                return Err(crate::SurgeError::Config(format!(
                    "capacity.blind_backoff ({}s) is too large to represent as a wake-up time; \
                     use a realistic duration (minutes to days)",
                    backoff.as_secs()
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_a_visible_conservative_backoff() {
        let cfg = CapacityConfig::default();
        assert_eq!(cfg.blind_backoff, Some(DEFAULT_BLIND_BACKOFF));
        assert_eq!(cfg.blind_park_limit, DEFAULT_BLIND_PARK_LIMIT);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn absent_section_parses_blind_backoff_as_none_not_the_default() {
        // Unlike `OutputSpillConfig` (whose absent-field default matches its
        // `Default` value), `blind_backoff` deliberately diverges here — see
        // the type doc. TOML has no `null`, so "operator deleted the line"
        // and "line was never there" are the same bytes on disk and must
        // parse the same way: `None`, not silently reintroducing the
        // default the operator just removed.
        let parsed: CapacityConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.blind_backoff, None);
        // `blind_park_limit` has no such tension — its own default fn fires
        // the same way whether reached via `Default` or via a missing key.
        assert_eq!(parsed.blind_park_limit, DEFAULT_BLIND_PARK_LIMIT);
    }

    #[test]
    fn blind_backoff_serializes_as_a_human_readable_duration() {
        // `humantime_serde` — the literal precedent this module copies from
        // `approvals.rs`'s `elevation_timeout` — must render `blind_backoff`
        // as a duration string like `"5m"`, not a raw nanosecond count, so a
        // generated `surge.toml` is something an operator can read and edit.
        let toml_s = toml::to_string(&CapacityConfig::default()).unwrap();
        assert!(
            toml_s.contains("5m"),
            "expected a human-readable duration in generated TOML, got: {toml_s}"
        );
    }

    #[test]
    fn operator_opts_out_of_blind_backoff_by_deleting_the_line() {
        // TOML has no `null` literal — `blind_backoff = none`/`= null` is a
        // parse error, not a way to spell "disabled". The only way to
        // express "disabled" is omitting the key entirely, which is exactly
        // what a config with only `blind_park_limit` set exercises here.
        let parsed: CapacityConfig = toml::from_str("blind_park_limit = 3\n").unwrap();
        assert_eq!(parsed.blind_backoff, None);
        assert_eq!(parsed.blind_park_limit, 3);
        assert!(parsed.validate().is_ok());
    }

    #[test]
    fn zero_blind_park_limit_is_rejected() {
        let cfg = CapacityConfig {
            blind_backoff: None,
            blind_park_limit: 0,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn absurdly_large_blind_backoff_is_rejected_as_a_config_error() {
        // Review finding #2: an operator sees a clear config error at load
        // time, not `CapacityPolicy::decide` clamping (or, before the fix,
        // panicking) three layers away with no visible signal.
        let cfg = CapacityConfig {
            blind_backoff: Some(Duration::from_secs(u64::MAX)),
            blind_park_limit: 1,
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("blind_backoff"),
            "error must name the offending field, got: {err}"
        );
    }

    #[test]
    fn realistic_blind_backoff_values_pass_validation() {
        for secs in [1, 60, 300, 3600, 86_400, 30 * 86_400] {
            let cfg = CapacityConfig {
                blind_backoff: Some(Duration::from_secs(secs)),
                blind_park_limit: 1,
            };
            assert!(cfg.validate().is_ok(), "{secs}s should be a valid backoff");
        }
    }

    #[test]
    fn round_trip_through_toml() {
        let cfg = CapacityConfig {
            blind_backoff: Some(Duration::from_secs(120)),
            blind_park_limit: 3,
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: CapacityConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }
}
