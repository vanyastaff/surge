//! Pure public guardian binding data; validation grants no live process authority.
//! Native authentication, executable verification and containment belong to owners.

use super::process::WriterCoverage;
use crate::{ContentHash, id::ExecutionWriterId};
use serde::{Deserialize, Serialize};

/// Static validation categories, without echoing untrusted binding values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GuardianIdentityError {
    /// Installation identifier is not canonical or is all zero.
    #[error("invalid guardian installation identifier")]
    InvalidInstallation,
    /// Occurrence wire spelling is not a canonical bare ULID.
    #[error("noncanonical guardian occurrence identifier")]
    NoncanonicalOccurrence,
    /// Executable digest wire spelling is not canonical SHA-256.
    #[error("noncanonical guardian executable hash")]
    NoncanonicalHash,
    /// Container discriminator is unsupported.
    #[error("unsupported guardian container kind")]
    UnsupportedKind,
    /// Container format version is unsupported.
    #[error("unsupported guardian container version")]
    UnsupportedVersion,
    /// Guardian protocol version is unsupported.
    #[error("unsupported guardian protocol version")]
    UnsupportedProtocol,
    /// PID or creation FILETIME is zero.
    #[error("invalid guardian process identity")]
    InvalidProcess,
    /// Nil IDs cannot identify ownership occurrences.
    #[error("invalid guardian ownership occurrence")]
    InvalidOccurrence,
    /// Guardian and child cannot share a PID.
    #[error("guardian and child process identities collide")]
    IdentityCollision,
    /// Public endpoint differs from the exact derived local label.
    #[error("guardian endpoint binding mismatch")]
    EndpointMismatch,
    /// This record cannot grant stronger containment coverage.
    #[error("invalid guardian container coverage")]
    InvalidCoverage,
}

/// Public installation binding, not a secret or authenticated capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GuardianInstallationId([u8; 32]);
impl GuardianInstallationId {
    /// Validate existing bytes; generation and private persistence are external.
    pub fn new(bytes: [u8; 32]) -> Result<Self, GuardianIdentityError> {
        if bytes == [0; 32] {
            return Err(GuardianIdentityError::InvalidInstallation);
        }
        Ok(Self(bytes))
    }
    /// Original installation bytes, without granting authority.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl TryFrom<String> for GuardianInstallationId {
    type Error = GuardianIdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(GuardianIdentityError::InvalidInstallation);
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(value, &mut bytes)
            .map_err(|_| GuardianIdentityError::InvalidInstallation)?;
        Self::new(bytes)
    }
}
impl From<GuardianInstallationId> for String {
    fn from(value: GuardianInstallationId) -> Self {
        hex::encode(value.0)
    }
}

