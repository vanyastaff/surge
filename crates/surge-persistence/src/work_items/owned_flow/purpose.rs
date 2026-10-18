//! Host-storage requirements selected before any startup manifest exists.
use super::{
    private_establishment::{self, PrivateOwner},
    private_inputs,
};
use crate::runs::bootstrap_operations::{self, BootstrapStoreError, BootstrapStoredPayload};
use crate::work_items::start_preparation::secure_lock::PreparationLock;
use crate::work_items::{Result, WorkItemError, WorkItemStore};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use surge_core::work_item::ContinuationInputRequirement;
use surge_core::{
    ContentHash, RunId,
    id::WorkItemOperationId,
    work_item::{OwnedFlowInputPurpose, OwnedFlowInputsManifest, OwnedFlowRequestIdentity},
};

const REQUEST_DOMAIN: &[u8] = b"surge/owned-flow/continuation-request/v1\0";

/// Whether an ordinary request body contains private transport input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrdinaryRequestPrivacy {
    /// The canonical request body contains no private transport input.
    Public,
    /// The canonical request body contains private transport input.
    Private,
}

/// Immutable host-storage purpose; this value grants no execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OwnedFlowPreparationPurpose {
    /// Ordinary request identity and MCP rules remain unchanged.
    Ordinary(OrdinaryRequestPrivacy),
    /// Private seed storage is required even with an empty effective MCP list.
    Continuation(ContinuationInputRequirement),
}
impl OwnedFlowPreparationPurpose {
    pub(super) fn private(&self) -> bool {
        !matches!(self, Self::Ordinary(OrdinaryRequestPrivacy::Public))
    }
    pub(super) fn source(&self) -> Option<&ContinuationInputRequirement> {
        match self {
            Self::Ordinary(_) => None,
            Self::Continuation(source) => Some(source),
        }
    }
}
fn request_bytes(body: &[u8], source: &ContinuationInputRequirement) -> Result<Vec<u8>> {
    private_inputs::canonical(&serde_json::json!({
        "schema_version": 1,
        "purpose": OwnedFlowInputPurpose::Continuation { source: source.clone() },
        "body_digest": ContentHash::compute(body),
    }))
}
pub(super) fn identity(
    store: &WorkItemStore,
    body: &[u8],
    purpose: &OwnedFlowPreparationPurpose,
) -> Result<(OwnedFlowRequestIdentity, Option<Arc<PrivateOwner>>)> {
    let Some(source) = purpose.source() else {
        return private_inputs::request_identity(store, body, purpose.private());
    };
    let owner = private_establishment::open(store, None, None)?;
    let tag =
        private_inputs::authentication(owner.key(), REQUEST_DOMAIN, &request_bytes(body, source)?)?;
    owner.verify()?;
    Ok((OwnedFlowRequestIdentity::Hmac { tag }, Some(owner)))
}
pub(super) fn verify_request(
    store: &WorkItemStore,
    operation: WorkItemOperationId,
    body: &[u8],
    identity: &OwnedFlowRequestIdentity,
    manifest: Option<&OwnedFlowInputsManifest>,
    lock: Option<&PreparationLock>,
    purpose: &OwnedFlowPreparationPurpose,
) -> Result<Option<Arc<PrivateOwner>>> {
    let Some(source) = purpose.source() else {
        return private_inputs::verify_request(store, operation, body, identity, manifest, lock);
    };
    if manifest.is_some_and(|value| {
        value.purpose()
            != &OwnedFlowInputPurpose::Continuation {
                source: source.clone(),
            }
    }) {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    let OwnedFlowRequestIdentity::Hmac { tag } = identity else {
        return Err(WorkItemError::PrivateInputsCorrupt);
    };
    let owner = private_establishment::open(store, None, lock)?;
    private_inputs::verify(
        owner.key(),
        REQUEST_DOMAIN,
        &request_bytes(body, source)?,
        tag,
    )
    .map_err(|error| match error {
        WorkItemError::PrivateInputsCorrupt => {
            WorkItemError::Conflict("owned Flow operation body changed".into())
        },
        other => other,
    })?;
    super::seed_objects::inspect_bound(store, operation, owner.clone())?;
    owner.verify()?;
    Ok(Some(owner))
}
struct SourceAssociation {
    planning: RunId,
    implementation: RunId,
    intent: ContentHash,
    capture: ContentHash,
}
fn bootstrap_error(error: BootstrapStoreError) -> WorkItemError {
    match error {
        BootstrapStoreError::Sqlite(error) => WorkItemError::Sql(error),
        BootstrapStoreError::NotFound => WorkItemError::NotFound,
        _ => WorkItemError::IndependentAcceptedDataInvalid,
    }
}
fn resolve_source(
    conn: &Connection,
    source: &ContinuationInputRequirement,
) -> Result<SourceAssociation> {
    let record = bootstrap_operations::read(conn, source.source_bootstrap())
        .map_err(bootstrap_error)?
        .ok_or(WorkItemError::NotFound)?;
    if !matches!(record.payload, BootstrapStoredPayload::V1 { .. })
        || record.status.implementation_run != source.source_run()
    {
        return Err(WorkItemError::IndependentAcceptedDataInvalid);
    }
    let value = SourceAssociation {
        planning: record.status.planning_run,
        implementation: record.status.implementation_run,
        intent: record.intent_fingerprint,
        capture: record.capture_fingerprint,
    };
    for run in [value.planning, value.implementation] {
        let registered: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?)",
            [run.to_string()],
            |row| row.get(0),
        )?;
        if !registered {
            return Err(WorkItemError::NotFound);
        }
        let deleted: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM run_host_tombstones WHERE run_id=?)",
            [run.to_string()],
            |row| row.get(0),
        )?;
        if deleted {
            return Err(WorkItemError::Conflict(
                "continuation source deletion already admitted".into(),
            ));
        }
    }
    Ok(value)
}
pub(super) fn reserve(
    conn: &Connection,
    operation: WorkItemOperationId,
    purpose: &OwnedFlowPreparationPurpose,
    establishment_nonce: Option<&str>,
) -> Result<()> {
    let Some(source) = purpose.source() else {
        return Ok(());
    };
    let value = resolve_source(conn, source)?;
    let nonce = establishment_nonce.ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
    conn.execute("INSERT INTO owned_flow_continuation_requirements(operation,source_bootstrap,planning_run,source_run,intent_fingerprint,capture_fingerprint,establishment_nonce) VALUES(?,?,?,?,?,?,?)",
        params![operation.to_string(), source.source_bootstrap().to_string(),value.planning.to_string(),value.implementation.to_string(),value.intent.to_string(),value.capture.to_string(),nonce])?;
    for (role, run) in [
        ("planning", value.planning),
        ("implementation", value.implementation),
    ] {
        conn.execute(
            "INSERT INTO owned_flow_source_pins(operation,source_run,role) VALUES(?,?,?)",
            params![operation.to_string(), run.to_string(), role],
        )?;
    }
    #[cfg(test)]
    super::continuation_tests::reservation_boundary(operation)?;
    Ok(())
}
pub(super) fn decode(
    conn: &Connection,
    operation: WorkItemOperationId,
    identity: &OwnedFlowRequestIdentity,
    encoded: Option<&str>,
) -> Result<OwnedFlowPreparationPurpose> {
    let purpose = match encoded {
        Some(value) => {
            serde_json::from_str(value).map_err(|_| WorkItemError::PrivateInputsCorrupt)?
        },
        None => OwnedFlowPreparationPurpose::Ordinary(
            if matches!(identity, OwnedFlowRequestIdentity::Hmac { .. }) {
                OrdinaryRequestPrivacy::Private
            } else {
                OrdinaryRequestPrivacy::Public
            },
        ),
    };
    let association: Option<(String,String,String,String,String,String)> = conn.query_row(
        "SELECT source_bootstrap,planning_run,source_run,intent_fingerprint,capture_fingerprint,establishment_nonce FROM owned_flow_continuation_requirements WHERE operation=?",
        [operation.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    match (purpose.source(), association) {
        (None, None) => {
            let dependent:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM owned_flow_source_pins WHERE operation=? UNION ALL SELECT 1 FROM owned_flow_private_object_pins WHERE operation=?)",
                params![operation.to_string(),operation.to_string()],|r|r.get(0))?;
            if dependent {
                return Err(WorkItemError::PrivateInputsCorrupt);
            }
        },
        (Some(source), Some(row)) => {
            let expected = resolve_source(conn, source)?;
            if row.0 != source.source_bootstrap().to_string()
                || row.1 != expected.planning.to_string()
                || row.2 != expected.implementation.to_string()
                || row.3 != expected.intent.to_string()
                || row.4 != expected.capture.to_string()
                || row.5.is_empty()
            {
                return Err(WorkItemError::PrivateInputsCorrupt);
            }
            super::seed_objects::validate_metadata(conn, operation, &row.5)?;
            let mut query=conn.prepare("SELECT role,source_run FROM owned_flow_source_pins WHERE operation=? ORDER BY role")?;
            let pins: Vec<(String, String)> = query
                .query_map([operation.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?;
            if pins
                != vec![
                    ("implementation".into(), expected.implementation.to_string()),
                    ("planning".into(), expected.planning.to_string()),
                ]
            {
                return Err(WorkItemError::PrivateInputsCorrupt);
            }
        },
        _ => return Err(WorkItemError::PrivateInputsCorrupt),
    }
    if purpose.private() != matches!(identity, OwnedFlowRequestIdentity::Hmac { .. }) {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    Ok(purpose)
}

fn required_operation(conn: &Connection, run: RunId) -> Result<Option<WorkItemOperationId>> {
    let row:Option<(String,String,Option<String>)>=conn.query_row(
        "SELECT operation,request_identity,input_purpose FROM owned_flow_operations WHERE run=?",
        [run.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    let Some((operation, identity, encoded)) = row else {
        return Ok(None);
    };
    let operation = operation
        .parse()
        .map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    let identity =
        serde_json::from_str(&identity).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    Ok(decode(conn, operation, &identity, encoded.as_deref())?
        .source()
        .map(|_| operation))
}
/// Independent registry requirement is read even when startup metadata is absent.
pub(in crate::work_items) fn refuse_execution(store: &WorkItemStore, run: RunId) -> Result<()> {
    let required = {
        let conn = store.pool.get()?;
        required_operation(&conn, run)?
    };
    let Some(operation) = required else {
        return Ok(());
    };
    let owner = private_establishment::open(store, None, None)?;
    super::seed_objects::require_sealed(store, operation, owner)?;
    Err(WorkItemError::ContinuationExecutionUnavailable)
}
/// Retained claims use their actual supplied transaction and owner, never another checkout.
pub(super) fn refuse_execution_in(
    conn: &Connection,
    run: RunId,
    owner: Option<&Arc<PrivateOwner>>,
) -> Result<()> {
    let Some(operation) = required_operation(conn, run)? else {
        return Ok(());
    };
    let owner = owner.ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
    owner.verify_in(conn)?;
    let stored = super::seed_objects::load(conn, operation, owner)?
        .ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
    if stored.manifest.is_none() {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    // Public entry points authenticate private bytes before acquiring their transaction.
    // This supplied-connection fence can only refuse execution, never expose seed contents.
    Err(WorkItemError::ContinuationExecutionUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuation_request_preimage_and_hmac_match_independent_fixed_wire() {
        let source = ContinuationInputRequirement::new(
            "run-00000000000000000000000005".parse().unwrap(),
            "run-00000000000000000000000004".parse().unwrap(),
        )
        .unwrap();
        let expected=br#"{"body_digest":"sha256:25d523a80604b988eb02dad7a4a095441d99723c9bc885c94590ff6356bcf106","purpose":{"kind":"continuation","source":{"source_bootstrap":"00000000000000000000000005","source_run":"00000000000000000000000004"}},"schema_version":1}"#;
        let actual = request_bytes(b"fixed continuation body", &source).unwrap();
        assert_eq!(actual, expected);
        let tag = private_inputs::authentication(&[7; 32], REQUEST_DOMAIN, &actual).unwrap();
        assert_eq!(
            String::from(tag),
            "hmac-sha256:5c13e5ee45e4141c087027516fbf2a6977182c6a63fe1aa533de6e46771bea85"
        );
    }
}
