//! Authentication of frozen private MCP inputs. Only the host coordinator uses this.
use super::private_files::PrivateFiles;
use crate::work_items::{Result, WorkItemError};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use surge_core::{RunId, id::WorkItemOperationId, mcp_config::McpServerRef, work_item::*};
const KEY_NAME: &str = "host-key-v1";
const KEY_MARKER: &str = "host-key-established-v1";
const KEY_MAGIC: &[u8] = b"surge-owned-flow-host-key-v1\0";
const REQUEST_DOMAIN: &[u8] = b"surge/owned-flow/request/v1\0";
const SERVER_DOMAIN: &[u8] = b"surge/owned-flow/mcp-server/v1\0";
const ENVELOPE_DOMAIN: &[u8] = b"surge/owned-flow/mcp-envelope/v1\0";
type Hmac256 = Hmac<Sha256>;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InputBinding {
    operation: WorkItemOperationId,
    request_identity: OwnedFlowRequestIdentity,
    binding: WorkItemBinding,
    run: RunId,
    workspace_owner: RunId,
    workspace: WorkItemWorkspace,
}
impl InputBinding {
    pub(super) fn from_manifest(manifest: &OwnedFlowInputsManifest) -> Self {
        Self {
            operation: manifest.operation(),
            request_identity: manifest.request_identity().clone(),
            binding: manifest.binding().clone(),
            run: manifest.run(),
            workspace_owner: manifest.workspace_owner(),
            workspace: manifest.workspace().clone(),
        }
    }
    pub(super) fn new(
        operation: WorkItemOperationId,
        request_identity: OwnedFlowRequestIdentity,
        binding: WorkItemBinding,
        run: RunId,
        workspace_owner: RunId,
        workspace: WorkItemWorkspace,
    ) -> Self {
        Self {
            operation,
            request_identity,
            binding,
            run,
            workspace_owner,
            workspace,
        }
    }
    pub(super) fn manifest(&self, mcp: FrozenOwnedFlowMcp) -> Result<OwnedFlowInputsManifest> {
        OwnedFlowInputsManifest::new(
            self.operation,
            self.request_identity.clone(),
            self.binding.clone(),
            self.run,
            self.workspace_owner,
            self.workspace.clone(),
            mcp,
        )
        .map_err(|_| WorkItemError::PrivateInputsCorrupt)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerObject {
    schema_version: u32,
    binding: InputBinding,
    index: u32,
    object_ref: OwnedFlowObjectRef,
    server: McpServerRef,
}
#[derive(PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeObject {
    schema_version: u32,
    binding: InputBinding,
    selection: OwnedFlowMcpSelection,
    entries: Vec<OwnedFlowMcpManifestEntry>,
    envelope_ref: OwnedFlowObjectRef,
}

fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut value = serde_json::to_value(value).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    value.sort_all_objects();
    serde_json::to_vec(&value).map_err(|_| WorkItemError::PrivateInputsCorrupt)
}
fn authentication(key: &[u8; 32], domain: &[u8], bytes: &[u8]) -> Result<HmacSha256Tag> {
    let mut mac = Hmac256::new_from_slice(key).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    mac.update(domain);
    mac.update(bytes);
    Ok(HmacSha256Tag::from_bytes(
        mac.finalize().into_bytes().into(),
    ))
}
fn verify(key: &[u8; 32], domain: &[u8], bytes: &[u8], tag: &HmacSha256Tag) -> Result<()> {
    let mut mac = Hmac256::new_from_slice(key).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    mac.update(domain);
    mac.update(bytes);
    mac.verify_slice(tag.as_bytes())
        .map_err(|_| WorkItemError::PrivateInputsCorrupt)
}
fn random_reference() -> Result<OwnedFlowObjectRef> {
    OwnedFlowObjectRef::new(hex::encode(rand::random::<[u8; 32]>()))
        .map_err(|_| WorkItemError::PrivateInputsCorrupt)
}
fn object_name(reference: &OwnedFlowObjectRef) -> String {
    format!("{}.json", reference.as_str())
}
fn decode_key(bytes: &[u8]) -> Result<[u8; 32]> {
    if bytes.len() != KEY_MAGIC.len() + 64 || !bytes.starts_with(KEY_MAGIC) {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    let middle = KEY_MAGIC.len() + 32;
    if Sha256::digest(&bytes[..middle]).as_slice() != &bytes[middle..] {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    bytes[KEY_MAGIC.len()..middle]
        .try_into()
        .map_err(|_| WorkItemError::PrivateInputsRecoveryRequired)
}
fn host_key(files: &PrivateFiles, allow_new: bool) -> Result<[u8; 32]> {
    let bytes = match files.read(KEY_NAME) {
        Ok(bytes) => bytes,
        Err(WorkItemError::PrivateInputsRecoveryRequired) if allow_new => {
            if !files.is_empty()? {
                // A concurrent publisher may have won. Existing-only retry never
                // treats a marker, orphan object or corrupt key as an empty store.
                files.read(KEY_NAME)?
            } else {
                let mut bytes = KEY_MAGIC.to_vec();
                bytes.extend(rand::random::<[u8; 32]>());
                bytes.extend(Sha256::digest(&bytes));
                files.publish(KEY_NAME, &bytes)?;
                files.read(KEY_NAME)?
            }
        },
        Err(error) => return Err(error),
    };
    let key = decode_key(&bytes)?;
    // A returned first identity always has a durable established marker. Losing
    // the key afterwards can never make this namespace eligible for new keying.
    if allow_new {
        files.publish(KEY_MARKER, b"established-v1")?;
    }
    if files.read(KEY_MARKER)? != b"established-v1" {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    Ok(key)
}
pub(super) fn request_identity(
    home: &Path,
    canonical_body: &[u8],
    explicit_private: bool,
    allow_new_key: bool,
) -> Result<OwnedFlowRequestIdentity> {
    if !explicit_private {
        return Ok(OwnedFlowRequestIdentity::PlainPublic {
            digest: surge_core::ContentHash::compute(canonical_body),
        });
    }
    let files = PrivateFiles::open(home, allow_new_key)?;
    let key = host_key(&files, allow_new_key)?;
    Ok(OwnedFlowRequestIdentity::Hmac {
        tag: authentication(&key, REQUEST_DOMAIN, canonical_body)?,
    })
}
pub(super) fn verify_request(
    home: &Path,
    canonical_body: &[u8],
    identity: &OwnedFlowRequestIdentity,
) -> Result<()> {
    match identity {
        OwnedFlowRequestIdentity::PlainPublic { digest }
            if *digest == surge_core::ContentHash::compute(canonical_body) =>
        {
            Ok(())
        },
        OwnedFlowRequestIdentity::PlainPublic { .. } => Err(WorkItemError::Conflict(
            "owned Flow operation body changed".into(),
        )),
        OwnedFlowRequestIdentity::Hmac { tag } => {
            let files = PrivateFiles::open(home, false)?;
            let key = host_key(&files, false)?;
            verify(&key, REQUEST_DOMAIN, canonical_body, tag)
                .map_err(|_| WorkItemError::Conflict("owned Flow operation body changed".into()))
        },
    }
}
pub(super) fn freeze(
    home: &Path,
    binding: &InputBinding,
    selection: OwnedFlowMcpSelection,
    servers: &[McpServerRef],
    allow_new_key: bool,
) -> Result<OwnedFlowInputsManifest> {
    if servers.is_empty() {
        return binding.manifest(FrozenOwnedFlowMcp::empty(selection));
    }
    let files = PrivateFiles::open(home, true)?;
    let key = host_key(&files, allow_new_key)?;
    let mut entries = Vec::with_capacity(servers.len());
    for (index, server) in servers.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
        let reference = random_reference()?;
        let object = ServerObject {
            schema_version: 1,
            binding: binding.clone(),
            index,
            object_ref: reference.clone(),
            server: server.clone(),
        };
        let bytes = canonical(&object)?;
        let tag = authentication(&key, SERVER_DOMAIN, &bytes)?;
        let entry = OwnedFlowMcpManifestEntry::new(index, server, reference.clone(), tag)
            .map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
        if !files.publish(&object_name(&reference), &bytes)? {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        entries.push(entry);
    }
    let reference = random_reference()?;
    let envelope = EnvelopeObject {
        schema_version: 1,
        binding: binding.clone(),
        selection,
        entries: entries.clone(),
        envelope_ref: reference.clone(),
    };
    let bytes = canonical(&envelope)?;
    let tag = authentication(&key, ENVELOPE_DOMAIN, &bytes)?;
    if !files.publish(&object_name(&reference), &bytes)? {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    let snapshot = FrozenOwnedFlowMcp::private(selection, entries, reference, tag)
        .map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    binding.manifest(snapshot)
}
/// This only verifies private payloads. It grants no claim/effect authority; the
/// coordinator must validate actual accepted SQL and startup before minting one.
pub(super) fn hydrate(
    home: &Path,
    manifest: &OwnedFlowInputsManifest,
) -> Result<Vec<McpServerRef>> {
    let FrozenOwnedFlowMcp::Private {
        selection,
        entries,
        envelope_ref,
        envelope_auth_tag,
    } = manifest.mcp()
    else {
        return Ok(Vec::new());
    };
    let files = PrivateFiles::open(home, false)?;
    let key = host_key(&files, false)?;
    let binding = InputBinding::from_manifest(manifest);
    let expected = EnvelopeObject {
        schema_version: 1,
        binding: binding.clone(),
        selection: *selection,
        entries: entries.clone(),
        envelope_ref: envelope_ref.clone(),
    };
    let envelope_bytes = files.read(&object_name(envelope_ref))?;
    verify(&key, ENVELOPE_DOMAIN, &envelope_bytes, envelope_auth_tag)?;
    let envelope: EnvelopeObject =
        serde_json::from_slice(&envelope_bytes).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    if envelope != expected || canonical(&envelope)? != envelope_bytes {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    let mut servers = Vec::with_capacity(entries.len());
    for entry in entries {
        let bytes = files.read(&object_name(entry.object_ref()))?;
        verify(&key, SERVER_DOMAIN, &bytes, entry.object_auth_tag())?;
        let object: ServerObject =
            serde_json::from_slice(&bytes).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
        if object.schema_version != 1
            || object.binding != binding
            || object.index != entry.index()
            || &object.object_ref != entry.object_ref()
            || canonical(&object)? != bytes
        {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        let projection = OwnedFlowMcpManifestEntry::new(
            object.index,
            &object.server,
            object.object_ref.clone(),
            entry.object_auth_tag().clone(),
        )
        .map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
        if &projection != entry {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        servers.push(object.server);
    }
    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::{ContentHash, id::WorkItemId};
    fn binding(identity: OwnedFlowRequestIdentity) -> InputBinding {
        let checkout = if cfg!(windows) {
            std::path::PathBuf::from(r"C:\repo")
        } else {
            std::path::PathBuf::from("/repo")
        };
        InputBinding::new(
            WorkItemOperationId::new(),
            identity,
            WorkItemBinding {
                item: WorkItemId::new(),
                revision: 1,
                requirements_hash: ContentHash::compute(b"actual accepted contract"),
                generation: 1,
            },
            RunId::new(),
            RunId::new(),
            WorkItemWorkspace {
                repository: checkout.join(".git"),
                path: checkout.join(".worktrees/owned"),
                checkout,
                ownership: "original host owner".into(),
                branch: "codex/owned".into(),
                base_commit: "a".repeat(40),
            },
        )
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn server(name: &str) -> McpServerRef {
        use surge_core::{mcp_config::McpTransportConfig, sandbox::SandboxMode};
        McpServerRef::new(
            name.into(),
            McpTransportConfig::stdio(
                "/PRIVATE-COMMAND-SENTINEL".into(),
                vec!["PRIVATE-ARG-SENTINEL".into(), "second".into()],
                [("TOKEN".into(), "PRIVATE-ENV-SENTINEL".into())]
                    .into_iter()
                    .collect(),
            ),
            Some(vec!["tool_two".into(), "tool_one".into()]),
            std::time::Duration::from_secs(19),
            false,
        )
        .with_sandbox(Some(SandboxMode::WorkspaceWrite))
    }
    #[test]
    fn empty_snapshots_never_open_private_namespace_and_keep_selection() {
        let home = tempfile::tempdir().unwrap();
        let identity = request_identity(home.path(), b"public body", false, false).unwrap();
        let context = binding(identity);
        for selection in [
            OwnedFlowMcpSelection::Explicit,
            OwnedFlowMcpSelection::HostDefault,
        ] {
            let manifest = freeze(home.path(), &context, selection, &[], false).unwrap();
            assert_eq!(manifest.mcp().selection(), selection);
            assert!(hydrate(home.path(), &manifest).unwrap().is_empty());
        }
        assert!(!home.path().join("work-items").exists());
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn private_inputs_keep_actual_policy_and_transport_but_public_projection_is_opaque() {
        let home = tempfile::tempdir().unwrap();
        let identity =
            request_identity(home.path(), b"explicit private request", true, true).unwrap();
        verify_request(home.path(), b"explicit private request", &identity).unwrap();
        assert!(verify_request(home.path(), b"changed private request", &identity).is_err());
        let context = binding(identity);
        // One legacy-shaped entry (no startup deadline) and one explicit one:
        // both must survive the authenticated freeze/hydrate round trip.
        let servers = vec![
            server("first"),
            server("second").with_startup_timeout(Some(std::time::Duration::from_secs(20))),
        ];
        let manifest = freeze(
            home.path(),
            &context,
            OwnedFlowMcpSelection::Explicit,
            &servers,
            false,
        )
        .unwrap();
        assert_eq!(hydrate(home.path(), &manifest).unwrap(), servers);
        let projected = serde_json::to_value(&manifest).unwrap();
        assert!(
            projected["mcp"]["entries"][0]
                .get("startup_timeout")
                .is_none()
        );
        assert_eq!(projected["mcp"]["entries"][1]["startup_timeout"], "20s");
        let mut widened = projected.clone();
        widened["mcp"]["entries"][1]["startup_timeout"] = serde_json::json!("10m");
        let widened: OwnedFlowInputsManifest = serde_json::from_value(widened).unwrap();
        assert!(hydrate(home.path(), &widened).is_err());
        let public = serde_json::to_string(&manifest).unwrap();
        for sentinel in [
            "PRIVATE-COMMAND-SENTINEL",
            "PRIVATE-ARG-SENTINEL",
            "PRIVATE-ENV-SENTINEL",
        ] {
            assert!(!public.contains(sentinel));
        }
        let mut foreign = serde_json::to_value(&manifest).unwrap();
        foreign["run"] = serde_json::to_value(RunId::new()).unwrap();
        let foreign: OwnedFlowInputsManifest = serde_json::from_value(foreign).unwrap();
        assert!(hydrate(home.path(), &foreign).is_err());
        let mut reordered = serde_json::to_value(&manifest).unwrap();
        reordered["mcp"]["entries"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        assert!(serde_json::from_value::<OwnedFlowInputsManifest>(reordered).is_err());
        let key_path = home
            .path()
            .join("work-items/private-flow-inputs/host-key-v1");
        std::fs::remove_file(&key_path).unwrap();
        assert!(hydrate(home.path(), &manifest).is_err());
        assert!(request_identity(home.path(), b"new body", true, true).is_err());
        assert!(!key_path.exists());
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn partial_list_and_object_transplants_fail_whole_envelope_authentication() {
        let home = tempfile::tempdir().unwrap();
        let identity =
            request_identity(home.path(), b"public host defaults", false, false).unwrap();
        let context = binding(identity);
        let manifest = freeze(
            home.path(),
            &context,
            OwnedFlowMcpSelection::HostDefault,
            &[server("one"), server("two")],
            true,
        )
        .unwrap();
        let mut value = serde_json::to_value(&manifest).unwrap();
        value["mcp"]["entries"].as_array_mut().unwrap().pop();
        let partial: OwnedFlowInputsManifest = serde_json::from_value(value).unwrap();
        assert!(hydrate(home.path(), &partial).is_err());
        let FrozenOwnedFlowMcp::Private { entries, .. } = manifest.mcp() else {
            panic!("private expected")
        };
        let parent = home.path().join("work-items/private-flow-inputs");
        let first = parent.join(object_name(entries[0].object_ref()));
        let second = parent.join(object_name(entries[1].object_ref()));
        std::fs::write(&second, std::fs::read(first).unwrap()).unwrap();
        assert!(hydrate(home.path(), &manifest).is_err());
    }
}
