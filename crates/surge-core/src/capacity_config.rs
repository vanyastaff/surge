//! `[capacity]` section of `surge.toml` — operator-tunable knobs for
//! [`crate::capacity::CapacityPolicy`] (Task 12, R37/R37.1/R38/R38.1).
//!
//! Kept in its own module rather than folded into `capacity.rs`, mirroring
//! the existing `spill_config.rs`/`loop_config.rs` split: a `surge.toml`
//! section is a different concern (parsing, defaults, validation against
//! other config) from the pure policy it configures, and `capacity.rs` is
//! already close to this crate's own size-budget guidance without it.

use chrono::Utc;
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

/// Default upper bound on the deterministic per-run "herd" jitter
/// [`crate::capacity::CapacityPolicy::apply_park_jitter`] adds to a park's
/// `wake_at` (Task 12 M4, ADR-0016 §14). 30 seconds: small relative to
/// [`DEFAULT_BLIND_BACKOFF`]'s 5 minutes (10%) — enough to break up many
/// runs waking in the same tick against one exhausted runtime, not so much
/// that any single run waits meaningfully longer than the policy already
/// decided. Visible and editable in `surge.toml` for the same reason
/// `DEFAULT_BLIND_BACKOFF` is (see that constant's doc): a real, generated
/// value an operator can find and tune, not an invisible in-code constant.
pub const DEFAULT_JITTER_MAX: Duration = Duration::from_secs(30);

fn default_jitter_max() -> Duration {
    DEFAULT_JITTER_MAX
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
    /// Cap on consecutive blind parks (`WakeBasis::PolicyBackoff`, no
    /// successful dispatch between them) before escalating to the operator
    /// instead of parking forever on guesses. Consumed by
    /// `surge-daemon::wake_scheduler::WakeScheduler`, which counts the
    /// streak from the run's own event log
    /// (`surge_persistence::runs::RunStatusSnapshot::consecutive_blind_parks`)
    /// and raises `EventPayload::EscalationRequested` once the count
    /// reaches this value — `CapacityPolicy::decide` itself is a single,
    /// stateless call and does not enforce this cap.
    #[serde(default = "default_blind_park_limit")]
    pub blind_park_limit: u32,
    /// Upper bound on the deterministic per-run jitter added to a park's
    /// `wake_at` (Task 12 M4). Unlike `blind_backoff`, there is no
    /// operator-facing "opt out" ambiguity to preserve: `0s` **is** the
    /// natural spelling of "no jitter", so — unlike `blind_backoff` —
    /// this field's parse-default and its `Default`-impl value are the
    /// same [`DEFAULT_JITTER_MAX`], both via [`default_jitter_max`]; a
    /// `surge.toml` missing this key behaves exactly like a freshly
    /// generated one.
    #[serde(default = "default_jitter_max", with = "humantime_serde")]
    pub jitter_max: Duration,
}