/// Frozen public installation/executable/protocol binding; runtime must authenticate it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAuthority")]
pub struct GuardianAuthority {
    installation_id: GuardianInstallationId,
    executable_hash: ContentHash,
    protocol_version: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAuthority {
    installation_id: GuardianInstallationId,
    executable_hash: String,
    protocol_version: u32,
}
impl GuardianAuthority {
    /// Bind typed data under the sole supported protocol, version one.
    pub fn new(
        installation_id: GuardianInstallationId,
        executable_hash: ContentHash,
        protocol_version: u32,
    ) -> Result<Self, GuardianIdentityError> {
        if protocol_version != 1 {
            return Err(GuardianIdentityError::UnsupportedProtocol);
        }
        Ok(Self {
            installation_id,
            executable_hash,
            protocol_version,
        })
    }
    /// Public installation binding.
    #[must_use]
    pub fn installation_id(&self) -> &GuardianInstallationId {
        &self.installation_id
    }
    /// Frozen executable digest; its verification belongs to the runtime owner.
    #[must_use]
    pub fn executable_hash(&self) -> &ContentHash {
        &self.executable_hash
    }
    /// Exact supported protocol version.
    #[must_use]
    pub fn protocol_version(&self) -> u32 {
        self.protocol_version
    }
}
impl TryFrom<RawAuthority> for GuardianAuthority {
    type Error = GuardianIdentityError;
    fn try_from(raw: RawAuthority) -> Result<Self, Self::Error> {
        let hash: ContentHash = raw
            .executable_hash
            .parse()
            .map_err(|_| GuardianIdentityError::NoncanonicalHash)?;
        if hash.to_string() != raw.executable_hash {
            return Err(GuardianIdentityError::NoncanonicalHash);
        }
        Self::new(raw.installation_id, hash, raw.protocol_version)
    }
}

/// Positive Windows PID and exact creation FILETIME; no liveness claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawProcess")]
pub struct WindowsGuardianProcessIdentity {
    pid: u32,
    creation_filetime: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProcess {
    pid: u32,
    creation_filetime: u64,
}
impl WindowsGuardianProcessIdentity {
    /// Validate existing native process identity values without observing a process.
    pub fn new(pid: u32, creation_filetime: u64) -> Result<Self, GuardianIdentityError> {
        if pid == 0 || creation_filetime == 0 {
            return Err(GuardianIdentityError::InvalidProcess);
        }
        Ok(Self {
            pid,
            creation_filetime,
        })
    }
    /// Positive native process identifier.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }
    /// Exact creation FILETIME; never an uptime or boot token.
    #[must_use]
    pub fn creation_filetime(&self) -> u64 {
        self.creation_filetime
    }
}
impl TryFrom<RawProcess> for WindowsGuardianProcessIdentity {
    type Error = GuardianIdentityError;
    fn try_from(raw: RawProcess) -> Result<Self, Self::Error> {
        Self::new(raw.pid, raw.creation_filetime)
    }
}

/// Immutable public binding of one guardian, child and ownership occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawIdentity")]
pub struct WindowsGuardianIdentity {
    authority: GuardianAuthority,
    occurrence: ExecutionWriterId,
    guardian: WindowsGuardianProcessIdentity,
    child: WindowsGuardianProcessIdentity,
    endpoint: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIdentity {
    authority: GuardianAuthority,
    occurrence: String,
    guardian: WindowsGuardianProcessIdentity,
    child: WindowsGuardianProcessIdentity,
    endpoint: String,
}
impl WindowsGuardianIdentity {
    /// Derive the exact 106-byte ASCII local label from validated binding data.
    pub fn new(
        authority: GuardianAuthority,
        occurrence: ExecutionWriterId,
        guardian: WindowsGuardianProcessIdentity,
        child: WindowsGuardianProcessIdentity,
    ) -> Result<Self, GuardianIdentityError> {
        if occurrence == ExecutionWriterId::nil() {
            return Err(GuardianIdentityError::InvalidOccurrence);
        }
        if guardian.pid == child.pid {
            return Err(GuardianIdentityError::IdentityCollision);
        }
        let endpoint = format!(
            "surge-guardian-{}-{}",
            hex::encode(authority.installation_id.0),
            occurrence.as_ulid().to_string().to_ascii_lowercase()
        );
        Ok(Self {
            authority,
            occurrence,
            guardian,
            child,
            endpoint,
        })
    }
    /// Public authority binding, which still requires live authentication.
    #[must_use]
    pub fn authority(&self) -> &GuardianAuthority {
        &self.authority
    }
    /// Exact nonnil ownership occurrence.
    #[must_use]
    pub fn occurrence(&self) -> ExecutionWriterId {
        self.occurrence
    }
    /// Recorded guardian identity, without proving its present existence.
    #[must_use]
    pub fn guardian(&self) -> &WindowsGuardianProcessIdentity {
        &self.guardian
    }
    /// Recorded child identity, without proving its present existence.
    #[must_use]
    pub fn child(&self) -> &WindowsGuardianProcessIdentity {
        &self.child
    }
    /// Public local label, never a path, secret or remote pipe locator.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
impl TryFrom<RawIdentity> for WindowsGuardianIdentity {
    type Error = GuardianIdentityError;
    fn try_from(raw: RawIdentity) -> Result<Self, Self::Error> {
        let occurrence: ExecutionWriterId = raw
            .occurrence
            .parse()
            .map_err(|_| GuardianIdentityError::NoncanonicalOccurrence)?;
        if occurrence.as_ulid().to_string() != raw.occurrence {
            return Err(GuardianIdentityError::NoncanonicalOccurrence);
        }
        let identity = Self::new(raw.authority, occurrence, raw.guardian, raw.child)?;
        if raw.endpoint != identity.endpoint {
            return Err(GuardianIdentityError::EndpointMismatch);
        }
        Ok(identity)
    }
}

/// Version-one public container record; coverage is permanently GroupOnly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawContainer")]
pub struct WindowsGuardianContainer {
    kind: String,
    version: u32,
    identity: WindowsGuardianIdentity,
    coverage: WriterCoverage,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContainer {
    kind: String,
    version: u32,
    identity: WindowsGuardianIdentity,
    coverage: WriterCoverage,
}
impl WindowsGuardianContainer {
    /// Wrap validated public data; grants neither containment nor live capability.
    #[must_use]
    pub fn new(identity: WindowsGuardianIdentity) -> Self {
        Self {
            kind: "windows_guardian".into(),
            version: 1,
            identity,
            coverage: WriterCoverage::GroupOnly,
        }
    }
    /// Exact supported format version.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }
    /// Immutable public identity data.
    #[must_use]
    pub fn identity(&self) -> &WindowsGuardianIdentity {
        &self.identity
    }
    /// GroupOnly does not prove all descendants are contained.
    #[must_use]
    pub fn coverage(&self) -> WriterCoverage {
        self.coverage
    }
}
impl TryFrom<RawContainer> for WindowsGuardianContainer {
    type Error = GuardianIdentityError;
    fn try_from(raw: RawContainer) -> Result<Self, Self::Error> {
        if raw.kind != "windows_guardian" {
            return Err(GuardianIdentityError::UnsupportedKind);
        }
        if raw.version != 1 {
            return Err(GuardianIdentityError::UnsupportedVersion);
        }
        if raw.coverage != WriterCoverage::GroupOnly {
            return Err(GuardianIdentityError::InvalidCoverage);
        }
        Ok(Self::new(raw.identity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentHash, id::ExecutionWriterId};
    use serde_json::{Value, json};

    const FIXTURE: &str = r#"{
      "kind":"windows_guardian","version":1,"coverage":"group_only",
      "identity":{
        "authority":{
          "installation_id":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
          "executable_hash":"sha256:0000000000000000000000000000000000000000000000000000000000000000",
          "protocol_version":1
        },
        "occurrence":"01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "guardian":{"pid":100,"creation_filetime":123},
        "child":{"pid":101,"creation_filetime":124},
        "endpoint":"surge-guardian-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef-01arz3ndektsv4rrffq69g5fav"
      }
    }"#;

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).unwrap()
    }
    fn decoded() -> WindowsGuardianContainer {
        serde_json::from_str(FIXTURE).unwrap()
    }
    fn reject(pointer: &str, bad: Value) {
        let mut wire = fixture();
        *wire.pointer_mut(pointer).unwrap() = bad;
        assert!(
            serde_json::from_value::<WindowsGuardianContainer>(wire).is_err(),
            "accepted invalid {pointer}"
        );
    }

