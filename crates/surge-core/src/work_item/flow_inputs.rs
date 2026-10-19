//! Public owned-flow startup inputs; serialization grants no hydration authority.
use super::{WorkItemBinding, WorkItemWorkspace};
use crate::id::WorkItemOperationId;
use crate::mcp_config::McpServerRef;
use crate::sandbox::SandboxMode;
use crate::{ContentHash, RunId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::Duration;

/// Malformed public projection, without echoing submitted values.
#[derive(Debug, thiserror::Error)]
pub enum OwnedFlowManifestError {
    #[error("invalid owned-flow object reference")]
    ObjectReference,
    #[error("invalid owned-flow HMAC tag")]
    AuthenticationTag,
    #[error("invalid owned-flow manifest identity or schema")]
    Identity,
    #[error("invalid owned-flow workspace binding")]
    Workspace,
    #[error("invalid owned-flow MCP public metadata")]
    ServerMetadata,
    #[error("invalid owned-flow ordered MCP snapshot")]
    Snapshot,
    #[error("owned-flow request identity does not match its input selection")]
    RequestIdentity,
}

/// HMAC-SHA256 output; a tag value alone is not a validated host capability.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HmacSha256Tag([u8; 32]);
impl HmacSha256Tag {
    /// Wrap actual HMAC output for a public authenticated reference.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    /// Exact authentication output, without a misleading SHA256 content label.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl std::fmt::Debug for HmacSha256Tag {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HmacSha256Tag([redacted])")
    }
}
impl TryFrom<String> for HmacSha256Tag {
    type Error = OwnedFlowManifestError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let suffix = value
            .strip_prefix("hmac-sha256:")
            .ok_or(OwnedFlowManifestError::AuthenticationTag)?;
        if !is_hex_identity(suffix) {
            return Err(OwnedFlowManifestError::AuthenticationTag);
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(suffix, &mut bytes)
            .map_err(|_| OwnedFlowManifestError::AuthenticationTag)?;
        Ok(Self(bytes))
    }
}
impl From<HmacSha256Tag> for String {
    fn from(tag: HmacSha256Tag) -> Self {
        format!("hmac-sha256:{}", hex::encode(tag.0))
    }
}

/// Random operation-owned object locator, excluding filesystem path syntax.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OwnedFlowObjectRef(String);
impl OwnedFlowObjectRef {
    /// Validate the lowercase encoding of 32 random reference bytes.
    ///
    /// # Errors
    /// Rejects any reference outside the exact opaque-reference format.
    pub fn new(value: String) -> Result<Self, OwnedFlowManifestError> {
        if !is_hex_identity(&value) {
            return Err(OwnedFlowManifestError::ObjectReference);
        }
        Ok(Self(value))
    }
    /// Opaque object name, suitable for a host-derived filename suffix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for OwnedFlowObjectRef {
    type Error = OwnedFlowManifestError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<OwnedFlowObjectRef> for String {
    fn from(reference: OwnedFlowObjectRef) -> Self {
        reference.0
    }
}
fn is_hex_identity(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// First-body identity preserves the public-request versus permitted-secret distinction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnedFlowRequestIdentity {
    /// Digest of a request with no private transport payload in its body.
    PlainPublic { digest: ContentHash },
    /// Host-authenticated identity when explicit transport bytes occur in the body.
    Hmac { tag: HmacSha256Tag },
}

/// Original explicit selection remains distinct from resolved host defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedFlowMcpSelection {
    /// Caller explicitly chose the server list, including an empty list.
    Explicit,
    /// Host defaults were resolved exactly once.
    HostDefault,
}

