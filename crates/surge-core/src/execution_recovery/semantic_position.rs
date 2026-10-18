//! Qualified semantic history positions. These values convey no host authority.
use crate::{ContentHash, RunId};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, num::NonZeroU64};

/// An invalid position or a comparison outside its selected source lineage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PositionError {
    #[error("semantic event positions must be nonzero")]
    Zero,
    #[error("semantic source run must be nonnil")]
    SourceRun,
    #[error("semantic history belongs to another seed or source boundary")]
    ForeignLineage,
    #[error("semantic position is inherited, not a local event index")]
    Inherited,
    #[error("semantic history has no recorded production position")]
    Unrecorded,
}
/// An original event occurrence, without a successor-local sequence conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawSourceEventRef")]
pub struct SourceEventRef {
    run: RunId,
    seq: NonZeroU64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSourceEventRef {
    run: RunId,
    seq: u64,
}
impl SourceEventRef {
    /// Require an actual source run and a positive source event sequence.
    pub fn new(run: RunId, seq: u64) -> Result<Self, PositionError> {
        if run == RunId::nil() {
            return Err(PositionError::SourceRun);
        }
        Ok(Self {
            run,
            seq: NonZeroU64::new(seq).ok_or(PositionError::Zero)?,
        })
    }
    #[must_use]
    /// The original run, never the successor owning a local materialization.
    pub fn run(self) -> RunId {
        self.run
    }
    #[must_use]
    /// The positive event index within the original run.
    pub fn seq(self) -> u64 {
        self.seq.get()
    }
}
impl TryFrom<RawSourceEventRef> for SourceEventRef {
    type Error = PositionError;
    fn try_from(raw: RawSourceEventRef) -> Result<Self, Self::Error> {
        Self::new(raw.run, raw.seq)
    }
}
/// Correlation identity of a semantic seed, never a hydration capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContinuationSeedId(ContentHash);
impl ContinuationSeedId {
    #[must_use]
    /// Correlate a seed by its recorded content digest without authenticating it.
    pub fn new(hash: ContentHash) -> Self {
        Self(hash)
    }
    #[must_use]
    /// The correlation digest.
    pub fn hash(self) -> ContentHash {
        self.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Position {
    Local(NonZeroU64),
    Inherited {
        seed: ContinuationSeedId,
        source: SourceEventRef,
    },
}
/// Semantic ordering in one selected lineage; deliberately has no global Ord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemanticPosition(Position);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InheritedWire {
    version: u32,
    kind: InheritedKind,
    seed: ContinuationSeedId,
    source: SourceEventRef,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InheritedKind {
    Inherited,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum PositionWire {
    Local(u64),
    Inherited(InheritedWire),
}
impl SemanticPosition {
    /// Require a positive successor-local event occurrence.
    pub fn local(seq: u64) -> Result<Self, PositionError> {
        Ok(Self(Position::Local(
            NonZeroU64::new(seq).ok_or(PositionError::Zero)?,
        )))
    }
    #[must_use]
    /// Preserve a source occurrence without assigning a successor-local index.
    pub fn inherited(seed: ContinuationSeedId, source: SourceEventRef) -> Self {
        Self(Position::Inherited { seed, source })
    }
    /// Return a local event index, refusing inherited occurrences.
    pub fn local_seq(self) -> Result<u64, PositionError> {
        match self.0 {
            Position::Local(seq) => Ok(seq.get()),
            Position::Inherited { .. } => Err(PositionError::Inherited),
        }
    }
    #[must_use]
    /// Return source attribution only for an inherited occurrence.
    pub fn provenance(self) -> Option<InheritedProvenance> {
        match self.0 {
            Position::Inherited { seed, source } => Some(InheritedProvenance { seed, source }),
            Position::Local(_) => None,
        }
    }
    /// Validate before comparing, including when only one position is inherited.
    pub fn compare(
        self,
        other: Self,
        lineage: Option<&SeedLineage>,
    ) -> Result<Ordering, PositionError> {
        self.validate(lineage)?;
        other.validate(lineage)?;
        Ok(match (self.0, other.0) {
            (Position::Local(a), Position::Local(b)) => a.cmp(&b),
            (Position::Inherited { source: a, .. }, Position::Inherited { source: b, .. }) => {
                a.seq.cmp(&b.seq)
            },
            (Position::Inherited { .. }, Position::Local(_)) => Ordering::Less,
            (Position::Local(_), Position::Inherited { .. }) => Ordering::Greater,
        })
    }
    /// Require inherited occurrences to match the selected seed, run and boundary.
    pub fn validate(self, lineage: Option<&SeedLineage>) -> Result<(), PositionError> {
        if let Position::Inherited { seed, source } = self.0 {
            let selected = lineage.ok_or(PositionError::ForeignLineage)?;
            if seed != selected.seed
                || source.run != selected.through.run
                || source.seq > selected.through.seq
            {
                return Err(PositionError::ForeignLineage);
            }
        }
        Ok(())
    }
}
impl Serialize for SemanticPosition {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Position::Local(seq) => serializer.serialize_u64(seq.get()),
            Position::Inherited { seed, source } => InheritedWire {
                version: 1,
                kind: InheritedKind::Inherited,
                seed,
                source,
            }
            .serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for SemanticPosition {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match PositionWire::deserialize(deserializer)? {
            PositionWire::Local(seq) => Self::local(seq).map_err(serde::de::Error::custom),
            PositionWire::Inherited(wire) if wire.version == 1 => {
                Ok(Self::inherited(wire.seed, wire.source))
            },
            PositionWire::Inherited(_) => Err(serde::de::Error::custom(
                "unsupported inherited position version",
            )),
        }
    }
}
/// Origin kept separately from the actual local materialization row index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InheritedProvenance {
    seed: ContinuationSeedId,
    source: SourceEventRef,
}
impl InheritedProvenance {
    #[must_use]
    /// Keep source attribution independent of a local materialization row.
    pub fn new(seed: ContinuationSeedId, source: SourceEventRef) -> Self {
        Self { seed, source }
    }
    #[must_use]
    /// The qualified semantic occurrence represented by this attribution.
    pub fn position(self) -> SemanticPosition {
        SemanticPosition::inherited(self.seed, self.source)
    }
    #[must_use]
    /// The original run and event occurrence.
    pub fn source(self) -> SourceEventRef {
        self.source
    }
    #[must_use]
    /// The selected seed correlation identity.
    pub fn seed(self) -> ContinuationSeedId {
        self.seed
    }
}
/// Structural comparison context; it is not authenticated host input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedLineage {
    seed: ContinuationSeedId,
    through: SourceEventRef,
}
impl SeedLineage {
    #[must_use]
    /// Select the inclusive source boundary for structural comparisons.
    pub fn new(seed: ContinuationSeedId, through: SourceEventRef) -> Self {
        Self { seed, through }
    }
    #[must_use]
    /// The selected seed correlation identity.
    pub fn seed(self) -> ContinuationSeedId {
        self.seed
    }
    #[must_use]
    /// The inclusive source boundary allowed by this comparison context.
    pub fn through(self) -> SourceEventRef {
        self.through
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn inherited_87_precedes_local_5_only_with_exact_lineage() {
        let run = RunId::new();
        let seed = ContinuationSeedId::new(ContentHash::compute(b"seed"));
        let lineage = SeedLineage::new(seed, SourceEventRef::new(run, 100).unwrap());
        let inherited = SemanticPosition::inherited(seed, SourceEventRef::new(run, 87).unwrap());
        let local = SemanticPosition::local(5).unwrap();
        assert_eq!(inherited.compare(local, Some(&lineage)), Ok(Ordering::Less));
        assert_eq!(
            inherited.compare(local, None),
            Err(PositionError::ForeignLineage)
        );
        for source in [
            SourceEventRef::new(RunId::new(), 87).unwrap(),
            SourceEventRef::new(run, 101).unwrap(),
        ] {
            assert_eq!(
                SemanticPosition::inherited(seed, source).compare(local, Some(&lineage)),
                Err(PositionError::ForeignLineage)
            );
        }
        assert_eq!(inherited.local_seq(), Err(PositionError::Inherited));
    }
    #[test]
    fn legacy_positive_numeric_shape_and_hostile_json() {
        assert_eq!(
            serde_json::to_value(SemanticPosition::local(87).unwrap()).unwrap(),
            json!(87)
        );
        assert_eq!(
            serde_json::from_value::<SemanticPosition>(json!(87))
                .unwrap()
                .local_seq(),
            Ok(87)
        );
        for invalid in [
            json!(0),
            json!(-1),
            json!({"kind":"local","seq":1}),
            json!({"kind":"inherited","version":2}),
            json!({"kind":"inherited","version":1,"seed":ContentHash::compute(b"seed"),"source":{"run":RunId::new(),"seq":0}}),
        ] {
            assert!(serde_json::from_value::<SemanticPosition>(invalid).is_err());
        }
        assert!(serde_json::from_str::<SemanticPosition>("18446744073709551616").is_err());
        let run = RunId::new();
        assert!(
            serde_json::from_str::<SourceEventRef>(&format!(
                r#"{{"run":"{}","seq":1,"seq":2}}"#,
                run.as_ulid()
            ))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SourceEventRef>(json!({"run":run,"seq":1,"extra":true}))
                .is_err()
        );
    }
}