    #[test]
    fn literal_v1_roundtrip_and_named_getters() {
        let container = decoded();
        assert_eq!(serde_json::to_value(&container).unwrap(), fixture());
        assert_eq!(container.version(), 1);
        assert_eq!(
            container.coverage(),
            super::super::process::WriterCoverage::GroupOnly
        );
        let identity = container.identity();
        assert_eq!(identity.endpoint().len(), 106);
        assert_eq!(identity.guardian().pid(), 100);
        assert_eq!(identity.child().creation_filetime(), 124);
        assert_eq!(identity.authority().protocol_version(), 1);
        assert_eq!(
            identity.authority().executable_hash(),
            &ContentHash::from_bytes([0; 32])
        );
        assert_eq!(
            identity.occurrence().as_ulid().to_string(),
            "01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            serde_json::to_value(identity.authority().installation_id()).unwrap(),
            fixture()["identity"]["authority"]["installation_id"]
        );
    }

    #[test]
    fn constructors_preserve_invariants_and_derive_endpoint() {
        assert!(GuardianInstallationId::new([0; 32]).is_err());
        let mut bytes = [0; 32];
        bytes[31] = 1;
        let installation = GuardianInstallationId::new(bytes).unwrap();
        assert_eq!(installation.as_bytes(), &bytes);
        assert!(
            GuardianAuthority::new(installation.clone(), ContentHash::from_bytes([0; 32]), 2)
                .is_err()
        );
        let authority =
            GuardianAuthority::new(installation, ContentHash::from_bytes([0; 32]), 1).unwrap();
        assert!(WindowsGuardianProcessIdentity::new(0, 1).is_err());
        assert!(WindowsGuardianProcessIdentity::new(1, 0).is_err());
        let guardian = WindowsGuardianProcessIdentity::new(100, 123).unwrap();
        let child = WindowsGuardianProcessIdentity::new(101, 124).unwrap();
        assert!(
            WindowsGuardianIdentity::new(
                authority.clone(),
                ExecutionWriterId::nil(),
                guardian.clone(),
                child.clone()
            )
            .is_err()
        );
        assert!(
            WindowsGuardianIdentity::new(
                authority.clone(),
                "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap(),
                guardian.clone(),
                guardian.clone()
            )
            .is_err()
        );
        let identity = WindowsGuardianIdentity::new(
            authority,
            "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().unwrap(),
            guardian,
            child,
        )
        .unwrap();
        assert_eq!(
            identity.endpoint(),
            "surge-guardian-0000000000000000000000000000000000000000000000000000000000000001-01arz3ndektsv4rrffq69g5fav"
        );
        assert_eq!(
            WindowsGuardianContainer::new(identity).coverage(),
            super::super::process::WriterCoverage::GroupOnly
        );
    }