impl Default for CapacityConfig {
    fn default() -> Self {
        Self {
            blind_backoff: Some(DEFAULT_BLIND_BACKOFF),
            blind_park_limit: default_blind_park_limit(),
            jitter_max: default_jitter_max(),
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
            //
            // Two representability layers, and both must hold (Task 12 M3,
            // review finding #2 on the M1 ticket): `chrono::Duration` (a
            // signed nanosecond count) has a *much* wider range than
            // `DateTime<Utc>` (bounded to roughly ±262,000 years, chrono's
            // proleptic-Gregorian `NaiveDate` limit). A value like
            // `9_000_000_000_000s` (~285,000 years) passes
            // `chrono::Duration::try_seconds` cleanly — it is nowhere near
            // that type's own i64-nanosecond ceiling — yet still overflows
            // `Utc::now() + backoff` once added to a real instant. Checking
            // only the first layer let exactly that value load without
            // error and reach `decide`'s clamp silently — the "silent
            // clamp three layers down" this validator's own doc says it
            // exists to prevent. Anchoring the check at an actual
            // `checked_add_signed` from `now` (not a fixed epoch) proves
            // the value the operator wrote is still representable *today*,
            // which is what `decide` will actually attempt at dispatch
            // time.
            let representable = i64::try_from(backoff.as_secs())
                .ok()
                .and_then(chrono::Duration::try_seconds)
                .is_some_and(|delta| Utc::now().checked_add_signed(delta).is_some());
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

/// Build the pure policy this section configures (Task 12 M3, acceptance
/// criterion B).
///
/// Before M3, `[capacity]` was a documented knob nothing read: every
/// `surge.toml` `surge init` writes carries a real, editable
/// `blind_backoff`, but no production caller ever turned it into a
/// [`crate::capacity::CapacityPolicy`] — an operator could edit the line
/// and change nothing. This `From` impl is the one conversion the engine's
/// production wiring (`surge-cli`, `surge-daemon`) calls at startup so that
/// stops being true; see those crates' `Engine` construction sites for the
/// call.
///
/// `blind_park_limit` has no field on [`crate::capacity::CapacityPolicy`]:
/// that cap is consumed by the caller that counts consecutive blind parks
/// across the event log (`surge-daemon`'s wake scheduler, Task 12 M4) —
/// `decide` itself is a single, stateless call and never enforces it (see
/// that field's own doc). `rotation` is always
/// [`crate::capacity::RotationPolicy::Disabled`]: Task 12 M3 does not wire
/// a verified rotation candidate into this conversion (R41 is deferred to
/// its own follow-up — see `docs/adr/0016-capacity-parking-and-wake.md`,
/// A2), and there is no `surge.toml` field yet for a caller to have
/// supplied one from.
impl From<&CapacityConfig> for crate::capacity::CapacityPolicy {
    fn from(cfg: &CapacityConfig) -> Self {
        Self {
            blind_backoff: cfg.blind_backoff,
            rotation: crate::capacity::RotationPolicy::Disabled,
            jitter_max: cfg.jitter_max,
        }
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
            jitter_max: default_jitter_max(),
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
            jitter_max: default_jitter_max(),
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("blind_backoff"),
            "error must name the offending field, got: {err}"
        );
    }

    #[test]
    fn blind_backoff_representable_as_a_chrono_duration_but_not_as_a_wake_up_instant_is_rejected() {
        // Task 12 M3, acceptance criterion C: `validate` closed only one of
        // two representability layers. `chrono::Duration::try_seconds`
        // checks fit against that type's own i64-nanosecond range (roughly
        // 292 billion years) — nowhere near this value — so a
        // pre-criterion-C `validate` accepted it. `DateTime<Utc>` is bounded
        // to roughly +/-262,000 years (chrono's proleptic-Gregorian
        // `NaiveDate` limit); 9_000_000_000_000s is about 285,000 years,
        // which overflows that *second* layer. Before the fix, this value
        // loaded cleanly and was silently clamped to `DateTime::<Utc>::
        // MAX_UTC` three layers down inside `CapacityPolicy::decide` —
        // exactly the outcome this validator's own doc says it exists to
        // prevent. Proven directly against `chrono::Duration::try_seconds`
        // below so this test cannot pass for the wrong reason (a value that
        // was never going to clear the first layer either).
        let absurd_secs: u64 = 9_000_000_000_000;
        assert!(
            i64::try_from(absurd_secs)
                .ok()
                .and_then(chrono::Duration::try_seconds)
                .is_some(),
            "test premise broken: {absurd_secs}s must clear the chrono::Duration layer alone \
             for this test to actually exercise the second (calendar) layer"
        );

        let cfg = CapacityConfig {
            blind_backoff: Some(Duration::from_secs(absurd_secs)),
            blind_park_limit: 1,
            jitter_max: default_jitter_max(),
        };
        let err = cfg
            .validate()
            .expect_err("a backoff that overflows DateTime<Utc> must be rejected at load time");
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
                jitter_max: default_jitter_max(),
            };
            assert!(cfg.validate().is_ok(), "{secs}s should be a valid backoff");
        }
    }

