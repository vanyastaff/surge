//! Host-observed writer identity and conservative cold-recovery evidence.
use crate::id::{ExecutionWriterId, StageInvocationId};
use serde::{Deserialize, Serialize};

/// Platform whose identity tokens were captured by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPlatform {
    Linux,
    MacOs,
    Windows,
}

/// PID bound to a machine, boot and precise process creation identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawIdentity")]
pub struct ProcessIdentity {
    platform: ProcessPlatform,
    machine: crate::ContentHash,
    boot: String,
    pid: u32,
    start: String,
}
#[derive(Deserialize)]
struct RawIdentity {
    platform: ProcessPlatform,
    machine: crate::ContentHash,
    boot: String,
    pid: u32,
    start: String,
}
impl TryFrom<RawIdentity> for ProcessIdentity {
    type Error = super::RecoveryIdentityError;
    fn try_from(raw: RawIdentity) -> Result<Self, Self::Error> {
        Self::new(raw.platform, raw.machine, raw.boot, raw.pid, raw.start)
    }
}
impl ProcessIdentity {
    /// Validate bounded host identity tokens; observation remains an I/O-owner responsibility.
    pub fn new(
        platform: ProcessPlatform,
        machine: crate::ContentHash,
        boot: String,
        pid: u32,
        start: String,
    ) -> Result<Self, super::RecoveryIdentityError> {
        if pid == 0
            || [&boot, &start].into_iter().any(|token| {
                token.is_empty() || token.len() > 256 || token.chars().any(char::is_control)
            })
        {
            return Err(super::RecoveryIdentityError::InvalidProcess);
        }
        let valid_boot = match platform {
            ProcessPlatform::Linux | ProcessPlatform::MacOs => {
                boot.len() == 36
                    && boot.chars().enumerate().all(|(index, ch)| {
                        if [8, 13, 18, 23].contains(&index) {
                            ch == '-'
                        } else {
                            ch.is_ascii_hexdigit()
                        }
                    })
            },
            ProcessPlatform::Windows => boot.parse::<u128>().is_ok_and(|value| value > 0),
        };
        if !valid_boot || !start.parse::<u128>().is_ok_and(|value| value > 0) {
            return Err(super::RecoveryIdentityError::InvalidProcess);
        }
        Ok(Self {
            platform,
            machine,
            boot,
            pid,
            start,
        })
    }
    /// Capturing platform.
    pub fn platform(&self) -> ProcessPlatform {
        self.platform
    }
    /// Stable machine token.
    pub fn machine(&self) -> &crate::ContentHash {
        &self.machine
    }
    /// Kernel boot token.
    pub fn boot(&self) -> &str {
        &self.boot
    }
    /// Host process ID.
    pub fn pid(&self) -> u32 {
        self.pid
    }
    /// Precise process creation token, never PID alone.
    pub fn start(&self) -> &str {
        &self.start
    }
}