/// Exact public metadata and opaque reference for one ordered frozen MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawMcpManifestEntry")]
pub struct OwnedFlowMcpManifestEntry {
    index: u32,
    name: String,
    allowed_tools: Option<Vec<String>>,
    #[serde(with = "humantime_serde")]
    call_timeout: Duration,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "humantime_serde"
    )]
    startup_timeout: Option<Duration>,
    restart_on_crash: bool,
    sandbox: Option<SandboxMode>,
    object_ref: OwnedFlowObjectRef,
    object_auth_tag: HmacSha256Tag,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMcpManifestEntry {
    index: u32,
    name: String,
    allowed_tools: Option<Vec<String>>,
    #[serde(with = "humantime_serde")]
    call_timeout: Duration,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "humantime_serde"
    )]
    startup_timeout: Option<Duration>,
    restart_on_crash: bool,
    sandbox: Option<SandboxMode>,
    object_ref: OwnedFlowObjectRef,
    object_auth_tag: HmacSha256Tag,
}
impl TryFrom<RawMcpManifestEntry> for OwnedFlowMcpManifestEntry {
    type Error = OwnedFlowManifestError;
    fn try_from(raw: RawMcpManifestEntry) -> Result<Self, Self::Error> {
        let entry = Self {
            index: raw.index,
            name: raw.name,
            allowed_tools: raw.allowed_tools,
            call_timeout: raw.call_timeout,
            startup_timeout: raw.startup_timeout,
            restart_on_crash: raw.restart_on_crash,
            sandbox: raw.sandbox,
            object_ref: raw.object_ref,
            object_auth_tag: raw.object_auth_tag,
        };
        entry.validate()?;
        Ok(entry)
    }
}
impl OwnedFlowMcpManifestEntry {
    /// Project frozen public settings without copying command, arguments or environment.
    ///
    /// # Errors
    /// Rejects out-of-bound names, tool metadata or server indices.
    pub fn new(
        index: u32,
        server: &McpServerRef,
        object_ref: OwnedFlowObjectRef,
        object_auth_tag: HmacSha256Tag,
    ) -> Result<Self, OwnedFlowManifestError> {
        let entry = Self {
            index,
            name: server.name.clone(),
            allowed_tools: server.allowed_tools.clone(),
            call_timeout: server.call_timeout,
            startup_timeout: server.startup_timeout,
            restart_on_crash: server.restart_on_crash,
            sandbox: server.sandbox,
            object_ref,
            object_auth_tag,
        };
        entry.validate()?;
        Ok(entry)
    }
    fn validate(&self) -> Result<(), OwnedFlowManifestError> {
        if self.index >= 128
            || !bounded_text(&self.name, 256)
            || self.allowed_tools.as_ref().is_some_and(|tools| {
                tools.len() > 256 || tools.iter().any(|tool| !bounded_text(tool, 8192))
            })
        {
            return Err(OwnedFlowManifestError::ServerMetadata);
        }
        Ok(())
    }
    /// Zero-based ordered position in the frozen whole-list envelope.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }
    /// Accepted server name, independent of its index.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Exact whitelist semantics, retaining `None` versus explicit empty.
    #[must_use]
    pub fn allowed_tools(&self) -> Option<&[String]> {
        self.allowed_tools.as_deref()
    }
    /// Exact per-call timeout.
    #[must_use]
    pub fn call_timeout(&self) -> Duration {
        self.call_timeout
    }
    /// Exact explicit startup deadline; `None` means the host default
    /// (`McpServerRef::effective_startup_timeout`). Absent from the encoded
    /// entry when `None`, so pre-existing snapshots keep their exact bytes.
    #[must_use]
    pub fn startup_timeout(&self) -> Option<Duration> {
        self.startup_timeout
    }
    /// Exact child restart policy.
    #[must_use]
    pub fn restart_on_crash(&self) -> bool {
        self.restart_on_crash
    }
    /// Exact per-server sandbox override.
    #[must_use]
    pub fn sandbox(&self) -> Option<&SandboxMode> {
        self.sandbox.as_ref()
    }
    /// Opaque immutable transport object reference.
    #[must_use]
    pub fn object_ref(&self) -> &OwnedFlowObjectRef {
        &self.object_ref
    }
    /// Host-generated object authentication tag, not itself a capability.
    #[must_use]
    pub fn object_auth_tag(&self) -> &HmacSha256Tag {
        &self.object_auth_tag
    }
}

