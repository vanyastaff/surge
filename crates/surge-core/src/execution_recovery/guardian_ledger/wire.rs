//! Strict outer-wire presence and canonical digest adapters.
use super::{
    GuardianLease, GuardianOperationId, GuardianRecordBody, GuardianRecordContext,
    GuardianRecordError, WindowsGuardianRecord,
};
use crate::ContentHash;
use serde::{
    Deserialize, Deserializer,
    de::{self, IntoDeserializer, Visitor},
};
use std::{fmt, marker::PhantomData};

pub(super) fn canonical_hash<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ContentHash, D::Error> {
    let text = String::deserialize(deserializer)?;
    parse_hash(&text).map_err(de::Error::custom)
}
fn parse_hash(text: &str) -> Result<ContentHash, GuardianRecordError> {
    let value: ContentHash = text.parse().map_err(|_| GuardianRecordError::InvalidHash)?;
    if value.to_string() != text {
        return Err(GuardianRecordError::InvalidHash);
    }
    Ok(value)
}
#[derive(Deserialize)]
#[serde(transparent)]
struct StrictHash(#[serde(deserialize_with = "canonical_hash")] ContentHash);

// deserialize_any is deliberate: serde's missing-field deserializer must error,
// whereas delegating to Option<T> would silently accept an omitted field.
struct RequiredNullable<T>(Option<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for RequiredNullable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RequiredVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for RequiredVisitor<T> {
            type Value = RequiredNullable<T>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .write_str("an explicitly present null, unsigned integer or canonical digest")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(RequiredNullable(None))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                T::deserialize(value.into_deserializer()).map(|value| RequiredNullable(Some(value)))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                T::deserialize(value.into_deserializer()).map(|value| RequiredNullable(Some(value)))
            }
        }
        deserializer.deserialize_any(RequiredVisitor(PhantomData))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawRecord {
    version: u32,
    context: GuardianRecordContext,
    operation: GuardianOperationId,
    predecessor: RequiredNullable<StrictHash>,
    lease: RequiredNullable<GuardianLease>,
    body: GuardianRecordBody,
}
impl TryFrom<RawRecord> for WindowsGuardianRecord {
    type Error = GuardianRecordError;
    fn try_from(raw: RawRecord) -> Result<Self, Self::Error> {
        if raw.version != 1 {
            return Err(GuardianRecordError::UnsupportedVersion);
        }
        Self::new(
            raw.context,
            raw.operation,
            raw.predecessor.0.map(|value| value.0),
            raw.lease.0,
            raw.body,
        )
    }
}