/// What the host can prove about descendants of an observed container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterCoverage {
    /// Recorded group only; unrestricted descendants may escape it.
    GroupOnly,
    /// A host-established containment contract covers this writer domain.
    CoveredDomain,
    /// A writer/effect may exist without complete local containment evidence.
    Incomplete,
}
/// Local group identity accompanying a validated leader identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawContainer")]
pub struct WriterContainer {
    identity: ProcessIdentity,
    group: u32,
    coverage: WriterCoverage,
}
#[derive(Deserialize)]
struct RawContainer {
    identity: ProcessIdentity,
    group: u32,
    coverage: WriterCoverage,
}
impl TryFrom<RawContainer> for WriterContainer {
    type Error = super::RecoveryIdentityError;
    fn try_from(raw: RawContainer) -> Result<Self, Self::Error> {
        Self::new(raw.identity, raw.group, raw.coverage)
    }
}
impl WriterContainer {
    /// Construct a nonzero isolated group identity recorded by a trusted host.
    pub fn new(
        identity: ProcessIdentity,
        group: u32,
        coverage: WriterCoverage,
    ) -> Result<Self, super::RecoveryIdentityError> {
        if group == 0 {
            return Err(super::RecoveryIdentityError::InvalidProcess);
        }
        Ok(Self {
            identity,
            group,
            coverage,
        })
    }
    /// Captured group leader.
    pub fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }
    /// Recorded owned process group.
    pub fn group(&self) -> u32 {
        self.group
    }
    /// Descendant/effect coverage contract.
    pub fn coverage(&self) -> WriterCoverage {
        self.coverage
    }
}
/// Evidence result; probe errors never imply death.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriterLiveness {
    Alive,
    Gone,
    Unknown(String),
}
/// The executable writer whose dispatch was preceded by an ownership intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionWriterKind {
    Provider,
    HostTool { call_id: String },
}
/// Durable pre-dispatch coverage intent. An absent establishment remains unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionWriterIntent {
    pub writer: ExecutionWriterId,
    pub invocation: StageInvocationId,
    pub kind: ExecutionWriterKind,
    pub owner: Option<ProcessIdentity>,
    /// False for arbitrary external effects which local process disappearance cannot settle.
    pub local_effects: bool,
}
/// Folded immutable intent with subsequent establishment/cleanup observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionWriterRecord {
    pub intent: ExecutionWriterIntent,
    pub container: Option<WriterContainer>,
    pub cleanup_confirmed: bool,
    pub conflicting_observation: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_boot_identity_cannot_be_a_mutable_wall_clock_timestamp() {
        let machine = crate::ContentHash::compute(b"fixed-machine");
        assert!(
            ProcessIdentity::new(
                ProcessPlatform::MacOs,
                machine,
                "1727671234567890".into(),
                42,
                "123456789".into()
            )
            .is_err()
        );
        let fixed = ProcessIdentity::new(
            ProcessPlatform::MacOs,
            machine,
            "7ED1F44C-5737-48C4-B681-28790FA3E314".into(),
            42,
            "123456789".into(),
        )
        .unwrap();
        let encoded = serde_json::to_value(&fixed).unwrap();
        assert_eq!(
            serde_json::from_value::<ProcessIdentity>(encoded).unwrap(),
            fixed
        );
        let mut untrusted = serde_json::to_value(&fixed).unwrap();
        untrusted["boot"] = serde_json::json!("1727671234567890");
        assert!(serde_json::from_value::<ProcessIdentity>(untrusted).is_err());
    }
}
/// Connection-local writer observation returned atomically with provider identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawObservation")]
pub struct ExecutionWriterObservation {
    writer: ExecutionWriterId,
    container: Option<WriterContainer>,
}
#[derive(Deserialize)]
struct RawObservation {
    writer: ExecutionWriterId,
    container: Option<WriterContainer>,
}
impl TryFrom<RawObservation> for ExecutionWriterObservation {
    type Error = super::RecoveryIdentityError;
    fn try_from(raw: RawObservation) -> Result<Self, Self::Error> {
        Self::new(raw.writer, raw.container)
    }
}
impl ExecutionWriterObservation {
    /// Validate the pre-dispatch writer identity; unavailable process evidence stays explicit.
    pub fn new(
        writer: ExecutionWriterId,
        container: Option<WriterContainer>,
    ) -> Result<Self, super::RecoveryIdentityError> {
        if writer.as_ulid() == ulid::Ulid::nil() {
            return Err(super::RecoveryIdentityError::InvalidProcess);
        }
        Ok(Self { writer, container })
    }
    /// Writer whose intent preceded dispatch.
    pub fn writer(&self) -> ExecutionWriterId {
        self.writer
    }
    /// Host process evidence, or None when capture was unavailable.
    pub fn container(&self) -> Option<&WriterContainer> {
        self.container.as_ref()
    }
}