/// Host-stored effective MCP inputs; this wire projection cannot authorize hydration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "RawFrozenMcp")]
pub enum FrozenOwnedFlowMcp {
    /// Host produced a genuine empty list; no private object or key exists for it.
    Empty { selection: OwnedFlowMcpSelection },
    /// Nonempty inputs require a verified whole-list envelope and private objects.
    Private {
        selection: OwnedFlowMcpSelection,
        entries: Vec<OwnedFlowMcpManifestEntry>,
        envelope_ref: OwnedFlowObjectRef,
        envelope_auth_tag: HmacSha256Tag,
    },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RawFrozenMcp {
    Empty {
        selection: OwnedFlowMcpSelection,
    },
    Private {
        selection: OwnedFlowMcpSelection,
        entries: Vec<OwnedFlowMcpManifestEntry>,
        envelope_ref: OwnedFlowObjectRef,
        envelope_auth_tag: HmacSha256Tag,
    },
}
impl TryFrom<RawFrozenMcp> for FrozenOwnedFlowMcp {
    type Error = OwnedFlowManifestError;
    fn try_from(raw: RawFrozenMcp) -> Result<Self, Self::Error> {
        match raw {
            RawFrozenMcp::Empty { selection } => Ok(Self::empty(selection)),
            RawFrozenMcp::Private {
                selection,
                entries,
                envelope_ref,
                envelope_auth_tag,
            } => Self::private(selection, entries, envelope_ref, envelope_auth_tag),
        }
    }
}
impl FrozenOwnedFlowMcp {
    /// Represent the host's already resolved empty effective list.
    #[must_use]
    pub fn empty(selection: OwnedFlowMcpSelection) -> Self {
        Self::Empty { selection }
    }
    /// Validate an ordered public snapshot of nonempty private inputs.
    ///
    /// # Errors
    /// Rejects empty, oversized, reordered, repeated or malformed entries/references.
    pub fn private(
        selection: OwnedFlowMcpSelection,
        entries: Vec<OwnedFlowMcpManifestEntry>,
        envelope_ref: OwnedFlowObjectRef,
        envelope_auth_tag: HmacSha256Tag,
    ) -> Result<Self, OwnedFlowManifestError> {
        let snapshot = Self::Private {
            selection,
            entries,
            envelope_ref,
            envelope_auth_tag,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
    /// Original input-selection semantics.
    #[must_use]
    pub fn selection(&self) -> OwnedFlowMcpSelection {
        match self {
            Self::Empty { selection } | Self::Private { selection, .. } => *selection,
        }
    }
    fn validate(&self) -> Result<(), OwnedFlowManifestError> {
        let Self::Private {
            entries,
            envelope_ref,
            ..
        } = self
        else {
            return Ok(());
        };
        if entries.is_empty() || entries.len() > 128 {
            return Err(OwnedFlowManifestError::Snapshot);
        }
        let mut names = BTreeSet::new();
        let mut references = BTreeSet::new();
        for (index, entry) in entries.iter().enumerate() {
            entry.validate()?;
            if usize::try_from(entry.index).ok() != Some(index)
                || !names.insert(entry.name())
                || !references.insert(entry.object_ref())
                || entry.object_ref() == envelope_ref
            {
                return Err(OwnedFlowManifestError::Snapshot);
            }
        }
        Ok(())
    }
}

/// Immutable public startup snapshot bound to the real accepted operation and attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawInputsManifest")]
pub struct OwnedFlowInputsManifest {
    schema_version: u32,
    operation: WorkItemOperationId,
    request_identity: OwnedFlowRequestIdentity,
    binding: WorkItemBinding,
    run: RunId,
    workspace_owner: RunId,
    workspace: WorkItemWorkspace,
    mcp: FrozenOwnedFlowMcp,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInputsManifest {
    schema_version: u32,
    operation: WorkItemOperationId,
    request_identity: OwnedFlowRequestIdentity,
    binding: WorkItemBinding,
    run: RunId,
    workspace_owner: RunId,
    workspace: WorkItemWorkspace,
    mcp: FrozenOwnedFlowMcp,
}
impl TryFrom<RawInputsManifest> for OwnedFlowInputsManifest {
    type Error = OwnedFlowManifestError;
    fn try_from(raw: RawInputsManifest) -> Result<Self, Self::Error> {
        if raw.schema_version != 1 {
            return Err(OwnedFlowManifestError::Identity);
        }
        Self::new(
            raw.operation,
            raw.request_identity,
            raw.binding,
            raw.run,
            raw.workspace_owner,
            raw.workspace,
            raw.mcp,
        )
    }
}
impl OwnedFlowInputsManifest {
    /// Validate a host-created projection; actual host-store/HMAC authority is separate.
    ///
    /// # Errors
    /// Rejects malformed identities, workspaces, selections or ordered snapshots.
    pub fn new(
        operation: WorkItemOperationId,
        request_identity: OwnedFlowRequestIdentity,
        binding: WorkItemBinding,
        run: RunId,
        workspace_owner: RunId,
        workspace: WorkItemWorkspace,
        mcp: FrozenOwnedFlowMcp,
    ) -> Result<Self, OwnedFlowManifestError> {
        if operation == WorkItemOperationId::nil()
            || run == RunId::nil()
            || workspace_owner == RunId::nil()
            || binding.item == crate::id::WorkItemId::nil()
            || binding.revision == 0
            || binding.generation == 0
        {
            return Err(OwnedFlowManifestError::Identity);
        }
        if !workspace.repository.is_absolute()
            || !workspace.checkout.is_absolute()
            || !workspace.path.is_absolute()
            || !bounded_text(&workspace.ownership, 1024)
            || !bounded_text(&workspace.branch, 1024)
            || workspace.base_commit.len() != 40
            || !workspace
                .base_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(OwnedFlowManifestError::Workspace);
        }
        mcp.validate()?;
        if matches!(
            (&mcp, &request_identity),
            (
                FrozenOwnedFlowMcp::Empty { .. },
                OwnedFlowRequestIdentity::Hmac { .. }
            ) | (
                FrozenOwnedFlowMcp::Private {
                    selection: OwnedFlowMcpSelection::Explicit,
                    ..
                },
                OwnedFlowRequestIdentity::PlainPublic { .. }
            )
        ) {
            return Err(OwnedFlowManifestError::RequestIdentity);
        }
        Ok(Self {
            schema_version: 1,
            operation,
            request_identity,
            binding,
            run,
            workspace_owner,
            workspace,
            mcp,
        })
    }
    /// Public snapshot schema, independent of journal schema.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    /// Original operation identity.
    #[must_use]
    pub fn operation(&self) -> WorkItemOperationId {
        self.operation
    }
    /// Immutable first-body request identity.
    #[must_use]
    pub fn request_identity(&self) -> &OwnedFlowRequestIdentity {
        &self.request_identity
    }
    /// Exact accepted reservation binding.
    #[must_use]
    pub fn binding(&self) -> &WorkItemBinding {
        &self.binding
    }
    /// Host-allocated execution identity.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }
    /// Original Git creation-owner identity, distinct from current claim token.
    #[must_use]
    pub fn workspace_owner(&self) -> RunId {
        self.workspace_owner
    }
    /// Immutable retained workspace association.
    #[must_use]
    pub fn workspace(&self) -> &WorkItemWorkspace {
        &self.workspace
    }
    /// Frozen public MCP snapshot, never a runtime capability.
    #[must_use]
    pub fn mcp(&self) -> &FrozenOwnedFlowMcp {
        &self.mcp
    }
}
fn bounded_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_config::{McpServerRef, McpTransportConfig};
    use std::collections::HashMap;

    fn binding() -> WorkItemBinding {
        WorkItemBinding {
            item: "00000000000000000000000001".parse().unwrap(),
            revision: 1,
            requirements_hash: ContentHash::compute(b"accepted flow"),
            generation: 1,
        }
    }
    fn workspace() -> WorkItemWorkspace {
        let checkout = if cfg!(windows) {
            std::path::PathBuf::from(r"C:\repo")
        } else {
            std::path::PathBuf::from("/repo")
        };
        WorkItemWorkspace {
            repository: checkout.join(".git"),
            path: checkout.join(".worktrees/flow"),
            checkout,
            ownership: "flow-owner".into(),
            branch: "codex/flow".into(),
            base_commit: "a".repeat(40),
        }
    }
    fn manifest(mcp: FrozenOwnedFlowMcp) -> OwnedFlowInputsManifest {
        let request_identity = match &mcp {
            FrozenOwnedFlowMcp::Empty { .. } => OwnedFlowRequestIdentity::PlainPublic {
                digest: ContentHash::compute(b"public request"),
            },
            FrozenOwnedFlowMcp::Private { .. } => OwnedFlowRequestIdentity::Hmac {
                tag: HmacSha256Tag::from_bytes([3; 32]),
            },
        };
        OwnedFlowInputsManifest::new(
            "00000000000000000000000002".parse().unwrap(),
            request_identity,
            binding(),
            "00000000000000000000000003".parse().unwrap(),
            "00000000000000000000000004".parse().unwrap(),
            workspace(),
            mcp,
        )
        .unwrap()
    }
    fn entry(index: u32, name: &str, tools: Option<Vec<String>>) -> OwnedFlowMcpManifestEntry {
        let server = McpServerRef::new(
            name.into(),
            McpTransportConfig::stdio(
                "COMMAND_PRIVATE_SENTINEL".into(),
                vec!["ARGS_PRIVATE_SENTINEL".into()],
                HashMap::from([("key".into(), "ENV_PRIVATE_SENTINEL".into())]),
            ),
            tools,
            Duration::from_secs(17),
            false,
        )
        .with_sandbox(Some(SandboxMode::ReadOnly));
        OwnedFlowMcpManifestEntry::new(
            index,
            &server,
            OwnedFlowObjectRef::new(format!("{index:064x}")).unwrap(),
            HmacSha256Tag::from_bytes([5; 32]),
        )
        .unwrap()
    }
    fn private(entries: Vec<OwnedFlowMcpManifestEntry>) -> FrozenOwnedFlowMcp {
        FrozenOwnedFlowMcp::private(
            OwnedFlowMcpSelection::Explicit,
            entries,
            OwnedFlowObjectRef::new("e".repeat(64)).unwrap(),
            HmacSha256Tag::from_bytes([7; 32]),
        )
        .unwrap()
    }

    #[test]
    fn empty_selection_roundtrips_and_remains_distinct() {
        let explicit = manifest(FrozenOwnedFlowMcp::empty(OwnedFlowMcpSelection::Explicit));
        let defaults = manifest(FrozenOwnedFlowMcp::empty(
            OwnedFlowMcpSelection::HostDefault,
        ));
        assert_ne!(explicit, defaults);
        for accepted in [explicit, defaults] {
            let value = serde_json::to_value(&accepted).unwrap();
            assert_eq!(value["mcp"]["kind"], "empty");
            assert!(value["mcp"].get("envelope_ref").is_none());
            assert_eq!(
                serde_json::from_value::<OwnedFlowInputsManifest>(value).unwrap(),
                accepted
            );
        }
    }

    #[test]
    fn private_projection_preserves_public_policy_without_transport() {
        let accepted = manifest(private(vec![
            entry(0, "alpha", None),
            entry(1, "beta", Some(vec![])),
        ]));
        let encoded = serde_json::to_string(&accepted).unwrap();
        let debug = format!("{accepted:?}");
        for sentinel in [
            "COMMAND_PRIVATE_SENTINEL",
            "ARGS_PRIVATE_SENTINEL",
            "ENV_PRIVATE_SENTINEL",
        ] {
            assert!(!encoded.contains(sentinel));
            assert!(!debug.contains(sentinel));
        }
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert!(value["mcp"]["entries"][0]["allowed_tools"].is_null());
        assert_eq!(
            value["mcp"]["entries"][1]["allowed_tools"],
            serde_json::json!([])
        );
        assert_eq!(value["mcp"]["entries"][0]["call_timeout"], "17s");
        assert!(value["mcp"]["entries"][0].get("startup_timeout").is_none());
        assert_eq!(value["mcp"]["entries"][0]["restart_on_crash"], false);
        assert_eq!(value["mcp"]["entries"][0]["sandbox"], "read-only");
        assert!(value["mcp"]["entries"][0].get("transport").is_none());
        assert_eq!(
            serde_json::from_value::<OwnedFlowInputsManifest>(value).unwrap(),
            accepted
        );
    }

    #[test]
    fn startup_timeout_is_projected_only_when_explicit() {
        let server = McpServerRef::new(
            "slow".into(),
            McpTransportConfig::stdio("python3".into(), vec![], HashMap::new()),
            None,
            Duration::from_millis(275),
            false,
        );
        let reference = OwnedFlowObjectRef::new(format!("{:064x}", 0)).unwrap();
        let legacy = OwnedFlowMcpManifestEntry::new(
            0,
            &server,
            reference.clone(),
            HmacSha256Tag::from_bytes([5; 32]),
        )
        .unwrap();
        let legacy_value = serde_json::to_value(&legacy).unwrap();
        assert!(legacy_value.get("startup_timeout").is_none());
        let decoded: OwnedFlowMcpManifestEntry =
            serde_json::from_value(legacy_value.clone()).unwrap();
        assert_eq!(decoded.startup_timeout(), None);
        assert_eq!(serde_json::to_value(&decoded).unwrap(), legacy_value);

        let explicit = OwnedFlowMcpManifestEntry::new(
            0,
            &server.with_startup_timeout(Some(Duration::from_secs(20))),
            reference,
            HmacSha256Tag::from_bytes([5; 32]),
        )
        .unwrap();
        let value = serde_json::to_value(&explicit).unwrap();
        assert_eq!(value["startup_timeout"], "20s");
        let decoded: OwnedFlowMcpManifestEntry = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, explicit);
        assert_ne!(decoded, legacy);
    }