    #[test]
    fn round_trip_through_toml() {
        let cfg = CapacityConfig {
            blind_backoff: Some(Duration::from_secs(120)),
            blind_park_limit: 3,
            jitter_max: default_jitter_max(),
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: CapacityConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn from_capacity_config_carries_blind_backoff_and_disables_rotation() {
        // Task 12 M3, acceptance criterion B's mapping half: a configured
        // `blind_backoff` must survive the conversion into
        // `CapacityPolicy` unchanged (this is the value the production
        // wiring in `surge-cli`/`surge-daemon` reads at startup), and
        // rotation must come out `Disabled` regardless of input — there is
        // no `surge.toml` field yet that could turn it on (R41 is
        // deferred; see this impl's own doc).
        let cfg = CapacityConfig {
            blind_backoff: Some(Duration::from_secs(777)),
            blind_park_limit: 9,
            jitter_max: default_jitter_max(),
        };
        let policy = crate::capacity::CapacityPolicy::from(&cfg);
        assert_eq!(policy.blind_backoff, Some(Duration::from_secs(777)));
        assert_eq!(policy.rotation, crate::capacity::RotationPolicy::Disabled);
    }

    #[test]
    fn from_capacity_config_carries_operator_opt_out() {
        // The `None` (opted-out) case must also survive unchanged — a
        // silently-reintroduced default here would be the same "config
        // that lies about what it does" defect this section's own module
        // doc says the `[capacity]` section exists to avoid.
        let cfg = CapacityConfig {
            blind_backoff: None,
            blind_park_limit: 1,
            jitter_max: default_jitter_max(),
        };
        let policy = crate::capacity::CapacityPolicy::from(&cfg);
        assert_eq!(policy.blind_backoff, None);
    }

    // ── `jitter_max` (Task 12 M4) ──

    #[test]
    fn default_jitter_max_is_visible_and_matches_the_documented_constant() {
        let cfg = CapacityConfig::default();
        assert_eq!(cfg.jitter_max, DEFAULT_JITTER_MAX);
        assert_eq!(DEFAULT_JITTER_MAX, Duration::from_secs(30));
    }

    #[test]
    fn absent_jitter_max_parses_to_the_default_not_zero() {
        // Unlike `blind_backoff`, `jitter_max` has no `None`/opt-out
        // ambiguity to preserve — a `surge.toml` missing the key gets the
        // same default a fresh `CapacityConfig::default()` does, not a
        // silently-disabling zero.
        let parsed: CapacityConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.jitter_max, DEFAULT_JITTER_MAX);
    }

    #[test]
    fn jitter_max_serializes_as_a_human_readable_duration() {
        let toml_s = toml::to_string(&CapacityConfig::default()).unwrap();
        assert!(
            toml_s.contains("30s"),
            "expected a human-readable jitter_max duration in generated TOML, got: {toml_s}"
        );
    }

    #[test]
    fn operator_can_disable_jitter_by_setting_it_to_zero() {
        let parsed: CapacityConfig = toml::from_str("jitter_max = \"0s\"\n").unwrap();
        assert_eq!(parsed.jitter_max, Duration::ZERO);
        assert!(parsed.validate().is_ok());
    }

    #[test]
    fn round_trip_jitter_max_through_toml() {
        let cfg = CapacityConfig {
            blind_backoff: Some(Duration::from_secs(120)),
            blind_park_limit: 3,
            jitter_max: Duration::from_secs(45),
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: CapacityConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn from_capacity_config_carries_jitter_max() {
        let cfg = CapacityConfig {
            blind_backoff: None,
            blind_park_limit: 1,
            jitter_max: Duration::from_secs(17),
        };
        let policy = crate::capacity::CapacityPolicy::from(&cfg);
        assert_eq!(policy.jitter_max, Duration::from_secs(17));
    }
}
