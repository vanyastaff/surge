//! Validated scalar and childless binding data; these types confer no runtime authority.

use super::super::guardian::{
    GuardianAuthority, WindowsGuardianContainer, WindowsGuardianProcessIdentity,
};
use super::GuardianRecordError;
use crate::id::{ExecutionWriterId, RunId, StageInvocationId};
use serde::{Deserialize, Serialize};

/// Public operation identity, generated and authenticated outside this pure model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GuardianOperationId([u8; 16]);

impl GuardianOperationId {
    /// Validate existing nonzero bytes without generating an operation.
    pub fn new(bytes: [u8; 16]) -> Result<Self, GuardianRecordError> {
        if bytes == [0; 16] {
            return Err(GuardianRecordError::InvalidOperation);
        }
        Ok(Self(bytes))
    }

    /// Exact operation bytes, independent of their wire spelling.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl TryFrom<String> for GuardianOperationId {
    type Error = GuardianRecordError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(GuardianRecordError::InvalidOperation);
        }
        let mut bytes = [0; 16];
        hex::decode_to_slice(value, &mut bytes)
            .map_err(|_| GuardianRecordError::InvalidOperation)?;
        Self::new(bytes)
    }
}

impl From<GuardianOperationId> for String {
    fn from(value: GuardianOperationId) -> Self {
        hex::encode(value.0)
    }
}