    #[test]
    fn malformed_or_mixed_snapshots_cannot_deserialize() {
        let accepted = manifest(FrozenOwnedFlowMcp::empty(OwnedFlowMcpSelection::Explicit));
        let mut mixed = serde_json::to_value(&accepted).unwrap();
        mixed["mcp"]["entries"] = serde_json::json!([]);
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(mixed).is_err());
        let mut forged = serde_json::to_value(&accepted).unwrap();
        forged["schema_version"] = serde_json::json!(2);
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(forged).is_err());
        let mut forged = serde_json::to_value(&accepted).unwrap();
        forged["binding"]["generation"] = serde_json::json!(0);
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(forged).is_err());
        let mut forged = serde_json::to_value(&accepted).unwrap();
        forged["request_identity"] =
            serde_json::json!({"kind":"hmac","tag": HmacSha256Tag::from_bytes([1;32])});
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(forged).is_err());
    }

    #[test]
    fn ordered_entries_reject_permutation_deletion_duplicates_and_empty_private() {
        let value = serde_json::to_value(manifest(private(vec![
            entry(0, "alpha", None),
            entry(1, "beta", None),
        ])))
        .unwrap();
        let mut reordered = value.clone();
        reordered["mcp"]["entries"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(reordered).is_err());
        let mut removed = value.clone();
        removed["mcp"]["entries"].as_array_mut().unwrap().remove(0);
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(removed).is_err());
        let mut duplicate = value.clone();
        duplicate["mcp"]["entries"][1]["name"] = serde_json::json!("alpha");
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(duplicate).is_err());
        let mut empty = value;
        empty["mcp"]["entries"] = serde_json::json!([]);
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(empty).is_err());
    }

    #[test]
    fn private_host_default_binds_unchanged_public_request_identity() {
        let mut value =
            serde_json::to_value(manifest(private(vec![entry(0, "alpha", None)]))).unwrap();
        value["request_identity"] = serde_json::json!({"kind":"plain_public","digest":ContentHash::compute(b"public request")});
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(value.clone()).is_err());
        value["mcp"]["selection"] = serde_json::json!("host_default");
        let accepted = serde_json::from_value::<OwnedFlowInputsManifest>(value).unwrap();
        assert_eq!(
            accepted.mcp().selection(),
            OwnedFlowMcpSelection::HostDefault
        );
    }

    #[test]
    fn opaque_references_and_hmac_tags_validate_without_echo() {
        for reference in ["../secret", "a", &"A".repeat(64)] {
            assert!(OwnedFlowObjectRef::new(reference.into()).is_err());
            assert!(
                serde_json::from_value::<OwnedFlowObjectRef>(serde_json::json!(reference)).is_err()
            );
        }
        assert!(
            serde_json::from_value::<HmacSha256Tag>(serde_json::json!(ContentHash::compute(
                b"secret"
            )))
            .is_err()
        );
        let tag = HmacSha256Tag::from_bytes([9; 32]);
        assert_eq!(
            serde_json::from_slice::<HmacSha256Tag>(&serde_json::to_vec(&tag).unwrap()).unwrap(),
            tag
        );
        assert!(!format!("{tag:?}").contains("090909"));
    }
}
