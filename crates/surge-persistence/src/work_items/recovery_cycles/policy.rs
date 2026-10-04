//! Host-frozen routing policy read from the immutable attempt configuration.
use super::*;
use surge_core::NodeKey;

/// Fallback selection mode. This is subscription availability, not a money cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaRoutingMode {
    /// Preserve the primary runtime; quota waits may still retry it.
    Disabled,
    /// Preserve the explicit host-accepted candidate order.
    Configured,
    /// Host resolves and freezes deterministic eligible registry order.
    Automatic,
}
/// One frozen launch target, including model and actual launch fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCandidatePolicy")]
pub struct FrozenQuotaCandidate {
    candidate: RecoveryCandidate,
    model: Option<String>,
    launch_hash: ContentHash,
    #[serde(default)]
    configured_pin: Option<ContentHash>,
    #[serde(default)]
    configured_route: Option<surge_core::config::CapacityRoute>,
}
#[derive(Deserialize)]
struct RawCandidatePolicy {
    candidate: RecoveryCandidate,
    model: Option<String>,
    launch_hash: ContentHash,
    #[serde(default)]
    configured_pin: Option<ContentHash>,
    #[serde(default)]
    configured_route: Option<surge_core::config::CapacityRoute>,
}
impl TryFrom<RawCandidatePolicy> for FrozenQuotaCandidate {
    type Error = WorkItemError;
    fn try_from(value: RawCandidatePolicy) -> Result<Self> {
        let mut candidate = Self::new(value.candidate, value.model, value.launch_hash)?;
        candidate.configured_pin = value.configured_pin;
        candidate.configured_route = value.configured_route;
        Ok(candidate)
    }
}
impl FrozenQuotaCandidate {
    /// Host snapshot from an eligible registry target; credentials are not stored here.
    pub fn new(
        candidate: RecoveryCandidate,
        model: Option<String>,
        launch_hash: ContentHash,
    ) -> Result<Self> {
        if model.as_ref().is_some_and(|value| !identifier(value)) {
            return Err(WorkItemError::Invalid(
                "invalid quota candidate model".into(),
            ));
        }
        Ok(Self {
            candidate,
            model,
            launch_hash,
            configured_pin: None,
            configured_route: None,
        })
    }
    /// Attach a host-created configured source snapshot. This is not observed account evidence.
    pub fn with_configured_snapshot(
        mut self,
        route: surge_core::config::CapacityRoute,
        pin: Option<ContentHash>,
    ) -> Self {
        self.configured_route = Some(route);
        self.configured_pin = pin;
        self
    }
    /// Host snapshot pin, absent for opaque or historic routes.
    pub fn configured_pin(&self) -> Option<&ContentHash> {
        self.configured_pin.as_ref()
    }
    /// Nonsecret configured source declaration.
    pub fn configured_route(&self) -> Option<&surge_core::config::CapacityRoute> {
        self.configured_route.as_ref()
    }
    /// Canonical runtime and actual account evidence.
    pub fn candidate(&self) -> &RecoveryCandidate {
        &self.candidate
    }
    /// Model pinned for this target, if explicitly selected.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
    /// Fingerprint of the actual pinned launch contract.
    pub fn launch_hash(&self) -> &ContentHash {
        &self.launch_hash
    }
}
/// Accepted routing policy for one graph stage; first candidate is its primary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawStagePolicy")]
pub struct FrozenQuotaStage {
    node: NodeKey,
    mode: QuotaRoutingMode,
    candidates: Vec<FrozenQuotaCandidate>,
    policy_backoff_ms: i64,
    observation_ttl_ms: i64,
}
#[derive(Deserialize)]
struct RawStagePolicy {
    node: NodeKey,
    mode: QuotaRoutingMode,
    candidates: Vec<FrozenQuotaCandidate>,
    policy_backoff_ms: i64,
    observation_ttl_ms: i64,
}
impl TryFrom<RawStagePolicy> for FrozenQuotaStage {
    type Error = WorkItemError;
    fn try_from(value: RawStagePolicy) -> Result<Self> {
        Self::new(
            value.node,
            value.mode,
            value.candidates,
            value.policy_backoff_ms,
            value.observation_ttl_ms,
        )
    }
}
impl FrozenQuotaStage {
    /// Reject aliases/accounts that would create two dispatch grants in one cycle.
    pub fn new(
        node: NodeKey,
        mode: QuotaRoutingMode,
        candidates: Vec<FrozenQuotaCandidate>,
        policy_backoff_ms: i64,
        observation_ttl_ms: i64,
    ) -> Result<Self> {
        if candidates.is_empty()
            || candidates.len() > MAX_CANDIDATES as usize
            || (mode == QuotaRoutingMode::Disabled && candidates.len() != 1)
            || policy_backoff_ms <= 0
            || policy_backoff_ms > MAX_BACKOFF_MS
            || observation_ttl_ms <= 0
            || observation_ttl_ms > MAX_BACKOFF_MS
        {
            return Err(WorkItemError::Invalid(
                "invalid frozen quota stage policy".into(),
            ));
        }
        let mut runtimes = std::collections::BTreeSet::new();
        let mut accounts = std::collections::BTreeSet::new();
        for target in &candidates {
            if !runtimes.insert(target.candidate.runtime())
                || !accounts.insert(target.candidate.key()?)
            {
                return Err(WorkItemError::Invalid(
                    "duplicate quota runtime/account in frozen policy".into(),
                ));
            }
        }
        Ok(Self {
            node,
            mode,
            candidates,
            policy_backoff_ms,
            observation_ttl_ms,
        })
    }
    /// Stage in the immutable accepted graph.
    pub fn node(&self) -> &NodeKey {
        &self.node
    }
    /// Accepted fallback selection mode.
    pub fn mode(&self) -> QuotaRoutingMode {
        self.mode
    }
    /// Accepted order; caller never inserts a new target on replay.
    pub fn candidates(&self) -> &[FrozenQuotaCandidate] {
        &self.candidates
    }
    /// Positive bounded retry delay, with no lifetime retry ceiling.
    pub fn policy_backoff_ms(&self) -> i64 {
        self.policy_backoff_ms
    }
    /// Bounded validity of provider observations.
    pub fn observation_ttl_ms(&self) -> i64 {
        self.observation_ttl_ms
    }
}
/// Frozen task configuration field `quota_recovery`, not mutable daemon state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawPolicy")]
pub struct FrozenQuotaPolicy {
    stages: Vec<FrozenQuotaStage>,
}
#[derive(Deserialize)]
struct RawPolicy {
    stages: Vec<FrozenQuotaStage>,
}
impl TryFrom<RawPolicy> for FrozenQuotaPolicy {
    type Error = WorkItemError;
    fn try_from(value: RawPolicy) -> Result<Self> {
        Self::new(value.stages)
    }
}
impl FrozenQuotaPolicy {
    /// Empty policies are valid for graphs with no provider stages.
    pub fn new(stages: Vec<FrozenQuotaStage>) -> Result<Self> {
        let mut nodes = std::collections::BTreeSet::new();
        if stages.iter().any(|stage| !nodes.insert(stage.node.clone())) {
            return Err(WorkItemError::Invalid(
                "duplicate stage in frozen quota policy".into(),
            ));
        }
        Ok(Self { stages })
    }
    /// Exact frozen stage; absence is not fallback permission.
    pub fn stage(&self, node: &NodeKey) -> Option<&FrozenQuotaStage> {
        self.stages.iter().find(|stage| &stage.node == node)
    }
    /// Host snapshots for read-only inspection.
    pub fn stages(&self) -> &[FrozenQuotaStage] {
        &self.stages
    }
    /// Bind receipts to the accepted policy bytes.
    pub fn content_hash(&self) -> Result<ContentHash> {
        Ok(ContentHash::compute(&serde_json::to_vec(self)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(runtime: &str, account: AccountEvidence) -> FrozenQuotaCandidate {
        FrozenQuotaCandidate::new(
            RecoveryCandidate::new(runtime.into(), account).unwrap(),
            None,
            ContentHash::compute(runtime.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn frozen_candidates_reject_known_account_aliases_and_disabled_fallback() {
        let account = AccountEvidence::Known {
            provider: "provider".into(),
            account_key: "account".into(),
        };
        let candidates = vec![candidate("a", account.clone()), candidate("b", account)];
        let node = NodeKey::try_from("implement").unwrap();
        assert!(
            FrozenQuotaStage::new(
                node.clone(),
                QuotaRoutingMode::Configured,
                candidates,
                1000,
                1000
            )
            .is_err()
        );
        let candidates = vec![
            candidate("a", AccountEvidence::Unknown),
            candidate("b", AccountEvidence::Unknown),
        ];
        assert!(
            FrozenQuotaStage::new(
                node.clone(),
                QuotaRoutingMode::Disabled,
                candidates.clone(),
                1000,
                1000
            )
            .is_err()
        );
        let stage =
            FrozenQuotaStage::new(node, QuotaRoutingMode::Configured, candidates, 1000, 1000)
                .unwrap();
        let mut raw = serde_json::to_value(stage).unwrap();
        raw["policy_backoff_ms"] = serde_json::json!(0);
        assert!(serde_json::from_value::<FrozenQuotaStage>(raw).is_err());
    }
}
