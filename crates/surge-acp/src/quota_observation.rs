//! Subscription evidence is separate from binary health and ACP capabilities.
//!
//! ACP v1 exposes no generic subscription-quota query. The default producer is
//! explicitly Unsupported; a typed provider rejection can supply exhaustion.
use serde::{Deserialize, Serialize};

/// Malformed host/provider quota metadata.
#[derive(Debug, thiserror::Error)]
#[error("invalid subscription quota observation: {0}")]
pub struct QuotaObservationError(&'static str);

/// Actual provider account identity, never inferred from an alias or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAccount")]
pub struct ProviderAccountIdentity {
    provider: String,
    account_key: String,
}
#[derive(Deserialize)]
struct RawAccount {
    provider: String,
    account_key: String,
}
impl TryFrom<RawAccount> for ProviderAccountIdentity {
    type Error = QuotaObservationError;
    fn try_from(value: RawAccount) -> Result<Self, Self::Error> {
        Self::new(value.provider, value.account_key)
    }
}
fn identifier(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
impl ProviderAccountIdentity {
    /// Accept only opaque observed identifiers, not account guesses.
    pub fn new(provider: String, account_key: String) -> Result<Self, QuotaObservationError> {
        if !identifier(&provider) || !identifier(&account_key) {
            return Err(QuotaObservationError("account identity"));
        }
        Ok(Self {
            provider,
            account_key,
        })
    }
    /// Canonical provider identity.
    pub fn provider(&self) -> &str {
        &self.provider
    }
    /// Opaque account identity returned by an actual observation.
    pub fn account_key(&self) -> &str {
        &self.account_key
    }
}
/// Available and Exhausted require actual evidence; neither is binary readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionAvailability {
    /// An actual provider observation reports available subscription capacity.
    Available,
    /// An actual provider observation reports exhaustion.
    Exhausted,
}
/// Bounded validity of a real subscription observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawObservation")]
pub struct ObservedSubscriptionQuota {
    availability: SubscriptionAvailability,
    observed_at_ms: i64,
    expires_at_ms: i64,
    reset_at_ms: Option<i64>,
    account: Option<ProviderAccountIdentity>,
}
#[derive(Deserialize)]
struct RawObservation {
    availability: SubscriptionAvailability,
    observed_at_ms: i64,
    expires_at_ms: i64,
    reset_at_ms: Option<i64>,
    account: Option<ProviderAccountIdentity>,
}
impl TryFrom<RawObservation> for ObservedSubscriptionQuota {
    type Error = QuotaObservationError;
    fn try_from(value: RawObservation) -> Result<Self, Self::Error> {
        Self::new(
            value.availability,
            value.observed_at_ms,
            value.expires_at_ms,
            value.reset_at_ms,
            value.account,
        )
    }
}
impl ObservedSubscriptionQuota {
    /// Preserve provider reset times without inventing one for unknown windows.
    pub fn new(
        availability: SubscriptionAvailability,
        observed_at_ms: i64,
        expires_at_ms: i64,
        reset_at_ms: Option<i64>,
        account: Option<ProviderAccountIdentity>,
    ) -> Result<Self, QuotaObservationError> {
        if observed_at_ms < 0
            || expires_at_ms <= observed_at_ms
            || reset_at_ms.is_some_and(|reset| reset < observed_at_ms)
        {
            return Err(QuotaObservationError("observation interval"));
        }
        Ok(Self {
            availability,
            observed_at_ms,
            expires_at_ms,
            reset_at_ms,
            account,
        })
    }
    /// Actual observed subscription state.
    pub fn availability(&self) -> SubscriptionAvailability {
        self.availability
    }
    /// Host observation time.
    pub fn observed_at_ms(&self) -> i64 {
        self.observed_at_ms
    }
    /// Evidence expires at this time.
    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
    /// Provider-reported reset, if known.
    pub fn reset_at_ms(&self) -> Option<i64> {
        self.reset_at_ms
    }
    /// Observed account; absence remains unknown.
    pub fn account(&self) -> Option<&ProviderAccountIdentity> {
        self.account.as_ref()
    }
    /// Expired evidence cannot authorize an availability claim.
    pub fn is_fresh(&self, now_ms: i64) -> bool {
        self.observed_at_ms <= now_ms && now_ms < self.expires_at_ms
    }
}
/// A probe's truthful result; unsupported and failed observations stay distinct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionQuotaProbe {
    /// Actual provider evidence.
    Observed(ObservedSubscriptionQuota),
    /// No supported quota endpoint exists for this runtime/adapter.
    Unsupported,
    /// Probe failed; availability was not established.
    Unknown,
}
/// ACP capabilities, login readiness and usage notifications are not quota queries.
pub fn probe_acp_subscription_quota() -> SubscriptionQuotaProbe {
    SubscriptionQuotaProbe::Unsupported
}
/// Convert only an actual typed quota rejection into exhausted evidence.
/// Other errors return no observation and cannot clear prior exhaustion.
pub fn observe_rate_limit(
    error: &crate::bridge::error::SendMessageError,
    observed_at_ms: i64,
    ttl_ms: i64,
) -> Result<Option<ObservedSubscriptionQuota>, QuotaObservationError> {
    let crate::bridge::error::SendMessageError::RateLimited { retry_after, .. } = error else {
        return Ok(None);
    };
    if ttl_ms <= 0 {
        return Err(QuotaObservationError("observation lifetime"));
    }
    let expires_at_ms = observed_at_ms
        .checked_add(ttl_ms)
        .ok_or(QuotaObservationError("expiry overflow"))?;
    let reset_at_ms = retry_after
        .map(|delay| {
            let millis = i64::try_from(delay.as_millis())
                .map_err(|_| QuotaObservationError("reset overflow"))?;
            observed_at_ms
                .checked_add(millis)
                .ok_or(QuotaObservationError("reset overflow"))
        })
        .transpose()?;
    ObservedSubscriptionQuota::new(
        SubscriptionAvailability::Exhausted,
        observed_at_ms,
        expires_at_ms,
        reset_at_ms,
        None,
    )
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_probe_does_not_claim_available() {
        assert_eq!(
            probe_acp_subscription_quota(),
            SubscriptionQuotaProbe::Unsupported
        );
    }
    #[test]
    fn typed_rejection_retains_unknown_reset_and_account() {
        let error = crate::bridge::error::SendMessageError::RateLimited {
            retry_after: None,
            details: "quota exhausted".into(),
        };
        let observed = observe_rate_limit(&error, 100, 1000).unwrap().unwrap();
        assert_eq!(observed.availability(), SubscriptionAvailability::Exhausted);
        assert_eq!(observed.reset_at_ms(), None);
        assert_eq!(observed.account(), None);
        assert!(observed.is_fresh(1099));
        assert!(!observed.is_fresh(1100));
    }
    #[test]
    fn malformed_account_and_observation_cannot_decode() {
        assert!(
            serde_json::from_str::<ProviderAccountIdentity>(
                r#"{"provider":"","account_key":"opaque"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<ObservedSubscriptionQuota>(r#"{"availability":"Available","observed_at_ms":100,"expires_at_ms":99,"reset_at_ms":null,"account":null}"#).is_err());
    }
}
