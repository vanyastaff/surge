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
/// Native named Job Object evidence bound to one precise Windows process identity.
///
/// The name is an unguessable host-generated identifier, not authority by itself.
/// Recovery must reopen it with owner-ACL verification and validate membership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawWindowsJob")]
pub struct WindowsJobIdentity {
    name: String,
    leader: ProcessIdentity,
}

#[derive(Deserialize)]
struct RawWindowsJob {
    name: String,
    leader: ProcessIdentity,
}

impl TryFrom<RawWindowsJob> for WindowsJobIdentity {
    type Error = super::RecoveryIdentityError;
    fn try_from(raw: RawWindowsJob) -> Result<Self, Self::Error> {
        Self::new(raw.name, raw.leader)
    }
}

impl WindowsJobIdentity {
    /// Validate a native host-generated name and exact Windows leader identity.
    pub fn new(
        name: String,
        leader: ProcessIdentity,
    ) -> Result<Self, super::RecoveryIdentityError> {
        let valid_nonce = name.strip_prefix("Global\\SurgeMcp-").is_some_and(|nonce| {
            nonce.len() == 64
                && nonce
                    .bytes()
                    .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
                && nonce.bytes().any(|ch| ch != b'0')
        });
        if !valid_nonce
            || leader.platform() != ProcessPlatform::Windows
            || leader.start().parse::<u64>().is_err()
        {
            return Err(super::RecoveryIdentityError::InvalidProcess);
        }
        Ok(Self { name, leader })
    }

    /// Exact native object name; validated again by the I/O owner when reopened.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Precise process identity whose membership must match before termination.
    pub fn leader(&self) -> &ProcessIdentity {
        &self.leader
    }
}

/// Local group identity accompanying a validated leader identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawContainer")]
pub struct WriterContainer {
    identity: ProcessIdentity,
    group: u32,
    coverage: WriterCoverage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    windows_job: Option<WindowsJobIdentity>,
}
#[derive(Deserialize)]
struct RawContainer {
    identity: ProcessIdentity,
    group: u32,
    coverage: WriterCoverage,
    #[serde(default)]
    windows_job: Option<WindowsJobIdentity>,
}
impl TryFrom<RawContainer> for WriterContainer {
    type Error = super::RecoveryIdentityError;
    fn try_from(raw: RawContainer) -> Result<Self, Self::Error> {
        match raw.windows_job {
            Some(job) => Self::with_windows_job(raw.identity, raw.group, raw.coverage, job),
            None => Self::new(raw.identity, raw.group, raw.coverage),
        }
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
            windows_job: None,
        })
    }
    /// Bind actual native Job Object evidence to its exact captured Windows leader.
    pub fn with_windows_job(
        identity: ProcessIdentity,
        group: u32,
        coverage: WriterCoverage,
        job: WindowsJobIdentity,
    ) -> Result<Self, super::RecoveryIdentityError> {
        if identity.platform() != ProcessPlatform::Windows
            || group != identity.pid()
            || job.leader() != &identity
        {
            return Err(super::RecoveryIdentityError::InvalidProcess);
        }
        let mut container = Self::new(identity, group, coverage)?;
        container.windows_job = Some(job);
        Ok(container)
    }

    /// Named Windows job evidence, absent for legacy records and Unix containers.
    pub fn windows_job(&self) -> Option<&WindowsJobIdentity> {
        self.windows_job.as_ref()
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
    /// Best-effort group cleanup (ADR-0021); never implies `cleanup_confirmed`.
    #[serde(default)]
    pub group_stopped: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_identity() -> ProcessIdentity {
        ProcessIdentity::new(
            ProcessPlatform::Windows,
            crate::ContentHash::compute(b"fixed-windows-machine"),
            "123456".into(),
            42,
            "987654321".into(),
        )
        .unwrap()
    }

    fn job_name() -> String {
        format!("Global\\SurgeMcp-{}", "a".repeat(64))
    }

    #[test]
    fn windows_job_metadata_roundtrips_with_exact_leader() {
        let identity = windows_identity();
        let job = WindowsJobIdentity::new(job_name(), identity.clone()).unwrap();
        let container =
            WriterContainer::with_windows_job(identity.clone(), 42, WriterCoverage::GroupOnly, job)
                .unwrap();
        assert_eq!(container.windows_job().unwrap().leader(), &identity);
        assert_eq!(container.windows_job().unwrap().name(), job_name());
        let json = serde_json::to_value(&container).unwrap();
        assert_eq!(
            serde_json::from_value::<WriterContainer>(json).unwrap(),
            container
        );
    }

    #[test]
    fn windows_job_metadata_rejects_forged_names_and_leaders() {
        let identity = windows_identity();
        for name in [
            String::new(),
            "Local\\SurgeMcp-any".into(),
            format!("Local\\SurgeMcp-{}", "a".repeat(64)),
            format!("Global\\SurgeMcp-{}", "0".repeat(64)),
            format!("Global\\SurgeMcp-{}", "A".repeat(64)),
            format!("Global\\SurgeMcp-{}", "a".repeat(63)),
        ] {
            assert!(WindowsJobIdentity::new(name, identity.clone()).is_err());
        }
        let job = WindowsJobIdentity::new(job_name(), identity.clone()).unwrap();
        let mut json = serde_json::to_value(
            WriterContainer::with_windows_job(identity, 42, WriterCoverage::GroupOnly, job)
                .unwrap(),
        )
        .unwrap();
        json["windows_job"]["leader"]["start"] = serde_json::json!("111");
        assert!(serde_json::from_value::<WriterContainer>(json.clone()).is_err());
        json["windows_job"]["leader"]["start"] = serde_json::json!("987654321");
        json["windows_job"]["name"] = serde_json::json!("Local\\unrelated");
        assert!(serde_json::from_value::<WriterContainer>(json).is_err());
    }

    #[test]
    fn legacy_windows_container_remains_deserializable_without_job_authority() {
        let container =
            WriterContainer::new(windows_identity(), 42, WriterCoverage::GroupOnly).unwrap();
        let json = serde_json::to_value(&container).unwrap();
        assert!(json.get("windows_job").is_none());
        let decoded = serde_json::from_value::<WriterContainer>(json).unwrap();
        assert!(decoded.windows_job().is_none());
    }

    #[test]
    fn unix_container_rejects_windows_job_metadata() {
        let windows = windows_identity();
        let unix = ProcessIdentity::new(
            ProcessPlatform::Linux,
            *windows.machine(),
            "7ED1F44C-5737-48C4-B681-28790FA3E314".into(),
            42,
            "123".into(),
        )
        .unwrap();
        assert!(WindowsJobIdentity::new(job_name(), unix.clone()).is_err());
        let job = WindowsJobIdentity::new(job_name(), windows).unwrap();
        assert!(
            WriterContainer::with_windows_job(unix.clone(), 42, WriterCoverage::GroupOnly, job)
                .is_err()
        );
        let mut json = serde_json::to_value(
            WriterContainer::new(unix, 42, WriterCoverage::GroupOnly).unwrap(),
        )
        .unwrap();
        json["windows_job"] = serde_json::json!({"name": job_name(), "leader": windows_identity()});
        assert!(serde_json::from_value::<WriterContainer>(json).is_err());
    }

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
