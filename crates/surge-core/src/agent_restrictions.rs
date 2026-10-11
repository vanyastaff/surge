//! Pure canonical-ID restrictions. Callers resolve registry aliases before evaluation.
//! Frozen and current policies must be evaluated independently, retaining both selectors.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Exact, nonblank registry or ACP identifier; display labels are never aliases.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RestrictionId(String);
impl RestrictionId {
    /// Validate an exact identifier without normalization.
    pub fn new(value: impl Into<String>) -> Result<Self, PolicyError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PolicyError::BlankIdentifier);
        }
        Ok(Self(value))
    }
    /// Original canonical spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::borrow::Borrow<str> for RestrictionId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}
impl AsRef<str> for RestrictionId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl std::fmt::Display for RestrictionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
impl TryFrom<String> for RestrictionId {
    type Error = PolicyError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<RestrictionId> for String {
    fn from(value: RestrictionId) -> Self {
        value.0
    }
}
/// Policy construction errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// Identifier contained no non-whitespace characters.
    #[error("restriction identifier must not be blank")]
    BlankIdentifier,
    /// A model is simultaneously permitted and prohibited.
    #[error("model {0} is both allowed and disabled")]
    ContradictoryModel(String),
    /// A selector cannot represent independent dimensions simultaneously.
    #[error("selector {0} is declared for multiple dimensions")]
    ConflictingSelector(String),
}
/// Independent canonical selection dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestrictionDimension {
    Model,
    Effort,
    Thinking,
}
/// Validated per-route restrictions. Omitted allow lists are unconstrained;
/// present empty lists deny every value, including an absent default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AgentRestrictionsInput", into = "AgentRestrictionsInput")]
pub struct AgentRestrictions {
    input: AgentRestrictionsInput,
}
/// Construction DTO; conversion validates cross-field invariants.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentRestrictionsInput {
    /// Optional exact canonical model values.
    pub allowed_models: Option<BTreeSet<RestrictionId>>,
    /// Exact prohibited models.
    pub disabled_models: BTreeSet<RestrictionId>,
    /// Optional exact canonical effort values.
    pub allowed_effort: Option<BTreeSet<RestrictionId>>,
    /// Optional exact canonical thinking values.
    pub allowed_thinking: Option<BTreeSet<RestrictionId>>,
    /// Explicit model control identity.
    pub model_option_id: Option<RestrictionId>,
    /// Explicit effort control identity.
    pub effort_option_id: Option<RestrictionId>,
    /// Explicit thinking control identity, independent of effort.
    pub thinking_option_id: Option<RestrictionId>,
}
impl TryFrom<AgentRestrictionsInput> for AgentRestrictions {
    type Error = PolicyError;
    fn try_from(input: AgentRestrictionsInput) -> Result<Self, Self::Error> {
        if let Some(allowed) = &input.allowed_models
            && let Some(conflict) = allowed.intersection(&input.disabled_models).next()
        {
            return Err(PolicyError::ContradictoryModel(
                conflict.as_str().to_owned(),
            ));
        }
        let mut selectors = BTreeSet::new();
        for selector in [
            &input.model_option_id,
            &input.effort_option_id,
            &input.thinking_option_id,
        ]
        .into_iter()
        .flatten()
        {
            if !selectors.insert(selector) {
                return Err(PolicyError::ConflictingSelector(
                    selector.as_str().to_owned(),
                ));
            }
        }
        Ok(Self { input })
    }
}
impl From<AgentRestrictions> for AgentRestrictionsInput {
    fn from(value: AgentRestrictions) -> Self {
        value.input
    }
}
/// Pure policy, with canonical registry keys and explicit family data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentRestrictionPolicy {
    /// Canonical routes disabled regardless of value permissions.
    pub disabled_agents: BTreeSet<RestrictionId>,
    /// Explicit provider families disabled regardless of route permissions.
    pub disabled_provider_families: BTreeSet<RestrictionId>,
    /// Restrictions keyed by canonical registry route.
    #[serde(deserialize_with = "deserialize_agents")]
    pub agents: BTreeMap<RestrictionId, AgentRestrictions>,
}
fn deserialize_agents<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<RestrictionId, AgentRestrictions>, D::Error> {
    struct UniqueAgents;
    impl<'de> serde::de::Visitor<'de> for UniqueAgents {
        type Value = BTreeMap<RestrictionId, AgentRestrictions>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a map with unique canonical agent routes")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut agents = BTreeMap::new();
            while let Some((route, rules)) = map.next_entry::<RestrictionId, AgentRestrictions>()? {
                if agents.contains_key(&route) {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate agent route {route}"
                    )));
                }
                agents.insert(route, rules);
            }
            Ok(agents)
        }
    }
    deserializer.deserialize_map(UniqueAgents)
}
/// Typed evaluation diagnostic. Runtime owners add node/session context.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RestrictionViolation {
    /// Canonical route disabled.
    #[error("agent route {0} is disabled")]
    DisabledAgent(String),
    /// Explicit family disabled.
    #[error("provider family {0} is disabled")]
    DisabledFamily(String),
    /// Cannot establish family while a family rule is active.
    #[error("provider family is unknown for route {0}")]
    UnknownFamily(String),
    /// Value denied or missing under an active value constraint.
    #[error("{dimension:?} value {value:?} violates restrictions")]
    ValueDenied {
        dimension: RestrictionDimension,
        value: Option<String>,
    },
    /// Explicit selector did not identify the observed control.
    #[error("{dimension:?} requires selector {expected}, observed {actual:?}")]
    SelectorMismatch {
        dimension: RestrictionDimension,
        expected: String,
        actual: Option<String>,
    },
    /// Required effort floor is opaque or cannot be established.
    #[error("effort {actual:?} cannot establish required floor {floor}")]
    EffortFloor {
        floor: String,
        actual: Option<String>,
    },
}
impl AgentRestrictionPolicy {
    /// Check canonical identity before reservation/provider RPC. No family inference.
    pub fn check_route(
        &self,
        route: &str,
        family: Option<&str>,
    ) -> Result<(), RestrictionViolation> {
        if self.disabled_agents.iter().any(|id| id.as_str() == route) {
            return Err(RestrictionViolation::DisabledAgent(route.to_owned()));
        }
        if !self.disabled_provider_families.is_empty() {
            let family = family
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| RestrictionViolation::UnknownFamily(route.to_owned()))?;
            if self
                .disabled_provider_families
                .iter()
                .any(|id| id.as_str() == family)
            {
                return Err(RestrictionViolation::DisabledFamily(family.to_owned()));
            }
        }
        Ok(())
    }
    /// Rules for a canonical route; aliases must already have been resolved.
    pub fn for_agent(&self, route: &str) -> Option<&AgentRestrictions> {
        self.agents.get(route)
    }
}
impl AgentRestrictions {
    /// Exact requested selector, if operator specified one.
    pub fn selector(&self, dimension: RestrictionDimension) -> Option<&str> {
        match dimension {
            RestrictionDimension::Model => self.input.model_option_id.as_ref(),
            RestrictionDimension::Effort => self.input.effort_option_id.as_ref(),
            RestrictionDimension::Thinking => self.input.thinking_option_id.as_ref(),
        }
        .map(RestrictionId::as_str)
    }
    /// Whether actual state must establish a value for this dimension.
    pub fn constrains(&self, dimension: RestrictionDimension) -> bool {
        self.allowed(dimension).is_some()
            || (dimension == RestrictionDimension::Model && !self.input.disabled_models.is_empty())
    }
    /// Exact permitted values; `None` means unconstrained and empty means deny all.
    pub fn allowed(&self, dimension: RestrictionDimension) -> Option<&BTreeSet<RestrictionId>> {
        match dimension {
            RestrictionDimension::Model => self.input.allowed_models.as_ref(),
            RestrictionDimension::Effort => self.input.allowed_effort.as_ref(),
            RestrictionDimension::Thinking => self.input.allowed_thinking.as_ref(),
        }
    }
    /// Exact model deny list, retained independently from optional permission lists.
    pub fn disabled_models(&self) -> &BTreeSet<RestrictionId> {
        &self.input.disabled_models
    }
    /// Evaluate canonical value and exact control identity; never use display labels.
    /// Call this separately for each frozen/current predicate and its retained binding.
    pub fn check_selection(
        &self,
        dimension: RestrictionDimension,
        option_id: Option<&str>,
        value: Option<&str>,
    ) -> Result<(), RestrictionViolation> {
        if let Some(expected) = self.selector(dimension)
            && option_id != Some(expected)
        {
            return Err(RestrictionViolation::SelectorMismatch {
                dimension,
                expected: expected.to_owned(),
                actual: option_id.map(str::to_owned),
            });
        }
        if !self.constrains(dimension) {
            return Ok(());
        }
        let permitted = value.is_some_and(|value| {
            !value.trim().is_empty()
                && self
                    .allowed(dimension)
                    .is_none_or(|allowed| allowed.iter().any(|id| id.as_str() == value))
                && (dimension != RestrictionDimension::Model
                    || !self
                        .input
                        .disabled_models
                        .iter()
                        .any(|id| id.as_str() == value))
        });
        if permitted {
            return Ok(());
        }
        Err(RestrictionViolation::ValueDenied {
            dimension,
            value: value.map(str::to_owned),
        })
    }
}
/// Shared legacy effort ordering. Rank is case-insensitive; policy IDs remain exact.
pub fn effort_rank(value: &str) -> Option<usize> {
    ["minimal", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .position(|known| known.eq_ignore_ascii_case(value))
}
/// Raise a preference to its ranked floor; opaque values retain legacy best effort.
pub fn effective_effort<'a>(value: Option<&'a str>, floor: Option<&'a str>) -> Option<&'a str> {
    match (value, floor) {
        (Some(value), Some(floor)) if matches!((effort_rank(value), effort_rank(floor)), (Some(v), Some(f)) if v < f) => {
            Some(floor)
        },
        (None, floor) => floor,
        (value, _) => value,
    }
}
/// Verify a required floor rather than silently accepting opaque values.
pub fn check_effort_floor(value: Option<&str>, floor: &str) -> Result<(), RestrictionViolation> {
    if matches!((value.and_then(effort_rank), effort_rank(floor)), (Some(v), Some(f)) if v >= f) {
        return Ok(());
    }
    Err(RestrictionViolation::EffortFloor {
        floor: floor.to_owned(),
        actual: value.map(str::to_owned),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn rules(json: &str) -> AgentRestrictions {
        serde_json::from_str(json).expect("valid rules")
    }
    #[test]
    fn rejects_contradictory_declarations() {
        assert!(
            serde_json::from_str::<AgentRestrictions>(
                r#"{"allowed_models":["M"],"disabled_models":["M"]}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<AgentRestrictions>(
                r#"{"model_option_id":"same","thinking_option_id":"same"}"#
            )
            .is_err()
        );
    }
    #[test]
    fn omitted_empty_and_exact_case_are_distinct() {
        assert!(
            rules("{}")
                .check_selection(RestrictionDimension::Model, None, None)
                .is_ok()
        );
        assert!(
            rules(r#"{"allowed_models":[]}"#)
                .check_selection(RestrictionDimension::Model, None, Some("M"))
                .is_err()
        );
        let rule = rules(r#"{"allowed_models":["Model-ID"]}"#);
        assert!(
            rule.check_selection(RestrictionDimension::Model, None, Some("Model-ID"))
                .is_ok()
        );
        for value in [None, Some("model-id"), Some("Pretty model")] {
            assert!(
                rule.check_selection(RestrictionDimension::Model, None, value)
                    .is_err()
            );
        }
    }
    #[test]
    fn route_and_family_precede_values() {
        let policy: AgentRestrictionPolicy=serde_json::from_str(r#"{"disabled_agents":["canonical"],"disabled_provider_families":["family"],"agents":{"canonical":{"allowed_models":["M"]}}}"#).unwrap();
        assert!(matches!(
            policy.check_route("canonical", Some("other")),
            Err(RestrictionViolation::DisabledAgent(_))
        ));
        assert!(matches!(
            policy.check_route("another", Some("family")),
            Err(RestrictionViolation::DisabledFamily(_))
        ));
        assert!(matches!(
            policy.check_route("another", None),
            Err(RestrictionViolation::UnknownFamily(_))
        ));
        assert!(policy.check_route("another", Some("other")).is_ok());
    }
    #[test]
    fn independent_selectors_cannot_be_merged() {
        let frozen = rules(r#"{"allowed_effort":["high"],"effort_option_id":"old"}"#);
        let current = rules(r#"{"allowed_effort":["high"],"effort_option_id":"new"}"#);
        assert!(
            frozen
                .check_selection(RestrictionDimension::Effort, Some("old"), Some("high"))
                .is_ok()
        );
        assert!(
            current
                .check_selection(RestrictionDimension::Effort, Some("old"), Some("high"))
                .is_err()
        );
        assert!(
            current
                .check_selection(RestrictionDimension::Effort, Some("new"), Some("high"))
                .is_ok()
        );
        assert!(
            frozen
                .check_selection(RestrictionDimension::Effort, Some("new"), Some("high"))
                .is_err()
        );
    }
    #[test]
    fn deny_models_and_independent_thinking() {
        let rule = rules(r#"{"disabled_models":["M"],"allowed_thinking":["enabled"]}"#);
        assert!(
            rule.check_selection(RestrictionDimension::Model, None, Some("M"))
                .is_err()
        );
        assert!(
            rule.check_selection(RestrictionDimension::Model, None, None)
                .is_err()
        );
        assert!(
            rule.check_selection(RestrictionDimension::Model, None, Some("m"))
                .is_ok()
        );
        assert!(
            rule.check_selection(RestrictionDimension::Thinking, None, Some("high"))
                .is_err()
        );
        assert!(
            rule.check_selection(RestrictionDimension::Thinking, None, Some("enabled"))
                .is_ok()
        );
    }
    #[test]
    fn rejects_blank_and_unknown_fields() {
        for json in [
            r#"{"allowed_models":[" "]}"#,
            r#"{"typo":[]}"#,
            r#"{"thinking_option_id":""}"#,
        ] {
            assert!(serde_json::from_str::<AgentRestrictions>(json).is_err());
        }
        for json in [r#"{"agents":{" ":{}}}"#, r#"{"unknown":[]}"#] {
            assert!(serde_json::from_str::<AgentRestrictionPolicy>(json).is_err());
        }
    }
    #[test]
    fn every_allowlist_and_exact_selector_survives_roundtrip() {
        for (dimension, field, selector) in [
            (
                RestrictionDimension::Model,
                "allowed_models",
                "model_option_id",
            ),
            (
                RestrictionDimension::Effort,
                "allowed_effort",
                "effort_option_id",
            ),
            (
                RestrictionDimension::Thinking,
                "allowed_thinking",
                "thinking_option_id",
            ),
        ] {
            let input = format!(r#"{{"{field}":[],"{selector}":"exact"}}"#);
            let original = rules(&input);
            let roundtrip = rules(&serde_json::to_string(&original).unwrap());
            assert_eq!(original, roundtrip);
            assert!(roundtrip.constrains(dimension));
            assert!(
                roundtrip
                    .check_selection(dimension, Some("exact"), Some("value"))
                    .is_err()
            );
            let selector_only = rules(&format!(r#"{{"{selector}":"exact"}}"#));
            assert!(
                selector_only
                    .check_selection(dimension, None, None)
                    .is_err()
            );
            assert!(
                selector_only
                    .check_selection(dimension, Some("EXACT"), None)
                    .is_err()
            );
            assert!(
                selector_only
                    .check_selection(dimension, Some("exact"), None)
                    .is_ok()
            );
        }
    }
    #[test]
    fn duplicate_route_declarations_are_rejected() {
        assert!(
            serde_json::from_str::<AgentRestrictionPolicy>(
                r#"{"agents":{"route":{"allowed_models":[]},"route":{}}}"#
            )
            .is_err()
        );
    }
    #[test]
    fn effort_floor_retains_legacy_but_required_unknown_fails() {
        assert_eq!(effective_effort(Some("low"), Some("high")), Some("high"));
        assert_eq!(
            effective_effort(Some("opaque"), Some("high")),
            Some("opaque")
        );
        assert_eq!(effort_rank("XHIGH"), Some(4));
        assert!(check_effort_floor(Some("XHIGH"), "high").is_ok());
        assert!(check_effort_floor(Some("opaque"), "high").is_err());
        assert!(check_effort_floor(Some("max"), "opaque").is_err());
        assert!(check_effort_floor(None, "high").is_err());
        assert!(check_effort_floor(Some("low"), "high").is_err());
    }
}