macro_rules! positive_counter {
    ($name:ident, $description:literal, $error:ident $(, $maximum:expr)?) => {
        #[doc = $description]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(try_from = "u64", into = "u64")]
        pub struct $name(u64);

        impl $name {
            /// Validate the positive counter within its declared storage domain.
            pub fn new(value: u64) -> Result<Self, GuardianRecordError> {
                if value == 0 {
                    return Err(GuardianRecordError::$error);
                }
                $(if value > $maximum {
                    return Err(GuardianRecordError::$error);
                })?
                Ok(Self(value))
            }

            /// Original validated counter, without granting authority.
            #[must_use]
            pub fn get(self) -> u64 {
                self.0
            }
        }

        impl TryFrom<u64> for $name {
            type Error = GuardianRecordError;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

positive_counter!(
    GuardianLease,
    "Positive guardian lease epoch.",
    InvalidCounter
);
positive_counter!(
    GuardianRegistryGeneration,
    "Positive registry ownership generation.",
    InvalidCounter
);
positive_counter!(
    GuardianBarrierGeneration,
    "Positive guardian execution barrier generation.",
    InvalidCounter
);
positive_counter!(
    GuardianQueryGeneration,
    "Positive guardian settlement query generation.",
    InvalidCounter
);
positive_counter!(
    GuardianRevocationGeneration,
    "Positive pre-spawn authorization revocation generation.",
    InvalidCounter
);
positive_counter!(
    GuardianHostObservationGeneration,
    "Positive generation of declared host termination evidence.",
    InvalidCounter
);
positive_counter!(
    GuardianJournalSequence,
    "Positive journal sequence bounded by SQLite's signed 64-bit integer domain.",
    InvalidSequence,
    i64::MAX as u64
);

/// Immutable identity context for one declared guardian record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawContext", into = "RawContext")]
pub struct GuardianRecordContext {
    run: RunId,
    invocation: StageInvocationId,
    occurrence: ExecutionWriterId,
    authority: GuardianAuthority,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContext {
    run: String,
    invocation: String,
    occurrence: String,
    authority: GuardianAuthority,
}

impl GuardianRecordContext {
    /// Bind nonnil typed identifiers to an already validated public authority.
    pub fn new(
        run: RunId,
        invocation: StageInvocationId,
        occurrence: ExecutionWriterId,
        authority: GuardianAuthority,
    ) -> Result<Self, GuardianRecordError> {
        if run == RunId::nil()
            || invocation == StageInvocationId::nil()
            || occurrence == ExecutionWriterId::nil()
        {
            return Err(GuardianRecordError::InvalidIdentifier);
        }
        Ok(Self {
            run,
            invocation,
            occurrence,
            authority,
        })
    }

    /// Owning run identifier.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }

    /// Logical stage invocation identifier.
    #[must_use]
    pub fn invocation(&self) -> StageInvocationId {
        self.invocation
    }

    /// Exact execution writer occurrence identifier.
    #[must_use]
    pub fn occurrence(&self) -> ExecutionWriterId {
        self.occurrence
    }

    /// Frozen public authority data, requiring independent runtime authentication.
    #[must_use]
    pub fn authority(&self) -> &GuardianAuthority {
        &self.authority
    }
}

impl TryFrom<RawContext> for GuardianRecordContext {
    type Error = GuardianRecordError;

    fn try_from(raw: RawContext) -> Result<Self, Self::Error> {
        let run: RunId = raw
            .run
            .parse()
            .map_err(|_| GuardianRecordError::InvalidIdentifier)?;
        let invocation: StageInvocationId = raw
            .invocation
            .parse()
            .map_err(|_| GuardianRecordError::InvalidIdentifier)?;
        let occurrence = canonical_occurrence(&raw.occurrence)?;
        if run.as_ulid().to_string() != raw.run
            || invocation.as_ulid().to_string() != raw.invocation
        {
            return Err(GuardianRecordError::InvalidIdentifier);
        }
        Self::new(run, invocation, occurrence, raw.authority)
    }
}

impl From<GuardianRecordContext> for RawContext {
    fn from(value: GuardianRecordContext) -> Self {
        Self {
            run: value.run.as_ulid().to_string(),
            invocation: value.invocation.as_ulid().to_string(),
            occurrence: value.occurrence.as_ulid().to_string(),
            authority: value.authority,
        }
    }
}

/// Guardian binding that makes no claim that a child has been created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawBinding", into = "RawBinding")]
pub struct WindowsGuardianBinding {
    authority: GuardianAuthority,
    occurrence: ExecutionWriterId,
    guardian: WindowsGuardianProcessIdentity,
    endpoint: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinding {
    authority: GuardianAuthority,
    occurrence: String,
    guardian: WindowsGuardianProcessIdentity,
    endpoint: String,
}

impl WindowsGuardianBinding {
    /// Derive the exact local label without fabricating a child identity.
    pub fn new(
        authority: GuardianAuthority,
        occurrence: ExecutionWriterId,
        guardian: WindowsGuardianProcessIdentity,
    ) -> Result<Self, GuardianRecordError> {
        if occurrence == ExecutionWriterId::nil() {
            return Err(GuardianRecordError::InvalidIdentifier);
        }
        let endpoint = format!(
            "surge-guardian-{}-{}",
            hex::encode(authority.installation_id().as_bytes()),
            occurrence.as_ulid().to_string().to_ascii_lowercase()
        );
        Ok(Self {
            authority,
            occurrence,
            guardian,
            endpoint,
        })
    }

    /// Retain only the already validated guardian portion of a G1 container.
    #[must_use]
    pub fn from_container(container: &WindowsGuardianContainer) -> Self {
        let identity = container.identity();
        Self {
            authority: identity.authority().clone(),
            occurrence: identity.occurrence(),
            guardian: identity.guardian().clone(),
            endpoint: identity.endpoint().to_owned(),
        }
    }

    /// Frozen public installation, executable and protocol binding.
    #[must_use]
    pub fn authority(&self) -> &GuardianAuthority {
        &self.authority
    }

    /// Exact execution writer occurrence identifier.
    #[must_use]
    pub fn occurrence(&self) -> ExecutionWriterId {
        self.occurrence
    }

    /// Declared guardian process identity, without a liveness claim.
    #[must_use]
    pub fn guardian(&self) -> &WindowsGuardianProcessIdentity {
        &self.guardian
    }

    /// Derived 106-byte ASCII label, never a secret or remote locator.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl TryFrom<RawBinding> for WindowsGuardianBinding {
    type Error = GuardianRecordError;

    fn try_from(raw: RawBinding) -> Result<Self, Self::Error> {
        let occurrence = canonical_occurrence(&raw.occurrence)?;
        let binding = Self::new(raw.authority, occurrence, raw.guardian)?;
        if raw.endpoint != binding.endpoint {
            return Err(GuardianRecordError::InvalidBinding);
        }
        Ok(binding)
    }
}

impl From<WindowsGuardianBinding> for RawBinding {
    fn from(value: WindowsGuardianBinding) -> Self {
        Self {
            authority: value.authority,
            occurrence: value.occurrence.as_ulid().to_string(),
            guardian: value.guardian,
            endpoint: value.endpoint,
        }
    }
}

fn canonical_occurrence(value: &str) -> Result<ExecutionWriterId, GuardianRecordError> {
    let occurrence: ExecutionWriterId = value
        .parse()
        .map_err(|_| GuardianRecordError::InvalidIdentifier)?;
    if occurrence.as_ulid().to_string() != value {
        return Err(GuardianRecordError::InvalidIdentifier);
    }
    Ok(occurrence)
}