    #[test]
    fn installation_wire_requires_exact_nonzero_lowercase_hex() {
        for bad in [
            "",
            "00",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
            " 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "g123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ] {
            reject("/identity/authority/installation_id", json!(bad));
        }
    }

    #[test]
    fn executable_hash_wire_rejects_shared_parser_aliases() {
        for bad in [
            "",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "SHA256:0000000000000000000000000000000000000000000000000000000000000000",
            "sha256:00",
        ] {
            reject("/identity/authority/executable_hash", json!(bad));
        }
    }

    #[test]
    fn versions_kind_and_coverage_are_frozen() {
        for pointer in ["/version", "/identity/authority/protocol_version"] {
            for bad in [
                json!(0),
                json!(2),
                json!(256),
                json!(-1),
                json!(1.0),
                json!("1"),
            ] {
                reject(pointer, bad);
            }
        }
        for bad in ["windows", "windows_guardian_v2", "", "Windows_Guardian"] {
            reject("/kind", json!(bad));
        }
        for bad in ["covered_domain", "incomplete", "GroupOnly", ""] {
            reject("/coverage", json!(bad));
        }
    }

    #[test]
    fn occurrence_and_endpoint_require_exact_binding() {
        for bad in [
            "00000000000000000000000000",
            "01arz3ndektsv4rrffq69g5fav",
            "writer-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "81ARZ3NDEKTSV4RRFFQ69G5FAV",
            "",
        ] {
            reject("/identity/occurrence", json!(bad));
        }
        for bad in [
            "",
            "\\remote\\pipe\\guardian",
            "surge-guardian-evil\n",
            "surge-guardian-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef-01arz3ndektsv4rrffq69g5faw",
        ] {
            reject("/identity/endpoint", json!(bad));
        }
        reject("/identity/endpoint", json!("x".repeat(129)));
        reject(
            "/identity/endpoint",
            json!(decoded().identity().endpoint().to_uppercase()),
        );
        reject("/identity/occurrence", json!("01ARZ3NDEKTSV4RRFFQ69G5FAW"));
    }

    #[test]
    fn numeric_process_wire_bounds_and_pid_collision() {
        for process in ["guardian", "child"] {
            for bad in [
                json!(0),
                json!(-1),
                json!(1.5),
                json!(4294967296_u64),
                json!("100"),
            ] {
                reject(&format!("/identity/{process}/pid"), bad);
            }
            for bad in [
                json!(0),
                json!(-1),
                json!(1.5),
                json!("18446744073709551616"),
            ] {
                reject(&format!("/identity/{process}/creation_filetime"), bad);
            }
        }
        reject("/identity/child/pid", json!(100));
        let mut wire = fixture();
        wire["identity"]["guardian"]["creation_filetime"] = json!(u64::MAX);
        wire["identity"]["guardian"]["pid"] = json!(u32::MAX);
        assert!(serde_json::from_value::<WindowsGuardianContainer>(wire).is_ok());
        let overflow = FIXTURE.replace(
            "\"creation_filetime\":123",
            "\"creation_filetime\":18446744073709551616",
        );
        assert!(serde_json::from_str::<WindowsGuardianContainer>(&overflow).is_err());
    }

    #[test]
    fn every_dto_rejects_unknown_fields_and_legacy_groups() {
        for pointer in [
            "",
            "/identity",
            "/identity/authority",
            "/identity/guardian",
            "/identity/child",
        ] {
            let mut wire = fixture();
            wire.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), json!(true));
            assert!(serde_json::from_value::<WindowsGuardianContainer>(wire).is_err());
        }
        let legacy = r#"{
            "identity": {
                "platform": "windows",
                "machine": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                "boot": "1",
                "pid": 100,
                "start": "123"
            },
            "group": 100,
            "coverage": "group_only"
        }"#;
        assert!(serde_json::from_str::<super::super::process::WriterContainer>(legacy).is_ok());
        assert!(serde_json::from_str::<WindowsGuardianContainer>(legacy).is_err());
    }
}
