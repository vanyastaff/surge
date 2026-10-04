//! Immutable provisional operation identity and the short atomic acceptance boundary.
use super::{
    private_inputs::{self, InputBinding},
    source_snapshot,
};
use crate::work_items::start_preparation::secure_lock::{PreparationLock, PreparationLockKey};
use crate::work_items::{self, Result, WorkItemError, WorkItemStore};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};
use surge_core::{
    RunId,
    id::{WorkItemId, WorkItemOperationId, WorkItemProjectId},
    mcp_config::McpServerRef,
    work_item::*,
};

/// First host-resolved graph and original retained workspace. It is immutable.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedFlowSourceSnapshot {
    /// Validated accepted Flow contract, preserving the exact raw prompt.
    pub contract: AcceptedFlowContract,
    /// Canonical base that gave the selected graph its scoped first resolution.
    pub source_base: std::path::PathBuf,
    /// Host-created workspace association, never adopted from a caller directory.
    pub workspace: WorkItemWorkspace,
}
/// A receipt conveys accepted identity; only preparation conveys ownership.
pub enum OwnedFlowPreparationResult {
    /// Exact replay, with no new preparation or effect authority.
    Replay(OwnedFlowReceipt),
    /// Host ownership for the original provisional operation.
    Preparing(Box<OwnedFlowPreparation>),
}
/// Atomic acceptance together with the actual retained launch owner.
pub struct OwnedFlowAcceptance {
    receipt: OwnedFlowReceipt,
    claim: work_items::WorkItemLaunchClaim,
}
impl OwnedFlowAcceptance {
    /// Transfer the same owning lock to the daemon supervisor and engine.
    #[must_use]
    pub fn into_parts(self) -> (OwnedFlowReceipt, work_items::WorkItemLaunchClaim) {
        (self.receipt, self.claim)
    }
}
/// Nonserializable owning capability retained by the blocking preparation worker.
pub struct OwnedFlowPreparation {
    store: WorkItemStore,
    operation: WorkItemOperationId,
    identity: OwnedFlowRequestIdentity,
    item: WorkItemId,
    run: RunId,
    workspace_owner: RunId,
    operation_lock: PreparationLock,
    launch_lock: Option<Arc<PreparationLock>>,
    token: String,
    consumed: bool,
}
struct Operation {
    identity: OwnedFlowRequestIdentity,
    item: WorkItemId,
    run: RunId,
    workspace_owner: RunId,
    operation_lock_identity: String,
    launch_lock_identity: Option<String>,
    source: Option<OwnedFlowSourceSnapshot>,
    config: Option<String>,
    inputs: Option<OwnedFlowInputsManifest>,
    receipt: Option<OwnedFlowReceipt>,
}
fn read(conn: &Connection, operation: WorkItemOperationId) -> Result<Option<Operation>> {
    conn.query_row("SELECT request_identity,item,run,workspace_owner,operation_lock_identity,launch_lock_identity,source_snapshot,config_snapshot,inputs_manifest,receipt FROM owned_flow_operations WHERE operation=?",[operation.to_string()],|r| {
        let id = |index|->rusqlite::Result<String> { r.get(index) };
        let parse_error = |index,error:Box<dyn std::error::Error+Send+Sync>| rusqlite::Error::FromSqlConversionFailure(index,rusqlite::types::Type::Text,error);
        let source:Option<String>=r.get(6)?;
        let inputs:Option<String>=r.get(8)?;
        let receipt:Option<String>=r.get(9)?;
        Ok(Operation { identity:work_items::json(r.get(0)?)?,item:id(1)?.parse().map_err(|e|parse_error(1,Box::new(e)))?,run:id(2)?.parse().map_err(|e|parse_error(2,Box::new(e)))?,workspace_owner:id(3)?.parse().map_err(|e|parse_error(3,Box::new(e)))?,operation_lock_identity:r.get(4)?,launch_lock_identity:r.get(5)?,source:source.map(work_items::json).transpose()?,config:r.get(7)?,inputs:inputs.map(work_items::json).transpose()?,receipt:receipt.map(work_items::json).transpose()? })
    }).optional().map_err(WorkItemError::from)
}
impl WorkItemStore {
    /// Inspect exact replay before configuration, locator or discovery I/O.
    ///
    /// # Errors
    /// Refuses reused bodies and missing/corrupt existing host authentication keys.
    pub fn replay_owned_flow(
        &self,
        operation: WorkItemOperationId,
        canonical_body: &[u8],
        explicit_private: bool,
    ) -> Result<Option<OwnedFlowReceipt>> {
        let value = read(&*self.pool.get()?, operation)?;
        let Some(value) = value else {
            return Ok(None);
        };
        if matches!(value.identity, OwnedFlowRequestIdentity::Hmac { .. }) != explicit_private {
            return Err(WorkItemError::Conflict(
                "owned Flow operation body changed".into(),
            ));
        }
        private_inputs::verify_request(&self.home, canonical_body, &value.identity)?;
        Ok(value.receipt)
    }
    /// Claim the original operation before resolving sources or provisioning Git.
    ///
    /// # Errors
    /// Refuses body conflicts, replaced ownership objects and concurrent owners.
    pub fn begin_owned_flow(
        &self,
        operation: WorkItemOperationId,
        canonical_body: &[u8],
        explicit_private: bool,
        now: i64,
    ) -> Result<OwnedFlowPreparationResult> {
        if operation == WorkItemOperationId::nil() {
            return Err(WorkItemError::Invalid("nil owned Flow operation".into()));
        }
        if let Some(receipt) =
            self.replay_owned_flow(operation, canonical_body, explicit_private)?
        {
            return Ok(OwnedFlowPreparationResult::Replay(receipt));
        }
        let operation_lock = if explicit_private {
            PreparationLock::acquire_private(
                &self.home,
                PreparationLockKey::FlowOperation(operation),
            )?
        } else {
            PreparationLock::acquire_stable(
                &self.home,
                PreparationLockKey::FlowOperation(operation),
            )?
        };
        let lock_identity = operation_lock.identity()?;
        let existing = read(&*self.pool.get()?, operation)?;
        let identity = if let Some(existing) = &existing {
            if existing.operation_lock_identity != lock_identity
                || matches!(existing.identity, OwnedFlowRequestIdentity::Hmac { .. })
                    != explicit_private
            {
                return Err(WorkItemError::Conflict(
                    "owned Flow operation ownership changed".into(),
                ));
            }
            private_inputs::verify_request(&self.home, canonical_body, &existing.identity)?;
            existing.identity.clone()
        } else {
            let allow_new = self.private_key_creation_allowed()?;
            private_inputs::request_identity(
                &self.home,
                canonical_body,
                explicit_private,
                allow_new,
            )?
        };
        let token = RunId::new().to_string();
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = if let Some(value) = read(&tx, operation)? {
            if value.identity != identity || value.operation_lock_identity != lock_identity {
                return Err(WorkItemError::Conflict(
                    "owned Flow first identity changed".into(),
                ));
            }
            if let Some(receipt) = value.receipt {
                return Ok(OwnedFlowPreparationResult::Replay(receipt));
            }
            tx.execute("UPDATE owned_flow_operations SET token=?,state='preparing' WHERE operation=? AND receipt IS NULL",params![token,operation.to_string()])?;
            value
        } else {
            let value = Operation {
                identity: identity.clone(),
                item: WorkItemId::new(),
                run: RunId::new(),
                workspace_owner: RunId::new(),
                operation_lock_identity: lock_identity.clone(),
                launch_lock_identity: None,
                source: None,
                config: None,
                inputs: None,
                receipt: None,
            };
            tx.execute("INSERT INTO owned_flow_operations(operation,request_identity,item,run,workspace_owner,operation_lock_identity,token,state,created_at_ms) VALUES(?,?,?,?,?,?,?,'preparing',?)",params![operation.to_string(),serde_json::to_string(&identity)?,value.item.to_string(),value.run.to_string(),value.workspace_owner.to_string(),lock_identity,token,now])?;
            value
        };
        tx.commit()?;
        Ok(OwnedFlowPreparationResult::Preparing(Box::new(
            OwnedFlowPreparation {
                store: self.clone(),
                operation,
                identity,
                item: value.item,
                run: value.run,
                workspace_owner: value.workspace_owner,
                operation_lock,
                launch_lock: None,
                token,
                consumed: false,
            },
        )))
    }
    fn private_key_creation_allowed(&self) -> Result<bool> {
        let conn = self.pool.get()?;
        let references:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM owned_flow_operations WHERE json_extract(request_identity,'$.kind')='hmac' OR json_extract(inputs_manifest,'$.mcp.kind')='private')",[],|r|r.get(0))?;
        Ok(!references)
    }
}
impl OwnedFlowPreparation {
    /// Stable host-allocated item, retained even if preparation is abandoned.
    #[must_use]
    pub fn item(&self) -> WorkItemId {
        self.item
    }
    /// Stable host-allocated run.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }
    /// Original Git creation owner, independent of current attempt claim tokens.
    #[must_use]
    pub fn workspace_owner(&self) -> RunId {
        self.workspace_owner
    }
    fn verify(&self, conn: &Connection) -> Result<Operation> {
        self.operation_lock.verify()?;
        let value = read(conn, self.operation)?.ok_or(WorkItemError::NotFound)?;
        let owned:bool=conn.query_row("SELECT token=? AND state='preparing' AND receipt IS NULL FROM owned_flow_operations WHERE operation=?",params![self.token,self.operation.to_string()],|r|r.get(0))?;
        if !owned
            || value.identity != self.identity
            || value.item != self.item
            || value.run != self.run
            || value.workspace_owner != self.workspace_owner
            || value.operation_lock_identity != self.operation_lock.identity()?
        {
            return Err(WorkItemError::Conflict(
                "owned Flow preparation changed".into(),
            ));
        }
        Ok(value)
    }
    /// Read the original graph/workspace first; replay never reopens its locator.
    pub fn source_snapshot(&self) -> Result<Option<OwnedFlowSourceSnapshot>> {
        Ok(self.verify(&*self.store.pool.get()?)?.source)
    }
    /// Frozen public startup is reused before consulting current host settings.
    pub fn startup_snapshot(&self) -> Result<Option<(String, OwnedFlowInputsManifest)>> {
        let value = self.verify(&*self.store.pool.get()?)?;
        match (value.config, value.inputs) {
            (Some(config), Some(inputs)) => Ok(Some((config, inputs))),
            (None, None) => Ok(None),
            _ => Err(WorkItemError::PrivateInputsCorrupt),
        }
    }
    /// Capture one scoped, bounded graph with retained no-follow source handles.
    pub fn capture_source(
        &self,
        base: &Path,
        locator: &Path,
    ) -> Result<super::OwnedFlowCapturedSource> {
        self.operation_lock.verify()?;
        let captured = source_snapshot::capture_flow_source(base, locator)?;
        captured.verify()?;
        Ok(captured)
    }
    /// Save the first resolved graph and Git intent; subsequent calls must be exact.
    pub fn freeze_source(
        &self,
        snapshot: &OwnedFlowSourceSnapshot,
        captured: Option<&super::OwnedFlowCapturedSource>,
    ) -> Result<()> {
        if let Some(captured) = captured {
            captured.verify()?;
            if captured.graph() != snapshot.contract.graph()
                || captured.base_path() != snapshot.source_base
            {
                return Err(WorkItemError::Conflict(
                    "owned Flow captured source changed".into(),
                ));
            }
        }
        let mut conn = self.store.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = self.verify(&tx)?;
        if let Some(source) = value.source {
            if source != *snapshot {
                return Err(WorkItemError::Conflict(
                    "owned Flow source snapshot changed".into(),
                ));
            }
        } else {
            snapshot
                .contract
                .validate_graph(snapshot.contract.graph())
                .map_err(|_| WorkItemError::Invalid("invalid accepted Flow graph".into()))?;
            tx.execute("UPDATE owned_flow_operations SET source_snapshot=? WHERE operation=? AND token=? AND source_snapshot IS NULL",params![serde_json::to_string(snapshot)?,self.operation.to_string(),self.token])?;
        }
        if let Some(captured) = captured {
            captured.verify()?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Capture the actual effective MCP list once, and persist only its public projection.
    pub fn freeze_startup(
        &mut self,
        public_config: &str,
        selection: OwnedFlowMcpSelection,
        servers: &[McpServerRef],
    ) -> Result<OwnedFlowInputsManifest> {
        let value = self.verify(&*self.store.pool.get()?)?;
        let source = value
            .source
            .ok_or_else(|| WorkItemError::Invalid("owned Flow source missing".into()))?;
        if let (Some(config), Some(inputs)) = (value.config, value.inputs) {
            if config != public_config || inputs.mcp().selection() != selection {
                return Err(WorkItemError::Conflict(
                    "owned Flow startup snapshot changed".into(),
                ));
            }
            return Ok(inputs);
        }
        let config: serde_json::Value = serde_json::from_str(public_config)
            .map_err(|_| WorkItemError::Invalid("invalid owned Flow public config".into()))?;
        if config
            .get("mcp_servers")
            .is_some_and(|v| v.as_array().is_none_or(|v| !v.is_empty()))
        {
            return Err(WorkItemError::Invalid(
                "owned Flow config is not public".into(),
            ));
        }
        if !servers.is_empty() {
            self.operation_lock.require_private()?;
        }
        let binding = WorkItemBinding {
            item: self.item,
            revision: 1,
            requirements_hash: source.contract.hash()?,
            generation: 1,
        };
        let context = InputBinding::new(
            self.operation,
            self.identity.clone(),
            binding,
            self.run,
            self.workspace_owner,
            source.workspace,
        );
        let inputs = private_inputs::freeze(
            &self.store.home,
            &context,
            selection,
            servers,
            self.store.private_key_creation_allowed()?,
        )?;
        let mut conn = self.store.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = self.verify(&tx)?;
        if value.config.is_some() || value.inputs.is_some() {
            return Err(WorkItemError::Conflict(
                "owned Flow startup already frozen".into(),
            ));
        }
        tx.execute("UPDATE owned_flow_operations SET config_snapshot=?,inputs_manifest=? WHERE operation=? AND token=?",params![public_config,serde_json::to_string(&inputs)?,self.operation.to_string(),self.token])?;
        tx.commit()?;
        Ok(inputs)
    }
    /// Capture and retain the original launch object's identity before acceptance.
    pub fn retain_launch_ownership(&mut self) -> Result<()> {
        if self.launch_lock.is_some() {
            return Ok(());
        }
        let lock = PreparationLock::acquire_stable(
            &self.store.home,
            PreparationLockKey::FlowLaunch {
                operation: self.operation,
                run: self.run,
            },
        )?;
        let identity = lock.identity()?;
        let mut conn = self.store.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = self.verify(&tx)?;
        if let Some(original) = value.launch_lock_identity {
            if original != identity {
                return Err(WorkItemError::Conflict(
                    "owned Flow launch object changed".into(),
                ));
            }
        } else {
            tx.execute("UPDATE owned_flow_operations SET launch_lock_identity=? WHERE operation=? AND token=? AND launch_lock_identity IS NULL",params![identity,self.operation.to_string(),self.token])?;
        }
        tx.commit()?;
        self.launch_lock = Some(Arc::new(lock));
        Ok(())
    }
    /// Accept the item, revision, Reserved attempt, launch intent and receipt together.
    /// The awaiting host consumes this after its worker returns; canceled callers
    /// only abandon preparation while that worker still owns both guards.
    pub fn finalize(mut self, now: i64) -> Result<OwnedFlowAcceptance> {
        let launch = self
            .launch_lock
            .as_ref()
            .ok_or_else(|| WorkItemError::Invalid("owned Flow launch ownership missing".into()))?;
        launch.verify()?;
        let mut conn = self.store.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = self.verify(&tx)?;
        if value.launch_lock_identity.as_deref() != Some(&launch.identity()?) {
            return Err(WorkItemError::Conflict(
                "owned Flow launch identity changed".into(),
            ));
        }
        let source = value
            .source
            .ok_or_else(|| WorkItemError::Invalid("owned Flow source snapshot missing".into()))?;
        let config = value
            .config
            .ok_or_else(|| WorkItemError::Invalid("owned Flow configuration missing".into()))?;
        let inputs = value
            .inputs
            .ok_or_else(|| WorkItemError::Invalid("owned Flow inputs missing".into()))?;
        let repository = source.workspace.repository.to_string_lossy();
        let project: Option<String> = tx
            .query_row(
                "SELECT id FROM work_item_projects WHERE repository=?",
                [repository.as_ref()],
                |r| r.get(0),
            )
            .optional()?;
        let project = project.map_or_else(
            || Ok(WorkItemProjectId::new()),
            |id| {
                id.parse()
                    .map_err(|_| WorkItemError::Invalid("invalid project identity".into()))
            },
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO work_item_projects(id,repository,checkout) VALUES(?,?,?)",
            params![
                project.to_string(),
                repository.as_ref(),
                source.workspace.checkout.to_string_lossy()
            ],
        )?;
        let title = &source.contract.graph().metadata.name;
        if title.trim().is_empty() || title.len() > 1024 {
            return Err(WorkItemError::Invalid("invalid Flow title".into()));
        }
        tx.execute("INSERT INTO work_items(id,project_id,title,accepted_revision,version,workspace,workspace_prepared) VALUES(?,?,?,1,1,?,1)",params![self.item.to_string(),project.to_string(),title,serde_json::to_string(&source.workspace)?])?;
        let accepted = WorkItemRevision {
            revision: 1,
            hash: source.contract.hash()?,
            origin: AcceptedWorkItemOrigin::Flow(source.contract.clone()),
            actor: "host".into(),
            accepted_proposal: None,
            accepted_at_ms: now,
        };
        tx.execute(
            "INSERT INTO work_item_revisions(item,revision,payload) VALUES(?,1,?)",
            params![self.item.to_string(), serde_json::to_string(&accepted)?],
        )?;
        self.store.reserve_with_host_run(
            &tx,
            self.item,
            1,
            source.contract.graph(),
            &config,
            self.run,
        )?;
        let binding = work_items::attempt(&tx, self.run)?.binding;
        if inputs.binding() != &binding
            || inputs.run() != self.run
            || inputs.operation() != self.operation
            || inputs.workspace() != &source.workspace
            || inputs.workspace_owner() != self.workspace_owner
        {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        let receipt = OwnedFlowReceipt {
            operation_id: self.operation,
            item: self.item,
            run: self.run,
            binding,
            workspace_owner: self.workspace_owner,
            accepted_at_ms: now,
        };
        let token = RunId::new().to_string();
        tx.execute(
            "UPDATE work_item_attempts SET claim_token=? WHERE run=? AND generation=?",
            params![token, self.run.to_string(), receipt.binding.generation],
        )?;
        tx.execute("UPDATE owned_flow_operations SET state='accepted',receipt=?,intent_state='pending' WHERE operation=? AND token=? AND state='preparing'",params![serde_json::to_string(&receipt)?,self.operation.to_string(),self.token])?;
        tx.commit()?;
        let claim = work_items::WorkItemLaunchClaim {
            lock: work_items::LaunchLock::OwnedFlow {
                guard: Arc::clone(launch),
                operation: self.operation,
                identity: launch.identity()?,
            },
            run: self.run,
            token,
            binding: receipt.binding.clone(),
        };
        self.consumed = true;
        Ok(OwnedFlowAcceptance { receipt, claim })
    }
}
impl Drop for OwnedFlowPreparation {
    fn drop(&mut self) {
        if !self.consumed
            && let Ok(conn) = self.store.pool.get()
            && let Err(error) = conn.execute(
                "UPDATE owned_flow_operations SET state='abandoned' WHERE operation=? AND token=? AND state='preparing' AND receipt IS NULL",
                params![self.operation.to_string(), self.token],
            )
        {
            tracing::warn!(%error, "could not abandon owned Flow preparation");
        }
    }
}
